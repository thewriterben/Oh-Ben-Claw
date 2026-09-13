//! The host verifies the base station's frames — on the bench, on the real
//! path. SPINE-AUTH.md §3.4, last bullet.
//!
//! Everything else that proves `LoraAuth` runs on synthetic lines. This runs
//! the production pieces end to end — `open_split` on the base station's
//! console, `run_gateway_rx` with a `LoraAuth` built from the same root the
//! stations were flashed with, facts landing in world memory — and then reads
//! the `spine.auth.gw-XX` fact back. Ignored by default: it needs a base
//! station on a port and the deployment root.
//!
//! ```powershell
//! $env:OBC_BASE_PORT = 'COM3'
//! $env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
//! cargo test --features hardware --test lora_gateway_live -- --ignored --nocapture
//! ```
//!
//! Two tests, run in sequence (one port). The first wants every frame the
//! base reports in 40 s to verify and land; the second, with a root the
//! stations do not have, wants every one of them refused as a bad tag and
//! nothing in world memory. The second is the one that says the first meant
//! something.

#![cfg(feature = "hardware")]

use std::sync::Arc;
use std::time::Duration;

use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::spine::lora_gateway::{open_split, run_gateway_rx, LoraAuth, AUTH_FACT_PREFIX};

const LISTEN: Duration = Duration::from_secs(40);

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set — see the file header"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Run the production RX path for `LISTEN` under `root`, and return every
/// `spine.auth.*` fact plus every `mesh.*` key that landed.
async fn listen(root: &str) -> (Vec<serde_json::Value>, Vec<String>) {
    let port = env("OBC_BASE_PORT");
    let auth = LoraAuth::new(root).expect("root");
    println!(
        "host root fingerprint {:04x} — compare with the stations' boot logs",
        auth.fingerprint()
    );
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    // The previous test's serial thread releases the port only when its next
    // line fails to send (the receiver is gone) — up to one keepalive later.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let (rx, _tx) = loop {
        match open_split(&port, 115_200) {
            Ok(pair) => break pair,
            Err(e) if std::time::Instant::now() < deadline => {
                println!("port busy ({e:#}); retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => panic!("base station console: {e:#}"),
        }
    };
    let w = Arc::clone(&world);
    let task = tokio::spawn(async move {
        let mut auth = auth;
        run_gateway_rx(rx, &mut auth, w, now_ms).await
    });
    tokio::time::sleep(LISTEN).await;
    task.abort();

    let mut auth_facts = Vec::new();
    let mut mesh_keys = Vec::new();
    for src in 0u8..=255 {
        let key = format!("{AUTH_FACT_PREFIX}gw-{src:02X}");
        if let Ok(Some(f)) = world.current(&key) {
            println!("{key}: {}", f.value);
            auth_facts.push(f.value);
        }
    }
    for key in ["mesh.gw-40", "mesh.gw-D8", "mesh.obc-esp32-s3-001"] {
        if let Ok(Some(_)) = world.current(key) {
            mesh_keys.push(key.to_string());
        }
    }
    (auth_facts, mesh_keys)
}

#[tokio::test]
#[ignore = "needs the base station on OBC_BASE_PORT and OBC_SPINE_ROOT"]
async fn every_frame_the_base_reports_verifies_on_the_host_and_lands() {
    let (facts, mesh) = listen(&env("OBC_SPINE_ROOT")).await;
    assert!(!facts.is_empty(), "no station was heard in {LISTEN:?}");
    let accepted: u64 = facts
        .iter()
        .map(|f| f["accepted"].as_u64().unwrap_or(0))
        .sum();
    let rejected: u64 = facts
        .iter()
        .map(|f| f["rejected"].as_u64().unwrap_or(0))
        .sum();
    println!("accepted {accepted}, rejected {rejected}, mesh facts {mesh:?}");
    assert!(
        accepted >= 3,
        "only {accepted} frames verified in {LISTEN:?}"
    );
    assert_eq!(
        rejected, 0,
        "the host refused frames the stations signed: {facts:?}"
    );
    assert!(
        !mesh.is_empty(),
        "verified frames did not reach world memory"
    );
}

#[tokio::test]
#[ignore = "needs the base station on OBC_BASE_PORT; run after the positive test"]
async fn under_a_root_the_stations_do_not_have_nothing_lands() {
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let (facts, mesh) = listen(wrong).await;
    assert!(!facts.is_empty(), "no station was heard in {LISTEN:?}");
    let accepted: u64 = facts
        .iter()
        .map(|f| f["accepted"].as_u64().unwrap_or(0))
        .sum();
    let rejected: u64 = facts
        .iter()
        .map(|f| f["rejected"].as_u64().unwrap_or(0))
        .sum();
    println!("accepted {accepted}, rejected {rejected}, mesh facts {mesh:?}");
    assert_eq!(accepted, 0, "a frame verified under the wrong root");
    assert!(rejected >= 3, "only {rejected} rejections in {LISTEN:?}");
    for f in &facts {
        let reason = f["last_rejected"]["reason"].as_str().unwrap_or("");
        assert!(reason.starts_with("bad tag"), "unexpected reason: {reason}");
    }
    assert!(
        mesh.is_empty(),
        "something reached world memory unverified: {mesh:?}"
    );
}

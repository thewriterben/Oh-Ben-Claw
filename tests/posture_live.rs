//! The brain's novelty reaches a node's reflex slot — on the bench, on the
//! real path. Walkthrough §A5g.
//!
//! `obc_agent::posture::PosturePolicy` with the production
//! `SerialCommandSink` on the base station, the production RX path
//! (`run_gateway_rx` with `LoraAuth`) bringing the node's replies into world
//! memory, and the node's real die-temperature rules from §A5f. Two
//! assessments — one the mushroom body would call novel, one familiar — and
//! for each: the `descend` reply the node sent back over the authenticated
//! link, and the LED the rule drove, read back with `gpio_read` over the same
//! link. The assessment itself is synthetic here: a real one needs a
//! trajectory store with episodes, which the bench body does not have yet.
//!
//! Preconditions: `scripts/bench_die_rule.py --node COM6 --base COM3` run
//! first (it leaves the pin-21 limit and the two rules loaded on the node and
//! the slot cleared), and the die temperature in the 36–50 °C band the levels
//! below assume (it was 38 °C on the bench; `scripts/probe_die_temp.py`).
//!
//! ```powershell
//! $env:OBC_BASE_PORT = 'COM3'
//! $env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
//! cargo test --features hardware --test posture_live -- --ignored --nocapture
//! ```

#![cfg(feature = "hardware")]

use std::sync::Arc;
use std::time::Duration;

use oh_ben_claw::agent::posture::{Posture, PostureConfig, PosturePolicy, PostureTarget};
use oh_ben_claw::memory::mushroom::Assessment;
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::spine::lora_gateway::{
    open_split, run_gateway_rx, AirWatch, CommandSink, GatewayHandle, LoraAuth, NodeCommand,
    SerialCommandSink,
};
use serde_json::Value;

const NODE: &str = "obc-esp32-s3-001";
const LED_PIN: i64 = 21;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set — see the file header"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Poll world memory for the node's reply to command `id`.
async fn reply_for(world: &WorldMemory, id: &str, timeout: Duration) -> Option<Value> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(f)) = world.current(&format!("mesh.{NODE}.cmd_result")) {
            if f.value.get("id").and_then(Value::as_str) == Some(id) {
                return Some(f.value);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

/// The `result` field, which the node sends as a JSON string — on a reply,
/// or on the policy's fact, which copies it.
fn result_of(reply: &Value) -> Value {
    match reply.get("result") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

/// The pin level in a `gpio_read` reply. The node sends `result` as a bare
/// number over the mesh; older paths sent a string. Accept both.
fn level_of(reply: &Value) -> i64 {
    match result_of(reply) {
        Value::Number(n) => n.as_i64().unwrap_or(-1),
        Value::String(s) => s.trim().parse().unwrap_or(-1),
        other => panic!("unexpected gpio_read result {other}"),
    }
}

/// Wait for the policy's own confirmation of its last send to `NODE`: the
/// `descending.<node>` fact with `answered` set (it resends on silence, so
/// the reply may carry a retry id — the fact is the contract, not the id).
async fn confirmed(world: &WorldMemory, timeout: Duration) -> Value {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(f)) = world.current(&format!("descending.{NODE}")) {
            if !f.value["answered"].is_null() {
                return f.value;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the policy never recorded an answer within {timeout:?}");
}

/// Send a command over the mesh with retries and wait for its reply.
async fn ask(sink: &dyn CommandSink, world: &WorldMemory, cmd: &str, args: Value) -> Value {
    for attempt in 0..3 {
        let id = format!("pl{}{}", now_ms() % 100_000, attempt);
        sink.send_command(&NodeCommand::new(NODE, &id, cmd, args.clone()))
            .await
            .expect("sink");
        if let Some(r) = reply_for(world, &id, Duration::from_secs(10)).await {
            return r;
        }
        println!("no reply to {cmd} ({id}); retrying");
    }
    panic!("no reply to {cmd} in three attempts");
}

#[tokio::test]
#[ignore = "needs the base station on OBC_BASE_PORT, OBC_SPINE_ROOT, and §A5f's rules on the node"]
async fn a_novel_objective_lights_the_led_and_a_familiar_one_clears_it() {
    let port = env("OBC_BASE_PORT");
    let auth = LoraAuth::new(&env("OBC_SPINE_ROOT")).unwrap();
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let (rx, wr) = open_split(&port, 115_200).expect("base station console");
    let w = Arc::clone(&world);
    let _rx_task = tokio::spawn(async move {
        let mut auth = auth;
        let mut air = AirWatch::new();
        run_gateway_rx(rx, &mut auth, &mut air, w, now_ms).await
    });
    let handle = Arc::new(GatewayHandle::open(wr, now_ms()));
    let sink: Arc<dyn CommandSink> = Arc::new(SerialCommandSink::new(handle));

    let cfg = PostureConfig {
        enabled: true,
        novel_level: 0.15, // 30 + 0.15·40 = 36 °C: below a 38 °C die → hot → LED on
        nodes: vec![PostureTarget {
            node_id: NODE.into(),
            slots: vec![0],
        }],
        ..PostureConfig::default()
    };
    let policy = PosturePolicy::new(cfg, Arc::clone(&sink), Some(Arc::clone(&world)));

    // Where we start: the slot cleared by §A5f, so the default 50 °C holds and
    // the LED is off.
    let led = ask(
        sink.as_ref(),
        &world,
        "gpio_read",
        serde_json::json!({ "pin": LED_PIN }),
    )
    .await;
    println!("LED before: {}", level_of(&led));

    // ── Novel ──────────────────────────────────────────────────────────────
    let novel = Assessment {
        novelty: 0.92,
        novel: true,
        success_prior: None,
    };
    let applied = policy
        .apply(
            &novel,
            "take the rover somewhere it has never been",
            now_ms(),
        )
        .await;
    assert_eq!(applied.posture, Posture::Cautious);
    assert_eq!(
        applied.sent,
        vec![NODE.to_string()],
        "failed: {:?}",
        applied.failed
    );
    // (retries + 1) attempts × timeout, plus the reply's own trip.
    let fact = confirmed(&world, Duration::from_secs(30)).await;
    println!("descend (cautious) confirmation: {fact}");
    assert_eq!(fact["answered"], true, "{fact}");
    assert_eq!(fact["ok"], true, "{fact}");
    assert_eq!(result_of(&fact)["active"], serde_json::json!([[0, 0.15]]));
    assert_eq!(policy.confirmed(NODE), Some(Posture::Cautious));
    // The rule needs a tick and its debounce; then the LED must be on (0).
    tokio::time::sleep(Duration::from_secs(12)).await;
    let led = ask(
        sink.as_ref(),
        &world,
        "gpio_read",
        serde_json::json!({ "pin": LED_PIN }),
    )
    .await;
    println!("LED after cautious: {}", level_of(&led));
    assert_eq!(
        level_of(&led),
        0,
        "cautious posture must light the LED (active-low)"
    );

    // ── Familiar ───────────────────────────────────────────────────────────
    let familiar = Assessment {
        novelty: 0.1,
        novel: false,
        success_prior: Some(0.9),
    };
    let applied = policy
        .apply(&familiar, "check the printer like every morning", now_ms())
        .await;
    assert_eq!(applied.posture, Posture::Default);
    assert_eq!(
        applied.sent,
        vec![NODE.to_string()],
        "failed: {:?}",
        applied.failed
    );
    let fact = confirmed(&world, Duration::from_secs(30)).await;
    println!("descend (clear) confirmation: {fact}");
    assert_eq!(fact["answered"], true, "{fact}");
    assert_eq!(result_of(&fact)["active"], serde_json::json!([]));
    assert_eq!(policy.confirmed(NODE), Some(Posture::Default));
    tokio::time::sleep(Duration::from_secs(12)).await;
    let led = ask(
        sink.as_ref(),
        &world,
        "gpio_read",
        serde_json::json!({ "pin": LED_PIN }),
    )
    .await;
    println!("LED after default: {}", level_of(&led));
    assert_eq!(level_of(&led), 1, "default posture must put the LED out");

    // And a second familiar turn sends nothing: posture is sent on change.
    let applied = policy
        .apply(&familiar, "check the printer again", now_ms())
        .await;
    assert!(applied.sent.is_empty());
}

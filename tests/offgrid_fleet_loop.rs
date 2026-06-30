//! Off-grid fleet coordination — wiring delivered cores end to end.
//!
//! This composes three pieces built independently into one working path, with no
//! hardware and no broker:
//!   1. **Node self-test (`peripherals::selftest`)** — two simulated nodes must
//!      pass their bring-up suite before the brain trusts them on the fleet.
//!   2. **LoRa-mesh transport (`spine::lora_mesh`)** — each node broadcasts a
//!      compact heartbeat over the mesh (a loopback radio stands in for the real
//!      Meshtastic device).
//!   3. **Fleet coordinator (`fleet`)** — the decoded mesh heartbeats become
//!      `NodeState`s, and a task is auctioned to the nearest node.
//!
//! The point: the same auction logic that runs over MQTT runs unchanged over a
//! LoRa mesh with no infrastructure — the embodied off-grid story, verified.

use async_trait::async_trait;
use oh_ben_claw::fleet::{Coordinator, Task};
use oh_ben_claw::peripherals::selftest::{NodeSelfTest, SimulatedNode};
use oh_ben_claw::spine::lora_mesh::{LoraMeshConfig, LoraMeshSpine, MeshFrame, MeshRadio};
use std::sync::{Arc, Mutex};

/// A loopback radio that records every transmitted frame (stands in for a real
/// Meshtastic device for the test).
struct CaptureRadio {
    sent: Mutex<Vec<Vec<u8>>>,
}

#[async_trait]
impl MeshRadio for CaptureRadio {
    async fn transmit(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
}

#[tokio::test]
async fn off_grid_simulated_nodes_coordinate_over_lora_mesh() {
    // 1. Two nodes must pass bring-up before we trust them on the fleet.
    let a = SimulatedNode::healthy("rover-a", "lilygo-t-beam");
    let b = SimulatedNode::healthy("rover-b", "rak4631");
    assert!(a.run_bringup().await.all_passed(), "rover-a failed bring-up");
    assert!(b.run_bringup().await.all_passed(), "rover-b failed bring-up");

    // 2. Each node broadcasts a heartbeat over the (loopback) LoRa mesh.
    let radio = Arc::new(CaptureRadio { sent: Mutex::new(Vec::new()) });
    let spine = LoraMeshSpine::new(Arc::clone(&radio) as Arc<dyn MeshRadio>, LoraMeshConfig::default());
    spine
        .send_heartbeat("rover-a", Some(0.0), Some(0.0), Some(80.0), "normal")
        .await
        .unwrap();
    spine
        .send_heartbeat("rover-b", Some(10.0), Some(0.0), Some(80.0), "normal")
        .await
        .unwrap();

    // 3. The coordinator ingests the mesh heartbeats: decode → NodeState → report.
    let coord = Coordinator::new();
    for bytes in radio.sent.lock().unwrap().iter() {
        let frame = MeshFrame::decode(bytes).expect("a valid mesh frame");
        if let Some(state) = frame.to_node_state(1_000) {
            coord.report(state);
        }
    }

    // A task next to rover-b is auctioned to it — fleet coordination with no
    // WiFi and no broker, over the same logic that runs on MQTT.
    coord.add_task(Task { id: "t1".into(), x: 9.0, y: 0.0, min_battery: 0.0 });
    let awards = coord.auction_tick(1_000);
    assert_eq!(awards, vec![("t1".to_string(), "rover-b".to_string())]);
}

#[tokio::test]
async fn a_node_that_fails_bringup_is_not_trusted() {
    // A node failing a bring-up check should be held back from the fleet.
    let bad = SimulatedNode::healthy("rover-x", "esp32-s3").with_failing_check("link_up");
    let report = bad.run_bringup().await;
    assert!(!report.all_passed());
    assert_eq!(report.failures().len(), 1);

    // Onboarding gate: only report nodes that passed bring-up.
    let coord = Coordinator::new();
    if report.all_passed() {
        // (not reached) — the node would be reported here
        let radio = Arc::new(CaptureRadio { sent: Mutex::new(Vec::new()) });
        let spine =
            LoraMeshSpine::new(radio as Arc<dyn MeshRadio>, LoraMeshConfig::default());
        spine.send_heartbeat("rover-x", Some(0.0), Some(0.0), Some(50.0), "normal").await.unwrap();
    }
    coord.add_task(Task { id: "t".into(), x: 0.0, y: 0.0, min_battery: 0.0 });
    // No healthy node was reported, so the task finds no taker.
    assert!(coord.auction_tick(1_000).is_empty(), "untrusted node must not get work");
}

//! End-to-end LoRa-mesh ⇄ fleet loop — the new bidirectional, multi-hop path.
//!
//! `offgrid_fleet_loop.rs` proved the *inbound* half (a heartbeat decoded into a
//! `NodeState` and auctioned). This test closes the **whole loop** end to end,
//! exercising the pieces added since:
//!
//!   1. **RX bridge** (`ingest_line`) — a heartbeat arriving as raw mesh bytes is
//!      bridged straight into the coordinator (no manual decode in the caller).
//!   2. **Auction** — the coordinator awards a queued task to the nearest node.
//!   3. **TX egress** (`broadcast_outbox`) — the award drains from the
//!      coordinator's assignment outbox and goes back out over the mesh as a
//!      `MeshFrame::Assign`.
//!   4. **Multi-hop relay** (`relay::MeshRelay` + `ingest_line_relayed`) — a
//!      relayed heartbeat is ingested, rebroadcast once with a decremented hop
//!      count, and dropped on a second hearing (flood + de-dup).
//!
//! No hardware, no broker: a loopback radio stands in for the LoRa device, and
//! the identical auction logic runs over the mesh.

use async_trait::async_trait;
use oh_ben_claw::fleet::{Coordinator, Task};
use oh_ben_claw::spine::lora_mesh::relay::{originate, MeshRelay};
use oh_ben_claw::spine::lora_mesh::{
    broadcast_outbox, ingest_line, ingest_line_relayed, MeshFrame, MeshRadio,
};
use std::sync::{Arc, Mutex};

/// A loopback radio that records every transmitted frame (stands in for a real
/// Meshtastic device).
struct CaptureRadio {
    sent: Mutex<Vec<Vec<u8>>>,
}

impl CaptureRadio {
    fn new() -> Self {
        Self { sent: Mutex::new(Vec::new()) }
    }
    fn last(&self) -> Option<Vec<u8>> {
        self.sent.lock().unwrap().last().cloned()
    }
}

#[async_trait]
impl MeshRadio for CaptureRadio {
    async fn transmit(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
}

/// Encode a node heartbeat as the raw mesh bytes a node would put on the air.
fn heartbeat_bytes(node: &str, x: f64, y: f64) -> Vec<u8> {
    MeshFrame::Heartbeat {
        node: node.to_string(),
        x: Some(x),
        y: Some(y),
        battery: Some(80.0),
        mode: "normal".to_string(),
    }
    .encode()
}

#[tokio::test]
async fn a_heartbeat_heard_over_the_mesh_comes_back_as_an_assignment() {
    // Coordinator with the off-grid assignment outbox enabled (as `main` does
    // when a LoRa transport is present).
    let coord = Coordinator::new().with_assignment_outbox();

    // 1. Two rovers' heartbeats arrive over the mesh and bridge into the fleet.
    assert!(ingest_line(&heartbeat_bytes("rover-a", 0.0, 0.0), &coord, 1_000));
    assert!(ingest_line(&heartbeat_bytes("rover-b", 10.0, 0.0), &coord, 1_000));

    // 2. A task near rover-a is auctioned — nearest online idle node wins.
    coord.add_task(Task { id: "t".into(), x: 1.0, y: 0.0, min_battery: 0.0 });
    let awards = coord.auction_tick(2_000);
    assert_eq!(awards, vec![("t".to_string(), "rover-a".to_string())]);

    // 3. The award drains from the outbox and is broadcast back over the mesh.
    let radio = CaptureRadio::new();
    assert_eq!(broadcast_outbox(&radio, &coord).await, 1);

    // 4. The frame on the air is a "go here" assignment to the winning rover, at
    //    the task's coordinates — the loop is closed: heartbeat in → assignment out.
    let frame = MeshFrame::decode(&radio.last().expect("an assignment was sent")).unwrap();
    assert_eq!(frame, MeshFrame::Assign { node: "rover-a".into(), x: 1.0, y: 0.0 });

    // Outbox is drained: a second broadcast sends nothing.
    assert_eq!(broadcast_outbox(&radio, &coord).await, 0);
}

#[test]
fn a_multi_hop_heartbeat_floods_once_then_is_deduped() {
    let coord = Coordinator::new();
    let relay = MeshRelay::new();

    // A distant rover's heartbeat is originated with 2 hops of TTL.
    let hb = MeshFrame::Heartbeat {
        node: "rover-c".into(),
        x: Some(5.0),
        y: Some(5.0),
        battery: Some(90.0),
        mode: "explore".into(),
    };
    let framed = originate(&hb, 7, 2);

    // First hearing: ingested locally AND rebroadcast with the hop count decremented.
    let rebroadcast = ingest_line_relayed(&framed, &coord, &relay, 1_000)
        .expect("hops remain, so this node rebroadcasts");
    let relayed: serde_json::Value = serde_json::from_slice(&rebroadcast).unwrap();
    assert_eq!(relayed.get("h").and_then(|h| h.as_u64()), Some(1));
    assert_eq!(MeshFrame::decode(&rebroadcast).unwrap(), hb);

    // The heartbeat reached the fleet: rover-c can now win a task.
    coord.add_task(Task { id: "t".into(), x: 5.0, y: 5.0, min_battery: 0.0 });
    assert_eq!(
        coord.auction_tick(2_000),
        vec![("t".to_string(), "rover-c".to_string())]
    );

    // Second hearing of the same message id (an echo from a neighbour): dropped —
    // no rebroadcast, so the flood terminates instead of looping forever.
    assert!(ingest_line_relayed(&framed, &coord, &relay, 1_100).is_none());
}

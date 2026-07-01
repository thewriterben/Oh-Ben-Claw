//! End-to-end MQTT-spine ⇄ fleet loop — the on-grid twin of `mesh_fleet_e2e`.
//!
//! Same heartbeat → auction → assignment story the LoRa mesh test proves, but over
//! the MQTT spine transport, and with no broker:
//!
//!   1. **Inbound** — `spine_heartbeat_handler` is the exact closure the live
//!      subscription installs. Invoking it with a topic + payload is precisely what
//!      an arriving `obc/fleet/heartbeat/{node}` message does: decode → `NodeState`
//!      → report into the coordinator.
//!   2. **Auction** — the coordinator awards a queued task to the nearest node.
//!   3. **Outbound** — the award drains from the assignment outbox and is rendered
//!      to the spine wire contract via `assignment_topic` / `assignment_payload`
//!      (the pure halves of `publish_assignment`), exactly as `main`'s egress does.
//!
//! Together with `mesh_fleet_e2e`, this shows the coordinator is transport-blind:
//! the identical auction runs whether heartbeats arrive over MQTT or LoRa.

use oh_ben_claw::fleet::{
    assignment_payload, assignment_topic, spine_heartbeat_handler, Coordinator, Task,
};
use oh_ben_claw::navigation::NavGoal;
use std::sync::Arc;

/// Wall-clock milliseconds — the same clock `spine_heartbeat_handler` stamps
/// `last_seen_ms` with, so the auction's freshness check sees the nodes as online.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn a_heartbeat_over_the_spine_comes_back_as_an_assignment() {
    let coord = Arc::new(Coordinator::new().with_assignment_outbox());

    // 1. Inbound: two heartbeats arrive on `obc/fleet/heartbeat/{node}` and the
    //    installed handler folds them into the coordinator.
    let handler = spine_heartbeat_handler(Arc::clone(&coord));
    handler(
        "obc/fleet/heartbeat/rover-a",
        br#"{"x":0.0,"y":0.0,"battery":80.0,"mode":"normal"}"#,
    );
    handler(
        "obc/fleet/heartbeat/rover-b",
        br#"{"x":10.0,"y":0.0,"battery":80.0,"mode":"normal"}"#,
    );

    // 2. A task near rover-a is auctioned to the nearest online idle node.
    coord.add_task(Task { id: "t".into(), x: 1.0, y: 0.0, min_battery: 0.0 });
    let awards = coord.auction_tick(now_ms());
    assert_eq!(awards, vec![("t".to_string(), "rover-a".to_string())]);

    // 3. Outbound: the award drains from the outbox and renders to the spine wire
    //    contract — `main`'s egress publishes exactly this topic + payload.
    let intents = coord.drain_outbox();
    assert_eq!(intents.len(), 1);
    let (node, x, y) = intents[0].clone();
    assert_eq!(node, "rover-a");

    let goal = NavGoal { x, y, tolerance: 0.5 };
    assert_eq!(assignment_topic(&node), "obc/fleet/assign/rover-a");
    assert_eq!(
        assignment_payload(&goal),
        serde_json::json!({ "x": 1.0, "y": 0.0, "tolerance": 0.5 })
    );
}

#[test]
fn a_malformed_heartbeat_payload_is_ignored() {
    // The handler must not panic or report a node on garbage — an offline / no-node
    // coordinator is the safe outcome.
    let coord = Arc::new(Coordinator::new());
    let handler = spine_heartbeat_handler(Arc::clone(&coord));
    handler("obc/fleet/heartbeat/rover-x", b"not json");
    handler("obc/fleet/heartbeat/", br#"{"x":1.0}"#); // empty node id

    // No node was registered, so a queued task finds nobody and stays queued.
    coord.add_task(Task { id: "t".into(), x: 0.0, y: 0.0, min_battery: 0.0 });
    assert!(coord.auction_tick(now_ms()).is_empty());
}

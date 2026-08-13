//! The fleet's spine bridge — MQTT ingress and egress for the coordinator.
//!
//! This lived in `src/fleet/mod.rs` until 2026-08-13 and was that module's
//! *only* reference to anything still in this tree: two lines, `SpineClient` +
//! `MessageHandler` and `TOPIC_PREFIX`. Everything else `fleet` needs —
//! `obc_memory`, `obc_navigation`, `obc_telemetry` — has been a crate for
//! weeks. Those two lines were the whole `spine <-> fleet` cycle *and* the only
//! thing keeping an 812-line coordinator in the core.
//!
//! Why this side and not the other: **a bridge belongs with the transport, not
//! with the domain.** `fleet` is allocation and task auction — registry, cost,
//! separation, who goes where. Topic strings, payload shapes, `MessageHandler`
//! and `SpineClient` are all facts about MQTT, and the coordinator is correct
//! without knowing any of them. `src/spine/lora_mesh.rs` already bridges the
//! *other* transport into the same coordinator from this side, which is the
//! detail that settled it: the same integration was being built from both ends,
//! and only one end was the transport.
//!
//! Sixth instance this month of trait-or-glue-where-the-dependency-is:
//! `SpineActuatorSink`, `SpineActionSink`, `AgentExecutor`, `Severity`,
//! `ReplayExecutor for Agent`, and now this.
//!
//! The pure halves are deliberately separate from the publishing half.
//! [`assignment_topic`] and [`assignment_payload`] are the wire contract and
//! are testable with no broker; `tests/spine_fleet_e2e.rs` drives them, and
//! `src/main.rs`'s egress path calls exactly the same two functions.

use crate::fleet::{Coordinator, NodeState};
use crate::navigation::NavGoal;
use crate::spine::{MessageHandler, SpineClient, TOPIC_PREFIX};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// MQTT topic filter for fleet node heartbeats (`obc/fleet/heartbeat/{node}`).
///
/// Came across with the bridge: a topic filter is a fact about the transport,
/// and it is the argument [`spine_heartbeat_handler`] is registered against.
pub const HEARTBEAT_FILTER: &str = "obc/fleet/heartbeat/+";

/// Wall-clock milliseconds. Local rather than imported: `fleet`'s copy is
/// private, and a bridge that borrowed it would be reaching back across the
/// edge this file exists to remove.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A spine message handler that ingests node heartbeats (`obc/fleet/heartbeat/
/// {node}`) into the coordinator: the node id is the last topic segment and the
/// payload carries `{x, y, battery, mode}`. Register with
/// [`SpineClient::subscribe_handler`].
pub fn spine_heartbeat_handler(coord: Arc<Coordinator>) -> MessageHandler {
    Arc::new(move |topic: &str, payload: &[u8]| {
        let id = topic.rsplit('/').next().unwrap_or("").to_string();
        if id.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(payload) else {
            return;
        };
        coord.report(NodeState {
            id,
            x: v.get("x").and_then(Value::as_f64),
            y: v.get("y").and_then(Value::as_f64),
            battery: v.get("battery").and_then(Value::as_f64),
            mode: v
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            busy: false,
            last_seen_ms: now_ms(),
        });
    })
}

/// The spine topic an assignment for `node` is published on (`obc/fleet/assign/{node}`).
/// Pure — the wire contract, testable without a broker.
pub fn assignment_topic(node: &str) -> String {
    format!("{TOPIC_PREFIX}/fleet/assign/{node}")
}

/// The assignment payload for `goal`. Pure — the wire contract, testable without
/// a broker (mirrors the LoRa side's `MeshFrame::Assign`).
pub fn assignment_payload(goal: &NavGoal) -> Value {
    json!({ "x": goal.x, "y": goal.y, "tolerance": goal.tolerance })
}

/// Publish an assignment back to a node over the spine (`obc/fleet/assign/{node}`).
pub async fn publish_assignment(
    spine: &SpineClient,
    node: &str,
    goal: &NavGoal,
) -> anyhow::Result<()> {
    spine
        .publish(&assignment_topic(node), &assignment_payload(goal))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_handler_ingests_a_node_report() {
        let coord = Arc::new(Coordinator::new());
        let handler = spine_heartbeat_handler(Arc::clone(&coord));
        let payload =
            serde_json::to_vec(&json!({ "x": 3.0, "y": 4.0, "battery": 72.0, "mode": "normal" }))
                .unwrap();
        handler("obc/fleet/heartbeat/rover-7", &payload);
        let status = coord.status(now_ms());
        let nodes = status["nodes"].as_array().unwrap();
        assert!(nodes
            .iter()
            .any(|n| n["id"] == "rover-7" && n["battery"] == 72.0));
    }
}

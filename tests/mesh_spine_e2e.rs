//! Phase B mesh spine — host-observable outbound→reply round trip (no hardware).
//!
//! Exercises the *logic* of the bidirectional LoRa mesh spine end-to-end on the host,
//! without any radio: the outbound half encodes a node command the way the base
//! station would transmit it, and the inbound half ingests the node's reply the way it
//! arrives off the gateway console. Together they prove the observable contract —
//! command routing, gated execution identity, and correlation — that the firmware and
//! two jumpers physically realise.

use async_trait::async_trait;
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::spine::lora_gateway::{ingest_gateway_line, CommandSink, NodeCommand};
use serde_json::json;
use std::sync::Mutex;

/// A [`CommandSink`] that records what it was asked to transmit (stands in for the
/// base-station Heltec's serial writer).
struct CapturingSink {
    sent: Mutex<Vec<String>>,
}

#[async_trait]
impl CommandSink for CapturingSink {
    async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push(cmd.encode());
        Ok(())
    }
}

#[tokio::test]
async fn outbound_command_then_reply_round_trips_through_the_mesh() {
    let node = "obc-esp32-s3-001";

    // ── Outbound: the agent addresses a gpio_write to the node over the mesh. ──
    let sink = CapturingSink { sent: Mutex::new(Vec::new()) };
    let cmd = NodeCommand::new(node, "h1", "gpio_write", json!({ "pin": 3, "value": 1 }));
    sink.send_command(&cmd).await.unwrap();

    // The exact line the base station transmits: a valid node request the node routes
    // on `to` and parses as id/cmd/args (no firmware request-format change needed).
    let line = sink.sent.lock().unwrap()[0].clone();
    let req: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(req["to"], json!(node), "the node routes on `to`");
    assert_eq!(req["id"], json!("h1"));
    assert_eq!(req["cmd"], json!("gpio_write"));
    assert_eq!(req["args"]["pin"], json!(3));
    assert!(!line.contains('\n'), "one line for the mesh");

    // ── Reply: the node executed under its on-MCU Track 0 gate and its cmd_result
    // rides LoRa home; the base-station console shows it and the inbound bridge
    // ingests it into world memory. ──
    let world = WorldMemory::open_in_memory().unwrap();
    let reply_line = format!(
        "SPINE ◄ src=2A seq=9 rssi=-40 dBm : {}",
        json!({
            "type": "cmd_result",
            "node_id": node,
            "id": "h1",
            "ok": true,
            "result": "gpio 3 = 1"
        })
    );
    let ing = ingest_gateway_line(&reply_line, &world, 1_000).expect("reply ingested");
    assert_eq!(ing.node_id, node);
    assert_eq!(ing.msg_type, "cmd_result");
    assert_eq!(ing.rssi_dbm, -40);

    // The reply is a queryable fact, correlatable back to the command's `id`.
    let fact = world
        .current(&format!("mesh.{node}.cmd_result"))
        .unwrap()
        .expect("cmd_result fact exists");
    assert_eq!(fact.value["id"], json!("h1"), "correlation id round-trips");
    assert_eq!(fact.value["ok"], json!(true));
    assert_eq!(fact.value["_mesh"]["rssi_dbm"], json!(-40), "mesh envelope attached");

    // The liveness rollup reflects the node's latest message type over the mesh.
    let link = world.current(&format!("mesh.{node}")).unwrap().expect("rollup fact exists");
    assert_eq!(link.value["last_type"], json!("cmd_result"));
    assert_eq!(link.value["rssi_dbm"], json!(-40));
}

#[tokio::test]
async fn a_command_for_another_node_is_addressed_to_that_node() {
    // Routing contract: the encoded line names the intended recipient, so a node that
    // isn't the target drops it (the firmware compares `to` against its own id).
    let sink = CapturingSink { sent: Mutex::new(Vec::new()) };
    let cmd = NodeCommand::new("node-b", "x9", "sensor_read", json!({ "kind": "dht22" }));
    sink.send_command(&cmd).await.unwrap();
    let req: serde_json::Value =
        serde_json::from_str(&sink.sent.lock().unwrap()[0]).unwrap();
    assert_eq!(req["to"], json!("node-b"));
    assert_ne!(req["to"], json!("obc-esp32-s3-001"));
}

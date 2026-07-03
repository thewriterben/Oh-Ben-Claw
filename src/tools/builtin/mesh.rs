//! Mesh command tool — the outbound half of the Phase B LoRa spine.
//!
//! Exposes the off-grid **return path** to the agent (System 2): `mesh_command`
//! addresses a node command over the LoRa mesh. It is the inverse of the inbound
//! gateway bridge ([`crate::spine::lora_gateway`], which ingests node messages into
//! world memory) — together they make the mesh a two-way link.
//!
//! The command is delivered toward the mesh, but **execution is gated on the node**:
//! the node feeds a mesh command through the exact same Track 0–gated request
//! dispatcher as a wired serial command, so a `gpio_write` over the air actuates only
//! within the node's on-MCU allow-list / range / rate limits. This tool therefore
//! declares a physical risk class so the host approval layer treats it accordingly.

use crate::memory::world::WorldMemory;
use crate::spine::lora_gateway::{CommandSink, NodeCommand};
use crate::spine::mesh_supervisor;
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: send a command to a node over the LoRa mesh (off-grid return path).
pub struct MeshCommandTool {
    sink: Arc<dyn CommandSink>,
}

impl MeshCommandTool {
    /// Build the tool over a command sink (the serial link to the base-station Heltec).
    pub fn new(sink: Arc<dyn CommandSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl Tool for MeshCommandTool {
    fn name(&self) -> &str {
        "mesh_command"
    }

    fn description(&self) -> &str {
        "Send a command to a node over the LoRa mesh (off-grid return path — no WiFi/MQTT). \
         Addresses a single node by id and delivers a node command (e.g. 'gpio_write', \
         'sensor_read', 'capabilities') with optional args. The node executes it under its \
         own on-MCU Track 0 safety gate; the reply, if any, returns over the mesh into world \
         memory. Use when a node is reachable only over LoRa."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": {
                    "type": "string",
                    "description": "Target node id (the node executes only if this matches its own id)."
                },
                "command": {
                    "type": "string",
                    "description": "The node command, e.g. 'gpio_write', 'sensor_read', 'capabilities'."
                },
                "args": {
                    "type": "object",
                    "description": "Command arguments (any JSON object the node's handler understands)."
                }
            },
            "required": ["node_id", "command"]
        })
    }

    fn risk_class(&self) -> RiskClass {
        // A mesh command can drive a remote physical actuator (e.g. a node gpio_write);
        // the node gates it on-MCU, but the host approval layer still treats it as a
        // physical, high-blast action (per-call approval, never `forever`).
        RiskClass::physical(true, BlastRadius::High)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let node_id = args.get("node_id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if node_id.is_empty() {
            return Ok(ToolResult::err("mesh_command requires a non-empty 'node_id'"));
        }
        let command = args.get("command").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if command.is_empty() {
            return Ok(ToolResult::err("mesh_command requires a non-empty 'command'"));
        }
        let cmd_args = args.get("args").cloned().unwrap_or_else(|| json!({}));
        let id = uuid::Uuid::new_v4().to_string();

        let node_cmd = NodeCommand::new(&node_id, &id, &command, cmd_args);
        match self.sink.send_command(&node_cmd).await {
            Ok(()) => Ok(ToolResult::ok(
                json!({
                    "sent": true,
                    "id": id,
                    "to": node_id,
                    "command": command,
                    "note": "delivered to the mesh; the node executes under its on-MCU Track 0 gate, \
                             and any reply returns over the mesh into world memory"
                })
                .to_string(),
            )),
            Err(e) => Ok(ToolResult::err(format!("mesh_command send failed: {e}"))),
        }
    }
}

/// Read-only view of the LoRa mesh's health, for System 2. When a mesh escalation
/// wakes the agent, this is how it sees *which* node is in trouble and its state, so it
/// can decide what to do (e.g. issue a diagnostic `mesh_command`, alert, or re-plan).
pub struct MeshStatusTool {
    world: Arc<WorldMemory>,
}

impl MeshStatusTool {
    pub fn new(world: Arc<WorldMemory>) -> Self {
        Self { world }
    }
}

#[async_trait]
impl Tool for MeshStatusTool {
    fn name(&self) -> &str {
        "mesh_status"
    }

    fn description(&self) -> &str {
        "Summarize the health of all LoRa mesh nodes from world memory: per-node health \
         (online/degraded/offline), whether the supervisor has presumed it lost \
         (escalated), link RSSI, last message type, seconds since last heard, and last \
         command outcome — plus fleet counts. Read-only. Call this when woken by a mesh \
         escalation (or anytime) to see which node needs attention, then act with \
         'mesh_command'."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let views = mesh_supervisor::snapshot(&self.world);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let (mut online, mut degraded, mut offline, mut escalated) = (0u64, 0u64, 0u64, 0u64);
        let mut nodes = Vec::with_capacity(views.len());
        for v in &views {
            let health = v.prev_health.map(|h| h.as_str()).unwrap_or("unknown");
            match health {
                "online" => online += 1,
                "degraded" => degraded += 1,
                "offline" => offline += 1,
                _ => {}
            }
            if v.escalated {
                escalated += 1;
            }
            let rollup = self.world.current(&format!("mesh.{}", v.node)).ok().flatten();
            let rssi = rollup
                .as_ref()
                .and_then(|f| f.value.get("rssi_dbm").and_then(|r| r.as_i64()));
            let last_type = rollup
                .as_ref()
                .and_then(|f| f.value.get("last_type").and_then(|t| t.as_str()))
                .unwrap_or("-")
                .to_string();
            nodes.push(json!({
                "node": v.node,
                "health": health,
                "escalated": v.escalated,
                "rssi_dbm": rssi,
                "last_type": last_type,
                "age_s": now.saturating_sub(v.last_seen_ms) / 1000,
                "last_cmd_ok": v.last_cmd_ok,
            }));
        }

        let out = json!({
            "summary": {
                "nodes": views.len(),
                "online": online,
                "degraded": degraded,
                "offline": offline,
                "escalated": escalated,
            },
            "nodes": nodes,
        });
        Ok(ToolResult::ok(out.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::world::WorldMemory;

    #[tokio::test]
    async fn mesh_status_summarizes_node_health() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe("mesh.n1", json!({ "last_type": "reflex", "rssi_dbm": -80 }), 1_000, 1_000, "t")
            .unwrap();
        world.observe("mesh.n1.health", json!({ "status": "offline" }), 1_000, 1_000, "t").unwrap();
        world
            .observe("mesh.n1.escalation", json!({ "status": "escalated" }), 1_000, 1_000, "t")
            .unwrap();

        let tool = MeshStatusTool::new(world);
        let res = tool.execute(json!({})).await.unwrap();
        assert!(res.is_ok());
        let v: Value = serde_json::from_str(res.output()).unwrap();
        assert_eq!(v["summary"]["nodes"], json!(1));
        assert_eq!(v["summary"]["offline"], json!(1));
        assert_eq!(v["summary"]["escalated"], json!(1));
        assert_eq!(v["nodes"][0]["node"], json!("n1"));
        assert_eq!(v["nodes"][0]["escalated"], json!(true));
        assert_eq!(v["nodes"][0]["rssi_dbm"], json!(-80));
        assert_eq!(v["nodes"][0]["health"], json!("offline"));
    }
}

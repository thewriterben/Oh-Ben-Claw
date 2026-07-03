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

use crate::spine::lora_gateway::{CommandSink, NodeCommand};
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

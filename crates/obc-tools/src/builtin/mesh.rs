//! Mesh command tool — the outbound half of the Phase B LoRa spine.
//!
//! Exposes the off-grid **return path** to the agent (System 2): `mesh_command`
//! addresses a node command over the LoRa mesh. It is the inverse of the inbound
//! gateway bridge ([`obc_spine::lora_gateway`], which ingests node messages into
//! world memory) — together they make the mesh a two-way link.
//!
//! The command is delivered toward the mesh, but **execution is gated on the node**:
//! the node feeds a mesh command through the exact same Track 0–gated request
//! dispatcher as a wired serial command, so a `gpio_write` over the air actuates only
//! within the node's on-MCU allow-list / range / rate limits. This tool therefore
//! declares a physical risk class so the host approval layer treats it accordingly.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_memory::world::WorldMemory;
use obc_spine::lora_gateway::{CommandSink, NodeCommand, MESH_LINE_BUDGET};
use obc_spine::mesh_supervisor;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: send a command to a node over the LoRa mesh (off-grid return path).
pub struct MeshCommandTool {
    sink: Arc<dyn CommandSink>,
    reply: Option<ReplyWait>,
}

/// Wait for the node's answer, resending when it does not come.
///
/// The mesh has no acknowledgement at any layer. Measured on the bench on
/// 2026-09-12 with the radio defects fixed: a plain half-duplex collision
/// still loses about one command or reply in five. The node's reply, when it
/// arrives, is ingested by the gateway bridge as the world-memory fact
/// `mesh.<node_id>.cmd_result` carrying the command's `id`; this polls for it
/// and, on silence, sends again with a fresh id — accepting a late answer to
/// any attempt, since every retried command is idempotent (see
/// [`RETRY_SAFE`]). Before this the tool reported `sent: true` for a frame
/// that may never have arrived, which for a tool classed physical/high-blast
/// is the wrong thing to be confident about.
pub struct ReplyWait {
    /// Where the gateway bridge writes `mesh.<node>.cmd_result`.
    pub world: Arc<WorldMemory>,
    /// How long to wait for one attempt's reply. A node answers in 1–2 s over
    /// one LoRa hop; 8 s leaves room for a relay.
    pub timeout_ms: u64,
    /// Resends after the first attempt goes unanswered.
    pub retries: u32,
}

/// Commands safe to send twice: applying them again yields the same node
/// state, so a lost reply can be retried without a second effect. Anything
/// not listed is sent once and reported as sent, as before.
pub const RETRY_SAFE: &[&str] = &[
    "descend",
    "capabilities",
    "announce",
    "gpio_read",
    "sensor_read",
    "gpio_write",
];

impl MeshCommandTool {
    /// Build the tool over a command sink (the serial link to the base-station Heltec).
    pub fn new(sink: Arc<dyn CommandSink>) -> Self {
        Self { sink, reply: None }
    }

    /// Await the node's reply through world memory and retry on silence.
    pub fn with_reply_wait(mut self, wait: ReplyWait) -> Self {
        self.reply = Some(wait);
        self
    }

    /// The `cmd_result` fact for `node_id` whose `id` is one of `ids`, if it
    /// has arrived. Latest fact only: the bridge supersedes the entity on
    /// every reply, and an older reply's id would not be in `ids`.
    fn reply_for(world: &WorldMemory, node_id: &str, ids: &[String]) -> Option<Value> {
        let fact = world
            .current(&format!("mesh.{node_id}.cmd_result"))
            .ok()
            .flatten()?;
        let id = fact.value.get("id").and_then(Value::as_str)?;
        ids.iter().any(|i| i == id).then(|| fact.value.clone())
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
         memory. Use when a node is reachable only over LoRa. Prefer 'descend' over \
         'gpio_write' when a node has slot-bound reflex rules: args {\"m\": [[slot, level], ...]} \
         with levels in [0, 1] moves the node's reflex thresholds within the ranges its rules \
         own, and the node keeps acting on its own sensors; {\"clear\": true} returns every \
         slot to its rule's default."
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
        let node_id = args
            .get("node_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if node_id.is_empty() {
            return Ok(ToolResult::err(
                "mesh_command requires a non-empty 'node_id'",
            ));
        }
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            return Ok(ToolResult::err(
                "mesh_command requires a non-empty 'command'",
            ));
        }
        let cmd_args = args.get("args").cloned().unwrap_or_else(|| json!({}));
        let id = uuid::Uuid::new_v4().to_string();

        // `descend` is built through its own constructor so a bad slot or
        // level is refused here, with the reason, rather than on the node.
        let node_cmd = if command == "descend" {
            let pairs: Vec<(u8, f64)> = match cmd_args.get("m") {
                Some(m) => match serde_json::from_value(m.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Ok(ToolResult::err(format!(
                            "descend: 'args.m' must be a list of [slot, level] pairs: {e}"
                        )))
                    }
                },
                None => Vec::new(),
            };
            let clear = cmd_args.get("clear").and_then(Value::as_bool) == Some(true);
            match NodeCommand::descend(&node_id, &id, &pairs, clear) {
                Ok(c) => c,
                Err(e) => return Ok(ToolResult::err(format!("mesh_command not sent: {e}"))),
            }
        } else {
            NodeCommand::new(&node_id, &id, &command, cmd_args)
        };

        // Refuse rather than report a send the mesh cannot make. The bridge's
        // line framer discards an over-budget line whole, so this used to return
        // `sent: true` for a command that never left — failing closed, which is
        // right, and silently, which is not.
        if !node_cmd.fits_one_frame() {
            return Ok(ToolResult::err(format!(
                "mesh_command not sent: the encoded command is {} bytes and one mesh frame \
                 carries {}. The node's line framer discards an over-long line whole, so this \
                 would have reported success and delivered nothing. Shorten 'args' or send it \
                 over a transport with no frame budget (MQTT/serial).",
                node_cmd.encoded_len(),
                MESH_LINE_BUDGET,
            )));
        }

        // Reply-awaited retry, when the world is reachable and the command
        // can safely be sent twice.
        if let Some(wait) = &self.reply {
            if RETRY_SAFE.contains(&command.as_str()) {
                return self
                    .send_awaiting_reply(wait, node_cmd, &node_id, &command)
                    .await;
            }
        }

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

impl MeshCommandTool {
    async fn send_awaiting_reply(
        &self,
        wait: &ReplyWait,
        first: NodeCommand,
        node_id: &str,
        command: &str,
    ) -> anyhow::Result<ToolResult> {
        const POLL_MS: u64 = 100;
        let base_id = first.id.clone();
        let mut ids: Vec<String> = Vec::new();
        for attempt in 0..=wait.retries {
            let cmd = if attempt == 0 {
                first.clone()
            } else {
                NodeCommand::new(
                    first.to.clone(),
                    format!("{base_id}r{attempt}"),
                    first.cmd.clone(),
                    first.args.clone(),
                )
            };
            ids.push(cmd.id.clone());
            if let Err(e) = self.sink.send_command(&cmd).await {
                return Ok(ToolResult::err(format!("mesh_command send failed: {e}")));
            }
            let deadline =
                tokio::time::Instant::now() + std::time::Duration::from_millis(wait.timeout_ms);
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
                if let Some(reply) = Self::reply_for(&wait.world, node_id, &ids) {
                    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let out = json!({
                        "sent": true,
                        "answered": true,
                        "attempts": attempt + 1,
                        "id": reply.get("id"),
                        "to": node_id,
                        "command": command,
                        "ok": ok,
                        "result": reply.get("result"),
                        "error": reply.get("error"),
                        "rssi_dbm": reply.pointer("/_mesh/rssi_dbm"),
                    });
                    return Ok(if ok {
                        ToolResult::ok(out.to_string())
                    } else {
                        ToolResult::err(out.to_string())
                    });
                }
            }
        }
        Ok(ToolResult::err(
            json!({
                "sent": true,
                "answered": false,
                "attempts": wait.retries + 1,
                "to": node_id,
                "command": command,
                "error": format!(
                    "no reply from {node_id} after {} attempt(s) of {} ms each — the node may be \
                     out of range, powered off, or the frames collided; the command may or may \
                     not have executed",
                    wait.retries + 1,
                    wait.timeout_ms
                ),
            })
            .to_string(),
        ))
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
        // Shared with the `GET /api/v1/mesh/status` gateway route (one SSOT).
        let out = mesh_supervisor::status_json(&self.world);
        Ok(ToolResult::ok(out.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_memory::world::{Origin, WorldMemory};
    use std::sync::Mutex;

    /// A mesh that answers from the `answer_from`-th send onward (1-based) by
    /// writing the node's reply into world memory the way the gateway bridge
    /// does, and drops every send before that — a lost frame or a lost reply
    /// look identical to the host, and that is the point.
    struct LossySink {
        world: Arc<WorldMemory>,
        sent: Mutex<Vec<NodeCommand>>,
        answer_from: usize,
    }

    #[async_trait]
    impl CommandSink for LossySink {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            let n = {
                let mut s = self.sent.lock().unwrap();
                s.push(cmd.clone());
                s.len()
            };
            if n >= self.answer_from {
                self.world
                    .observe_as(
                        &format!("mesh.{}.cmd_result", cmd.to),
                        json!({
                            "id": cmd.id, "node_id": cmd.to, "ok": true,
                            "result": "{\"active\":[[3,1.0]],\"applied\":1}",
                            "type": "cmd_result", "_mesh": {"rssi_dbm": -51}
                        }),
                        n as u64,
                        n as u64,
                        obc_spine::lora_gateway::SOURCE,
                        Origin::Observed,
                    )
                    .unwrap();
            }
            Ok(())
        }
    }

    fn tool(answer_from: usize, retries: u32) -> (MeshCommandTool, Arc<LossySink>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let sink = Arc::new(LossySink {
            world: Arc::clone(&world),
            sent: Mutex::new(Vec::new()),
            answer_from,
        });
        let t = MeshCommandTool::new(Arc::clone(&sink) as Arc<dyn CommandSink>).with_reply_wait(
            ReplyWait {
                world,
                timeout_ms: 250,
                retries,
            },
        );
        (t, sink)
    }

    fn descend_args() -> Value {
        json!({ "node_id": "n1", "command": "descend", "args": { "m": [[3, 1.0]] } })
    }

    #[tokio::test]
    async fn a_lost_first_frame_is_resent_and_the_answer_reported_with_the_attempt_count() {
        let (t, sink) = tool(2, 2);
        let res = t.execute(descend_args()).await.unwrap();
        assert!(res.is_ok(), "{}", res.output());
        let v: Value = serde_json::from_str(res.output()).unwrap();
        assert_eq!(v["answered"], json!(true));
        assert_eq!(v["attempts"], json!(2));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["rssi_dbm"], json!(-51));
        let sent = sink.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_ne!(sent[0].id, sent[1].id, "every attempt carries its own id");
        assert!(
            sent[1].id.starts_with(&sent[0].id),
            "…derived from the first"
        );
        assert_eq!(sent[0].args, sent[1].args, "same idempotent command");
    }

    #[tokio::test]
    async fn silence_after_every_attempt_is_an_error_not_a_sent_true() {
        let (t, sink) = tool(usize::MAX, 2);
        let res = t.execute(descend_args()).await.unwrap();
        assert!(!res.is_ok());
        let v: Value = serde_json::from_str(res.error.as_deref().unwrap()).unwrap();
        assert_eq!(v["answered"], json!(false));
        assert_eq!(v["attempts"], json!(3));
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("may or may not have executed"));
        assert_eq!(sink.sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_command_that_is_not_safe_to_repeat_is_sent_once() {
        let (t, sink) = tool(usize::MAX, 2);
        let res = t
            .execute(
                json!({ "node_id": "n1", "command": "set_reflex_rules", "args": { "rules": [] } }),
            )
            .await
            .unwrap();
        assert!(res.is_ok());
        let v: Value = serde_json::from_str(res.output()).unwrap();
        assert_eq!(v["sent"], json!(true));
        assert!(
            v.get("answered").is_none(),
            "no reply wait for a non-idempotent command"
        );
        assert_eq!(sink.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_refusal_from_the_node_comes_back_as_an_error_with_the_reason() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        struct Refuser(Arc<WorldMemory>);
        #[async_trait]
        impl CommandSink for Refuser {
            async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
                self.0
                    .observe_as(
                        &format!("mesh.{}.cmd_result", cmd.to),
                        json!({ "id": cmd.id, "ok": false, "error": "descend refused: slot 16 out of range (max 15)",
                                "type": "cmd_result", "_mesh": {"rssi_dbm": -50} }),
                        1, 1, obc_spine::lora_gateway::SOURCE, Origin::Observed,
                    )
                    .unwrap();
                Ok(())
            }
        }
        let t = MeshCommandTool::new(Arc::new(Refuser(Arc::clone(&world)))).with_reply_wait(
            ReplyWait {
                world,
                timeout_ms: 250,
                retries: 2,
            },
        );
        let res = t
            .execute(json!({ "node_id": "n1", "command": "descend", "args": { "m": [[3, 0.5]] } }))
            .await
            .unwrap();
        assert!(!res.is_ok());
        let v: Value = serde_json::from_str(res.error.as_deref().unwrap()).unwrap();
        assert_eq!(v["answered"], json!(true));
        assert_eq!(
            v["attempts"],
            json!(1),
            "a refusal is an answer, not silence"
        );
        assert!(v["error"].as_str().unwrap().contains("slot 16"));
    }

    #[tokio::test]
    async fn mesh_status_summarizes_node_health() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex", "rssi_dbm": -80 }),
                1_000,
                1_000,
                obc_spine::lora_gateway::SOURCE,
                Origin::Observed, // a node is a node because a radio was heard
            )
            .unwrap();
        world
            .observe(
                "mesh.n1.health",
                json!({ "status": "offline" }),
                1_000,
                1_000,
                "t",
            )
            .unwrap();
        world
            .observe(
                "mesh.n1.escalation",
                json!({ "status": "escalated" }),
                1_000,
                1_000,
                "t",
            )
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

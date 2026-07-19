//! World-memory tool — record and query time-valid facts about the world (Phase 18).
//!
//! Exposes [`crate::memory::world::WorldMemory`] to the agent so it (and the
//! subsystem suites) can `observe` real-world state and recall it with
//! `current`/`at`/`history`. Writes are non-destructive and time-valid.

use crate::memory::world::{Origin, WorldMemory};
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Provenance stamped on every fact written through this tool.
///
/// Deliberately a constant, not a parameter: this is the agent's own write path, so
/// anything it records here is an *assertion*, whatever the agent believes the ultimate
/// source to be. Consumers that gate on provenance (e.g. the mesh supervisor, which
/// only treats `lora-gateway` facts as evidence a radio exists) rely on an agent being
/// unable to claim otherwise.
pub const AGENT_SOURCE: &str = "agent";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tool: record and query the temporal world model.
pub struct WorldMemoryTool {
    mem: Arc<WorldMemory>,
}

impl WorldMemoryTool {
    /// Build a tool over a shared world-memory store.
    pub fn new(mem: Arc<WorldMemory>) -> Self {
        Self { mem }
    }
}

#[async_trait]
impl Tool for WorldMemoryTool {
    fn name(&self) -> &str {
        "world_memory"
    }

    fn description(&self) -> &str {
        "Record and query time-valid facts about the physical world (rooms, \
         devices, sensors, subjects). Actions: 'observe' (record a fact for an \
         entity), 'current' (latest value), 'at' (value at a past timestamp), \
         'history' (full trail), 'entities' (list known entities). Use this to \
         remember and recall the real-world state over time. Provenance is stamped \
         automatically and cannot be set by the caller — to attribute a reading to a \
         device, put that in the value (e.g. {\"reported_by\":\"pir\",\"c\":21.5})."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["observe", "current", "at", "history", "entities"]
                },
                "entity": {
                    "type": "string",
                    "description": "The thing the fact is about (e.g. 'living_room.temp', 'front_door.lock')."
                },
                "value": {
                    "description": "The value to record (any JSON), required for 'observe'."
                },
                "valid_from": {
                    "type": "integer",
                    "description": "When the fact became true (ms since epoch); defaults to now."
                },
                "ts": {
                    "type": "integer",
                    "description": "Timestamp (ms since epoch) for the 'at' query."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match action {
            "observe" => {
                let Some(entity) = args.get("entity").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::err("'observe' requires 'entity'"));
                };
                // `mesh.*` belongs to the LoRa gateway and the mesh supervisor: those
                // facts are perception (what a radio actually said) and derived health.
                // An agent note filed in there is read back as fleet state — a bench
                // run once had System 2 record an incident at `mesh.escalation_status`,
                // which the supervisor then tracked as a node, escalated, and alarmed
                // on forever. Notes about the mesh are welcome; they just don't get to
                // masquerade as the mesh.
                if entity.starts_with("mesh.") {
                    return Ok(ToolResult::err(
                        "'mesh.*' is reserved for mesh perception written by the LoRa \
                         gateway and supervisor — writing there would be read back as \
                         real node state. Use `mesh_status` to read the mesh, and \
                         `record_incident` to write down what you concluded (it files \
                         under 'incident.<subject>' for you).",
                    ));
                }
                let mut value = args.get("value").cloned().unwrap_or(Value::Null);
                let now = now_ms();
                let valid_from = args
                    .get("valid_from")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(now);
                // Provenance is stamped here, never taken from the caller. It used to be
                // a tool parameter, which meant an agent could declare its own writes to
                // be anything — including `lora-gateway`, the source the mesh supervisor
                // trusts to decide a radio exists. Self-declared provenance is not
                // provenance (bench audit, 2026-07-17).
                //
                // The old parameter also conflated two different things: *provenance*
                // (who wrote this fact — always the agent, here) and *attribution* (who
                // the agent claims told it, e.g. a PIR). Attribution is content, so a
                // caller-supplied `source` is folded into the value as `reported_by`
                // rather than silently dropped.
                if let Some(claimed) = args.get("source").and_then(|v| v.as_str()) {
                    if let Value::Object(ref mut m) = value {
                        m.entry("reported_by")
                            .or_insert_with(|| Value::String(claimed.to_string()));
                    }
                }
                let fact = self.mem.observe_as(
                    entity,
                    value,
                    valid_from,
                    now,
                    AGENT_SOURCE,
                    Origin::Asserted,
                )?;
                Ok(ToolResult::ok(serde_json::to_string(&fact)?))
            }
            "current" => {
                let Some(entity) = args.get("entity").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::err("'current' requires 'entity'"));
                };
                match self.mem.current(entity)? {
                    Some(f) => Ok(ToolResult::ok(serde_json::to_string(&f)?)),
                    None => Ok(ToolResult::ok(format!("No current fact for '{entity}'"))),
                }
            }
            "at" => {
                let Some(entity) = args.get("entity").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::err("'at' requires 'entity'"));
                };
                let Some(ts) = args.get("ts").and_then(|v| v.as_u64()) else {
                    return Ok(ToolResult::err("'at' requires 'ts' (ms since epoch)"));
                };
                match self.mem.at(entity, ts)? {
                    Some(f) => Ok(ToolResult::ok(serde_json::to_string(&f)?)),
                    None => Ok(ToolResult::ok(format!("No fact for '{entity}' at {ts}"))),
                }
            }
            "history" => {
                let Some(entity) = args.get("entity").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::err("'history' requires 'entity'"));
                };
                let hist = self.mem.history(entity)?;
                Ok(ToolResult::ok(serde_json::to_string(&hist)?))
            }
            "entities" => {
                let es = self.mem.entities()?;
                Ok(ToolResult::ok(serde_json::to_string(&es)?))
            }
            other => Ok(ToolResult::err(format!("Unknown action: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> WorldMemoryTool {
        WorldMemoryTool::new(Arc::new(WorldMemory::open_in_memory().unwrap()))
    }

    #[tokio::test]
    async fn observing_into_the_reserved_mesh_namespace_is_refused() {
        // Bench regression, 2026-07-17: a System 2 note at `mesh.escalation_status` was
        // read back by the mesh supervisor as a node and escalated as lost.
        let t = tool();
        let r = t
            .execute(json!({
                "action": "observe",
                "entity": "mesh.escalation_status",
                "value": { "status": "critical" }
            }))
            .await
            .unwrap();
        assert!(!r.success, "mesh.* is reserved for mesh perception");
        // The reason rides in `error`, not `output` — a refusal has to say why, or the
        // caller just sees `success: false` and guesses (bench, 2026-07-17).
        let why = r.error.unwrap_or_default();
        assert!(why.contains("incident."), "points at a safe namespace: {why}");

        // A note *about* the mesh, filed outside the namespace, is fine.
        let ok = t
            .execute(json!({
                "action": "observe",
                "entity": "incident.obc-esp32-s3-001",
                "value": { "status": "presumed lost" }
            }))
            .await
            .unwrap();
        assert!(ok.success);
    }

    #[tokio::test]
    async fn an_agent_cannot_forge_the_provenance_of_its_own_write() {
        // The mesh supervisor decides a radio exists by trusting facts sourced
        // `lora-gateway`. If the agent could declare its own source, it could
        // manufacture a node — self-declared provenance is not provenance.
        let t = tool();
        let r = t
            .execute(json!({
                "action": "observe",
                "entity": "incident.n1",
                "value": { "status": "presumed lost" },
                "source": "lora-gateway"
            }))
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains(r#""source":"agent""#), "stamped, not claimed: {}", r.output);
        assert!(!r.output.contains("\"source\":\"lora-gateway\""), "the claim is not honoured");
    }

    #[tokio::test]
    async fn a_claimed_source_is_kept_as_attribution_in_the_value() {
        // Provenance (who wrote it) and attribution (who the agent says told it) are
        // different things. The write is the agent's; the PIR claim is content.
        let t = tool();
        let r = t
            .execute(json!({
                "action": "observe",
                "entity": "room.motion",
                "value": { "motion": true },
                "source": "pir"
            }))
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains(r#""source":"agent""#), "provenance is the agent");
        assert!(r.output.contains(r#""reported_by":"pir""#), "attribution preserved: {}", r.output);
    }

    #[tokio::test]
    async fn observe_then_current_roundtrips() {
        let t = tool();
        let r = t
            .execute(
                json!({"action": "observe", "entity": "room.temp", "value": 21.5, "source": "pir"}),
            )
            .await
            .unwrap();
        assert!(r.success);
        let cur = t
            .execute(json!({"action": "current", "entity": "room.temp"}))
            .await
            .unwrap();
        assert!(cur.output.contains("21.5"));
        assert!(cur.output.contains("\"entity\":\"room.temp\""));
    }

    #[tokio::test]
    async fn current_missing_entity_message() {
        let t = tool();
        let r = t
            .execute(json!({"action": "current", "entity": "ghost"}))
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.output.contains("No current fact"));
    }

    #[tokio::test]
    async fn observe_requires_entity() {
        let t = tool();
        let r = t.execute(json!({"action": "observe"})).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn at_query_returns_time_correct_value() {
        let t = tool();
        t.execute(json!({"action":"observe","entity":"door","value":"locked","valid_from":1000}))
            .await
            .unwrap();
        t.execute(json!({"action":"observe","entity":"door","value":"unlocked","valid_from":2000}))
            .await
            .unwrap();
        let past = t
            .execute(json!({"action":"at","entity":"door","ts":1500}))
            .await
            .unwrap();
        assert!(past.output.contains("locked"));
        let entities = t.execute(json!({"action":"entities"})).await.unwrap();
        assert!(entities.output.contains("door"));
    }

    #[tokio::test]
    async fn unknown_action_errors() {
        let t = tool();
        let r = t.execute(json!({"action": "frobnicate"})).await.unwrap();
        assert!(!r.success);
    }
}

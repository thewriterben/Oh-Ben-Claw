//! World-memory tool — record and query time-valid facts about the world (Phase 18).
//!
//! Exposes [`crate::memory::world::WorldMemory`] to the agent so it (and the
//! subsystem suites) can `observe` real-world state and recall it with
//! `current`/`at`/`history`. Writes are non-destructive and time-valid.

use crate::memory::world::WorldMemory;
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
         remember and recall the real-world state over time."
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
                },
                "source": {
                    "type": "string",
                    "description": "Who reported the fact (default 'agent')."
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
                let value = args.get("value").cloned().unwrap_or(Value::Null);
                let now = now_ms();
                let valid_from = args
                    .get("valid_from")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(now);
                let source = args
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent");
                let fact = self.mem.observe(entity, value, valid_from, now, source)?;
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

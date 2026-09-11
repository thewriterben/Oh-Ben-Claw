//! `memory` tool — the agent's two bounded note files (parity item 4).
//!
//! Until 2026-09-11 this was a process-local `HashMap` whose description
//! promised a "persistent key-value store"; nothing was ever written to disk
//! and every restart forgot everything. It now fronts
//! [`obc_memory::notes::Notes`]: `MEMORY.md` (working notes, 2,200 chars) and
//! `USER.md` (the operator, 1,375 chars), both shown to the model at the top
//! of every prompt. Gate it like any writing tool: `[autonomy] always_ask =
//! ["memory"]` makes each write wait for the operator.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_memory::notes::{Notes, Target, MEMORY_LIMIT, USER_LIMIT};
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: curate the two note files.
pub struct MemoryTool {
    notes: Arc<Notes>,
}

impl MemoryTool {
    pub fn new(notes: Arc<Notes>) -> Self {
        Self { notes }
    }

    /// The notes at their default location (`<data dir>/notes/`).
    pub fn in_default_dir() -> anyhow::Result<Self> {
        Ok(Self::new(Arc::new(Notes::open(Notes::default_dir())?)))
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Your two note files, shown to you at the start of every conversation: \
         target `memory` = working notes about this machine, the bench and ongoing \
         work (2,200 characters); target `user` = who the operator is and how they \
         want things done (1,375 characters). Actions: list, add, replace, remove. \
         One short factual line per entry. When a file is full, replace or remove \
         an entry before adding. Not for sensor facts (world_memory) or transcripts."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "replace", "remove"],
                    "description": "What to do."
                },
                "target": {
                    "type": "string",
                    "enum": ["memory", "user"],
                    "description": "Which file (required for add, replace, remove; list shows both when omitted)."
                },
                "text": {
                    "type": "string",
                    "description": "The entry text (add, replace). One line."
                },
                "index": {
                    "type": "integer",
                    "description": "1-based entry number (replace, remove), as shown by list."
                }
            },
            "required": ["action"]
        })
    }

    fn risk_class(&self) -> RiskClass {
        // Writes a file the model reads back every turn: not physical, but not
        // safely replayable either (a replayed `add` duplicates an entry).
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(s) => match Target::parse(s) {
                Some(t) => Some(t),
                None => {
                    return Ok(ToolResult::err(format!(
                        "unknown target '{s}': use memory or user"
                    )))
                }
            },
            None => None,
        };
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let outcome = match action {
            "list" => {
                let mut out = String::new();
                for t in match target {
                    Some(t) => vec![t],
                    None => vec![Target::User, Target::Memory],
                } {
                    let entries = self.notes.entries(t);
                    let used = self.notes.read(t).chars().count();
                    out.push_str(&format!(
                        "{} ({} entries, {}/{} chars)\n",
                        t.file_name(),
                        entries.len(),
                        used,
                        t.limit()
                    ));
                    for (i, e) in entries.iter().enumerate() {
                        out.push_str(&format!("  {}. {}\n", i + 1, e));
                    }
                }
                return Ok(ToolResult::ok(out.trim_end().to_string()));
            }
            "add" => match target {
                Some(t) => self.notes.add(t, text),
                None => return Ok(ToolResult::err("add needs a target (memory or user)")),
            },
            "replace" => match target {
                Some(t) => self.notes.replace(t, index, text),
                None => return Ok(ToolResult::err("replace needs a target, an index and text")),
            },
            "remove" => match target {
                Some(t) => self.notes.remove(t, index),
                None => return Ok(ToolResult::err("remove needs a target and an index")),
            },
            other => {
                return Ok(ToolResult::err(format!(
                    "unknown action '{other}': use list, add, replace or remove"
                )))
            }
        };
        Ok(match outcome {
            Ok(u) => ToolResult::ok(format!(
                "{} now {} entries, {}/{} chars",
                target.map(|t| t.file_name()).unwrap_or("notes"),
                u.entries,
                u.used,
                u.limit
            )),
            Err(e) => ToolResult::err(e.to_string()),
        })
    }
}

/// So callers can say the limits without importing obc-memory.
pub const LIMITS: (usize, usize) = (MEMORY_LIMIT, USER_LIMIT);

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> MemoryTool {
        let dir = std::env::temp_dir().join(format!("obc-memory-tool-{}", uuid::Uuid::new_v4()));
        MemoryTool::new(Arc::new(Notes::open(dir).unwrap()))
    }

    #[tokio::test]
    async fn add_list_replace_remove_through_the_tool() {
        let t = tool();
        let r = t
            .execute(json!({"action": "add", "target": "user", "text": "Name: Benji"}))
            .await
            .unwrap();
        assert!(r.success, "{:?}", r.error);
        assert_eq!(r.output, "USER.md now 1 entries, 14/1375 chars");
        t.execute(json!({"action": "add", "target": "memory", "text": "Ollama on :11434"}))
            .await
            .unwrap();
        let list = t.execute(json!({"action": "list"})).await.unwrap().output;
        assert!(
            list.starts_with(
                "USER.md (1 entries, 14/1375 chars)\n  1. Name: Benji\nMEMORY.md (1 entries"
            ),
            "{list}"
        );
        let r = t
            .execute(json!({"action": "replace", "target": "memory", "index": 1, "text": "Ollama serves on port 11434"}))
            .await
            .unwrap();
        assert!(r.success);
        let r = t
            .execute(json!({"action": "remove", "target": "memory", "index": 5}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("has no entry 5"));
        let r = t
            .execute(json!({"action": "add", "target": "world", "text": "x"}))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[test]
    fn the_tool_is_named_memory_and_is_not_replayable() {
        let t = tool();
        assert_eq!(t.name(), "memory");
        assert!(!t.risk_class().reversible);
        assert!(!t.risk_class().physical);
    }
}

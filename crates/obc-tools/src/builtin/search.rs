//! `search_sessions` tool — full-text search over every past conversation
//! (parity item 4, 2026-09-11). Backed by the FTS5 table `messages_fts` that
//! `MemoryStore` keeps beside `messages`; ranked by BM25, returned with a
//! snippet, the session and the timestamp so the agent can quote where it
//! learned something.

use crate::traits::{Tool, ToolResult};
use async_trait::async_trait;
use obc_memory::MemoryStore;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: search past conversations.
pub struct SearchSessionsTool {
    memory: Arc<MemoryStore>,
}

impl SearchSessionsTool {
    pub fn new(memory: Arc<MemoryStore>) -> Self {
        Self { memory }
    }
}

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 25;

#[async_trait]
impl Tool for SearchSessionsTool {
    fn name(&self) -> &str {
        "search_sessions"
    }

    fn description(&self) -> &str {
        "Full-text search over every past conversation with the operator, across \
         all sessions, ranked by relevance. Use it when asked what was said, decided \
         or measured earlier, or to recall a path, number or name from a previous \
         session. Returns short snippets with the session and time; ask for a \
         session's messages if you need the full exchange."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Words to look for (any of them; exact words, not phrases)."
                },
                "limit": {
                    "type": "integer",
                    "description": "How many hits to return (default 8, max 25)."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);
        let hits = match self.memory.search_messages(query, limit) {
            Ok(h) => h,
            Err(e) => return Ok(ToolResult::err(format!("search failed: {e}"))),
        };
        if hits.is_empty() {
            return Ok(ToolResult::ok(format!("No past message mentions: {query}")));
        }
        let mut out = format!("{} hit(s) for \"{query}\":\n", hits.len());
        for h in &hits {
            let session = if h.session_title.trim().is_empty() {
                h.message.session_id.clone()
            } else {
                h.session_title.clone()
            };
            out.push_str(&format!(
                "- [{}] {} {} #{}: {}\n",
                session,
                h.message.created_at.format("%Y-%m-%d %H:%M"),
                h.message.role,
                h.message.id,
                h.snippet.replace('\n', " ")
            ));
        }
        Ok(ToolResult::ok(out.trim_end().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_memory::ChatRole;

    #[tokio::test]
    async fn finds_what_was_said_in_another_session() {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let a = memory.create_session("bench day").unwrap();
        memory
            .append_message(
                &a,
                ChatRole::User,
                "the printer nozzle should be 203C for PETG",
            )
            .unwrap();
        let b = memory.create_session("other").unwrap();
        memory
            .append_message(&b, ChatRole::Assistant, "the LoRa gateway is on COM3")
            .unwrap();
        let t = SearchSessionsTool::new(memory);
        let r = t
            .execute(json!({"query": "nozzle temperature"}))
            .await
            .unwrap();
        assert!(r.success);
        assert!(
            r.output.contains("[bench day]") && r.output.contains("203C"),
            "{}",
            r.output
        );
        assert!(!r.output.contains("COM3"));
        let r = t.execute(json!({"query": "spectrometer"})).await.unwrap();
        assert_eq!(r.output, "No past message mentions: spectrometer");
        let r = t.execute(json!({"query": "  "})).await;
        assert!(r.is_err());
    }
}

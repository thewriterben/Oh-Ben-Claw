//! Memory tool — store and retrieve notes in a persistent key-value store.

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Tool: store and retrieve notes in a persistent in-memory key-value store.
pub struct MemoryTool {
    store: Arc<RwLock<HashMap<String, String>>>,
}

impl MemoryTool {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Store and retrieve notes or facts in a persistent key-value store. \
         Use this to remember information across turns in a conversation, \
         such as user preferences, task state, or important facts. \
         Notes persist for the lifetime of the current session."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The memory operation to perform.",
                    "enum": ["set", "get", "delete", "list", "clear"]
                },
                "key": {
                    "type": "string",
                    "description": "The key to store or retrieve (required for 'set', 'get', 'delete')."
                },
                "value": {
                    "type": "string",
                    "description": "The value to store (required for 'set')."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?
            .to_string();

        match action.as_str() {
            "set" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?
                    .to_string();
                let value = args
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'value' parameter"))?
                    .to_string();
                self.store.write().insert(key.clone(), value);
                Ok(ToolResult::ok(format!("Stored key '{}'", key)))
            }

            "get" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?;
                match self.store.read().get(key) {
                    Some(value) => Ok(ToolResult::ok(value.clone())),
                    None => Ok(ToolResult::ok(format!("Key '{}' not found", key))),
                }
            }

            "delete" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?
                    .to_string();
                let removed = self.store.write().remove(&key).is_some();
                Ok(ToolResult::ok(if removed {
                    format!("Deleted key '{}'", key)
                } else {
                    format!("Key '{}' not found", key)
                }))
            }

            "list" => {
                let keys: Vec<String> = self.store.read().keys().cloned().collect();
                if keys.is_empty() {
                    Ok(ToolResult::ok("No keys stored"))
                } else {
                    let mut sorted = keys;
                    sorted.sort();
                    Ok(ToolResult::ok(sorted.join("\n")))
                }
            }

            "clear" => {
                let count = self.store.read().len();
                self.store.write().clear();
                Ok(ToolResult::ok(format!("Cleared {} keys", count)))
            }

            other => Ok(ToolResult::err(format!("Unknown action: {}", other))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_and_get() {
        let tool = MemoryTool::new();
        tool.execute(json!({"action": "set", "key": "name", "value": "Ben"}))
            .await
            .unwrap();
        let result = tool
            .execute(json!({"action": "get", "key": "name"}))
            .await
            .unwrap();
        assert_eq!(result.output, "Ben");
    }

    #[tokio::test]
    async fn get_missing_key() {
        let tool = MemoryTool::new();
        let result = tool
            .execute(json!({"action": "get", "key": "missing"}))
            .await
            .unwrap();
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn list_keys() {
        let tool = MemoryTool::new();
        tool.execute(json!({"action": "set", "key": "a", "value": "1"}))
            .await
            .unwrap();
        tool.execute(json!({"action": "set", "key": "b", "value": "2"}))
            .await
            .unwrap();
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.output.contains("a"));
        assert!(result.output.contains("b"));
    }

    #[tokio::test]
    async fn clear_store() {
        let tool = MemoryTool::new();
        tool.execute(json!({"action": "set", "key": "x", "value": "1"}))
            .await
            .unwrap();
        tool.execute(json!({"action": "clear"})).await.unwrap();
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.output.contains("No keys"));
    }
}

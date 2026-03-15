//! Delegation tools — the four tools the orchestrator uses to manage sub-agents.
//!
//! These tools are registered in the orchestrator's tool registry and allow the
//! LLM to spawn, delegate to, list, and stop sub-agents dynamically.
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `spawn_agent` | Spawn a new named sub-agent with a role and optional config |
//! | `delegate_task` | Send a task to one or more sub-agents and get the result |
//! | `list_agents` | List all sub-agents and their current status |
//! | `stop_agent` | Stop a sub-agent and remove it from the active pool |

use crate::agent::pool::{AgentPool, SubAgentSpec};
use crate::config::ProviderConfig;
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

// ── SpawnAgentTool ────────────────────────────────────────────────────────────

/// Spawn a new named sub-agent with a role and optional configuration.
pub struct SpawnAgentTool {
    pool: AgentPool,
}

impl SpawnAgentTool {
    pub fn new(pool: AgentPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn name(&self) -> &str {
        "spawn_agent"
    }

    fn description(&self) -> &str {
        "Spawn a new specialized sub-agent with a given name and role. \
         The sub-agent will have its own tool registry and can be delegated tasks. \
         Returns the agent name on success."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for this sub-agent (e.g., 'researcher', 'coder', 'planner')"
                },
                "role": {
                    "type": "string",
                    "description": "Natural-language description of this agent's role and expertise"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Optional custom system prompt. If not provided, one is generated from the role."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of tool names to give this agent. Empty means all default tools. Use ['none'] for no tools."
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override (e.g., 'gpt-4o', 'claude-3-5-sonnet-20241022')"
                },
                "provider": {
                    "type": "string",
                    "description": "Optional provider override (e.g., 'openai', 'anthropic', 'ollama')"
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Maximum tool-use iterations for this sub-agent (default: 8)"
                }
            },
            "required": ["name", "role"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("name is required"))?;
        let role = args["role"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("role is required"))?;

        let mut spec = SubAgentSpec::new(name, role);

        if let Some(prompt) = args["system_prompt"].as_str() {
            spec.system_prompt = prompt.to_string();
        }

        if let Some(tools) = args["tools"].as_array() {
            spec.tools = tools
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        if let Some(max_iter) = args["max_iterations"].as_u64() {
            spec.max_iterations = max_iter as usize;
        }

        // Build a provider override if model or provider is specified
        if args["model"].is_string() || args["provider"].is_string() {
            let mut provider = ProviderConfig::default();
            if let Some(model) = args["model"].as_str() {
                provider.model = model.to_string();
            }
            if let Some(p) = args["provider"].as_str() {
                provider.name = p.to_string();
            }
            spec.provider = Some(provider);
        }

        match self.pool.spawn(spec) {
            Ok(()) => Ok(ToolResult::ok(format!(
                "Sub-agent '{}' spawned successfully with role: {}",
                name, role
            ))),
            Err(e) => Ok(ToolResult::err(format!("Failed to spawn agent '{}': {}", name, e))),
        }
    }
}

// ── DelegateTaskTool ──────────────────────────────────────────────────────────

/// Delegate a task to one or more sub-agents and return the result(s).
pub struct DelegateTaskTool {
    pool: AgentPool,
    /// The current session ID — used as the base for sub-agent sessions.
    session_id: Arc<Mutex<String>>,
}

impl DelegateTaskTool {
    pub fn new(pool: AgentPool, session_id: Arc<Mutex<String>>) -> Self {
        Self { pool, session_id }
    }
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Delegate a task to a named sub-agent and return its response. \
         For parallel delegation, provide multiple agent names and tasks. \
         The sub-agent will use its own tool registry and system prompt."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Name of the sub-agent to delegate to (for single delegation)"
                },
                "task": {
                    "type": "string",
                    "description": "The task or question to send to the sub-agent"
                },
                "parallel": {
                    "type": "array",
                    "description": "For parallel delegation: array of {agent, task} objects",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent": { "type": "string" },
                            "task": { "type": "string" }
                        },
                        "required": ["agent", "task"]
                    }
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let session_id = self.session_id.lock().await.clone();

        // Parallel delegation mode
        if let Some(parallel) = args["parallel"].as_array() {
            if parallel.is_empty() {
                return Ok(ToolResult::err("parallel array is empty".to_string()));
            }

            let tasks: Vec<(String, String)> = parallel
                .iter()
                .filter_map(|item| {
                    let agent = item["agent"].as_str()?.to_string();
                    let task = item["task"].as_str()?.to_string();
                    Some((agent, task))
                })
                .collect();

            if tasks.is_empty() {
                return Ok(ToolResult::err(
                    "No valid {agent, task} pairs found in parallel array".to_string(),
                ));
            }

            let agent_names: Vec<String> = tasks.iter().map(|(a, _)| a.clone()).collect();
            let results = self.pool.delegate_parallel(tasks, &session_id).await;

            let mut output = String::new();
            for (name, result) in &results {
                match result {
                    Ok(response) => {
                        output.push_str(&format!("## {} result:\n{}\n\n", name, response));
                    }
                    Err(e) => {
                        output.push_str(&format!("## {} error:\n{}\n\n", name, e));
                    }
                }
            }

            let all_ok = results.iter().all(|(_, r)| r.is_ok());
            tracing::info!(
                agents = ?agent_names,
                success = all_ok,
                "Parallel delegation complete"
            );

            return Ok(ToolResult::ok(output.trim_end().to_string()));
        }

        // Single delegation mode
        let agent_name = match args["agent"].as_str() {
            Some(n) => n,
            None => return Ok(ToolResult::err("'agent' is required for single delegation".to_string())),
        };
        let task = match args["task"].as_str() {
            Some(t) => t,
            None => return Ok(ToolResult::err("'task' is required".to_string())),
        };

        // Use a sub-session ID so the sub-agent's history doesn't pollute the main session
        let sub_session = format!("{}-{}", session_id, agent_name);

        tracing::info!(agent = %agent_name, task_len = task.len(), "Delegating task to sub-agent");

        match self.pool.delegate(agent_name, task, &sub_session).await {
            Ok(response) => Ok(ToolResult::ok(response)),
            Err(e) => Ok(ToolResult::err(format!(
                "Delegation to '{}' failed: {}",
                agent_name, e
            ))),
        }
    }
}

// ── ListAgentsTool ────────────────────────────────────────────────────────────

/// List all sub-agents and their current status.
pub struct ListAgentsTool {
    pool: AgentPool,
}

impl ListAgentsTool {
    pub fn new(pool: AgentPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &str {
        "list_agents"
    }

    fn description(&self) -> &str {
        "List all spawned sub-agents, their roles, current status (idle/busy/stopped), \
         tool count, and task completion statistics."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_stopped": {
                    "type": "boolean",
                    "description": "Whether to include stopped agents in the list (default: false)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let include_stopped = args["include_stopped"].as_bool().unwrap_or(false);
        let agents = self.pool.list();

        let filtered: Vec<_> = agents
            .iter()
            .filter(|a| {
                include_stopped
                    || a.status != crate::agent::pool::SubAgentStatus::Stopped
            })
            .collect();

        if filtered.is_empty() {
            return Ok(ToolResult::ok("No active sub-agents in the pool.".to_string()));
        }

        let mut output = format!("{} sub-agent(s) in pool:\n\n", filtered.len());
        for agent in &filtered {
            output.push_str(&format!(
                "- **{}** [{:?}] — {}\n  Tools: {} | Completed: {} | Failed: {}\n",
                agent.name,
                agent.status,
                agent.role,
                agent.tool_count,
                agent.tasks_completed,
                agent.tasks_failed,
            ));
        }

        Ok(ToolResult::ok(output.trim_end().to_string()))
    }
}

// ── StopAgentTool ─────────────────────────────────────────────────────────────

/// Stop a sub-agent and mark it as stopped.
pub struct StopAgentTool {
    pool: AgentPool,
}

impl StopAgentTool {
    pub fn new(pool: AgentPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Tool for StopAgentTool {
    fn name(&self) -> &str {
        "stop_agent"
    }

    fn description(&self) -> &str {
        "Stop a named sub-agent. Stopped agents cannot accept new tasks but their \
         history remains in memory. A new agent with the same name can be spawned afterwards."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the sub-agent to stop"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let name = match args["name"].as_str() {
            Some(n) => n,
            None => return Ok(ToolResult::err("'name' is required".to_string())),
        };

        match self.pool.stop(name) {
            Ok(()) => Ok(ToolResult::ok(format!("Sub-agent '{}' stopped.", name))),
            Err(e) => Ok(ToolResult::err(format!("Failed to stop '{}': {}", name, e))),
        }
    }
}

// ── Factory function ──────────────────────────────────────────────────────────

/// Build all four delegation tools and return them as a `Vec<Box<dyn Tool>>`.
///
/// These should be added to the orchestrator's tool registry at startup.
pub fn delegation_tools(pool: AgentPool, session_id: Arc<Mutex<String>>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SpawnAgentTool::new(pool.clone())),
        Box::new(DelegateTaskTool::new(pool.clone(), session_id)),
        Box::new(ListAgentsTool::new(pool.clone())),
        Box::new(StopAgentTool::new(pool)),
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::pool::AgentPool;
    use crate::config::ProviderConfig;
    use crate::memory::MemoryStore;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn make_pool() -> AgentPool {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        AgentPool::new(ProviderConfig::default(), memory)
    }

    fn make_session() -> Arc<Mutex<String>> {
        Arc::new(Mutex::new("test-session".to_string()))
    }

    #[tokio::test]
    async fn spawn_tool_creates_agent() {
        let pool = make_pool();
        let tool = SpawnAgentTool::new(pool.clone());
        let result = tool
            .execute(json!({"name": "tester", "role": "Run tests"}))
            .await
            .unwrap();
        assert!(result.is_ok());
        assert!(pool.exists("tester"));
    }

    #[tokio::test]
    async fn spawn_tool_duplicate_fails() {
        let pool = make_pool();
        let tool = SpawnAgentTool::new(pool.clone());
        tool.execute(json!({"name": "dup", "role": "Role A"}))
            .await
            .unwrap();
        let result = tool
            .execute(json!({"name": "dup", "role": "Role B"}))
            .await
            .unwrap();
        // Returns an error ToolResult, not a Rust error
        assert!(!result.is_ok());
    }

    #[tokio::test]
    async fn list_tool_empty_pool() {
        let pool = make_pool();
        let tool = ListAgentsTool::new(pool);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.is_ok());
        assert!(result.output().contains("No active sub-agents"));
    }

    #[tokio::test]
    async fn list_tool_shows_spawned_agents() {
        let pool = make_pool();
        let spawn_tool = SpawnAgentTool::new(pool.clone());
        spawn_tool
            .execute(json!({"name": "writer", "role": "Write content"}))
            .await
            .unwrap();

        let list_tool = ListAgentsTool::new(pool);
        let result = list_tool.execute(json!({})).await.unwrap();
        assert!(result.is_ok());
        assert!(result.output().contains("writer"));
    }

    #[tokio::test]
    async fn stop_tool_stops_agent() {
        let pool = make_pool();
        let spawn_tool = SpawnAgentTool::new(pool.clone());
        spawn_tool
            .execute(json!({"name": "stopper", "role": "To be stopped"}))
            .await
            .unwrap();

        let stop_tool = StopAgentTool::new(pool.clone());
        let result = stop_tool
            .execute(json!({"name": "stopper"}))
            .await
            .unwrap();
        assert!(result.is_ok());
        assert!(!pool.exists("stopper"));
    }

    #[tokio::test]
    async fn stop_tool_nonexistent_agent() {
        let pool = make_pool();
        let tool = StopAgentTool::new(pool);
        let result = tool.execute(json!({"name": "ghost"})).await.unwrap();
        assert!(!result.is_ok());
    }

    #[tokio::test]
    async fn delegate_tool_missing_agent_field() {
        let pool = make_pool();
        let session = make_session();
        let tool = DelegateTaskTool::new(pool, session);
        let result = tool
            .execute(json!({"task": "Do something"}))
            .await
            .unwrap();
        assert!(!result.is_ok());
    }

    #[tokio::test]
    async fn delegate_tool_nonexistent_agent() {
        let pool = make_pool();
        let session = make_session();
        let tool = DelegateTaskTool::new(pool, session);
        let result = tool
            .execute(json!({"agent": "ghost", "task": "Do something"}))
            .await
            .unwrap();
        assert!(!result.is_ok());
    }

    #[tokio::test]
    async fn delegation_tools_factory_returns_four_tools() {
        let pool = make_pool();
        let session = make_session();
        let tools = delegation_tools(pool, session);
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"delegate_task"));
        assert!(names.contains(&"list_agents"));
        assert!(names.contains(&"stop_agent"));
    }
}

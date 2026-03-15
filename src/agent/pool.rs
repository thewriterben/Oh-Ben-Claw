//! AgentPool — a registry of named sub-agents for multi-agent delegation.
//!
//! The `AgentPool` allows the orchestrator (the main Oh-Ben-Claw brain) to
//! spawn, manage, and delegate work to specialized sub-agents. Each sub-agent
//! has its own:
//!
//! - **Name and role** — a unique identifier and a natural-language role description
//! - **LLM provider** — can use a different model or provider than the orchestrator
//! - **Tool registry** — a filtered subset of the orchestrator's tools, or entirely new ones
//! - **Memory context** — shares the same `MemoryStore` but uses a dedicated session
//! - **System prompt** — specialized instructions for the sub-agent's role
//!
//! # Architecture
//!
//! ```text
//! Orchestrator (Oh-Ben-Claw brain)
//!     │
//!     ├─ spawn_agent("researcher", ...)  ──► AgentPool
//!     ├─ delegate_task("researcher", ...) ──► SubAgent::process()
//!     ├─ delegate_task("coder", ...)      ──► SubAgent::process()
//!     └─ list_agents()                   ──► [researcher, coder, ...]
//! ```

use crate::agent::{Agent, AgentHandle};
use crate::config::{AgentConfig, ProviderConfig};
use crate::memory::MemoryStore;
use crate::providers;
use crate::tools::{default_tools, Tool};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Sub-Agent Specification ───────────────────────────────────────────────────

/// The specification for a sub-agent — everything needed to spawn it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentSpec {
    /// Unique name for this sub-agent (e.g., "researcher", "coder", "planner").
    pub name: String,
    /// Natural-language description of this agent's role.
    pub role: String,
    /// System prompt for this sub-agent. If empty, a default is generated from `role`.
    #[serde(default)]
    pub system_prompt: String,
    /// LLM provider config. If not set, inherits from the orchestrator.
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
    /// Tool names to give this sub-agent. If empty, all default tools are given.
    /// Use `["none"]` to give the sub-agent no tools (pure reasoning only).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Maximum tool-use iterations for this sub-agent.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_max_iterations() -> usize {
    8
}

impl SubAgentSpec {
    /// Create a new spec with sensible defaults.
    pub fn new(name: impl Into<String>, role: impl Into<String>) -> Self {
        let name = name.into();
        let role = role.into();
        Self {
            system_prompt: format!(
                "You are {name}, a specialized AI sub-agent. Your role: {role}. \
                 Be concise, focused, and complete your assigned task thoroughly. \
                 Return a clear, structured result."
            ),
            name,
            role,
            provider: None,
            tools: Vec::new(),
            max_iterations: default_max_iterations(),
        }
    }

    /// Override the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Restrict to a specific set of tool names.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Use a specific provider config.
    pub fn with_provider(mut self, provider: ProviderConfig) -> Self {
        self.provider = Some(provider);
        self
    }
}

// ── Sub-Agent Entry ───────────────────────────────────────────────────────────

/// The status of a sub-agent in the pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    /// The sub-agent is idle and ready to accept tasks.
    Idle,
    /// The sub-agent is currently processing a task.
    Busy,
    /// The sub-agent has been stopped and cannot accept new tasks.
    Stopped,
}

/// A live sub-agent entry in the pool.
pub struct SubAgentEntry {
    pub spec: SubAgentSpec,
    pub handle: AgentHandle,
    pub status: SubAgentStatus,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
}

// ── AgentPool ─────────────────────────────────────────────────────────────────

/// A registry of named sub-agents for multi-agent delegation.
///
/// The pool is `Clone`-able and `Send + Sync` — it can be shared across
/// the gateway, CLI channel, and delegation tools.
#[derive(Clone)]
pub struct AgentPool {
    agents: Arc<Mutex<HashMap<String, SubAgentEntry>>>,
    /// The orchestrator's provider config — used as fallback for sub-agents.
    default_provider: ProviderConfig,
    /// Shared memory store — sub-agents get their own sessions within it.
    memory: Arc<MemoryStore>,
}

impl AgentPool {
    /// Create a new empty pool.
    pub fn new(default_provider: ProviderConfig, memory: Arc<MemoryStore>) -> Self {
        Self {
            agents: Arc::new(Mutex::new(HashMap::new())),
            default_provider,
            memory,
        }
    }

    /// Spawn a new sub-agent from a spec and add it to the pool.
    ///
    /// Returns an error if an agent with the same name already exists and is not stopped.
    pub fn spawn(&self, spec: SubAgentSpec) -> Result<()> {
        let mut agents = self.agents.lock().unwrap();

        // Check if an active agent with this name already exists
        if let Some(existing) = agents.get(&spec.name) {
            if existing.status != SubAgentStatus::Stopped {
                bail!(
                    "Sub-agent '{}' already exists and is {:?}. Stop it first.",
                    spec.name,
                    existing.status
                );
            }
        }

        // Build the provider
        let provider_config = spec.provider.clone().unwrap_or_else(|| self.default_provider.clone());
        let provider = providers::from_config(&provider_config)?;

        // Build the tool registry
        let all_tools = default_tools();
        let tools: Vec<Box<dyn Tool>> = if spec.tools.is_empty() {
            // Give all default tools
            all_tools
        } else if spec.tools == ["none"] {
            // No tools — pure reasoning
            Vec::new()
        } else {
            // Filter to the requested tool names
            let requested: std::collections::HashSet<&str> =
                spec.tools.iter().map(|s| s.as_str()).collect();
            all_tools
                .into_iter()
                .filter(|t| requested.contains(t.name()))
                .collect()
        };

        // Build the agent config
        let agent_config = AgentConfig {
            name: spec.name.clone(),
            system_prompt: spec.system_prompt.clone(),
            max_tool_iterations: spec.max_iterations,
        };

        // Build the agent
        let agent = Arc::new(Agent::new(
            agent_config,
            provider,
            Arc::clone(&self.memory),
            tools,
        ));

        let handle = AgentHandle::new(Arc::clone(&agent), provider_config);

        let entry = SubAgentEntry {
            spec: spec.clone(),
            handle,
            status: SubAgentStatus::Idle,
            tasks_completed: 0,
            tasks_failed: 0,
        };

        agents.insert(spec.name.clone(), entry);
        tracing::info!(agent = %spec.name, role = %spec.role, "Sub-agent spawned");
        Ok(())
    }

    /// Stop a sub-agent and mark it as stopped.
    pub fn stop(&self, name: &str) -> Result<()> {
        let mut agents = self.agents.lock().unwrap();
        match agents.get_mut(name) {
            None => bail!("Sub-agent '{}' not found", name),
            Some(entry) => {
                if entry.status == SubAgentStatus::Busy {
                    bail!("Sub-agent '{}' is currently busy — wait for it to finish first", name);
                }
                entry.status = SubAgentStatus::Stopped;
                tracing::info!(agent = %name, "Sub-agent stopped");
                Ok(())
            }
        }
    }

    /// Delegate a task to a named sub-agent and wait for the result.
    pub async fn delegate(&self, name: &str, task: &str, session_id: &str) -> Result<String> {
        // Get the handle — clone it so we can release the lock before awaiting
        let handle = {
            let mut agents = self.agents.lock().unwrap();
            match agents.get_mut(name) {
                None => bail!("Sub-agent '{}' not found", name),
                Some(entry) => {
                    if entry.status == SubAgentStatus::Stopped {
                        bail!("Sub-agent '{}' is stopped", name);
                    }
                    if entry.status == SubAgentStatus::Busy {
                        bail!("Sub-agent '{}' is currently busy", name);
                    }
                    entry.status = SubAgentStatus::Busy;
                    entry.handle.clone()
                }
            }
        };

        // Run the task
        let result = handle.process(session_id, task).await;

        // Update status and counters
        {
            let mut agents = self.agents.lock().unwrap();
            if let Some(entry) = agents.get_mut(name) {
                match &result {
                    Ok(_) => {
                        entry.tasks_completed += 1;
                    }
                    Err(_) => {
                        entry.tasks_failed += 1;
                    }
                }
                entry.status = SubAgentStatus::Idle;
            }
        }

        match result {
            Ok(response) => Ok(response.message),
            Err(e) => bail!("Sub-agent '{}' failed: {}", name, e),
        }
    }

    /// Delegate the same task to multiple sub-agents in parallel and collect results.
    pub async fn delegate_parallel(
        &self,
        tasks: Vec<(String, String)>, // (agent_name, task)
        session_id: &str,
    ) -> Vec<(String, Result<String>)> {
        let mut handles = Vec::new();

        for (name, task) in tasks {
            let pool = self.clone();
            let session = session_id.to_string();
            let handle = tokio::spawn(async move {
                let result = pool.delegate(&name, &task, &session).await;
                (name, result)
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(("unknown".to_string(), Err(anyhow::anyhow!("Task panicked: {}", e)))),
            }
        }
        results
    }

    /// List all sub-agents and their current status.
    pub fn list(&self) -> Vec<SubAgentInfo> {
        let agents = self.agents.lock().unwrap();
        agents
            .values()
            .map(|entry| SubAgentInfo {
                name: entry.spec.name.clone(),
                role: entry.spec.role.clone(),
                status: entry.status.clone(),
                tool_count: entry.handle.tool_count(),
                tasks_completed: entry.tasks_completed,
                tasks_failed: entry.tasks_failed,
            })
            .collect()
    }

    /// Get the number of active (non-stopped) sub-agents.
    pub fn active_count(&self) -> usize {
        let agents = self.agents.lock().unwrap();
        agents
            .values()
            .filter(|e| e.status != SubAgentStatus::Stopped)
            .count()
    }

    /// Check if a named sub-agent exists and is active.
    pub fn exists(&self, name: &str) -> bool {
        let agents = self.agents.lock().unwrap();
        agents
            .get(name)
            .map(|e| e.status != SubAgentStatus::Stopped)
            .unwrap_or(false)
    }
}

/// A summary of a sub-agent's state — safe to serialize and send over the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentInfo {
    pub name: String,
    pub role: String,
    pub status: SubAgentStatus,
    pub tool_count: usize,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryStore;

    fn make_pool() -> AgentPool {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let provider = ProviderConfig::default();
        AgentPool::new(provider, memory)
    }

    #[test]
    fn spawn_and_list_agents() {
        let pool = make_pool();
        let spec = SubAgentSpec::new("researcher", "Research topics and summarize findings");
        pool.spawn(spec).unwrap();

        let agents = pool.list();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "researcher");
        assert_eq!(agents[0].status, SubAgentStatus::Idle);
    }

    #[test]
    fn spawn_duplicate_active_agent_fails() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("coder", "Write code")).unwrap();
        let result = pool.spawn(SubAgentSpec::new("coder", "Write more code"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn stop_agent_marks_as_stopped() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("planner", "Plan tasks")).unwrap();
        pool.stop("planner").unwrap();

        let agents = pool.list();
        assert_eq!(agents[0].status, SubAgentStatus::Stopped);
        assert!(!pool.exists("planner"));
    }

    #[test]
    fn stop_nonexistent_agent_fails() {
        let pool = make_pool();
        let result = pool.stop("ghost");
        assert!(result.is_err());
    }

    #[test]
    fn spawn_after_stop_succeeds() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("writer", "Write content")).unwrap();
        pool.stop("writer").unwrap();
        // Should succeed — the stopped entry is replaced
        pool.spawn(SubAgentSpec::new("writer", "Write better content")).unwrap();
        assert!(pool.exists("writer"));
    }

    #[test]
    fn active_count_excludes_stopped() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("a1", "Role A")).unwrap();
        pool.spawn(SubAgentSpec::new("a2", "Role B")).unwrap();
        assert_eq!(pool.active_count(), 2);
        pool.stop("a1").unwrap();
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn spec_with_tools_filter() {
        let spec = SubAgentSpec::new("analyst", "Analyse data")
            .with_tools(vec!["shell".to_string(), "file_read".to_string()]);
        assert_eq!(spec.tools, vec!["shell", "file_read"]);
    }

    #[test]
    fn spec_system_prompt_override() {
        let spec = SubAgentSpec::new("custom", "Custom role")
            .with_system_prompt("You are a custom agent.");
        assert_eq!(spec.system_prompt, "You are a custom agent.");
    }

    #[test]
    fn list_returns_all_including_stopped() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("x", "X")).unwrap();
        pool.spawn(SubAgentSpec::new("y", "Y")).unwrap();
        pool.stop("x").unwrap();
        // list() returns all entries including stopped
        let all = pool.list();
        assert_eq!(all.len(), 2);
    }
}

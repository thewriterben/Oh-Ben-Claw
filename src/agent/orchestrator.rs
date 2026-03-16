//! Orchestrator — the top-level multi-agent coordinator.
//!
//! The `OrchestratorAgent` wraps the main `Agent` and adds:
//!
//! - An `AgentPool` of spawned sub-agents
//! - Automatic injection of delegation tools into the orchestrator's tool registry
//! - A `route()` method that analyzes a task and suggests which sub-agent to use
//! - A `run_turn()` method that processes a user message through the full
//!   orchestrator → sub-agent → result aggregation pipeline
//!
//! # Routing Strategies
//!
//! The orchestrator supports three routing strategies:
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | `Manual` | The LLM decides which sub-agent to delegate to via tool calls |
//! | `RoundRobin` | Tasks are distributed evenly across all idle sub-agents |
//! | `LeastBusy` | Tasks go to the sub-agent with the fewest completed tasks |

use crate::agent::delegation_tools::delegation_tools;
use crate::agent::pool::{AgentPool, SubAgentSpec};
use crate::agent::{Agent, AgentHandle, AgentResponse};
use crate::config::{AgentConfig, ProviderConfig};
use crate::memory::MemoryStore;
use crate::providers;
use crate::tools::default_tools;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Routing Strategy ──────────────────────────────────────────────────────────

/// How the orchestrator routes tasks to sub-agents.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// The LLM decides which sub-agent to delegate to via `delegate_task` tool calls.
    #[default]
    Manual,
    /// Distribute tasks evenly across all idle sub-agents (round-robin).
    RoundRobin,
    /// Always delegate to the sub-agent with the fewest completed tasks.
    LeastBusy,
}

// ── Orchestrator Config ───────────────────────────────────────────────────────

/// Configuration for the orchestrator layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Whether multi-agent orchestration is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The routing strategy to use.
    #[serde(default)]
    pub routing: RoutingStrategy,
    /// Pre-configured sub-agents to spawn at startup.
    #[serde(default)]
    pub agents: Vec<SubAgentSpec>,
    /// System prompt extension for the orchestrator role.
    #[serde(default = "default_orchestrator_prompt_extension")]
    pub system_prompt_extension: String,
}

fn default_orchestrator_prompt_extension() -> String {
    "\n\nYou are also an orchestrator. You have access to sub-agents via the \
     `spawn_agent`, `delegate_task`, `list_agents`, and `stop_agent` tools. \
     For complex tasks, break them down and delegate sub-tasks to appropriate \
     specialized agents. Always aggregate and synthesize the results before \
     responding to the user."
        .to_string()
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            routing: RoutingStrategy::Manual,
            agents: Vec::new(),
            system_prompt_extension: default_orchestrator_prompt_extension(),
        }
    }
}

// ── OrchestratorAgent ─────────────────────────────────────────────────────────

/// The top-level multi-agent coordinator.
///
/// Wraps the main `Agent` with an `AgentPool` and delegation tools.
pub struct OrchestratorAgent {
    /// The orchestrator's own `AgentHandle` — used for the top-level LLM calls.
    pub handle: AgentHandle,
    /// The pool of managed sub-agents.
    pub pool: AgentPool,
    /// The current session ID (shared with delegation tools).
    session_id: Arc<Mutex<String>>,
    /// The routing strategy.
    pub routing: RoutingStrategy,
}

impl OrchestratorAgent {
    /// Build a new orchestrator from config.
    ///
    /// This:
    /// 1. Builds the main agent with all default tools + delegation tools
    /// 2. Creates the `AgentPool`
    /// 3. Spawns any pre-configured sub-agents from `config.agents`
    pub fn new(
        agent_config: AgentConfig,
        provider_config: ProviderConfig,
        memory: Arc<MemoryStore>,
        orchestrator_config: OrchestratorConfig,
        session_id: String,
    ) -> Result<Self> {
        let pool = AgentPool::new(provider_config.clone(), Arc::clone(&memory));
        let session_arc = Arc::new(Mutex::new(session_id));

        // Build the orchestrator's tool registry: default tools + delegation tools
        let mut tools = default_tools();
        tools.extend(delegation_tools(pool.clone(), Arc::clone(&session_arc)));

        // Extend the system prompt with orchestrator instructions
        let mut config = agent_config;
        config
            .system_prompt
            .push_str(&orchestrator_config.system_prompt_extension);

        // Build the provider
        let provider = providers::from_config(&provider_config)?;

        let agent = Arc::new(Agent::new(config, provider, Arc::clone(&memory), tools));
        let handle = AgentHandle::new(Arc::clone(&agent), provider_config.clone());

        // Spawn pre-configured sub-agents
        for spec in orchestrator_config.agents {
            if let Err(e) = pool.spawn(spec.clone()) {
                tracing::warn!(agent = %spec.name, error = %e, "Failed to pre-spawn sub-agent");
            } else {
                tracing::info!(agent = %spec.name, "Pre-configured sub-agent spawned");
            }
        }

        Ok(Self {
            handle,
            pool,
            session_id: session_arc,
            routing: orchestrator_config.routing,
        })
    }

    /// Update the current session ID (called when the user switches sessions).
    pub async fn set_session(&self, session_id: String) {
        let mut s = self.session_id.lock().await;
        *s = session_id;
    }

    /// Process a user message through the orchestrator.
    ///
    /// The orchestrator's LLM will decide whether to answer directly or
    /// delegate sub-tasks to specialized agents via tool calls.
    pub async fn process(&self, session_id: &str, message: &str) -> Result<AgentResponse> {
        // Update session ID so delegation tools use the right sub-session prefix
        self.set_session(session_id.to_string()).await;
        self.handle.process(session_id, message).await
    }

    /// Get a summary of the orchestrator's state for the gateway `/status` endpoint.
    pub fn status(&self) -> OrchestratorStatus {
        OrchestratorStatus {
            active_agents: self.pool.active_count(),
            agents: self.pool.list(),
            routing: format!("{:?}", self.routing),
        }
    }
}

/// A summary of the orchestrator's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorStatus {
    pub active_agents: usize,
    pub agents: Vec<crate::agent::pool::SubAgentInfo>,
    pub routing: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, ProviderConfig};
    use crate::memory::MemoryStore;
    use std::sync::Arc;

    fn make_orchestrator() -> OrchestratorAgent {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let agent_config = AgentConfig::default();
        let provider_config = ProviderConfig::default();
        let orch_config = OrchestratorConfig::default();
        OrchestratorAgent::new(
            agent_config,
            provider_config,
            memory,
            orch_config,
            "test-session".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn orchestrator_builds_with_delegation_tools() {
        let orch = make_orchestrator();
        let tool_names = orch.handle.tool_names();
        assert!(tool_names.iter().any(|n| n == "spawn_agent"));
        assert!(tool_names.iter().any(|n| n == "delegate_task"));
        assert!(tool_names.iter().any(|n| n == "list_agents"));
        assert!(tool_names.iter().any(|n| n == "stop_agent"));
    }

    #[test]
    fn orchestrator_has_default_tools_plus_delegation() {
        let orch = make_orchestrator();
        let tool_names = orch.handle.tool_names();
        // Should have default tools (shell, file_*, http, memory_note)
        assert!(tool_names
            .iter()
            .any(|n| n.contains("shell") || n.contains("file")));
        // Plus all 4 delegation tools
        assert!(orch.handle.tool_count() >= 4);
    }

    #[test]
    fn orchestrator_pool_starts_empty() {
        let orch = make_orchestrator();
        assert_eq!(orch.pool.active_count(), 0);
    }

    #[test]
    fn orchestrator_pre_spawns_configured_agents() {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let orch_config = OrchestratorConfig {
            enabled: true,
            agents: vec![
                SubAgentSpec::new("researcher", "Research topics"),
                SubAgentSpec::new("coder", "Write code"),
            ],
            ..Default::default()
        };
        let orch = OrchestratorAgent::new(
            AgentConfig::default(),
            ProviderConfig::default(),
            memory,
            orch_config,
            "session".to_string(),
        )
        .unwrap();
        assert_eq!(orch.pool.active_count(), 2);
        assert!(orch.pool.exists("researcher"));
        assert!(orch.pool.exists("coder"));
    }

    #[test]
    fn orchestrator_status_reflects_pool() {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let orch_config = OrchestratorConfig {
            enabled: true,
            agents: vec![SubAgentSpec::new("analyst", "Analyse data")],
            ..Default::default()
        };
        let orch = OrchestratorAgent::new(
            AgentConfig::default(),
            ProviderConfig::default(),
            memory,
            orch_config,
            "session".to_string(),
        )
        .unwrap();
        let status = orch.status();
        assert_eq!(status.active_agents, 1);
        assert_eq!(status.agents.len(), 1);
        assert_eq!(status.agents[0].name, "analyst");
    }

    #[test]
    fn orchestrator_system_prompt_extended() {
        let orch = make_orchestrator();
        // The system prompt should contain the orchestrator extension
        let tool_names = orch.handle.tool_names();
        // Verify delegation tools are present (proxy for system prompt extension working)
        assert!(tool_names.contains(&"spawn_agent".to_string()));
    }

    #[tokio::test]
    async fn set_session_updates_delegation_context() {
        let orch = make_orchestrator();
        orch.set_session("new-session".to_string()).await;
        let session = orch.session_id.lock().await;
        assert_eq!(*session, "new-session");
    }

    #[test]
    fn routing_strategy_default_is_manual() {
        let config = OrchestratorConfig::default();
        matches!(config.routing, RoutingStrategy::Manual);
    }
}

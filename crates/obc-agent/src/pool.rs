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

use crate::scope::{Scope, ScopedResolver, VaultScopedFill};
use crate::AgentConfig;
use crate::{Agent, AgentHandle};
use anyhow::{bail, Result};
use obc_conscience::ReachGate;
use obc_memory::MemoryStore;
use obc_providers::ProviderConfig;
use obc_tools::{default_tools_with_reach, Tool};
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
    /// Tool names or globs to give this sub-agent, cut from the pool's scope
    /// ([`crate::scope`]). Empty inherits the pool's scope; `["none"]` gives
    /// no tools (pure reasoning only).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Vault credential names or globs this sub-agent's egress may inject,
    /// cut from the pool's scope. Empty inherits it; `["none"]` gives none.
    #[serde(default)]
    pub vaults: Vec<String>,
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
            vaults: Vec::new(),
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

    /// Restrict to a set of vault credential names.
    pub fn with_vaults(mut self, vaults: Vec<String>) -> Self {
        self.vaults = vaults;
        self
    }

    /// Use a specific provider config.
    pub fn with_provider(mut self, provider: ProviderConfig) -> Self {
        self.provider = Some(provider);
        self
    }
}

// ── Sub-Agent Posture ─────────────────────────────────────────────────────────

/// The parent's safety layers, attached to every sub-agent the pool spawns.
/// Until 2026-11-02 a sub-agent was a bare `Agent::new`: no tool policy, no
/// Track 0 gate, no approval, no trust, default taint mode and the unfenced
/// default tool set, so delegating was a way round all of them.
#[derive(Clone, Default)]
pub struct SubAgentPosture {
    pub policy: Option<obc_safety::PolicyEngine>,
    pub safety: Option<Arc<obc_safety::SafetyGate>>,
    pub approval: Option<Arc<obc_approval::ApprovalManager>>,
    pub trust: Option<Arc<obc_safety::trust::TrustScorer>>,
    pub obs: Option<Arc<obc_observability::ObsContext>>,
    pub cost: Option<(Arc<obc_cost::CostTracker>, f64, f64)>,
    pub taint_mode: obc_safety::taint::TaintMode,
    /// The parent's own tools (its fences included). None = the default set.
    pub tools: Option<Vec<Arc<dyn Tool>>>,
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
    /// Conscience egress reach gate applied to every spawned sub-agent's tools,
    /// so a delegated agent cannot bypass the allowlist the orchestrator honors.
    reach: Option<ReachGate>,
    /// Track 0 auditor, so a sub-agent's reach refusal is a tamper-evident record.
    auditor: Option<Arc<Mutex<obc_safety::ActionAuditor>>>,
    /// Conscience credential resolver (item (b)), so a spawned sub-agent injects
    /// a reach-named credential at its egress boundary too.
    resolver: Option<Arc<dyn obc_tools::credentials::CredentialResolver>>,
    /// The parent's safety layers, attached to every sub-agent.
    posture: SubAgentPosture,
    /// The ceiling every sub-agent's scope is cut from.
    scope: Scope,
}

impl AgentPool {
    /// Create a new empty pool.
    pub fn new(default_provider: ProviderConfig, memory: Arc<MemoryStore>) -> Self {
        Self {
            agents: Arc::new(Mutex::new(HashMap::new())),
            default_provider,
            memory,
            reach: None,
            auditor: None,
            resolver: None,
            posture: SubAgentPosture::default(),
            scope: Scope::unrestricted(),
        }
    }

    /// Attach the parent's safety layers to every sub-agent this pool spawns.
    pub fn with_posture(mut self, posture: SubAgentPosture) -> Self {
        self.posture = posture;
        self
    }

    /// Set the ceiling every sub-agent's tool and vault scope is cut from.
    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Attach a conscience reach gate (and optional auditor + credential
    /// resolver) applied to every sub-agent this pool spawns — closing the
    /// delegation bypass where a sub-agent would otherwise receive ungated egress
    /// tools. Returns self so it chains onto `AgentPool::new(...)`.
    pub fn with_reach_gate(
        mut self,
        reach: Option<ReachGate>,
        auditor: Option<Arc<Mutex<obc_safety::ActionAuditor>>>,
        resolver: Option<Arc<dyn obc_tools::credentials::CredentialResolver>>,
    ) -> Self {
        self.reach = reach;
        self.auditor = auditor;
        self.resolver = resolver;
        self
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
        let provider_config = spec
            .provider
            .clone()
            .unwrap_or_else(|| self.default_provider.clone());
        let provider = obc_providers::from_config(&provider_config)?;

        // The child's scope, cut from the pool's ceiling; a literal tool or
        // vault name the parent lacks refuses the spawn.
        let scope = self.scope.narrow(&spec.tools, &spec.vaults)?;

        // Build the tool registry. Sub-agents get the SAME conscience reach gate
        // (and auditor) as the orchestrator, so delegation is not an egress
        // bypass — a spawned agent's HTTP/browser calls hit the same allowlist.
        // The credential resolver (item (b)) is the orchestrator's, limited to
        // the child's vault scope.
        let resolver = self.resolver.clone().map(|r| {
            Arc::new(ScopedResolver::new(r, scope.clone(), &spec.name))
                as Arc<dyn obc_tools::credentials::CredentialResolver>
        });
        let all_tools: Vec<Arc<dyn Tool>> = match &self.posture.tools {
            // The parent's own tools, fences included. Its `http` carries the
            // unscoped resolver, so it is swapped for one with the scoped one;
            // its `browser_fill_secret` does too, so it is wrapped.
            Some(parent) => {
                let mut http = obc_tools::HttpTool::new();
                if let Some(gate) = self.reach.clone() {
                    http = http.with_reach_gate(gate);
                }
                if let Some(a) = self.auditor.clone() {
                    http = http.with_auditor(a);
                }
                if let Some(r) = resolver {
                    http = http.with_resolver(r);
                }
                let mut tools: Vec<Arc<dyn Tool>> = parent
                    .iter()
                    .filter(|t| t.name() != http.name())
                    .map(|t| {
                        if t.name() == VaultScopedFill::TOOL {
                            Arc::new(VaultScopedFill::new(t.clone(), scope.clone(), &spec.name))
                                as Arc<dyn Tool>
                        } else {
                            t.clone()
                        }
                    })
                    .collect();
                if parent.iter().any(|t| t.name() == http.name()) {
                    tools.push(Arc::new(http));
                }
                tools
            }
            None => default_tools_with_reach(self.reach.clone(), self.auditor.clone(), resolver)
                .into_iter()
                .map(Arc::from)
                .collect(),
        };
        let tools: Vec<Arc<dyn Tool>> = all_tools
            .into_iter()
            .filter(|t| scope.allows_tool(t.name()))
            .collect();

        // Build the agent config
        let agent_config = AgentConfig {
            name: spec.name.clone(),
            system_prompt: spec.system_prompt.clone(),
            max_tool_iterations: spec.max_iterations,
            ..AgentConfig::default()
        };

        // Build the agent with the parent's safety layers and its own scope.
        let posture = self.posture.clone();
        let mut agent = Agent::new(agent_config, provider, Arc::clone(&self.memory), Vec::new())
            .with_tool_scope(scope)
            .with_taint_mode(posture.taint_mode);
        agent.add_tool_arcs(tools);
        if let Some(policy) = posture.policy {
            agent = agent.with_policy(policy);
        }
        if let Some(gate) = posture.safety {
            agent = agent.with_safety_gate(gate);
        }
        if let Some(a) = self.auditor.clone() {
            agent = agent.with_action_auditor(a);
        }
        if let Some(approval) = posture.approval {
            agent = agent.with_approval(approval);
        }
        if let Some(trust) = posture.trust {
            agent = agent.with_trust(trust);
        }
        if let Some(obs) = posture.obs {
            agent = agent.with_obs(obs);
        }
        if let Some((tracker, in_price, out_price)) = posture.cost {
            agent = agent.with_cost(tracker, in_price, out_price);
        }
        let agent = Arc::new(agent);

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
                    bail!(
                        "Sub-agent '{}' is currently busy — wait for it to finish first",
                        name
                    );
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
                Err(e) => results.push((
                    "unknown".to_string(),
                    Err(anyhow::anyhow!("Task panicked: {}", e)),
                )),
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
    use obc_memory::MemoryStore;

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
        pool.spawn(SubAgentSpec::new("coder", "Write code"))
            .unwrap();
        let result = pool.spawn(SubAgentSpec::new("coder", "Write more code"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn stop_agent_marks_as_stopped() {
        let pool = make_pool();
        pool.spawn(SubAgentSpec::new("planner", "Plan tasks"))
            .unwrap();
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
        pool.spawn(SubAgentSpec::new("writer", "Write content"))
            .unwrap();
        pool.stop("writer").unwrap();
        // Should succeed — the stopped entry is replaced
        pool.spawn(SubAgentSpec::new("writer", "Write better content"))
            .unwrap();
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

    // ── Scopes and posture (PERSONAL-AGENTS-2026-11 item 9) ──────────────────

    fn handle_of(pool: &AgentPool, name: &str) -> AgentHandle {
        pool.agents.lock().unwrap()[name].handle.clone()
    }

    fn scoped_pool(tools: &[&str], vaults: &[&str]) -> AgentPool {
        make_pool().with_scope(Scope::from_config(&crate::scope::ScopeConfig {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            vaults: vaults.iter().map(|s| s.to_string()).collect(),
        }))
    }

    #[test]
    fn registered_tools_stay_inside_the_ceiling() {
        let pool = scoped_pool(&["http", "browser_*"], &["*"]);
        pool.spawn(SubAgentSpec::new("web", "Browse")).unwrap();
        let names = handle_of(&pool, "web").tool_names();
        assert!(names.iter().any(|n| n == "http"), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("browser_")), "{names:?}");
        assert!(
            !names.iter().any(|n| n == "shell" || n == "file"),
            "{names:?}"
        );
    }

    #[test]
    fn a_child_glob_cannot_widen_the_ceiling() {
        let pool = scoped_pool(&["http"], &["*"]);
        pool.spawn(SubAgentSpec::new("greedy", "Wants it all").with_tools(vec!["*".into()]))
            .unwrap();
        assert_eq!(handle_of(&pool, "greedy").tool_names(), vec!["http"]);
    }

    #[test]
    fn a_literal_outside_the_ceiling_refuses_the_spawn() {
        let pool = scoped_pool(&["http"], &["github"]);
        let err = pool
            .spawn(SubAgentSpec::new("sh", "Shell").with_tools(vec!["shell".into()]))
            .unwrap_err();
        assert!(
            err.to_string().contains("outside the parent's scope"),
            "{err}"
        );
        let err = pool
            .spawn(SubAgentSpec::new("aws", "Cloud").with_vaults(vec!["aws_root".into()]))
            .unwrap_err();
        assert!(err.to_string().contains("vault 'aws_root'"), "{err}");
        assert!(!pool.exists("sh") && !pool.exists("aws"));
    }

    #[tokio::test]
    async fn the_parent_policy_reaches_the_sub_agent() {
        let policy = obc_safety::PolicyEngine::new(vec![obc_safety::ToolPolicy {
            name: "no-shell".into(),
            tool_pattern: "shell".into(),
            arg_contains: None,
            action: obc_safety::ToolPolicyAction::Deny,
            reason: Some("parent forbids it".into()),
        }]);
        let pool = make_pool().with_posture(SubAgentPosture {
            policy: Some(policy),
            ..Default::default()
        });
        pool.spawn(SubAgentSpec::new("worker", "Work")).unwrap();
        let result = handle_of(&pool, "worker")
            .execute_tool_direct("shell", serde_json::json!({ "command": "echo hi" }))
            .await
            .unwrap();
        assert!(
            !result.is_ok(),
            "a delegated shell call bypassed the parent's policy"
        );
        let error = result.error.unwrap_or_default();
        assert!(error.contains("no-shell"), "{error}");
    }

    #[tokio::test]
    async fn the_chokepoint_refuses_a_registered_tool_outside_the_scope() {
        // Registered but out of scope: the chokepoint, not registration, refuses.
        let scope = Scope::unrestricted().narrow(&["http".into()], &[]).unwrap();
        let agent = Agent::new(
            AgentConfig::default(),
            obc_providers::from_config(&ProviderConfig::default()).unwrap(),
            Arc::new(MemoryStore::open_in_memory().unwrap()),
            vec![Box::new(obc_tools::ShellTool::new())],
        )
        .with_tool_scope(scope);
        let handle = AgentHandle::new(Arc::new(agent), ProviderConfig::default());
        let result = handle
            .execute_tool_direct("shell", serde_json::json!({ "command": "echo hi" }))
            .await
            .unwrap();
        let error = result.error.unwrap_or_default();
        assert!(error.contains("outside this sub-agent's scope"), "{error}");
    }
}

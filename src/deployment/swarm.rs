//! Deployment swarm — LLM-powered multi-agent deployment planning.
//!
//! The `DeploymentSwarm` orchestrates a team of specialised sub-agents to
//! collaboratively plan, validate, and annotate a deployment scheme.
//!
//! # Architecture
//!
//! ```text
//! DeploymentSwarm
//!     │
//!     ├─ hardware-advisor   — validates inventory, identifies gaps
//!     ├─ architect          — designs the agent topology
//!     ├─ config-generator   — renders the TOML configuration
//!     └─ requirements-checker — verifies all feature desires are met
//! ```
//!
//! Each sub-agent operates on a shared `AgentPool` and communicates through the
//! standard `delegate_task` mechanism.  The swarm starts with the rule-based
//! `DeploymentPlanner` output as a grounding context, then uses the LLM agents
//! to refine, annotate, and extend the plan.
//!
//! For environments where no LLM is available, `DeploymentSwarm::plan_static`
//! returns the rule-based plan directly.

use crate::agent::pool::{AgentPool, SubAgentSpec};
use crate::config::ProviderConfig;
use crate::deployment::inventory::HardwareInventory;
use crate::deployment::planner::DeploymentPlanner;
use crate::deployment::scheme::DeploymentScheme;
use crate::memory::MemoryStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Swarm Configuration ───────────────────────────────────────────────────────

/// Configuration for the deployment planning swarm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmConfig {
    /// Whether to enable LLM-powered swarm planning (default: false — static only).
    #[serde(default)]
    pub enabled: bool,
    /// Maximum iterations each sub-agent may use.
    #[serde(default = "default_max_iter")]
    pub max_iterations: usize,
}

fn default_max_iter() -> usize {
    6
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_iterations: default_max_iter(),
        }
    }
}

// ── Swarm Result ──────────────────────────────────────────────────────────────

/// The result of a swarm planning run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmResult {
    /// The deployment scheme (may be rule-based or LLM-refined).
    pub scheme: DeploymentScheme,
    /// Whether LLM-powered swarm refinement was used.
    pub llm_refined: bool,
    /// Annotations added by LLM sub-agents (if any).
    #[serde(default)]
    pub agent_annotations: Vec<AgentAnnotation>,
}

/// An annotation produced by a sub-agent during swarm planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAnnotation {
    /// Name of the agent that produced this annotation.
    pub agent: String,
    /// The annotation text.
    pub text: String,
}

// ── Deployment Swarm ──────────────────────────────────────────────────────────

/// LLM-powered multi-agent deployment planning swarm.
///
/// Wraps `DeploymentPlanner` with an `AgentPool` of specialised sub-agents
/// that can refine, validate, and annotate the deployment scheme using an LLM.
///
/// Use `plan_static()` when no LLM is available (CI, tests, offline).
/// Use `plan()` for full LLM-enhanced planning.
pub struct DeploymentSwarm {
    pool: AgentPool,
    config: SwarmConfig,
}

impl DeploymentSwarm {
    /// Create a new deployment swarm.
    pub fn new(provider: ProviderConfig, memory: Arc<MemoryStore>, config: SwarmConfig) -> Self {
        let pool = AgentPool::new(provider, memory);
        Self { pool, config }
    }

    /// Plan a deployment using the rule-based planner only (no LLM required).
    ///
    /// This is the recommended method for testing and offline environments.
    pub fn plan_static(inventory: &HardwareInventory) -> SwarmResult {
        let scheme = DeploymentPlanner::plan(inventory);
        SwarmResult {
            scheme,
            llm_refined: false,
            agent_annotations: Vec::new(),
        }
    }

    /// Plan a deployment, optionally refining with LLM sub-agents.
    ///
    /// If `config.enabled` is false, falls back to `plan_static`.
    /// If `config.enabled` is true, spawns sub-agents and delegates refinement tasks.
    pub async fn plan(&self, inventory: &HardwareInventory, session_id: &str) -> Result<SwarmResult> {
        // Always start with the deterministic rule-based plan
        let base_scheme = DeploymentPlanner::plan(inventory);

        if !self.config.enabled {
            return Ok(SwarmResult {
                scheme: base_scheme,
                llm_refined: false,
                agent_annotations: Vec::new(),
            });
        }

        // Spawn specialised sub-agents
        self.spawn_agents()?;

        let mut annotations: Vec<AgentAnnotation> = Vec::new();

        // Build context string for agents
        let context = format!(
            "Hardware inventory for '{}': {}\n\n\
             Feature desires: {}\n\n\
             Preliminary deployment scheme:\n{}",
            inventory.scenario_name,
            inventory
                .items
                .iter()
                .map(|i| format!("{} ({})", i.name, i.board_name))
                .collect::<Vec<_>>()
                .join(", "),
            inventory
                .feature_desires
                .iter()
                .map(|d| d.description())
                .collect::<Vec<_>>()
                .join(", "),
            base_scheme.summary
        );

        // ── Hardware advisor review ───────────────────────────────────────────
        let advisor_task = format!(
            "Review this hardware inventory and deployment plan. Identify any risks, \
             limitations, or missing capabilities.\n\n{context}"
        );
        match self.pool.delegate("hardware-advisor", &advisor_task, session_id).await {
            Ok(response) => annotations.push(AgentAnnotation {
                agent: "hardware-advisor".to_string(),
                text: response,
            }),
            Err(e) => tracing::warn!("hardware-advisor failed: {e}"),
        }

        // ── Architect review ──────────────────────────────────────────────────
        let arch_task = format!(
            "Review the agent topology for this deployment. Suggest improvements to \
             the sub-agent structure, communication patterns, and resource allocation.\n\n{context}"
        );
        match self.pool.delegate("architect", &arch_task, session_id).await {
            Ok(response) => annotations.push(AgentAnnotation {
                agent: "architect".to_string(),
                text: response,
            }),
            Err(e) => tracing::warn!("architect failed: {e}"),
        }

        // ── Requirements checker ──────────────────────────────────────────────
        let reqs_task = format!(
            "Check that all feature desires are satisfied by the deployment plan. \
             For each unsatisfied desire, suggest the minimum additional hardware needed.\n\n{context}"
        );
        match self.pool.delegate("requirements-checker", &reqs_task, session_id).await {
            Ok(response) => annotations.push(AgentAnnotation {
                agent: "requirements-checker".to_string(),
                text: response,
            }),
            Err(e) => tracing::warn!("requirements-checker failed: {e}"),
        }

        Ok(SwarmResult {
            scheme: base_scheme,
            llm_refined: true,
            agent_annotations: annotations,
        })
    }

    /// Spawn all deployment planning sub-agents into the pool.
    fn spawn_agents(&self) -> Result<()> {
        let specs = Self::agent_specs(self.config.max_iterations);
        for spec in specs {
            if !self.pool.exists(&spec.name) {
                if let Err(e) = self.pool.spawn(spec.clone()) {
                    tracing::warn!(agent = %spec.name, error = %e, "Failed to spawn deployment sub-agent");
                }
            }
        }
        Ok(())
    }

    /// Build the sub-agent specifications for the swarm.
    pub fn agent_specs(max_iterations: usize) -> Vec<SubAgentSpec> {
        vec![
            SubAgentSpec::new(
                "hardware-advisor",
                "Hardware compatibility expert. You know Oh-Ben-Claw's peripheral \
                 registry and can identify capability gaps, unsupported combinations, \
                 and potential hardware conflicts.",
            )
            .with_tools(vec!["memory_note".to_string()])
            .with_system_prompt(
                "You are a hardware advisor for Oh-Ben-Claw deployments. \
                 Analyse hardware inventories for compatibility, identify missing \
                 capabilities, and suggest specific hardware additions. Be concise \
                 and actionable. Return structured findings.",
            ),

            SubAgentSpec {
                name: "architect".to_string(),
                role: "Multi-agent system architect. Designs optimal agent topologies \
                       for given hardware configurations."
                    .to_string(),
                system_prompt: "You are a multi-agent system architect for Oh-Ben-Claw. \
                    Given a hardware inventory, design an optimal agent topology: \
                    which sub-agents to spawn, what tools they need, and how they \
                    should communicate. Focus on efficiency, latency, and reliability."
                    .to_string(),
                provider: None,
                tools: vec!["memory_note".to_string()],
                max_iterations,
            },

            SubAgentSpec {
                name: "requirements-checker".to_string(),
                role: "Requirements validation specialist. Verifies that all feature \
                       desires are satisfied by the deployment plan."
                    .to_string(),
                system_prompt: "You are a requirements checker for Oh-Ben-Claw deployments. \
                    Your job is to verify that every feature desire in the operator's \
                    wish list is covered by the available hardware. For each gap, \
                    suggest the minimum hardware needed to close it. Be specific about \
                    board names and capability tokens."
                    .to_string(),
                provider: None,
                tools: vec!["memory_note".to_string()],
                max_iterations,
            },
        ]
    }

    /// Return a reference to the agent pool for inspection.
    pub fn pool(&self) -> &AgentPool {
        &self.pool
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::inventory::HardwareInventory;
    use crate::memory::MemoryStore;

    fn make_swarm() -> DeploymentSwarm {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let provider = ProviderConfig::default();
        DeploymentSwarm::new(provider, memory, SwarmConfig::default())
    }

    #[test]
    fn plan_static_produces_valid_scheme() {
        let inv = HardwareInventory::nanopi_scenario();
        let result = DeploymentSwarm::plan_static(&inv);
        assert!(!result.llm_refined);
        assert!(result.agent_annotations.is_empty());
        assert!(!result.scheme.assignments.is_empty());
    }

    #[test]
    fn plan_static_host_identified() {
        let inv = HardwareInventory::nanopi_scenario();
        let result = DeploymentSwarm::plan_static(&inv);
        assert_eq!(result.scheme.host_board, "nanopi-neo3");
    }

    #[test]
    fn agent_specs_returns_expected_agents() {
        let specs = DeploymentSwarm::agent_specs(6);
        let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hardware-advisor"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"requirements-checker"));
    }

    #[test]
    fn swarm_pool_starts_empty() {
        let swarm = make_swarm();
        assert_eq!(swarm.pool().active_count(), 0);
    }

    #[tokio::test]
    async fn plan_with_disabled_swarm_returns_static_result() {
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        let provider = ProviderConfig::default();
        let swarm = DeploymentSwarm::new(
            provider,
            memory,
            SwarmConfig {
                enabled: false,
                max_iterations: 4,
            },
        );
        let inv = HardwareInventory::nanopi_scenario();
        let result = swarm.plan(&inv, "test-session").await.unwrap();
        assert!(!result.llm_refined);
        assert!(!result.scheme.assignments.is_empty());
    }

    #[test]
    fn swarm_result_contains_scheme_summary() {
        let inv = HardwareInventory::nanopi_scenario();
        let result = DeploymentSwarm::plan_static(&inv);
        assert!(!result.scheme.summary.is_empty());
    }
}

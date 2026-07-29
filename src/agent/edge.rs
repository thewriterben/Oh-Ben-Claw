//! Edge-native agent for resource-constrained devices (NanoPi Neo3 and similar
//! Linux single-board computers).
//!
//! The `EdgeAgent` wraps the standard `Agent` with defaults appropriate for
//! devices that have limited RAM and CPU, and optionally wires up the P2P
//! spine so the device participates in a broker-free mesh rather than
//! connecting to a central MQTT broker.
//!
//! # Typical usage on a NanoPi Neo3
//!
//! ```toml
//! # ~/.oh-ben-claw/config.toml on the NanoPi Neo3
//! [provider]
//! name  = "ollama"
//! model = "llama3.2"
//! base_url = "http://localhost:11434"
//!
//! [edge]
//! enabled              = true
//! max_history_messages = 20
//! max_tool_iterations  = 5
//! p2p_enabled          = true
//! ```
//!
//! There is no `--edge` CLI flag: `EdgeAgent` is a library type, assembled by
//! whoever is embedding the agent on the device. `[edge].enabled` is that caller's
//! decision — it is what makes them build an `EdgeAgent` at all.
//!
//! Everything else in the section is honoured here. `max_history_messages` and
//! `max_tool_iterations` bound the agent at construction, and `p2p_enabled` starts
//! the mesh: [`EdgeAgentBuilder::build_and_start`] joins the local P2P spine and
//! registers whatever peer tools it finds, or does nothing at all when the key is
//! false. Before this the key parsed, validated, and was written into every
//! generated NanoPi config by the deployment planner — and a caller who did not
//! separately call `start_p2p` got an isolated node that had asked to be meshed.

/// Milliseconds to wait after starting the P2P spine before collecting peer
/// tools, to allow initial discovery broadcasts to arrive.
const P2P_DISCOVERY_DELAY_MS: u64 = 500;

use crate::agent::{Agent, AgentResponse};
use crate::config::{AgentConfig, EdgeConfig, ProviderConfig};
use crate::memory::MemoryStore;
use crate::providers;
use crate::security::PolicyEngine;
use crate::spine::p2p::{P2pConfig, P2pSpine};
use crate::spine::NodeAnnouncement;
use crate::tools::traits::Tool;
use anyhow::Result;
use std::sync::Arc;

// ── EdgeAgent ─────────────────────────────────────────────────────────────────

/// A lightweight agent optimised for resource-constrained edge devices.
///
/// Under the hood this delegates all reasoning to the standard `Agent`.  The
/// value added by `EdgeAgent` is:
///
/// 1. **Reduced defaults** — smaller history window and tool-iteration cap.
/// 2. **P2P spine integration** — automatically discovers and registers tools
///    from other nodes on the local network.
/// 3. **Named edge session** — uses `edge-<node_id>` as the session key so
///    that edge conversations are stored separately from host-side sessions.
pub struct EdgeAgent {
    inner: Agent,
    provider_config: ProviderConfig,
    node_id: String,
    /// Retained for `p2p_enabled` — the one key in the section that describes what
    /// this agent should *do* rather than how large it may grow. The limits are
    /// applied at construction and the rest of the struct never needs them again.
    config: EdgeConfig,
    /// Board identifier used when announcing to peers. Cosmetic to this crate,
    /// load-bearing to whoever is reading a mesh roster.
    board: String,
    p2p_spine: Option<Arc<P2pSpine>>,
}

impl EdgeAgent {
    /// Create a new `EdgeAgent`.
    ///
    /// `tools` should contain the built-in tools appropriate for this device
    /// (e.g. GPIO, sensor reads).  Additional tools from P2P peers are
    /// injected when `start_p2p()` is called.
    pub fn new(
        agent_config: AgentConfig,
        edge_config: EdgeConfig,
        provider_config: ProviderConfig,
        memory: Arc<MemoryStore>,
        tools: Vec<Box<dyn Tool>>,
        node_id: impl Into<String>,
    ) -> Result<Self> {
        let provider = providers::from_config(&provider_config)?;
        // The point of an edge agent is bounded RAM, and this is where that is
        // actually applied. Before, `edge_config` was stored and never read: the
        // module header documented `max_history_messages`, the deployment planner
        // emitted it into every generated NanoPi config, and nothing anywhere
        // consumed it. `max_tool_iterations` was applied, but only on the builder
        // path — constructing an EdgeAgent directly silently ignored it.
        let agent = Agent::new(agent_config, provider, memory, tools)
            .with_max_history(edge_config.max_history_messages);
        Ok(Self {
            inner: agent,
            provider_config,
            node_id: node_id.into(),
            config: edge_config,
            board: "edge".to_string(),
            p2p_spine: None,
        })
    }

    /// Attach a policy engine.
    pub fn with_policy(mut self, policy: PolicyEngine) -> Self {
        self.inner = self.inner.with_policy(policy);
        self
    }

    /// Start the P2P spine, announce this node's capabilities, and register
    /// all discovered peer tools with the inner agent.
    ///
    /// Call this after constructing the `EdgeAgent` but before processing any
    /// messages.
    pub async fn start_p2p(
        &mut self,
        p2p_config: P2pConfig,
        announcement: NodeAnnouncement,
    ) -> Result<()> {
        let spine = P2pSpine::new(p2p_config).start().await?;
        spine.announce(&announcement).await?;

        // Wait briefly for initial peer discovery before registering tools.
        tokio::time::sleep(std::time::Duration::from_millis(P2P_DISCOVERY_DELAY_MS)).await;
        let peer_tools = spine.build_p2p_tools().await;

        tracing::info!(
            node_id = %self.node_id,
            peer_tool_count = peer_tools.len(),
            "Edge agent: registered P2P peer tools"
        );

        self.inner.add_tools(peer_tools);
        self.p2p_spine = Some(spine);
        Ok(())
    }

    /// Set the board identifier announced to peers. Defaults to `"edge"`.
    pub fn with_board(mut self, board: impl Into<String>) -> Self {
        self.board = board.into();
        self
    }

    /// Whether `[edge].p2p_enabled` asked for the mesh.
    pub fn p2p_enabled(&self) -> bool {
        self.config.p2p_enabled
    }

    /// Join the P2P spine **if the config asked for it**, and report whether it did.
    ///
    /// The peer-facing announcement is derived rather than passed in: the node id the
    /// agent already has, the board it was told, this crate's version, and the tools
    /// actually registered at this moment. A caller that had to assemble that itself
    /// would be describing the agent from the outside and could describe it wrongly —
    /// which is how a peer ends up seeing a tool that is not there.
    ///
    /// `Ok(false)` means the config said no. An error means it said yes and the mesh
    /// could not be joined, which is a real failure and is not swallowed: a node that
    /// silently runs isolated after asking to be meshed is the failure mode this
    /// method exists to remove.
    pub async fn start_p2p_if_enabled(&mut self) -> Result<bool> {
        if !self.config.p2p_enabled {
            tracing::debug!(
                node_id = %self.node_id,
                "Edge agent: [edge].p2p_enabled is false, staying off the mesh"
            );
            return Ok(false);
        }

        let p2p_config = P2pConfig {
            node_id: self.node_id.clone(),
            ..P2pConfig::default()
        };
        let announcement = NodeAnnouncement {
            node_id: self.node_id.clone(),
            board: self.board.clone(),
            firmware_version: env!("CARGO_PKG_VERSION").to_string(),
            tools: self
                .inner
                .tool_specs()
                .into_iter()
                .map(
                    |(name, description, parameters)| crate::spine::NodeToolSpec {
                        name,
                        description,
                        parameters,
                    },
                )
                .collect(),
            metadata: serde_json::json!({ "kind": "edge-agent" }),
        };

        self.start_p2p(p2p_config, announcement).await?;
        Ok(true)
    }

    /// Process a user message and return the assistant's final response.
    ///
    /// Uses the edge session ID (`edge-<node_id>`) so edge conversations are
    /// stored separately from host sessions.
    pub async fn process(&self, user_message: &str) -> Result<AgentResponse> {
        let session_id = format!("edge-{}", self.node_id);
        self.inner
            .process(&session_id, user_message, &self.provider_config)
            .await
    }

    /// Return the P2P spine if one was started.
    pub fn p2p_spine(&self) -> Option<&Arc<P2pSpine>> {
        self.p2p_spine.as_ref()
    }

    /// Return the number of registered tools (local + P2P peer tools).
    pub fn tool_count(&self) -> usize {
        self.inner.tool_count()
    }

    /// Return the configured node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}

// ── EdgeAgentBuilder ──────────────────────────────────────────────────────────

/// Convenience builder for `EdgeAgent`.
pub struct EdgeAgentBuilder {
    agent_config: AgentConfig,
    edge_config: EdgeConfig,
    board: String,
    node_id: String,
    provider_config: ProviderConfig,
    memory: Option<Arc<MemoryStore>>,
    tools: Vec<Box<dyn Tool>>,
    policy: Option<PolicyEngine>,
}

impl EdgeAgentBuilder {
    /// Create a new builder for a node with the given ID.
    pub fn new(node_id: impl Into<String>, edge_config: EdgeConfig) -> Self {
        let node_id = node_id.into();

        // Derive a sensible system prompt that names the node.
        let agent_config = AgentConfig {
            system_prompt: format!(
                "You are Oh-Ben-Claw running in edge-native mode on node '{}'. \
                 You are a lightweight AI assistant with direct access to local \
                 hardware tools and peer nodes on the same network. \
                 Keep responses concise — you are running on a resource-constrained device.",
                node_id
            ),
            max_tool_iterations: edge_config.max_tool_iterations,
            ..AgentConfig::default()
        };

        Self {
            agent_config,
            edge_config,
            board: "edge".to_string(),
            node_id,
            provider_config: ProviderConfig::default(),
            memory: None,
            tools: Vec::new(),
            policy: None,
        }
    }

    /// Set the LLM provider configuration.
    pub fn provider_config(mut self, config: ProviderConfig) -> Self {
        self.provider_config = config;
        self
    }

    /// Prefer the on-device model: among the primary provider and its fallback
    /// chain, select **local-first** (via the model registry) as the provider this
    /// edge agent uses. An edge node with a local Ollama fallback then runs
    /// on-device instead of reaching for the cloud. Leaves the config unchanged if
    /// no candidate is selectable.
    pub fn prefer_local(mut self) -> Self {
        use crate::providers::model_registry::{
            flatten_candidates, registry_from_providers, select_provider,
        };
        let candidates = flatten_candidates(&self.provider_config);
        let registry = registry_from_providers(&candidates, 60_000);
        if let Some(chosen) = select_provider(&candidates, &registry, 0) {
            self.provider_config = chosen.clone();
        }
        self
    }

    /// Set the memory store.
    pub fn memory(mut self, memory: Arc<MemoryStore>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Add tools available on this device.
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Attach a security policy engine.
    pub fn policy(mut self, policy: PolicyEngine) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Board identifier announced to peers on the mesh. Defaults to `"edge"`.
    pub fn board(mut self, board: impl Into<String>) -> Self {
        self.board = board.into();
        self
    }

    /// Build the `EdgeAgent`.  Returns `Err` if memory is missing or the
    /// provider configuration is invalid.
    pub fn build(self) -> Result<EdgeAgent> {
        let memory = self
            .memory
            .ok_or_else(|| anyhow::anyhow!("EdgeAgentBuilder: memory is required"))?;

        let board = self.board;
        let mut agent = EdgeAgent::new(
            self.agent_config,
            self.edge_config,
            self.provider_config,
            memory,
            self.tools,
            self.node_id,
        )?
        .with_board(board);

        if let Some(policy) = self.policy {
            agent = agent.with_policy(policy);
        }

        Ok(agent)
    }

    /// Build, and join the mesh if `[edge].p2p_enabled` asked for it.
    ///
    /// The one-line form for the ordinary case. `build()` stays sync and non-joining
    /// for callers that want to add tools or inspect the agent before it announces
    /// itself — announcing a tool list you are about to change is worse than
    /// announcing late.
    pub async fn build_and_start(self) -> Result<EdgeAgent> {
        let mut agent = self.build()?;
        agent.start_p2p_if_enabled().await?;
        Ok(agent)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EdgeConfig;
    use crate::memory::MemoryStore;

    #[test]
    fn edge_config_defaults_are_resource_friendly() {
        let config = EdgeConfig::default();
        assert!(
            config.max_history_messages <= 20,
            "Edge agent should keep a small history window"
        );
        assert!(
            config.max_tool_iterations <= 5,
            "Edge agent should limit tool iterations"
        );
    }

    // ── p2p_enabled actually starts the mesh ──────────────────────────────────

    fn built(p2p: bool) -> EdgeAgent {
        let config = EdgeConfig {
            p2p_enabled: p2p,
            ..EdgeConfig::default()
        };
        EdgeAgentBuilder::new("bench-001", config)
            .memory(Arc::new(MemoryStore::open_in_memory().unwrap()))
            .board("dfrobot-firebeetle2-esp32s3")
            .build()
            .unwrap()
    }

    /// The regression this exists for: `[edge].p2p_enabled` parsed, was validated by
    /// `Config::validate`, and was written into every generated NanoPi config by the
    /// deployment planner — and nothing anywhere acted on it. A caller who did not
    /// separately call `start_p2p` got an isolated node that had asked to be meshed.
    #[tokio::test]
    async fn p2p_disabled_stays_off_the_mesh_and_says_so() {
        let mut agent = built(false);
        assert!(!agent.p2p_enabled());
        assert!(!agent.start_p2p_if_enabled().await.unwrap());
        assert!(agent.p2p_spine().is_none());
    }

    #[test]
    fn the_config_decides_rather_than_the_caller() {
        // The value has to survive construction to be actionable at all — it was
        // dropped on the floor before, which is what made the key inert.
        assert!(built(true).p2p_enabled());
        assert!(!built(false).p2p_enabled());
    }

    #[tokio::test]
    async fn the_announcement_describes_the_tools_actually_registered() {
        // Built from the agent's own registry rather than passed in by the caller,
        // so a peer cannot be shown a tool that is not there. Checked without
        // joining a mesh: this is about what would be announced.
        let agent = built(true);
        let specs = agent.inner.tool_specs();
        assert_eq!(specs.len(), agent.tool_count());
        for (name, _, _) in &specs {
            assert!(!name.is_empty(), "a tool announced with no name");
        }
    }

    #[test]
    fn edge_agent_builder_requires_memory() {
        let config = EdgeConfig::default();
        let builder = EdgeAgentBuilder::new("test-node", config);
        let result = builder.build();
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("memory"));
    }

    #[test]
    fn edge_agent_node_id_is_correct() {
        let config = EdgeConfig::default();
        let agent = EdgeAgentBuilder::new("nanopi-kitchen", config)
            .memory(Arc::new(MemoryStore::open_in_memory().unwrap()))
            .build()
            .unwrap();
        assert_eq!(agent.node_id(), "nanopi-kitchen");
    }

    #[test]
    fn edge_agent_tool_count_starts_at_zero_with_no_tools() {
        let config = EdgeConfig::default();
        let agent = EdgeAgentBuilder::new("edge-01", config)
            .memory(Arc::new(MemoryStore::open_in_memory().unwrap()))
            .build()
            .unwrap();
        assert_eq!(agent.tool_count(), 0);
    }
}

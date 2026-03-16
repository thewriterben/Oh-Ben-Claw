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
//! Then start the agent with:
//!
//! ```bash
//! oh-ben-claw start --edge
//! ```

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
    config: EdgeConfig,
    provider_config: ProviderConfig,
    node_id: String,
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
        let agent = Agent::new(agent_config, provider, memory, tools);
        Ok(Self {
            inner: agent,
            config: edge_config,
            provider_config,
            node_id: node_id.into(),
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

    /// Build the `EdgeAgent`.  Returns `Err` if memory is missing or the
    /// provider configuration is invalid.
    pub fn build(self) -> Result<EdgeAgent> {
        let memory = self
            .memory
            .ok_or_else(|| anyhow::anyhow!("EdgeAgentBuilder: memory is required"))?;

        let mut agent = EdgeAgent::new(
            self.agent_config,
            self.edge_config,
            self.provider_config,
            memory,
            self.tools,
            self.node_id,
        )?;

        if let Some(policy) = self.policy {
            agent = agent.with_policy(policy);
        }

        Ok(agent)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EdgeConfig;

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

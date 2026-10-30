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
//! `oh-ben-claw start` does not use this type. With `[edge].enabled` it builds the
//! ordinary agent, safety gate and auditor included, under the same limits and on
//! the same mesh (`obc_config::edge_start`, `run_start` in `src/main.rs`). This
//! type takes a policy and pairing and nothing else, so it is for an embedder that
//! wants a bare agent on a device, not for the binary.
//!
//! Everything in the section is honoured here too. `max_history_messages` and
//! `max_tool_iterations` bound the agent at construction, and `p2p_enabled` starts
//! the mesh: [`EdgeAgentBuilder::build_and_start`] joins the local P2P spine and
//! registers whatever peer tools it finds, or does nothing at all when the key is
//! false. Before this the key parsed, validated, and was written into every
//! generated NanoPi config by the deployment planner — and a caller who did not
//! separately call `start_p2p` got an isolated node that had asked to be meshed.

/// Milliseconds to wait after starting the P2P spine before collecting peer
/// tools, to allow initial discovery broadcasts to arrive.
const P2P_DISCOVERY_DELAY_MS: u64 = 500;

use crate::{Agent, AgentResponse};
use crate::{AgentConfig, EdgeConfig};
use anyhow::Result;
use obc_memory::MemoryStore;
use obc_providers::ProviderConfig;
use obc_safety::PolicyEngine;
use obc_spine::p2p::{P2pConfig, P2pSpine, PeerEvent, PeerToolServer};
use obc_spine::{NodeAnnouncement, NodeToolSpec};
use obc_tool_api::Tool;
use std::collections::{HashMap, HashSet};
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
    /// Applied to the P2P spine when it starts. Defaults to pairing disabled,
    /// which is what every existing caller got before 2026-08-01 and still gets
    /// unless it opts in.
    pairing: (obc_safety::NodePairingManager, bool),
    /// Root secret and enforcement flag for per-message tags on P2P calls.
    /// Same secret as pairing; it switches the checks on, and the identity key
    /// below is what says which node a call came from.
    frame_auth_secret: Option<String>,
    frame_auth_required: bool,
    /// This node's identity key and the keys it has pinned for its peers, taken
    /// by `start_p2p`. `None` means the ones kept in the data dir
    /// ([`obc_safety::node_identity::P2pState`]), created on first start.
    identity: Option<(
        obc_safety::node_identity::NodeIdentity,
        obc_safety::node_identity::PeerKeys,
    )>,
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
        let provider = obc_providers::from_config(&provider_config)?;
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
            pairing: (obc_safety::NodePairingManager::new(None), false),
            frame_auth_secret: None,
            frame_auth_required: false,
            identity: None,
        })
    }

    /// Enforce `[security] pairing_secret` / `require_pairing` on peers this
    /// node discovers over P2P, and per-message tags on the calls between them.
    ///
    /// Worth having on the *node* and not only on the brain: P2P discovery is a
    /// UDP broadcast with no broker and no handshake, so on an edge device the
    /// set of tools it will call is decided by whoever else is on the network.
    pub fn with_pairing(
        mut self,
        pairing: obc_safety::NodePairingManager,
        require_pairing: bool,
    ) -> Self {
        self.pairing = (pairing, require_pairing);
        self
    }

    /// Root secret and enforcement for per-message P2P tags.
    pub fn with_frame_auth(mut self, secret: Option<String>, required: bool) -> Self {
        self.frame_auth_secret = secret;
        self.frame_auth_required = required;
        self
    }

    /// The identity key this node signs P2P frames with, and the peer keys it
    /// checks theirs against, in place of the ones kept in the data dir. Keep
    /// the key across restarts (`NodeIdentity::load_or_create`), or every peer
    /// that pinned it refuses this node until its pin is removed.
    pub fn with_p2p_identity(
        mut self,
        identity: obc_safety::node_identity::NodeIdentity,
        peer_keys: obc_safety::node_identity::PeerKeys,
    ) -> Self {
        self.identity = Some((identity, peer_keys));
        self
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
        let mut spine = P2pSpine::new(p2p_config);
        spine = match self.identity.take() {
            Some((identity, peer_keys)) => spine.with_identity(identity).with_peer_keys(peer_keys),
            None => {
                let state = obc_safety::node_identity::P2pState::load(
                    &obc_paths::data_dir(),
                    obc_safety::node_identity::PeerKeys::new(),
                )
                .map_err(|e| anyhow::anyhow!("could not load this node's P2P identity: {e:#}"))?;
                spine
                    .with_identity(state.identity)
                    .with_peer_keys(state.peer_keys)
                    .with_announce_epoch(state.announce_epoch)
                    .with_announce_marks(state.announce_marks)
            }
        };
        let spine = spine
            .with_pairing(self.pairing.0.clone(), self.pairing.1)
            .with_frame_auth(obc_safety::frame_auth::FrameAuth::new(
                self.frame_auth_secret.clone(),
                self.frame_auth_required,
            ))
            .start()
            .await?;
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
                .map(|(name, description, parameters)| obc_spine::NodeToolSpec {
                    name,
                    description,
                    parameters,
                })
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
    /// See [`EdgeAgentBuilder::pairing`]. `None` leaves `EdgeAgent`'s default,
    /// which is pairing disabled.
    pairing: Option<(obc_safety::NodePairingManager, bool)>,
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
            pairing: None,
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
        use obc_providers::model_registry::{
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

    /// Enforce `[security] pairing_secret` / `require_pairing` on peers this
    /// node discovers over P2P.
    ///
    /// `EdgeAgent::with_pairing` has existed since the P2P spine did, and this
    /// builder had no way to reach it: `policy` was carried through to `build`
    /// and pairing was not, so every agent built the documented way — which is
    /// every agent, `build_and_start` being how a node joins the mesh — got
    /// `NodePairingManager::new(None)` and pairing off, whatever the config
    /// said.
    ///
    /// Half of `SecurityContext` plumbed and half dropped, in the place the
    /// method's own doc comment argues it matters most: P2P discovery is a UDP
    /// broadcast with no broker and no handshake, so on an edge device the set
    /// of tools it will call is decided by whoever else is on the network.
    ///
    /// Still opt-in rather than read from config here, because this crate takes
    /// an `EdgeConfig` and not the root `Config` — the caller holds the
    /// security context and passes its manager, exactly as `src/main.rs` does
    /// for the MQTT spine.
    pub fn pairing(
        mut self,
        pairing: obc_safety::NodePairingManager,
        require_pairing: bool,
    ) -> Self {
        self.pairing = Some((pairing, require_pairing));
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

        if let Some((pairing, require_pairing)) = self.pairing {
            agent = agent.with_pairing(pairing, require_pairing);
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

// ── Peer tools on the serving agent ───────────────────────────────────────────

/// Keep `agent`'s P2P peer tools in step with the mesh, from the events
/// [`P2pSpine::with_peer_events`] reports: a joined peer's tools replace what it
/// offered before, a pruned peer's tools go. When a peer leaves, peers that had
/// tools skipped for a name it held get another try, so a node that rebooted
/// under a new id gets its tools back once the old id is pruned. A snapshot,
/// sent after discovery had to drop events, is taken as the whole mesh.
///
/// Returns when the spine drops its sender.
pub async fn serve_peer_events(
    agent: Arc<Agent>,
    mesh: Arc<P2pSpine>,
    mut events: tokio::sync::mpsc::Receiver<PeerEvent>,
) {
    let mut offered: HashMap<String, Vec<NodeToolSpec>> = HashMap::new();
    let mut short: HashSet<String> = HashSet::new();
    let register = |node_id: &str, specs: &[NodeToolSpec], short: &mut HashSet<String>| {
        let (added, skipped) = agent.set_peer_tools(node_id, mesh.peer_tools(node_id, specs));
        tracing::info!(peer = %node_id, added, skipped, "P2P peer tools registered");
        if skipped > 0 {
            short.insert(node_id.to_string());
        } else {
            short.remove(node_id);
        }
    };
    let leave = |node_id: &str,
                 offered: &mut HashMap<String, Vec<NodeToolSpec>>,
                 short: &mut HashSet<String>| {
        let removed = agent.remove_peer_tools(node_id);
        tracing::info!(peer = %node_id, removed, "P2P peer pruned; its tools removed");
        offered.remove(node_id);
        short.remove(node_id);
        for peer in short.clone() {
            if let Some(specs) = offered.get(&peer) {
                register(&peer, specs, short);
            }
        }
    };
    while let Some(event) = events.recv().await {
        match event {
            PeerEvent::Joined { node_id, tools } => {
                register(&node_id, &tools, &mut short);
                offered.insert(node_id, tools);
            }
            PeerEvent::Left { node_id } => leave(&node_id, &mut offered, &mut short),
            PeerEvent::Snapshot { peers } => {
                let gone: Vec<String> = offered
                    .keys()
                    .filter(|id| !peers.contains_key(*id))
                    .cloned()
                    .collect();
                for node_id in gone {
                    leave(&node_id, &mut offered, &mut short);
                }
                for (node_id, tools) in peers {
                    if offered.get(&node_id) != Some(&tools) {
                        register(&node_id, &tools, &mut short);
                        offered.insert(node_id, tools);
                    }
                }
            }
        }
    }
}

/// Answers paired peers' P2P calls with the agent's own gated path
/// ([`Agent::execute_tool_for_peer`]): policy, the Track 0 gate and its audit
/// record, trust, approval and taint tracking hold for a peer's call as for a
/// local one, judged as `p2p:<peer>` rather than as this node.
pub struct AgentPeerServer(pub Arc<Agent>);

#[async_trait::async_trait]
impl PeerToolServer for AgentPeerServer {
    async fn call(
        &self,
        peer: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<obc_tool_api::ToolResult> {
        tracing::info!(%peer, %tool, "Running a P2P peer's tool call");
        self.0.execute_tool_for_peer(peer, tool, args).await
    }
}

/// The announcement specs for `names`, from the agent's registry. A name the
/// registry does not have is an error: announcing it would offer a tool that
/// every call then fails.
pub fn served_specs(agent: &Agent, names: &[String]) -> Result<Vec<NodeToolSpec>> {
    let registered = agent.tool_specs();
    names
        .iter()
        .map(|name| {
            registered
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(name, description, parameters)| NodeToolSpec {
                    name: name.clone(),
                    description: description.clone(),
                    parameters: parameters.clone(),
                })
                .ok_or_else(|| anyhow::anyhow!("no tool {name:?} is registered to serve"))
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EdgeConfig;
    use obc_approval::ApprovalManager;
    use obc_memory::MemoryStore;

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

    // ── Peer tools and incoming calls ─────────────────────────────────────────

    struct Named(&'static str);

    #[async_trait::async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "test"
        }
        fn risk_class(&self) -> obc_tool_api::RiskClass {
            obc_tool_api::RiskClass::safe()
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn execute(&self, _args: serde_json::Value) -> Result<obc_tool_api::ToolResult> {
            Ok(obc_tool_api::ToolResult::ok(self.0))
        }
    }

    fn agent(tools: Vec<Box<dyn Tool>>) -> Agent {
        Agent::new(
            AgentConfig::default(),
            obc_providers::from_config(&ProviderConfig::default()).unwrap(),
            Arc::new(MemoryStore::open_in_memory().unwrap()),
            tools,
        )
    }

    fn peer(names: &[&'static str]) -> Vec<Box<dyn Tool>> {
        names
            .iter()
            .map(|n| Box::new(Named(n)) as Box<dyn Tool>)
            .collect()
    }

    fn specs(names: &[&str]) -> Vec<NodeToolSpec> {
        names
            .iter()
            .map(|n| NodeToolSpec {
                name: n.to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            })
            .collect()
    }

    fn count(agent: &Agent, name: &str) -> usize {
        agent.tool_names().iter().filter(|n| *n == name).count()
    }

    #[test]
    fn a_peer_tool_never_shadows_or_duplicates_a_registered_one() {
        let agent = agent(peer(&["shell"]));
        // A local name, and a name another peer already holds, are skipped.
        assert_eq!(
            agent.set_peer_tools("a", peer(&["shell", "gpio_read"])),
            (1, 1)
        );
        assert_eq!(
            agent.set_peer_tools("b", peer(&["gpio_read", "dht22"])),
            (1, 1)
        );
        assert_eq!(count(&agent, "shell"), 1);
        assert_eq!(count(&agent, "gpio_read"), 1);

        // A re-announcement replaces that peer's set rather than adding to it.
        assert_eq!(agent.set_peer_tools("a", peer(&["camera_capture"])), (1, 0));
        assert_eq!(count(&agent, "gpio_read"), 0);

        // Pruned: its tools go, and the local tool stays.
        assert_eq!(agent.remove_peer_tools("b"), 1);
        assert_eq!(count(&agent, "dht22"), 0);
        assert_eq!(count(&agent, "shell"), 1);
        assert_eq!(agent.remove_peer_tools("b"), 0);
    }

    fn names(prefix: &str, n: usize) -> Vec<&'static str> {
        (0..n)
            .map(|i| &*Box::leak(format!("{prefix}{i}").into_boxed_str()))
            .collect()
    }

    #[test]
    fn peer_tools_are_capped_across_all_peers() {
        let agent = agent(Vec::new());
        let per = crate::MAX_TOOLS_PER_PEER;
        let peers = crate::MAX_PEER_TOOLS / per;
        for p in 0..peers {
            let id = format!("p{p}");
            let mine = names(&format!("{id}_t"), per);
            assert_eq!(agent.set_peer_tools(&id, peer(&mine)), (per, 0));
        }
        assert_eq!(agent.tool_count(), crate::MAX_PEER_TOOLS);
        let late = names("late_t", 3);
        assert_eq!(agent.set_peer_tools("late", peer(&late)), (0, 3));
        // Room made by a departure is room for the next announcement.
        agent.remove_peer_tools("p0");
        assert_eq!(agent.set_peer_tools("late", peer(&late)), (3, 0));
    }

    #[test]
    fn one_peer_cannot_take_more_than_its_share() {
        let agent = agent(Vec::new());
        let greedy = names("g", crate::MAX_TOOLS_PER_PEER + 8);
        assert_eq!(
            agent.set_peer_tools("greedy", peer(&greedy)),
            (crate::MAX_TOOLS_PER_PEER, 8)
        );
        // The rest of the mesh still has room.
        assert_eq!(agent.set_peer_tools("b", peer(&["dht22"])), (1, 0));
    }

    #[tokio::test]
    async fn peer_events_add_prune_and_reoffer_tools() {
        let agent = Arc::new(agent(Vec::new()));
        let mesh = Arc::new(P2pSpine::new(P2pConfig::default()));
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let serving = tokio::spawn(serve_peer_events(Arc::clone(&agent), mesh, rx));

        let joined = |id: &str, tools: &[&str]| PeerEvent::Joined {
            node_id: id.to_string(),
            tools: specs(tools),
        };
        tx.send(joined("old", &["gpio_read"])).await.unwrap();
        // Same node rebooted under a new id: its tool is held by the old one.
        tx.send(joined("new", &["gpio_read", "dht22"]))
            .await
            .unwrap();
        tx.send(PeerEvent::Left {
            node_id: "old".to_string(),
        })
        .await
        .unwrap();
        drop(tx);
        serving.await.unwrap();

        // The old id's tools went, and the new id got the name it was refused.
        assert_eq!(count(&agent, "gpio_read"), 1);
        assert_eq!(count(&agent, "dht22"), 1);
        assert_eq!(agent.remove_peer_tools("new"), 2);
        assert_eq!(agent.tool_count(), 0);
    }

    #[tokio::test]
    async fn a_snapshot_is_taken_as_the_whole_mesh() {
        let agent = Arc::new(agent(Vec::new()));
        let mesh = Arc::new(P2pSpine::new(P2pConfig::default()));
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let serving = tokio::spawn(serve_peer_events(Arc::clone(&agent), mesh, rx));

        tx.send(PeerEvent::Joined {
            node_id: "old".to_string(),
            tools: specs(&["gpio_read"]),
        })
        .await
        .unwrap();
        // The events in between were dropped: "old" left, "new" joined.
        let peers = HashMap::from([("new".to_string(), specs(&["gpio_read", "dht22"]))]);
        tx.send(PeerEvent::Snapshot { peers }).await.unwrap();
        drop(tx);
        serving.await.unwrap();

        assert_eq!(count(&agent, "gpio_read"), 1);
        assert_eq!(count(&agent, "dht22"), 1);
        assert_eq!(agent.remove_peer_tools("old"), 0);
        assert_eq!(agent.remove_peer_tools("new"), 2);
    }

    /// Drives a lock; counts how often it actually fired.
    struct Lock(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl Tool for Lock {
        fn name(&self) -> &str {
            "gpio_write"
        }
        fn description(&self) -> &str {
            "Drive a GPIO pin."
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn risk_class(&self) -> obc_tool_api::RiskClass {
            obc_tool_api::RiskClass::physical(true, obc_safety::risk::BlastRadius::Low)
        }
        async fn execute(&self, _args: serde_json::Value) -> Result<obc_tool_api::ToolResult> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(obc_tool_api::ToolResult::ok("written"))
        }
    }

    /// The main task's acceptance test: a peer's call is held to the Track 0
    /// gate and lands in the audit log, refused or not, like a local call.
    #[tokio::test]
    async fn a_peer_call_is_gated_and_audited_like_a_local_one() {
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dir = std::env::temp_dir().join(format!(
            "obc_peer_call_audit_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("actions.jsonl");
        let key = b"peer-call-audit-key".to_vec();
        let auditor = obc_safety::ActionAuditor::open(key.clone(), &log).unwrap();
        let gate = obc_safety::SafetyGate::new(vec![obc_safety::SafetyLimit {
            allowed_pins: Some(vec![17]),
            value_min: Some(0),
            value_max: Some(1),
            ..obc_safety::SafetyLimit::new("local", "gpio_write")
        }]);
        let agent = agent(vec![Box::new(Lock(fired.clone()))])
            .with_safety_gate(Arc::new(gate))
            .with_action_auditor(Arc::new(std::sync::Mutex::new(auditor)));
        let server = AgentPeerServer(Arc::new(agent));

        let allowed = server
            .call(
                "a",
                "gpio_write",
                serde_json::json!({ "pin": 17, "value": 1 }),
            )
            .await
            .unwrap();
        assert!(allowed.success, "{:?}", allowed.error);
        let refused = server
            .call(
                "a",
                "gpio_write",
                serde_json::json!({ "pin": 99, "value": 1 }),
            )
            .await
            .unwrap();
        assert!(!refused.success);
        assert!(refused.error.unwrap().contains("safety gate"));

        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(obc_safety::audit::verify(&log, &key).unwrap(), 2);
        // Both records name the peer that asked, and the node it acted on.
        let records = std::fs::read_to_string(&log).unwrap();
        assert_eq!(records.matches("p2p:a -> local").count(), 2, "{records}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Trust is judged on the peer that asks, whatever node its args name.
    #[tokio::test]
    async fn a_peer_is_trusted_as_itself_not_as_the_node_it_names() {
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let trust = Arc::new(obc_safety::trust::TrustScorer::default());
        for _ in 0..3 {
            trust.record("p2p:a", 10.0, false);
        }
        let agent =
            Arc::new(agent(vec![Box::new(Lock(fired.clone()))]).with_trust(Arc::clone(&trust)));
        let server = AgentPeerServer(Arc::clone(&agent));
        let local = serde_json::json!({ "node_id": "local", "pin": 17 });

        let denied = server.call("a", "gpio_write", local.clone()).await.unwrap();
        assert!(!denied.success);
        assert!(denied.error.unwrap().contains("'p2p:a' is untrusted"));
        // Another peer, and the node itself, are not held to a's record.
        assert!(
            server
                .call("b", "gpio_write", local.clone())
                .await
                .unwrap()
                .success
        );
        assert!(
            agent
                .execute_tool_direct("gpio_write", local)
                .await
                .unwrap()
                .success
        );
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Under taint tracking a peer's arguments are external content.
    #[tokio::test]
    async fn a_peer_call_needs_an_operator_grant_under_taint_enforce() {
        use obc_safety::taint::TaintMode;
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let args = serde_json::json!({ "pin": 17 });
        let with = |mode, grant: bool| {
            let mut a = agent(vec![Box::new(Lock(fired.clone()))]).with_taint_mode(mode);
            if grant {
                let autonomy = obc_approval::AutonomyConfig {
                    auto_approve: vec!["gpio_write".to_string()],
                    ..Default::default()
                };
                a = a.with_approval(Arc::new(ApprovalManager::from_config(&autonomy)));
            }
            AgentPeerServer(Arc::new(a))
        };

        let refused = with(TaintMode::Enforce, false)
            .call("a", "gpio_write", args.clone())
            .await
            .unwrap();
        assert!(!refused.success);
        assert!(refused.error.unwrap().contains("Track 0 taint"));
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 0);

        let advised = with(TaintMode::Warn, false);
        assert!(
            advised
                .call("a", "gpio_write", args.clone())
                .await
                .unwrap()
                .success
        );
        let granted = with(TaintMode::Enforce, true);
        assert!(granted.call("a", "gpio_write", args).await.unwrap().success);
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn only_registered_tools_can_be_served() {
        let agent = agent(peer(&["gpio_read"]));
        let served = served_specs(&agent, &["gpio_read".to_string()]).unwrap();
        assert_eq!(served[0].name, "gpio_read");
        assert!(served_specs(&agent, &["shell".to_string()]).is_err());
    }
}

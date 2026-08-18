//! Oh-Ben-Claw Communication Spine
//!
//! This module implements the MQTT-based communication backbone that connects
//! the central brain agent with all distributed peripheral nodes.
//!
//! # Topic Hierarchy
//!
//! ```text
//! obc/
//! +-- nodes/
//! |   +-- {node_id}/
//! |   |   +-- announce    # Node publishes its capabilities on connect
//! |   |   +-- heartbeat   # Node publishes a heartbeat every N seconds
//! |   |   +-- status      # Node publishes its current status
//! +-- tools/
//! |   +-- {node_id}/
//! |   |   +-- call/{tool_name}   # Brain publishes a tool call request
//! |   |   +-- result/{call_id}  # Node publishes the tool call result
//! +-- broadcast/
//!     +-- command    # Brain publishes a command to all nodes
//! ```

pub mod action;
pub mod actuator;
/// The fleet coordinator's MQTT bridge — heartbeat ingress and assignment
/// egress. Moved here from `obc_fleet` on 2026-08-13; it was that module's
/// only edge into this tree, and `lora_mesh` already bridged the other
/// transport into the same coordinator from this side.
pub mod fleet_bridge;
pub mod lora_gateway;
pub mod lora_mesh;
pub mod mesh_supervisor;
pub mod p2p;
/// The spine's `SpeechSink` — utterances out over MQTT. Moved here from
/// `audio::suite` on 2026-08-13, where it was holding a `SpineClient`.
pub mod speech;

pub use action::SpineActionSink;
pub use actuator::SpineActuatorSink;

use anyhow::{bail, Result};
use async_trait::async_trait;
use obc_tool_api::{Tool, ToolResult};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex, RwLock};

/// The MQTT topic prefix for all Oh-Ben-Claw messages.
pub const TOPIC_PREFIX: &str = "obc";

/// Topic for node announcements.
pub fn topic_announce(node_id: &str) -> String {
    format!("{TOPIC_PREFIX}/nodes/{node_id}/announce")
}

/// Topic for node heartbeats.
pub fn topic_heartbeat(node_id: &str) -> String {
    format!("{TOPIC_PREFIX}/nodes/{node_id}/heartbeat")
}

/// Topic for tool call requests from the brain to a specific node.
pub fn topic_tool_call(node_id: &str, tool_name: &str) -> String {
    format!("{TOPIC_PREFIX}/tools/{node_id}/call/{tool_name}")
}

/// Topic for tool call results from a node back to the brain.
pub fn topic_tool_result(node_id: &str, call_id: &str) -> String {
    format!("{TOPIC_PREFIX}/tools/{node_id}/result/{call_id}")
}

/// Topic for broadcast commands from the brain to all nodes.
pub fn topic_broadcast() -> String {
    format!("{TOPIC_PREFIX}/broadcast/command")
}

// ── Node Announcement ────────────────────────────────────────────────────────

/// A description of a single tool exposed by a peripheral node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// The announcement payload published by a peripheral node on connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAnnouncement {
    pub node_id: String,
    pub board: String,
    pub firmware_version: String,
    pub tools: Vec<NodeToolSpec>,
    #[serde(default)]
    pub metadata: Value,
}

// ── Tool Call Protocol ───────────────────────────────────────────────────────

/// A tool call request published by the brain to a peripheral node.
///
/// `ctr`/`mac` as in [`ToolCallResult`] — `SPINE-AUTH.md` §3.3, the tag as a
/// field. This is the direction §2 calls safety-critical, because a tool call
/// actuates: a forged one drives a pin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub args: Value,
    /// Who is calling, for a receiver that has to pick a key.
    ///
    /// Absent until 2026-08-01, and its absence was the reason a P2P receiver
    /// could not verify anything: the frame said what to do and never said who
    /// was asking. An attacker chooses this field freely — that is fine, and it
    /// is the point. It selects which key the tag is checked against, and
    /// claiming to be another node means being unable to produce that node's
    /// tag. Asserted here, proven by `mac`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Monotonic per-node counter, covered by `mac`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctr: Option<u32>,
    /// Truncated HMAC over the request body, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

impl ToolCallRequest {
    /// The bytes the tag covers: this request with `ctr` and `mac` removed.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.ctr = None;
        unsigned.mac = None;
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }

    /// Attach a counter and tag for `key_node`, if `frame_auth` has a secret.
    ///
    /// `key_node` is whose key signs this, which is **not** the same as who the
    /// call is addressed to. On MQTT the brain signs with the destination
    /// node's derived key — that key is a shared secret between exactly those
    /// two, so it authenticates the sender to the receiver. On P2P the caller
    /// signs with its own, and sets `from` so the receiver knows which to check.
    ///
    /// A no-op without a secret, which is the shipped default — an unsigned
    /// request on a deployment that never configured one is not a downgrade, it
    /// is the only thing that has ever been sent.
    pub fn sign(
        &mut self,
        frame_auth: &obc_safety::frame_auth::FrameAuth,
        counters: &obc_safety::frame_auth::OutboundCounters,
        key_node: &str,
    ) {
        if !frame_auth.is_enabled() {
            return;
        }
        let Some(ctr) = counters.next(key_node) else {
            // Exhausted rather than wrapped. Leaving it unsigned is the honest
            // outcome: the receiver refuses it when enforcing, which is a loud
            // failure, and wrapping would be a silent one.
            tracing::error!(
                node = %key_node,
                "Outbound counter exhausted; sending unsigned. Re-provision this node's key."
            );
            return;
        };
        self.ctr = Some(ctr);
        self.mac = frame_auth.tag_outbound(key_node, ctr, &self.signed_bytes());
    }
}

/// A tool call result published by a peripheral node back to the brain.
///
/// `ctr` and `mac` are `SPINE-AUTH.md` §3.3: on MQTT and P2P the tag rides as
/// fields rather than as a byte prefix, since neither transport has a frame
/// budget worth defending. Both are optional on the wire so a node that has not
/// been upgraded still parses — whether an untagged result is *accepted* is
/// `[security] require_frame_auth`, not a parsing question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Monotonic per-node counter, covered by `mac`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctr: Option<u32>,
    /// Truncated HMAC over the result body, hex. See `obc_safety::frame_auth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

impl ToolCallResult {
    /// The exact bytes the tag covers: the result with `ctr` and `mac` removed.
    ///
    /// Signing the message minus its own signature is the only shape that works,
    /// and re-serializing a canonical subset is the only way to get the same
    /// bytes on both ends — the sender's field order and spacing are not
    /// something the receiver can reconstruct from the text it received.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.ctr = None;
        unsigned.mac = None;
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }
}

// ── Pending Call Registry ────────────────────────────────────────────────────

/// A map from call_id to a one-shot sender waiting for the result.
type PendingCalls = Arc<Mutex<HashMap<String, oneshot::Sender<ToolCallResult>>>>;

/// A map from node_id to a list of that node's tool specs.
type NodeRegistry = Arc<RwLock<HashMap<String, NodeAnnouncement>>>;

// ── Spine Client ───────────────────────────────────────────────────────────────

/// A handler for messages on a subscribed topic filter: `(topic, payload)`.
pub type MessageHandler = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

/// Registered `(topic_filter, handler)` pairs the event loop dispatches to.
type Handlers = Arc<std::sync::Mutex<Vec<(String, MessageHandler)>>>;

/// MQTT topic-filter match (`+` = one level, `#` = rest).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let fs: Vec<&str> = filter.split('/').collect();
    let ts: Vec<&str> = topic.split('/').collect();
    for (i, f) in fs.iter().enumerate() {
        match *f {
            "#" => return true,
            "+" => {
                if i >= ts.len() {
                    return false;
                }
            }
            seg => {
                if i >= ts.len() || ts[i] != seg {
                    return false;
                }
            }
        }
    }
    fs.len() == ts.len()
}

// ── Admission ────────────────────────────────────────────────────────────────

/// What the brain does with an announcement, before its tools reach the registry.
///
/// A separate type, and a pure function to produce it, because the decision
/// otherwise lives inside an MQTT poll loop that no test can reach. The loop
/// should be plumbing; this is the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Register the node's tools.
    Admit,
    /// Refuse: the node did not prove who it is and `require_pairing` is set.
    Refuse { reason: String },
}

/// Decide whether an announcement may register its tools.
///
/// Until 2026-08-01 there was no decision: `NodePairingManager` was constructed
/// at startup, `pair_node` had no callers, and `[security] require_pairing = true`
/// was validated at boot and then gated nothing — a node could announce any tools
/// it liked and the brain would register them. `security/trust.rs` opens by
/// asserting "OBC already authenticates nodes (HMAC pairing) … that trust is
/// *static*" and builds behavioural hardening on that premise. The premise was
/// false, which made the hardening on top of it a decoration.
///
/// Pairing is still evaluated when `require_pairing` is off, so status is
/// observable — you can see which nodes would be refused before turning the key
/// on. Only the *refusal* is gated.
pub fn admit_announcement(
    pairing: &obc_safety::NodePairingManager,
    require_pairing: bool,
    node_id: &str,
    metadata: &Value,
) -> Admission {
    let status = pairing.pair_node(node_id, Some(metadata));
    match status {
        obc_safety::PairingStatus::Paired => Admission::Admit,
        _ if !require_pairing => Admission::Admit,
        other => Admission::Refuse {
            reason: other.to_string(),
        },
    }
}

/// A client for the Oh-Ben-Claw MQTT communication spine.
pub struct SpineClient {
    config: SpineConfig,
    client_id: String,
    mqtt_client: Option<AsyncClient>,
    pending_calls: PendingCalls,
    node_registry: NodeRegistry,
    handlers: Handlers,
    /// Node pairing, and whether an unpaired node is refused or merely noted.
    pairing: obc_safety::NodePairingManager,
    require_pairing: bool,
    /// Per-message authentication for inbound results (`SPINE-AUTH.md` §3.3).
    frame_auth: Arc<obc_safety::frame_auth::FrameAuth>,
    /// Monotonic counters for outbound tool calls, one per node.
    out_counters: Arc<obc_safety::frame_auth::OutboundCounters>,
}

impl SpineClient {
    /// Create a new `SpineClient` from configuration.
    pub fn new(config: SpineConfig, client_id: impl Into<String>) -> Self {
        Self {
            config,
            client_id: client_id.into(),
            mqtt_client: None,
            pending_calls: Arc::new(Mutex::new(HashMap::new())),
            node_registry: Arc::new(RwLock::new(HashMap::new())),
            handlers: Arc::new(std::sync::Mutex::new(Vec::new())),
            // Defaults keep every existing caller's behaviour: no secret means
            // `pair_node` marks everything Paired, and `require_pairing` is off.
            // A deployment opts in through `with_pairing`.
            pairing: obc_safety::NodePairingManager::new(None),
            require_pairing: false,
            frame_auth: Arc::new(obc_safety::frame_auth::FrameAuth::new(None, false)),
            out_counters: Arc::new(obc_safety::frame_auth::OutboundCounters::new()),
        }
    }

    /// Authenticate inbound tool-call results (`[security] require_frame_auth`).
    ///
    /// A result is not a command, which is why this is easy to leave out and
    /// worth not leaving out: results land in world memory, and reflexes act on
    /// world memory without waking the model. A forged reading fires a rule.
    pub fn with_frame_auth(mut self, frame_auth: obc_safety::frame_auth::FrameAuth) -> Self {
        self.frame_auth = Arc::new(frame_auth);
        self
    }

    /// Enforce `[security] pairing_secret` / `require_pairing` on announcements.
    ///
    /// Separate from `new` so the security config reaches the spine explicitly
    /// at the one place that wires them together, rather than the spine reaching
    /// into a global.
    pub fn with_pairing(
        mut self,
        pairing: obc_safety::NodePairingManager,
        require_pairing: bool,
    ) -> Self {
        self.pairing = pairing;
        self.require_pairing = require_pairing;
        self
    }

    /// Connect to the MQTT broker and spawn the event loop.
    ///
    /// Returns the `SpineClient` and a handle to the background event loop task.
    pub async fn connect(mut self) -> Result<Arc<Self>> {
        let mut opts = MqttOptions::new(&self.client_id, &self.config.host, self.config.port);
        opts.set_keep_alive(Duration::from_secs(30));
        opts.set_clean_session(true);

        if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            opts.set_credentials(user, pass);
        }

        let (client, mut event_loop) = AsyncClient::new(opts, 128);
        self.mqtt_client = Some(client.clone());

        // Subscribe to node announcements and tool results
        client
            .subscribe(format!("{TOPIC_PREFIX}/nodes/+/announce"), QoS::AtLeastOnce)
            .await?;
        client
            .subscribe(format!("{TOPIC_PREFIX}/tools/+/result/+"), QoS::AtLeastOnce)
            .await?;

        let pending_calls = Arc::clone(&self.pending_calls);
        let node_registry = Arc::clone(&self.node_registry);
        let handlers = Arc::clone(&self.handlers);
        let pairing = self.pairing.clone();
        let require_pairing = self.require_pairing;
        let frame_auth = Arc::clone(&self.frame_auth);

        // Spawn the event loop handler
        tokio::spawn(async move {
            loop {
                match event_loop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let topic = publish.topic.clone();
                        let payload = publish.payload.clone();

                        // Dispatch to any registered generic handlers (fleet
                        // heartbeats, custom subscriptions). Locked briefly; the
                        // handler call is synchronous.
                        {
                            let hs = handlers.lock().unwrap_or_else(|p| p.into_inner());
                            for (filter, handler) in hs.iter() {
                                if topic_matches(filter, &topic) {
                                    handler(&topic, &payload);
                                }
                            }
                        }

                        if topic.contains("/announce") {
                            // Parse node announcement and register it
                            if let Ok(announcement) =
                                serde_json::from_slice::<NodeAnnouncement>(&payload)
                            {
                                let node_id = announcement.node_id.clone();
                                match admit_announcement(
                                    &pairing,
                                    require_pairing,
                                    &node_id,
                                    &announcement.metadata,
                                ) {
                                    Admission::Admit => {
                                        tracing::info!(
                                            node_id = %node_id,
                                            board = %announcement.board,
                                            tool_count = announcement.tools.len(),
                                            "Node announced on spine"
                                        );
                                        node_registry.write().await.insert(node_id, announcement);
                                    }
                                    Admission::Refuse { reason } => {
                                        // Loudly: a node the operator expects to
                                        // see, silently absent, is a worse
                                        // afternoon than a refusal they can read.
                                        tracing::warn!(
                                            node_id = %node_id,
                                            board = %announcement.board,
                                            tool_count = announcement.tools.len(),
                                            %reason,
                                            "Refused a node announcement: its tools are NOT registered \
                                             ([security] require_pairing is on)"
                                        );
                                    }
                                }
                            }
                        } else if topic.contains("/result/") {
                            // Parse tool call result and wake the waiting caller
                            if let Ok(result) = serde_json::from_slice::<ToolCallResult>(&payload) {
                                // `obc/tools/{node}/result/{call_id}` — the node
                                // is the third segment, and it is the identity
                                // the tag is verified against.
                                let node = topic.split('/').nth(2).unwrap_or_default().to_string();
                                if let obc_safety::frame_auth::FrameVerdict::Reject { reason } =
                                    frame_auth.verify_inbound(
                                        &node,
                                        result.ctr,
                                        result.mac.as_deref(),
                                        &result.signed_bytes(),
                                    )
                                {
                                    tracing::warn!(
                                        node = %node,
                                        call_id = %result.call_id,
                                        %reason,
                                        "Refused a tool result: the caller will time out rather \
                                         than act on it ([security] require_frame_auth is on)"
                                    );
                                    continue;
                                }
                                let call_id = result.call_id.clone();
                                if let Some(sender) = pending_calls.lock().await.remove(&call_id) {
                                    let _ = sender.send(result);
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("MQTT event loop error: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });

        tracing::info!(
            host = %self.config.host,
            port = self.config.port,
            client_id = %self.client_id,
            "Connected to MQTT spine"
        );

        Ok(Arc::new(self))
    }

    /// Publish a node announcement to the spine.
    pub async fn announce(&self, announcement: &NodeAnnouncement) -> Result<()> {
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;
        let topic = topic_announce(&announcement.node_id);
        let payload = serde_json::to_vec(announcement)?;
        client
            .publish(topic, QoS::AtLeastOnce, true, payload)
            .await?;
        Ok(())
    }

    /// Invoke a tool on a specific peripheral node via the spine.
    pub async fn invoke_tool(
        &self,
        node_id: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolCallResult> {
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;

        let call_id = uuid::Uuid::new_v4().to_string();
        let mut request = ToolCallRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.to_string(),
            args,
            // The brain does not name itself: on MQTT the key is derived from
            // the destination node, so `from` would be a label rather than a
            // selector. The far end already knows whose key it holds.
            from: None,
            ctr: None,
            mac: None,
        };
        request.sign(&self.frame_auth, &self.out_counters, node_id);

        let (tx, rx) = oneshot::channel();
        self.pending_calls.lock().await.insert(call_id.clone(), tx);

        let request_topic = topic_tool_call(node_id, tool_name);
        let payload = serde_json::to_vec(&request)?;
        client
            .publish(request_topic, QoS::AtLeastOnce, false, payload)
            .await?;

        let timeout = Duration::from_secs(self.config.tool_timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => bail!("Tool call channel dropped for call_id={}", call_id),
            Err(_) => {
                self.pending_calls.lock().await.remove(&call_id);
                bail!(
                    "Tool call timed out after {}s (node={}, tool={})",
                    self.config.tool_timeout_secs,
                    node_id,
                    tool_name
                )
            }
        }
    }

    /// Return all currently known peripheral nodes and their tool specs.
    pub async fn known_nodes(&self) -> HashMap<String, NodeAnnouncement> {
        self.node_registry.read().await.clone()
    }

    /// Publish a JSON payload to an arbitrary spine topic (used by reflex
    /// actions, escalation events, etc.). Errors if the spine isn't connected.
    pub async fn publish(&self, topic: &str, payload: &Value) -> Result<()> {
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;
        let bytes = serde_json::to_vec(payload)?;
        client
            .publish(topic, QoS::AtLeastOnce, false, bytes)
            .await?;
        Ok(())
    }

    /// Subscribe a generic handler to a topic filter (e.g. fleet heartbeats).
    /// The handler is invoked with `(topic, payload)` for every matching message.
    /// Can be called after [`connect`](Self::connect).
    pub async fn subscribe_handler(
        &self,
        filter: impl Into<String>,
        handler: MessageHandler,
    ) -> Result<()> {
        let filter = filter.into();
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;
        client.subscribe(filter.clone(), QoS::AtLeastOnce).await?;
        self.handlers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((filter, handler));
        Ok(())
    }

    /// Build a list of `Box<dyn Tool>` from all currently known MQTT nodes.
    pub async fn build_mqtt_tools(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        let registry = self.node_registry.read().await;
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        for (node_id, announcement) in registry.iter() {
            for spec in &announcement.tools {
                tools.push(Box::new(MqttNodeTool {
                    node_id: node_id.clone(),
                    spec: spec.clone(),
                    spine: Arc::clone(self),
                }));
            }
        }
        tools
    }
}

// ── MQTT Node Tool ────────────────────────────────────────────────────────────

/// A tool that delegates execution to a peripheral node via the MQTT spine.
struct MqttNodeTool {
    node_id: String,
    spec: NodeToolSpec,
    spine: Arc<SpineClient>,
}

#[async_trait]
impl Tool for MqttNodeTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    /// Physical unless a node says otherwise — and no node can say otherwise yet.
    ///
    /// `NodeToolSpec` carries `name`, `description` and `parameters`. There is
    /// no risk field, so the host cannot know whether `gpio_write` on a remote
    /// node drives an LED or a door strike. Until 2026-08-18 this impl simply
    /// omitted `risk_class`, taking the trait default — `RiskClass::safe()`,
    /// non-physical — for every tool any peripheral node has ever announced.
    ///
    /// What that cost is specific. `track0_authorize` opens with
    /// `if !risk.physical { return Ok(()); }`, so for node tools it skipped both
    /// of the things it exists to do: the `SafetyGate` limit check on
    /// `(node_id, tool, pin, value)`, and the tamper-evident audit record. The
    /// function extracts `pin` and `value` from the arguments — it was written
    /// for exactly these calls, and never saw one.
    ///
    /// Reversible with a low blast radius, deliberately. That is enough to route
    /// the call through the gate and into the audit chain without forcing a
    /// prompt on every sensor read: `requires_per_call_approval` is
    /// `physical && (!reversible || High)`. Claiming High here would be the
    /// honest-looking choice that trains an operator to approve everything.
    ///
    /// The real fix is a risk field on the announcement, so a node declares what
    /// its own tools do and the absence of a declaration still lands here. That
    /// is a protocol and firmware change; this is the host half, and it is the
    /// half that was silently wrong.
    fn risk_class(&self) -> obc_tool_api::RiskClass {
        obc_tool_api::RiskClass::physical(true, obc_tool_api::BlastRadius::Low)
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let result = self
            .spine
            .invoke_tool(&self.node_id, &self.spec.name, args)
            .await?;

        if result.ok {
            Ok(ToolResult::ok(result.output.unwrap_or_default()))
        } else {
            Ok(ToolResult::err(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_matching_handles_wildcards() {
        assert!(topic_matches(
            "obc/fleet/heartbeat/+",
            "obc/fleet/heartbeat/node-1"
        ));
        assert!(!topic_matches(
            "obc/fleet/heartbeat/+",
            "obc/fleet/heartbeat/node-1/extra"
        ));
        assert!(!topic_matches(
            "obc/fleet/heartbeat/+",
            "obc/fleet/assign/node-1"
        ));
        assert!(topic_matches("obc/#", "obc/anything/at/all"));
        assert!(topic_matches(
            "obc/fleet/heartbeat/n1",
            "obc/fleet/heartbeat/n1"
        ));
        assert!(!topic_matches(
            "obc/fleet/heartbeat/+",
            "obc/fleet/heartbeat"
        ));
    }

    #[test]
    fn topic_formats_are_correct() {
        assert_eq!(topic_announce("node-1"), "obc/nodes/node-1/announce");
        assert_eq!(topic_heartbeat("node-1"), "obc/nodes/node-1/heartbeat");
        assert_eq!(
            topic_tool_call("node-1", "camera_capture"),
            "obc/tools/node-1/call/camera_capture"
        );
        assert_eq!(
            topic_tool_result("node-1", "call-abc"),
            "obc/tools/node-1/result/call-abc"
        );
        assert_eq!(topic_broadcast(), "obc/broadcast/command");
    }

    #[test]
    fn node_announcement_serializes_correctly() {
        let announcement = NodeAnnouncement {
            node_id: "esp32-s3-kitchen".to_string(),
            board: "waveshare-esp32-s3-touch-lcd-2.1".to_string(),
            firmware_version: "0.1.0".to_string(),
            tools: vec![NodeToolSpec {
                name: "camera_capture".to_string(),
                description: "Capture a JPEG image.".to_string(),
                parameters: serde_json::json!({}),
            }],
            metadata: serde_json::json!({"location": "kitchen"}),
        };
        let json = serde_json::to_string(&announcement).unwrap();
        assert!(json.contains("esp32-s3-kitchen"));
        assert!(json.contains("camera_capture"));
    }

    #[test]
    fn spine_config_defaults_are_sensible() {
        let config = SpineConfig::default();
        assert_eq!(config.kind, "mqtt");
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 1883);
        assert!(!config.tls);
        assert_eq!(config.tool_timeout_secs, 30);
    }
}

// ── The spine's own configuration blocks ────────────────────────────────────
// Moved here from the root config module on 2026-08-13, with the serde default
// helpers only they use. That module re-exports both names, so every call site
// outside this directory is unchanged.
//
// They were this module's entire dependency on anything still in the core
// tree -- four references, and two of the seven remaining cycles ran through
// them. The rule is the one every extracted crate here already follows: the
// module owns its config block, and the root `Config` composes it.
//
// `default_tool_timeout_secs` came too. The lift script refuses to move a
// helper anything else still references, and checked.

fn default_bus_host() -> String {
    "localhost".to_string()
}
fn default_bus_kind() -> String {
    "mqtt".to_string()
}
fn default_bus_port() -> u16 {
    1883
}
fn default_tool_timeout_secs() -> u64 {
    30
}
fn default_mesh_escalated_probe_interval_ms() -> u64 {
    300_000 // 5 min — slow enough not to hammer a dead node, fast enough to self-heal
}
fn default_mesh_recovery_interval_ms() -> u64 {
    30_000
}
fn default_mesh_stale_ms() -> u64 {
    60_000
}
fn default_mesh_tick_ms() -> u64 {
    5_000
}

/// Configuration for the MQTT communication spine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpineConfig {
    /// The spine kind: `"mqtt"` (default) or `"p2p"` (broker-free local mesh).
    #[serde(default = "default_bus_kind")]
    pub kind: String,
    /// The MQTT broker hostname.
    #[serde(default = "default_bus_host")]
    pub host: String,
    /// The MQTT broker port.
    #[serde(default = "default_bus_port")]
    pub port: u16,
    /// Whether to use TLS for the MQTT connection.
    #[serde(default)]
    pub tls: bool,
    /// Path to a custom CA certificate file for TLS verification.
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    /// Path to a client certificate file for mTLS authentication.
    #[serde(default)]
    pub client_cert_path: Option<String>,
    /// Path to a client private key file for mTLS authentication.
    #[serde(default)]
    pub client_key_path: Option<String>,
    /// Optional MQTT username.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional MQTT password.
    #[serde(default)]
    pub password: Option<String>,
    /// Timeout in seconds for tool call responses from peripheral nodes.
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
    // ── P2P-specific fields (used when `kind = "p2p"`) ─────────────────────
    /// Unique identifier for this node in the P2P mesh.
    /// Defaults to a random UUID prefix if not set.
    #[serde(default)]
    pub p2p_node_id: Option<String>,
    /// Local address to bind the P2P TCP server to (default: `"0.0.0.0"`).
    #[serde(default)]
    pub p2p_bind_host: Option<String>,
    /// TCP port on which this node accepts P2P tool-call connections (default: 44445).
    #[serde(default)]
    pub p2p_tcp_port: Option<u16>,
    /// UDP port used for P2P peer discovery broadcasts (default: 44444).
    #[serde(default)]
    pub p2p_discovery_port: Option<u16>,
    /// Seconds after which a silent peer is removed from the P2P registry (default: 60).
    #[serde(default)]
    pub p2p_peer_timeout_secs: Option<u64>,
    /// How often (in seconds) to broadcast a P2P presence announcement (default: 10).
    #[serde(default)]
    pub p2p_announce_interval_secs: Option<u64>,
}

impl Default for SpineConfig {
    fn default() -> Self {
        Self {
            kind: default_bus_kind(),
            host: default_bus_host(),
            port: default_bus_port(),
            tls: false,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            username: None,
            password: None,
            tool_timeout_secs: default_tool_timeout_secs(),
            p2p_node_id: None,
            p2p_bind_host: None,
            p2p_tcp_port: None,
            p2p_discovery_port: None,
            p2p_peer_timeout_secs: None,
            p2p_announce_interval_secs: None,
        }
    }
}

/// Mesh supervisor (Phase B "fold mesh into brain"): a host control loop that turns the
/// mesh facts in world memory into a derived per-node health view and optional
/// autonomous recovery commands. Requires `[perception].world_memory`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshSupervisorConfig {
    /// Enable the supervisor loop.
    #[serde(default)]
    pub enabled: bool,
    /// A node with no mesh message newer than this (ms) is considered offline.
    #[serde(default = "default_mesh_stale_ms")]
    pub stale_ms: u64,
    /// Supervisor tick cadence (ms).
    #[serde(default = "default_mesh_tick_ms")]
    pub tick_ms: u64,
    /// Command to auto-issue to a node that has gone offline (e.g. `"capabilities"` to
    /// ping it). `None` = observe-only (record health, never send).
    #[serde(default)]
    pub recover: Option<String>,
    /// Minimum interval (ms) between recovery commands to the same node.
    #[serde(default = "default_mesh_recovery_interval_ms")]
    pub min_recovery_interval_ms: u64,
    /// A node continuously offline for at least this long (ms) is escalated (presumed
    /// lost): fast recovery pings stop and a `mesh.<node>.escalation` fact is raised
    /// (cleared automatically if the node returns). `0` disables escalation.
    #[serde(default)]
    pub escalate_after_ms: u64,
    /// After escalation, an escalated node is not abandoned: it still gets a *slow*
    /// "are you back?" probe at this interval (ms), so a node whose passive beacons are
    /// lost to RF but which still answers a direct command recovers on its own (the
    /// reply refreshes `last_seen` and the next tick clears the escalation). Should be
    /// well above `min_recovery_interval_ms` so a genuinely dead node isn't hammered.
    /// `0` disables the escalated probe (revert to "give up entirely" on escalation).
    #[serde(default = "default_mesh_escalated_probe_interval_ms")]
    pub escalated_probe_interval_ms: u64,
}

impl Default for MeshSupervisorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stale_ms: default_mesh_stale_ms(),
            tick_ms: default_mesh_tick_ms(),
            recover: None,
            min_recovery_interval_ms: default_mesh_recovery_interval_ms(),
            escalate_after_ms: 0,
            escalated_probe_interval_ms: default_mesh_escalated_probe_interval_ms(),
        }
    }
}

//! Oh-Ben-Claw Peer-to-Peer Spine
//!
//! This module implements a broker-free communication layer that lets
//! Oh-Ben-Claw nodes discover each other on the local network and exchange
//! tool calls directly — no central MQTT broker required.
//!
//! # Protocol Overview
//!
//! ## Discovery (UDP broadcast)
//!
//! Each node periodically broadcasts a `P2pAnnounce` JSON datagram on the
//! configured `discovery_port` (default 44444) to `255.255.255.255`.  Any
//! node that receives the datagram learns about the sender's TCP address and
//! registers it in the local peer registry.
//!
//! ## Tool calls (direct TCP)
//!
//! Each node listens on a TCP port (`tcp_port`, default 44445).  Messages are
//! framed with a 4-byte big-endian length prefix followed by UTF-8 JSON:
//!
//! ```text
//! ┌──────────────────────────┐
//! │ len: u32 big-endian (4B) │
//! │ JSON payload (len bytes) │
//! └──────────────────────────┘
//! ```
//!
//! Both `ToolCallRequest` and `ToolCallResult` use the same structures as the
//! MQTT spine, so existing serialisation code is reused unchanged.
//!
//! ## Security
//!
//! P2P traffic is intentionally scoped to the local network segment.  For
//! production deployments that cross network boundaries, wrap the TCP
//! connections in a VPN (Tailscale, WireGuard) or enable the TLS tunnel
//! (`src/tunnel/`).

use crate::{NodeAnnouncement, NodeToolSpec, ToolCallRequest, ToolCallResult};
use anyhow::{bail, Result};
use async_trait::async_trait;
use obc_tool_api::{Tool, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, RwLock};

/// Maximum permitted size of a single P2P TCP message frame (1 MiB).
const MAX_FRAME_SIZE: usize = 1024 * 1024;

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration for the peer-to-peer spine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// Unique identifier for this node in the P2P mesh.
    #[serde(default = "default_p2p_node_id")]
    pub node_id: String,
    /// The local address to bind the TCP server to.
    #[serde(default = "default_p2p_bind_host")]
    pub bind_host: String,
    /// TCP port on which this node accepts incoming tool-call connections.
    #[serde(default = "default_p2p_tcp_port")]
    pub tcp_port: u16,
    /// UDP port used for peer discovery broadcasts.
    #[serde(default = "default_p2p_discovery_port")]
    pub discovery_port: u16,
    /// Seconds after which a peer with no heartbeat is removed from the registry.
    #[serde(default = "default_p2p_peer_timeout_secs")]
    pub peer_timeout_secs: u64,
    /// Timeout in seconds for tool call responses from remote peers.
    #[serde(default = "default_p2p_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
    /// How often (in seconds) to broadcast a presence announcement.
    #[serde(default = "default_p2p_announce_interval_secs")]
    pub announce_interval_secs: u64,
}

fn default_p2p_node_id() -> String {
    format!("obc-node-{}", &uuid::Uuid::new_v4().to_string()[..8])
}

fn default_p2p_bind_host() -> String {
    "0.0.0.0".to_string()
}

fn default_p2p_tcp_port() -> u16 {
    44445
}

fn default_p2p_discovery_port() -> u16 {
    44444
}

fn default_p2p_peer_timeout_secs() -> u64 {
    60
}

fn default_p2p_tool_timeout_secs() -> u64 {
    30
}

fn default_p2p_announce_interval_secs() -> u64 {
    10
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            node_id: default_p2p_node_id(),
            bind_host: default_p2p_bind_host(),
            tcp_port: default_p2p_tcp_port(),
            discovery_port: default_p2p_discovery_port(),
            peer_timeout_secs: default_p2p_peer_timeout_secs(),
            tool_timeout_secs: default_p2p_tool_timeout_secs(),
            announce_interval_secs: default_p2p_announce_interval_secs(),
        }
    }
}

impl crate::SpineConfig {
    /// The P2P spine `[spine]` describes: each `p2p_*` key that is set, the
    /// default for each that is not, and the spine's own `tool_timeout_secs`.
    ///
    /// The `p2p_*` keys parsed from the day they were added and nothing read
    /// them, because nothing in the binary built a `P2pSpine`. This is where
    /// they start to mean something.
    pub fn p2p_config(&self) -> P2pConfig {
        let d = P2pConfig::default();
        P2pConfig {
            node_id: self.p2p_node_id.clone().unwrap_or(d.node_id),
            bind_host: self.p2p_bind_host.clone().unwrap_or(d.bind_host),
            tcp_port: self.p2p_tcp_port.unwrap_or(d.tcp_port),
            discovery_port: self.p2p_discovery_port.unwrap_or(d.discovery_port),
            peer_timeout_secs: self.p2p_peer_timeout_secs.unwrap_or(d.peer_timeout_secs),
            tool_timeout_secs: self.tool_timeout_secs,
            announce_interval_secs: self
                .p2p_announce_interval_secs
                .unwrap_or(d.announce_interval_secs),
        }
    }
}

// ── Wire Types ────────────────────────────────────────────────────────────────

/// UDP broadcast datagram used for peer discovery.
///
/// A node sends this periodically so that other nodes can learn its TCP
/// address and tool capabilities without a central directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pAnnounce {
    /// Node announcement payload (capabilities, tool specs, etc.).
    pub announcement: NodeAnnouncement,
    /// The IP address peers should connect to for tool calls.
    pub tcp_host: String,
    /// The TCP port peers should connect to for tool calls.
    pub tcp_port: u16,
    /// Ed25519 signature over the rest of this datagram, by the key in
    /// `announcement.metadata.p2p_pubkey`. Proves the announcer holds the key
    /// it asks peers to pin, and binds its address to it, so no other mesh
    /// member can redirect calls meant for this node to itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    /// Higher in every announcement a node sends (`epoch << 32 | count`, the
    /// epoch from [`obc_safety::node_identity::next_boot_epoch`]), and under
    /// `sig`. A peer takes an announcement only if it is newer than the last
    /// one it took for that id, so a signed datagram replayed from another
    /// address cannot move the node there.
    #[serde(default)]
    pub seq: u64,
}

impl P2pAnnounce {
    /// Stamp `identity`'s public key into the announcement and sign the whole:
    /// the datagram to send, as a JSON value, since that is what is signed.
    fn seal(mut self, identity: &obc_safety::node_identity::NodeIdentity) -> Value {
        if !self.announcement.metadata.is_object() {
            self.announcement.metadata = serde_json::json!({});
        }
        self.announcement.metadata["p2p_pubkey"] = Value::String(identity.public_hex());
        self.sig = None;
        let mut datagram = serde_json::to_value(&self).unwrap_or_default();
        let sig = identity.sign_announcement(&announce_body(&datagram));
        datagram["sig"] = Value::String(sig);
        datagram
    }

    /// The key this announcement asks to be pinned, if it carries one and its
    /// signature verifies under it over `body` (see [`announce_body`]).
    fn proven_key(&self, body: &[u8]) -> Option<ed25519_dalek::VerifyingKey> {
        let public = self.announcement.metadata.get("p2p_pubkey")?.as_str()?;
        let sig = self.sig.as_deref()?;
        if !obc_safety::node_identity::verify_announcement(public, body, sig) {
            return None;
        }
        obc_safety::node_identity::parse_public(public)
    }
}

/// The bytes an announcement's `sig` covers: the datagram as it arrived, less
/// `sig`, re-encoded. Signed over the JSON rather than [`P2pAnnounce`], so the
/// fields a newer peer adds are covered here too instead of being dropped by
/// the parse and breaking its signature.
fn announce_body(datagram: &Value) -> Vec<u8> {
    let body: serde_json::Map<String, Value> = datagram
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(field, _)| field.as_str() != "sig")
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    serde_json::to_vec(&body).unwrap_or_default()
}

/// The first announcement sequence of a start under `epoch`.
fn epoch_seq(epoch: u32) -> u64 {
    u64::from(epoch) << 32
}

/// The clock's seconds, as an announce epoch for a node given none.
fn unix_epoch() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
        .unwrap_or_default()
}

// ── Peer Registry ─────────────────────────────────────────────────────────────

/// A discovered peer on the local network.
#[derive(Debug, Clone)]
pub struct P2pPeer {
    /// The full announcement (capabilities, tool specs).
    pub announcement: NodeAnnouncement,
    /// TCP address to dial for tool calls.
    pub tcp_addr: SocketAddr,
    /// Timestamp of the last received announcement (for TTL expiry).
    pub last_seen: Instant,
    /// The `seq` of that announcement: an older or equal one is refused.
    pub announce_seq: u64,
}

/// A change in who is on the mesh, pushed as it happens to a caller that keeps
/// peer tools registered (see [`P2pSpine::with_peer_events`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PeerEvent {
    /// A peer was admitted, or re-announced a different tool list. Its tools
    /// replace whatever it offered before.
    Joined {
        node_id: String,
        tools: Vec<NodeToolSpec>,
    },
    /// A peer went silent past `peer_timeout_secs` and was pruned.
    Left { node_id: String },
    /// Every admitted peer and its tools, sent after events were dropped
    /// because the receiver lagged: anyone not in it has left.
    Snapshot {
        peers: HashMap<String, Vec<NodeToolSpec>>,
    },
}

/// Grace the caller gives a call past the server's own `tool_timeout_secs`, so
/// a call the server finished is not reported here as timed out (and retried).
const CALL_MARGIN_SECS: u64 = 5;

/// How long an incoming connection may take to send its request.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Incoming connections handled at once; past this, new ones are closed.
const MAX_INBOUND_CONNECTIONS: usize = 32;

type PeerRegistry = Arc<RwLock<HashMap<String, P2pPeer>>>;

/// Runs a tool a paired peer asked this node for. The agent implements it over
/// its own gated path (policy, Track 0, trust, approval, audit), so a call from
/// a peer is held to everything a local one is.
///
/// `peer` is the caller's node id, proven: the call was signed with the
/// Ed25519 key pinned for `peer` when it paired (`obc_safety::node_identity`),
/// so another mesh member holding the shared secret cannot call as it. What
/// the pin is worth is said there: trust on first use, unless the operator
/// pinned the key in `[security] peer_keys`.
#[async_trait]
pub trait PeerToolServer: Send + Sync {
    /// Run `tool` with `args` on behalf of `peer`.
    async fn call(&self, peer: &str, tool: &str, args: Value) -> Result<ToolResult>;
}

/// What this node answers for: the handler, and the names it announced.
struct Served {
    handler: Arc<dyn PeerToolServer>,
    tools: HashSet<String>,
}

// ── P2pSpine ──────────────────────────────────────────────────────────────────

/// A broker-free communication spine based on UDP discovery and direct TCP
/// tool calls.
///
/// `P2pSpine` mirrors the public API of `SpineClient` so that the rest of the
/// codebase can swap between MQTT and P2P spines via configuration.
pub struct P2pSpine {
    config: P2pConfig,
    peer_registry: PeerRegistry,
    /// Our own announcement — set once we call `start()`.
    local_announcement: RwLock<Option<NodeAnnouncement>>,
    /// Node pairing, and whether an unpaired peer is refused or merely noted.
    ///
    /// This transport needs it more than MQTT does: discovery is a UDP broadcast
    /// on the local network with no broker, no credentials and no handshake, so
    /// anyone who can send a datagram can offer the brain a set of tools.
    pairing: obc_safety::NodePairingManager,
    require_pairing: bool,
    /// Per-message authentication, both directions. Unlike MQTT — where the far
    /// end is firmware — both ends of a P2P call are this codebase, so a call
    /// can be signed here and verified there today.
    frame_auth: Arc<obc_safety::frame_auth::FrameAuth>,
    out_counters: Arc<obc_safety::frame_auth::OutboundCounters>,
    /// This node's own key: it signs our announcements, calls and answers.
    identity: obc_safety::node_identity::NodeIdentity,
    /// Peers' keys, pinned when they pair: what their frames verify against.
    peer_keys: Arc<obc_safety::node_identity::PeerKeys>,
    /// The `seq` of our last announcement; each one sent is one higher.
    announce_seq: std::sync::atomic::AtomicU64,
    /// Where admitted and pruned peers are reported, when someone listens.
    peer_events: Option<mpsc::Sender<PeerEvent>>,
    /// Incoming calls are answered only once [`P2pSpine::serve`] sets this;
    /// until then every one is refused.
    served: RwLock<Option<Served>>,
    /// Random per process, sent in every announcement: our own broadcast
    /// carries it, another board configured with our node id does not.
    instance: String,
    /// Sources already reported as using our node id, so it is said once each.
    duplicates_seen: std::sync::Mutex<HashSet<SocketAddr>>,
    /// Set when a peer event was dropped; a [`PeerEvent::Snapshot`] follows.
    resync: std::sync::atomic::AtomicBool,
}

impl P2pSpine {
    /// Create a new `P2pSpine` from configuration.
    pub fn new(config: P2pConfig) -> Self {
        Self {
            config,
            peer_registry: Arc::new(RwLock::new(HashMap::new())),
            local_announcement: RwLock::new(None),
            pairing: obc_safety::NodePairingManager::new(None),
            require_pairing: false,
            frame_auth: Arc::new(obc_safety::frame_auth::FrameAuth::new(None, false)),
            out_counters: Arc::new(obc_safety::frame_auth::OutboundCounters::new()),
            identity: obc_safety::node_identity::NodeIdentity::generate(),
            peer_keys: Arc::new(obc_safety::node_identity::PeerKeys::new()),
            announce_seq: std::sync::atomic::AtomicU64::new(epoch_seq(unix_epoch())),
            peer_events: None,
            served: RwLock::new(None),
            instance: uuid::Uuid::new_v4().to_string(),
            duplicates_seen: std::sync::Mutex::new(HashSet::new()),
            resync: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Report admitted and pruned peers on `tx` as discovery sees them, so the
    /// caller can add and remove their tools instead of polling the registry.
    pub fn with_peer_events(mut self, tx: mpsc::Sender<PeerEvent>) -> Self {
        self.peer_events = Some(tx);
        self
    }

    /// Authenticate P2P tool calls in both directions
    /// (`[security] require_frame_auth`).
    pub fn with_frame_auth(mut self, frame_auth: obc_safety::frame_auth::FrameAuth) -> Self {
        self.frame_auth = Arc::new(frame_auth);
        self
    }

    /// This node's identity key. Without it a fresh one is generated, which
    /// peers that pinned the last one refuse: give a node that restarts its
    /// persisted key (`NodeIdentity::load_or_create`).
    pub fn with_identity(mut self, identity: obc_safety::node_identity::NodeIdentity) -> Self {
        self.identity = identity;
        self
    }

    /// The peer keys to start from: `[security] peer_keys`, and the pins
    /// learned before a restart.
    pub fn with_peer_keys(mut self, peer_keys: obc_safety::node_identity::PeerKeys) -> Self {
        self.peer_keys = Arc::new(peer_keys);
        self
    }

    /// The epoch this start announces under, from
    /// [`obc_safety::node_identity::next_boot_epoch`]. Without it the clock is
    /// used, and a board whose clock resets is ignored by peers that remember
    /// a higher sequence from it until they forget it (`peer_timeout_secs`).
    pub fn with_announce_epoch(self, epoch: u32) -> Self {
        self.announce_seq
            .store(epoch_seq(epoch), std::sync::atomic::Ordering::Relaxed);
        self
    }

    /// This node's public key, hex: what peers pin, and what an operator puts
    /// in their `[security] peer_keys`.
    pub fn public_key_hex(&self) -> String {
        self.identity.public_hex()
    }

    /// Enforce `[security] pairing_secret` / `require_pairing` on discovered peers.
    pub fn with_pairing(
        mut self,
        pairing: obc_safety::NodePairingManager,
        require_pairing: bool,
    ) -> Self {
        self.pairing = pairing;
        self.require_pairing = require_pairing;
        self
    }

    /// Start the P2P spine: bind the TCP server, bind the UDP discovery socket,
    /// and spawn background tasks.
    ///
    /// Returns `Arc<P2pSpine>` so it can be shared across tasks.
    pub async fn start(self) -> Result<Arc<Self>> {
        let arc = Arc::new(self);

        // ── TCP server ────────────────────────────────────────────────────────
        let tcp_addr = format!("{}:{}", arc.config.bind_host, arc.config.tcp_port);
        let listener = TcpListener::bind(&tcp_addr).await?;
        tracing::info!(addr = %tcp_addr, "P2P spine TCP server listening");

        let server = Arc::clone(&arc);
        // Requests are read before anything authenticates them, so the number
        // in flight is capped and each must arrive in REQUEST_READ_TIMEOUT.
        let slots = Arc::new(tokio::sync::Semaphore::new(MAX_INBOUND_CONNECTIONS));
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                            tracing::warn!(
                                peer = %peer,
                                cap = MAX_INBOUND_CONNECTIONS,
                                "P2P connection refused: too many in flight"
                            );
                            continue;
                        };
                        tracing::debug!(peer = %peer, "Incoming P2P TCP connection");
                        let spine = Arc::clone(&server);
                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_tcp_connection(stream, spine, REQUEST_READ_TIMEOUT).await
                            {
                                tracing::warn!(error = %e, "P2P TCP handler error");
                            }
                            drop(slot);
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "P2P TCP accept error");
                    }
                }
            }
        });

        // ── UDP discovery receiver ────────────────────────────────────────────
        let discovery_bind = format!("0.0.0.0:{}", arc.config.discovery_port);
        let udp_rx = UdpSocket::bind(&discovery_bind).await?;
        udp_rx.set_broadcast(true)?;
        tracing::info!(
            port = arc.config.discovery_port,
            "P2P discovery UDP socket bound"
        );

        let discovery = Arc::clone(&arc);
        tokio::spawn(async move {
            // Bounded waits, so a quiet network still has its stale peers pruned.
            let sweep = Duration::from_secs(discovery.config.announce_interval_secs.max(1));
            let mut buf = [0u8; 4096];
            loop {
                match tokio::time::timeout(sweep, udp_rx.recv_from(&mut buf)).await {
                    Ok(Ok((len, src))) => {
                        if let Ok(datagram) = serde_json::from_slice::<Value>(&buf[..len]) {
                            discovery.on_announce(datagram, src).await;
                        }
                    }
                    Ok(Err(e)) => tracing::warn!(error = %e, "UDP receive error"),
                    Err(_) => {}
                }
                discovery.evict_stale().await;
            }
        });

        // ── Periodic UDP announcement sender ──────────────────────────────────
        let arc2 = Arc::clone(&arc);
        tokio::spawn(async move {
            let interval = Duration::from_secs(arc2.config.announce_interval_secs);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(e) = arc2.broadcast_presence().await {
                    tracing::warn!(error = %e, "Failed to broadcast P2P presence");
                }
            }
        });

        Ok(arc)
    }

    /// Admit (or refuse) one discovery datagram, and report the peer when it is
    /// new or its tool list changed.
    async fn on_announce(&self, datagram: Value, src: SocketAddr) {
        let body = announce_body(&datagram);
        let Ok(announce) = serde_json::from_value::<P2pAnnounce>(datagram) else {
            return;
        };
        let node_id = announce.announcement.node_id.clone();
        // Broadcast comes back to the sender; this node is not its own peer.
        // Our id with another instance is a second board configured with it.
        if node_id == self.config.node_id {
            let instance = announce.announcement.metadata.get("p2p_instance");
            if instance.and_then(Value::as_str) != Some(self.instance.as_str()) {
                let mut seen = self
                    .duplicates_seen
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if seen.insert(src) {
                    tracing::error!(
                        node_id = %node_id,
                        src = %src,
                        "Another node on the mesh announces this node's p2p_node_id: \
                         give each board its own, or peers will see only one of them"
                    );
                }
            }
            return;
        }
        if let super::Admission::Refuse { reason } = super::admit_announcement(
            &self.pairing,
            self.require_pairing,
            &node_id,
            &announce.announcement.metadata,
        ) {
            tracing::warn!(
                node_id = %node_id,
                src = %src,
                tool_count = announce.announcement.tools.len(),
                %reason,
                "Refused a P2P peer: its tools are NOT registered \
                 ([security] require_pairing is on)"
            );
            return;
        }
        let identified = self
            .admit_identity(&node_id, announce.proven_key(&body))
            .await;
        if let Err(reason) = &identified {
            if self.frame_auth.is_enforcing() {
                tracing::warn!(
                    node_id = %node_id,
                    src = %src,
                    %reason,
                    "Refused a P2P peer: its tools are NOT registered \
                     ([security] require_frame_auth is on)"
                );
                return;
            }
            tracing::info!(node_id = %node_id, %reason, "Would refuse a P2P peer's identity");
        }

        // If the sender broadcast "0.0.0.0" (bind-all), use the actual UDP
        // source IP so we can dial back to the peer.
        let tcp_host = if announce.tcp_host == "0.0.0.0" {
            src.ip().to_string()
        } else {
            announce.tcp_host.clone()
        };
        let tcp_addr = match format!("{}:{}", tcp_host, announce.tcp_port).parse::<SocketAddr>() {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(src = %src, error = %e, "Invalid TCP address in P2P announcement");
                return;
            }
        };
        tracing::debug!(node_id = %node_id, tcp_addr = %tcp_addr, "Discovered P2P peer via UDP");
        let tools = announce.announcement.tools.clone();
        let mut registry = self.peer_registry.write().await;
        // Only a signature makes `seq` the node's own; an announcement with
        // none is taken on trust anyway, which only frame auth off allows.
        let stale = registry
            .get(&node_id)
            .is_some_and(|peer| announce.seq <= peer.announce_seq);
        if identified.is_ok() && stale {
            tracing::debug!(
                node_id = %node_id,
                src = %src,
                seq = announce.seq,
                "Ignored a P2P announcement no newer than the last one taken: a replay"
            );
            return;
        }
        let previous = registry.insert(
            node_id.clone(),
            P2pPeer {
                announcement: announce.announcement,
                tcp_addr,
                last_seen: Instant::now(),
                announce_seq: announce.seq,
            },
        );
        drop(registry);
        if previous.is_none_or(|p| p.announcement.tools != tools) {
            self.emit(PeerEvent::Joined { node_id, tools }).await;
        }
    }

    /// Pin the key a paired announcement proves, or say why it cannot be.
    /// Membership is what makes a first key worth pinning, so a node that is
    /// admitted only because `require_pairing` is off has its key checked
    /// against a pin but never pinned.
    async fn admit_identity(
        &self,
        node_id: &str,
        proven: Option<ed25519_dalek::VerifyingKey>,
    ) -> Result<(), String> {
        let Some(key) = proven else {
            return Err("its announcement carries no key it signed with".to_string());
        };
        let mismatch = || {
            format!(
                "it announces a key other than the one pinned for {node_id:?}: another node \
                 claiming its id, or a re-keyed node whose old pin an operator has to remove"
            )
        };
        match self.peer_keys.key_for(node_id) {
            Some(pinned) if pinned == key => return Ok(()),
            Some(_) => return Err(mismatch()),
            None => {}
        }
        if !matches!(
            self.pairing.status(node_id),
            obc_safety::PairingStatus::Paired
        ) {
            return Err(format!(
                "{node_id:?} is not paired, so its key is not pinned"
            ));
        }
        // Pinning writes the pin file, which is no work for an async worker.
        let keys = Arc::clone(&self.peer_keys);
        let id = node_id.to_string();
        let verdict = tokio::task::spawn_blocking(move || keys.pin(&id, key))
            .await
            .map_err(|e| format!("pinning its key failed: {e}"))?;
        match verdict {
            obc_safety::node_identity::PinVerdict::Known => Ok(()),
            obc_safety::node_identity::PinVerdict::Pinned => {
                tracing::info!(
                    node_id = %node_id,
                    key = %hex::encode(key.to_bytes()),
                    "Pinned a P2P peer's identity key"
                );
                Ok(())
            }
            obc_safety::node_identity::PinVerdict::Mismatch => Err(mismatch()),
        }
    }

    /// Drop peers silent past `peer_timeout_secs`, and report each one.
    async fn evict_stale(&self) {
        let timeout = self.config.peer_timeout_secs;
        let mut gone = Vec::new();
        self.peer_registry.write().await.retain(|id, peer| {
            let alive = peer.last_seen.elapsed().as_secs() < timeout;
            if !alive {
                tracing::info!(node_id = %id, "Evicting stale P2P peer");
                gone.push(id.clone());
            }
            alive
        });
        for node_id in gone {
            self.emit(PeerEvent::Left { node_id }).await;
        }
        if self.resync.load(std::sync::atomic::Ordering::Acquire) {
            self.flush_resync().await;
        }
    }

    /// Send the pending [`PeerEvent::Snapshot`], if the receiver has room.
    async fn flush_resync(&self) {
        use std::sync::atomic::Ordering;
        let Some(tx) = &self.peer_events else { return };
        let peers = self
            .peer_registry
            .read()
            .await
            .iter()
            .map(|(id, p)| (id.clone(), p.announcement.tools.clone()))
            .collect();
        if tx.try_send(PeerEvent::Snapshot { peers }).is_ok() {
            self.resync.store(false, Ordering::Release);
        }
    }

    /// Report `event` without waiting on the receiver: discovery (eviction
    /// included) must not stall behind it. A dropped event is made good by a
    /// [`PeerEvent::Snapshot`] once the receiver has room.
    async fn emit(&self, event: PeerEvent) {
        use std::sync::atomic::Ordering;
        let Some(tx) = &self.peer_events else { return };
        if self.resync.load(Ordering::Acquire) {
            // The snapshot holds this event's effect, sent now or later.
            self.flush_resync().await;
            return;
        }
        // A closed receiver means nobody registers peer tools any more.
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
            if !self.resync.swap(true, Ordering::AcqRel) {
                tracing::warn!("P2P peer events are backing up; a snapshot will follow");
            }
        }
    }

    /// Publish our node announcement to the local network via UDP broadcast.
    pub async fn announce(&self, announcement: &NodeAnnouncement) -> Result<()> {
        *self.local_announcement.write().await = Some(announcement.clone());
        self.broadcast_presence().await
    }

    /// Broadcast the stored local announcement via UDP.
    async fn broadcast_presence(&self) -> Result<()> {
        let announcement = self.local_announcement.read().await.clone();
        let mut announcement = match announcement {
            Some(a) => a,
            None => return Ok(()), // not yet announced
        };
        // Peers that require pairing admit only a fresh token; one stamped at
        // `announce` time would expire while we keep announcing.
        if !announcement.metadata.is_object() {
            announcement.metadata = serde_json::json!({});
        }
        if let Some(token) = self.pairing.token_for(&announcement.node_id) {
            announcement.metadata["pairing_token"] = serde_json::to_value(token)?;
        }
        announcement.metadata["p2p_instance"] = Value::String(self.instance.clone());

        let payload = P2pAnnounce {
            announcement,
            tcp_host: self.config.bind_host.clone(),
            tcp_port: self.config.tcp_port,
            sig: None,
            seq: self
                .announce_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1,
        }
        .seal(&self.identity);

        let json = serde_json::to_vec(&payload)?;

        // We need a fresh socket per broadcast to avoid bind conflicts with the
        // receiver socket spawned in `start()`.
        let udp_tx = UdpSocket::bind("0.0.0.0:0").await?;
        udp_tx.set_broadcast(true)?;
        let dest = SocketAddr::from((Ipv4Addr::BROADCAST, self.config.discovery_port));
        udp_tx.send_to(&json, dest).await?;

        tracing::debug!(
            node_id = %self.config.node_id,
            "Broadcast P2P presence announcement"
        );
        Ok(())
    }

    /// Invoke a tool on a remote peer node via a direct TCP connection.
    pub async fn invoke_tool(
        &self,
        node_id: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolCallResult> {
        let tcp_addr = {
            let registry = self.peer_registry.read().await;
            let peer = registry
                .get(node_id)
                .ok_or_else(|| anyhow::anyhow!("Unknown P2P peer: {}", node_id))?;
            peer.tcp_addr
        };

        let call_id = uuid::Uuid::new_v4().to_string();
        let mut request = ToolCallRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.to_string(),
            args,
            from: Some(self.config.node_id.clone()),
            ctr: None,
            mac: None,
            sig: None,
        };
        // The tag under the shared secret proves membership; the signature, by
        // our own key and bound to `node_id`, proves the call is ours.
        request.sign_p2p(
            &self.frame_auth,
            &self.identity,
            &self.out_counters,
            &self.config.node_id,
            node_id,
        );

        let payload = serde_json::to_vec(&request)?;

        // Connect and send
        let mut stream =
            tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(tcp_addr))
                .await
                .map_err(|_| anyhow::anyhow!("TCP connect timed out for node={}", node_id))?
                .map_err(|e| anyhow::anyhow!("TCP connect failed for node={}: {}", node_id, e))?;

        write_framed(&mut stream, &payload).await?;

        // The answer comes back on this stream. (It used to be awaited from the
        // listener instead, where nothing ever sent it, so no call returned.)
        // The peer gives the tool `tool_timeout_secs` and then answers either
        // way, so wait a margin past that before calling the outcome unknown.
        let wait = self.config.tool_timeout_secs + CALL_MARGIN_SECS;
        let reply =
            match tokio::time::timeout(Duration::from_secs(wait), read_framed(&mut stream)).await {
                Ok(reply) => reply?,
                Err(_) => bail!(
                    "P2P tool call got no answer in {wait}s (node={node_id}, tool={tool_name}): \
                 it may still have run there, so check before retrying it"
                ),
            };
        let result: ToolCallResult = serde_json::from_slice(&reply)?;
        if result.call_id != call_id {
            bail!("P2P peer {node_id} answered a different call");
        }
        // The peer tags as a member and signs as itself, under the key it
        // paired with; an answer that does not verify both ways is not its.
        if let obc_safety::frame_auth::FrameVerdict::Reject { reason } =
            self.frame_auth.verify_p2p_inbound(
                self.peer_keys.key_for(node_id).as_ref(),
                &obc_safety::frame_auth::P2pFrame {
                    kind: obc_safety::node_identity::FrameKind::Result,
                    from: node_id,
                    to: &self.config.node_id,
                    ctr: result.ctr,
                    mac: result.mac.as_deref(),
                    sig: result.sig.as_deref(),
                    payload: &result.signed_bytes(),
                },
            )
        {
            bail!("Refused a P2P result from {node_id} ([security] require_frame_auth): {reason}");
        }
        Ok(result)
    }

    /// Answer paired peers' calls to `tools` through `handler`, and announce
    /// them. Refused unless pairing and frame auth are both enforced: pairing
    /// admits only nodes holding the mesh secret and pins each one's key, and
    /// frame auth checks every call against the caller's pinned key (see
    /// [`PeerToolServer`]).
    pub async fn serve(
        &self,
        handler: Arc<dyn PeerToolServer>,
        tools: Vec<NodeToolSpec>,
    ) -> Result<()> {
        let announcement = self.start_serving(handler, tools).await?;
        self.announce(&announcement).await
    }

    /// [`P2pSpine::serve`] short of broadcasting: the announcement to send.
    async fn start_serving(
        &self,
        handler: Arc<dyn PeerToolServer>,
        tools: Vec<NodeToolSpec>,
    ) -> Result<NodeAnnouncement> {
        if !(self.require_pairing && self.pairing.is_enabled() && self.frame_auth.is_enforcing()) {
            bail!(
                "serving P2P tool calls needs [security] require_pairing, a pairing_secret \
                 and require_frame_auth"
            );
        }
        let Some(mut announcement) = self.local_announcement.read().await.clone() else {
            bail!("announce on the mesh before serving tools on it");
        };
        *self.served.write().await = Some(Served {
            handler,
            tools: tools.iter().map(|t| t.name.clone()).collect(),
        });
        announcement.tools = tools;
        Ok(announcement)
    }

    /// The answer to one incoming call, or `None` to drop it unanswered (its
    /// frame did not verify, so there is nobody to answer).
    async fn answer(&self, request: ToolCallRequest) -> Option<ToolCallResult> {
        // `from` says which pinned key to check, the tag says the caller is a
        // member, and a good signature says it is `from`. No `from`, or no key
        // pinned for it, means nothing to verify against, which when enforcing
        // is a refusal rather than a shrug.
        let caller = request.from.clone().unwrap_or_default();
        let key = self.peer_keys.key_for(&caller);
        let payload = request.signed_bytes();
        // A member whose announcement has not reached us yet: say so, signed,
        // rather than leave it waiting out its timeout. Nothing runs, and the
        // tag check moves no counter.
        if key.is_none()
            && self.frame_auth.is_enforcing()
            && self
                .frame_auth
                .tag_verifies(&caller, request.ctr, request.mac.as_deref(), &payload)
        {
            tracing::info!(
                call_id = %request.call_id,
                from = %caller,
                "Refused a P2P tool call from a member whose key is not pinned yet"
            );
            let mut refusal = ToolCallResult {
                call_id: request.call_id,
                ok: false,
                output: None,
                error: Some(format!(
                    "no key is pinned for {caller:?} on this node yet; announce first"
                )),
                ctr: None,
                mac: None,
                sig: None,
            };
            refusal.sign_p2p(
                &self.frame_auth,
                &self.identity,
                &self.out_counters,
                &self.config.node_id,
                &caller,
            );
            return Some(refusal);
        }
        if let obc_safety::frame_auth::FrameVerdict::Reject { reason } =
            self.frame_auth.verify_p2p_inbound(
                key.as_ref(),
                &obc_safety::frame_auth::P2pFrame {
                    kind: obc_safety::node_identity::FrameKind::Request,
                    from: &caller,
                    to: &self.config.node_id,
                    ctr: request.ctr,
                    mac: request.mac.as_deref(),
                    sig: request.sig.as_deref(),
                    payload: &payload,
                },
            )
        {
            tracing::warn!(
                call_id = %request.call_id,
                tool = %request.tool_name,
                from = %if caller.is_empty() { "<unstated>" } else { &caller },
                %reason,
                "Refused a P2P tool call ([security] require_frame_auth is on)"
            );
            return None;
        }

        let mut result = match self.run_for_peer(&caller, &request).await {
            Ok(r) => ToolCallResult {
                call_id: request.call_id,
                ok: r.success,
                output: Some(r.output),
                error: r.error,
                ctr: None,
                mac: None,
                sig: None,
            },
            Err(why) => {
                tracing::warn!(
                    call_id = %request.call_id,
                    tool = %request.tool_name,
                    from = %caller,
                    %why,
                    "Refused a P2P tool call"
                );
                ToolCallResult {
                    call_id: request.call_id,
                    ok: false,
                    output: None,
                    error: Some(why.to_string()),
                    ctr: None,
                    mac: None,
                    sig: None,
                }
            }
        };
        // Signed as this node, refusals included: the caller drops an answer it
        // cannot verify, and a refusal it cannot see is a timeout.
        result.sign_p2p(
            &self.frame_auth,
            &self.identity,
            &self.out_counters,
            &self.config.node_id,
            &caller,
        );
        Some(result)
    }

    /// Run a verified request, if this node serves it to this caller.
    async fn run_for_peer(&self, caller: &str, request: &ToolCallRequest) -> Result<ToolResult> {
        let handler = {
            let served = self.served.read().await;
            let Some(served) = served.as_ref() else {
                bail!("this node serves no tools over P2P");
            };
            // `serve` checked this; without it, anyone on the LAN could call.
            if !self.frame_auth.is_enforcing() {
                bail!("this node serves P2P calls only with frame auth enforced");
            }
            let paired = matches!(
                self.pairing.status(caller),
                obc_safety::PairingStatus::Paired
            );
            if !paired || !self.peer_registry.read().await.contains_key(caller) {
                bail!("{caller:?} is not a paired peer of this node");
            }
            if !served.tools.contains(&request.tool_name) {
                bail!("tool {:?} is not served over P2P", request.tool_name);
            }
            Arc::clone(&served.handler)
        };
        tracing::info!(
            call_id = %request.call_id,
            tool = %request.tool_name,
            from = %caller,
            "Serving a P2P tool call"
        );
        // Run on its own task: a slow tool is never cancelled part-way through
        // an action (its audit record already written). Past the timeout the
        // caller is told the outcome is unknown, and the run finishes alone.
        let timeout = Duration::from_secs(self.config.tool_timeout_secs);
        let (peer, tool, args) = (
            caller.to_string(),
            request.tool_name.clone(),
            request.args.clone(),
        );
        let mut run = tokio::spawn(async move {
            let result = handler.call(&peer, &tool, args).await;
            (peer, tool, result)
        });
        match tokio::time::timeout(timeout, &mut run).await {
            Ok(Ok((_, _, result))) => result,
            Ok(Err(e)) => bail!("tool {:?} failed to run: {e}", request.tool_name),
            Err(_) => {
                tokio::spawn(async move {
                    if let Ok((peer, tool, result)) = run.await {
                        let ok = result.as_ref().is_ok_and(|r| r.success);
                        tracing::warn!(%peer, %tool, ok, "P2P tool call finished after its caller was answered");
                    }
                });
                bail!(
                    "tool {:?} still running after {}s; it was not stopped, and its \
                     outcome will not be reported, so check before retrying it",
                    request.tool_name,
                    timeout.as_secs()
                )
            }
        }
    }

    /// Return a snapshot of all currently discovered peers and their tool specs.
    pub async fn known_nodes(&self) -> HashMap<String, NodeAnnouncement> {
        self.peer_registry
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.announcement.clone()))
            .collect()
    }

    /// Build a list of `Box<dyn Tool>` wrapping all tools on discovered peers.
    pub async fn build_p2p_tools(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        let registry = self.peer_registry.read().await;
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        for (node_id, peer) in registry.iter() {
            for spec in &peer.announcement.tools {
                tools.push(Box::new(P2pNodeTool {
                    node_id: node_id.clone(),
                    spec: spec.clone(),
                    spine: Arc::clone(self),
                }));
            }
        }
        tools
    }

    /// The tools `node_id` offers, as tools this agent can call (one per spec,
    /// under the spec's own name). For a [`PeerEvent::Joined`].
    pub fn peer_tools(
        self: &Arc<Self>,
        node_id: &str,
        specs: &[NodeToolSpec],
    ) -> Vec<Box<dyn Tool>> {
        specs
            .iter()
            .map(|spec| {
                Box::new(P2pNodeTool {
                    node_id: node_id.to_string(),
                    spec: spec.clone(),
                    spine: Arc::clone(self),
                }) as Box<dyn Tool>
            })
            .collect()
    }
}

// ── TCP Framing Helpers ───────────────────────────────────────────────────────

/// Write a length-prefixed frame to a TCP stream.
async fn write_framed(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    let len = payload.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    Ok(())
}

/// Read a length-prefixed frame from a TCP stream.
async fn read_framed(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        bail!("P2P frame too large: {} bytes", len);
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

// ── TCP Connection Handler ────────────────────────────────────────────────────

/// Handle a single incoming TCP connection: read one `ToolCallRequest` and
/// write its answer back on the same connection.
async fn handle_tcp_connection(
    mut stream: TcpStream,
    spine: Arc<P2pSpine>,
    read_timeout: Duration,
) -> Result<()> {
    let payload = tokio::time::timeout(read_timeout, read_framed(&mut stream))
        .await
        .map_err(|_| anyhow::anyhow!("P2P request not received in {read_timeout:?}"))??;
    let Ok(request) = serde_json::from_slice::<ToolCallRequest>(&payload) else {
        tracing::warn!("Received unrecognised P2P TCP message; ignoring");
        return Ok(());
    };
    tracing::debug!(
        call_id = %request.call_id,
        tool = %request.tool_name,
        "Received P2P tool call request"
    );
    if let Some(result) = spine.answer(request).await {
        write_framed(&mut stream, &serde_json::to_vec(&result)?).await?;
    }
    Ok(())
}

// ── P2P Node Tool ─────────────────────────────────────────────────────────────

/// A `Tool` implementation that delegates execution to a remote P2P peer.
struct P2pNodeTool {
    node_id: String,
    spec: crate::NodeToolSpec,
    spine: Arc<P2pSpine>,
}

#[async_trait]
impl Tool for P2pNodeTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    /// Same reasoning as `MqttNodeTool::risk_class`, and more so.
    ///
    /// A P2P peer is discovered over a UDP broadcast with no broker and no
    /// handshake — the set of tools this agent can be handed is decided by
    /// whoever else is on the network. Taking the non-physical default here
    /// meant a peer-announced `gpio_write` bypassed the Track 0 gate and the
    /// audit chain entirely.
    fn risk_class(&self) -> obc_tool_api::RiskClass {
        obc_tool_api::RiskClass::physical(true, obc_tool_api::BlastRadius::Low)
    }

    /// Same reasoning as `MqttNodeTool::output_trust`, and again more so: there
    /// is no broker and no pairing step in front of a UDP-discovered peer, so
    /// this is the least measured content in the system. It was arriving marked
    /// `Trusted`.
    fn output_trust(&self) -> obc_tool_api::OutputTrust {
        obc_tool_api::OutputTrust::External
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
                result
                    .error
                    .unwrap_or_else(|| "Unknown P2P error".to_string()),
            ))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_p2p_config_has_sensible_values() {
        let config = P2pConfig {
            node_id: "test-node".to_string(),
            ..P2pConfig::default()
        };
        assert_eq!(config.tcp_port, 44445);
        assert_eq!(config.discovery_port, 44444);
        assert_eq!(config.peer_timeout_secs, 60);
        assert_eq!(config.tool_timeout_secs, 30);
        assert_eq!(config.announce_interval_secs, 10);
    }

    #[test]
    fn p2p_announce_serializes_correctly() {
        use crate::{NodeAnnouncement, NodeToolSpec};

        let announce = P2pAnnounce {
            announcement: NodeAnnouncement {
                node_id: "nanopi-kitchen".to_string(),
                board: "nanopi-neo3".to_string(),
                firmware_version: "0.1.0".to_string(),
                tools: vec![NodeToolSpec {
                    name: "gpio_read".to_string(),
                    description: "Read a GPIO pin.".to_string(),
                    parameters: serde_json::json!({}),
                }],
                metadata: serde_json::json!({}),
            },
            tcp_host: "192.168.1.10".to_string(),
            tcp_port: 44445,
            sig: None,
            seq: 7,
        };

        let json = serde_json::to_string(&announce).unwrap();
        let back: P2pAnnounce = serde_json::from_str(&json).unwrap();
        assert_eq!(back.announcement.node_id, "nanopi-kitchen");
        assert_eq!(back.tcp_port, 44445);
        assert_eq!(back.seq, 7);
    }

    #[test]
    fn p2p_config_serializes_and_deserializes() {
        let config = P2pConfig {
            node_id: "edge-node-01".to_string(),
            bind_host: "0.0.0.0".to_string(),
            tcp_port: 44445,
            discovery_port: 44444,
            peer_timeout_secs: 120,
            tool_timeout_secs: 15,
            announce_interval_secs: 5,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: P2pConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_id, "edge-node-01");
        assert_eq!(back.peer_timeout_secs, 120);
    }

    #[test]
    fn p2p_spine_can_be_constructed() {
        let config = P2pConfig {
            node_id: "test-spine".to_string(),
            ..P2pConfig::default()
        };
        let spine = P2pSpine::new(config);
        // Just verify construction succeeds — network operations are tested
        // via integration tests that require an actual network interface.
        assert_eq!(spine.config.node_id, "test-spine");
    }

    #[test]
    fn spine_p2p_keys_reach_the_p2p_config() {
        let set = crate::SpineConfig {
            p2p_node_id: Some("nanopi-kitchen".to_string()),
            p2p_bind_host: Some("192.168.1.20".to_string()),
            p2p_tcp_port: Some(45001),
            p2p_discovery_port: Some(45000),
            p2p_peer_timeout_secs: Some(90),
            p2p_announce_interval_secs: Some(5),
            tool_timeout_secs: 12,
            ..crate::SpineConfig::default()
        }
        .p2p_config();
        assert_eq!(set.node_id, "nanopi-kitchen");
        assert_eq!(set.bind_host, "192.168.1.20");
        assert_eq!((set.tcp_port, set.discovery_port), (45001, 45000));
        assert_eq!(set.peer_timeout_secs, 90);
        assert_eq!(set.announce_interval_secs, 5);
        assert_eq!(set.tool_timeout_secs, 12);

        // Unset keys fall back to the P2P defaults, not to zero.
        let unset = crate::SpineConfig::default().p2p_config();
        assert_eq!((unset.tcp_port, unset.discovery_port), (44445, 44444));
        assert!(unset.node_id.starts_with("obc-node-"), "{}", unset.node_id);
    }

    // ── Discovery events and serving ──────────────────────────────────────────

    const SECRET: &str = "5f3c9a1e7b2d4c6f8a0e1b3d5c7f9a2e4b6d8f0a1c3e5b7d9f2a4c6e8b0d1f3a";

    fn spec(name: &str) -> NodeToolSpec {
        NodeToolSpec {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    fn announcement(node_id: &str, tools: &[&str]) -> NodeAnnouncement {
        NodeAnnouncement {
            node_id: node_id.to_string(),
            board: "nanopi-neo3".to_string(),
            firmware_version: "0.1.0".to_string(),
            tools: tools.iter().map(|t| spec(t)).collect(),
            metadata: serde_json::json!({}),
        }
    }

    /// Each test node's identity key, the same every time it is asked for.
    fn identity_of(node_id: &str) -> obc_safety::node_identity::NodeIdentity {
        obc_safety::node_identity::NodeIdentity::from_hex(&format!("{:0>64}", hex::encode(node_id)))
            .unwrap()
    }

    /// The datagram `node_id` sends, pairing token and key included, as
    /// `broadcast_presence` stamps it.
    fn datagram(node_id: &str, tools: &[&str], tcp_port: u16) -> Value {
        datagram_signed_by(node_id, tools, tcp_port, &identity_of(node_id))
    }

    /// As [`datagram`], under someone's key: a mesh member holds the pairing
    /// secret, so it can make every part of this except the key.
    fn datagram_signed_by(
        node_id: &str,
        tools: &[&str],
        tcp_port: u16,
        identity: &obc_safety::node_identity::NodeIdentity,
    ) -> Value {
        unsealed(node_id, tools, tcp_port).seal(identity)
    }

    /// As [`datagram`] before it is signed, each one a step later than the last.
    fn unsealed(node_id: &str, tools: &[&str], tcp_port: u16) -> P2pAnnounce {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut announcement = announcement(node_id, tools);
        let token = obc_safety::NodePairingManager::new(Some(SECRET.to_string()))
            .token_for(node_id)
            .unwrap();
        announcement.metadata["pairing_token"] = serde_json::to_value(token).unwrap();
        P2pAnnounce {
            announcement,
            tcp_host: "127.0.0.1".to_string(),
            tcp_port,
            sig: None,
            seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    fn src() -> SocketAddr {
        "127.0.0.1:9".parse().unwrap()
    }

    fn free_ports() -> (u16, u16) {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        (
            tcp.local_addr().unwrap().port(),
            udp.local_addr().unwrap().port(),
        )
    }

    /// A node that requires pairing and frame auth, as `[edge] p2p_enabled`
    /// now insists.
    fn secured(node_id: &str) -> P2pSpine {
        let (tcp_port, discovery_port) = free_ports();
        P2pSpine::new(P2pConfig {
            node_id: node_id.to_string(),
            bind_host: "127.0.0.1".to_string(),
            tcp_port,
            discovery_port,
            tool_timeout_secs: 5,
            ..P2pConfig::default()
        })
        .with_pairing(
            obc_safety::NodePairingManager::new(Some(SECRET.to_string())),
            true,
        )
        .with_frame_auth(obc_safety::frame_auth::FrameAuth::new(
            Some(SECRET.to_string()),
            true,
        ))
        .with_identity(identity_of(node_id))
    }

    /// Answers with who asked for what, and remembers each call.
    #[derive(Default)]
    struct Echo {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl PeerToolServer for Echo {
        async fn call(&self, peer: &str, tool: &str, _args: Value) -> Result<ToolResult> {
            self.calls
                .lock()
                .unwrap()
                .push((peer.to_string(), tool.to_string()));
            Ok(ToolResult::ok(format!("{tool} for {peer}")))
        }
    }

    /// A call from `from`, signed with `from`'s key.
    fn signed_call(
        from: &str,
        tool: &str,
        counters: &obc_safety::frame_auth::OutboundCounters,
    ) -> ToolCallRequest {
        let mut request = ToolCallRequest {
            call_id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.to_string(),
            args: serde_json::json!({ "pin": 3 }),
            from: Some(from.to_string()),
            ctr: None,
            mac: None,
            sig: None,
        };
        let auth = obc_safety::frame_auth::FrameAuth::new(Some(SECRET.to_string()), true);
        request.sign_p2p(&auth, &identity_of(from), counters, from, "brain");
        request
    }

    #[tokio::test]
    async fn discovery_reports_joins_changes_and_departures() {
        let (tx, mut rx) = mpsc::channel(8);
        let spine = Arc::new(secured("brain").with_peer_events(tx));

        spine
            .on_announce(datagram("a", &["gpio_read"], 44445), src())
            .await;
        assert_eq!(
            rx.try_recv().unwrap(),
            PeerEvent::Joined {
                node_id: "a".to_string(),
                tools: vec![spec("gpio_read")],
            }
        );
        // Every interval repeats the announcement; only a change is news.
        spine
            .on_announce(datagram("a", &["gpio_read"], 44445), src())
            .await;
        assert!(rx.try_recv().is_err());
        spine
            .on_announce(datagram("a", &["gpio_read", "dht22"], 44445), src())
            .await;
        assert!(matches!(
            rx.try_recv().unwrap(),
            PeerEvent::Joined { tools, .. } if tools.len() == 2
        ));

        // Its own broadcast, and a peer with no pairing token, are not peers.
        spine
            .on_announce(datagram("brain", &["shell"], 44445), src())
            .await;
        let rogue = P2pAnnounce {
            announcement: announcement("rogue", &["shell"]),
            tcp_host: "127.0.0.1".to_string(),
            tcp_port: 44445,
            sig: None,
            seq: 1,
        };
        spine
            .on_announce(serde_json::to_value(rogue).unwrap(), src())
            .await;
        assert!(rx.try_recv().is_err());
        assert_eq!(spine.known_nodes().await.keys().collect::<Vec<_>>(), ["a"]);

        // Silent past the timeout: pruned, and reported so its tools go too.
        spine
            .peer_registry
            .write()
            .await
            .get_mut("a")
            .unwrap()
            .last_seen = Instant::now() - Duration::from_secs(61);
        spine.evict_stale().await;
        assert_eq!(
            rx.try_recv().unwrap(),
            PeerEvent::Left {
                node_id: "a".to_string()
            }
        );
        assert!(spine.known_nodes().await.is_empty());
    }

    #[tokio::test]
    async fn serving_needs_pairing_frame_auth_and_an_announcement() {
        let echo: Arc<dyn PeerToolServer> = Arc::new(Echo::default());
        let tools = || vec![spec("gpio_read")];

        let open = P2pSpine::new(P2pConfig::default());
        *open.local_announcement.write().await = Some(announcement("open", &[]));
        assert!(open
            .start_serving(Arc::clone(&echo), tools())
            .await
            .is_err());

        // Pairing without enforced frame auth: `from` would be taken on trust.
        let unsigned = secured("brain").with_frame_auth(obc_safety::frame_auth::FrameAuth::new(
            Some(SECRET.to_string()),
            false,
        ));
        *unsigned.local_announcement.write().await = Some(announcement("brain", &[]));
        assert!(unsigned
            .start_serving(Arc::clone(&echo), tools())
            .await
            .is_err());

        let quiet = secured("brain");
        assert!(quiet
            .start_serving(Arc::clone(&echo), tools())
            .await
            .is_err());

        let brain = secured("brain");
        *brain.local_announcement.write().await = Some(announcement("brain", &[]));
        let announced = brain.start_serving(echo, tools()).await.unwrap();
        assert_eq!(announced.tools, tools());
    }

    #[tokio::test]
    async fn incoming_calls_need_a_tag_a_paired_caller_and_a_served_tool() {
        let echo = Arc::new(Echo::default());
        let brain = secured("brain");
        *brain.local_announcement.write().await = Some(announcement("brain", &[]));
        brain.on_announce(datagram("a", &[], 44445), src()).await;
        let counters = obc_safety::frame_auth::OutboundCounters::new();

        // Nothing served yet: refused, and the refusal is signed.
        let refused = brain
            .answer(signed_call("a", "gpio_read", &counters))
            .await
            .unwrap();
        assert!(!refused.ok);
        assert!(refused.mac.is_some());

        brain
            .start_serving(echo.clone(), vec![spec("gpio_read")])
            .await
            .unwrap();

        let served = brain
            .answer(signed_call("a", "gpio_read", &counters))
            .await
            .unwrap();
        assert!(served.ok, "{:?}", served.error);
        assert_eq!(served.output.as_deref(), Some("gpio_read for a"));
        // The caller can check it came from the brain.
        let caller = obc_safety::frame_auth::FrameAuth::new(Some(SECRET.to_string()), true);
        assert_eq!(
            caller.verify_inbound(
                "brain",
                served.ctr,
                served.mac.as_deref(),
                &served.signed_bytes()
            ),
            obc_safety::frame_auth::FrameVerdict::Accept
        );

        let unserved = brain
            .answer(signed_call("a", "shell", &counters))
            .await
            .unwrap();
        assert!(!unserved.ok);
        assert!(unserved.error.unwrap().contains("not served"));

        // A tag that does not cover the body, or no tag: dropped unanswered.
        let mut tampered = signed_call("a", "gpio_read", &counters);
        tampered.args = serde_json::json!({ "pin": 4 });
        assert!(brain.answer(tampered).await.is_none());
        let mut bare = signed_call("a", "gpio_read", &counters);
        (bare.ctr, bare.mac) = (None, None);
        assert!(brain.answer(bare).await.is_none());
        // A good signature does not stand in for the tag: membership is
        // checked as well as identity.
        let mut untagged = signed_call("a", "gpio_read", &counters);
        untagged.mac = None;
        assert!(brain.answer(untagged).await.is_none());

        // A member that has not announced to this node yet: no key is pinned
        // for it, so nothing runs, but it is told why instead of timing out.
        let stranger_counters = obc_safety::frame_auth::OutboundCounters::new();
        let stranger = brain
            .answer(signed_call("b", "gpio_read", &stranger_counters))
            .await
            .expect("a member with no pin yet is answered");
        assert!(!stranger.ok);
        assert!(stranger.error.unwrap().contains("announce first"));
        assert!(stranger.sig.is_some());
        // Someone outside the mesh gets nothing.
        let mut outsider = signed_call("c", "gpio_read", &stranger_counters);
        outsider.mac = Some("00".repeat(32));
        assert!(brain.answer(outsider).await.is_none());

        assert_eq!(
            *echo.calls.lock().unwrap(),
            [("a".to_string(), "gpio_read".to_string())]
        );
    }

    #[tokio::test]
    async fn a_member_cannot_call_as_another_member() {
        let echo = Arc::new(Echo::default());
        let brain = secured("brain");
        *brain.local_announcement.write().await = Some(announcement("brain", &[]));
        brain.on_announce(datagram("a", &[], 44445), src()).await;
        brain.on_announce(datagram("b", &[], 44446), src()).await;
        brain
            .start_serving(echo.clone(), vec![spec("gpio_read")])
            .await
            .unwrap();

        // `a` holds the pairing secret, so it can derive `b`'s HMAC key and tag
        // a call as `b` that the shared-secret check alone would take. What it
        // cannot do is sign with `b`'s identity key.
        let counters = obc_safety::frame_auth::OutboundCounters::new();
        let mut forged = ToolCallRequest {
            call_id: uuid::Uuid::new_v4().to_string(),
            tool_name: "gpio_read".to_string(),
            args: serde_json::json!({ "pin": 3 }),
            from: Some("b".to_string()),
            ctr: None,
            mac: None,
            sig: None,
        };
        let auth = obc_safety::frame_auth::FrameAuth::new(Some(SECRET.to_string()), true);
        forged.sign_p2p(&auth, &identity_of("a"), &counters, "b", "brain");
        assert_eq!(
            auth.verify_inbound(
                "b",
                forged.ctr,
                forged.mac.as_deref(),
                &forged.signed_bytes()
            ),
            obc_safety::frame_auth::FrameVerdict::Accept,
            "the HMAC tag alone does not tell members apart"
        );
        assert!(brain.answer(forged).await.is_none());
        assert!(echo.calls.lock().unwrap().is_empty());

        // `b` itself is still served, and its window was not moved by the forgery.
        let genuine = brain
            .answer(signed_call("b", "gpio_read", &counters))
            .await
            .unwrap();
        assert!(genuine.ok, "{:?}", genuine.error);
        assert_eq!(genuine.output.as_deref(), Some("gpio_read for b"));

        // And `b` can tell the answer came from the brain, not another member.
        let brain_key =
            obc_safety::node_identity::parse_public(&identity_of("brain").public_hex()).unwrap();
        assert!(obc_safety::node_identity::verify_frame(
            &brain_key,
            obc_safety::node_identity::FrameKind::Result,
            "brain",
            "b",
            genuine.ctr.unwrap(),
            &genuine.signed_bytes(),
            genuine.sig.as_deref().unwrap(),
        ));
    }

    #[tokio::test]
    async fn an_announcement_under_another_key_does_not_take_over_an_id() {
        let brain = secured("brain");
        brain.on_announce(datagram("b", &[], 44446), src()).await;

        // `a` announces as `b`, pairing token and all, under its own key.
        brain
            .on_announce(
                datagram_signed_by("b", &["gpio_read"], 50000, &identity_of("a")),
                src(),
            )
            .await;
        // `a` replays `b`'s own announcement with its address swapped in.
        let mut redirected = datagram("b", &["gpio_read"], 44446);
        redirected["tcp_port"] = 50000.into();
        brain.on_announce(redirected, src()).await;

        let registry = brain.peer_registry.read().await;
        let b = registry.get("b").unwrap();
        assert_eq!(b.tcp_addr.port(), 44446);
        assert!(b.announcement.tools.is_empty());
    }

    #[tokio::test]
    async fn a_replayed_bind_all_announcement_does_not_move_the_node() {
        let brain = secured("brain");
        let b_at = |ip: &str| SocketAddr::from(([10, 0, 0, ip.parse().unwrap()], 9));
        let mut bind_all = unsealed("b", &[], 44446);
        bind_all.tcp_host = "0.0.0.0".to_string();
        let genuine = bind_all.seal(&identity_of("b"));
        brain.on_announce(genuine.clone(), b_at("2")).await;

        // `a` resends `b`'s datagram, signature intact, from its own address:
        // with a bind-all host, the address dialled is the one it came from.
        brain.on_announce(genuine, b_at("66")).await;
        async fn tcp_addr(brain: &P2pSpine) -> SocketAddr {
            brain.peer_registry.read().await.get("b").unwrap().tcp_addr
        }
        assert_eq!(tcp_addr(&brain).await, "10.0.0.2:44446".parse().unwrap());

        // `b`'s next announcement, from wherever it now is, is taken.
        let mut moved = unsealed("b", &[], 44446);
        moved.tcp_host = "0.0.0.0".to_string();
        brain
            .on_announce(moved.seal(&identity_of("b")), b_at("3"))
            .await;
        assert_eq!(tcp_addr(&brain).await, "10.0.0.3:44446".parse().unwrap());
    }

    #[tokio::test]
    async fn an_announcement_with_fields_this_version_lacks_still_verifies() {
        let brain = secured("brain");
        let identity = identity_of("a");
        let mut newer = unsealed("a", &["gpio_read"], 44445);
        newer.announcement.metadata["p2p_pubkey"] = Value::String(identity.public_hex());
        let mut datagram = serde_json::to_value(&newer).unwrap();
        datagram["hop_limit"] = 3.into();
        datagram["announcement"]["region"] = "kitchen".into();
        datagram["sig"] = identity.sign_announcement(&announce_body(&datagram)).into();

        brain.on_announce(datagram, src()).await;
        assert!(brain.peer_registry.read().await.contains_key("a"));
        assert!(brain.peer_keys.key_for("a").is_some());
    }

    #[tokio::test]
    async fn only_a_paired_node_has_its_key_pinned() {
        // Pairing evaluated but not required, frame auth off: an unpaired node
        // is admitted, as before, but its key is not pinned.
        let spine = P2pSpine::new(P2pConfig {
            node_id: "brain".to_string(),
            ..P2pConfig::default()
        })
        .with_pairing(
            obc_safety::NodePairingManager::new(Some(SECRET.to_string())),
            false,
        );
        let unpaired = P2pAnnounce {
            announcement: announcement("c", &["gpio_read"]),
            tcp_host: "127.0.0.1".to_string(),
            tcp_port: 44447,
            sig: None,
            seq: 1,
        }
        .seal(&identity_of("c"));
        spine.on_announce(unpaired, src()).await;
        assert!(spine.peer_registry.read().await.contains_key("c"));
        assert!(spine.peer_keys.key_for("c").is_none());

        spine.on_announce(datagram("a", &[], 44445), src()).await;
        assert!(spine.peer_keys.key_for("a").is_some());
    }

    #[tokio::test]
    async fn a_call_round_trips_between_two_nodes() {
        let echo = Arc::new(Echo::default());
        let brain = secured("brain").start().await.unwrap();
        let brain_port = brain.config.tcp_port;
        *brain.local_announcement.write().await = Some(announcement("brain", &[]));
        brain
            .start_serving(echo.clone(), vec![spec("gpio_read")])
            .await
            .unwrap();

        let edge = Arc::new(secured("a"));
        brain.on_announce(datagram("a", &[], 44445), src()).await;
        edge.on_announce(datagram("brain", &["gpio_read"], brain_port), src())
            .await;

        let result = edge
            .invoke_tool("brain", "gpio_read", serde_json::json!({ "pin": 3 }))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.output.as_deref(), Some("gpio_read for a"));

        // A refusal comes back as a refusal, not as a timeout.
        let refused = edge
            .invoke_tool("brain", "shell", serde_json::json!({}))
            .await
            .unwrap();
        assert!(!refused.ok);
        assert_eq!(echo.calls.lock().unwrap().len(), 1);
    }

    /// Takes longer than the server's timeout, then says it finished.
    struct Slow(mpsc::Sender<()>);

    #[async_trait]
    impl PeerToolServer for Slow {
        async fn call(&self, _peer: &str, _tool: &str, _args: Value) -> Result<ToolResult> {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            self.0.send(()).await.ok();
            Ok(ToolResult::ok("done"))
        }
    }

    #[tokio::test]
    async fn a_slow_tool_is_answered_for_and_never_cancelled() {
        let (done_tx, mut done) = mpsc::channel(1);
        let mut brain = secured("brain");
        brain.config.tool_timeout_secs = 1;
        let brain = brain.start().await.unwrap();
        let brain_port = brain.config.tcp_port;
        *brain.local_announcement.write().await = Some(announcement("brain", &[]));
        brain
            .start_serving(Arc::new(Slow(done_tx)), vec![spec("gpio_write")])
            .await
            .unwrap();
        // The caller has the same timeout: it must still hear the server out.
        let mut edge = secured("a");
        edge.config.tool_timeout_secs = 1;
        brain.on_announce(datagram("a", &[], 44445), src()).await;
        edge.on_announce(datagram("brain", &["gpio_write"], brain_port), src())
            .await;

        let result = edge
            .invoke_tool("brain", "gpio_write", serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.error.as_deref().unwrap().contains("still running"),
            "{:?}",
            result.error
        );
        // And the tool ran to the end rather than being dropped mid-action.
        tokio::time::timeout(Duration::from_secs(5), done.recv())
            .await
            .expect("the tool was cancelled")
            .unwrap();
    }

    #[tokio::test]
    async fn a_second_board_with_our_id_is_reported_and_not_admitted() {
        let spine = secured("brain");
        let mut own = datagram("brain", &["shell"], 44445);
        own["announcement"]["metadata"]["p2p_instance"] = Value::String(spine.instance.clone());
        spine.on_announce(own, src()).await;
        assert!(spine.duplicates_seen.lock().unwrap().is_empty());

        // Same id, another process: a second board configured with it.
        spine
            .on_announce(datagram("brain", &["shell"], 44445), src())
            .await;
        assert!(spine.duplicates_seen.lock().unwrap().contains(&src()));
        assert!(spine.known_nodes().await.is_empty());
    }

    #[tokio::test]
    async fn a_full_event_channel_never_blocks_discovery() {
        let (tx, mut rx) = mpsc::channel(1);
        let spine = secured("brain").with_peer_events(tx);
        // Nobody is reading: the second join cannot be queued, and must not wait.
        tokio::time::timeout(Duration::from_secs(1), async {
            spine
                .on_announce(datagram("a", &["gpio_read"], 44445), src())
                .await;
            spine
                .on_announce(datagram("b", &["dht22"], 44445), src())
                .await;
            spine.evict_stale().await;
        })
        .await
        .expect("discovery blocked on the channel");
        assert!(matches!(rx.recv().await, Some(PeerEvent::Joined { .. })));

        // With room again, the next pass sends the whole mesh.
        spine.evict_stale().await;
        let Some(PeerEvent::Snapshot { peers }) = rx.recv().await else {
            panic!("expected a snapshot");
        };
        assert_eq!(peers.len(), 2);
        assert_eq!(peers["b"], vec![spec("dht22")]);
        spine.evict_stale().await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_connection_must_send_its_request_in_time() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _idle = TcpStream::connect(addr).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let err = handle_tcp_connection(
            stream,
            Arc::new(secured("brain")),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not received"), "{err}");
    }

    #[tokio::test]
    async fn incoming_connections_are_capped() {
        let brain = secured("brain").start().await.unwrap();
        let addr = format!("127.0.0.1:{}", brain.config.tcp_port);
        let mut idle = Vec::new();
        for _ in 0..MAX_INBOUND_CONNECTIONS {
            idle.push(TcpStream::connect(&addr).await.unwrap());
        }
        // One past the cap is closed at once instead of waiting for a request.
        let mut over = TcpStream::connect(&addr).await.unwrap();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(3), over.read(&mut byte)).await;
        assert!(
            matches!(read, Ok(Ok(0)) | Ok(Err(_))),
            "the connection over the cap was held open"
        );
        drop(idle);
    }
}

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

use crate::spine::{NodeAnnouncement, ToolCallRequest, ToolCallResult};
use crate::tools::traits::{Tool, ToolResult};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{oneshot, Mutex, RwLock};

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
}

type PeerRegistry = Arc<RwLock<HashMap<String, P2pPeer>>>;
type PendingCalls = Arc<Mutex<HashMap<String, oneshot::Sender<ToolCallResult>>>>;

// ── P2pSpine ──────────────────────────────────────────────────────────────────

/// A broker-free communication spine based on UDP discovery and direct TCP
/// tool calls.
///
/// `P2pSpine` mirrors the public API of `SpineClient` so that the rest of the
/// codebase can swap between MQTT and P2P spines via configuration.
pub struct P2pSpine {
    config: P2pConfig,
    peer_registry: PeerRegistry,
    pending_calls: PendingCalls,
    /// Our own announcement — set once we call `start()`.
    local_announcement: RwLock<Option<NodeAnnouncement>>,
}

impl P2pSpine {
    /// Create a new `P2pSpine` from configuration.
    pub fn new(config: P2pConfig) -> Self {
        Self {
            config,
            peer_registry: Arc::new(RwLock::new(HashMap::new())),
            pending_calls: Arc::new(Mutex::new(HashMap::new())),
            local_announcement: RwLock::new(None),
        }
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

        let pending_calls = Arc::clone(&arc.pending_calls);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        tracing::debug!(peer = %peer, "Incoming P2P TCP connection");
                        let pending = Arc::clone(&pending_calls);
                        tokio::spawn(async move {
                            if let Err(e) = handle_tcp_connection(stream, pending).await {
                                tracing::warn!(error = %e, "P2P TCP handler error");
                            }
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

        let peer_registry = Arc::clone(&arc.peer_registry);
        let peer_timeout = arc.config.peer_timeout_secs;
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match udp_rx.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        if let Ok(announce) = serde_json::from_slice::<P2pAnnounce>(&buf[..len]) {
                            let node_id = announce.announcement.node_id.clone();
                            // If the sender broadcast "0.0.0.0" (bind-all), use the
                            // actual UDP source IP so we can dial back to the peer.
                            let tcp_host = if announce.tcp_host == "0.0.0.0" {
                                src.ip().to_string()
                            } else {
                                announce.tcp_host.clone()
                            };
                            let tcp_addr_str = format!("{}:{}", tcp_host, announce.tcp_port);
                            match tcp_addr_str.parse::<SocketAddr>() {
                                Ok(tcp_addr) => {
                                    tracing::debug!(
                                        node_id = %node_id,
                                        tcp_addr = %tcp_addr,
                                        "Discovered P2P peer via UDP"
                                    );
                                    let mut registry = peer_registry.write().await;
                                    registry.insert(
                                        node_id,
                                        P2pPeer {
                                            announcement: announce.announcement,
                                            tcp_addr,
                                            last_seen: Instant::now(),
                                        },
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        src = %src,
                                        error = %e,
                                        "Invalid TCP address in P2P announcement"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "UDP receive error");
                    }
                }

                // Evict stale peers
                let mut registry = peer_registry.write().await;
                registry.retain(|id, peer| {
                    let alive = peer.last_seen.elapsed().as_secs() < peer_timeout;
                    if !alive {
                        tracing::info!(node_id = %id, "Evicting stale P2P peer");
                    }
                    alive
                });
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

    /// Publish our node announcement to the local network via UDP broadcast.
    pub async fn announce(&self, announcement: &NodeAnnouncement) -> Result<()> {
        *self.local_announcement.write().await = Some(announcement.clone());
        self.broadcast_presence().await
    }

    /// Broadcast the stored local announcement via UDP.
    async fn broadcast_presence(&self) -> Result<()> {
        let announcement = self.local_announcement.read().await.clone();
        let announcement = match announcement {
            Some(a) => a,
            None => return Ok(()), // not yet announced
        };

        let payload = P2pAnnounce {
            announcement,
            tcp_host: self.config.bind_host.clone(),
            tcp_port: self.config.tcp_port,
        };

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
        let request = ToolCallRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.to_string(),
            args,
        };

        let (tx, rx) = oneshot::channel();
        self.pending_calls.lock().await.insert(call_id.clone(), tx);

        let payload = serde_json::to_vec(&request)?;

        // Connect and send
        let mut stream =
            tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(tcp_addr))
                .await
                .map_err(|_| anyhow::anyhow!("TCP connect timed out for node={}", node_id))?
                .map_err(|e| anyhow::anyhow!("TCP connect failed for node={}: {}", node_id, e))?;

        write_framed(&mut stream, &payload).await?;

        // Wait for the result to come back via the TCP server
        let timeout = Duration::from_secs(self.config.tool_timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => bail!("P2P call channel dropped for call_id={}", call_id),
            Err(_) => {
                self.pending_calls.lock().await.remove(&call_id);
                bail!(
                    "P2P tool call timed out after {}s (node={}, tool={})",
                    self.config.tool_timeout_secs,
                    node_id,
                    tool_name
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

/// Handle a single incoming TCP connection: read a `ToolCallRequest`, resolve
/// it against pending_calls, and write back the `ToolCallResult`.
///
/// In a full implementation the node would execute the tool locally and send
/// the result.  Here we route the result back to any local waiter (i.e. the
/// caller that originally invoked `invoke_tool()`).  For cross-node calls the
/// responding node writes a `ToolCallResult` frame back over the same
/// connection.
async fn handle_tcp_connection(mut stream: TcpStream, pending_calls: PendingCalls) -> Result<()> {
    let payload = read_framed(&mut stream).await?;

    // Try to interpret as a ToolCallResult (response from a remote node)
    if let Ok(result) = serde_json::from_slice::<ToolCallResult>(&payload) {
        let call_id = result.call_id.clone();
        if let Some(sender) = pending_calls.lock().await.remove(&call_id) {
            let _ = sender.send(result);
        }
        return Ok(());
    }

    // Try to interpret as a ToolCallRequest (incoming call from a remote node)
    if let Ok(request) = serde_json::from_slice::<ToolCallRequest>(&payload) {
        tracing::debug!(
            call_id = %request.call_id,
            tool = %request.tool_name,
            "Received P2P tool call request"
        );
        // Callers that registered a pending call locally will not be waiting
        // here, so we send an "unknown tool" error back to the remote caller.
        let result = ToolCallResult {
            call_id: request.call_id,
            ok: false,
            output: None,
            error: Some(format!(
                "Tool '{}' is not directly handled by this P2P listener; \
                 route through the agent loop",
                request.tool_name
            )),
        };
        let resp_payload = serde_json::to_vec(&result)?;
        write_framed(&mut stream, &resp_payload).await?;
        return Ok(());
    }

    tracing::warn!("Received unrecognised P2P TCP message; ignoring");
    Ok(())
}

// ── P2P Node Tool ─────────────────────────────────────────────────────────────

/// A `Tool` implementation that delegates execution to a remote P2P peer.
struct P2pNodeTool {
    node_id: String,
    spec: crate::spine::NodeToolSpec,
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
        use crate::spine::{NodeAnnouncement, NodeToolSpec};

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
        };

        let json = serde_json::to_string(&announce).unwrap();
        let back: P2pAnnounce = serde_json::from_str(&json).unwrap();
        assert_eq!(back.announcement.node_id, "nanopi-kitchen");
        assert_eq!(back.tcp_port, 44445);
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
}

//! LoRa-mesh spine transport — off-grid fleet coordination, no WiFi, no broker.
//!
//! The MQTT and P2P spines assume IP connectivity. This transport targets the
//! opposite regime: a fleet spread across kilometres with **no infrastructure**,
//! coordinating over a long-range LoRa mesh (Meshtastic-class radios — T-Beam,
//! Heltec, RAK4631). That regime is brutal on a protocol: payloads cap around
//! ~230 bytes, latency is seconds, and there is no broker to fan messages out.
//!
//! So this is deliberately **not** full tool-RPC (a JSON tool call won't fit a LoRa
//! frame). It carries exactly what a fleet needs to coordinate off-grid: compact
//! **heartbeats** (a node's pose/battery/mode) and **assignments** (go here). Those
//! map straight onto the [`crate::fleet`] coordinator — a LoRa heartbeat becomes a
//! `fleet::NodeState`, so the auction/exploration logic we already have runs over
//! the mesh unchanged.
//!
//! The radio itself is abstracted behind [`MeshRadio`]: a real implementation talks
//! to a Meshtastic device over serial; tests use an in-memory loopback. Everything
//! here is hardware-free and testable.

use crate::fleet::{Coordinator, NodeState};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// LoRa-mesh radio parameters. `max_payload` is the hard frame limit the codec is
/// held to (Meshtastic's usable payload is ~200–237 bytes depending on settings).
#[derive(Debug, Clone)]
pub struct LoraMeshConfig {
    /// Regulatory region (e.g. `"US"`, `"EU868"`).
    pub region: String,
    /// Centre frequency in MHz.
    pub freq_mhz: f64,
    /// Maximum on-air payload in bytes; frames larger than this are refused.
    pub max_payload: usize,
    /// This node's mesh address.
    pub node_num: u32,
}

impl Default for LoraMeshConfig {
    fn default() -> Self {
        Self { region: "US".to_string(), freq_mhz: 915.0, max_payload: 230, node_num: 0 }
    }
}

/// A compact fleet message sized for a single LoRa frame. Uses short JSON keys so
/// it stays small yet debuggable; absent optional fields are omitted to save bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshFrame {
    /// A node reporting its state (pose / battery / mode).
    Heartbeat {
        node: String,
        x: Option<f64>,
        y: Option<f64>,
        battery: Option<f64>,
        mode: String,
    },
    /// A coordinator assigning a node a target.
    Assign { node: String, x: f64, y: f64 },
}

impl MeshFrame {
    /// Encode to compact bytes for transmission.
    pub fn encode(&self) -> Vec<u8> {
        let v = match self {
            MeshFrame::Heartbeat { node, x, y, battery, mode } => {
                let mut m = serde_json::Map::new();
                m.insert("t".into(), json!("hb"));
                m.insert("n".into(), json!(node));
                if let Some(x) = x {
                    m.insert("x".into(), json!(x));
                }
                if let Some(y) = y {
                    m.insert("y".into(), json!(y));
                }
                if let Some(b) = battery {
                    m.insert("b".into(), json!(b));
                }
                m.insert("m".into(), json!(mode));
                Value::Object(m)
            }
            MeshFrame::Assign { node, x, y } => json!({ "t": "as", "n": node, "x": x, "y": y }),
        };
        serde_json::to_vec(&v).unwrap_or_default()
    }

    /// Decode a received frame. `None` if the bytes are not a valid frame.
    pub fn decode(bytes: &[u8]) -> Option<MeshFrame> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        match v.get("t").and_then(Value::as_str)? {
            "hb" => Some(MeshFrame::Heartbeat {
                node: v.get("n")?.as_str()?.to_string(),
                x: v.get("x").and_then(Value::as_f64),
                y: v.get("y").and_then(Value::as_f64),
                battery: v.get("b").and_then(Value::as_f64),
                mode: v.get("m").and_then(Value::as_str).unwrap_or("unknown").to_string(),
            }),
            "as" => Some(MeshFrame::Assign {
                node: v.get("n")?.as_str()?.to_string(),
                x: v.get("x")?.as_f64()?,
                y: v.get("y")?.as_f64()?,
            }),
            _ => None,
        }
    }

    /// Encoded size in bytes.
    pub fn encoded_len(&self) -> usize {
        self.encode().len()
    }

    /// Bridge a heartbeat into a [`fleet::NodeState`](crate::fleet::NodeState) so
    /// the fleet coordinator (allocation, auction, exploration) runs over the mesh
    /// unchanged. `None` for non-heartbeat frames.
    pub fn to_node_state(&self, now_ms: u64) -> Option<NodeState> {
        match self {
            MeshFrame::Heartbeat { node, x, y, battery, mode } => Some(NodeState {
                id: node.clone(),
                x: *x,
                y: *y,
                battery: *battery,
                mode: mode.clone(),
                busy: false,
                last_seen_ms: now_ms,
            }),
            MeshFrame::Assign { .. } => None,
        }
    }
}

// ── RX bridge: received frames → fleet coordinator ──────────────────────────────

/// Bridge a received mesh frame into the fleet coordinator: a heartbeat becomes a
/// reported `NodeState` (so the auction/exploration logic runs over the mesh).
/// Returns `true` if the frame was a heartbeat (and thus ingested).
pub fn bridge_frame(frame: &MeshFrame, coord: &Coordinator, now_ms: u64) -> bool {
    match frame.to_node_state(now_ms) {
        Some(state) => {
            coord.report(state);
            true
        }
        None => false,
    }
}

/// Decode one received line of frame bytes and bridge it into the coordinator.
/// Returns `true` if a heartbeat was ingested; malformed lines are ignored.
pub fn ingest_line(bytes: &[u8], coord: &Coordinator, now_ms: u64) -> bool {
    match MeshFrame::decode(bytes) {
        Some(frame) => bridge_frame(&frame, coord, now_ms),
        None => false,
    }
}

/// A LoRa-mesh radio. A real implementation drives a Meshtastic device over serial;
/// the spine only needs to hand it framed bytes to transmit.
#[async_trait]
pub trait MeshRadio: Send + Sync {
    /// Transmit one frame onto the mesh.
    async fn transmit(&self, bytes: &[u8]) -> anyhow::Result<()>;
}

/// The LoRa-mesh spine: frames fleet messages and hands them to a [`MeshRadio`],
/// refusing any frame that would exceed the configured on-air payload limit.
pub struct LoraMeshSpine {
    radio: Arc<dyn MeshRadio>,
    cfg: LoraMeshConfig,
}

impl LoraMeshSpine {
    pub fn new(radio: Arc<dyn MeshRadio>, cfg: LoraMeshConfig) -> Self {
        Self { radio, cfg }
    }

    /// Frame, size-check, and transmit. Returns an error if the encoded frame
    /// exceeds `max_payload` (it must be split or shortened by the caller).
    pub async fn send_frame(&self, frame: &MeshFrame) -> anyhow::Result<()> {
        let bytes = frame.encode();
        if bytes.len() > self.cfg.max_payload {
            return Err(anyhow::anyhow!(
                "LoRa frame is {} bytes, over the {}-byte on-air limit",
                bytes.len(),
                self.cfg.max_payload
            ));
        }
        self.radio.transmit(&bytes).await
    }

    /// Broadcast this node's heartbeat over the mesh.
    pub async fn send_heartbeat(
        &self,
        node: &str,
        x: Option<f64>,
        y: Option<f64>,
        battery: Option<f64>,
        mode: &str,
    ) -> anyhow::Result<()> {
        self.send_frame(&MeshFrame::Heartbeat {
            node: node.to_string(),
            x,
            y,
            battery,
            mode: mode.to_string(),
        })
        .await
    }

    /// Send a coordinator assignment over the mesh.
    pub async fn send_assignment(&self, node: &str, x: f64, y: f64) -> anyhow::Result<()> {
        self.send_frame(&MeshFrame::Assign { node: node.to_string(), x, y }).await
    }

    pub fn config(&self) -> &LoraMeshConfig {
        &self.cfg
    }
}

// ── TX bridge: coordinator assignments → mesh ───────────────────────────────────

/// Broadcast a coordinator's queued assignment intents onto the mesh: each drained
/// `(node, x, y)` becomes a [`MeshFrame::Assign`] transmitted through `radio`. This
/// is the outbound half of the fleet↔mesh bridge (the inbound half is
/// [`ingest_line`]); together they let the fleet coordinate off-grid — heartbeats
/// in, "go here" assignments out. Returns the number of frames sent.
pub async fn broadcast_outbox<R: MeshRadio + ?Sized>(radio: &R, coord: &Coordinator) -> usize {
    let mut sent = 0;
    for (node, x, y) in coord.drain_outbox() {
        let frame = MeshFrame::Assign { node, x, y };
        if radio.transmit(&frame.encode()).await.is_ok() {
            sent += 1;
        }
    }
    sent
}

/// Transmit a single assignment as a [`MeshFrame::Assign`]. When `ttl > 0` the frame
/// is wrapped with relay metadata ([`relay::originate`]) so it can hop across the
/// mesh; `ttl == 0` sends a bare single-hop frame. Returns whether the send
/// succeeded. Lets `main` emit assignments without importing the [`MeshRadio`] trait.
pub async fn send_assignment_frame<R: MeshRadio + ?Sized>(
    radio: &R,
    node: &str,
    x: f64,
    y: f64,
    id: u64,
    ttl: u8,
) -> bool {
    let frame = MeshFrame::Assign { node: node.to_string(), x, y };
    let bytes = if ttl > 0 { relay::originate(&frame, id, ttl) } else { frame.encode() };
    radio.transmit(&bytes).await.is_ok()
}

// ── Multi-hop relay (TTL rebroadcast + de-duplication) ──────────────────────────

/// Multi-hop flooding over the single-hop broadcast transport. A frame may carry
/// two optional envelope keys — `i` (a unique message id) and `h` (remaining hops).
/// A node that hears an id it hasn't seen processes it locally and, if hops remain,
/// rebroadcasts it with `h-1`; a repeat id is dropped. Bare frames (no `i`) are
/// single-hop as before, so this is fully backward-compatible and the node firmware
/// stays a dumb byte relay — multi-hop is entirely a host-side concern.
pub mod relay {
    use super::MeshFrame;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// What to do with a received frame.
    #[derive(Debug, Clone, PartialEq)]
    pub struct RelayDecision {
        /// Whether this node should handle the frame (ingest it) — false for a
        /// duplicate already seen, or unparseable bytes.
        pub process_local: bool,
        /// Bytes to rebroadcast (with decremented hop count), if any.
        pub rebroadcast: Option<Vec<u8>>,
    }

    /// Wrap a frame with relay metadata so it floods up to `ttl` hops. `id` must be
    /// unique per originated message (a monotonic counter is fine).
    pub fn originate(frame: &MeshFrame, id: u64, ttl: u8) -> Vec<u8> {
        let mut v: Value = match serde_json::from_slice(&frame.encode()) {
            Ok(v) => v,
            Err(_) => return frame.encode(),
        };
        if let Value::Object(ref mut m) = v {
            m.insert("i".into(), json!(id));
            m.insert("h".into(), json!(ttl));
        }
        serde_json::to_vec(&v).unwrap_or_else(|_| frame.encode())
    }

    /// A bounded de-duplication window over recently-seen message ids.
    pub struct MeshRelay {
        seen: Mutex<VecDeque<u64>>,
        cap: usize,
    }

    impl MeshRelay {
        pub fn new() -> Self {
            Self { seen: Mutex::new(VecDeque::new()), cap: 512 }
        }

        /// Record `id`; returns `true` if it is newly seen, `false` if a duplicate.
        fn mark_seen(&self, id: u64) -> bool {
            let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            if seen.contains(&id) {
                return false;
            }
            seen.push_back(id);
            while seen.len() > self.cap {
                seen.pop_front();
            }
            true
        }

        /// Decide how to handle received frame bytes: whether to process it locally
        /// and whether (and what) to rebroadcast.
        pub fn on_receive(&self, bytes: &[u8]) -> RelayDecision {
            let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
                return RelayDecision { process_local: false, rebroadcast: None };
            };
            // Bare frame (no relay envelope): single-hop, process, never rebroadcast.
            let Some(id) = v.get("i").and_then(Value::as_u64) else {
                return RelayDecision { process_local: true, rebroadcast: None };
            };
            if !self.mark_seen(id) {
                // Already flooded through here — drop to break the loop.
                return RelayDecision { process_local: false, rebroadcast: None };
            }
            let ttl = v.get("h").and_then(Value::as_u64).unwrap_or(0);
            let rebroadcast = if ttl > 0 {
                let mut relayed = v.clone();
                if let Value::Object(ref mut m) = relayed {
                    m.insert("h".into(), json!(ttl - 1));
                }
                serde_json::to_vec(&relayed).ok()
            } else {
                None
            };
            RelayDecision { process_local: true, rebroadcast }
        }
    }

    impl Default for MeshRelay {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// Decode + bridge a received line into the coordinator, applying multi-hop relay:
/// ingests the frame locally when appropriate and returns any bytes the caller
/// should rebroadcast (with a decremented hop count). Combines [`ingest_line`] with
/// [`relay::MeshRelay`].
pub fn ingest_line_relayed(
    bytes: &[u8],
    coord: &Coordinator,
    relay: &relay::MeshRelay,
    now_ms: u64,
) -> Option<Vec<u8>> {
    let decision = relay.on_receive(bytes);
    if decision.process_local {
        let _ = ingest_line(bytes, coord, now_ms);
    }
    decision.rebroadcast
}

// ── Serial radio + RX loop (real hardware; `--features hardware`) ────────────────
//
// The pluggable-radio counterpart to the codec above: a `MeshRadio` over a serial
// link to a LoRa node (T-Beam / Heltec / RAK4631) flashed with OBC firmware that
// exchanges `MeshFrame`s as newline-delimited bytes over its USB serial. This is
// *not* a full Meshtastic-protobuf client — it speaks OBC's own compact frame
// codec, consistent with the rest of this module. Serial I/O is feature-gated
// behind `hardware` (tokio-serial), matching the peripheral drivers.
#[cfg(feature = "hardware")]
mod serial {
    use super::relay::MeshRelay;
    use super::{ingest_line, ingest_line_relayed, Coordinator, MeshRadio};
    use anyhow::Context;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
    use tokio::sync::Mutex;
    use tokio_serial::{SerialPortBuilderExt, SerialStream};

    /// A [`MeshRadio`](super::MeshRadio) over a serial link to an OBC-firmware LoRa
    /// node. `transmit` writes a newline-framed frame; the paired read half drives
    /// [`run_serial_rx`].
    pub struct SerialMeshRadio {
        writer: Mutex<WriteHalf<SerialStream>>,
    }

    impl SerialMeshRadio {
        /// Open the serial port; returns the radio (for transmit) and the read half
        /// for the RX loop.
        pub fn open(port: &str, baud: u32) -> anyhow::Result<(Self, ReadHalf<SerialStream>)> {
            let serial = tokio_serial::new(port, baud)
                .open_native_async()
                .with_context(|| format!("failed to open LoRa serial port {port}"))?;
            let (rd, wr) = tokio::io::split(serial);
            Ok((Self { writer: Mutex::new(wr) }, rd))
        }
    }

    #[async_trait::async_trait]
    impl MeshRadio for SerialMeshRadio {
        async fn transmit(&self, bytes: &[u8]) -> anyhow::Result<()> {
            let mut w = self.writer.lock().await;
            w.write_all(bytes).await?;
            w.write_all(b"\n").await?;
            w.flush().await?;
            Ok(())
        }
    }

    /// RX loop: read newline-framed frames from the LoRa node and bridge each into
    /// the fleet coordinator. Runs until the serial link closes (EOF / error).
    pub async fn run_serial_rx<F>(
        read_half: ReadHalf<SerialStream>,
        coord: Arc<Coordinator>,
        now_ms: F,
    ) where
        F: Fn() -> u64 + Send,
    {
        let mut lines = BufReader::new(read_half).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let _ = ingest_line(line.trim().as_bytes(), &coord, now_ms());
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("LoRa serial RX error: {e}");
                    break;
                }
            }
        }
    }

    /// RX loop with multi-hop relay: like [`run_serial_rx`], but each received frame
    /// is run through a [`MeshRelay`] — ingested locally when appropriate and
    /// rebroadcast over `radio` (with a decremented hop count) when hops remain, so
    /// heartbeats and assignments flood beyond a single hop.
    pub async fn run_serial_rx_relay<F>(
        read_half: ReadHalf<SerialStream>,
        coord: Arc<Coordinator>,
        radio: Arc<SerialMeshRadio>,
        relay: Arc<MeshRelay>,
        now_ms: F,
    ) where
        F: Fn() -> u64 + Send,
    {
        let mut lines = BufReader::new(read_half).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if let Some(rebroadcast) =
                        ingest_line_relayed(line.trim().as_bytes(), &coord, &relay, now_ms())
                    {
                        let _ = radio.transmit(&rebroadcast).await;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("LoRa serial RX error: {e}");
                    break;
                }
            }
        }
    }
}

#[cfg(feature = "hardware")]
pub use serial::{run_serial_rx, run_serial_rx_relay, SerialMeshRadio};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory radio that records every transmitted frame (for tests).
    struct LoopbackRadio {
        sent: Mutex<Vec<Vec<u8>>>,
    }
    impl LoopbackRadio {
        fn new() -> Self {
            Self { sent: Mutex::new(Vec::new()) }
        }
        fn last(&self) -> Option<Vec<u8>> {
            self.sent.lock().unwrap().last().cloned()
        }
    }
    #[async_trait]
    impl MeshRadio for LoopbackRadio {
        async fn transmit(&self, bytes: &[u8]) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn heartbeat_frame_round_trips() {
        let f = MeshFrame::Heartbeat {
            node: "rover-7".into(),
            x: Some(3.0),
            y: Some(4.0),
            battery: Some(72.0),
            mode: "normal".into(),
        };
        let back = MeshFrame::decode(&f.encode()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn assignment_frame_round_trips() {
        let f = MeshFrame::Assign { node: "rover-2".into(), x: 9.5, y: 0.5 };
        assert_eq!(MeshFrame::decode(&f.encode()).unwrap(), f);
    }

    fn idle_node(id: &str, t: u64) -> NodeState {
        NodeState {
            id: id.into(),
            x: Some(0.0),
            y: Some(0.0),
            battery: Some(90.0),
            mode: "normal".into(),
            busy: false,
            last_seen_ms: t,
        }
    }

    #[tokio::test]
    async fn coordinator_assignments_are_broadcast_over_the_mesh() {
        // Coordinator with the off-grid outbox enabled; one idle node reports in.
        let coord = Coordinator::new().with_assignment_outbox();
        coord.report(idle_node("rover-a", 1_000));
        coord.add_task(crate::fleet::Task { id: "t".into(), x: 5.0, y: 6.0, min_battery: 0.0 });
        // A tick allocates the task -> the intent lands in the outbox.
        assert_eq!(coord.tick(2_000), vec![("t".to_string(), "rover-a".to_string())]);

        // Broadcasting drains the outbox into MeshFrame::Assign frames on the radio.
        let radio = LoopbackRadio::new();
        assert_eq!(broadcast_outbox(&radio, &coord).await, 1);
        assert_eq!(
            MeshFrame::decode(&radio.last().unwrap()).unwrap(),
            MeshFrame::Assign { node: "rover-a".into(), x: 5.0, y: 6.0 }
        );
        // Drained: a second broadcast sends nothing.
        assert_eq!(broadcast_outbox(&radio, &coord).await, 0);
    }

    #[test]
    fn a_bare_frame_is_processed_but_never_relayed() {
        let relay = relay::MeshRelay::new();
        let bytes = MeshFrame::Assign { node: "r".into(), x: 1.0, y: 2.0 }.encode();
        let d = relay.on_receive(&bytes);
        assert!(d.process_local);
        assert!(d.rebroadcast.is_none());
    }

    #[test]
    fn a_relayed_frame_floods_then_dedups() {
        let relay = relay::MeshRelay::new();
        let origin = MeshFrame::Assign { node: "r".into(), x: 1.0, y: 2.0 };
        let bytes = relay::originate(&origin, 42, 2);
        // First hearing: process locally + rebroadcast with hops decremented to 1.
        let d = relay.on_receive(&bytes);
        assert!(d.process_local);
        let rebc = d.rebroadcast.expect("hops remain, so it rebroadcasts");
        let v: serde_json::Value = serde_json::from_slice(&rebc).unwrap();
        assert_eq!(v.get("h").and_then(|h| h.as_u64()), Some(1));
        // The rebroadcast payload still decodes to the original frame.
        assert_eq!(MeshFrame::decode(&rebc).unwrap(), origin);
        // Hearing the same id again (echo from a neighbour): dropped, no rebroadcast.
        let d2 = relay.on_receive(&bytes);
        assert!(!d2.process_local);
        assert!(d2.rebroadcast.is_none());
    }

    #[test]
    fn a_relayed_frame_with_no_hops_left_is_the_last_stop() {
        let relay = relay::MeshRelay::new();
        let bytes = relay::originate(&MeshFrame::Heartbeat {
            node: "n".into(),
            x: None,
            y: None,
            battery: None,
            mode: "idle".into(),
        }, 7, 0);
        let d = relay.on_receive(&bytes);
        assert!(d.process_local, "still ingested at the final hop");
        assert!(d.rebroadcast.is_none(), "ttl 0 → no further flooding");
    }

    #[test]
    fn relayed_ingest_bridges_a_heartbeat_and_returns_the_rebroadcast() {
        let coord = Coordinator::new();
        let relay = relay::MeshRelay::new();
        let hb = MeshFrame::Heartbeat {
            node: "rover-z".into(),
            x: Some(2.0),
            y: Some(3.0),
            battery: Some(80.0),
            mode: "explore".into(),
        };
        let bytes = relay::originate(&hb, 100, 3);
        let rebc = ingest_line_relayed(&bytes, &coord, &relay, 5_000);
        assert!(rebc.is_some(), "hops remain, so caller should rebroadcast");
        // The heartbeat was bridged: the node is now auctionable.
        coord.add_task(crate::fleet::Task { id: "t".into(), x: 2.0, y: 3.0, min_battery: 0.0 });
        assert_eq!(coord.auction_tick(6_000), vec![("t".to_string(), "rover-z".to_string())]);
    }

    #[tokio::test]
    async fn assignments_are_not_collected_without_the_outbox() {
        // Default coordinator (no outbox) collects nothing — single-brain pays zero.
        let coord = Coordinator::new();
        coord.report(idle_node("rover-a", 1_000));
        coord.add_task(crate::fleet::Task { id: "t".into(), x: 5.0, y: 6.0, min_battery: 0.0 });
        coord.tick(2_000);
        let radio = LoopbackRadio::new();
        assert_eq!(broadcast_outbox(&radio, &coord).await, 0);
    }

    #[test]
    fn a_compact_heartbeat_fits_a_lora_frame() {
        let f = MeshFrame::Heartbeat {
            node: "n12".into(),
            x: Some(123.4),
            y: Some(567.8),
            battery: Some(88.0),
            mode: "normal".into(),
        };
        assert!(f.encoded_len() < 230, "heartbeat is {} bytes", f.encoded_len());
    }

    #[tokio::test]
    async fn spine_transmits_a_heartbeat_through_the_radio() {
        let radio = Arc::new(LoopbackRadio::new());
        let spine = LoraMeshSpine::new(Arc::clone(&radio) as Arc<dyn MeshRadio>, LoraMeshConfig::default());
        spine
            .send_heartbeat("rover-1", Some(1.0), Some(2.0), Some(50.0), "normal")
            .await
            .unwrap();
        let bytes = radio.last().expect("a frame was transmitted");
        let frame = MeshFrame::decode(&bytes).unwrap();
        assert_eq!(
            frame,
            MeshFrame::Heartbeat {
                node: "rover-1".into(),
                x: Some(1.0),
                y: Some(2.0),
                battery: Some(50.0),
                mode: "normal".into()
            }
        );
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused() {
        let radio = Arc::new(LoopbackRadio::new());
        // a tiny payload cap forces the refusal path
        let cfg = LoraMeshConfig { max_payload: 16, ..Default::default() };
        let spine = LoraMeshSpine::new(radio, cfg);
        let err = spine
            .send_heartbeat("a-very-long-node-name-that-will-not-fit", Some(1.0), Some(2.0), Some(3.0), "normal")
            .await;
        assert!(err.is_err(), "oversized frame must be refused, not truncated");
    }

    #[test]
    fn heartbeat_bridges_into_the_fleet_coordinator() {
        let f = MeshFrame::Heartbeat {
            node: "rover-9".into(),
            x: Some(5.0),
            y: Some(6.0),
            battery: Some(64.0),
            mode: "normal".into(),
        };
        let state = f.to_node_state(1_000).expect("heartbeat → NodeState");
        assert_eq!(state.id, "rover-9");
        assert_eq!(state.x, Some(5.0));
        assert_eq!(state.battery, Some(64.0));
        assert_eq!(state.last_seen_ms, 1_000);
        // an assignment is not a node report
        assert!(MeshFrame::Assign { node: "x".into(), x: 0.0, y: 0.0 }.to_node_state(1).is_none());
    }

    #[test]
    fn a_received_heartbeat_line_is_bridged_into_the_coordinator() {
        let coord = Coordinator::new();
        let hb = MeshFrame::Heartbeat {
            node: "rover-a".into(),
            x: Some(0.0),
            y: Some(0.0),
            battery: Some(80.0),
            mode: "normal".into(),
        };
        assert!(ingest_line(&hb.encode(), &coord, 1_000));
        // the node is now known: a task at the origin is auctioned to it
        coord.add_task(crate::fleet::Task { id: "t".into(), x: 0.0, y: 0.0, min_battery: 0.0 });
        assert_eq!(coord.auction_tick(1_000), vec![("t".to_string(), "rover-a".to_string())]);
    }

    #[test]
    fn assignments_and_garbage_lines_are_not_node_reports() {
        let coord = Coordinator::new();
        let asn = MeshFrame::Assign { node: "x".into(), x: 1.0, y: 2.0 };
        assert!(!ingest_line(&asn.encode(), &coord, 1));
        assert!(!ingest_line(b"not a frame", &coord, 1));
    }
}

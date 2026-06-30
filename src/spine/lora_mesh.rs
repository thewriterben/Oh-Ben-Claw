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

use crate::fleet::NodeState;
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
}

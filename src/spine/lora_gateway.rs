//! Host-side LoRa **mesh gateway bridge** — the far end of the Phase B spine.
//!
//! A base-station Heltec (running `firmware/heltec-lora-linktest`) hears OBC node
//! spine messages over the air and prints each on its USB console as a line like:
//!
//! ```text
//! SPINE ◄ src=28 seq=30 rssi=-42 dBm : {"type":"reflex","node_id":"obc-esp32-s3-001",...}
//! ```
//!
//! This module reads that console, parses the `SPINE ◄ … : <json>` gateway format,
//! and ingests each node message into [`WorldMemory`] — so link state, power mode,
//! and reflex/safing reports heard across the mesh land in the brain's world model,
//! exactly as if the node were on the wired MQTT spine.
//!
//! It is deliberately the *inverse* of [`super::lora_mesh`]: that module speaks OBC's
//! compact fleet codec (`{"t":"hb"}` heartbeats / `{"t":"as"}` assignments); this one
//! ingests the node's own autonomous JSON (`{"type":…,"node_id":…}`) as reported by
//! the gateway. The parsing + ingest core is hardware-free and unit-tested; only the
//! serial read loop is gated behind the `hardware` feature (tokio-serial), matching
//! the rest of the peripheral I/O.

use crate::memory::world::WorldMemory;
use async_trait::async_trait;
use serde_json::{json, Value};

/// One received frame as reported by a gateway `SPINE ◄` console line.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayFrame {
    /// Originating node id (low byte of its MAC), from `src=` (hex).
    pub src: u8,
    /// Per-source sequence, from `seq=`.
    pub seq: u8,
    /// Received signal strength in dBm, from `rssi=`.
    pub rssi_dbm: i32,
    /// The raw node payload after the ` : ` delimiter (expected to be JSON).
    pub payload: String,
}

/// A summary of what an ingested line contributed to world memory.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayIngest {
    /// The node the message came from (`node_id` field, or `mesh-<src>` fallback).
    pub node_id: String,
    /// The message `type` (`reflex`, `link_state`, `power_mode`, `gw_keepalive`, …).
    pub msg_type: String,
    /// Link quality at the gateway.
    pub rssi_dbm: i32,
}

/// Leading run of `s` whose chars satisfy `pred` (used to lift a token off a field).
fn leading(s: &str, pred: impl Fn(char) -> bool) -> &str {
    let end = s
        .char_indices()
        .find(|(_, c)| !pred(*c))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// The slice of `s` immediately following the first occurrence of `key`.
fn field_after<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let i = s.find(key)? + key.len();
    Some(&s[i..])
}

/// Parse a gateway console line, returning the frame only for **received** (`◄`)
/// messages. TX lines (`►`), relay lines (`⇒`), malformed-frame notices, and boot
/// logs all return `None`. Any surrounding log prefix/ANSI is tolerated — we anchor
/// on the `SPINE ◄` marker and the ` : ` payload delimiter.
pub fn parse_gateway_line(line: &str) -> Option<GatewayFrame> {
    let start = line.find("SPINE ◄")?;
    let rest = &line[start..];

    let src = u8::from_str_radix(
        leading(field_after(rest, "src=")?, |c| c.is_ascii_hexdigit()),
        16,
    )
    .ok()?;
    let seq: u8 = leading(field_after(rest, "seq=")?, |c| c.is_ascii_digit())
        .parse()
        .ok()?;
    let rssi: i32 = leading(field_after(rest, "rssi=")?, |c| c == '-' || c.is_ascii_digit())
        .parse()
        .ok()?;
    // Compact JSON never contains " : " (space-colon-space), so it's a safe split.
    let payload = rest.split_once(" : ").map(|(_, p)| p.trim().to_string())?;
    if payload.is_empty() {
        return None;
    }

    Some(GatewayFrame { src, seq, rssi_dbm: rssi, payload })
}

/// Source tag stamped on every fact this bridge writes.
pub const SOURCE: &str = "lora-gateway";

/// Parse one gateway console line and, if it carries a node message, ingest it into
/// world memory. Writes two facts, both valid *now*:
///
/// - `mesh.<node_id>.<type>` — the node's payload (augmented with a `_mesh`
///   envelope carrying `src`/`seq`/`rssi_dbm`), so per-message-type state is queryable.
/// - `mesh.<node_id>` — a compact liveness/link rollup (`rssi_dbm`, `seq`, `src`,
///   `last_type`), so `current("mesh.<node_id>")` answers "is this node alive, and
///   how strong is the mesh link?".
///
/// Returns a [`GatewayIngest`] summary, or `None` for non-`◄` or non-JSON lines.
pub fn ingest_gateway_line(line: &str, world: &WorldMemory, now_ms: u64) -> Option<GatewayIngest> {
    let frame = parse_gateway_line(line)?;
    let payload: Value = serde_json::from_str(&frame.payload).ok()?;

    let node_id = payload
        .get("node_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("mesh-{:02x}", frame.src));
    let msg_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("status")
        .to_string();

    let mesh_meta = json!({
        "src": format!("{:02X}", frame.src),
        "seq": frame.seq,
        "rssi_dbm": frame.rssi_dbm,
    });

    // Per-type fact: the node payload with a mesh envelope attached.
    let mut enriched = payload.clone();
    if let Value::Object(ref mut m) = enriched {
        m.insert("_mesh".into(), mesh_meta.clone());
    }
    let _ = world.observe(&format!("mesh.{node_id}.{msg_type}"), enriched, now_ms, now_ms, SOURCE);

    // Liveness/link rollup fact.
    let link = json!({
        "rssi_dbm": frame.rssi_dbm,
        "seq": frame.seq,
        "src": format!("{:02X}", frame.src),
        "last_type": msg_type,
    });
    let _ = world.observe(&format!("mesh.{node_id}"), link, now_ms, now_ms, SOURCE);

    Some(GatewayIngest { node_id, msg_type, rssi_dbm: frame.rssi_dbm })
}

// ── Outbound: host → node commands over the mesh (return path) ───────────────────
//
// The inverse direction of the bridge. A command originated on the host travels out
// the base-station Heltec, over LoRa, off the gateway Heltec's UART to the node, and
// into the node's *existing gated command dispatcher* — so a mesh command actuates
// only under the node's on-MCU Track 0 gate, exactly like a wired serial command.

/// A command addressed to a specific node, carried over the mesh. Encodes to the
/// node's own request line (`id`/`cmd`/`args`) plus a `to` routing field the node
/// firmware matches against its id (ignored by the node's request parser itself, so
/// no firmware request-format change is needed).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCommand {
    /// Target node id — the node executes only if this matches its own id.
    pub to: String,
    /// Correlation id; the node echoes it in its response so replies can be matched.
    pub id: String,
    /// The node command, e.g. `"gpio_write"`, `"sensor_read"`, `"capabilities"`.
    pub cmd: String,
    /// Command arguments (any JSON object the node's handler understands).
    pub args: Value,
}

impl NodeCommand {
    /// Build a command for `to`, tagged with correlation `id`.
    pub fn new(
        to: impl Into<String>,
        id: impl Into<String>,
        cmd: impl Into<String>,
        args: Value,
    ) -> Self {
        Self { to: to.into(), id: id.into(), cmd: cmd.into(), args }
    }

    /// Encode to the single newline-free line the gateway will carry over LoRa and
    /// the node will feed to its request dispatcher.
    pub fn encode(&self) -> String {
        json!({ "id": self.id, "to": self.to, "cmd": self.cmd, "args": self.args }).to_string()
    }
}

/// A transport that delivers a [`NodeCommand`] toward the mesh. The serial
/// implementation writes the encoded line to the base-station Heltec's console; tests
/// use an in-memory mock. Lets the `mesh_command` tool stay transport-blind.
#[async_trait]
pub trait CommandSink: Send + Sync {
    /// Deliver `cmd` toward the mesh (the node still gates execution on-MCU).
    async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()>;
}

// ── Serial console reader (real hardware; `--features hardware`) ─────────────────
//
// Opens the base-station Heltec's USB console and drives [`ingest_gateway_line`]
// over every line. Read-only: this side never transmits (the gateway relays and
// keepalives on its own). tokio-serial is feature-gated like the other drivers.
#[cfg(feature = "hardware")]
mod serial {
    use super::{ingest_gateway_line, CommandSink, NodeCommand};
    use crate::memory::world::WorldMemory;
    use anyhow::Context;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
    use tokio::sync::Mutex;
    use tokio_serial::{SerialPortBuilderExt, SerialStream};

    /// Open the base-station Heltec console and split it into (read, write) halves so
    /// the inbound RX ingest loop and the outbound command sink can share the one
    /// exclusive serial port.
    pub fn open_split(
        port: &str,
        baud: u32,
    ) -> anyhow::Result<(ReadHalf<SerialStream>, WriteHalf<SerialStream>)> {
        let serial = tokio_serial::new(port, baud)
            .open_native_async()
            .with_context(|| format!("failed to open LoRa gateway console {port}"))?;
        Ok(tokio::io::split(serial))
    }

    /// RX loop: read newline-framed console lines from the base-station Heltec and
    /// bridge each node message into world memory. Runs until the port closes.
    pub async fn run_gateway_rx<F>(read: ReadHalf<SerialStream>, world: Arc<WorldMemory>, now_ms: F)
    where
        F: Fn() -> u64 + Send,
    {
        let mut lines = BufReader::new(read).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if let Some(ing) = ingest_gateway_line(&line, &world, now_ms()) {
                        tracing::info!(
                            node = %ing.node_id,
                            msg = %ing.msg_type,
                            rssi = ing.rssi_dbm,
                            "LoRa gateway → world memory"
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("LoRa gateway serial RX error: {e}");
                    break;
                }
            }
        }
    }

    /// Outbound [`CommandSink`] over the base-station Heltec's serial write half:
    /// writes each command as a newline-framed line onto the console, which the
    /// station transmits over LoRa.
    pub struct SerialCommandSink {
        writer: Mutex<WriteHalf<SerialStream>>,
    }

    impl SerialCommandSink {
        pub fn new(writer: WriteHalf<SerialStream>) -> Self {
            Self { writer: Mutex::new(writer) }
        }
    }

    #[async_trait::async_trait]
    impl CommandSink for SerialCommandSink {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            let line = cmd.encode();
            let mut w = self.writer.lock().await;
            w.write_all(line.as_bytes()).await?;
            w.write_all(b"\n").await?;
            w.flush().await?;
            Ok(())
        }
    }
}

#[cfg(feature = "hardware")]
pub use serial::{open_split, run_gateway_rx, SerialCommandSink};

#[cfg(test)]
mod tests {
    use super::*;

    const REFLEX_LINE: &str = "SPINE ◄ src=28 seq=30 rssi=-42 dBm : {\"type\":\"reflex\",\"node_id\":\"obc-esp32-s3-001\",\"rule\":\"safe-link-offline\"}";

    #[test]
    fn parses_a_received_frame() {
        let f = parse_gateway_line(REFLEX_LINE).expect("a ◄ line parses");
        assert_eq!(f.src, 0x28);
        assert_eq!(f.seq, 30);
        assert_eq!(f.rssi_dbm, -42);
        assert!(f.payload.starts_with("{\"type\":\"reflex\""));
    }

    #[test]
    fn tolerates_an_esp_log_prefix() {
        let line = "I (34567) heltec_lora_linktest: SPINE ◄ src=A2 seq=7 rssi=-91 dBm : {\"type\":\"gw_keepalive\",\"node_id\":\"gw-A2\"}";
        let f = parse_gateway_line(line).expect("prefix is tolerated");
        assert_eq!(f.src, 0xA2);
        assert_eq!(f.seq, 7);
        assert_eq!(f.rssi_dbm, -91);
    }

    #[test]
    fn ignores_tx_relay_keepalive_and_boot_lines() {
        assert!(parse_gateway_line("SPINE ► (uart) seq=5 (34 B) {\"type\":\"link_state\"}").is_none());
        assert!(parse_gateway_line("SPINE ⇒ relay src=28 seq=30 ttl=1").is_none());
        assert!(parse_gateway_line("SPINE ► (keepalive) seq=3").is_none());
        assert!(parse_gateway_line("SPINE ◄ malformed frame (5 B)").is_none());
        assert!(parse_gateway_line("Gateway A2 — UART1 ⇄ LoRa.").is_none());
    }

    #[test]
    fn ingests_a_reflex_into_world_memory() {
        let world = WorldMemory::open_in_memory().unwrap();
        let ing = ingest_gateway_line(REFLEX_LINE, &world, 1_000).expect("ingested");
        assert_eq!(ing.node_id, "obc-esp32-s3-001");
        assert_eq!(ing.msg_type, "reflex");
        assert_eq!(ing.rssi_dbm, -42);

        // Per-type fact carries the payload + the mesh envelope.
        let f = world
            .current("mesh.obc-esp32-s3-001.reflex")
            .unwrap()
            .expect("per-type fact exists");
        assert_eq!(f.value.get("rule").and_then(|v| v.as_str()), Some("safe-link-offline"));
        assert_eq!(
            f.value.get("_mesh").and_then(|m| m.get("rssi_dbm")).and_then(|v| v.as_i64()),
            Some(-42)
        );
        assert_eq!(f.source, SOURCE);

        // Liveness rollup answers "is the node alive, how strong is the link?".
        let link = world
            .current("mesh.obc-esp32-s3-001")
            .unwrap()
            .expect("rollup fact exists");
        assert_eq!(link.value.get("rssi_dbm").and_then(|v| v.as_i64()), Some(-42));
        assert_eq!(link.value.get("last_type").and_then(|v| v.as_str()), Some("reflex"));
    }

    #[test]
    fn a_frame_without_node_id_falls_back_to_the_src_address() {
        let world = WorldMemory::open_in_memory().unwrap();
        let line = "SPINE ◄ src=0C seq=1 rssi=-10 dBm : {\"type\":\"status\",\"v\":42}";
        let ing = ingest_gateway_line(line, &world, 5).expect("ingested");
        assert_eq!(ing.node_id, "mesh-0c");
        assert!(world.current("mesh.mesh-0c.status").unwrap().is_some());
    }

    #[test]
    fn a_non_json_payload_is_not_ingested() {
        let world = WorldMemory::open_in_memory().unwrap();
        assert!(ingest_gateway_line("SPINE ◄ src=28 seq=1 rssi=-5 dBm : not json", &world, 1).is_none());
    }

    #[test]
    fn a_command_encodes_to_the_node_request_line() {
        let cmd = NodeCommand::new(
            "obc-esp32-s3-001",
            "req-7",
            "gpio_write",
            json!({ "pin": 3, "value": 1 }),
        );
        let line = cmd.encode();
        assert!(!line.contains('\n'), "must be a single line for the mesh");
        let v: Value = serde_json::from_str(&line).unwrap();
        // The node's request parser reads id/cmd/args; `to` is the routing field.
        assert_eq!(v.get("id").and_then(Value::as_str), Some("req-7"));
        assert_eq!(v.get("to").and_then(Value::as_str), Some("obc-esp32-s3-001"));
        assert_eq!(v.get("cmd").and_then(Value::as_str), Some("gpio_write"));
        assert_eq!(v.get("args").and_then(|a| a.get("pin")).and_then(Value::as_i64), Some(3));
    }

    /// In-memory sink that records every command it's asked to send (for tests).
    struct MockSink {
        sent: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait]
    impl CommandSink for MockSink {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(cmd.encode());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_sink_forwards_the_encoded_command() {
        let sink = MockSink { sent: std::sync::Mutex::new(Vec::new()) };
        let cmd = NodeCommand::new("node-a", "id-1", "sensor_read", json!({ "kind": "dht22" }));
        sink.send_command(&cmd).await.unwrap();
        let sent = sink.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], cmd.encode());
    }
}

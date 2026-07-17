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
    // Real console lines end with an ANSI color-reset (`\x1b[0m`) AFTER the
    // payload — trailing escape bytes break serde_json, so cut at the first ESC
    // (bench-caught 2026-07-17: every frame silently failed to ingest).
    let payload = rest.split_once(" : ").map(|(_, p)| {
        p.split('\u{1b}').next().unwrap_or("").trim().to_string()
    })?;
    if payload.is_empty() {
        return None;
    }

    Some(GatewayFrame { src, seq, rssi_dbm: rssi, payload })
}

/// A decoded ClawCam field summary — the compact camera-on-mesh payload (see the ClawCam
/// `mesh.field_summary` codec). Rides the same spine as node JSON but in a pipe-delimited
/// wire form (`CC|dev=…|det=…|sp=…|tc=…|bat=…`) small enough for a LoRa frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ClawCamSummary {
    pub device_id: String,
    pub ts: Option<i64>,
    pub total: u32,
    pub species: Vec<(String, u32)>,
    pub temperature_c: Option<f64>,
    pub battery_percent: Option<f64>,
    /// The node's own reported uplink RSSI (distinct from the gateway link RSSI).
    pub rssi: Option<f64>,
}

/// Parse a ClawCam field-summary payload (`CC|…`). Returns `None` unless the magic prefix
/// is present and a `dev=` field is found. Inverse of the ClawCam `encode_summary`.
pub fn parse_clawcam_summary(payload: &str) -> Option<ClawCamSummary> {
    let mut it = payload.trim().split('|');
    if it.next() != Some("CC") {
        return None;
    }
    let mut s = ClawCamSummary {
        device_id: String::new(), ts: None, total: 0, species: Vec::new(),
        temperature_c: None, battery_percent: None, rssi: None,
    };
    let mut have_dev = false;
    for kv in it {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "dev" => { s.device_id = v.to_string(); have_dev = true; }
            "ts" => s.ts = v.parse().ok(),
            "det" => s.total = v.parse().unwrap_or(0),
            "sp" => {
                for item in v.split(',') {
                    if let Some((name, cnt)) = item.rsplit_once(':') {
                        if let Ok(c) = cnt.parse::<u32>() {
                            s.species.push((name.to_string(), c));
                        }
                    }
                }
            }
            "tc" => s.temperature_c = v.parse().ok(),
            "bat" => s.battery_percent = v.parse().ok(),
            "rssi" => s.rssi = v.parse().ok(),
            _ => {}
        }
    }
    if have_dev {
        Some(s)
    } else {
        None
    }
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
    // Node JSON (`{"type":…}`) is the common case; a ClawCam `CC|…` field summary is the
    // camera-on-mesh case (G2). Anything else is ignored.
    if let Ok(payload) = serde_json::from_str::<Value>(&frame.payload) {
        return Some(ingest_node_json(&frame, payload, world, now_ms));
    }
    if let Some(summary) = parse_clawcam_summary(&frame.payload) {
        return Some(ingest_clawcam_summary(&frame, summary, world, now_ms));
    }
    None
}

/// Ingest a node's own JSON payload as `mesh.<node_id>.<type>` + a `mesh.<node_id>` rollup.
fn ingest_node_json(
    frame: &GatewayFrame, payload: Value, world: &WorldMemory, now_ms: u64,
) -> GatewayIngest {
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

    GatewayIngest { node_id, msg_type, rssi_dbm: frame.rssi_dbm }
}

/// Ingest a ClawCam field summary heard over the mesh (G2) as `clawcam.<device>.field`
/// (the full summary + mesh envelope) plus a compact `clawcam.<device>` rollup — so an
/// off-grid camera's counts/conditions land in the brain's world model.
fn ingest_clawcam_summary(
    frame: &GatewayFrame, s: ClawCamSummary, world: &WorldMemory, now_ms: u64,
) -> GatewayIngest {
    let dev = s.device_id.clone();
    let species: Vec<Value> = s
        .species
        .iter()
        .map(|(name, count)| json!({ "subject": name, "count": count }))
        .collect();
    let top = species.first().cloned();

    let field = json!({
        "device_id": dev,
        "total": s.total,
        "species": species,
        "temperature_c": s.temperature_c,
        "battery_percent": s.battery_percent,
        "node_rssi": s.rssi,
        "ts": s.ts,
        "_mesh": {
            "src": format!("{:02X}", frame.src),
            "seq": frame.seq,
            "rssi_dbm": frame.rssi_dbm,
        },
    });
    let _ = world.observe(&format!("clawcam.{dev}.field"), field, now_ms, now_ms, SOURCE);

    let rollup = json!({
        "total": s.total,
        "top_species": top,
        "temperature_c": s.temperature_c,
        "battery_percent": s.battery_percent,
        "rssi_dbm": frame.rssi_dbm,
    });
    let _ = world.observe(&format!("clawcam.{dev}"), rollup, now_ms, now_ms, SOURCE);

    GatewayIngest { node_id: dev, msg_type: "clawcam_field".to_string(), rssi_dbm: frame.rssi_dbm }
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
// over every line.
//
// Implementation note: this deliberately uses the BLOCKING `serialport` crate on
// a dedicated reader thread feeding a tokio channel — NOT tokio-serial. On
// Windows, tokio-serial/mio-serial reads were observed to pend forever with no
// bytes and no errors (bench, 2026-07-17), while a blocking reader on the same
// port streamed happily. Boring beats async here.
#[cfg(feature = "hardware")]
mod serial {
    use super::{ingest_gateway_line, CommandSink, NodeCommand};
    use crate::memory::world::WorldMemory;
    use anyhow::Context;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Shared write handle to the base-station console.
    pub type SharedSerialWriter = Arc<Mutex<Box<dyn serialport::SerialPort>>>;

    /// Open the base-station Heltec console. Returns a channel of console lines
    /// (fed by a dedicated blocking reader thread) and a shared write handle for
    /// the outbound command sink.
    pub fn open_split(
        port: &str,
        baud: u32,
    ) -> anyhow::Result<(mpsc::Receiver<String>, SharedSerialWriter)> {
        let mut serial = serialport::new(port, baud)
            .timeout(Duration::from_millis(250))
            .open()
            .with_context(|| format!("failed to open LoRa gateway console {port}"))?;
        // ESP32 dev boards wire DTR/RTS to the auto-download circuit (EN/IO0).
        // Wrong line states can HOLD THE BOARD IN RESET or — if the open glitches
        // a reset while DTR=1/RTS=0 — strap it into DOWNLOAD MODE (dark, silent).
        // Bench-swept on a Heltec V3 (CP2102), 2026-07-17:
        //   steady DTR=0 RTS=1 → board HELD IN RESET
        //   DTR=1/RTS=0 during a reset edge → download mode
        //   both LOW → straps read high → clean boot, safe steady state
        // So: drive both low immediately and hold; give a possibly-reset board a
        // boot window before reading.
        match serial.write_request_to_send(false) {
            Ok(()) => tracing::info!("[lora_gateway] RTS deasserted"),
            Err(e) => tracing::warn!("[lora_gateway] RTS deassert FAILED: {e}"),
        }
        match serial.write_data_terminal_ready(false) {
            Ok(()) => tracing::info!("[lora_gateway] DTR deasserted"),
            Err(e) => tracing::warn!("[lora_gateway] DTR deassert FAILED: {e}"),
        }
        std::thread::sleep(Duration::from_millis(1500));
        tracing::info!("[lora_gateway] boot window elapsed; lines held low (run state)");

        let writer: SharedSerialWriter = Arc::new(Mutex::new(
            serial
                .try_clone()
                .context("failed to clone serial handle for the command sink")?,
        ));

        // Dedicated blocking reader thread: accumulate bytes into newline-framed
        // lines and push them over the channel. Read timeouts are the idle path.
        let (tx, rx) = mpsc::channel::<String>(256);
        std::thread::Builder::new()
            .name("lora-gateway-rx".into())
            .spawn(move || {
                let mut buf = [0u8; 512];
                let mut line: Vec<u8> = Vec::with_capacity(256);
                loop {
                    match serial.read(&mut buf) {
                        Ok(0) => {}
                        Ok(n) => {
                            for &b in &buf[..n] {
                                if b == b'\n' || b == b'\r' {
                                    if !line.is_empty() {
                                        let s = String::from_utf8_lossy(&line).into_owned();
                                        line.clear();
                                        if tx.blocking_send(s).is_err() {
                                            return; // receiver dropped — shut down
                                        }
                                    }
                                } else {
                                    line.push(b);
                                    if line.len() > 4096 {
                                        line.clear(); // runaway line — discard
                                    }
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            tracing::warn!("[lora_gateway] serial read error: {e}");
                            return;
                        }
                    }
                }
            })
            .context("failed to spawn LoRa gateway reader thread")?;

        Ok((rx, writer))
    }

    /// RX loop: take console lines off the reader-thread channel and bridge each
    /// node message into world memory. Runs until the reader thread exits.
    pub async fn run_gateway_rx<F>(
        mut lines: mpsc::Receiver<String>,
        world: Arc<WorldMemory>,
        now_ms: F,
    ) where
        F: Fn() -> u64 + Send,
    {
        while let Some(line) = lines.recv().await {
            // Raw-line visibility: silence must never again be ambiguous between
            // "no bytes" and "bytes that don't parse" (bench lesson, 2026-07-17).
            // Debug level — enable with RUST_LOG=debug when diagnosing.
            tracing::debug!(
                "[lora_gateway] raw: {}",
                line.chars().take(110).collect::<String>()
            );
            if let Some(ing) = ingest_gateway_line(&line, &world, now_ms()) {
                tracing::info!(
                    node = %ing.node_id,
                    msg = %ing.msg_type,
                    rssi = ing.rssi_dbm,
                    "LoRa gateway → world memory"
                );
            }
        }
    }

    /// Outbound [`CommandSink`] over the base-station Heltec's console: writes each
    /// command as a newline-framed line, which the station transmits over LoRa.
    pub struct SerialCommandSink {
        writer: SharedSerialWriter,
    }

    impl SerialCommandSink {
        pub fn new(writer: SharedSerialWriter) -> Self {
            Self { writer }
        }
    }

    #[async_trait::async_trait]
    impl CommandSink for SerialCommandSink {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            let line = cmd.encode();
            let writer = Arc::clone(&self.writer);
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let mut w = writer.lock().expect("serial writer lock poisoned");
                w.write_all(line.as_bytes())?;
                w.write_all(b"\n")?;
                w.flush()?;
                Ok(())
            })
            .await??;
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

    /// Regression: real ESP-IDF console lines carry ANSI color codes AROUND the
    /// whole line — including a `\x1b[0m` reset AFTER the JSON payload. That
    /// trailing escape made serde_json reject every payload, so frames parsed
    /// but never ingested (bench, 2026-07-17). Byte-for-byte bench line:
    #[test]
    fn strips_trailing_ansi_from_the_payload() {
        let line = "\u{1b}[0;32mI (93772) heltec_lora_linktest: SPINE ◄ src=D8 seq=11 rssi=-10 dBm : {\"node_id\":\"gw-D8\",\"type\":\"gw_keepalive\",\"seq\":11}\u{1b}[0m";
        let f = parse_gateway_line(line).expect("ANSI-wrapped line parses");
        assert_eq!(f.src, 0xD8);
        assert_eq!(f.seq, 11);
        assert_eq!(f.rssi_dbm, -10);
        // The payload must be CLEAN JSON — no escape bytes.
        assert_eq!(
            f.payload,
            "{\"node_id\":\"gw-D8\",\"type\":\"gw_keepalive\",\"seq\":11}"
        );
        serde_json::from_str::<serde_json::Value>(&f.payload)
            .expect("payload is valid JSON after ANSI strip");
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

    // ── ClawCam camera-on-mesh field summaries (G2) ──────────────────────────────

    const CC_LINE: &str = "SPINE ◄ src=1A seq=5 rssi=-95 dBm : CC|dev=north-ridge-01|ts=1720000000|det=40|sp=deer:20,fox:12,turkey:8|tc=14.5|bat=78|rssi=-97";

    #[test]
    fn parses_a_clawcam_summary_payload() {
        let s = parse_clawcam_summary(
            "CC|dev=n1|ts=1000|det=32|sp=deer:20,fox:12|tc=14.5|bat=78|rssi=-97",
        )
        .expect("CC payload parses");
        assert_eq!(s.device_id, "n1");
        assert_eq!(s.total, 32);
        assert_eq!(s.species, vec![("deer".into(), 20), ("fox".into(), 12)]);
        assert_eq!(s.temperature_c, Some(14.5));
        assert_eq!(s.battery_percent, Some(78.0));
        assert_eq!(s.rssi, Some(-97.0));
    }

    #[test]
    fn non_cc_and_missing_dev_do_not_parse() {
        assert!(parse_clawcam_summary("XX|dev=n1|det=1").is_none());
        assert!(parse_clawcam_summary("CC|det=1|tc=5").is_none()); // no dev
    }

    #[test]
    fn ingests_a_clawcam_summary_into_world_memory() {
        let world = WorldMemory::open_in_memory().unwrap();
        let ing = ingest_gateway_line(CC_LINE, &world, 2_000).expect("ingested");
        assert_eq!(ing.node_id, "north-ridge-01");
        assert_eq!(ing.msg_type, "clawcam_field");
        assert_eq!(ing.rssi_dbm, -95); // gateway link rssi, not the node's own -97

        // Full field fact carries totals, species, conditions + mesh envelope.
        let f = world
            .current("clawcam.north-ridge-01.field")
            .unwrap()
            .expect("field fact exists");
        assert_eq!(f.value.get("total").and_then(|v| v.as_u64()), Some(40));
        assert_eq!(f.value.get("temperature_c").and_then(|v| v.as_f64()), Some(14.5));
        assert_eq!(f.value.get("node_rssi").and_then(|v| v.as_f64()), Some(-97.0));
        assert_eq!(
            f.value.get("_mesh").and_then(|m| m.get("rssi_dbm")).and_then(|v| v.as_i64()),
            Some(-95)
        );
        let sp = f.value.get("species").and_then(|v| v.as_array()).unwrap();
        assert_eq!(sp[0].get("subject").and_then(|v| v.as_str()), Some("deer"));
        assert_eq!(sp[0].get("count").and_then(|v| v.as_u64()), Some(20));

        // Rollup answers "how much activity, and is the camera OK?".
        let r = world.current("clawcam.north-ridge-01").unwrap().expect("rollup");
        assert_eq!(r.value.get("total").and_then(|v| v.as_u64()), Some(40));
        assert_eq!(
            r.value.get("top_species").and_then(|t| t.get("subject")).and_then(|v| v.as_str()),
            Some("deer")
        );
    }
}

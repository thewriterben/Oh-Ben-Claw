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
//! Since 2026-09-13 every frame on the air is authenticated (SPINE-AUTH.md
//! step 4) and the station prints the frame's `ctr=` and `mac=` on the line:
//!
//! ```text
//! SPINE ◄ src=40 seq=67 ctr=835 mac=1f0e…c3 rssi=-50 dBm snr=12 dB : {"type":…}
//! ```
//!
//! [`LoraAuth`] verifies that tag *again* on the host, under the same root
//! the stations hold, and judges the counter against a per-station window
//! persisted in world memory — so the host trusts the station's radio, not
//! its console: a replaced or replayed base station cannot put a frame into
//! world memory that a station's key did not sign. A line without a tag is
//! refused, not tolerated; there is no unverified ingest path from the
//! serial loop.
//!
//! It is deliberately the *inverse* of [`super::lora_mesh`]: that module speaks OBC's
//! compact fleet codec (`{"t":"hb"}` heartbeats / `{"t":"as"}` assignments); this one
//! ingests the node's own autonomous JSON (`{"type":…,"node_id":…}`) as reported by
//! the gateway. The parsing + ingest core is hardware-free and unit-tested; only the
//! serial read loop is gated behind the `hardware` feature (tokio-serial), matching
//! the rest of the peripheral I/O.

use async_trait::async_trait;
use obc_memory::world::{Origin, WorldMemory};
use serde_json::{json, Value};
use std::sync::Arc;

/// One received frame as reported by a gateway `SPINE ◄` console line.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayFrame {
    /// Originating node id (low byte of its MAC), from `src=` (hex).
    pub src: u8,
    /// Per-source sequence, from `seq=`.
    pub seq: u8,
    /// The frame counter the tag covers, from `ctr=`. `None` on a line from
    /// a station running firmware older than step 4 — which [`LoraAuth`]
    /// refuses.
    pub ctr: Option<u32>,
    /// The frame's tag, from `mac=` (sixteen hex digits). `None` as above.
    pub mac: Option<[u8; obc_safety::spine_tag::TAG_LEN]>,
    /// Received signal strength in dBm, from `rssi=`.
    pub rssi_dbm: i32,
    /// The node payload after the ` : ` delimiter, trimmed (expected to be JSON).
    pub payload: String,
    /// The payload exactly as the station printed it — untrimmed — which is
    /// what the tag was computed over. Verification uses this, not `payload`.
    pub signed: String,
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
    // A refusal line starts with the same marker and describes a frame the
    // station threw away. It is excluded here explicitly rather than by
    // accident: today it lacks `seq=` and would fail below anyway, and that
    // is exactly the kind of incidental safety that stops being true the day
    // someone adds a field to the firmware's warning. See `parse_gateway_refusal`.
    if rest.starts_with(REJECTED_MARKER) {
        return None;
    }

    let src = u8::from_str_radix(
        leading(field_after(rest, "src=")?, |c| c.is_ascii_hexdigit()),
        16,
    )
    .ok()?;
    let seq: u8 = leading(field_after(rest, "seq=")?, |c| c.is_ascii_digit())
        .parse()
        .ok()?;
    let rssi: i32 = leading(field_after(rest, "rssi=")?, |c| {
        c == '-' || c.is_ascii_digit()
    })
    .parse()
    .ok()?;
    // Compact JSON never contains " : " (space-colon-space), so it's a safe split.
    // Real console lines end with an ANSI color-reset (`\x1b[0m`) AFTER the
    // payload — trailing escape bytes break serde_json, so cut at the first ESC
    // (bench-caught 2026-07-17: every frame silently failed to ingest).
    let (header, signed) = rest.split_once(" : ")?;
    let signed = signed.split('\u{1b}').next().unwrap_or("").to_string();
    let payload = signed.trim().to_string();
    if payload.is_empty() {
        return None;
    }
    // The authentication fields live in the header, before the payload, so a
    // payload that happens to contain "ctr=" cannot supply them.
    let ctr: Option<u32> =
        field_after(header, "ctr=").and_then(|s| leading(s, |c| c.is_ascii_digit()).parse().ok());
    let mac = field_after(header, "mac=").and_then(|s| {
        let hex = leading(s, |c| c.is_ascii_hexdigit());
        if hex.len() != 2 * obc_safety::spine_tag::TAG_LEN {
            return None;
        }
        let mut out = [0u8; obc_safety::spine_tag::TAG_LEN];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(out)
    });

    Some(GatewayFrame {
        src,
        seq,
        ctr,
        mac,
        rssi_dbm: rssi,
        payload,
        signed,
    })
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
        device_id: String::new(),
        ts: None,
        total: 0,
        species: Vec::new(),
        temperature_c: None,
        battery_percent: None,
        rssi: None,
    };
    let mut have_dev = false;
    for kv in it {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k {
            "dev" => {
                s.device_id = v.to_string();
                have_dev = true;
            }
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
/// world memory **without verifying it**. The serial loop does not call this;
/// it goes through [`LoraAuth::admit`] first and then [`ingest_frame`]. This
/// exists for the parse-and-ingest tests and the e2e harness, which exercise
/// what a verified frame becomes in world memory, not whether it was verified.
///
/// Returns a [`GatewayIngest`] summary, or `None` for non-`◄` or non-JSON lines.
pub fn ingest_gateway_line(line: &str, world: &WorldMemory, now_ms: u64) -> Option<GatewayIngest> {
    let frame = parse_gateway_line(line)?;
    ingest_frame(&frame, world, now_ms)
}

/// Ingest a parsed (and, on the serial path, verified) frame into world
/// memory. Writes two facts, both valid *now*:
///
/// - `mesh.<node_id>.<type>` — the node's payload (augmented with a `_mesh`
///   envelope carrying `src`/`seq`/`rssi_dbm`), so per-message-type state is queryable.
/// - `mesh.<node_id>` — a compact liveness/link rollup (`rssi_dbm`, `seq`, `src`,
///   `last_type`), so `current("mesh.<node_id>")` answers "is this node alive, and
///   how strong is the mesh link?".
///
/// Returns `None` for a payload that is neither node JSON nor a ClawCam summary.
pub fn ingest_frame(
    frame: &GatewayFrame,
    world: &WorldMemory,
    now_ms: u64,
) -> Option<GatewayIngest> {
    // Node JSON (`{"type":…}`) is the common case; a ClawCam `CC|…` field summary is the
    // camera-on-mesh case (G2). Anything else is ignored.
    if let Ok(payload) = serde_json::from_str::<Value>(&frame.payload) {
        return Some(ingest_node_json(frame, payload, world, now_ms));
    }
    if let Some(summary) = parse_clawcam_summary(&frame.payload) {
        return Some(ingest_clawcam_summary(frame, summary, world, now_ms));
    }
    None
}

// ── Host-side verification of what the base station heard ───────────────────
//
// SPINE-AUTH.md §3.4, the last bullet. The stations verify every frame on the
// air (step 4); until this existed the host took the base station's console
// at its word, so the trust boundary sat at a USB cable. Now the host verifies
// the same tag under the same root, and the base is a transcriber the host can
// check rather than an oracle it has to believe.

/// Why [`LoraAuth`] refused a line. Each is a distinct operator message and a
/// distinct field in the `spine.auth.<station>` fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraRefused {
    /// The line carries no `ctr=`/`mac=`: a station running firmware older
    /// than step 4, or a console that is not a station's. Refused rather
    /// than tolerated — see SPINE-AUTH.md §4 on permissive modes.
    Unsigned,
    /// The tag does not verify under the key derived for `src`: a different
    /// root, a forgery, or a console line altered in transit.
    BadTag,
    /// The counter was already accepted: a replay of a line the host has
    /// seen — which the station's own window would have caught on the air,
    /// so on this path it means the console, not the radio.
    Replayed,
    /// Older than the window can judge.
    TooOld,
}

impl LoraRefused {
    pub fn as_str(self) -> &'static str {
        match self {
            LoraRefused::Unsigned => "unsigned (no ctr=/mac= on the line: pre-step-4 firmware?)",
            LoraRefused::BadTag => "bad tag (wrong root, forged, or altered)",
            LoraRefused::Replayed => "counter already accepted (replayed)",
            LoraRefused::TooOld => "counter older than the receive window",
        }
    }
}

/// Prefix of the per-station facts this verifier writes.
pub const AUTH_FACT_PREFIX: &str = "spine.auth.";

/// How many stations are currently alarmed — the one number the standard
/// safing rule `safe-spine-forgery` watches, the way `mesh.escalated_count`
/// drives `safe-mesh-node-lost`. Written on change only.
pub const AUTH_ALARM_COUNT_FACT: &str = "spine.auth.alarm_count";

/// A station's alarm clears itself this long after its last `BadTag` or
/// `Replayed`, so a burst reads as one incident with a start and an end.
pub const AUTH_ALARM_CLEAR_MS: u64 = 10 * 60 * 1000;

impl LoraRefused {
    /// Whether this refusal is an incident (DECISIONS.md 2026-09-14). A bad
    /// tag that reaches the host is never corruption — the radio drops CRC
    /// failures — so it is a wrong root or a forgery; a replayed counter is
    /// a replay. `TooOld` is the bounded post-reset gap the design chose,
    /// and `Unsigned` is old firmware: both stay on the auth fact and alarm
    /// nothing.
    pub fn is_incident(self) -> bool {
        matches!(self, LoraRefused::BadTag | LoraRefused::Replayed)
    }
}

/// A station's open alarm: when the burst began, how many incidents it has
/// held, and when the last one was (the clear timer runs from there).
#[derive(Debug, Clone)]
struct AuthAlarm {
    since_ms: u64,
    last_ms: u64,
    count: u64,
    /// Row id of the `spine.auth.<station>.alarm` fact, the support for the count.
    fact_id: Option<i64>,
}

/// Host-side authentication of station frames: the tag under the deployment
/// root, and an anti-replay window per station persisted in world memory
/// with `M = 1` (`SPINE-REPLAY.md` §3: the host has SQLite and no wear
/// problem, so it persists every accept and loses nothing on restart).
///
/// The fact `spine.auth.gw-XX` holds `{ctr, accepted, rejected, last_rejected}`:
/// `ctr` is the persisted high-water mark the window resumes from, and the
/// rest is what an operator (or a reflex) needs to notice a station that is
/// sending frames the host will not take.
pub struct LoraAuth {
    root: Vec<u8>,
    keys: std::collections::HashMap<u8, [u8; 32]>,
    window: obc_safety::replay::ReplayWindow,
    /// Stations whose window has been resumed from world memory.
    resumed: std::collections::HashSet<u8>,
    tally: std::collections::HashMap<u8, Tally>,
    /// Stations with an open alarm (`BadTag`/`Replayed` within
    /// [`AUTH_ALARM_CLEAR_MS`]).
    alarms: std::collections::HashMap<u8, AuthAlarm>,
}

/// What the `spine.auth.<station>` fact carries besides the counter.
#[derive(Debug, Clone, Default)]
struct Tally {
    accepted: u64,
    rejected: u64,
    last_rejected: Option<Value>,
}

impl LoraAuth {
    /// Shortest root accepted — the stations enforce the same at build time.
    pub const MIN_ROOT_LEN: usize = 32;

    /// From the deployment root as a string (the same bytes the stations
    /// were built with in `OBC_SPINE_ROOT`).
    pub fn new(root: &str) -> anyhow::Result<Self> {
        let root = root.trim();
        anyhow::ensure!(
            root.len() >= Self::MIN_ROOT_LEN,
            "spine root is {} characters; the stations require at least {} \
             (`openssl rand -hex 32`)",
            root.len(),
            Self::MIN_ROOT_LEN
        );
        Ok(Self {
            root: root.as_bytes().to_vec(),
            keys: Default::default(),
            window: obc_safety::replay::ReplayWindow::new(),
            resumed: Default::default(),
            tally: Default::default(),
            alarms: Default::default(),
        })
    }

    /// From `[lora_gateway]`: `spine_root_file` (preferred — the secret stays
    /// out of the config) or `spine_root` inline. Exactly one must be set;
    /// without a root there is no verifying, and no unverified path.
    pub fn from_config(inline: Option<&str>, file: Option<&str>) -> anyhow::Result<Self> {
        match (inline, file) {
            (Some(_), Some(_)) => {
                anyhow::bail!("[lora_gateway] set spine_root or spine_root_file, not both")
            }
            (Some(root), None) => Self::new(root),
            (None, Some(path)) => {
                let expanded = expand_home(path);
                let root = std::fs::read_to_string(&expanded).map_err(|e| {
                    anyhow::anyhow!("[lora_gateway] spine_root_file {expanded}: {e}")
                })?;
                Self::new(&root)
            }
            (None, None) => anyhow::bail!(
                "[lora_gateway] spine_root_file (or spine_root) is required: every station \
                 frame has been authenticated since 2026-09-13 (SPINE-AUTH.md step 4) and \
                 the host verifies each one under the same root the stations were built \
                 with (OBC_SPINE_ROOT). There is no unverified ingest path."
            ),
        }
    }

    /// Two bytes of SHA-256 over the root — the same fingerprint the stations
    /// print at boot, so a mismatch is visible before the first rejection.
    pub fn fingerprint(&self) -> u16 {
        obc_safety::spine_tag::root_fingerprint(&self.root)
    }

    /// The station id a `src` byte denotes, as the stations name themselves.
    pub fn station(src: u8) -> String {
        format!("gw-{src:02X}")
    }

    fn key_for(&mut self, src: u8) -> [u8; 32] {
        let root = &self.root;
        *self
            .keys
            .entry(src)
            .or_insert_with(|| obc_safety::spine_tag::derive_node_key(root, &Self::station(src)))
    }

    /// Verify `frame` and judge its counter, persisting the window's
    /// high-water mark and the outcome to world memory. `Ok` means the
    /// payload may be ingested; `Err` says why it must not be, and the same
    /// reason is on the `spine.auth.<station>` fact.
    pub fn admit(
        &mut self,
        frame: &GatewayFrame,
        world: &WorldMemory,
        now_ms: u64,
    ) -> Result<(), LoraRefused> {
        let station = Self::station(frame.src);
        let key_name = format!("{AUTH_FACT_PREFIX}{station}");
        if !self.resumed.contains(&frame.src) {
            // First frame from this station since the host started: resume
            // its window from the persisted mark, or start fresh.
            if let Ok(Some(fact)) = world.current(&key_name) {
                if let Some(h) = fact.value.get("ctr").and_then(Value::as_u64) {
                    self.window.resume(&station, h as u32);
                }
                let v = &fact.value;
                self.tally.insert(
                    frame.src,
                    Tally {
                        accepted: v.get("accepted").and_then(Value::as_u64).unwrap_or(0),
                        rejected: v.get("rejected").and_then(Value::as_u64).unwrap_or(0),
                        last_rejected: v.get("last_rejected").filter(|r| !r.is_null()).cloned(),
                    },
                );
            }
            // An alarm left open by the previous process is still open: its
            // clear timer runs from when it was last written.
            if let Ok(Some(alarm)) = world.current(&format!("{key_name}.alarm")) {
                if alarm.value.get("status").and_then(Value::as_str) == Some("alarmed") {
                    self.alarms.insert(
                        frame.src,
                        AuthAlarm {
                            since_ms: alarm
                                .value
                                .get("since_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or(alarm.valid_from),
                            last_ms: alarm.valid_from,
                            count: alarm
                                .value
                                .get("count")
                                .and_then(Value::as_u64)
                                .unwrap_or(1),
                            fact_id: Some(alarm.id),
                        },
                    );
                }
            }
            self.resumed.insert(frame.src);
        }

        let verdict = match (frame.ctr, frame.mac) {
            (Some(ctr), Some(mac)) => {
                let key = self.key_for(frame.src);
                if !obc_safety::spine_tag::verify(
                    &key,
                    frame.src,
                    ctr,
                    frame.signed.as_bytes(),
                    &mac,
                ) {
                    Err(LoraRefused::BadTag)
                } else {
                    // Tag first, counter second: an unsigned counter must not
                    // be able to move the window.
                    use obc_safety::replay::ReplayVerdict;
                    match self.window.admit(&station, ctr) {
                        ReplayVerdict::Fresh => Ok(()),
                        ReplayVerdict::Duplicate => Err(LoraRefused::Replayed),
                        ReplayVerdict::TooOld => Err(LoraRefused::TooOld),
                    }
                }
            }
            _ => Err(LoraRefused::Unsigned),
        };

        let tally = self.tally.entry(frame.src).or_default();
        match verdict {
            Ok(()) => tally.accepted += 1,
            Err(why) => {
                tally.rejected += 1;
                tally.last_rejected = Some(json!({
                    "ctr": frame.ctr,
                    "reason": why.as_str(),
                    "at_ms": now_ms,
                    "rssi_dbm": frame.rssi_dbm,
                }));
            }
        }
        // Written on every frame, accepted or not: `ctr` is the persisted
        // high-water mark (M = 1), the rest is the operator's view.
        let fact = json!({
            "station": station,
            "ctr": self.window.highest(&station),
            "accepted": tally.accepted,
            "rejected": tally.rejected,
            "last_rejected": tally.last_rejected,
        });
        let auth_fact_id = world
            .observe_as(&key_name, fact, now_ms, now_ms, SOURCE, Origin::Observed)
            .ok()
            .map(|f| f.id);

        // An incident opens the station's alarm, or extends the one open.
        // The fact is written once per burst — the count travels on the
        // clear — so a flood of forged frames is one incident, not a flood
        // of facts (DECISIONS.md 2026-09-14).
        if let Err(why) = verdict {
            if why.is_incident() {
                match self.alarms.get_mut(&frame.src) {
                    Some(a) => {
                        a.count += 1;
                        a.last_ms = now_ms;
                    }
                    None => {
                        let fact = world
                            .observe_derived_from(
                                &format!("{key_name}.alarm"),
                                json!({
                                    "status": "alarmed",
                                    "station": station,
                                    "reason": why.as_str(),
                                    "ctr": frame.ctr,
                                    "rssi_dbm": frame.rssi_dbm,
                                    "count": 1,
                                    "since_ms": now_ms,
                                }),
                                now_ms,
                                now_ms,
                                SOURCE,
                                &auth_fact_id.into_iter().collect::<Vec<_>>(),
                            )
                            .ok();
                        tracing::warn!(
                            station = %station,
                            ctr = frame.ctr,
                            rssi = frame.rssi_dbm,
                            "[lora_gateway] ALARM: {} — a station is sending frames the host refuses; escalating",
                            why.as_str()
                        );
                        self.alarms.insert(
                            frame.src,
                            AuthAlarm {
                                since_ms: now_ms,
                                last_ms: now_ms,
                                count: 1,
                                fact_id: fact.map(|f| f.id),
                            },
                        );
                        self.record_alarm_count(world, now_ms);
                    }
                }
            }
        }
        self.sweep_alarms(world, now_ms);
        verdict
    }

    /// Close every alarm whose last incident is older than
    /// [`AUTH_ALARM_CLEAR_MS`]. Runs on every frame from any station, which
    /// is the only clock the verifier has; a link with no traffic at all
    /// keeps its alarms, and has nothing to judge anyway.
    fn sweep_alarms(&mut self, world: &WorldMemory, now_ms: u64) {
        let expired: Vec<u8> = self
            .alarms
            .iter()
            .filter(|(_, a)| now_ms.saturating_sub(a.last_ms) >= AUTH_ALARM_CLEAR_MS)
            .map(|(src, _)| *src)
            .collect();
        if expired.is_empty() {
            return;
        }
        for src in expired {
            let Some(a) = self.alarms.remove(&src) else {
                continue;
            };
            let station = Self::station(src);
            let _ = world.observe_derived_from(
                &format!("{AUTH_FACT_PREFIX}{station}.alarm"),
                json!({
                    "status": "cleared",
                    "station": station,
                    "count": a.count,
                    "since_ms": a.since_ms,
                    "last_ms": a.last_ms,
                    "until_ms": now_ms,
                }),
                now_ms,
                now_ms,
                SOURCE,
                &a.fact_id.into_iter().collect::<Vec<_>>(),
            );
            tracing::info!(
                station = %station,
                incidents = a.count,
                "[lora_gateway] alarm cleared: no refused frame for {} s",
                AUTH_ALARM_CLEAR_MS / 1000
            );
        }
        self.record_alarm_count(world, now_ms);
    }

    /// `spine.auth.alarm_count` — how many stations are alarmed — derived
    /// from the open alarm facts. The reflex engine reads this one number.
    fn record_alarm_count(&self, world: &WorldMemory, now_ms: u64) {
        let support: Vec<i64> = self.alarms.values().filter_map(|a| a.fact_id).collect();
        let _ = world.observe_derived_from(
            AUTH_ALARM_COUNT_FACT,
            json!(self.alarms.len() as u64),
            now_ms,
            now_ms,
            SOURCE,
            &support,
        );
    }

    /// Stations currently alarmed, for `mesh_status` and `status`.
    pub fn alarmed_stations(world: &WorldMemory) -> Vec<Value> {
        let mut out = Vec::new();
        for e in world.entities().unwrap_or_default() {
            if !(e.starts_with(AUTH_FACT_PREFIX) && e.ends_with(".alarm")) {
                continue;
            }
            if let Ok(Some(f)) = world.current(&e) {
                if f.value.get("status").and_then(Value::as_str) == Some("alarmed") {
                    out.push(f.value.clone());
                }
            }
        }
        out
    }
}

// ── What the station refused on the air ─────────────────────────────────────
//
// DECISIONS.md 2026-09-15. Everything above this line is the host's own
// cryptographic judgement of a frame the station *accepted*, and it is only
// reachable by a station that disagrees with the host about the root — a
// replaced or mis-provisioned base. A stranger transmitting forged frames at
// an honest station never reaches it: the station refuses at the radio and
// forwards nothing (`heltec-lora-linktest/src/main.rs`: "Nothing unverified
// reaches the UART, the log line the host parses, or the relay").
//
// What the station *does* do is print one console line per refusal, on the
// same wire the host is already reading. This section reads those lines. The
// resulting signal is deliberately weaker than the one above and is kept in
// its own type for that reason: it is asserted by the station over an
// unauthenticated console, so anyone who can write to that serial line can
// fabricate it. It advises; it must never safe the mesh, and it never touches
// `spine.auth.<station>`, whose meaning stays "the host refused a frame".

/// The station's refusal line marker. The accepted-frame marker is a prefix
/// of this one, so [`parse_gateway_line`] checks for it and bails.
const REJECTED_MARKER: &str = "SPINE ◄ REJECTED";

/// The station's wording for a tag that did not verify —
/// `Refused::as_str` in `firmware/heltec-lora-linktest/src/main.rs`, which is
/// a separate workspace and cannot share the constant. Matched on the prefix
/// so the parenthetical can be reworded freely; pinned by
/// `the_firmwares_bad_tag_wording_is_the_one_we_match`. If the firmware drops
/// the phrase entirely this fails *safe* — no burst is opened, nothing is
/// escalated — and the drift shows up as a debug line rather than as a false
/// alarm, which is the right direction for a forgeable input.
const STATION_BAD_TAG: &str = "bad tag";

/// One frame the **station** refused on the air, from a `SPINE ◄ REJECTED`
/// console line. Unauthenticated: this is the station's word, not the host's.
///
/// Note what this line does *not* say: which station refused it. The console
/// belongs to one station and the host knows the port, not the id, so the
/// refuser is identified only as "the station on this console". With two
/// stations that is unambiguous; with three it would not be, and the fix is a
/// station id on the firmware's warning line. `TODO(source)` — see
/// SPINE-REPLAY.md §6 step 7.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayRefusal {
    /// The refused frame's **claimed** origin, from `src=` (hex). A forger
    /// picks this freely, so it names who is being impersonated, never who is
    /// transmitting — and it must never be read as the station that refused.
    pub src: u8,
    /// The counter on the refused frame, from `ctr=`.
    pub ctr: Option<u32>,
    /// Signal strength the station reported, from `rssi=` — the one field a
    /// forger cannot choose, and the reason it is worth carrying.
    pub rssi_dbm: i32,
    /// The station's reason text, verbatim.
    pub reason: String,
    /// Whether the reason is a bad tag: the only kind that is evidence of a
    /// forgery rather than of RF (`runt`, `seq`), of ordinary relay traffic
    /// (`already accepted` — which the station prints for neither a duplicate
    /// nor a replay, since it cannot tell them apart) or of the station's own
    /// bookkeeping (`older than the receive window`, `NVS refused`).
    pub bad_tag: bool,
}

/// Parse a station refusal line. Returns `None` for every other line,
/// including accepted frames.
pub fn parse_gateway_refusal(line: &str) -> Option<GatewayRefusal> {
    let start = line.find(REJECTED_MARKER)?;
    let rest = &line[start + REJECTED_MARKER.len()..];

    let src = u8::from_str_radix(
        leading(field_after(rest, "src=")?, |c| c.is_ascii_hexdigit()),
        16,
    )
    .ok()?;
    let rssi: i32 = leading(field_after(rest, "rssi=")?, |c| {
        c == '-' || c.is_ascii_digit()
    })
    .parse()
    .ok()?;
    let ctr: Option<u32> =
        field_after(rest, "ctr=").and_then(|s| leading(s, |c| c.is_ascii_digit()).parse().ok());
    // The station prints `… ({} B): {reason}`; the reason runs to the ANSI
    // colour reset, as the payload does on an accepted line.
    let reason = field_after(rest, "): ")?
        .split('\u{1b}')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if reason.is_empty() {
        return None;
    }
    let bad_tag = reason.starts_with(STATION_BAD_TAG);

    Some(GatewayRefusal {
        src,
        ctr,
        rssi_dbm: rssi,
        reason,
        bad_tag,
    })
}

/// Prefix of the per-station facts the air watch writes. Distinct from
/// [`AUTH_FACT_PREFIX`] on purpose: one entity is what the host proved, the
/// other is what the station said.
pub const AIR_FACT_PREFIX: &str = "spine.air.";

/// How many stations are currently reporting refused frames on the air — the
/// one number the advisory rule watches, as `spine.auth.alarm_count` is for
/// the authenticated alarm. Written on change only.
pub const AIR_REFUSED_COUNT_FACT: &str = "spine.air.refused_count";

/// A burst closes this long after its last refused frame, matching
/// [`AUTH_ALARM_CLEAR_MS`] so the two signals describe an incident the same way.
pub const AIR_REFUSED_CLEAR_MS: u64 = AUTH_ALARM_CLEAR_MS;

/// Most claimed sources that may hold an open burst at once.
///
/// The key of a burst fact is the *claimed* source, which a forger chooses,
/// so without a cap one transmitter cycling `src` could mint 256 entities in
/// world memory through an input that is unauthenticated by construction.
/// Past the cap the refusals are counted on [`AIR_REFUSED_COUNT_FACT`]'s
/// overflow rather than given entities of their own — the operator still
/// learns that something is spraying, which is the more useful finding
/// anyway, and the store is not the thing that pays for it.
pub const AIR_MAX_TRACKED_SOURCES: usize = 8;

/// An open burst of refusals at one station.
#[derive(Debug, Clone)]
struct AirBurst {
    since_ms: u64,
    last_ms: u64,
    count: u64,
    fact_id: Option<i64>,
}

/// Watches what the stations say they are refusing on the air. Held beside
/// [`LoraAuth`] rather than inside it: the verifier's state is evidence, and
/// this is hearsay, and mixing them would let the weaker one borrow the
/// stronger one's authority.
#[derive(Debug, Default)]
pub struct AirWatch {
    bursts: std::collections::HashMap<u8, AirBurst>,
    /// Refusals dropped because [`AIR_MAX_TRACKED_SOURCES`] bursts were
    /// already open. Counted, never given an entity.
    overflow: u64,
}

impl AirWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adopt the bursts a previous process left open, so they can be closed.
    ///
    /// Without this a restart during a burst orphans the fact: the new watch
    /// knows nothing, `sweep` returns early when it has nothing to expire, and
    /// `spine.air.refused_count` stays at its old value **forever** — a rule
    /// firing on a condition that ended, which is the failure this repository
    /// keeps calling silent degradation. The clear timer resumes from when the
    /// fact was last written, exactly as [`LoraAuth`] resumes an open alarm.
    ///
    /// The resumed `count` is what the open fact carried, which is what it had
    /// at the moment the burst opened — a restart mid-burst therefore
    /// undercounts. Recorded rather than hidden: the count is for an
    /// operator's sense of scale, and the alternative is a write per frame.
    pub fn resume(&mut self, world: &WorldMemory) {
        for entity in world.entities().unwrap_or_default() {
            if !(entity.starts_with(AIR_FACT_PREFIX) && entity.ends_with(".refused")) {
                continue;
            }
            let Ok(Some(fact)) = world.current(&entity) else {
                continue;
            };
            if fact.value.get("status").and_then(Value::as_str) != Some("refusing") {
                continue;
            }
            let Some(src) = fact
                .value
                .get("claimed_src")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("gw-"))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            else {
                continue;
            };
            self.bursts.insert(
                src,
                AirBurst {
                    since_ms: fact
                        .value
                        .get("since_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(fact.valid_from),
                    last_ms: fact.valid_from,
                    count: fact.value.get("count").and_then(Value::as_u64).unwrap_or(1),
                    fact_id: Some(fact.id),
                },
            );
            let claimed = LoraAuth::station(src);
            tracing::info!(
                claimed_src = %claimed,
                "[lora_gateway] resumed an open on-air refusal burst left by the previous process"
            );
        }
    }

    /// Record one refusal the station reported. Only a bad tag opens or
    /// extends a burst; every other reason is logged and dropped. Like the
    /// auth alarm, the fact is written once per burst and the count travels
    /// on the clear, so a flood of forged frames is one incident.
    pub fn observe(&mut self, r: &GatewayRefusal, world: &WorldMemory, now_ms: u64) {
        if !r.bad_tag {
            tracing::debug!(
                station = %LoraAuth::station(r.src),
                ctr = r.ctr,
                rssi = r.rssi_dbm,
                "[lora_gateway] station refused a frame (not a bad tag): {}",
                r.reason
            );
            return;
        }
        let claimed = LoraAuth::station(r.src);
        // Read the cap before taking the mutable borrow below.
        let at_cap = self.bursts.len() >= AIR_MAX_TRACKED_SOURCES;
        let known = self.bursts.contains_key(&r.src);
        match self.bursts.get_mut(&r.src) {
            Some(b) => {
                b.count += 1;
                b.last_ms = now_ms;
            }
            None if at_cap && !known => {
                // Someone is cycling `src`. Do not mint an entity per id.
                self.overflow += 1;
                if self.overflow == 1 || self.overflow.is_multiple_of(100) {
                    tracing::warn!(
                        claimed_src = %claimed,
                        overflow = self.overflow,
                        "[lora_gateway] ON AIR: refusals now name more than {} distinct sources \
                         — a transmitter is cycling the claimed id; counting without tracking",
                        AIR_MAX_TRACKED_SOURCES
                    );
                }
                self.record_count(world, now_ms);
            }
            None => {
                let fact = world
                    .observe_as(
                        &format!("{AIR_FACT_PREFIX}{claimed}.refused"),
                        json!({
                            "status": "refusing",
                            // The frame's *claimed* origin — who is being
                            // impersonated. Named `claimed_src`, never
                            // `station`, because the obvious misreading ("this
                            // station is refusing") sends an operator to the
                            // wrong board: the refuser is the station on this
                            // host's console, which the console line does not
                            // identify.
                            "claimed_src": claimed,
                            "refused_by": "the station on this host's console",
                            "reason": r.reason,
                            "ctr": r.ctr,
                            // The one field a forger does not choose.
                            "rssi_dbm": r.rssi_dbm,
                            "count": 1,
                            "since_ms": now_ms,
                            // Said on the record, because this fact will be
                            // read by people and by rules: the station is the
                            // only witness and its console is not authenticated.
                            "evidence": "station-asserted (unauthenticated console)",
                        }),
                        now_ms,
                        now_ms,
                        SOURCE,
                        Origin::Observed,
                    )
                    .ok();
                tracing::warn!(
                    claimed_src = %claimed,
                    ctr = r.ctr,
                    rssi = r.rssi_dbm,
                    "[lora_gateway] ON AIR: the station on this console is refusing frames that \
                     claim to come from {} — it cannot authenticate them (station-asserted)",
                    claimed
                );
                self.bursts.insert(
                    r.src,
                    AirBurst {
                        since_ms: now_ms,
                        last_ms: now_ms,
                        count: 1,
                        fact_id: fact.map(|f| f.id),
                    },
                );
                self.record_count(world, now_ms);
            }
        }
    }

    /// Close every burst whose last refusal is older than
    /// [`AIR_REFUSED_CLEAR_MS`]. Driven from the RX loop on *every* console
    /// line, not only on refusals — otherwise a burst that stops would stay
    /// open forever, which is the failure mode of a signal that only hears
    /// bad news.
    pub fn sweep(&mut self, world: &WorldMemory, now_ms: u64) {
        let expired: Vec<u8> = self
            .bursts
            .iter()
            .filter(|(_, b)| now_ms.saturating_sub(b.last_ms) >= AIR_REFUSED_CLEAR_MS)
            .map(|(src, _)| *src)
            .collect();
        if expired.is_empty() {
            return;
        }
        for src in expired {
            let Some(b) = self.bursts.remove(&src) else {
                continue;
            };
            let claimed = LoraAuth::station(src);
            let _ = world.observe_derived_from(
                &format!("{AIR_FACT_PREFIX}{claimed}.refused"),
                json!({
                    "status": "quiet",
                    "claimed_src": claimed,
                    "count": b.count,
                    "since_ms": b.since_ms,
                    "last_ms": b.last_ms,
                    "until_ms": now_ms,
                }),
                now_ms,
                now_ms,
                SOURCE,
                &b.fact_id.into_iter().collect::<Vec<_>>(),
            );
            tracing::info!(
                claimed_src = %claimed,
                refused = b.count,
                "[lora_gateway] on-air refusals naming {claimed} have stopped: none for {} s",
                AIR_REFUSED_CLEAR_MS / 1000
            );
        }
        self.record_count(world, now_ms);
    }

    /// `spine.air.refused_count` — how many distinct claimed sources are
    /// currently being refused on the air. The rule reads this one number;
    /// it stays a plain count so the condition is a comparison, and the
    /// overflow is not folded in (it would make the number mean two things).
    fn record_count(&self, world: &WorldMemory, now_ms: u64) {
        let support: Vec<i64> = self.bursts.values().filter_map(|b| b.fact_id).collect();
        let _ = world.observe_derived_from(
            AIR_REFUSED_COUNT_FACT,
            json!(self.bursts.len() as u64),
            now_ms,
            now_ms,
            SOURCE,
            &support,
        );
    }

    /// Claimed sources whose frames are currently being refused on the air,
    /// for `mesh_status` and `status`.
    pub fn refusing_stations(world: &WorldMemory) -> Vec<Value> {
        let mut out = Vec::new();
        for e in world.entities().unwrap_or_default() {
            if !(e.starts_with(AIR_FACT_PREFIX) && e.ends_with(".refused")) {
                continue;
            }
            if let Ok(Some(f)) = world.current(&e) {
                if f.value.get("status").and_then(Value::as_str) == Some("refusing") {
                    out.push(f.value.clone());
                }
            }
        }
        out
    }
}

/// `~/x` → `<home>/x`; anything else unchanged.
fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Ingest a node's own JSON payload as `mesh.<node_id>.<type>` + a `mesh.<node_id>` rollup.
fn ingest_node_json(
    frame: &GatewayFrame,
    payload: Value,
    world: &WorldMemory,
    now_ms: u64,
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
        "ctr": frame.ctr,
        "rssi_dbm": frame.rssi_dbm,
    });

    // Per-type fact: the node payload with a mesh envelope attached.
    let mut enriched = payload.clone();
    if let Value::Object(ref mut m) = enriched {
        m.insert("_mesh".into(), mesh_meta.clone());
    }
    // Observed: this arrived over the air from the node's own radio. The gateway is a
    // transcriber here, not an interpreter — which is exactly what makes it evidence.
    let _ = world.observe_as(
        &format!("mesh.{node_id}.{msg_type}"),
        enriched,
        now_ms,
        now_ms,
        SOURCE,
        Origin::Observed,
    );

    // Liveness/link rollup fact.
    let link = json!({
        "rssi_dbm": frame.rssi_dbm,
        "seq": frame.seq,
        "src": format!("{:02X}", frame.src),
        "last_type": msg_type,
    });
    let _ = world.observe_as(
        &format!("mesh.{node_id}"),
        link,
        now_ms,
        now_ms,
        SOURCE,
        Origin::Observed,
    );

    GatewayIngest {
        node_id,
        msg_type,
        rssi_dbm: frame.rssi_dbm,
    }
}

/// Ingest a ClawCam field summary heard over the mesh (G2) as `clawcam.<device>.field`
/// (the full summary + mesh envelope) plus a compact `clawcam.<device>` rollup — so an
/// off-grid camera's counts/conditions land in the brain's world model.
fn ingest_clawcam_summary(
    frame: &GatewayFrame,
    s: ClawCamSummary,
    world: &WorldMemory,
    now_ms: u64,
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
    let _ = world.observe(
        &format!("clawcam.{dev}.field"),
        field,
        now_ms,
        now_ms,
        SOURCE,
    );

    let rollup = json!({
        "total": s.total,
        "top_species": top,
        "temperature_c": s.temperature_c,
        "battery_percent": s.battery_percent,
        "rssi_dbm": frame.rssi_dbm,
    });
    let _ = world.observe(&format!("clawcam.{dev}"), rollup, now_ms, now_ms, SOURCE);

    GatewayIngest {
        node_id: dev,
        msg_type: "clawcam_field".to_string(),
        rssi_dbm: frame.rssi_dbm,
    }
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
        Self {
            to: to.into(),
            id: id.into(),
            cmd: cmd.into(),
            args,
        }
    }

    /// A descending modulation for `to`: `(slot, level)` pairs, levels in
    /// `[0, 1]`, only the slots being changed. The node applies them
    /// all-or-nothing to its reflex slots (`firmware/obc-esp32-s3/src/reflex.rs`);
    /// with `clear`, it first returns every slot to its rule's default. Refuses
    /// here what the node would refuse, so a bad message never spends a frame.
    pub fn descend(
        to: impl Into<String>,
        id: impl Into<String>,
        pairs: &[(u8, f64)],
        clear: bool,
    ) -> anyhow::Result<Self> {
        for (slot, level) in pairs {
            anyhow::ensure!(
                (*slot as usize) < obc_reflex::MAX_SLOTS,
                "descend: slot {slot} out of range (max {})",
                obc_reflex::MAX_SLOTS - 1
            );
            anyhow::ensure!(
                *level >= 0.0 && *level <= 1.0,
                "descend: slot {slot}: level {level} not in [0, 1]"
            );
        }
        let m: Vec<Value> = pairs
            .iter()
            .map(|(s, l)| json!([s, (l * 1000.0).round() / 1000.0]))
            .collect();
        let args = if clear {
            json!({ "clear": true, "m": m })
        } else {
            json!({ "m": m })
        };
        Ok(Self::new(to, id, "descend", args))
    }

    /// Encode to the single newline-free line the gateway will carry over LoRa and
    /// the node will feed to its request dispatcher.
    pub fn encode(&self) -> String {
        json!({ "id": self.id, "to": self.to, "cmd": self.cmd, "args": self.args }).to_string()
    }

    /// Encoded length in bytes, against [`MESH_LINE_BUDGET`].
    pub fn encoded_len(&self) -> usize {
        self.encode().len()
    }

    /// Whether this command survives the trip.
    ///
    /// A line longer than the budget is discarded whole by the bridge's
    /// `LineFramer` — correctly, since sending a prefix of a JSON command is
    /// worse. But nothing on the host side was checking, so an oversized
    /// `mesh_command` returned `sent: true` for a command the mesh never
    /// carried. It fails closed, which is the right direction, and silently,
    /// which is not: for a tool classed `physical(true, BlastRadius::High)` the
    /// operator's log said the command went.
    pub fn fits_one_frame(&self) -> bool {
        self.encoded_len() <= MESH_LINE_BUDGET
    }
}

/// The longest command line the mesh can carry, in bytes.
///
/// This is the station's `MAX_AUTH_PAYLOAD`
/// (`firmware/heltec-lora-linktest/src/spine.rs`): the 240-byte radio budget
/// less the 12 bytes an authenticated frame spends on `[ctr:u32][mac:8]`
/// (SPINE-AUTH.md §3.2, on the wire since 2026-09-13). Duplicated here
/// because the firmware builds for xtensa and the host cannot link it.
/// `tests/spine_payload_budget.rs` pins the two together — it includes the
/// firmware source via `#[path]` and fails if they ever disagree, which is the
/// same arrangement `tests/firmware_spine_framing.rs` already uses to run the
/// framer's own tests on the host.
pub const MESH_LINE_BUDGET: usize = 228;

/// A transport that delivers a [`NodeCommand`] toward the mesh. The serial
/// implementation writes the encoded line to the base-station Heltec's console; tests
/// use an in-memory mock. Lets the `mesh_command` tool stay transport-blind.
#[async_trait]
pub trait CommandSink: Send + Sync {
    /// Deliver `cmd` toward the mesh (the node still gates execution on-MCU).
    async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()>;
}

// ── The host's link to the base station: a fact, a handle, a supervisor ──────────
//
// On 2026-09-13 the brain lost its base station's serial port eleven minutes
// after opening it (`os error 22`, a surprise-removed USB device), ran headless
// for fourteen more, and the only place that said so was one WARN line. The
// mesh supervisor, deaf, presumed both nodes lost while they beaconed normally;
// the posture policy's sends failed with "serial I/O thread has exited"; a
// person restarted the process and everything came back — which is the proof
// that the fix is cheap. OBC-Prime `docs/SPINE-LOSS.md` is the design; this is
// it. Three parts, hardware-free so they are tested where the serial port is
// not: the link has a fact (`spine.gateway`), the sink survives an outage
// (the writer sits behind a handle the supervisor swaps), and the port is
// reopened with bounded backoff for as long as the process lives.

/// Outbound command queue into the single serial I/O thread.
pub type SerialWriterHandle = tokio::sync::mpsc::Sender<String>;

/// What the serial I/O thread hands the RX loop: a console line, or the
/// reason it is stopping. The reason travels in-band so the loss is recorded
/// with the error that caused it rather than as a bare closed channel.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleEvent {
    Line(String),
    /// The I/O thread is exiting; the port is gone until reopened.
    Closed(String),
}

/// Inbound console events from the single serial I/O thread.
pub type ConsoleLines = tokio::sync::mpsc::Receiver<ConsoleEvent>;

/// The one entity that says whether the brain can hear the mesh right now.
/// One fact, not one per station: it describes the host's serial link; the
/// stations' own liveness stays in `mesh.gw-XX` and `spine.auth.gw-XX`.
pub const GATEWAY_FACT: &str = "spine.gateway";

/// Longest wait between reopen attempts.
pub const REOPEN_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Wait before reopen attempt number `attempt` (1-based): 1 s, 2 s, 4 s, …,
/// capped at [`REOPEN_BACKOFF_MAX`]. No attempt limit anywhere — a body that
/// runs unattended does not give up on its own spine because a hub blinked at
/// 3 a.m.
pub fn reopen_backoff(attempt: u32) -> std::time::Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(5);
    std::time::Duration::from_secs(secs).min(REOPEN_BACKOFF_MAX)
}

/// The state of the host's serial link to the base station, as recorded in
/// the [`GATEWAY_FACT`] fact on every transition and every reopen attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayLink {
    /// The port is open and the I/O thread is running. `attempts` is how many
    /// reopen attempts the last outage took (0 for the first open), so an
    /// outage is legible from the fact history alone.
    Open { since_ms: u64, attempts: u32 },
    /// The I/O thread exited with `error`; nothing has been tried yet.
    Lost { since_ms: u64, error: String },
    /// `attempts` reopens have failed since the loss; the next is due at
    /// `next_attempt_ms`. `error` is the most recent failure.
    Reopening {
        since_ms: u64,
        error: String,
        attempts: u32,
        next_attempt_ms: u64,
    },
}

impl GatewayLink {
    pub fn state(&self) -> &'static str {
        match self {
            GatewayLink::Open { .. } => "open",
            GatewayLink::Lost { .. } => "lost",
            GatewayLink::Reopening { .. } => "reopening",
        }
    }

    /// When this state began.
    pub fn since_ms(&self) -> u64 {
        match self {
            GatewayLink::Open { since_ms, .. }
            | GatewayLink::Lost { since_ms, .. }
            | GatewayLink::Reopening { since_ms, .. } => *since_ms,
        }
    }

    /// The error behind a loss, if the link is not open.
    pub fn error(&self) -> Option<&str> {
        match self {
            GatewayLink::Open { .. } => None,
            GatewayLink::Lost { error, .. } | GatewayLink::Reopening { error, .. } => Some(error),
        }
    }

    /// What a caller that needs the link gets told while it is down — the same
    /// words as the fact, so a `descending.<node>` error and world memory agree.
    pub fn refusal(&self) -> String {
        match self {
            GatewayLink::Open { .. } => "gateway open".to_string(),
            GatewayLink::Lost { error, .. } => format!("gateway lost: {error}"),
            GatewayLink::Reopening {
                error, attempts: 0, ..
            } => format!("gateway reopening: {error}"),
            GatewayLink::Reopening {
                error, attempts, ..
            } => format!("gateway reopening ({attempts} failed so far): {error}"),
        }
    }

    fn to_json(&self, port: &str) -> Value {
        let mut v = json!({ "state": self.state(), "port": port, "since_ms": self.since_ms() });
        match self {
            GatewayLink::Open { attempts, .. } => {
                v["attempts"] = json!(attempts);
            }
            GatewayLink::Lost { error, .. } => {
                v["error"] = json!(error);
                v["attempts"] = json!(0);
            }
            GatewayLink::Reopening {
                error,
                attempts,
                next_attempt_ms,
                ..
            } => {
                v["error"] = json!(error);
                v["attempts"] = json!(attempts);
                v["next_attempt_ms"] = json!(next_attempt_ms);
            }
        }
        v
    }

    /// Write this state as the [`GATEWAY_FACT`] fact.
    pub fn record(&self, world: &WorldMemory, port: &str, now_ms: u64) {
        let _ = world.observe_as(
            GATEWAY_FACT,
            self.to_json(port),
            now_ms,
            now_ms,
            SOURCE,
            Origin::Observed,
        );
    }

    /// The link state world memory currently holds, if a gateway has ever
    /// written one. `None` means no serial gateway is part of this body.
    pub fn read(world: &WorldMemory) -> Option<GatewayLink> {
        let f = world.current(GATEWAY_FACT).ok().flatten()?;
        let v = &f.value;
        let since_ms = v
            .get("since_ms")
            .and_then(Value::as_u64)
            .unwrap_or(f.valid_from);
        let error = v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let attempts = v.get("attempts").and_then(Value::as_u64).unwrap_or(0) as u32;
        match v.get("state").and_then(Value::as_str)? {
            "open" => Some(GatewayLink::Open { since_ms, attempts }),
            "lost" => Some(GatewayLink::Lost { since_ms, error }),
            "reopening" => Some(GatewayLink::Reopening {
                since_ms,
                error,
                attempts,
                next_attempt_ms: v
                    .get("next_attempt_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            }),
            _ => None,
        }
    }
}

/// The sink's view of the link: the current writer (into the I/O thread that
/// owns the port) and the link state. The supervisor replaces the writer on
/// every reopen, so the `Arc<dyn CommandSink>` handed out at startup keeps
/// working across outages.
pub struct GatewayHandle {
    inner: std::sync::RwLock<(Option<SerialWriterHandle>, GatewayLink)>,
}

impl GatewayHandle {
    /// A handle around a freshly opened port.
    pub fn open(writer: SerialWriterHandle, now_ms: u64) -> Self {
        Self {
            inner: std::sync::RwLock::new((
                Some(writer),
                GatewayLink::Open {
                    since_ms: now_ms,
                    attempts: 0,
                },
            )),
        }
    }

    /// A handle for a port that could not be opened at startup: the link
    /// starts `lost` with the open error, and the supervisor reopens it as it
    /// would after a loss. A body whose base station is unplugged when it
    /// boots is not misconfigured; it is in an outage that began at t = 0
    /// (2026-09-14: the third time a bench that was simply unplugged left the
    /// brain refusing to start until someone noticed).
    pub fn lost(error: String, now_ms: u64) -> Self {
        Self {
            inner: std::sync::RwLock::new((
                None,
                GatewayLink::Lost {
                    since_ms: now_ms,
                    error,
                },
            )),
        }
    }

    /// The link as the handle last heard it.
    pub fn link(&self) -> GatewayLink {
        self.inner.read().unwrap().1.clone()
    }

    fn set(&self, writer: Option<SerialWriterHandle>, link: GatewayLink) {
        *self.inner.write().unwrap() = (writer, link);
    }

    fn writer(&self) -> Result<SerialWriterHandle, String> {
        let g = self.inner.read().unwrap();
        match (&g.0, &g.1) {
            (Some(w), GatewayLink::Open { .. }) => Ok(w.clone()),
            (_, link) => Err(link.refusal()),
        }
    }
}

/// Outbound [`CommandSink`] over the base-station Heltec's console: queues each
/// command (newline-framed) into the serial I/O thread, which writes it on the
/// one true handle; the station then transmits it over LoRa. While the link is
/// lost or reopening the send fails fast with the link state — the same words
/// as the [`GATEWAY_FACT`] fact — and nothing is queued: the gateway does not
/// retry what it was asked to send while down; the caller decides.
pub struct SerialCommandSink {
    handle: Arc<GatewayHandle>,
}

impl SerialCommandSink {
    pub fn new(handle: Arc<GatewayHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl CommandSink for SerialCommandSink {
    async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
        let writer = self.handle.writer().map_err(|why| anyhow::anyhow!(why))?;
        writer
            .send(cmd.encode())
            .await
            .map_err(|_| anyhow::anyhow!("serial I/O thread has exited"))?;
        Ok(())
    }
}

/// RX loop: take console lines off the reader-thread channel, verify each
/// received frame under the deployment root, and bridge the verified ones
/// into world memory. Runs until the reader thread exits, and returns why
/// (the I/O error it reported, or a note that it ended without one). `auth`
/// is borrowed, not consumed: the supervisor runs this again on every reopen
/// with the same per-station windows, so a reopen admits no replay of what
/// was accepted before the loss.
pub async fn run_gateway_rx<F>(
    mut lines: ConsoleLines,
    auth: &mut LoraAuth,
    air: &mut AirWatch,
    world: Arc<WorldMemory>,
    now_ms: F,
) -> String
where
    F: Fn() -> u64 + Send,
{
    while let Some(event) = lines.recv().await {
        let line = match event {
            ConsoleEvent::Line(line) => line,
            ConsoleEvent::Closed(why) => return why,
        };
        // Raw-line visibility: silence must never again be ambiguous between
        // "no bytes" and "bytes that don't parse" (bench lesson, 2026-07-17).
        // Debug level — enable with RUST_LOG=debug when diagnosing.
        tracing::debug!(
            "[lora_gateway] raw: {}",
            line.chars().take(110).collect::<String>()
        );
        let Some(frame) = parse_gateway_line(&line) else {
            // Not a frame. It may still be the station saying it refused one
            // — the only way the host ever hears about a forgery on the air
            // (DECISIONS.md 2026-09-15).
            if let Some(refusal) = parse_gateway_refusal(&line) {
                let now = now_ms();
                air.observe(&refusal, &world, now);
                air.sweep(&world, now);
            }
            continue;
        };
        let now = now_ms();
        // Every console line is a clock tick for the burst timer, not just the
        // refusals: a burst that stops has to be able to close.
        air.sweep(&world, now);
        match auth.admit(&frame, &world, now) {
            Ok(()) => {
                if let Some(ing) = ingest_frame(&frame, &world, now) {
                    tracing::info!(
                        node = %ing.node_id,
                        msg = %ing.msg_type,
                        rssi = ing.rssi_dbm,
                        ctr = frame.ctr,
                        "LoRa gateway → world memory (verified)"
                    );
                }
            }
            Err(why) => tracing::warn!(
                station = %LoraAuth::station(frame.src),
                ctr = frame.ctr,
                rssi = frame.rssi_dbm,
                "[lora_gateway] REJECTED frame: {} — payload not ingested",
                why.as_str()
            ),
        }
    }
    "serial I/O thread ended without reporting an error".to_string()
}

/// Own the base-station link for the life of the process. Runs the RX loop
/// on `first` — or, when the first open failed, starts in the outage with
/// that error — and whenever the I/O thread exits records `lost` with its
/// error, then reopens with [`reopen_backoff`] until it succeeds: recording
/// `reopening` before each attempt and `open` after, swapping the writer in
/// `handle` so the sink keeps working, then running the RX loop again with
/// the same `auth`. Never returns.
///
/// `open` is whatever produces a `(lines, writer)` pair: `open_split` on
/// hardware, a pair of channels in tests. It is awaited, so a blocking open
/// can be moved off the runtime by the caller.
///
/// The [`AirWatch`] is owned here for the same reason `auth` is passed in:
/// an open burst of on-air refusals must survive a reopen, or a forger could
/// hide behind a flapping USB cable.
pub async fn supervise_gateway<O, Fut, F>(
    port: String,
    first: Result<ConsoleLines, String>,
    mut open: O,
    mut auth: LoraAuth,
    world: Arc<WorldMemory>,
    handle: Arc<GatewayHandle>,
    now_ms: F,
) where
    O: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<(ConsoleLines, SerialWriterHandle)>>,
    F: Fn() -> u64 + Send + Clone,
{
    let mut lines = match first {
        Ok(lines) => lines,
        Err(error) => {
            tracing::warn!(port = %port, "[lora_gateway] not open at start — reopening with backoff: {error}");
            reopen_until_open(&port, error, &mut open, &world, &handle, &now_ms).await
        }
    };
    let mut air = AirWatch::new();
    air.resume(&world);
    loop {
        let error = run_gateway_rx(
            lines,
            &mut auth,
            &mut air,
            Arc::clone(&world),
            now_ms.clone(),
        )
        .await;
        tracing::warn!(port = %port, "[lora_gateway] link lost — reopening with backoff: {error}");
        lines = reopen_until_open(&port, error, &mut open, &world, &handle, &now_ms).await;
    }
}

/// The outage: record `lost`, then `reopening` before each attempt, and
/// `open` when one succeeds. Returns the new line channel.
async fn reopen_until_open<O, Fut, F>(
    port: &str,
    error: String,
    open: &mut O,
    world: &WorldMemory,
    handle: &GatewayHandle,
    now_ms: &F,
) -> ConsoleLines
where
    O: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<(ConsoleLines, SerialWriterHandle)>>,
    F: Fn() -> u64,
{
    let lost_at = now_ms();
    let link = GatewayLink::Lost {
        since_ms: lost_at,
        error: error.clone(),
    };
    handle.set(None, link.clone());
    link.record(world, port, lost_at);

    let mut attempts: u32 = 0;
    let mut last_error = error;
    loop {
        attempts += 1;
        let wait = reopen_backoff(attempts);
        let now = now_ms();
        let link = GatewayLink::Reopening {
            since_ms: lost_at,
            error: last_error.clone(),
            attempts: attempts - 1,
            next_attempt_ms: now + wait.as_millis() as u64,
        };
        handle.set(None, link.clone());
        link.record(world, port, now);
        tokio::time::sleep(wait).await;
        match open().await {
            Ok((rd, wr)) => {
                let now = now_ms();
                let link = GatewayLink::Open {
                    since_ms: now,
                    attempts,
                };
                handle.set(Some(wr), link.clone());
                link.record(world, port, now);
                tracing::info!(
                    port = %port,
                    outage_ms = now.saturating_sub(lost_at),
                    attempts,
                    "[lora_gateway] link opened"
                );
                return rd;
            }
            Err(e) => {
                last_error = format!("{e:#}");
                tracing::warn!(
                    port = %port,
                    attempt = attempts,
                    "[lora_gateway] reopen failed: {last_error}"
                );
            }
        }
    }
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
    use super::{ConsoleEvent, ConsoleLines, SerialWriterHandle};
    use anyhow::Context;
    use std::io::{Read, Write};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Minimum spacing between consecutive commands written to the base console.
    ///
    /// A 240-byte line clears a 115200 baud link in about 21 ms, so this is
    /// roughly twice the worst case — enough for the board's reader to drain
    /// stdin between commands, short enough that no operator will notice it.
    const INTER_COMMAND_GAP: Duration = Duration::from_millis(50);

    /// Open the base-station Heltec console. Returns a channel of console lines
    /// and a command-queue sender — BOTH serviced by one dedicated I/O thread on
    /// one handle. No `try_clone`: duplicated Windows COM handles fail writes
    /// with `os error 22` (ERROR_BAD_COMMAND) while the original reads fine —
    /// bench-caught 2026-07-17 when the first over-the-air command never left
    /// the PC.
    pub fn open_split(port: &str, baud: u32) -> anyhow::Result<(ConsoleLines, SerialWriterHandle)> {
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

        // Single I/O thread, single handle: interleave 250 ms-timeout reads with
        // draining the outbound command queue. Read timeouts are the idle path.
        let (line_tx, line_rx) = mpsc::channel::<ConsoleEvent>(256);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(32);
        std::thread::Builder::new()
            .name("lora-gateway-io".into())
            .spawn(move || {
                let mut buf = [0u8; 512];
                let mut line: Vec<u8> = Vec::with_capacity(256);
                loop {
                    // 1) Outbound: drain any queued commands (newline-framed).
                    let mut wrote_one = false;
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        // Space consecutive commands. Newline framing is correct here,
                        // but it only helps a reader that keeps up; on 2026-07-19 two
                        // commands written back to back (77 B then 99 B, microseconds
                        // apart) reached the base faster than its console reader drained
                        // stdin, and both were transmitted as mid-string fragments. The
                        // firmware reader is fixed, but every board in the field runs
                        // the old one until it is reflashed, so pace the host too — this
                        // is the side we can change without a flash.
                        if wrote_one {
                            std::thread::sleep(INTER_COMMAND_GAP);
                        }
                        wrote_one = true;
                        let r = serial
                            .write_all(cmd.as_bytes())
                            .and_then(|()| serial.write_all(b"\n"))
                            .and_then(|()| serial.flush());
                        match r {
                            Ok(()) => tracing::info!(
                                "[lora_gateway] command written to base console ({} B)",
                                cmd.len() + 1
                            ),
                            Err(e) => tracing::warn!("[lora_gateway] serial write error: {e}"),
                        }
                    }
                    // 2) Inbound: read with timeout, frame into lines.
                    match serial.read(&mut buf) {
                        Ok(0) => {}
                        Ok(n) => {
                            for &b in &buf[..n] {
                                if b == b'\n' || b == b'\r' {
                                    if !line.is_empty() {
                                        let s = String::from_utf8_lossy(&line).into_owned();
                                        line.clear();
                                        if line_tx.blocking_send(ConsoleEvent::Line(s)).is_err() {
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
                            // The port is gone (2026-09-13: `os error 22`, a
                            // surprise-removed device). Say why on the way out
                            // so the supervisor records the loss with its cause.
                            tracing::warn!("[lora_gateway] serial read error: {e}");
                            let _ = line_tx.blocking_send(ConsoleEvent::Closed(e.to_string()));
                            return;
                        }
                    }
                }
            })
            .context("failed to spawn LoRa gateway I/O thread")?;

        Ok((line_rx, cmd_tx))
    }
}

#[cfg(feature = "hardware")]
pub use serial::open_split;

#[cfg(test)]
mod tests {
    use super::*;

    const REFLEX_LINE: &str = "SPINE ◄ src=28 seq=30 rssi=-42 dBm : {\"type\":\"reflex\",\"node_id\":\"obc-esp32-s3-001\",\"rule\":\"safe-link-offline\"}";

    /// A `cmd_result` heard over the air lands as the fact `mesh_command`'s
    /// reply-wait polls, with the command's id on it. The line is verbatim from
    /// the base console on 2026-09-12 (`results/bench_descend_lora-20260912-233135.json`,
    /// step b2) — ANSI colour and all, since that is what the serial port hands
    /// the host.
    #[test]
    fn a_real_reply_line_becomes_the_fact_the_tool_waits_for() {
        let line = "\u{1b}[0;32mI (147833) heltec_lora_linktest: SPINE ◄ src=40 seq=51 rssi=-50 dBm snr=12 dB : \
                    {\"id\":\"b2\",\"node_id\":\"obc-esp32-s3-001\",\"ok\":true,\"result\":\"{\\\"active\\\":[[3,0.0]],\\\"applied\\\":1}\",\"type\":\"cmd_result\"}\u{1b}[0m";
        let world = WorldMemory::open_in_memory().unwrap();
        let ing = ingest_gateway_line(line, &world, 1_000).expect("a ◄ line with JSON ingests");
        assert_eq!(ing.node_id, "obc-esp32-s3-001");
        assert_eq!(ing.msg_type, "cmd_result");
        let fact = world
            .current("mesh.obc-esp32-s3-001.cmd_result")
            .unwrap()
            .expect("the reply fact exists");
        assert_eq!(fact.value["id"], json!("b2"));
        assert_eq!(fact.value["ok"], json!(true));
        assert_eq!(fact.value["_mesh"]["rssi_dbm"], json!(-50));
        // The node's `result` is a JSON document inside a string; the tool
        // hands it on as-is, and a consumer parses it once more.
        let inner: Value = serde_json::from_str(fact.value["result"].as_str().unwrap()).unwrap();
        assert_eq!(inner["active"], json!([[3, 0.0]]));
    }

    #[test]
    fn a_descend_encodes_sparse_pairs_the_node_parses() {
        let c =
            NodeCommand::descend("obc-esp32-s3-001", "a7", &[(3, 0.5), (7, 1.0)], false).unwrap();
        assert_eq!(c.cmd, "descend");
        assert_eq!(c.args, json!({ "m": [[3, 0.5], [7, 1.0]] }));
        assert!(c.fits_one_frame());
        // The node reads `m` back as `Vec<(u8, f64)>`.
        let back: Vec<(u8, f64)> = serde_json::from_value(c.args["m"].clone()).unwrap();
        assert_eq!(back, vec![(3, 0.5), (7, 1.0)]);
        let cleared = NodeCommand::descend("n", "a7", &[], true).unwrap();
        assert_eq!(cleared.args, json!({ "clear": true, "m": [] }));
    }

    #[test]
    fn a_descend_the_node_would_refuse_is_refused_before_it_spends_a_frame() {
        let err = NodeCommand::descend("n", "a7", &[(16, 0.5)], false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("slot 16"), "{err}");
        let err = NodeCommand::descend("n", "a7", &[(0, 1.01)], false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not in [0, 1]"), "{err}");
        assert!(NodeCommand::descend("n", "a7", &[(0, f64::NAN)], false).is_err());
    }

    #[test]
    fn descend_levels_are_rounded_so_a_frame_never_carries_float_noise() {
        let c = NodeCommand::descend("n", "a7", &[(1, 1.0 / 3.0)], false).unwrap();
        assert_eq!(c.args["m"][0][1], json!(0.333));
    }

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

    /// The gateway console gained an `snr=` field between `rssi=` and the payload
    /// separator, so that RSSI and SNR together distinguish a weak link from an
    /// overdriven receiver. The parser reads fields by key rather than position, which
    /// is what makes that safe — this pins it, because a positional parser would have
    /// broken silently on every frame the moment the firmware was flashed.
    #[test]
    fn parses_a_line_carrying_the_added_snr_field() {
        let line = "\u{1b}[0;32mI (93772) heltec_lora_linktest: SPINE ◄ src=D8 seq=11 rssi=-10 dBm snr=-7 dB : {\"node_id\":\"gw-D8\",\"type\":\"gw_keepalive\",\"seq\":11}\u{1b}[0m";
        let f = parse_gateway_line(line).expect("line with snr= parses");
        assert_eq!(f.src, 0xD8);
        assert_eq!(f.seq, 11);
        assert_eq!(
            f.rssi_dbm, -10,
            "rssi is still read correctly with a field after it"
        );
        assert_eq!(
            f.payload, "{\"node_id\":\"gw-D8\",\"type\":\"gw_keepalive\",\"seq\":11}",
            "the payload split still lands after the new field"
        );
    }

    #[test]
    fn ignores_tx_relay_keepalive_and_boot_lines() {
        assert!(
            parse_gateway_line("SPINE ► (uart) seq=5 (34 B) {\"type\":\"link_state\"}").is_none()
        );
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
        assert_eq!(
            f.value.get("rule").and_then(|v| v.as_str()),
            Some("safe-link-offline")
        );
        assert_eq!(
            f.value
                .get("_mesh")
                .and_then(|m| m.get("rssi_dbm"))
                .and_then(|v| v.as_i64()),
            Some(-42)
        );
        assert_eq!(f.source, SOURCE);

        // Liveness rollup answers "is the node alive, how strong is the link?".
        let link = world
            .current("mesh.obc-esp32-s3-001")
            .unwrap()
            .expect("rollup fact exists");
        assert_eq!(
            link.value.get("rssi_dbm").and_then(|v| v.as_i64()),
            Some(-42)
        );
        assert_eq!(
            link.value.get("last_type").and_then(|v| v.as_str()),
            Some("reflex")
        );
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
        assert!(
            ingest_gateway_line("SPINE ◄ src=28 seq=1 rssi=-5 dBm : not json", &world, 1).is_none()
        );
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
        assert_eq!(
            v.get("to").and_then(Value::as_str),
            Some("obc-esp32-s3-001")
        );
        assert_eq!(v.get("cmd").and_then(Value::as_str), Some("gpio_write"));
        assert_eq!(
            v.get("args")
                .and_then(|a| a.get("pin"))
                .and_then(Value::as_i64),
            Some(3)
        );
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
        let sink = MockSink {
            sent: std::sync::Mutex::new(Vec::new()),
        };
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
        assert_eq!(
            f.value.get("temperature_c").and_then(|v| v.as_f64()),
            Some(14.5)
        );
        assert_eq!(
            f.value.get("node_rssi").and_then(|v| v.as_f64()),
            Some(-97.0)
        );
        assert_eq!(
            f.value
                .get("_mesh")
                .and_then(|m| m.get("rssi_dbm"))
                .and_then(|v| v.as_i64()),
            Some(-95)
        );
        let sp = f.value.get("species").and_then(|v| v.as_array()).unwrap();
        assert_eq!(sp[0].get("subject").and_then(|v| v.as_str()), Some("deer"));
        assert_eq!(sp[0].get("count").and_then(|v| v.as_u64()), Some(20));

        // Rollup answers "how much activity, and is the camera OK?".
        let r = world
            .current("clawcam.north-ridge-01")
            .unwrap()
            .expect("rollup");
        assert_eq!(r.value.get("total").and_then(|v| v.as_u64()), Some(40));
        assert_eq!(
            r.value
                .get("top_species")
                .and_then(|t| t.get("subject"))
                .and_then(|v| v.as_str()),
            Some("deer")
        );
    }

    // ── Host-side verification (SPINE-AUTH.md §3.4, last bullet) ─────────────

    const ROOT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A console line as the base station prints it since step 4, tagged the
    /// way the station tags it: key = HKDF(root, "gw-XX"), tag over
    /// `src ‖ ctr ‖ payload` — computed with the host's `spine_tag`, which
    /// `tests/spine_auth_vectors.rs` pins to the station's `auth.rs`.
    fn signed_line(root: &str, src: u8, ctr: u32, payload: &str) -> String {
        let key = obc_safety::spine_tag::derive_node_key(root.as_bytes(), &format!("gw-{src:02X}"));
        let mac = obc_safety::spine_tag::tag(&key, src, ctr, payload.as_bytes());
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "\u{1b}[0;32mI (93772) heltec_lora_linktest: SPINE ◄ src={src:02X} seq={} ctr={ctr} \
             mac={hex} rssi=-50 dBm snr=12 dB : {payload}\u{1b}[0m",
            ctr & 0xff
        )
    }

    const KEEPALIVE: &str = r#"{"node_id":"gw-40","type":"gw_keepalive","seq":1}"#;

    #[test]
    fn a_step_4_line_parses_its_counter_and_tag() {
        let f = parse_gateway_line(&signed_line(ROOT, 0x40, 835, KEEPALIVE)).unwrap();
        assert_eq!(f.src, 0x40);
        assert_eq!(f.seq, (835u32 & 0xff) as u8, "seq is the low byte of ctr");
        assert_eq!(f.ctr, Some(835));
        assert!(f.mac.is_some());
        assert_eq!(f.payload, KEEPALIVE);
        assert_eq!(f.signed, KEEPALIVE);
    }

    #[test]
    fn a_pre_step_4_line_parses_without_them() {
        let f = parse_gateway_line(REFLEX_LINE).unwrap();
        assert_eq!(f.ctr, None);
        assert_eq!(f.mac, None);
    }

    #[test]
    fn a_ctr_in_the_payload_is_not_the_frames_counter() {
        // The header is what carries ctr=/mac=; a payload that happens to
        // contain the text must not be read as one.
        let line = "SPINE ◄ src=40 seq=1 rssi=-50 dBm : {\"note\":\"ctr=999 mac=00\"}";
        let f = parse_gateway_line(line).unwrap();
        assert_eq!(f.ctr, None);
        assert_eq!(f.mac, None);
    }

    #[test]
    fn a_verified_frame_is_admitted_and_its_mark_persisted() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let f = parse_gateway_line(&signed_line(ROOT, 0x40, 835, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Ok(()));
        let fact = world
            .current("spine.auth.gw-40")
            .unwrap()
            .expect("auth fact");
        assert_eq!(fact.value["ctr"], 835);
        assert_eq!(fact.value["accepted"], 1);
        assert_eq!(fact.value["rejected"], 0);
        assert!(fact.value["last_rejected"].is_null());
    }

    #[test]
    fn a_frame_under_a_different_root_is_refused_as_a_bad_tag() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let other = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let f = parse_gateway_line(&signed_line(other, 0x40, 835, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Err(LoraRefused::BadTag));
        let fact = world.current("spine.auth.gw-40").unwrap().unwrap();
        assert_eq!(fact.value["rejected"], 1);
        assert_eq!(fact.value["last_rejected"]["ctr"], 835);
        assert!(fact.value["last_rejected"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("bad tag"));
        assert!(
            fact.value["ctr"].is_null(),
            "a bad tag must not move the window"
        );
    }

    #[test]
    fn an_altered_payload_fails_the_tag() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let line = signed_line(ROOT, 0x40, 835, KEEPALIVE).replace("\"seq\":1", "\"seq\":2");
        let f = parse_gateway_line(&line).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Err(LoraRefused::BadTag));
    }

    #[test]
    fn an_unsigned_line_is_refused_not_tolerated() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let f = parse_gateway_line(REFLEX_LINE).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Err(LoraRefused::Unsigned));
    }

    #[test]
    fn the_same_counter_twice_is_a_replay_even_with_a_valid_tag() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let f = parse_gateway_line(&signed_line(ROOT, 0x40, 835, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Ok(()));
        assert_eq!(auth.admit(&f, &world, 1_001), Err(LoraRefused::Replayed));
        let old = parse_gateway_line(&signed_line(ROOT, 0x40, 835 - 200, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&old, &world, 1_002), Err(LoraRefused::TooOld));
        let fact = world.current("spine.auth.gw-40").unwrap().unwrap();
        assert_eq!(fact.value["accepted"], 1);
        assert_eq!(fact.value["rejected"], 2);
    }

    // ── A rejection is an incident (DECISIONS.md 2026-09-14) ─────────────────

    fn alarm_count(world: &WorldMemory) -> Option<u64> {
        world
            .current(AUTH_ALARM_COUNT_FACT)
            .unwrap()
            .and_then(|f| f.value.as_u64())
    }

    #[test]
    fn a_bad_tag_opens_one_alarm_per_burst_and_the_count_the_reflex_reads() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        assert_eq!(
            alarm_count(&world),
            None,
            "a healthy mesh has no count at all"
        );
        let other = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        // A wrong-root station keeps sending: three bad tags in a minute.
        for (i, ctr) in [835u32, 836, 837].iter().enumerate() {
            let f = parse_gateway_line(&signed_line(other, 0x40, *ctr, KEEPALIVE)).unwrap();
            assert_eq!(
                auth.admit(&f, &world, 1_000 + i as u64 * 20_000),
                Err(LoraRefused::BadTag)
            );
        }
        let alarms = world.history("spine.auth.gw-40.alarm").unwrap();
        assert_eq!(alarms.len(), 1, "one fact per burst, not per frame");
        let a = &alarms[0].value;
        assert_eq!(a["status"], json!("alarmed"));
        assert_eq!(a["station"], json!("gw-40"));
        assert_eq!(a["ctr"], json!(835));
        assert!(a["reason"].as_str().unwrap().starts_with("bad tag"));
        assert_eq!(alarm_count(&world), Some(1));
        assert_eq!(
            LoraAuth::alarmed_stations(&world).len(),
            1,
            "mesh_status sees it"
        );

        // Quiet — genuine frames from another station keep the clock going —
        // and ten minutes after the last bad tag it clears with the count.
        let good = parse_gateway_line(&signed_line(ROOT, 0xD8, 1, KEEPALIVE)).unwrap();
        assert_eq!(
            auth.admit(&good, &world, 41_000 + AUTH_ALARM_CLEAR_MS - 1),
            Ok(())
        );
        assert_eq!(alarm_count(&world), Some(1), "one ms short: still alarmed");
        let good = parse_gateway_line(&signed_line(ROOT, 0xD8, 2, KEEPALIVE)).unwrap();
        assert_eq!(
            auth.admit(&good, &world, 41_000 + AUTH_ALARM_CLEAR_MS),
            Ok(())
        );
        let alarms = world.history("spine.auth.gw-40.alarm").unwrap();
        assert_eq!(alarms.len(), 2);
        assert_eq!(alarms[1].value["status"], json!("cleared"));
        assert_eq!(
            alarms[1].value["count"],
            json!(3),
            "the burst's size travels on the clear"
        );
        assert_eq!(alarm_count(&world), Some(0));
        assert!(LoraAuth::alarmed_stations(&world).is_empty());
    }

    #[test]
    fn a_replay_alarms_but_the_post_reset_gap_and_old_firmware_do_not() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        // Old firmware: refused, no alarm.
        let f = parse_gateway_line(REFLEX_LINE).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_000), Err(LoraRefused::Unsigned));
        assert_eq!(alarm_count(&world), None);
        // A station reset's bounded gap: refused, no alarm.
        let f = parse_gateway_line(&signed_line(ROOT, 0x40, 835, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&f, &world, 1_001), Ok(()));
        let old = parse_gateway_line(&signed_line(ROOT, 0x40, 835 - 200, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&old, &world, 1_002), Err(LoraRefused::TooOld));
        assert_eq!(alarm_count(&world), None);
        assert!(world.current("spine.auth.gw-40.alarm").unwrap().is_none());
        // The same counter again with a valid tag: a replay, and an incident.
        assert_eq!(auth.admit(&f, &world, 1_003), Err(LoraRefused::Replayed));
        assert_eq!(alarm_count(&world), Some(1));
        assert!(world
            .current("spine.auth.gw-40.alarm")
            .unwrap()
            .unwrap()
            .value["reason"]
            .as_str()
            .unwrap()
            .contains("replayed"));
    }

    #[test]
    fn an_alarm_open_when_the_process_restarts_is_still_open() {
        let world = WorldMemory::open_in_memory().unwrap();
        let other = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        {
            let mut auth = LoraAuth::new(ROOT).unwrap();
            let f = parse_gateway_line(&signed_line(other, 0x40, 835, KEEPALIVE)).unwrap();
            assert_eq!(auth.admit(&f, &world, 1_000), Err(LoraRefused::BadTag));
        }
        // New process, same store: the first frame from the station resumes
        // the alarm, and it clears on the same timer from when it was written.
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let good = parse_gateway_line(&signed_line(ROOT, 0x40, 900, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&good, &world, 5_000), Ok(()));
        assert_eq!(alarm_count(&world), Some(1), "adopted, not forgotten");
        let good = parse_gateway_line(&signed_line(ROOT, 0x40, 901, KEEPALIVE)).unwrap();
        assert_eq!(
            auth.admit(&good, &world, 1_000 + AUTH_ALARM_CLEAR_MS),
            Ok(())
        );
        assert_eq!(alarm_count(&world), Some(0));
        assert_eq!(
            world.history("spine.auth.gw-40.alarm").unwrap().len(),
            2,
            "alarmed once, cleared once, across the restart"
        );
    }

    /// The host persists with M = 1: after a restart a new verifier resumes
    /// from the fact and refuses everything at or below the mark, including
    /// a replay of the very frame accepted before the restart.
    #[test]
    fn a_restarted_host_refuses_what_it_accepted_before() {
        let world = WorldMemory::open_in_memory().unwrap();
        let before = parse_gateway_line(&signed_line(ROOT, 0x40, 835, KEEPALIVE)).unwrap();
        {
            let mut auth = LoraAuth::new(ROOT).unwrap();
            assert_eq!(auth.admit(&before, &world, 1_000), Ok(()));
        }
        // Restart: a fresh verifier over the same world memory.
        let mut auth = LoraAuth::new(ROOT).unwrap();
        assert_eq!(
            auth.admit(&before, &world, 2_000),
            Err(LoraRefused::Replayed)
        );
        let never_seen = parse_gateway_line(&signed_line(ROOT, 0x40, 830, KEEPALIVE)).unwrap();
        assert_eq!(
            auth.admit(&never_seen, &world, 2_001),
            Err(LoraRefused::Replayed),
            "below the mark and unseen: the restarted host cannot know, so it refuses"
        );
        let next = parse_gateway_line(&signed_line(ROOT, 0x40, 836, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&next, &world, 2_002), Ok(()));
        let fact = world.current("spine.auth.gw-40").unwrap().unwrap();
        assert_eq!(fact.value["ctr"], 836);
        assert_eq!(fact.value["accepted"], 2, "counts carry across the restart");
        assert_eq!(fact.value["rejected"], 2);
    }

    #[test]
    fn stations_are_judged_independently() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let a = parse_gateway_line(&signed_line(ROOT, 0x40, 100, KEEPALIVE)).unwrap();
        let b = parse_gateway_line(&signed_line(ROOT, 0xD8, 100, KEEPALIVE)).unwrap();
        assert_eq!(auth.admit(&a, &world, 1), Ok(()));
        assert_eq!(auth.admit(&b, &world, 2), Ok(()));
        // A frame from D8 tagged with 40's key does not verify as D8.
        let spoof = signed_line(ROOT, 0x40, 101, KEEPALIVE).replace("src=40", "src=D8");
        let s = parse_gateway_line(&spoof).unwrap();
        assert_eq!(auth.admit(&s, &world, 3), Err(LoraRefused::BadTag));
    }

    #[test]
    fn the_verified_path_ingests_the_same_facts_as_before() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut auth = LoraAuth::new(ROOT).unwrap();
        let payload =
            r#"{"type":"reflex","node_id":"obc-esp32-s3-001","rule":"safe-link-offline"}"#;
        let f = parse_gateway_line(&signed_line(ROOT, 0x28, 30, payload)).unwrap();
        assert_eq!(auth.admit(&f, &world, 5_000), Ok(()));
        let ing = ingest_frame(&f, &world, 5_000).unwrap();
        assert_eq!(ing.node_id, "obc-esp32-s3-001");
        assert_eq!(ing.msg_type, "reflex");
        assert!(world
            .current("mesh.obc-esp32-s3-001.reflex")
            .unwrap()
            .is_some());
    }

    #[test]
    fn the_root_has_to_be_long_enough_and_come_from_exactly_one_place() {
        assert!(LoraAuth::new("short").is_err());
        assert!(LoraAuth::new(ROOT).is_ok());
        assert!(
            LoraAuth::from_config(None, None).is_err(),
            "no root: refuse to start"
        );
        assert!(
            LoraAuth::from_config(Some(ROOT), Some("x")).is_err(),
            "both: ambiguous"
        );
        assert!(LoraAuth::from_config(Some(ROOT), None).is_ok());
        assert!(LoraAuth::from_config(None, Some("/definitely/not/here")).is_err());
        let dir = std::env::temp_dir().join(format!("obc-spine-root-{}", std::process::id()));
        std::fs::write(&dir, format!("{ROOT}\n")).unwrap();
        let a = LoraAuth::from_config(None, Some(dir.to_str().unwrap())).unwrap();
        assert_eq!(
            a.fingerprint(),
            LoraAuth::new(ROOT).unwrap().fingerprint(),
            "trailing newline ignored"
        );
        let _ = std::fs::remove_file(dir);
    }

    // ── A lost spine (2026-09-13) ────────────────────────────────────────────

    #[test]
    fn reopen_backoff_doubles_from_one_second_and_stops_at_thirty() {
        let secs: Vec<u64> = (1..=9).map(|n| reopen_backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 30, 30, 30, 30]);
        assert_eq!(
            reopen_backoff(0).as_secs(),
            1,
            "a zeroth attempt is the first"
        );
        assert_eq!(
            reopen_backoff(u32::MAX).as_secs(),
            30,
            "no overflow, no unbounded wait"
        );
    }

    #[test]
    fn the_gateway_fact_round_trips_every_state() {
        let world = WorldMemory::open_in_memory().unwrap();
        assert_eq!(GatewayLink::read(&world), None, "no gateway, no fact");
        for link in [
            GatewayLink::Open {
                since_ms: 10,
                attempts: 0,
            },
            GatewayLink::Lost {
                since_ms: 20,
                error: "os error 22".into(),
            },
            GatewayLink::Reopening {
                since_ms: 20,
                error: "port busy".into(),
                attempts: 3,
                next_attempt_ms: 28_000,
            },
            GatewayLink::Open {
                since_ms: 30_000,
                attempts: 4,
            },
        ] {
            link.record(&world, "COM3", link.since_ms());
            assert_eq!(GatewayLink::read(&world), Some(link.clone()));
            let f = world.current(GATEWAY_FACT).unwrap().unwrap();
            assert_eq!(f.value["port"], json!("COM3"));
            assert_eq!(f.value["state"], json!(link.state()));
            assert_eq!(f.source, SOURCE);
            assert_eq!(f.origin, Origin::Observed);
        }
        let history = world.history(GATEWAY_FACT).unwrap();
        let states: Vec<&str> = history
            .iter()
            .map(|f| f.value["state"].as_str().unwrap())
            .collect();
        assert_eq!(states, vec!["open", "lost", "reopening", "open"]);
        assert_eq!(
            history[3].value["attempts"],
            json!(4),
            "the outage is legible from the history"
        );
    }

    // ── On-air refusals (DECISIONS.md 2026-09-15) ───────────────────────────

    /// Verbatim from the firmware's `warn!` format string —
    /// `"SPINE ◄ REJECTED src={src:02X} ctr={ctr} rssi={} dBm ({} B): {}"` —
    /// with the ANSI the serial port actually hands the host.
    const REJECTED_LINE: &str = "\u{1b}[0;33mW (98312) heltec_lora_linktest: SPINE ◄ REJECTED \
         src=40 ctr=1207 rssi=-51 dBm (63 B): bad tag (wrong root, forged, or corrupt)\u{1b}[0m";

    #[test]
    fn a_station_refusal_line_parses_and_is_not_a_frame() {
        let r = parse_gateway_refusal(REJECTED_LINE).expect("a REJECTED line parses");
        assert_eq!(r.src, 0x40);
        assert_eq!(r.ctr, Some(1207));
        assert_eq!(r.rssi_dbm, -51);
        assert!(r.bad_tag, "{}", r.reason);
        // The load-bearing half: it must never be read as a received frame.
        // It carries `src=`, `ctr=` and `rssi=`, so only the explicit marker
        // check stands between it and the ingest path.
        assert!(
            parse_gateway_line(REJECTED_LINE).is_none(),
            "a refused frame must not ingest as a received one"
        );
        // And an accepted line is not a refusal.
        assert!(parse_gateway_refusal(REFLEX_LINE).is_none());
    }

    /// Pins the firmware's wording. `Refused::as_str` lives in
    /// `firmware/heltec-lora-linktest`, a separate workspace that cannot
    /// share the constant, so the coupling is a string and this is the thing
    /// that notices when it moves. If this fails, the detector has gone
    /// silent, not wrong — check the firmware before changing the constant.
    #[test]
    fn the_firmwares_bad_tag_wording_is_the_one_we_match() {
        assert!("bad tag (wrong root, forged, or corrupt)".starts_with(STATION_BAD_TAG));
        for other in [
            "runt (not a v2 frame)",
            "seq is not the low byte of ctr",
            "counter already accepted",
            "counter older than the receive window",
            "NVS refused the receive ceiling",
        ] {
            assert!(!other.starts_with(STATION_BAD_TAG), "{other}");
        }
    }

    #[test]
    fn only_a_bad_tag_opens_a_burst() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut air = AirWatch::new();
        for reason in [
            "runt (not a v2 frame)",
            "counter older than the receive window",
            "NVS refused the receive ceiling",
        ] {
            let line = format!("SPINE ◄ REJECTED src=40 ctr=9 rssi=-51 dBm (63 B): {reason}");
            let r = parse_gateway_refusal(&line).unwrap();
            assert!(!r.bad_tag, "{reason}");
            air.observe(&r, &world, 1_000);
        }
        assert!(
            world.current("spine.air.gw-40.refused").unwrap().is_none(),
            "RF noise and the station's own bookkeeping are not forgery evidence"
        );
        assert_eq!(AirWatch::refusing_stations(&world).len(), 0);
    }

    #[test]
    fn a_flood_of_forgeries_is_one_incident_with_a_count_and_an_end() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut air = AirWatch::new();
        let r = parse_gateway_refusal(REJECTED_LINE).unwrap();

        for i in 0..50 {
            air.observe(&r, &world, 1_000 + i * 10);
        }
        // One fact for the burst, not fifty.
        let history = world.history("spine.air.gw-40.refused").unwrap();
        assert_eq!(
            history.len(),
            1,
            "one fact per burst, the count on the clear"
        );
        let open = world
            .current("spine.air.gw-40.refused")
            .unwrap()
            .expect("the burst is open");
        assert_eq!(open.value["status"], json!("refusing"));
        assert_eq!(open.value["rssi_dbm"], json!(-51));
        assert_eq!(
            open.value["evidence"],
            json!("station-asserted (unauthenticated console)"),
            "the fact says how much it is worth"
        );
        // Caught on the bench, 2026-09-15: `src` is the refused frame's
        // *claimed* origin, not the refuser. The first cut called it
        // `station`, and the log line said "gw-D8 is refusing frames" when
        // gw-D8 was the board being impersonated — an operator following that
        // goes to the wrong radio.
        assert_eq!(open.value["claimed_src"], json!("gw-40"));
        assert!(
            open.value.get("station").is_none(),
            "no field whose obvious reading is the refusing station"
        );
        assert_eq!(
            open.value["refused_by"],
            json!("the station on this host's console"),
            "the console line does not name the refuser, and the fact says so \
             rather than implying one"
        );
        assert_eq!(
            world
                .current(AIR_REFUSED_COUNT_FACT)
                .unwrap()
                .unwrap()
                .value,
            json!(1)
        );
        // The authenticated alarm is untouched: nothing here is the host's
        // own judgement, and the two signals must not be confusable.
        assert!(world.current("spine.auth.gw-40.alarm").unwrap().is_none());
        assert!(world.current(AUTH_ALARM_COUNT_FACT).unwrap().is_none());

        // Still open just before the clear window, closed on it.
        air.sweep(&world, 1_000 + 490 + AIR_REFUSED_CLEAR_MS - 1);
        assert_eq!(
            world
                .current("spine.air.gw-40.refused")
                .unwrap()
                .unwrap()
                .value["status"],
            json!("refusing")
        );
        air.sweep(&world, 1_000 + 490 + AIR_REFUSED_CLEAR_MS);
        let closed = world.current("spine.air.gw-40.refused").unwrap().unwrap();
        assert_eq!(closed.value["status"], json!("quiet"));
        assert_eq!(
            closed.value["count"],
            json!(50),
            "the count travels on the clear"
        );
        assert_eq!(
            world
                .current(AIR_REFUSED_COUNT_FACT)
                .unwrap()
                .unwrap()
                .value,
            json!(0)
        );
        assert_eq!(AirWatch::refusing_stations(&world).len(), 0);
    }

    /// A restart mid-burst must not orphan the fact. `sweep` returns early
    /// when it has nothing to expire, so a watch that came up empty would
    /// never rewrite `spine.air.refused_count` and the rule would fire on a
    /// condition that had ended — for as long as the process lived.
    #[test]
    fn a_restart_adopts_an_open_burst_instead_of_orphaning_it() {
        let world = WorldMemory::open_in_memory().unwrap();
        let r = parse_gateway_refusal(REJECTED_LINE).unwrap();
        {
            let mut air = AirWatch::new();
            air.observe(&r, &world, 1_000);
            air.observe(&r, &world, 2_000);
        } // the process dies here, mid-burst

        let mut reborn = AirWatch::new();
        reborn.resume(&world);
        assert_eq!(
            world
                .current(AIR_REFUSED_COUNT_FACT)
                .unwrap()
                .unwrap()
                .value,
            json!(1),
            "still open, and still counted, immediately after the restart"
        );
        // The clear timer runs from the fact, not from the restart.
        reborn.sweep(&world, 2_000 + AIR_REFUSED_CLEAR_MS);
        let closed = world.current("spine.air.gw-40.refused").unwrap().unwrap();
        assert_eq!(closed.value["status"], json!("quiet"));
        assert_eq!(
            world
                .current(AIR_REFUSED_COUNT_FACT)
                .unwrap()
                .unwrap()
                .value,
            json!(0),
            "the count the rule reads returns to zero"
        );
    }

    /// A forger who cycles the claimed `src` must not be able to mint an
    /// entity per id in world memory. The key is a value the attacker picks,
    /// through an input that is unauthenticated by construction, so the cap is
    /// the only thing between them and the store.
    #[test]
    fn cycling_the_claimed_source_cannot_flood_world_memory() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut air = AirWatch::new();
        for src in 0u8..=255 {
            let line = format!(
                "SPINE ◄ REJECTED src={src:02X} ctr=7 rssi=-40 dBm (63 B): \
                 bad tag (wrong root, forged, or corrupt)"
            );
            let r = parse_gateway_refusal(&line).unwrap();
            air.observe(&r, &world, 1_000 + u64::from(src));
        }
        let minted = world
            .entities()
            .unwrap()
            .into_iter()
            .filter(|e| e.starts_with(AIR_FACT_PREFIX) && e.ends_with(".refused"))
            .count();
        assert_eq!(
            minted, AIR_MAX_TRACKED_SOURCES,
            "256 claimed sources must not become 256 entities"
        );
        assert_eq!(
            world
                .current(AIR_REFUSED_COUNT_FACT)
                .unwrap()
                .unwrap()
                .value,
            json!(AIR_MAX_TRACKED_SOURCES as u64),
            "the count the rule reads stays a plain count, not count + overflow"
        );
        // The rule still fires: the operator learns something is spraying,
        // which is the more useful finding than any single id.
        assert!(AirWatch::refusing_stations(&world).len() == AIR_MAX_TRACKED_SOURCES);
    }

    /// A fake port: each open hands the test the sender side, so it can feed
    /// lines and "pull the cable" by sending `Closed`.
    struct FakePort {
        opens: Arc<std::sync::Mutex<Vec<tokio::sync::mpsc::Sender<ConsoleEvent>>>>,
        cmds: Arc<std::sync::Mutex<Vec<tokio::sync::mpsc::Receiver<String>>>>,
        /// How many opens fail before one succeeds, per outage.
        fail_first: Arc<std::sync::atomic::AtomicU32>,
    }

    impl FakePort {
        fn open(&self) -> anyhow::Result<(ConsoleLines, SerialWriterHandle)> {
            use std::sync::atomic::Ordering;
            let left = self.fail_first.load(Ordering::SeqCst);
            if left > 0 {
                self.fail_first.store(left - 1, Ordering::SeqCst);
                anyhow::bail!("failed to open LoRa gateway console COM3: Access is denied");
            }
            let (line_tx, line_rx) = tokio::sync::mpsc::channel(16);
            let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(16);
            self.opens.lock().unwrap().push(line_tx);
            self.cmds.lock().unwrap().push(cmd_rx);
            Ok((line_rx, cmd_tx))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_sink_survives_an_outage_and_the_fact_history_tells_it() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let port = FakePort {
            opens: Default::default(),
            cmds: Default::default(),
            fail_first: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        };
        let (first_rx, first_wr) = port.open().unwrap();
        // The outage to come: two reopens refused (the port still held), the third succeeds.
        port.fail_first
            .store(2, std::sync::atomic::Ordering::SeqCst);
        // A clock the test drives: tokio's paused time, in ms from an epoch.
        let t0 = tokio::time::Instant::now();
        let now_ms = move || 1_000_000 + t0.elapsed().as_millis() as u64;
        let handle = Arc::new(GatewayHandle::open(first_wr, now_ms()));
        GatewayLink::Open {
            since_ms: now_ms(),
            attempts: 0,
        }
        .record(&world, "COM3", now_ms());
        let sink: Arc<dyn CommandSink> = Arc::new(SerialCommandSink::new(Arc::clone(&handle)));

        let opens = Arc::clone(&port.opens);
        let cmds = Arc::clone(&port.cmds);
        let fail_first = Arc::clone(&port.fail_first);
        let auth = LoraAuth::new(ROOT).unwrap();
        let w = Arc::clone(&world);
        let h = Arc::clone(&handle);
        tokio::spawn(async move {
            let port = FakePort {
                opens,
                cmds,
                fail_first,
            };
            supervise_gateway(
                "COM3".into(),
                Ok(first_rx),
                move || {
                    let r = port.open();
                    async move { r }
                },
                auth,
                w,
                h,
                now_ms,
            )
            .await;
        });

        let cmd = NodeCommand::new("obc-esp32-s3-001", "a1", "capabilities", json!({}));
        // Before the loss: a send lands in the first port's command queue.
        sink.send_command(&cmd).await.unwrap();
        assert_eq!(
            port.cmds.lock().unwrap()[0].try_recv().unwrap(),
            cmd.encode()
        );

        // Pull the cable: the I/O thread reports why and exits.
        let first_lines = port.opens.lock().unwrap()[0].clone();
        first_lines
            .send(ConsoleEvent::Closed("os error 22".into()))
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        // `lost` is recorded and the first reopen is scheduled in the same breath.
        let link = GatewayLink::read(&world).unwrap();
        assert!(
            matches!(&link, GatewayLink::Reopening { error, attempts: 0, .. } if error == "os error 22"),
            "{link:?}"
        );
        // During it: the send fails fast, with the link state, and queues nothing.
        let err = sink.send_command(&cmd).await.unwrap_err().to_string();
        assert_eq!(err, "gateway reopening: os error 22");

        // 1 s: first reopen fails; 2 s more: second fails; 4 s more: third succeeds.
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let err = sink.send_command(&cmd).await.unwrap_err().to_string();
        assert!(
            err.starts_with("gateway reopening (1 failed so far): failed to open"),
            "{err}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        assert!(matches!(
            GatewayLink::read(&world).unwrap(),
            GatewayLink::Reopening { attempts: 2, .. }
        ));
        tokio::time::sleep(std::time::Duration::from_millis(4_100)).await;
        let open = GatewayLink::read(&world).unwrap();
        assert!(
            matches!(open, GatewayLink::Open { attempts: 3, .. }),
            "{open:?}"
        );
        assert_eq!(port.opens.lock().unwrap().len(), 2, "one reopen succeeded");

        // After: the *same* sink delivers into the new port.
        sink.send_command(&cmd).await.unwrap();
        assert_eq!(
            port.cmds.lock().unwrap()[1].try_recv().unwrap(),
            cmd.encode()
        );
        // And the new port's lines are verified and ingested with the same auth.
        let line = signed_line(ROOT, 0x40, 30, KEEPALIVE);
        let second_lines = port.opens.lock().unwrap()[1].clone();
        second_lines.send(ConsoleEvent::Line(line)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(world.current("mesh.gw-40").unwrap().is_some());

        let states: Vec<String> = world
            .history(GATEWAY_FACT)
            .unwrap()
            .iter()
            .map(|f| {
                format!(
                    "{}:{}",
                    f.value["state"].as_str().unwrap(),
                    f.value["attempts"]
                )
            })
            .collect();
        assert_eq!(
            states,
            vec![
                "open:0",
                "lost:0",
                "reopening:0",
                "reopening:1",
                "reopening:2",
                "open:3"
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_port_absent_at_boot_is_an_outage_from_t0_not_a_refusal() {
        // 2026-09-14 09:10: the bench was unplugged when the machine came up,
        // and the brain exited. Now the same start records `lost` with the
        // open error, the sink refuses with it, and the port is taken the
        // moment it appears.
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let port = FakePort {
            opens: Default::default(),
            cmds: Default::default(),
            fail_first: Arc::new(std::sync::atomic::AtomicU32::new(1)),
        };
        let t0 = tokio::time::Instant::now();
        let now_ms = move || 5_000_000 + t0.elapsed().as_millis() as u64;
        let boot_error =
            "failed to open LoRa gateway console COM3: The system cannot find the file specified.";
        let handle = Arc::new(GatewayHandle::lost(boot_error.into(), now_ms()));
        let sink: Arc<dyn CommandSink> = Arc::new(SerialCommandSink::new(Arc::clone(&handle)));
        let (opens, cmds, fail_first) = (
            Arc::clone(&port.opens),
            Arc::clone(&port.cmds),
            Arc::clone(&port.fail_first),
        );
        let (w, h) = (Arc::clone(&world), Arc::clone(&handle));
        tokio::spawn(async move {
            let port = FakePort {
                opens,
                cmds,
                fail_first,
            };
            supervise_gateway(
                "COM3".into(),
                Err(boot_error.into()),
                move || {
                    let r = port.open();
                    async move { r }
                },
                LoraAuth::new(ROOT).unwrap(),
                w,
                h,
                now_ms,
            )
            .await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let cmd = NodeCommand::new("obc-esp32-s3-001", "b1", "capabilities", json!({}));
        let err = sink.send_command(&cmd).await.unwrap_err().to_string();
        assert_eq!(err, format!("gateway reopening: {boot_error}"));
        // First attempt at +1 s fails (still unplugged); second at +3 s finds it.
        tokio::time::sleep(std::time::Duration::from_millis(3_100)).await;
        assert!(
            matches!(
                GatewayLink::read(&world).unwrap(),
                GatewayLink::Open { attempts: 2, .. }
            ),
            "{:?}",
            GatewayLink::read(&world)
        );
        sink.send_command(&cmd).await.unwrap();
        assert_eq!(
            port.cmds.lock().unwrap()[0].try_recv().unwrap(),
            cmd.encode()
        );
        let states: Vec<String> = world
            .history(GATEWAY_FACT)
            .unwrap()
            .iter()
            .map(|f| f.value["state"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(states, vec!["lost", "reopening", "reopening", "open"]);
    }
}

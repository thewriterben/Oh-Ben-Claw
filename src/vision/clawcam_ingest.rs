//! ClawCam detections → bitemporal world memory (S1 "Remember").
//!
//! ClawCam is Oh-Ben-Claw's vision subsystem. Its gateway produces AI
//! classifications (species, confidence, model, and a human **review state**);
//! this module folds those classifications into [`WorldMemory`] so the brain
//! *remembers* what each camera saw and when — queryable across valid time.
//!
//! Each detection becomes one observation of a subject entity
//! (`vision.subject.{species}`), so the agent can ask "when did we last see a
//! deer, and was it verified?" via [`WorldMemory::current`] /
//! [`WorldMemory::history`]. The original machine output is never mutated; the
//! review state simply rides along on the fact's value.
//!
//! ```text
//!  ClawCam gateway        adapter / MCP            Oh-Ben-Claw
//!  inference_results  ──▶  get_inference_results ──▶ ingest_clawcam_detections
//!  (label, conf,                                     └─▶ WorldMemory.observe(
//!   review_state, ran_at)                                  "vision.subject.deer", …)
//! ```

use crate::mcp::client::McpClient;
use crate::memory::world::WorldMemory;
use obc_conscience::{Conscience, PerceptionDecision, PerceptionRefusal};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// One ClawCam classification, as produced by the gateway's
/// `inference_results` rows (`get_inference_results` / `list_species_detections`).
///
/// Unknown/extra fields are ignored, and most fields are optional so partial
/// tool payloads still ingest cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawCamDetection {
    /// Gateway event this classification belongs to.
    pub event_id: String,
    /// Camera that produced it, when known.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Highest-confidence label (e.g. `"animal"`, `"deer"`).
    #[serde(default)]
    pub top_label: Option<String>,
    /// Confidence in `[0, 1]` of the top detection.
    #[serde(default)]
    pub top_confidence: Option<f64>,
    /// Resolved species/taxon of the top detection, when available.
    #[serde(default)]
    pub top_species: Option<String>,
    /// Human-review state (DATA_MODEL.md). Defaults to `unreviewed`.
    #[serde(default = "default_review_state")]
    pub review_state: String,
    /// ISO-8601 timestamp the inference ran (becomes the fact's valid-time).
    #[serde(default)]
    pub ran_at: Option<String>,
}

fn default_review_state() -> String {
    "unreviewed".to_string()
}

/// The subject a detection is about — species preferred, else label, else
/// `"unknown"`. Used to build the world-memory entity key.
pub fn detection_subject(d: &ClawCamDetection) -> String {
    d.top_species
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(d.top_label.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

/// Lowercase, underscore-separated slug for an entity key segment.
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_us = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parse an ISO-8601 / RFC-3339 timestamp to milliseconds since the epoch.
fn parse_iso_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
        .filter(|ms| *ms >= 0)
        .map(|ms| ms as u64)
}

/// World-memory entity key for a detection subject.
pub fn subject_entity(d: &ClawCamDetection) -> String {
    format!("vision.subject.{}", slug(&detection_subject(d)))
}

/// Fold ClawCam detections into [`WorldMemory`] as bitemporal observations.
///
/// `ingested_at_ms` is the transaction time (when the brain learned of these);
/// each fact's valid-time comes from the detection's `ran_at` (falling back to
/// `ingested_at_ms` when absent or unparseable). Returns the entity key written
/// for each detection, in order.
pub fn ingest_clawcam_detections(
    world: &WorldMemory,
    detections: &[ClawCamDetection],
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let mut entities = Vec::with_capacity(detections.len());
    for d in detections {
        let entity = subject_entity(d);
        // Idempotent on `event_id`. A poll re-reads the source, so the same events come
        // back every cycle, and recording them again is not a second sighting of the
        // same animal — it is the same sighting, counted twice.
        //
        // Measured on the bench store before this check existed: 50 distinct events
        // written 9,600 times over six hours of polling, `vision.count.white_tailed_deer`
        // reading 5,376 from roughly 17 real deer, and the vision reflex escalating to
        // the 30B model every hour on detections from 6 July because each re-write reset
        // the belief's age. Three separate symptoms, one cause.
        //
        // A detection with no `event_id` cannot be deduplicated and is written through:
        // dropping it would silently lose data from any source that does not supply one.
        if !d.event_id.is_empty() && world.has_value_field(&entity, "event_id", &d.event_id)? {
            continue;
        }
        let valid_from = d
            .ran_at
            .as_deref()
            .and_then(parse_iso_ms)
            .unwrap_or(ingested_at_ms);
        let value = json!({
            "event_id": d.event_id,
            "device_id": d.device_id,
            "label": d.top_label,
            "species": d.top_species,
            "confidence": d.top_confidence,
            "review_state": d.review_state,
        });
        world.observe(&entity, value, valid_from, ingested_at_ms, source)?;
        entities.push(entity);
    }
    Ok(entities)
}

/// Result of applying the conscience perception gate to a batch of detections.
pub struct ConscienceFilter {
    /// Detections permitted by the consent registry — safe to ingest.
    pub kept: Vec<ClawCamDetection>,
    /// Refused detections, with the reason. Dropped before world memory and the
    /// reasoner; returned so a caller with an audit log can record them.
    pub refused: Vec<(String, PerceptionRefusal)>,
}

/// Apply the conscience perception gate (consent registry) to detections
/// **before** they reach world memory or the reasoner — the Track 0 pattern
/// for perception (`obc_conscience`). A detection whose subject is not
/// permitted — a human on a wildlife body, an unlisted class, or a
/// low-confidence label that might be a person — is dropped here: never stored,
/// never seen by the model, so it can neither be kept nor inject the reasoner.
///
/// Keyed on the classifier's own subject label (`detection_subject`), so the
/// operator declares the labels their classifier emits (e.g. allow `"animal"`,
/// deny `"person"`). A detection with no confidence value fails closed.
/// Refusals are logged (first-class) and returned for the audit log. Mapping a
/// species taxonomy onto broader consent classes is future work (see
/// CONSCIENCE.md §6).
pub fn conscience_filter(detections: &[ClawCamDetection], conscience: &Conscience) -> ConscienceFilter {
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for d in detections {
        let subject = detection_subject(d);
        // No confidence on the detection → 0.0 → the gate fails closed.
        let confidence = d.top_confidence.unwrap_or(0.0) as f32;
        match conscience.may_perceive(&subject, confidence) {
            PerceptionDecision::Allow { .. } => kept.push(d.clone()),
            PerceptionDecision::Refuse(reason) => {
                tracing::warn!(subject = %subject, refusal = %reason,
                               "conscience: perception refused (frame dropped, not stored)");
                refused.push((subject, reason));
            }
        }
    }
    ConscienceFilter { kept, refused }
}

/// Conscience-gated ingest: filter detections through the perception gate, then
/// fold only the permitted ones into world memory. Drop-in replacement for
/// [`ingest_clawcam_detections`] wherever a [`Conscience`] is available. Returns
/// the ingested entity keys and the refusals (for the audit log).
pub fn ingest_clawcam_detections_gated(
    world: &WorldMemory,
    detections: &[ClawCamDetection],
    ingested_at_ms: u64,
    source: &str,
    conscience: &Conscience,
) -> anyhow::Result<(Vec<String>, Vec<(String, PerceptionRefusal)>)> {
    let filtered = conscience_filter(detections, conscience);
    let entities = ingest_clawcam_detections(world, &filtered.kept, ingested_at_ms, source)?;
    Ok((entities, filtered.refused))
}

/// Convenience: parse a JSON array of detections (e.g. the body of a
/// `get_inference_results` tool result) and ingest them.
pub fn ingest_clawcam_json(
    world: &WorldMemory,
    detections_json: &serde_json::Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let detections: Vec<ClawCamDetection> = serde_json::from_value(detections_json.clone())?;
    ingest_clawcam_detections(world, &detections, ingested_at_ms, source)
}

/// Pull `ClawCamDetection`s out of a ClawCam tool result, tolerating the shapes
/// the gateway detection tools return:
///   * `{"results": [ … ]}`  — `list_species_detections`
///   * `{"result": { … }}`   — `get_inference_results`
///   * a bare JSON array, or a single detection object with an `event_id`.
///
/// Error/empty results (`{"ok": false, …}`) yield no detections.
fn extract_detections(tool_result: &serde_json::Value) -> Vec<ClawCamDetection> {
    let raw: Vec<serde_json::Value> =
        if let Some(arr) = tool_result.get("results").and_then(|r| r.as_array()) {
            arr.clone()
        } else if let Some(res) = tool_result.get("result") {
            match res.as_array() {
                Some(arr) => arr.clone(),
                None => vec![res.clone()],
            }
        } else if let Some(arr) = tool_result.as_array() {
            arr.clone()
        } else if tool_result.get("event_id").is_some() {
            vec![tool_result.clone()]
        } else {
            Vec::new()
        };
    raw.into_iter()
        .filter_map(|v| serde_json::from_value::<ClawCamDetection>(v).ok())
        .collect()
}

/// Auto-ingest primitive: fold a ClawCam detection **tool result** directly into
/// world memory, accepting any of the gateway detection-tool result shapes (see
/// [`extract_detections`]). This is what a perception poll calls after invoking
/// a ClawCam MCP tool.
pub fn ingest_tool_result(
    world: &WorldMemory,
    tool_result: &serde_json::Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let detections = extract_detections(tool_result);
    ingest_clawcam_detections(world, &detections, ingested_at_ms, source)
}

/// Poll a ClawCam detection tool over a live MCP connection and fold the result
/// into world memory in one step — the perception → memory seam.
///
/// Typical call: `tool = "list_species_detections"`, `args = {"min_confidence": 0.5}`.
/// The MCP client lock is released before the (synchronous) ingest runs.
pub async fn poll_clawcam_into_world(
    client: Arc<Mutex<McpClient>>,
    world: &WorldMemory,
    tool: &str,
    args: serde_json::Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let raw = {
        let mut guard = client.lock().await;
        guard.call_tool(tool, args).await?
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw)?;
    ingest_tool_result(world, &parsed, ingested_at_ms, source)
}

/// [`ingest_tool_result`] with the conscience perception gate applied: refused
/// detections (humans, unlisted classes, low-confidence) are dropped before
/// world memory and the reasoner. Returns ingested entities and the refusals.
pub fn ingest_tool_result_gated(
    world: &WorldMemory,
    tool_result: &serde_json::Value,
    ingested_at_ms: u64,
    source: &str,
    conscience: &Conscience,
) -> anyhow::Result<(Vec<String>, Vec<(String, PerceptionRefusal)>)> {
    let detections = extract_detections(tool_result);
    ingest_clawcam_detections_gated(world, &detections, ingested_at_ms, source, conscience)
}

/// [`poll_clawcam_into_world`] with the conscience perception gate applied — the
/// runtime-default perception path. When conscience is disabled the gate admits
/// everything, so this is a safe drop-in for the plain poll. Returns the
/// ingested entity keys (refusals are logged inside the gate).
pub async fn poll_clawcam_into_world_gated(
    client: Arc<Mutex<McpClient>>,
    world: &WorldMemory,
    tool: &str,
    args: serde_json::Value,
    ingested_at_ms: u64,
    source: &str,
    conscience: &Conscience,
) -> anyhow::Result<Vec<String>> {
    let raw = {
        let mut guard = client.lock().await;
        guard.call_tool(tool, args).await?
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw)?;
    let (entities, _refused) =
        ingest_tool_result_gated(world, &parsed, ingested_at_ms, source, conscience)?;
    Ok(entities)
}

// ── ClawCam as a full perceive subsystem (health / audio / state) ───────────────
//
// A ClawCam node is more than a species feed: it reports its own *health*
// (including battery), classifies *audio* (glassbreak, BirdNET), and tracks
// *device state*. These fold into OBC's existing suites so a camera's battery is a
// managed power source, a glassbreak is an audio-suite alarm, and a camera going
// offline is a comms event — each then visible to reflexes, safing, and foresight.

use crate::audio::suite::HeardEvent;
use crate::comms::LinkReading;
use crate::power::{BatteryReading, ChargeState};

/// A ClawCam node-health row (from `get_node_health`). Tolerant: every field but
/// the id is optional, so partial payloads still parse.
#[derive(Debug, Clone, Deserialize)]
pub struct ClawCamNodeHealth {
    #[serde(alias = "device_id")]
    pub node_id: String,
    #[serde(default)]
    pub battery_pct: Option<f64>,
    #[serde(default)]
    pub charging: Option<bool>,
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default)]
    pub rssi_dbm: Option<f64>,
    #[serde(default)]
    pub last_seen_ms: Option<u64>,
}

/// A ClawCam audio classification (from `list_audio_classifications`).
#[derive(Debug, Clone, Deserialize)]
pub struct ClawCamAudioClass {
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub top_label: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// Node health → a [`BatteryReading`] for the power suite (so a camera's pack is a
/// managed source that can drive `power.mode`, safing, and foresight). `None` when
/// the node reports no battery.
pub fn node_health_to_battery(h: &ClawCamNodeHealth) -> Option<BatteryReading> {
    h.battery_pct.map(|soc| BatteryReading {
        soc_pct: soc,
        voltage: None,
        current_a: None,
        charging: if h.charging == Some(true) {
            ChargeState::Charging
        } else {
            ChargeState::Discharging
        },
        source: Some(format!("clawcam:{}", h.node_id)),
    })
}

/// Node health → a [`LinkReading`] for the comms suite (a camera's reachability
/// becomes `link.clawcam:{node}` and feeds `net.mode`).
pub fn node_health_to_link(h: &ClawCamNodeHealth) -> LinkReading {
    LinkReading {
        link: format!("clawcam:{}", h.node_id),
        rssi_dbm: h.rssi_dbm,
        latency_ms: None,
        loss_pct: None,
        up: h.online,
        source: Some(h.node_id.clone()),
    }
}

/// An audio classification → a [`HeardEvent`] for the audio suite (a glassbreak or
/// birdsong becomes `audio.clawcam:{node}`, classifiable as an alarm by safing).
pub fn audio_class_to_event(a: &ClawCamAudioClass) -> HeardEvent {
    let node = a.device_id.clone().unwrap_or_else(|| "unknown".to_string());
    HeardEvent {
        stream: format!("clawcam:{node}"),
        text: None,
        label: a.top_label.clone().or_else(|| a.label.clone()),
        confidence: a.confidence.unwrap_or(0.0),
        source: a.device_id.clone(),
    }
}

/// Pull `ClawCamNodeHealth` rows out of a `get_node_health` tool result, tolerating
/// the same shapes as detections (`{results:[…]}`, `{result:…}`, bare array/object).
pub fn extract_node_health(tool_result: &serde_json::Value) -> Vec<ClawCamNodeHealth> {
    extract_rows(tool_result, "node_id")
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Pull `ClawCamAudioClass` rows out of a `list_audio_classifications` result.
pub fn extract_audio_classes(tool_result: &serde_json::Value) -> Vec<ClawCamAudioClass> {
    extract_rows(tool_result, "device_id")
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Shared row extraction across the gateway result shapes. `id_key` is a field that
/// identifies a bare single-object payload.
fn extract_rows(tool_result: &serde_json::Value, id_key: &str) -> Vec<serde_json::Value> {
    if let Some(arr) = tool_result.get("results").and_then(|r| r.as_array()) {
        arr.clone()
    } else if let Some(res) = tool_result.get("result") {
        match res.as_array() {
            Some(arr) => arr.clone(),
            None => vec![res.clone()],
        }
    } else if let Some(arr) = tool_result.as_array() {
        arr.clone()
    } else if tool_result.get(id_key).is_some() {
        vec![tool_result.clone()]
    } else {
        Vec::new()
    }
}

/// Record ClawCam node health into world memory as `clawcam.node.{id}` facts (for
/// observability and querying), and return the node ids written. Pair with the
/// converters above to also drive the power/comms suites.
pub fn ingest_node_health(
    world: &WorldMemory,
    health: &[ClawCamNodeHealth],
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let mut ids = Vec::with_capacity(health.len());
    for h in health {
        let valid_from = h.last_seen_ms.unwrap_or(ingested_at_ms);
        world.observe(
            &format!("clawcam.node.{}", h.node_id),
            json!({
                "battery_pct": h.battery_pct,
                "charging": h.charging,
                "online": h.online,
                "rssi_dbm": h.rssi_dbm,
            }),
            valid_from,
            ingested_at_ms,
            source,
        )?;
        ids.push(h.node_id.clone());
    }
    Ok(ids)
}

/// Bump a per-subject rolling sighting counter (`vision.count.{subject}`) so the
/// foresight layer can fit a *detection-rate* trend (e.g. intrusions accelerating).
/// `subjects` are the entity keys returned by [`ingest_clawcam_detections`]; the
/// `vision.subject.` prefix is stripped to form the count key. Returns each new
/// count.
pub fn record_subject_counts(
    world: &WorldMemory,
    subjects: &[String],
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<u64>> {
    let mut out = Vec::with_capacity(subjects.len());
    for entity in subjects {
        let subject = entity.strip_prefix("vision.subject.").unwrap_or(entity);
        let key = format!("vision.count.{subject}");
        let prev = world
            .current(&key)?
            .and_then(|f| f.value.get("value").and_then(|v| v.as_u64()))
            .unwrap_or(0);
        let next = prev + 1;
        // Deliberately written with *no* in-list, unlike every other derived fact in the
        // codebase. A running total's real support is every detection that ever
        // incremented it — 5,110 of them for white-tailed deer on the bench store — and
        // the honest encoding of that is either an unusable list or a chain of 5,110
        // links, each fact resting on its predecessor. Walking such a chain is one
        // `dependents` query per link, at startup, before the gateway binds.
        //
        // Recording `[prev]` alone would be worse than nothing: it looks like support
        // while carrying almost none of the actual dependency, and it would let a single
        // retraction thousands of links back unwind the whole history in a boot path.
        //
        // So the counter says "unknown support", which is true, and is inert to every
        // sweep. An accumulator is the case where an in-list does not fit; the mechanism
        // is for facts computed from a bounded set of current readings.
        world.observe(
            &key,
            json!({ "value": next }),
            ingested_at_ms,
            ingested_at_ms,
            source,
        )?;
        out.push(next);
    }
    Ok(out)
}

/// One-time correction for counters inflated by re-ingested events.
///
/// Recounts each `vision.count.<subject>` as the number of **distinct** `event_id`s ever
/// recorded under the matching `vision.subject.<subject>`, and writes the true value.
///
/// This exists because the counter is an accumulator: it was incremented once per
/// ingest, and before ingest was idempotent that meant once per poll per event. Nothing
/// else can repair it — supersession only ever writes `prev + 1`, so the error compounds
/// rather than washing out.
///
/// Corrects rather than deletes. The inflated values stay in `history()` as superseded
/// rows, so "we used to think there had been 5,376 deer" remains answerable, which is the
/// same rule every other withdrawal here follows.
///
/// Returns `(subject, old, new)` for every counter it changed. Idempotent: a second run
/// finds the values already correct and writes nothing.
pub fn recount_subjects(
    world: &WorldMemory,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<(String, u64, u64)>> {
    let mut fixed = Vec::new();
    for entity in world.entities()? {
        let Some(subject) = entity.strip_prefix("vision.subject.") else {
            continue;
        };
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut without_id = 0u64;
        for f in world.history(&entity)? {
            match f.value.get("event_id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => {
                    seen.insert(id.to_string());
                }
                // No id means it could not be deduplicated on the way in either, so each
                // write really was a separate report as far as anything here can tell.
                _ => without_id += 1,
            }
        }
        let truth = seen.len() as u64 + without_id;
        let key = format!("vision.count.{subject}");
        let old = world
            .current(&key)?
            .and_then(|f| f.value.get("value").and_then(|v| v.as_u64()))
            .unwrap_or(0);
        if old != truth {
            world.observe(
                &key,
                json!({ "value": truth }),
                ingested_at_ms,
                ingested_at_ms,
                source,
            )?;
            fixed.push((subject.to_string(), old, truth));
        }
    }
    Ok(fixed)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod recount_tests {
    use super::*;
    use crate::memory::world::WorldMemory;

    #[test]
    fn recount_replaces_an_inflated_counter_with_the_distinct_event_count() {
        let w = WorldMemory::open_in_memory().unwrap();
        // Three distinct events, written the way a non-idempotent poll would: repeatedly.
        for round in 0..4u64 {
            for ev in ["evt-1", "evt-2", "evt-3"] {
                w.observe(
                    "vision.subject.red_fox",
                    json!({ "event_id": ev }),
                    1_000 + round,
                    1_000 + round,
                    "clawcam",
                )
                .unwrap();
            }
        }
        w.observe(
            "vision.count.red_fox",
            json!({ "value": 12 }),
            1_000,
            1_000,
            "clawcam",
        )
        .unwrap();

        let fixed = recount_subjects(&w, 9_000, "clawcam").unwrap();
        assert_eq!(fixed, vec![("red_fox".to_string(), 12, 3)]);
        assert_eq!(
            w.current("vision.count.red_fox").unwrap().unwrap().value["value"],
            json!(3)
        );
        // Corrected, not erased: what we used to think is still answerable.
        assert_eq!(
            w.at("vision.count.red_fox", 5_000).unwrap().unwrap().value["value"],
            json!(12)
        );

        // Idempotent.
        assert!(recount_subjects(&w, 10_000, "clawcam").unwrap().is_empty());
    }

    #[test]
    fn detections_without_an_event_id_each_count_once() {
        // They could not be deduplicated on the way in, so as far as anything here can
        // tell each write was a separate report. Collapsing them would invent a merge.
        let w = WorldMemory::open_in_memory().unwrap();
        for i in 0..3 {
            w.observe(
                "vision.subject.coyote",
                json!({ "n": i }),
                1_000 + i,
                1_000 + i,
                "clawcam",
            )
            .unwrap();
        }
        let fixed = recount_subjects(&w, 9_000, "clawcam").unwrap();
        assert_eq!(fixed, vec![("coyote".to_string(), 0, 3)]);
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;
    use crate::memory::world::WorldMemory;

    fn det(event_id: &str, species: &str, ran_at: &str) -> ClawCamDetection {
        serde_json::from_value(json!({
            "event_id": event_id,
            "top_species": species,
            "top_label": species,
            "top_confidence": 0.9,
            "review_state": "verified",
            "ran_at": ran_at,
        }))
        .unwrap()
    }

    #[test]
    fn re_polling_the_same_events_records_them_once() {
        // The bench failure, in miniature: a poll re-reads its source, so the same
        // events come back every cycle. Before this, six hours of polling turned 50
        // events into 9,600 rows.
        let w = WorldMemory::open_in_memory().unwrap();
        let batch = vec![
            det("evt-1", "white_tailed_deer", "2026-07-06T11:00:00Z"),
            det("evt-2", "white_tailed_deer", "2026-07-06T11:30:00Z"),
            det("evt-3", "red_fox", "2026-07-06T12:00:00Z"),
        ];

        let first = ingest_clawcam_detections(&w, &batch, 1_000, "clawcam").unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(w.count().unwrap(), 3);

        for tick in 1..=5 {
            let again =
                ingest_clawcam_detections(&w, &batch, 1_000 + tick * 60_000, "clawcam").unwrap();
            assert!(again.is_empty(), "poll {tick} re-recorded a known event");
        }
        assert_eq!(w.count().unwrap(), 3, "five more polls wrote nothing");

        // And the belief keeps its original age, which is what stops a freshness-keyed
        // reflex escalating on July data forever.
        let deer = w
            .current("vision.subject.white_tailed_deer")
            .unwrap()
            .unwrap();
        assert_eq!(deer.ingested_at, 1_000);
    }

    #[test]
    fn a_genuinely_new_event_still_lands() {
        let w = WorldMemory::open_in_memory().unwrap();
        let batch = vec![det("evt-1", "red_fox", "2026-07-06T12:00:00Z")];
        ingest_clawcam_detections(&w, &batch, 1_000, "clawcam").unwrap();

        let next = vec![
            det("evt-1", "red_fox", "2026-07-06T12:00:00Z"), // seen
            det("evt-9", "red_fox", "2026-07-07T09:00:00Z"), // new
        ];
        let wrote = ingest_clawcam_detections(&w, &next, 2_000, "clawcam").unwrap();
        assert_eq!(wrote.len(), 1, "only the new one");
        assert_eq!(
            w.current("vision.subject.red_fox").unwrap().unwrap().value["event_id"],
            json!("evt-9")
        );
    }

    #[test]
    fn a_detection_without_an_event_id_is_written_through() {
        // Cannot be deduplicated, and dropping it would silently lose data from a source
        // that does not supply ids. Duplicates are recoverable; missing detections are not.
        let w = WorldMemory::open_in_memory().unwrap();
        let batch = vec![det("", "coyote", "2026-07-06T12:00:00Z")];
        ingest_clawcam_detections(&w, &batch, 1_000, "clawcam").unwrap();
        ingest_clawcam_detections(&w, &batch, 2_000, "clawcam").unwrap();
        assert_eq!(w.history("vision.subject.coyote").unwrap().len(), 2);
    }

    #[test]
    fn the_same_event_under_a_different_subject_is_not_suppressed() {
        // Dedup is per entity, not global: one event that yields two subjects is two
        // beliefs, and collapsing them would lose one.
        let w = WorldMemory::open_in_memory().unwrap();
        let batch = vec![
            det("evt-1", "red_fox", "2026-07-06T12:00:00Z"),
            det("evt-1", "coyote", "2026-07-06T12:00:00Z"),
        ];
        let wrote = ingest_clawcam_detections(&w, &batch, 1_000, "clawcam").unwrap();
        assert_eq!(wrote.len(), 2);
    }
}

/// The camera that produced the detection recorded under `entity`, if the fact
/// carries one.
///
/// `poll_clawcam_into_world` writes `device_id` into the fact value (see the
/// json! block above) but returns only entity names, so the camera identity is
/// in the world model rather than in the return value. Spatial fusion needs it
/// to know *where* the detection happened, and reading it back here keeps the
/// poll signature — and its callers and tests — untouched.
///
/// Returns `None` for an unknown entity, a fact with no `device_id`, or a read
/// error: a missing camera means no hazard is stamped, which is the safe
/// direction to fail. It is not silent — the caller logs only when cells are
/// actually marked, so a camera that never resolves shows up as a hazard that
/// never appears.
pub fn detection_node_of(world: &WorldMemory, entity: &str) -> Option<String> {
    let fact = world.current(entity).ok().flatten()?;
    fact.value
        .get("device_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(
        event: &str,
        species: Option<&str>,
        conf: f64,
        review: &str,
        ran_at: Option<&str>,
    ) -> ClawCamDetection {
        ClawCamDetection {
            event_id: event.to_string(),
            device_id: Some("node-1".to_string()),
            top_label: Some("animal".to_string()),
            top_confidence: Some(conf),
            top_species: species.map(|s| s.to_string()),
            review_state: review.to_string(),
            ran_at: ran_at.map(|s| s.to_string()),
        }
    }

    #[test]
    fn subject_prefers_species_then_label() {
        assert_eq!(
            subject_entity(&det("e", Some("Deer"), 0.9, "unreviewed", None)),
            "vision.subject.deer"
        );
        let mut d = det("e", None, 0.9, "unreviewed", None);
        d.top_label = Some("Wild Boar".to_string());
        assert_eq!(subject_entity(&d), "vision.subject.wild_boar");
    }

    #[test]
    fn ingest_records_current_fact_with_review_state() {
        let world = WorldMemory::open_in_memory().unwrap();
        let dets = vec![det(
            "evt-1",
            Some("deer"),
            0.91,
            "verified",
            Some("2026-06-05T12:00:00Z"),
        )];
        let entities = ingest_clawcam_detections(&world, &dets, 9_999, "clawcam").unwrap();
        assert_eq!(entities, vec!["vision.subject.deer"]);

        let fact = world.current("vision.subject.deer").unwrap().unwrap();
        assert_eq!(fact.value["review_state"], "verified");
        assert_eq!(fact.value["event_id"], "evt-1");
        assert!((fact.value["confidence"].as_f64().unwrap() - 0.91).abs() < 1e-9);
        assert_eq!(fact.source, "clawcam");
        // valid_from comes from ran_at (2026-06-05T12:00:00Z), not ingested_at.
        assert_eq!(fact.valid_from, 1_780_660_800_000);
        assert_eq!(fact.ingested_at, 9_999);
    }

    #[test]
    fn missing_ran_at_falls_back_to_ingested_at() {
        let world = WorldMemory::open_in_memory().unwrap();
        let dets = vec![det("evt-2", Some("fox"), 0.5, "unreviewed", None)];
        ingest_clawcam_detections(&world, &dets, 5_000, "clawcam").unwrap();
        let fact = world.current("vision.subject.fox").unwrap().unwrap();
        assert_eq!(fact.valid_from, 5_000);
    }

    #[test]
    fn successive_sightings_build_history() {
        let world = WorldMemory::open_in_memory().unwrap();
        ingest_clawcam_detections(
            &world,
            &[det(
                "e1",
                Some("deer"),
                0.8,
                "unreviewed",
                Some("2026-06-05T12:00:00Z"),
            )],
            1,
            "clawcam",
        )
        .unwrap();
        ingest_clawcam_detections(
            &world,
            &[det(
                "e2",
                Some("deer"),
                0.95,
                "verified",
                Some("2026-06-06T12:00:00Z"),
            )],
            2,
            "clawcam",
        )
        .unwrap();
        let history = world.history("vision.subject.deer").unwrap();
        assert_eq!(history.len(), 2);
        // The latest belief is the verified, higher-confidence sighting.
        let current = world.current("vision.subject.deer").unwrap().unwrap();
        assert_eq!(current.value["event_id"], "e2");
        assert_eq!(current.value["review_state"], "verified");
    }

    #[test]
    fn json_array_ingests() {
        let world = WorldMemory::open_in_memory().unwrap();
        let body = serde_json::json!([
            {"event_id": "e1", "top_species": "deer", "top_confidence": 0.7, "review_state": "needs_review"},
            {"event_id": "e2", "top_label": "bird"}
        ]);
        let entities = ingest_clawcam_json(&world, &body, 1_000, "clawcam").unwrap();
        assert_eq!(entities, vec!["vision.subject.deer", "vision.subject.bird"]);
        // Defaulted review state on the second (no review_state given).
        let bird = world.current("vision.subject.bird").unwrap().unwrap();
        assert_eq!(bird.value["review_state"], "unreviewed");
    }

    #[test]
    fn tool_result_results_array_shape_ingests() {
        // list_species_detections shape: {"ok", "count", "results": [...]}.
        let world = WorldMemory::open_in_memory().unwrap();
        let tr = serde_json::json!({
            "ok": true,
            "count": 1,
            "results": [
                {"event_id": "e1", "top_species": "deer", "top_confidence": 0.9,
                 "review_state": "verified", "ran_at": "2026-06-05T12:00:00Z"}
            ],
        });
        let ents = ingest_tool_result(&world, &tr, 1, "clawcam").unwrap();
        assert_eq!(ents, vec!["vision.subject.deer"]);
        let fact = world.current("vision.subject.deer").unwrap().unwrap();
        assert_eq!(fact.value["review_state"], "verified");
        assert_eq!(fact.valid_from, 1_780_660_800_000);
    }

    #[test]
    fn tool_result_single_result_object_shape_ingests() {
        // get_inference_results shape: {"ok", "event_id", "result": {...}}.
        let world = WorldMemory::open_in_memory().unwrap();
        let tr = serde_json::json!({
            "ok": true,
            "event_id": "e9",
            "result": {"event_id": "e9", "top_label": "bird", "top_confidence": 0.6},
        });
        let ents = ingest_tool_result(&world, &tr, 5, "clawcam").unwrap();
        assert_eq!(ents, vec!["vision.subject.bird"]);
        assert_eq!(
            world
                .current("vision.subject.bird")
                .unwrap()
                .unwrap()
                .valid_from,
            5
        );
    }

    #[test]
    fn tool_result_error_or_empty_ingests_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let err = serde_json::json!({"ok": false, "error": "no inference result"});
        assert!(ingest_tool_result(&world, &err, 1, "clawcam")
            .unwrap()
            .is_empty());
        let empty = serde_json::json!({"ok": true, "count": 0, "results": []});
        assert!(ingest_tool_result(&world, &empty, 1, "clawcam")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn node_health_converts_to_battery_and_link() {
        let h = ClawCamNodeHealth {
            node_id: "cam-3".into(),
            battery_pct: Some(42.0),
            charging: Some(false),
            online: Some(true),
            rssi_dbm: Some(-61.0),
            last_seen_ms: Some(9_000),
        };
        let batt = node_health_to_battery(&h).expect("battery present");
        assert!((batt.soc_pct - 42.0).abs() < 1e-9);
        assert_eq!(batt.charging, ChargeState::Discharging);
        assert_eq!(batt.source.as_deref(), Some("clawcam:cam-3"));

        let link = node_health_to_link(&h);
        assert_eq!(link.link, "clawcam:cam-3");
        assert_eq!(link.up, Some(true));
        assert_eq!(link.rssi_dbm, Some(-61.0));
    }

    #[test]
    fn node_without_battery_yields_no_reading() {
        let h = ClawCamNodeHealth {
            node_id: "cam-x".into(),
            battery_pct: None,
            charging: None,
            online: Some(false),
            rssi_dbm: None,
            last_seen_ms: None,
        };
        assert!(node_health_to_battery(&h).is_none());
        // an offline node still produces a link reading (down)
        assert_eq!(node_health_to_link(&h).up, Some(false));
    }

    #[test]
    fn audio_classification_converts_to_heard_event() {
        let a = ClawCamAudioClass {
            device_id: Some("cam-1".into()),
            top_label: Some("glassbreak".into()),
            label: None,
            confidence: Some(0.88),
        };
        let ev = audio_class_to_event(&a);
        assert_eq!(ev.stream, "clawcam:cam-1");
        assert_eq!(ev.label.as_deref(), Some("glassbreak"));
        assert!((ev.confidence - 0.88).abs() < 1e-9);
    }

    #[test]
    fn node_health_tool_result_ingests_to_world() {
        let world = WorldMemory::open_in_memory().unwrap();
        let tr = serde_json::json!({
            "ok": true,
            "results": [
                {"node_id": "cam-1", "battery_pct": 55.0, "online": true, "rssi_dbm": -50.0},
                {"device_id": "cam-2", "battery_pct": 12.0, "online": false}
            ]
        });
        let rows = extract_node_health(&tr);
        assert_eq!(rows.len(), 2);
        let ids = ingest_node_health(&world, &rows, 1_000, "clawcam").unwrap();
        assert_eq!(ids, vec!["cam-1", "cam-2"]);
        let cam2 = world.current("clawcam.node.cam-2").unwrap().unwrap();
        assert_eq!(cam2.value["online"], false);
        assert!((cam2.value["battery_pct"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    }

    #[test]
    fn subject_counts_accumulate_for_foresight() {
        let world = WorldMemory::open_in_memory().unwrap();
        let subjects = vec!["vision.subject.person".to_string()];
        let c1 = record_subject_counts(&world, &subjects, 1_000, "clawcam").unwrap();
        let c2 = record_subject_counts(&world, &subjects, 2_000, "clawcam").unwrap();
        assert_eq!(c1, vec![1]);
        assert_eq!(c2, vec![2]);
        // the trendable count fact exists for foresight to fit
        let fact = world.current("vision.count.person").unwrap().unwrap();
        assert_eq!(fact.value["value"], 2);
    }

    // -- conscience perception gate at the ingest boundary --

    fn wildlife_conscience() -> Conscience {
        use obc_conscience::{ConscienceConfig, ConsentRule, Transmit};
        Conscience::new(&ConscienceConfig {
            enabled: true,
            subjects: vec![
                ConsentRule::allow("deer", 30, Transmit::WeightsOnly),
                ConsentRule::allow("animal", 30, Transmit::WeightsOnly),
                ConsentRule::deny("person"),
            ],
            confidence_threshold: 0.6,
            ..Default::default()
        })
    }

    #[test]
    fn conscience_filter_drops_humans_and_keeps_wildlife() {
        let c = wildlife_conscience();
        let dets = vec![
            det("e1", Some("deer"), 0.95, "unreviewed", None),
            det("e2", Some("person"), 0.99, "unreviewed", None), // denied
        ];
        let out = conscience_filter(&dets, &c);
        assert_eq!(out.kept.len(), 1);
        assert_eq!(out.kept[0].event_id, "e1");
        assert_eq!(out.refused.len(), 1);
        assert_eq!(out.refused[0].0, "person");
    }

    #[test]
    fn conscience_filter_fails_closed_on_low_confidence() {
        let c = wildlife_conscience();
        // a low-confidence "deer" might be a person — refuse before ingest
        let dets = vec![det("e", Some("deer"), 0.40, "unreviewed", None)];
        let out = conscience_filter(&dets, &c);
        assert!(out.kept.is_empty());
        assert!(matches!(out.refused[0].1, PerceptionRefusal::LowConfidence { .. }));
    }

    #[test]
    fn gated_ingest_writes_only_permitted_subjects() {
        let world = WorldMemory::open_in_memory().unwrap();
        let c = wildlife_conscience();
        let dets = vec![
            det("e1", Some("deer"), 0.95, "unreviewed", None),
            det("e2", Some("person"), 0.99, "unreviewed", None),
        ];
        let (entities, refused) =
            ingest_clawcam_detections_gated(&world, &dets, 1_000, "test", &c).unwrap();
        assert_eq!(entities, vec!["vision.subject.deer".to_string()]);
        assert_eq!(refused.len(), 1);
        // the person was never written to world memory
        assert!(world.current("vision.subject.person").unwrap().is_none());
    }

    #[test]
    fn disabled_conscience_lets_everything_through() {
        use obc_conscience::ConscienceConfig;
        let c = Conscience::new(&ConscienceConfig::default()); // disabled
        let dets = vec![det("e", Some("person"), 0.99, "unreviewed", None)];
        assert_eq!(conscience_filter(&dets, &c).kept.len(), 1);
    }
}

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn det(event: &str, species: Option<&str>, conf: f64, review: &str, ran_at: Option<&str>) -> ClawCamDetection {
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
        assert_eq!(subject_entity(&det("e", Some("Deer"), 0.9, "unreviewed", None)), "vision.subject.deer");
        let mut d = det("e", None, 0.9, "unreviewed", None);
        d.top_label = Some("Wild Boar".to_string());
        assert_eq!(subject_entity(&d), "vision.subject.wild_boar");
    }

    #[test]
    fn ingest_records_current_fact_with_review_state() {
        let world = WorldMemory::open_in_memory().unwrap();
        let dets = vec![det("evt-1", Some("deer"), 0.91, "verified", Some("2026-06-05T12:00:00Z"))];
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
            &[det("e1", Some("deer"), 0.8, "unreviewed", Some("2026-06-05T12:00:00Z"))],
            1,
            "clawcam",
        )
        .unwrap();
        ingest_clawcam_detections(
            &world,
            &[det("e2", Some("deer"), 0.95, "verified", Some("2026-06-06T12:00:00Z"))],
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
        assert_eq!(world.current("vision.subject.bird").unwrap().unwrap().valid_from, 5);
    }

    #[test]
    fn tool_result_error_or_empty_ingests_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let err = serde_json::json!({"ok": false, "error": "no inference result"});
        assert!(ingest_tool_result(&world, &err, 1, "clawcam").unwrap().is_empty());
        let empty = serde_json::json!({"ok": true, "count": 0, "results": []});
        assert!(ingest_tool_result(&world, &empty, 1, "clawcam").unwrap().is_empty());
    }
}

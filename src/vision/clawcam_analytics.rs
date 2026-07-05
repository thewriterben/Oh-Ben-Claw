//! ClawCam analytics reports → world memory → "today is weird" reflexes.
//!
//! Detections tell the brain *what* a camera saw; the gateway's analytics
//! reports tell it whether today is **abnormal**. This module folds three
//! read-only report tools into [`WorldMemory`] as `clawcam.analytics.*` facts
//! on a slow cadence, so the reflex layer (see
//! [`super::clawcam_rules::vision_analytics_rules`]) can *act* on an unusual
//! day instead of merely reporting it:
//!
//!  * `get_anomaly_report` → `clawcam.analytics.anomaly` — the latest day's
//!    z-score against the whole series. A strong **drop** is the signature of
//!    a knocked-over/obstructed camera or a dead PIR; a strong **spike** is a
//!    surge worth waking System 2 for.
//!  * `get_encounter_report` → `clawcam.analytics.encounters` — independent
//!    visits (not frames), with the dominant subject.
//!  * `get_calibration_report` → `clawcam.analytics.calibration` — whether
//!    model confidence still agrees with human review, and the suggested
//!    accept threshold.
//!
//! All three gateway tools return `{"ok": true, "report": {…}}`; extraction is
//! tolerant of that wrapper or a bare report object, and a report with no data
//! (no dated detections / nothing reviewed) yields no fact rather than a fake
//! zero. Numeric fact values ride in the `{"value": …}` shape the reflex
//! snapshot already understands, so `Condition::Sensor` thresholds work
//! unchanged; booleans that reflexes must match ride as strings (the
//! `Condition::State` contract).

use crate::mcp::client::McpClient;
use crate::memory::world::WorldMemory;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Entity for the latest-day anomaly z-score fact.
pub const ANOMALY_ENTITY: &str = "clawcam.analytics.anomaly";
/// Entity for the encounter (independent-visit) summary fact.
pub const ENCOUNTERS_ENTITY: &str = "clawcam.analytics.encounters";
/// Entity for the confidence-calibration summary fact.
pub const CALIBRATION_ENTITY: &str = "clawcam.analytics.calibration";

/// Unwrap a gateway analytics tool result: `{"ok": true, "report": {…}}` →
/// the report object. A bare report object passes through; an explicit
/// `{"ok": false, …}` error yields `None`.
fn extract_report(tool_result: &Value) -> Option<&Value> {
    if tool_result.get("ok").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    match tool_result.get("report") {
        Some(r) if r.is_object() => Some(r),
        Some(_) => None,
        None if tool_result.is_object() => Some(tool_result),
        None => None,
    }
}

/// Fold an anomaly report into world memory as [`ANOMALY_ENTITY`].
///
/// The fact reflects the **latest day** in the series (the day reflexes can
/// still act on): `value` is that day's signed z-score (negative = quieter
/// than normal), `kind` is `"spike"`, `"drop"`, or `"none"` (present but not
/// anomalous). An empty series (no dated detections yet) records nothing and
/// returns `Ok(None)` — absence of data is not a calm day.
pub fn ingest_anomaly_report(
    world: &WorldMemory,
    tool_result: &Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Option<String>> {
    let Some(report) = extract_report(tool_result) else {
        return Ok(None);
    };
    let Some(latest) = report
        .get("series")
        .and_then(Value::as_array)
        .and_then(|s| s.last())
    else {
        return Ok(None);
    };
    let z = latest.get("z").and_then(Value::as_f64).unwrap_or(0.0);
    let anomalous = latest.get("anomaly").and_then(Value::as_bool).unwrap_or(false);
    let kind = if anomalous {
        latest
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string()
    } else {
        "none".to_string()
    };
    world.observe(
        ANOMALY_ENTITY,
        json!({
            "value": z,
            "kind": kind,
            "date": latest.get("date").cloned().unwrap_or(Value::Null),
            "count": latest.get("count").cloned().unwrap_or(Value::Null),
            "mean": report.get("mean").cloned().unwrap_or(Value::Null),
            "days": report.get("days").cloned().unwrap_or(Value::Null),
            "z_threshold": report.get("z_threshold").cloned().unwrap_or(Value::Null),
        }),
        ingested_at_ms,
        ingested_at_ms,
        source,
    )?;
    Ok(Some(ANOMALY_ENTITY.to_string()))
}

/// Fold an encounter report into world memory as [`ENCOUNTERS_ENTITY`].
///
/// `value` is the total number of independent encounters (visits, not frames);
/// the dominant subject (most encounters) rides along for context. A report
/// with zero detections records nothing.
pub fn ingest_encounter_report(
    world: &WorldMemory,
    tool_result: &Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Option<String>> {
    let Some(report) = extract_report(tool_result) else {
        return Ok(None);
    };
    let total = report
        .get("total_encounters")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let detections = report
        .get("total_detections")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if detections <= 0.0 {
        return Ok(None);
    }
    // Dominant subject: most encounters, ties broken by name for determinism.
    let top = report
        .get("by_subject")
        .and_then(Value::as_object)
        .and_then(|m| {
            m.iter()
                .map(|(name, v)| {
                    (
                        v.get("encounters").and_then(Value::as_i64).unwrap_or(0),
                        name.clone(),
                    )
                })
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
        });
    let (top_encounters, top_subject) = match top {
        Some((n, name)) => (json!(n), json!(name)),
        None => (Value::Null, Value::Null),
    };
    world.observe(
        ENCOUNTERS_ENTITY,
        json!({
            "value": total,
            "detections": detections,
            "top_subject": top_subject,
            "top_encounters": top_encounters,
            "gap_minutes": report.get("gap_minutes").cloned().unwrap_or(Value::Null),
        }),
        ingested_at_ms,
        ingested_at_ms,
        source,
    )?;
    Ok(Some(ENCOUNTERS_ENTITY.to_string()))
}

/// Fold a calibration report into world memory as [`CALIBRATION_ENTITY`].
///
/// `value` is the suggested accept threshold; `well_calibrated` rides as the
/// **string** `"true"`/`"false"` so a `Condition::State` reflex can match it
/// (the reflex State contract is string equality). Nothing is recorded until
/// at least one classification has been human-reviewed — an unreviewed model
/// is *unknown*, not miscalibrated.
pub fn ingest_calibration_report(
    world: &WorldMemory,
    tool_result: &Value,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Option<String>> {
    let Some(report) = extract_report(tool_result) else {
        return Ok(None);
    };
    let reviewed = report.get("reviewed").and_then(Value::as_f64).unwrap_or(0.0);
    if reviewed <= 0.0 {
        return Ok(None);
    }
    let well = report
        .get("well_calibrated")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    world.observe(
        CALIBRATION_ENTITY,
        json!({
            "value": report.get("suggested_threshold").cloned().unwrap_or(Value::Null),
            "well_calibrated": if well { "true" } else { "false" },
            "reviewed": reviewed,
            "precision": report.get("overall_precision").cloned().unwrap_or(Value::Null),
            "target_precision": report.get("target_precision").cloned().unwrap_or(Value::Null),
        }),
        ingested_at_ms,
        ingested_at_ms,
        source,
    )?;
    Ok(Some(CALIBRATION_ENTITY.to_string()))
}

/// Poll all three analytics tools over a live MCP connection and fold whatever
/// succeeds into world memory — the slow-cadence perception → memory seam for
/// "is today normal?". Per-tool failures log and skip (a gateway missing one
/// report never blocks the others); the connection erroring on **every** tool
/// surfaces as an error so the caller's loop can log one warning.
pub async fn poll_clawcam_analytics(
    client: Arc<Mutex<McpClient>>,
    world: &WorldMemory,
    ingested_at_ms: u64,
    source: &str,
) -> anyhow::Result<Vec<String>> {
    let mut entities = Vec::new();
    let mut errors = 0usize;
    for (tool, ingest) in [
        (
            "get_anomaly_report",
            ingest_anomaly_report as fn(&WorldMemory, &Value, u64, &str) -> anyhow::Result<Option<String>>,
        ),
        ("get_encounter_report", ingest_encounter_report),
        ("get_calibration_report", ingest_calibration_report),
    ] {
        let raw = {
            let mut guard = client.lock().await;
            guard.call_tool(tool, json!({})).await
        };
        match raw {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(parsed) => {
                    if let Some(entity) = ingest(world, &parsed, ingested_at_ms, source)? {
                        entities.push(entity);
                    }
                }
                Err(e) => {
                    errors += 1;
                    tracing::warn!("ClawCam analytics: {tool} returned non-JSON: {e}");
                }
            },
            Err(e) => {
                errors += 1;
                tracing::warn!("ClawCam analytics: {tool} call failed: {e}");
            }
        }
    }
    if errors == 3 {
        anyhow::bail!("all ClawCam analytics tools failed");
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anomaly_result(z: f64, anomaly: bool, kind: &str) -> Value {
        let kind_v = if anomaly { json!(kind) } else { Value::Null };
        let anomalies = u32::from(anomaly);
        json!({
            "ok": true,
            "report": {
                "days": 7, "mean": 12.0, "stdev": 3.0, "z_threshold": 2.0,
                "anomalies": anomalies,
                "series": [
                    {"date": "2026-07-03", "count": 12, "z": 0.0, "anomaly": false, "kind": null},
                    {"date": "2026-07-04", "count": 3, "z": z, "anomaly": anomaly, "kind": kind_v},
                ],
            },
        })
    }

    #[test]
    fn anomaly_latest_day_becomes_fact_with_signed_z() {
        let world = WorldMemory::open_in_memory().unwrap();
        let out = ingest_anomaly_report(&world, &anomaly_result(-2.6, true, "drop"), 1_000, "clawcam").unwrap();
        assert_eq!(out.as_deref(), Some(ANOMALY_ENTITY));
        let fact = world.current(ANOMALY_ENTITY).unwrap().unwrap();
        assert!((fact.value["value"].as_f64().unwrap() + 2.6).abs() < 1e-9);
        assert_eq!(fact.value["kind"], "drop");
        assert_eq!(fact.value["date"], "2026-07-04");
        assert_eq!(fact.source, "clawcam");
    }

    #[test]
    fn non_anomalous_day_records_kind_none() {
        let world = WorldMemory::open_in_memory().unwrap();
        ingest_anomaly_report(&world, &anomaly_result(0.4, false, ""), 1_000, "clawcam").unwrap();
        let fact = world.current(ANOMALY_ENTITY).unwrap().unwrap();
        assert_eq!(fact.value["kind"], "none");
    }

    #[test]
    fn empty_series_records_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let empty = json!({"ok": true, "report": {"days": 0, "series": []}});
        let out = ingest_anomaly_report(&world, &empty, 1_000, "clawcam").unwrap();
        assert!(out.is_none());
        assert!(world.current(ANOMALY_ENTITY).unwrap().is_none());
    }

    #[test]
    fn error_result_records_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let err = json!({"ok": false, "error": "boom"});
        assert!(ingest_anomaly_report(&world, &err, 1_000, "clawcam").unwrap().is_none());
        assert!(ingest_encounter_report(&world, &err, 1_000, "clawcam").unwrap().is_none());
        assert!(ingest_calibration_report(&world, &err, 1_000, "clawcam").unwrap().is_none());
    }

    #[test]
    fn encounters_fact_carries_dominant_subject() {
        let world = WorldMemory::open_in_memory().unwrap();
        let result = json!({
            "ok": true,
            "report": {
                "gap_minutes": 30, "total_encounters": 7, "total_detections": 19,
                "by_subject": {
                    "deer": {"encounters": 5, "detections": 15, "compression": 3.0},
                    "fox":  {"encounters": 2, "detections": 4,  "compression": 2.0},
                },
            },
        });
        let out = ingest_encounter_report(&world, &result, 2_000, "clawcam").unwrap();
        assert_eq!(out.as_deref(), Some(ENCOUNTERS_ENTITY));
        let fact = world.current(ENCOUNTERS_ENTITY).unwrap().unwrap();
        assert!((fact.value["value"].as_f64().unwrap() - 7.0).abs() < 1e-9);
        assert_eq!(fact.value["top_subject"], "deer");
        assert_eq!(fact.value["top_encounters"], 5);
    }

    #[test]
    fn zero_detection_encounter_report_records_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let result = json!({"ok": true, "report": {"total_encounters": 0, "total_detections": 0, "by_subject": {}}});
        assert!(ingest_encounter_report(&world, &result, 2_000, "clawcam").unwrap().is_none());
    }

    #[test]
    fn calibration_bool_rides_as_state_matchable_string() {
        let world = WorldMemory::open_in_memory().unwrap();
        let result = json!({
            "ok": true,
            "report": {
                "reviewed": 40, "confirmed": 25, "rejected": 15,
                "overall_precision": 0.62, "target_precision": 0.9,
                "well_calibrated": false, "suggested_threshold": 0.8,
            },
        });
        let out = ingest_calibration_report(&world, &result, 3_000, "clawcam").unwrap();
        assert_eq!(out.as_deref(), Some(CALIBRATION_ENTITY));
        let fact = world.current(CALIBRATION_ENTITY).unwrap().unwrap();
        // String, not bool — the reflex State condition matches strings.
        assert_eq!(fact.value["well_calibrated"], "false");
        assert!((fact.value["value"].as_f64().unwrap() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn unreviewed_calibration_records_nothing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let result = json!({"ok": true, "report": {"reviewed": 0, "well_calibrated": true}});
        assert!(ingest_calibration_report(&world, &result, 3_000, "clawcam").unwrap().is_none());
    }

    #[test]
    fn bare_report_object_is_accepted() {
        let world = WorldMemory::open_in_memory().unwrap();
        let bare = anomaly_result(2.4, true, "spike")["report"].clone();
        let out = ingest_anomaly_report(&world, &bare, 1_000, "clawcam").unwrap();
        assert_eq!(out.as_deref(), Some(ANOMALY_ENTITY));
        let fact = world.current(ANOMALY_ENTITY).unwrap().unwrap();
        assert_eq!(fact.value["kind"], "spike");
    }
}

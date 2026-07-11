//! GNSS decode tool — expose the Conservation Grid **G3** NMEA decoder.
//!
//! `gnss_fix` takes a raw NMEA `GGA` sentence (what a bare GPS/GNSS receiver emits) and
//! returns the decoded fix (lat/lon/alt, quality, satellites, HDOP) plus the vehicle
//! projected into the fleet's local ENU frame as a [`NodeState`] — so a node with only a
//! GPS module joins the same coordinate frame as the drones and cameras.
//!
//! Frame resolution order: an explicit `origin` argument wins; otherwise the
//! world-memory-anchored site frame (G0, [`crate::geo::anchor`]) when the tool is built
//! `with_world` and a site is anchored; otherwise the fix's own position (node at 0,0).
//! The output's `frame` field says which one was used.
//!
//! Pure computation: no world change, so it's safe (no approval). Backed by
//! [`crate::gnss`].

use crate::geo::anchor as geo_anchor;
use crate::geo::{GeoFrame, GeoPoint};
use crate::gnss::{parse_gga, FixQuality};
use crate::memory::world::WorldMemory;
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// GNSS decode tool; optionally frame-aware through world memory.
#[derive(Default)]
pub struct GnssFixTool {
    world: Option<Arc<WorldMemory>>,
}

impl GnssFixTool {
    /// Stateless tool: frame defaults to the fix's own position.
    pub fn new() -> Self {
        Self { world: None }
    }

    /// Frame-aware tool: when no explicit `origin` is given, fixes are projected
    /// into the world-memory-anchored site frame (if one is anchored).
    pub fn with_world(world: Arc<WorldMemory>) -> Self {
        Self { world: Some(world) }
    }
}

fn quality_label(q: FixQuality) -> String {
    match q {
        FixQuality::NoFix => "no_fix".to_string(),
        FixQuality::Gps => "gps".to_string(),
        FixQuality::Dgps => "dgps".to_string(),
        FixQuality::Rtk => "rtk".to_string(),
        FixQuality::RtkFloat => "rtk_float".to_string(),
        FixQuality::Other(n) => format!("other({n})"),
    }
}

#[async_trait]
impl Tool for GnssFixTool {
    fn name(&self) -> &str {
        "gnss_fix"
    }

    fn description(&self) -> &str {
        "Decode a raw NMEA GGA sentence from a GPS/GNSS receiver into a position and a fleet \
         node (Conservation Grid GNSS tier). Provide `sentence` (e.g. \
         \"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,...\"). Optional `origin` \
         ([lat, lon]) anchors the local ENU frame; otherwise the anchored site frame \
         (see `site_anchor`) is used when one exists, else the fix itself. Optional \
         `node_id` and `now_ms`. Returns the decoded fix (lat/lon/alt, quality, \
         satellites, hdop, has_fix), a fleet node_state (local x/y metres), and which \
         `frame` was used. Pure computation — no approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["sentence"],
            "properties": {
                "sentence": { "type": "string", "description": "Raw NMEA GGA sentence." },
                "origin": {
                    "type": "array",
                    "description": "Optional ENU frame origin [lat, lon]; defaults to the fix position.",
                    "items": { "type": "number" }
                },
                "node_id": { "type": "string", "description": "Id for the projected node (default \"gnss\")." },
                "now_ms": { "type": "integer", "description": "Timestamp for the node heartbeat (default 0)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(sentence) = args.get("sentence").and_then(Value::as_str) else {
            return Ok(ToolResult::err("'gnss_fix' requires 'sentence' (a string)"));
        };
        let fix = match parse_gga(sentence) {
            Ok(f) => f,
            Err(e) => return Ok(ToolResult::err(format!("could not parse GGA: {e}"))),
        };

        // Frame origin: explicit [lat, lon] wins; else the anchored site frame (G0)
        // when available; else the fix's own position.
        let (frame, frame_kind) = match args.get("origin").and_then(Value::as_array) {
            Some(o) => {
                let lat = o.first().and_then(Value::as_f64);
                let lon = o.get(1).and_then(Value::as_f64);
                match (lat, lon) {
                    (Some(lat), Some(lon)) => {
                        (GeoFrame::new(GeoPoint::new(lat, lon, 0.0)), "explicit")
                    }
                    _ => return Ok(ToolResult::err("'origin' must be [lat, lon]")),
                }
            }
            None => {
                let anchored = self
                    .world
                    .as_deref()
                    .and_then(|w| geo_anchor::anchored_frame(w).ok().flatten());
                match anchored {
                    Some(f) => (f, "site"),
                    None => (GeoFrame::new(fix.to_geopoint()), "fix"),
                }
            }
        };
        let node_id = args
            .get("node_id")
            .and_then(Value::as_str)
            .unwrap_or("gnss");
        let now_ms = args.get("now_ms").and_then(Value::as_u64).unwrap_or(0);

        let node = fix.to_node_state(&frame, node_id, now_ms);

        Ok(ToolResult::ok(
            json!({
                "fix": {
                    "lat": fix.lat,
                    "lon": fix.lon,
                    "alt_m": fix.alt_m,
                    "fix_quality": fix.fix_quality,
                    "quality": quality_label(fix.quality()),
                    "has_fix": fix.has_fix(),
                    "satellites": fix.satellites,
                    "hdop": fix.hdop,
                    "time_utc": fix.time_utc,
                },
                "node_state": {
                    "id": node.id,
                    "x": node.x,
                    "y": node.y,
                    "mode": node.mode,
                    "busy": node.busy,
                    "last_seen_ms": node.last_seen_ms,
                },
                "frame": frame_kind,
            })
            .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GGA: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";

    #[test]
    fn gnss_fix_is_safe() {
        assert!(!GnssFixTool::new().risk_class().physical);
    }

    #[tokio::test]
    async fn decodes_a_fix_and_projects_a_node() {
        let r = GnssFixTool::new()
            .execute(json!({ "sentence": GGA }))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!((v["fix"]["lat"].as_f64().unwrap() - 48.1173).abs() < 1e-4);
        assert_eq!(v["fix"]["quality"], json!("gps"));
        assert_eq!(v["fix"]["has_fix"], json!(true));
        assert_eq!(v["fix"]["satellites"], json!(8));
        // No explicit origin => node sits at the fix (near 0,0).
        assert!(v["node_state"]["x"].as_f64().unwrap().abs() < 1e-6);
        assert_eq!(v["node_state"]["mode"], json!("gnss_fix"));
    }

    #[tokio::test]
    async fn origin_offset_places_node_off_zero() {
        // Origin one arc-minute west of the fix => node lands east (+x) of it.
        let r = GnssFixTool::new()
            .execute(json!({ "sentence": GGA, "origin": [48.1173, 11.5] }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!(v["node_state"]["x"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn anchored_site_frame_is_used_when_no_origin() {
        use crate::geo::Site;
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        // Anchor a site origin one arc-minute west of the fix (48.1173 N, 11.5 E).
        let site = Site {
            id: "s1".into(),
            name: String::new(),
            origin: GeoPoint::new(48.1173, 11.5, 0.0),
            boundary: vec![],
            dem_ref: None,
        };
        crate::geo::anchor::anchor_site(&world, &site, 1_000, "test").unwrap();

        let r = GnssFixTool::with_world(world)
            .execute(json!({ "sentence": GGA }))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["frame"], json!("site"));
        // The fix lands east (+x) of the anchored origin, not at 0,0.
        assert!(v["node_state"]["x"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn without_anchor_world_aware_tool_falls_back_to_fix_frame() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let r = GnssFixTool::with_world(world)
            .execute(json!({ "sentence": GGA }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["frame"], json!("fix"));
        assert!(v["node_state"]["x"].as_f64().unwrap().abs() < 1e-6);
    }

    #[tokio::test]
    async fn explicit_origin_beats_the_anchored_frame() {
        use crate::geo::Site;
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let site = Site {
            id: "s1".into(),
            name: String::new(),
            origin: GeoPoint::new(10.0, 10.0, 0.0),
            boundary: vec![],
            dem_ref: None,
        };
        crate::geo::anchor::anchor_site(&world, &site, 1_000, "test").unwrap();

        let r = GnssFixTool::with_world(world)
            .execute(json!({ "sentence": GGA, "origin": [48.1173, 11.5] }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["frame"], json!("explicit"));
    }

    #[tokio::test]
    async fn missing_sentence_is_soft_error() {
        let r = GnssFixTool::new().execute(json!({})).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn garbled_sentence_is_soft_error() {
        let r = GnssFixTool::new()
            .execute(json!({ "sentence": "$GPRMC,not,a,gga,sentence" }))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn bad_origin_is_soft_error() {
        let r = GnssFixTool::new()
            .execute(json!({ "sentence": GGA, "origin": [48.0] }))
            .await
            .unwrap();
        assert!(!r.success);
    }
}

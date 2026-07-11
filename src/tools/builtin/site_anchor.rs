//! Site-anchor tool — pin the live geospatial frame in world memory (G0 exit).
//!
//! `site_anchor` lets the agent (or an operator through it) anchor a survey [`Site`]
//! as the fleet's shared earth frame, and convert poses through it: local ENU metres
//! ↔ `(lat, lon)`. Backed by [`crate::geo::anchor`] over the shared [`WorldMemory`],
//! so the anchor survives restarts and re-anchoring stays auditable (time-valid,
//! non-destructive).
//!
//! Pure bookkeeping + math — no physical effect, so the risk class is safe.

use crate::geo::anchor as geo_anchor;
use crate::geo::{Enu, GeoPoint, Site};
use crate::memory::world::WorldMemory;
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tool: anchor and query the fleet's shared geospatial frame.
pub struct SiteAnchorTool {
    world: Arc<WorldMemory>,
}

impl SiteAnchorTool {
    /// Build the tool over the shared world-memory store.
    pub fn new(world: Arc<WorldMemory>) -> Self {
        Self { world }
    }

    fn parse_boundary(v: &Value) -> Result<Vec<GeoPoint>, String> {
        let arr = v
            .as_array()
            .ok_or("'boundary' must be an array of [lat, lon] points")?;
        let mut out = Vec::with_capacity(arr.len());
        for (i, p) in arr.iter().enumerate() {
            let pa = p
                .as_array()
                .ok_or(format!("boundary[{i}] must be [lat, lon]"))?;
            let lat = pa
                .first()
                .and_then(Value::as_f64)
                .ok_or(format!("boundary[{i}].lat"))?;
            let lon = pa
                .get(1)
                .and_then(Value::as_f64)
                .ok_or(format!("boundary[{i}].lon"))?;
            let alt = pa.get(2).and_then(Value::as_f64).unwrap_or(0.0);
            out.push(GeoPoint::new(lat, lon, alt));
        }
        Ok(out)
    }

    fn site_json(site: &Site) -> Value {
        json!({
            "id": site.id,
            "name": site.name,
            "origin": { "lat": site.origin.lat, "lon": site.origin.lon, "alt": site.origin.alt },
            "boundary_points": site.boundary.len(),
        })
    }
}

#[async_trait]
impl Tool for SiteAnchorTool {
    fn name(&self) -> &str {
        "site_anchor"
    }

    fn description(&self) -> &str {
        "Anchor and query the fleet's shared geospatial frame (Conservation Grid). Actions: \
         'anchor' (pin a site: `boundary` polygon of [lat, lon] points and/or `origin` \
         [lat, lon, alt?], optional `id`/`name`), 'get' (the current anchor), 'to_geo' \
         (local ENU metres `e`,`n`,`u?` → lat/lon/alt), 'from_geo' (`lat`,`lon`,`alt?` → \
         local ENU metres), 'contains' (is `lat`,`lon` inside the site boundary). Once \
         anchored, node poses and site plans share one earth frame. Pure bookkeeping — \
         no approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["anchor", "get", "to_geo", "from_geo", "contains"]
                },
                "boundary": {
                    "type": "array",
                    "description": "Site polygon: array of [lat, lon] (optionally [lat, lon, alt]). For 'anchor'.",
                    "items": { "type": "array", "items": { "type": "number" } }
                },
                "origin": {
                    "type": "array",
                    "description": "Frame origin [lat, lon] or [lat, lon, alt]; defaults to the boundary centroid.",
                    "items": { "type": "number" }
                },
                "id": { "type": "string", "description": "Site id (default \"site\")." },
                "name": { "type": "string", "description": "Human-readable site name." },
                "e": { "type": "number", "description": "East metres, for 'to_geo'." },
                "n": { "type": "number", "description": "North metres, for 'to_geo'." },
                "u": { "type": "number", "description": "Up metres, for 'to_geo' (default 0)." },
                "lat": { "type": "number", "description": "Latitude, for 'from_geo'/'contains'." },
                "lon": { "type": "number", "description": "Longitude, for 'from_geo'/'contains'." },
                "alt": { "type": "number", "description": "Altitude metres, for 'from_geo' (default 0)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "anchor" => {
                let boundary = match args.get("boundary") {
                    Some(v) => match Self::parse_boundary(v) {
                        Ok(b) => b,
                        Err(e) => return Ok(ToolResult::err(e)),
                    },
                    None => Vec::new(),
                };
                let id = args.get("id").and_then(Value::as_str).unwrap_or("site");
                let name = args.get("name").and_then(Value::as_str).unwrap_or("");
                let mut site = Site::new(id, name, boundary);
                match args.get("origin").and_then(Value::as_array) {
                    Some(o) => {
                        let lat = o.first().and_then(Value::as_f64);
                        let lon = o.get(1).and_then(Value::as_f64);
                        let alt = o.get(2).and_then(Value::as_f64).unwrap_or(0.0);
                        match (lat, lon) {
                            (Some(lat), Some(lon)) => site.origin = GeoPoint::new(lat, lon, alt),
                            _ => {
                                return Ok(ToolResult::err(
                                    "'origin' must be [lat, lon] or [lat, lon, alt]",
                                ))
                            }
                        }
                    }
                    None if site.boundary.is_empty() => {
                        return Ok(ToolResult::err(
                            "'anchor' needs an 'origin' [lat, lon] and/or a 'boundary' polygon",
                        ));
                    }
                    None => {}
                }
                let now = now_ms();
                geo_anchor::anchor_site(&self.world, &site, now, "site_anchor")?;
                Ok(ToolResult::ok(
                    json!({ "anchored": Self::site_json(&site), "valid_from": now }).to_string(),
                ))
            }
            "get" => match geo_anchor::anchored_site(&self.world)? {
                Some(site) => Ok(ToolResult::ok(
                    json!({ "site": Self::site_json(&site) }).to_string(),
                )),
                None => Ok(ToolResult::ok("No site anchored".to_string())),
            },
            "to_geo" => {
                let (Some(e), Some(n)) = (
                    args.get("e").and_then(Value::as_f64),
                    args.get("n").and_then(Value::as_f64),
                ) else {
                    return Ok(ToolResult::err("'to_geo' requires 'e' and 'n' (metres)"));
                };
                let u = args.get("u").and_then(Value::as_f64).unwrap_or(0.0);
                match geo_anchor::enu_to_geodetic(&self.world, Enu::new(e, n, u))? {
                    Some(p) => Ok(ToolResult::ok(
                        json!({ "lat": p.lat, "lon": p.lon, "alt": p.alt }).to_string(),
                    )),
                    None => Ok(ToolResult::err("No site anchored — run 'anchor' first")),
                }
            }
            "from_geo" => {
                let (Some(lat), Some(lon)) = (
                    args.get("lat").and_then(Value::as_f64),
                    args.get("lon").and_then(Value::as_f64),
                ) else {
                    return Ok(ToolResult::err("'from_geo' requires 'lat' and 'lon'"));
                };
                let alt = args.get("alt").and_then(Value::as_f64).unwrap_or(0.0);
                match geo_anchor::geodetic_to_enu(&self.world, GeoPoint::new(lat, lon, alt))? {
                    Some(enu) => Ok(ToolResult::ok(
                        json!({ "e": enu.e, "n": enu.n, "u": enu.u }).to_string(),
                    )),
                    None => Ok(ToolResult::err("No site anchored — run 'anchor' first")),
                }
            }
            "contains" => {
                let (Some(lat), Some(lon)) = (
                    args.get("lat").and_then(Value::as_f64),
                    args.get("lon").and_then(Value::as_f64),
                ) else {
                    return Ok(ToolResult::err("'contains' requires 'lat' and 'lon'"));
                };
                match geo_anchor::anchored_site(&self.world)? {
                    Some(site) => Ok(ToolResult::ok(
                        json!({ "contains": site.contains(GeoPoint::new(lat, lon, 0.0)) })
                            .to_string(),
                    )),
                    None => Ok(ToolResult::err("No site anchored — run 'anchor' first")),
                }
            }
            other => Ok(ToolResult::err(format!("Unknown action: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> SiteAnchorTool {
        SiteAnchorTool::new(Arc::new(WorldMemory::open_in_memory().unwrap()))
    }

    const SQUARE: &str = r#"[[45.4,-122.7],[45.4,-122.5],[45.6,-122.5],[45.6,-122.7]]"#;

    fn square_args() -> Value {
        json!({
            "action": "anchor",
            "id": "s1",
            "name": "North Ridge",
            "boundary": serde_json::from_str::<Value>(SQUARE).unwrap()
        })
    }

    #[test]
    fn site_anchor_is_safe() {
        assert!(!tool().risk_class().physical);
    }

    #[tokio::test]
    async fn anchor_then_get() {
        let t = tool();
        let r = t.execute(square_args()).await.unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let g = t.execute(json!({ "action": "get" })).await.unwrap();
        assert!(g.success);
        assert!(g.output.contains("\"id\":\"s1\""), "out: {}", g.output);
        assert!(
            g.output.contains("45.5"),
            "centroid origin expected: {}",
            g.output
        );
    }

    #[tokio::test]
    async fn get_without_anchor_is_informative() {
        let g = tool().execute(json!({ "action": "get" })).await.unwrap();
        assert!(g.success);
        assert!(g.output.contains("No site anchored"));
    }

    #[tokio::test]
    async fn pose_round_trip_through_tool() {
        let t = tool();
        t.execute(square_args()).await.unwrap();

        let geo = t
            .execute(json!({ "action": "to_geo", "e": 100.0, "n": 50.0 }))
            .await
            .unwrap();
        assert!(geo.success, "err: {:?}", geo.error);
        let v: Value = serde_json::from_str(&geo.output).unwrap();

        let back = t
            .execute(json!({ "action": "from_geo", "lat": v["lat"], "lon": v["lon"] }))
            .await
            .unwrap();
        assert!(back.success);
        let b: Value = serde_json::from_str(&back.output).unwrap();
        assert!((b["e"].as_f64().unwrap() - 100.0).abs() < 1e-6);
        assert!((b["n"].as_f64().unwrap() - 50.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn conversions_without_anchor_are_soft_errors() {
        let t = tool();
        let r = t
            .execute(json!({ "action": "to_geo", "e": 1.0, "n": 1.0 }))
            .await
            .unwrap();
        assert!(!r.success);
        let r = t
            .execute(json!({ "action": "from_geo", "lat": 45.5, "lon": -122.6 }))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn contains_uses_the_anchored_boundary() {
        let t = tool();
        t.execute(square_args()).await.unwrap();
        let inside = t
            .execute(json!({ "action": "contains", "lat": 45.5, "lon": -122.6 }))
            .await
            .unwrap();
        assert!(inside.output.contains("true"));
        let outside = t
            .execute(json!({ "action": "contains", "lat": 10.0, "lon": 10.0 }))
            .await
            .unwrap();
        assert!(outside.output.contains("false"));
    }

    #[tokio::test]
    async fn anchor_with_origin_only() {
        let t = tool();
        let r = t
            .execute(json!({ "action": "anchor", "origin": [45.5, -122.6, 100.0], "id": "pin" }))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let geo = t
            .execute(json!({ "action": "to_geo", "e": 0.0, "n": 0.0 }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&geo.output).unwrap();
        assert!((v["lat"].as_f64().unwrap() - 45.5).abs() < 1e-9);
        assert!((v["alt"].as_f64().unwrap() - 100.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn anchor_without_origin_or_boundary_errors() {
        let r = tool().execute(json!({ "action": "anchor" })).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn unknown_action_errors() {
        let r = tool()
            .execute(json!({ "action": "frobnicate" }))
            .await
            .unwrap();
        assert!(!r.success);
    }
}

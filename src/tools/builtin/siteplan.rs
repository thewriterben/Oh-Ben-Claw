//! Site-planning tool — expose the Conservation Grid **G1** coverage optimizer.
//!
//! `plan_site` takes a survey-area boundary polygon (`[lat, lon]` vertices) and a node
//! budget and returns an optimized, mesh-connected, coverage-maximizing placement — each
//! node's geodetic + local ENU position, the coverage fraction, and a paste-ready
//! `[[site.node]]` TOML block for deployment. Pure computation: no world change, so it's
//! safe (no approval). Backed by [`crate::siteplan::plan_site`].

use crate::geo::{GeoPoint, Site};
use crate::siteplan::{plan_site, PlacementSpec};
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Stateless planner tool.
#[derive(Default)]
pub struct SitePlanTool;

impl SitePlanTool {
    pub fn new() -> Self {
        Self
    }
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

#[async_trait]
impl Tool for SitePlanTool {
    fn name(&self) -> &str {
        "plan_site"
    }

    fn description(&self) -> &str {
        "Plan camera/sensor node placement over a survey area (Conservation Grid). Provide \
         `boundary` (a polygon: array of [lat, lon] points) and `budget` (node count); \
         returns an optimized, mesh-connected placement maximizing detection coverage, with \
         each node's lat/lon + local metres, the coverage fraction, and a paste-ready TOML \
         block. Optional: detection_radius_m, min_spacing_m, mesh_range_m, lattice_step_m, \
         demand_step_m, require_mesh_connectivity, site_id. Pure planning — no approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["boundary", "budget"],
            "properties": {
                "boundary": {
                    "type": "array",
                    "description": "Site polygon: array of [lat, lon] (optionally [lat, lon, alt]).",
                    "items": { "type": "array", "items": { "type": "number" } }
                },
                "budget": { "type": "integer", "description": "Number of nodes to place." },
                "detection_radius_m": { "type": "number", "description": "Coverage radius per node (default 30)." },
                "min_spacing_m": { "type": "number", "description": "Minimum node spacing (default 20)." },
                "mesh_range_m": { "type": "number", "description": "Mesh link range (default 250)." },
                "lattice_step_m": { "type": "number", "description": "Candidate lattice step (default 10)." },
                "demand_step_m": { "type": "number", "description": "Coverage sampling step (default 10)." },
                "require_mesh_connectivity": { "type": "boolean", "description": "Keep the placement mesh-connected (default true)." },
                "site_id": { "type": "string", "description": "Id used in node ids + TOML (default \"site\")." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(boundary_val) = args.get("boundary") else {
            return Ok(ToolResult::err("'plan_site' requires 'boundary'"));
        };
        let boundary = match parse_boundary(boundary_val) {
            Ok(b) => b,
            Err(e) => return Ok(ToolResult::err(e)),
        };
        if boundary.len() < 3 {
            return Ok(ToolResult::err("'boundary' needs at least 3 points"));
        }
        let site_id = args
            .get("site_id")
            .and_then(Value::as_str)
            .unwrap_or("site")
            .to_string();
        let site = Site::new(site_id.clone(), "", boundary);

        let d = PlacementSpec::default();
        let spec = PlacementSpec {
            budget: args
                .get("budget")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(d.budget),
            detection_radius_m: args
                .get("detection_radius_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.detection_radius_m),
            min_spacing_m: args
                .get("min_spacing_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.min_spacing_m),
            mesh_range_m: args
                .get("mesh_range_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.mesh_range_m),
            lattice_step_m: args
                .get("lattice_step_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.lattice_step_m),
            demand_step_m: args
                .get("demand_step_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.demand_step_m),
            require_mesh_connectivity: args
                .get("require_mesh_connectivity")
                .and_then(Value::as_bool)
                .unwrap_or(d.require_mesh_connectivity),
            node_height_m: args
                .get("node_height_m")
                .and_then(Value::as_f64)
                .unwrap_or(d.node_height_m),
        };

        let plan = plan_site(&site, &spec);
        let nodes: Vec<Value> = plan
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                json!({
                    "id": format!("{}-n{:02}", site_id, i + 1),
                    "lat": n.geo.lat, "lon": n.geo.lon, "alt": n.geo.alt,
                    "enu_e": n.enu.e, "enu_n": n.enu.n, "covers": n.covers
                })
            })
            .collect();

        Ok(ToolResult::ok(
            json!({
                "site_id": site_id,
                "node_count": plan.nodes.len(),
                "coverage_fraction": plan.coverage_fraction,
                "covered_points": plan.covered_points,
                "demand_points": plan.demand_points,
                "mesh_connected": plan.mesh_connected,
                "summary": plan.summary(),
                "nodes": nodes,
                "toml": plan.to_toml(&site_id),
            })
            .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Value {
        // ~100 m square near (45.5, -122.6).
        let dlat = 50.0 / 111_194.9;
        let dlon = 50.0 / (111_194.9 * 45.5_f64.to_radians().cos());
        let (lat, lon) = (45.5, -122.6);
        json!([
            [lat - dlat, lon - dlon],
            [lat - dlat, lon + dlon],
            [lat + dlat, lon + dlon],
            [lat + dlat, lon - dlon]
        ])
    }

    #[test]
    fn plan_site_is_safe() {
        assert!(!SitePlanTool::new().risk_class().physical);
    }

    #[tokio::test]
    async fn plans_a_square() {
        let r = SitePlanTool::new()
            .execute(json!({ "boundary": square(), "budget": 4, "site_id": "s1" }))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!(v["node_count"].as_u64().unwrap() >= 2);
        assert!(v["coverage_fraction"].as_f64().unwrap() > 0.0);
        assert_eq!(v["mesh_connected"], json!(true));
        assert!(v["toml"].as_str().unwrap().contains("[[site.node]]"));
        assert_eq!(
            v["nodes"].as_array().unwrap().len(),
            v["node_count"].as_u64().unwrap() as usize
        );
    }

    #[tokio::test]
    async fn too_few_points_is_soft_error() {
        let r = SitePlanTool::new()
            .execute(json!({ "boundary": [[0.0, 0.0], [1.0, 1.0]], "budget": 2 }))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn missing_boundary_is_soft_error() {
        let r = SitePlanTool::new()
            .execute(json!({ "budget": 2 }))
            .await
            .unwrap();
        assert!(!r.success);
    }
}

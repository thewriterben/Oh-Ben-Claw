//! Aerial-status tool — expose the Conservation Grid **G8** drone adapter.
//!
//! `aerial_status` takes a drone's geodetic telemetry (lat/lon/alt, battery, armed, mode)
//! and an optional survey-area geofence, and returns two things the fleet + safety layers
//! consume: the vehicle projected into the fleet's local ENU frame as a [`NodeState`]
//! (so a UAV joins the same auction/exploration geometry as a ground robot), and the
//! aerial Track-0 [`flight_safe`] verdict (refuse on low battery or outside the geofence).
//!
//! Pure computation: no world change, so it's safe (no approval). Backed by
//! [`crate::aerial`].

use crate::aerial::{flight_safe, AerialTelemetry};
use crate::geo::{GeoFrame, GeoPoint, Site};
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Stateless aerial adapter tool.
#[derive(Default)]
pub struct AerialStatusTool;

impl AerialStatusTool {
    pub fn new() -> Self {
        Self
    }
}

fn parse_boundary(v: &Value) -> Result<Vec<GeoPoint>, String> {
    let arr = v
        .as_array()
        .ok_or("'geofence' must be an array of [lat, lon] points")?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, p) in arr.iter().enumerate() {
        let pa = p
            .as_array()
            .ok_or(format!("geofence[{i}] must be [lat, lon]"))?;
        let lat = pa
            .first()
            .and_then(Value::as_f64)
            .ok_or(format!("geofence[{i}].lat"))?;
        let lon = pa
            .get(1)
            .and_then(Value::as_f64)
            .ok_or(format!("geofence[{i}].lon"))?;
        let alt = pa.get(2).and_then(Value::as_f64).unwrap_or(0.0);
        out.push(GeoPoint::new(lat, lon, alt));
    }
    Ok(out)
}

fn parse_telemetry(v: &Value) -> Result<AerialTelemetry, String> {
    let obj = v.as_object().ok_or("'telemetry' must be an object")?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or("telemetry.id (string) is required")?;
    let lat = obj
        .get("lat")
        .and_then(Value::as_f64)
        .ok_or("telemetry.lat (number) is required")?;
    let lon = obj
        .get("lon")
        .and_then(Value::as_f64)
        .ok_or("telemetry.lon (number) is required")?;
    let battery = obj
        .get("battery_percent")
        .and_then(Value::as_f64)
        .ok_or("telemetry.battery_percent (number) is required")?;
    Ok(AerialTelemetry {
        id: id.to_string(),
        lat,
        lon,
        alt_m: obj.get("alt_m").and_then(Value::as_f64).unwrap_or(0.0),
        battery_percent: battery,
        armed: obj.get("armed").and_then(Value::as_bool).unwrap_or(false),
        mode: obj
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

#[async_trait]
impl Tool for AerialStatusTool {
    fn name(&self) -> &str {
        "aerial_status"
    }

    fn description(&self) -> &str {
        "Project a drone's telemetry into the fleet and check flight safety (Conservation \
         Grid aerial tier). Provide `telemetry` (object: id, lat, lon, battery_percent, and \
         optional alt_m, armed, mode). Optional `geofence` (a polygon: array of [lat, lon]) \
         and `min_battery_percent` (default 20). Returns the vehicle as a fleet node_state \
         (local ENU x/y, battery, mode, busy) plus a flight_safe verdict (clear + reason). \
         Pure computation — no approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["telemetry"],
            "properties": {
                "telemetry": {
                    "type": "object",
                    "description": "Drone state (MAVLink-ish), geodetic.",
                    "required": ["id", "lat", "lon", "battery_percent"],
                    "properties": {
                        "id": { "type": "string" },
                        "lat": { "type": "number" },
                        "lon": { "type": "number" },
                        "alt_m": { "type": "number", "description": "Altitude AGL (default 0)." },
                        "battery_percent": { "type": "number" },
                        "armed": { "type": "boolean", "description": "Motors armed / in flight (default false)." },
                        "mode": { "type": "string", "description": "Autopilot mode (e.g. AUTO, LOITER, RTL)." }
                    }
                },
                "geofence": {
                    "type": "array",
                    "description": "Optional site boundary polygon: array of [lat, lon]. Used as the ENU frame origin and the flight geofence.",
                    "items": { "type": "array", "items": { "type": "number" } }
                },
                "min_battery_percent": { "type": "number", "description": "Refuse flight below this (default 20)." },
                "now_ms": { "type": "integer", "description": "Timestamp for the node heartbeat (default 0)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(tv) = args.get("telemetry") else {
            return Ok(ToolResult::err("'aerial_status' requires 'telemetry'"));
        };
        let telem = match parse_telemetry(tv) {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::err(e)),
        };

        // Optional geofence: doubles as the ENU frame origin (site centroid).
        let site = match args.get("geofence") {
            Some(g) => match parse_boundary(g) {
                Ok(b) if b.len() >= 3 => Some(Site::new("geofence", "", b)),
                Ok(_) => return Ok(ToolResult::err("'geofence' needs at least 3 points")),
                Err(e) => return Ok(ToolResult::err(e)),
            },
            None => None,
        };

        // Frame origin: the geofence centroid if given, else the drone's own position
        // (which places the node at the local origin).
        let frame = match &site {
            Some(s) => s.frame(),
            None => GeoFrame::new(telem.position()),
        };
        let min_batt = args
            .get("min_battery_percent")
            .and_then(Value::as_f64)
            .unwrap_or(20.0);
        let now_ms = args.get("now_ms").and_then(Value::as_u64).unwrap_or(0);

        let node = telem.to_node_state(&frame, now_ms);
        let verdict = flight_safe(&telem, min_batt, site.as_ref());

        Ok(ToolResult::ok(
            json!({
                "node_state": {
                    "id": node.id,
                    "x": node.x,
                    "y": node.y,
                    "battery": node.battery,
                    "mode": node.mode,
                    "busy": node.busy,
                    "last_seen_ms": node.last_seen_ms,
                },
                "flight_safe": verdict.is_none(),
                "reason": verdict,
                "geofenced": site.is_some(),
                "min_battery_percent": min_batt,
            })
            .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_geofence() -> Value {
        // ~a degree-ish square around (45.5, -122.6).
        json!([
            [45.4, -122.7],
            [45.4, -122.5],
            [45.6, -122.5],
            [45.6, -122.7]
        ])
    }

    fn telem(lat: f64, lon: f64, battery: f64, armed: bool) -> Value {
        json!({ "id": "uav-1", "lat": lat, "lon": lon, "alt_m": 40.0,
                "battery_percent": battery, "armed": armed, "mode": "AUTO" })
    }

    #[test]
    fn aerial_status_is_safe() {
        assert!(!AerialStatusTool::new().risk_class().physical);
    }

    #[tokio::test]
    async fn projects_node_and_clears_flight_inside_geofence() {
        let r = AerialStatusTool::new()
            .execute(json!({ "telemetry": telem(45.5, -122.6, 90.0, true), "geofence": square_geofence() }))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["flight_safe"], json!(true));
        assert_eq!(v["reason"], Value::Null);
        assert_eq!(v["geofenced"], json!(true));
        assert_eq!(v["node_state"]["id"], json!("uav-1"));
        assert_eq!(v["node_state"]["busy"], json!(true));
        assert_eq!(v["node_state"]["mode"], json!("AUTO"));
        // Armed vehicle reports a battery and a finite ENU position.
        assert!(v["node_state"]["x"].as_f64().is_some());
        assert!(v["node_state"]["y"].as_f64().is_some());
    }

    #[tokio::test]
    async fn low_battery_refuses_flight() {
        let r = AerialStatusTool::new()
            .execute(json!({ "telemetry": telem(45.5, -122.6, 10.0, true), "min_battery_percent": 20.0 }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["flight_safe"], json!(false));
        assert!(v["reason"].as_str().unwrap().contains("battery"));
    }

    #[tokio::test]
    async fn outside_geofence_refuses_flight() {
        // Far outside the square.
        let r = AerialStatusTool::new()
            .execute(json!({ "telemetry": telem(10.0, 10.0, 90.0, true), "geofence": square_geofence() }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["flight_safe"], json!(false));
        assert!(v["reason"].as_str().unwrap().contains("geofence"));
    }

    #[tokio::test]
    async fn disarmed_reports_idle_and_not_busy() {
        let r = AerialStatusTool::new()
            .execute(json!({ "telemetry": telem(45.5, -122.6, 90.0, false) }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["node_state"]["busy"], json!(false));
        assert_eq!(v["node_state"]["mode"], json!("idle"));
        assert_eq!(v["geofenced"], json!(false));
    }

    #[tokio::test]
    async fn missing_telemetry_is_soft_error() {
        let r = AerialStatusTool::new().execute(json!({})).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn bad_telemetry_fields_is_soft_error() {
        let r = AerialStatusTool::new()
            .execute(json!({ "telemetry": { "id": "x", "lat": 1.0 } }))
            .await
            .unwrap();
        assert!(!r.success);
    }
}

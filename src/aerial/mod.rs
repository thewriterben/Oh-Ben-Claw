//! Aerial tier — plug a drone into the existing fleet as a body-agnostic node (G8).
//!
//! The fleet coordinator ([`crate::fleet`]) is deliberately body-agnostic: it allocates
//! tasks to any node that reports a [`NodeState`] (pose / battery / mode). This module is
//! the thin adapter that lets an aerial vehicle be one of those nodes — it maps a drone's
//! **geodetic** telemetry (lat/lon/alt, MAVLink-style) into the fleet's **local metric**
//! frame using a site [`GeoFrame`], so a UAV joins the same auction and coordinated
//! exploration as a ground robot with no new coordination code.
//!
//! It also carries the aerial-specific safety check the fleet layer doesn't: a
//! Track-0-flavoured [`flight_safe`] that refuses flight on low battery or outside the
//! site geofence. Pure and hardware-free — a real MAVLink/PX4 link would feed
//! [`AerialTelemetry`] in; here it's just data.

use crate::fleet::NodeState;
use crate::geo::{GeoFrame, GeoPoint, Site};
use serde::{Deserialize, Serialize};

/// A snapshot of an aerial vehicle's state (MAVLink-ish), in geodetic coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AerialTelemetry {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt_m: f64,
    pub battery_percent: f64,
    /// Motors armed (in flight / committed).
    pub armed: bool,
    /// Flight mode string from the autopilot (e.g. "AUTO", "LOITER", "RTL").
    #[serde(default)]
    pub mode: String,
}

impl AerialTelemetry {
    pub fn position(&self) -> GeoPoint {
        GeoPoint::new(self.lat, self.lon, self.alt_m)
    }

    /// Project this vehicle into the fleet's local metric frame as a [`NodeState`].
    ///
    /// The site `frame`'s ENU east/north become the fleet's `x`/`y`, so the coordinator's
    /// nearest-node and conflict-avoidance geometry works on the drone unchanged. A
    /// disarmed vehicle reports `mode = "idle"` and `busy = false`; armed reports its
    /// autopilot mode (or `"flying"`) and `busy = true`.
    pub fn to_node_state(&self, frame: &GeoFrame, now_ms: u64) -> NodeState {
        let enu = frame.to_enu(self.position());
        let mode = if self.armed {
            if self.mode.is_empty() { "flying".to_string() } else { self.mode.clone() }
        } else {
            "idle".to_string()
        };
        NodeState {
            id: self.id.clone(),
            x: Some(enu.e),
            y: Some(enu.n),
            battery: Some(self.battery_percent),
            mode,
            busy: self.armed,
            last_seen_ms: now_ms,
        }
    }
}

/// Aerial safety verdict: `None` = clear to fly, `Some(reason)` = must not.
///
/// Refuses flight when the battery is below `min_battery_percent`, or — when a `geofence`
/// site is supplied — when the vehicle is outside its boundary polygon. This is the
/// aerial Track-0 gate; the fleet layer treats it as advisory and the autopilot keeps its
/// own limits.
pub fn flight_safe(
    telemetry: &AerialTelemetry,
    min_battery_percent: f64,
    geofence: Option<&Site>,
) -> Option<String> {
    if telemetry.battery_percent < min_battery_percent {
        return Some(format!(
            "battery {:.0}% below minimum {:.0}%",
            telemetry.battery_percent, min_battery_percent
        ));
    }
    if let Some(site) = geofence {
        if !site.boundary.is_empty() && !site.contains(telemetry.position()) {
            return Some(format!("outside geofence '{}'", site.id));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> GeoFrame {
        GeoFrame::new(GeoPoint::new(45.5, -122.6, 0.0))
    }

    fn square_site() -> Site {
        Site::new(
            "reserve",
            "",
            vec![
                GeoPoint::new(45.4, -122.7, 0.0),
                GeoPoint::new(45.4, -122.5, 0.0),
                GeoPoint::new(45.6, -122.5, 0.0),
                GeoPoint::new(45.6, -122.7, 0.0),
            ],
        )
    }

    fn drone(lat: f64, lon: f64, battery: f64, armed: bool) -> AerialTelemetry {
        AerialTelemetry {
            id: "uav-1".into(), lat, lon, alt_m: 40.0,
            battery_percent: battery, armed, mode: "AUTO".into(),
        }
    }

    #[test]
    fn origin_maps_to_zero_and_armed_is_busy() {
        let d = drone(45.5, -122.6, 90.0, true);
        let ns = d.to_node_state(&frame(), 1_000);
        assert_eq!(ns.id, "uav-1");
        assert!(ns.x.unwrap().abs() < 1e-6 && ns.y.unwrap().abs() < 1e-6);
        assert_eq!(ns.battery, Some(90.0));
        assert_eq!(ns.mode, "AUTO");
        assert!(ns.busy);
        assert_eq!(ns.last_seen_ms, 1_000);
    }

    #[test]
    fn disarmed_is_idle_and_not_busy() {
        let ns = drone(45.5, -122.6, 90.0, false).to_node_state(&frame(), 5);
        assert_eq!(ns.mode, "idle");
        assert!(!ns.busy);
    }

    #[test]
    fn north_offset_gives_positive_north() {
        // ~0.001 deg north → ~111 m north, ~0 east.
        let ns = drone(45.501, -122.6, 80.0, true).to_node_state(&frame(), 0);
        assert!(ns.y.unwrap() > 100.0 && ns.y.unwrap() < 120.0);
        assert!(ns.x.unwrap().abs() < 1e-3);
    }

    #[test]
    fn flight_safe_ok_inside_and_charged() {
        assert!(flight_safe(&drone(45.5, -122.6, 60.0, true), 20.0, Some(&square_site())).is_none());
    }

    #[test]
    fn flight_unsafe_on_low_battery() {
        let r = flight_safe(&drone(45.5, -122.6, 15.0, true), 20.0, None).unwrap();
        assert!(r.contains("battery"));
    }

    #[test]
    fn flight_unsafe_outside_geofence() {
        let r = flight_safe(&drone(10.0, 10.0, 90.0, true), 20.0, Some(&square_site())).unwrap();
        assert!(r.contains("geofence"));
    }

    #[test]
    fn no_geofence_means_only_battery_gates() {
        assert!(flight_safe(&drone(10.0, 10.0, 90.0, true), 20.0, None).is_none());
    }
}

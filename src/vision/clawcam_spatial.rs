//! Spatial fusion — turn a camera detection into a hazard region on the map.
//!
//! The HIL loop proved a detection can reroute navigation; this generalizes it with
//! real geometry. Cameras are fixed sensors at known world positions; when a
//! camera sees something a mobile robot should avoid (an animal, a person), the
//! brain stamps a hazard disc around that camera's location into the occupancy grid
//! via [`crate::navigation::NavController::mark_obstacle`]. The planner's costmap
//! inflation then keeps the robot clear — perception from a *static* node shaping a
//! *mobile* node's path, both on the one world map.

use crate::navigation::NavController;
use std::collections::HashMap;

/// Known world positions `(x, y)` of fixed camera nodes.
#[derive(Debug, Clone, Default)]
pub struct CameraMap {
    positions: HashMap<String, (f64, f64)>,
}

impl CameraMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or move) a camera's world position.
    pub fn set(&mut self, node_id: impl Into<String>, x: f64, y: f64) {
        self.positions.insert(node_id.into(), (x, y));
    }

    /// The world position of a camera node, if known.
    pub fn get(&self, node_id: &str) -> Option<(f64, f64)> {
        self.positions.get(node_id).copied()
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// The world points inside a disc of `radius` around `center`, sampled on a grid of
/// spacing `step`. Pure (no map needed) so it is easy to test and reason about.
pub fn hazard_points(center: (f64, f64), radius: f64, step: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if step <= 0.0 || radius < 0.0 {
        return out;
    }
    let r2 = radius * radius;
    let n = (radius / step).floor() as i64;
    for i in -n..=n {
        for j in -n..=n {
            let dx = i as f64 * step;
            let dy = j as f64 * step;
            if dx * dx + dy * dy <= r2 {
                out.push((center.0 + dx, center.1 + dy));
            }
        }
    }
    out
}

/// Mark a hazard disc around camera `node_id` (from `map`) into the navigation grid
/// as occupied, so the planner routes a mobile robot clear of what the camera saw.
/// Returns the number of cells actually marked (0 if the camera is unknown or no
/// grid is configured). Pair with `NavController::with_inflation` for clearance.
pub fn mark_detection_hazard(
    nav: &NavController,
    map: &CameraMap,
    node_id: &str,
    radius: f64,
    step: f64,
) -> usize {
    let Some(center) = map.get(node_id) else {
        return 0;
    };
    hazard_points(center, radius, step)
        .into_iter()
        .filter(|&(x, y)| nav.mark_obstacle(x, y, true))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_map_round_trips() {
        let mut m = CameraMap::new();
        assert!(m.is_empty());
        m.set("cam-door", 5.0, 2.0);
        assert_eq!(m.get("cam-door"), Some((5.0, 2.0)));
        assert_eq!(m.get("cam-unknown"), None);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn hazard_points_stay_within_the_radius_and_include_the_center() {
        let pts = hazard_points((10.0, 10.0), 1.0, 0.5);
        assert!(!pts.is_empty());
        assert!(pts.contains(&(10.0, 10.0)), "the center is a hazard point");
        for (x, y) in &pts {
            let d2 = (x - 10.0).powi(2) + (y - 10.0).powi(2);
            assert!(d2 <= 1.0 + 1e-9, "point ({x},{y}) is within the radius");
        }
    }

    #[test]
    fn larger_radius_marks_more_points() {
        let small = hazard_points((0.0, 0.0), 1.0, 0.5).len();
        let large = hazard_points((0.0, 0.0), 3.0, 0.5).len();
        assert!(large > small, "a wider hazard covers more cells: {small} → {large}");
    }

    #[test]
    fn degenerate_inputs_yield_no_points() {
        assert!(hazard_points((0.0, 0.0), 1.0, 0.0).is_empty());
        assert!(hazard_points((0.0, 0.0), -1.0, 0.5).is_empty());
    }
}

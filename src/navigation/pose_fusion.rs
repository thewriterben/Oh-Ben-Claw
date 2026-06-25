//! Pose fusion (SLAM-lite) — fuse multiple pose estimates into one.
//!
//! Real platforms localize from several noisy, partial sources (wheel odometry,
//! GPS, IMU/visual estimates). This module fuses them into a single best pose: a
//! weighted average of position and a **circular** weighted mean of heading (so
//! 350° and 10° fuse to 0°, not 180°). The fused pose is written to the canonical
//! `sensor.pos_x/pos_y/heading` world-memory entities the navigation localizer
//! already reads, plus a `nav.pose_fused` record — so fusion drops in front of
//! navigation with zero changes to it.
//!
//! This is not full SLAM (no map building / loop closure); it is the sensor-
//! fusion layer that a SLAM front-end would feed, and the seam where one can land.

use super::{value_of, Pose};
use crate::memory::world::WorldMemory;
use serde_json::json;
use std::sync::Arc;

/// One pose estimate source: the world-memory entities holding its x/y/heading
/// and a non-negative fusion weight (higher = more trusted).
#[derive(Debug, Clone)]
pub struct PoseSource {
    pub x_entity: String,
    pub y_entity: String,
    pub heading_entity: String,
    pub weight: f64,
}

impl PoseSource {
    /// A source whose entities follow `sensor.{prefix}_x/_y/_heading`.
    pub fn with_prefix(prefix: &str, weight: f64) -> Self {
        Self {
            x_entity: format!("sensor.{prefix}_x"),
            y_entity: format!("sensor.{prefix}_y"),
            heading_entity: format!("sensor.{prefix}_heading"),
            weight,
        }
    }
}

/// Fuses several [`PoseSource`]s into a single pose, recorded into world memory.
pub struct PoseFuser {
    sources: Vec<PoseSource>,
    world: Arc<WorldMemory>,
    /// Output entities for the fused pose (default `sensor.pos_x/pos_y/heading`).
    out: (String, String, String),
    source: String,
}

impl PoseFuser {
    /// Build a fuser over the given sources, writing the canonical pose entities.
    pub fn new(sources: Vec<PoseSource>, world: Arc<WorldMemory>) -> Self {
        Self {
            sources,
            world,
            out: (
                "sensor.pos_x".to_string(),
                "sensor.pos_y".to_string(),
                "sensor.heading".to_string(),
            ),
            source: "pose_fusion".to_string(),
        }
    }

    /// Override the fused-pose output entities.
    pub fn with_output(mut self, x: impl Into<String>, y: impl Into<String>, heading: impl Into<String>) -> Self {
        self.out = (x.into(), y.into(), heading.into());
        self
    }

    /// Number of configured sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Read every available source, fuse them, and write the result to the output
    /// entities + `nav.pose_fused`. Returns the fused pose, or `None` when no
    /// source has a complete reading this tick.
    pub fn fuse(&self, now_ms: u64) -> anyhow::Result<Option<Pose>> {
        let mut wsum = 0.0;
        let mut xs = 0.0;
        let mut ys = 0.0;
        let mut sin = 0.0;
        let mut cos = 0.0;
        let mut used = 0usize;

        for s in &self.sources {
            let x = self.world.current(&s.x_entity)?.and_then(|f| value_of(&f.value));
            let y = self.world.current(&s.y_entity)?.and_then(|f| value_of(&f.value));
            let h = self.world.current(&s.heading_entity)?.and_then(|f| value_of(&f.value));
            if let (Some(x), Some(y), Some(h)) = (x, y, h) {
                let w = s.weight.max(0.0);
                if w == 0.0 {
                    continue;
                }
                wsum += w;
                xs += w * x;
                ys += w * y;
                let r = h.to_radians();
                sin += w * r.sin();
                cos += w * r.cos();
                used += 1;
            }
        }

        if used == 0 || wsum <= 0.0 {
            return Ok(None);
        }

        let fused = Pose {
            x: xs / wsum,
            y: ys / wsum,
            heading_deg: sin.atan2(cos).to_degrees(),
        };

        let (ex, ey, eh) = &self.out;
        self.world.observe(ex, json!({ "value": fused.x, "sources": used }), now_ms, now_ms, &self.source)?;
        self.world.observe(ey, json!({ "value": fused.y, "sources": used }), now_ms, now_ms, &self.source)?;
        self.world.observe(eh, json!({ "value": fused.heading_deg, "sources": used }), now_ms, now_ms, &self.source)?;
        self.world.observe(
            "nav.pose_fused",
            json!({ "x": fused.x, "y": fused.y, "heading_deg": fused.heading_deg, "sources": used }),
            now_ms,
            now_ms,
            &self.source,
        )?;
        Ok(Some(fused))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(world: &WorldMemory, prefix: &str, x: f64, y: f64, h: f64, t: u64) {
        world.observe(&format!("sensor.{prefix}_x"), json!({"value": x}), t, t, "src").unwrap();
        world.observe(&format!("sensor.{prefix}_y"), json!({"value": y}), t, t, "src").unwrap();
        world.observe(&format!("sensor.{prefix}_heading"), json!({"value": h}), t, t, "src").unwrap();
    }

    #[test]
    fn weighted_average_of_two_sources() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        put(&world, "odom", 0.0, 0.0, 0.0, 1_000);
        put(&world, "gps", 10.0, 0.0, 0.0, 1_000);
        // odom weight 3, gps weight 1 → fused x = (3*0 + 1*10)/4 = 2.5
        let fuser = PoseFuser::new(
            vec![PoseSource::with_prefix("odom", 3.0), PoseSource::with_prefix("gps", 1.0)],
            Arc::clone(&world),
        );
        let p = fuser.fuse(2_000).unwrap().unwrap();
        assert!((p.x - 2.5).abs() < 1e-9);
        // fused pose written to the canonical entity the localizer reads
        let fact = world.current("sensor.pos_x").unwrap().unwrap();
        assert!((fact.value["value"].as_f64().unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn heading_uses_circular_mean() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        put(&world, "a", 0.0, 0.0, 350.0, 1_000);
        put(&world, "b", 0.0, 0.0, 10.0, 1_000);
        let fuser = PoseFuser::new(
            vec![PoseSource::with_prefix("a", 1.0), PoseSource::with_prefix("b", 1.0)],
            Arc::clone(&world),
        );
        let p = fuser.fuse(2_000).unwrap().unwrap();
        // 350° and 10° average to 0°, not 180°
        assert!(p.heading_deg.abs() < 1e-6, "got {}", p.heading_deg);
    }

    #[test]
    fn no_sources_present_returns_none() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let fuser = PoseFuser::new(vec![PoseSource::with_prefix("odom", 1.0)], Arc::clone(&world));
        assert!(fuser.fuse(1_000).unwrap().is_none());
    }

    #[test]
    fn partial_source_is_skipped() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        // only x/y for odom, missing heading → odom skipped; gps complete → used
        world.observe("sensor.odom_x", json!({"value": 5.0}), 1, 1, "s").unwrap();
        world.observe("sensor.odom_y", json!({"value": 5.0}), 1, 1, "s").unwrap();
        put(&world, "gps", 1.0, 2.0, 90.0, 1_000);
        let fuser = PoseFuser::new(
            vec![PoseSource::with_prefix("odom", 1.0), PoseSource::with_prefix("gps", 1.0)],
            Arc::clone(&world),
        );
        let p = fuser.fuse(2_000).unwrap().unwrap();
        assert!((p.x - 1.0).abs() < 1e-9 && (p.y - 2.0).abs() < 1e-9);
    }
}

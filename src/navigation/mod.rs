//! Navigation suite — the first *fusing* subsystem (localization + movement).
//!
//! Where the other suites are one-sided (sensing perceives, movement acts),
//! navigation closes a loop across two of them: it **localizes** by reading pose
//! facts the sensing layer puts in world memory (`sensor.pos_x/pos_y/heading`),
//! and **drives** toward a goal through the Track 0–bounded
//! [`MovementController`] (a steering servo + a drive motor). Perceive (pose) →
//! remember (`nav.pose`) → act (steer + drive) → remember (`nav.status`), all on
//! the shared spine, with every command still gate-bounded.
//!
//! The current goal is held internally so a host loop can step toward it on a
//! cadence while the `navigate` tool (or the agent) sets/clears it. `nav.status`
//! carries a categorical `state` (`driving`/`arrived`/`no_fix`) reflexes can match.

pub mod costmap;
pub mod exploration;
pub mod mapping;
pub mod particle;
pub mod planning;
pub mod pose_fusion;
pub mod slam;

use crate::memory::world::WorldMemory;
use crate::movement::{MovementCommand, MovementController};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Extract a scalar from a world-memory fact value (number or `{value: …}`).
fn value_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Object(o) => o.get("value").and_then(value_of),
        _ => None,
    }
}

/// Normalize an angle (degrees) to `(-180, 180]`.
fn norm180(d: f64) -> f64 {
    let m = (d + 180.0).rem_euclid(360.0);
    m - 180.0
}

/// A 2D pose estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub heading_deg: f64,
}

/// A navigation goal: a target position with an arrival tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NavGoal {
    pub x: f64,
    pub y: f64,
    /// Distance within which the goal is considered reached.
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
}

fn default_tolerance() -> f64 {
    0.5
}

/// The result of one navigation step.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NavOutcome {
    /// No goal set.
    Idle,
    /// No pose fix available (cannot localize).
    NoFix,
    /// Reached the final goal; the drive was stopped.
    Arrived { pose: Pose },
    /// Reached an intermediate waypoint; advancing to the next.
    WaypointReached { pose: Pose, remaining: usize },
    /// Driving toward the current waypoint.
    Driving {
        pose: Pose,
        distance: f64,
        heading_error: f64,
    },
}

/// Steering/drive gains and limits.
#[derive(Debug, Clone, Copy)]
pub struct NavGains {
    /// Proportional gain mapping heading error (deg) to steering angle (deg).
    pub heading_kp: f64,
    /// Maximum steering angle magnitude (deg).
    pub max_steer_deg: f64,
    /// Cruise drive speed (−1..1) when aligned with the bearing.
    pub forward_speed: f64,
    /// Heading error (deg) within which we drive at full cruise speed; beyond it
    /// the drive is reduced so the platform turns in place.
    pub align_threshold_deg: f64,
}

impl Default for NavGains {
    fn default() -> Self {
        Self {
            heading_kp: 1.0,
            max_steer_deg: 45.0,
            forward_speed: 0.5,
            align_threshold_deg: 15.0,
        }
    }
}

/// Fuses localization (pose from world memory) with movement (steer + drive) to
/// navigate toward a goal.
pub struct NavController {
    movement: Arc<MovementController>,
    world: Option<Arc<WorldMemory>>,
    steer: (String, i64),
    drive: (String, i64),
    gains: NavGains,
    pose_entities: (String, String, String),
    waypoints: Mutex<VecDeque<NavGoal>>,
    grid: Option<Arc<Mutex<planning::OccupancyGrid>>>,
    sensor_max_range: f64,
    /// Inflation params `(inscribed_radius, inflation_radius, decay)` for
    /// clearance-aware planning; `None` ⇒ plain A*.
    inflation: Option<(f64, f64, f64)>,
    source: String,
}

/// The result of an autonomous exploration step.
#[derive(Debug, Clone, PartialEq)]
pub enum ExploreOutcome {
    /// No occupancy grid configured.
    NoGrid,
    /// No pose fix — cannot pick a frontier from an unknown start.
    NoFix,
    /// Reachable space fully explored — no frontiers remain.
    Complete,
    /// Already driving toward a goal (exploration in progress).
    EnRoute { goal: NavGoal },
    /// Headed to a newly chosen frontier.
    Exploring { goal: NavGoal },
}

/// The result of an obstacle-aware planning attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanOutcome {
    /// No occupancy grid configured — caller should fall back to a direct goal.
    NoGrid,
    /// No pose fix — cannot plan from an unknown start.
    NoFix,
    /// No obstacle-free path to the goal.
    NoPath,
    /// Planned a path of `usize` waypoints (now the active path).
    Planned(usize),
}

impl NavController {
    /// Build a controller over a gated movement controller, naming the steering
    /// servo and drive motor `(name, channel)` pairs.
    pub fn new(movement: Arc<MovementController>, steer: (String, i64), drive: (String, i64)) -> Self {
        Self {
            movement,
            world: None,
            steer,
            drive,
            gains: NavGains::default(),
            pose_entities: (
                "sensor.pos_x".to_string(),
                "sensor.pos_y".to_string(),
                "sensor.heading".to_string(),
            ),
            waypoints: Mutex::new(VecDeque::new()),
            grid: None,
            sensor_max_range: 10.0,
            inflation: None,
            source: "navigation".to_string(),
        }
    }

    /// Record pose/goal/status into world memory (enables §3 Remember + reflexes).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Override steering/drive gains.
    pub fn with_gains(mut self, gains: NavGains) -> Self {
        self.gains = gains;
        self
    }

    /// Override the world-memory entities read for pose (x, y, heading).
    pub fn with_pose_entities(mut self, x: impl Into<String>, y: impl Into<String>, heading: impl Into<String>) -> Self {
        self.pose_entities = (x.into(), y.into(), heading.into());
        self
    }

    /// Override the world-memory `source` label (default `"navigation"`).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    fn waypoints_lock(&self) -> std::sync::MutexGuard<'_, VecDeque<NavGoal>> {
        self.waypoints.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Set a single navigation goal (replaces any active path; records `nav.goal`).
    pub fn set_goal(&self, goal: NavGoal, now_ms: u64) {
        self.set_path(vec![goal], now_ms);
    }

    /// Set a multi-waypoint path, driven in order (records `nav.path`). The first
    /// waypoint is also recorded as `nav.goal` for status compatibility.
    pub fn set_path(&self, goals: Vec<NavGoal>, now_ms: u64) {
        if let Some(world) = &self.world {
            let _ = world.observe(
                "nav.path",
                json!({ "count": goals.len() }),
                now_ms,
                now_ms,
                &self.source,
            );
            if let Some(first) = goals.first() {
                let _ = world.observe(
                    "nav.goal",
                    json!({ "x": first.x, "y": first.y, "tolerance": first.tolerance }),
                    now_ms,
                    now_ms,
                    &self.source,
                );
            }
        }
        *self.waypoints_lock() = goals.into_iter().collect();
    }

    /// Clear the active goal / path.
    pub fn clear_goal(&self) {
        self.waypoints_lock().clear();
    }

    /// The current (front) waypoint, if any.
    pub fn current_goal(&self) -> Option<NavGoal> {
        self.waypoints_lock().front().copied()
    }

    /// Number of waypoints still queued (including the current one).
    pub fn remaining(&self) -> usize {
        self.waypoints_lock().len()
    }

    /// Attach an occupancy grid for obstacle-aware planning.
    pub fn with_grid(mut self, grid: Arc<Mutex<planning::OccupancyGrid>>) -> Self {
        self.grid = Some(grid);
        self
    }

    /// Plan with a safety margin: cells within `inscribed_radius` of an obstacle
    /// are lethal, and proximity out to `inflation_radius` is penalized (decay
    /// rate `decay`). Without this, planning hugs obstacles.
    pub fn with_inflation(mut self, inscribed_radius: f64, inflation_radius: f64, decay: f64) -> Self {
        self.inflation = Some((inscribed_radius, inflation_radius, decay));
        self
    }

    /// Whether obstacle-aware planning is available.
    pub fn has_grid(&self) -> bool {
        self.grid.is_some()
    }

    /// Mark the grid cell at a world point occupied (or free). Returns `false`
    /// with no grid or out of bounds.
    pub fn mark_obstacle(&self, x: f64, y: f64, occupied: bool) -> bool {
        match &self.grid {
            Some(grid) => {
                let mut g = grid.lock().unwrap_or_else(|p| p.into_inner());
                g.set_world(x, y, if occupied { planning::Cell::Occupied } else { planning::Cell::Free })
            }
            None => false,
        }
    }

    /// Number of occupied cells in the grid (0 with no grid).
    pub fn obstacle_count(&self) -> usize {
        match &self.grid {
            Some(grid) => grid.lock().unwrap_or_else(|p| p.into_inner()).occupied_count(),
            None => 0,
        }
    }

    /// Sensor max range used by [`integrate_scan`](Self::integrate_scan).
    pub fn with_sensor_range(mut self, max_range: f64) -> Self {
        self.sensor_max_range = max_range;
        self
    }

    /// Integrate a range scan into the occupancy grid from the current pose:
    /// `beams` are `(bearing_deg, range)` relative to the robot heading. Localizes
    /// first; returns `false` with no grid or no pose fix. This is online mapping —
    /// perception building the map the planner uses.
    pub fn integrate_scan(&self, beams: &[(f64, f64)], now_ms: u64) -> anyhow::Result<bool> {
        let Some(grid) = &self.grid else {
            return Ok(false);
        };
        let Some(pose) = self.estimate_pose(now_ms)? else {
            return Ok(false);
        };
        let mut g = grid.lock().unwrap_or_else(|p| p.into_inner());
        mapping::integrate_scan(&mut g, pose.x, pose.y, pose.heading_deg, beams, self.sensor_max_range);
        Ok(true)
    }

    /// One autonomous-exploration step: if not already en route to a goal, pick
    /// the nearest reachable frontier and plan a route to it. When no frontiers
    /// remain, the reachable space is fully explored ([`ExploreOutcome::Complete`]).
    pub fn explore_step(&self, now_ms: u64) -> anyhow::Result<ExploreOutcome> {
        if self.grid.is_none() {
            return Ok(ExploreOutcome::NoGrid);
        }
        // Still driving toward the last frontier — let the drive loop finish it.
        if let Some(goal) = self.current_goal() {
            return Ok(ExploreOutcome::EnRoute { goal });
        }
        let Some(pose) = self.estimate_pose(now_ms)? else {
            return Ok(ExploreOutcome::NoFix);
        };
        let goal = {
            let grid = self.grid.as_ref().unwrap();
            let g = grid.lock().unwrap_or_else(|p| p.into_inner());
            exploration::nearest_frontier_goal(&g, (pose.x, pose.y), 0.5)
        };
        match goal {
            Some(goal) => {
                self.set_path(vec![goal], now_ms);
                Ok(ExploreOutcome::Exploring { goal })
            }
            None => Ok(ExploreOutcome::Complete),
        }
    }

    /// Plan an obstacle-free path from the current pose to `goal` and set it as
    /// the active path. Records `nav.pose` (via localization). See [`PlanOutcome`].
    pub fn plan_to(&self, goal: NavGoal, now_ms: u64) -> anyhow::Result<PlanOutcome> {
        let Some(grid) = &self.grid else {
            return Ok(PlanOutcome::NoGrid);
        };
        let Some(pose) = self.estimate_pose(now_ms)? else {
            return Ok(PlanOutcome::NoFix);
        };
        let goals = {
            let g = grid.lock().unwrap_or_else(|p| p.into_inner());
            match self.inflation {
                Some((inscribed, infl, decay)) => {
                    // Clearance-aware: inflate obstacles, then plan with a margin.
                    let field = costmap::inflate(&g, inscribed, infl, decay);
                    costmap::plan_inflated(&g, &field, (pose.x, pose.y), (goal.x, goal.y)).map(|pts| {
                        pts.into_iter()
                            .map(|(x, y)| NavGoal { x, y, tolerance: goal.tolerance })
                            .collect::<Vec<_>>()
                    })
                }
                None => planning::plan_goals(&g, (pose.x, pose.y), (goal.x, goal.y), goal.tolerance),
            }
        };
        match goals {
            Some(goals) => {
                let n = goals.len();
                self.set_path(goals, now_ms);
                Ok(PlanOutcome::Planned(n))
            }
            None => Ok(PlanOutcome::NoPath),
        }
    }

    /// Estimate the current pose from the configured world-memory entities,
    /// recording it as `nav.pose`. `None` when any component is missing.
    pub fn estimate_pose(&self, now_ms: u64) -> anyhow::Result<Option<Pose>> {
        let Some(world) = &self.world else {
            return Ok(None);
        };
        let (ex, ey, eh) = &self.pose_entities;
        let x = world.current(ex)?.and_then(|f| value_of(&f.value));
        let y = world.current(ey)?.and_then(|f| value_of(&f.value));
        let h = world.current(eh)?.and_then(|f| value_of(&f.value));
        let (Some(x), Some(y), Some(heading_deg)) = (x, y, h) else {
            return Ok(None);
        };
        let pose = Pose { x, y, heading_deg };
        world.observe(
            "nav.pose",
            json!({ "x": pose.x, "y": pose.y, "heading_deg": pose.heading_deg }),
            now_ms,
            now_ms,
            &self.source,
        )?;
        Ok(Some(pose))
    }

    fn record_status(&self, now_ms: u64, body: Value) {
        if let Some(world) = &self.world {
            let _ = world.observe("nav.status", body, now_ms, now_ms, &self.source);
        }
    }

    async fn apply(&self, cmd: MovementCommand, now_ms: u64) -> anyhow::Result<()> {
        self.movement
            .apply(&cmd, now_ms)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
            .map(|_| ())
    }

    /// Stop now: clear the goal and command the drive to stop. Safe/reversible —
    /// always allowed without approval.
    pub async fn halt(&self, now_ms: u64) -> anyhow::Result<()> {
        self.clear_goal();
        self.apply(MovementCommand::Stop { name: self.drive.0.clone(), channel: self.drive.1 }, now_ms)
            .await?;
        self.record_status(now_ms, json!({ "state": "halted" }));
        Ok(())
    }

    /// One navigation step toward the current goal. Localizes, computes the
    /// bearing/heading error, commands steer + drive (or stops on arrival), and
    /// records `nav.status`. A no-op (`Idle`) when no goal is set.
    pub async fn step_toward_goal(&self, now_ms: u64) -> anyhow::Result<NavOutcome> {
        let Some(goal) = self.current_goal() else {
            return Ok(NavOutcome::Idle);
        };
        let Some(pose) = self.estimate_pose(now_ms)? else {
            self.record_status(now_ms, json!({ "state": "no_fix" }));
            return Ok(NavOutcome::NoFix);
        };

        let dx = goal.x - pose.x;
        let dy = goal.y - pose.y;
        let distance = (dx * dx + dy * dy).sqrt();

        if distance <= goal.tolerance {
            // Reached this waypoint — advance the path.
            let remaining = {
                let mut wp = self.waypoints_lock();
                wp.pop_front();
                wp.len()
            };
            if remaining == 0 {
                // Final goal: stop the drive.
                self.apply(MovementCommand::Stop { name: self.drive.0.clone(), channel: self.drive.1 }, now_ms)
                    .await?;
                self.record_status(now_ms, json!({ "state": "arrived", "distance": distance }));
                return Ok(NavOutcome::Arrived { pose });
            }
            // More waypoints ahead — keep rolling toward the next one.
            self.record_status(
                now_ms,
                json!({ "state": "waypoint_reached", "remaining": remaining }),
            );
            return Ok(NavOutcome::WaypointReached { pose, remaining });
        }

        let bearing = dy.atan2(dx).to_degrees();
        let heading_error = norm180(bearing - pose.heading_deg);

        // Steering: proportional to heading error, clamped.
        let steer_deg = (self.gains.heading_kp * heading_error)
            .clamp(-self.gains.max_steer_deg, self.gains.max_steer_deg);
        // Drive: full cruise when roughly aligned, reduced while turning in place.
        let aligned = heading_error.abs() <= self.gains.align_threshold_deg;
        let speed = if aligned { self.gains.forward_speed } else { self.gains.forward_speed * 0.3 };

        self.apply(
            MovementCommand::ServoAngle { name: self.steer.0.clone(), channel: self.steer.1, degrees: steer_deg },
            now_ms,
        )
        .await?;
        self.apply(
            MovementCommand::MotorSpeed { name: self.drive.0.clone(), channel: self.drive.1, speed },
            now_ms,
        )
        .await?;

        self.record_status(
            now_ms,
            json!({ "state": "driving", "distance": distance, "heading_error": heading_error }),
        );
        Ok(NavOutcome::Driving { pose, distance, heading_error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::LoggingActuatorSink;
    use crate::security::limits::{SafetyGate, SafetyLimit};

    fn nav(world: &Arc<WorldMemory>) -> NavController {
        let mut steer_lim = SafetyLimit::new("rover", "servo_angle");
        steer_lim.allowed_pins = Some(vec![0]);
        steer_lim.value_min = Some(-90);
        steer_lim.value_max = Some(90);
        let mut drive_lim = SafetyLimit::new("rover", "motor_speed");
        drive_lim.allowed_pins = Some(vec![1]);
        drive_lim.value_min = Some(-100);
        drive_lim.value_max = Some(100);
        let mut stop_lim = SafetyLimit::new("rover", "stop");
        stop_lim.allowed_pins = Some(vec![1]);
        stop_lim.value_min = Some(0);
        stop_lim.value_max = Some(0);
        let movement = Arc::new(
            MovementController::new(
                "rover",
                Arc::new(SafetyGate::new(vec![steer_lim, drive_lim, stop_lim])),
                Arc::new(LoggingActuatorSink),
            )
            .with_world_memory(Arc::clone(world)),
        );
        NavController::new(movement, ("steer".to_string(), 0), ("drive".to_string(), 1))
            .with_world_memory(Arc::clone(world))
    }

    fn set_pose(world: &WorldMemory, x: f64, y: f64, heading: f64, t: u64) {
        world.observe("sensor.pos_x", json!({"value": x}), t, t, "odom").unwrap();
        world.observe("sensor.pos_y", json!({"value": y}), t, t, "odom").unwrap();
        world.observe("sensor.heading", json!({"value": heading}), t, t, "odom").unwrap();
    }

    #[test]
    fn norm180_wraps() {
        assert!((norm180(190.0) - (-170.0)).abs() < 1e-9);
        assert!((norm180(-190.0) - 170.0).abs() < 1e-9);
        assert!((norm180(45.0) - 45.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_pose_reads_and_records() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        set_pose(&world, 1.0, 2.0, 90.0, 1_000);
        let pose = n.estimate_pose(1_000).unwrap().unwrap();
        assert_eq!(pose, Pose { x: 1.0, y: 2.0, heading_deg: 90.0 });
        let fact = world.current("nav.pose").unwrap().unwrap();
        assert_eq!(fact.value["heading_deg"], 90.0);
    }

    #[tokio::test]
    async fn idle_without_goal() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        assert_eq!(n.step_toward_goal(1_000).await.unwrap(), NavOutcome::Idle);
    }

    #[tokio::test]
    async fn no_fix_without_pose() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        n.set_goal(NavGoal { x: 5.0, y: 0.0, tolerance: 0.5 }, 1_000);
        assert_eq!(n.step_toward_goal(1_000).await.unwrap(), NavOutcome::NoFix);
    }

    #[tokio::test]
    async fn arrival_stops_drive_and_clears_goal() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        set_pose(&world, 5.0, 0.0, 0.0, 1_000);
        n.set_goal(NavGoal { x: 5.1, y: 0.0, tolerance: 0.5 }, 1_000);
        let out = n.step_toward_goal(2_000).await.unwrap();
        assert!(matches!(out, NavOutcome::Arrived { .. }));
        assert!(n.current_goal().is_none());
        assert_eq!(world.current("nav.status").unwrap().unwrap().value["state"], "arrived");
    }

    #[tokio::test]
    async fn side_goal_produces_steer_and_drives() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        // facing east (0°), goal due north → bearing 90°, large heading error
        set_pose(&world, 0.0, 0.0, 0.0, 1_000);
        n.set_goal(NavGoal { x: 0.0, y: 10.0, tolerance: 0.5 }, 1_000);
        let out = n.step_toward_goal(2_000).await.unwrap();
        match out {
            NavOutcome::Driving { heading_error, distance, .. } => {
                assert!((heading_error - 90.0).abs() < 1e-6);
                assert!((distance - 10.0).abs() < 1e-6);
            }
            other => panic!("expected driving, got {other:?}"),
        }
        // a steering command was recorded as the actuator's state
        let steer = world.current("actuator.steer").unwrap().unwrap();
        assert!(steer.value["value"].as_f64().unwrap().abs() > 1.0);
    }

    #[tokio::test]
    async fn multi_waypoint_path_advances_then_arrives() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        // Two waypoints: (2,0) then (4,0); robot starts at origin facing east.
        n.set_path(
            vec![
                NavGoal { x: 2.0, y: 0.0, tolerance: 0.3 },
                NavGoal { x: 4.0, y: 0.0, tolerance: 0.3 },
            ],
            0,
        );
        assert_eq!(n.remaining(), 2);

        // At the first waypoint → WaypointReached, advances to the second.
        set_pose(&world, 2.0, 0.0, 0.0, 1_000);
        let out = n.step_toward_goal(1_000).await.unwrap();
        assert!(matches!(out, NavOutcome::WaypointReached { remaining: 1, .. }));
        assert_eq!(n.remaining(), 1);
        assert!((n.current_goal().unwrap().x - 4.0).abs() < 1e-9);

        // At the second (final) waypoint → Arrived, queue empty.
        set_pose(&world, 4.0, 0.0, 0.0, 2_000);
        let out = n.step_toward_goal(2_000).await.unwrap();
        assert!(matches!(out, NavOutcome::Arrived { .. }));
        assert_eq!(n.remaining(), 0);
    }

    #[tokio::test]
    async fn plan_to_routes_around_obstacle() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let mut grid = planning::OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cy in 0..8 {
            grid.set(5, cy, planning::Cell::Occupied); // wall with a gap at the top
        }
        let n = nav(&world).with_grid(Arc::new(Mutex::new(grid)));
        set_pose(&world, 0.5, 0.5, 0.0, 1_000);
        let out = n.plan_to(NavGoal { x: 9.5, y: 0.5, tolerance: 0.3 }, 1_000).unwrap();
        match out {
            PlanOutcome::Planned(k) => assert!(k >= 2, "expected a detour, got {k} waypoints"),
            other => panic!("expected Planned, got {other:?}"),
        }
        assert!(n.remaining() >= 2);
    }

    #[tokio::test]
    async fn scan_builds_map_used_by_planning() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world)
            .with_grid(Arc::new(Mutex::new(planning::OccupancyGrid::new(0.0, 0.0, 1.0, 12, 12))));
        set_pose(&world, 0.5, 0.5, 0.0, 1_000);
        // a beam straight ahead hits an obstacle at range 4 → a cell gets marked
        let used = n.integrate_scan(&[(0.0, 4.0)], 1_000).unwrap();
        assert!(used);
        assert!(n.obstacle_count() >= 1, "the scan should mark an obstacle");
    }

    #[tokio::test]
    async fn explore_step_picks_a_frontier_then_reports_en_route() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let mut grid = planning::OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cx in 1..4 {
            for cy in 1..4 {
                grid.set(cx, cy, planning::Cell::Free); // a known pocket in unknown
            }
        }
        let n = nav(&world).with_grid(Arc::new(Mutex::new(grid)));
        set_pose(&world, 2.5, 2.5, 0.0, 1_000);

        let out = n.explore_step(1_000).unwrap();
        assert!(matches!(out, ExploreOutcome::Exploring { .. }), "should head to a frontier, got {out:?}");
        assert!(n.current_goal().is_some());
        // still driving there → en route, not a new pick
        assert!(matches!(n.explore_step(1_000).unwrap(), ExploreOutcome::EnRoute { .. }));
    }

    #[tokio::test]
    async fn plan_to_with_inflation_refuses_a_too_tight_gap() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let mut grid = planning::OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cy in 1..10 {
            grid.set(5, cy, planning::Cell::Occupied); // wall, one-cell gap at (5,0)
        }
        // a wide robot (inscribed 1.5) can't fit the gap → clearance planner refuses
        let n = nav(&world)
            .with_grid(Arc::new(Mutex::new(grid)))
            .with_inflation(1.5, 3.0, 1.0);
        set_pose(&world, 0.5, 0.5, 0.0, 1_000);
        assert_eq!(
            n.plan_to(NavGoal { x: 9.5, y: 0.5, tolerance: 0.3 }, 1_000).unwrap(),
            PlanOutcome::NoPath
        );
    }

    #[tokio::test]
    async fn plan_to_without_grid_reports_no_grid() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        set_pose(&world, 0.0, 0.0, 0.0, 1_000);
        assert_eq!(
            n.plan_to(NavGoal { x: 1.0, y: 0.0, tolerance: 0.3 }, 1_000).unwrap(),
            PlanOutcome::NoGrid
        );
    }

    #[tokio::test]
    async fn converges_to_goal_ahead_over_steps() {
        // Simple sim: facing the goal (east), each driving step advances x by the
        // commanded speed × a step scale; heading stays aligned.
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let n = nav(&world);
        let mut x = 0.0;
        set_pose(&world, x, 0.0, 0.0, 0);
        n.set_goal(NavGoal { x: 5.0, y: 0.0, tolerance: 0.3 }, 0);

        let mut arrived = false;
        let mut t = 1_000u64;
        for _ in 0..200 {
            match n.step_toward_goal(t).await.unwrap() {
                NavOutcome::Arrived { .. } => {
                    arrived = true;
                    break;
                }
                NavOutcome::Driving { .. } => {
                    // plant integrates the commanded drive (speed 0.5 → +0.5 m/step)
                    x += 0.5;
                    set_pose(&world, x, 0.0, 0.0, t);
                }
                other => panic!("unexpected {other:?}"),
            }
            t += 1_000;
        }
        assert!(arrived, "navigation did not converge");
        assert!(n.current_goal().is_none());
    }
}

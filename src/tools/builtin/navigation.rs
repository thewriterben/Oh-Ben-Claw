//! Navigation tools — set a goal (gated) and observe/stop (safe).
//!
//! Two tools with deliberately different risk postures: **`navigate`** commits
//! the platform to drive toward a goal, so it is classed physical/high-blast and
//! approval-gated (the actual motion is still Track 0–bounded in the movement
//! controller). **`nav_status`** only observes or *stops*, so it is `safe` —
//! halting and querying must never require approval.

use crate::memory::world::WorldMemory;
use crate::navigation::{NavController, NavGoal, PlanOutcome};
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
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

/// Parse scan beams from a JSON value: an array of `[bearing_deg, range]` pairs
/// or `{angle, range}` objects. Malformed entries are skipped.
fn parse_beams(v: Option<&Value>) -> Vec<(f64, f64)> {
    let Some(Value::Array(arr)) = v else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|b| -> Option<(f64, f64)> {
            if let Some(pair) = b.as_array() {
                Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
            } else {
                Some((b.get("angle")?.as_f64()?, b.get("range")?.as_f64()?))
            }
        })
        .collect()
}

/// Tool: set a navigation goal (starts autonomous driving toward it).
pub struct NavigateTool {
    controller: Arc<NavController>,
}

impl NavigateTool {
    pub fn new(controller: Arc<NavController>) -> Self {
        Self { controller }
    }
}

#[async_trait]
impl Tool for NavigateTool {
    fn name(&self) -> &str {
        "navigate"
    }

    fn description(&self) -> &str {
        "Drive toward a goal or along a waypoint path. Provide a single goal via \
         `x`, `y` (+ optional `tolerance`), OR a `waypoints` array of \
         {x, y, tolerance?} driven in order. The platform localizes from sensor \
         pose and steers/drives toward each, bounded by the node's Track 0 limits. \
         Physical action — approval-gated. Use nav_status to stop or check progress."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "number", "description": "Single-goal X coordinate." },
                "y": { "type": "number", "description": "Single-goal Y coordinate." },
                "tolerance": { "type": "number", "description": "Arrival radius (default 0.5)." },
                "waypoints": {
                    "type": "array",
                    "description": "Ordered path; overrides x/y when present.",
                    "items": {
                        "type": "object",
                        "required": ["x", "y"],
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "tolerance": { "type": "number" }
                        }
                    }
                }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        // Commits the platform to move — physical, high blast: per-call approval.
        RiskClass::physical(true, BlastRadius::High)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        // A waypoint path takes precedence over a single goal.
        if let Some(wp) = args.get("waypoints") {
            let goals: Vec<NavGoal> = match serde_json::from_value(wp.clone()) {
                Ok(g) => g,
                Err(e) => return Ok(ToolResult::err(format!("invalid waypoints: {e}"))),
            };
            if goals.is_empty() {
                return Ok(ToolResult::err("'waypoints' must be non-empty"));
            }
            let n = goals.len();
            self.controller.set_path(goals, now_ms());
            return Ok(ToolResult::ok(
                json!({ "waypoints": n, "status": "navigating" }).to_string(),
            ));
        }
        let goal: NavGoal = match serde_json::from_value(args) {
            Ok(g) => g,
            Err(e) => return Ok(ToolResult::err(format!("invalid goal: {e}"))),
        };
        // Obstacle-aware: plan a path around known obstacles when a grid is set.
        if self.controller.has_grid() {
            match self.controller.plan_to(goal, now_ms())? {
                PlanOutcome::Planned(n) => {
                    return Ok(ToolResult::ok(
                        json!({ "goal": goal, "planned_waypoints": n, "status": "navigating" })
                            .to_string(),
                    ))
                }
                PlanOutcome::NoPath => {
                    return Ok(ToolResult::err("no obstacle-free path to the goal"))
                }
                // No fix or no grid → fall through to a direct goal (best effort).
                PlanOutcome::NoFix | PlanOutcome::NoGrid => {}
            }
        }
        self.controller.set_goal(goal, now_ms());
        Ok(ToolResult::ok(
            json!({ "goal": goal, "status": "navigating" }).to_string(),
        ))
    }
}

/// Tool: build the occupancy map — mark obstacles / free space (always safe).
pub struct NavMapTool {
    controller: Arc<NavController>,
}

impl NavMapTool {
    pub fn new(controller: Arc<NavController>) -> Self {
        Self { controller }
    }
}

#[async_trait]
impl Tool for NavMapTool {
    fn name(&self) -> &str {
        "nav_map"
    }

    fn description(&self) -> &str {
        "Update the navigation occupancy map. Set `action` to 'mark' (mark the \
         cell at x,y occupied — an obstacle), 'free' (mark it clear), 'scan' \
         (integrate a range scan from the current pose: `beams` is an array of \
         [bearing_deg, range] relative to heading — clears free space and marks \
         hits), or 'status'. The map feeds obstacle-aware path planning. \
         Non-actuating — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["mark", "free", "scan", "status"] },
                "x": { "type": "number", "description": "World X (mark/free)." },
                "y": { "type": "number", "description": "World Y (mark/free)." },
                "beams": {
                    "type": "array",
                    "description": "Range beams for 'scan': each [bearing_deg, range].",
                    "items": { "type": "array", "items": { "type": "number" } }
                }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "scan" {
            let beams = parse_beams(args.get("beams"));
            if beams.is_empty() {
                return Ok(ToolResult::err("'scan' requires a non-empty 'beams' array"));
            }
            return Ok(match self.controller.integrate_scan(&beams, now_ms())? {
                true => ToolResult::ok(
                    json!({ "integrated": beams.len(), "obstacles": self.controller.obstacle_count() })
                        .to_string(),
                ),
                false => ToolResult::err("no grid configured or no pose fix"),
            });
        }
        if action == "status" {
            return Ok(ToolResult::ok(
                json!({
                    "has_grid": self.controller.has_grid(),
                    "obstacles": self.controller.obstacle_count(),
                })
                .to_string(),
            ));
        }
        let (Some(x), Some(y)) = (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) else {
            return Ok(ToolResult::err(format!("'{action}' requires 'x' and 'y'")));
        };
        let occupied = match action {
            "mark" => true,
            "free" => false,
            other => return Ok(ToolResult::err(format!("unknown action: '{other}'"))),
        };
        if self.controller.mark_obstacle(x, y, occupied) {
            Ok(ToolResult::ok(
                json!({ "x": x, "y": y, "occupied": occupied, "obstacles": self.controller.obstacle_count() })
                    .to_string(),
            ))
        } else {
            Ok(ToolResult::err("no grid configured or point out of bounds"))
        }
    }
}

/// Tool: observe navigation state or stop — always safe (no approval).
pub struct NavStatusTool {
    controller: Arc<NavController>,
    world: Arc<WorldMemory>,
}

impl NavStatusTool {
    pub fn new(controller: Arc<NavController>, world: Arc<WorldMemory>) -> Self {
        Self { controller, world }
    }

    fn status(&self) -> ToolResult {
        let cur = |e: &str| self.world.current(e).ok().flatten().map(|f| f.value);
        ToolResult::ok(
            json!({
                "goal": self.controller.current_goal(),
                "remaining": self.controller.remaining(),
                "pose": cur("nav.pose"),
                "status": cur("nav.status"),
            })
            .to_string(),
        )
    }

    async fn stop(&self) -> ToolResult {
        match self.controller.halt(now_ms()).await {
            Ok(()) => ToolResult::ok(json!({ "status": "halted" }).to_string()),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }
}

#[async_trait]
impl Tool for NavStatusTool {
    fn name(&self) -> &str {
        "nav_status"
    }

    fn description(&self) -> &str {
        "Observe or stop navigation. Set `action` to 'status' (current goal, pose, \
         and driving state) or 'stop' (clear the goal and stop the drive). \
         Non-actuating except for the always-safe stop — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["status", "stop"], "description": "Operation." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        Ok(
            match args.get("action").and_then(Value::as_str).unwrap_or("") {
                "status" => self.status(),
                "stop" => self.stop().await,
                other => ToolResult::err(format!("unknown action: '{other}'")),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::{LoggingActuatorSink, MovementController};
    use crate::security::limits::{SafetyGate, SafetyLimit};

    fn controller(world: &Arc<WorldMemory>) -> Arc<NavController> {
        let mut steer = SafetyLimit::new("rover", "servo_angle");
        steer.allowed_pins = Some(vec![0]);
        steer.value_min = Some(-90);
        steer.value_max = Some(90);
        let mut drive = SafetyLimit::new("rover", "motor_speed");
        drive.allowed_pins = Some(vec![1]);
        drive.value_min = Some(-100);
        drive.value_max = Some(100);
        let mut stop = SafetyLimit::new("rover", "stop");
        stop.allowed_pins = Some(vec![1]);
        stop.value_min = Some(0);
        stop.value_max = Some(0);
        let movement = Arc::new(
            MovementController::new(
                "rover",
                Arc::new(SafetyGate::new(vec![steer, drive, stop])),
                Arc::new(LoggingActuatorSink),
            )
            .with_world_memory(Arc::clone(world)),
        );
        Arc::new(
            NavController::new(movement, ("steer".into(), 0), ("drive".into(), 1))
                .with_world_memory(Arc::clone(world)),
        )
    }

    #[test]
    fn navigate_gated_status_safe() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let c = controller(&world);
        assert!(NavigateTool::new(Arc::clone(&c))
            .risk_class()
            .requires_per_call_approval());
        assert!(!NavStatusTool::new(c, world).risk_class().physical);
    }

    #[tokio::test]
    async fn navigate_sets_goal_then_status_and_stop() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let c = controller(&world);
        let nav = NavigateTool::new(Arc::clone(&c));
        let st = NavStatusTool::new(Arc::clone(&c), Arc::clone(&world));

        let r = nav.execute(json!({ "x": 3.0, "y": 4.0 })).await.unwrap();
        assert!(r.success, "navigate failed: {:?}", r.error);
        assert!(c.current_goal().is_some());

        let r = st.execute(json!({ "action": "status" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!((v["goal"]["x"].as_f64().unwrap() - 3.0).abs() < 1e-9);

        let r = st.execute(json!({ "action": "stop" })).await.unwrap();
        assert!(r.success);
        assert!(c.current_goal().is_none());
    }

    #[tokio::test]
    async fn invalid_goal_is_soft_error() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let nav = NavigateTool::new(controller(&world));
        let r = nav.execute(json!({ "x": 1.0 })).await.unwrap(); // missing y
        assert!(!r.success);
    }

    #[tokio::test]
    async fn navigate_accepts_waypoint_path() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let c = controller(&world);
        let nav = NavigateTool::new(Arc::clone(&c));
        let r = nav
            .execute(json!({ "waypoints": [{ "x": 1.0, "y": 0.0 }, { "x": 2.0, "y": 0.0 }] }))
            .await
            .unwrap();
        assert!(r.success, "waypoints failed: {:?}", r.error);
        assert_eq!(c.remaining(), 2);
    }

    fn controller_with_grid(world: &Arc<WorldMemory>) -> Arc<NavController> {
        use crate::navigation::planning::OccupancyGrid;
        use std::sync::Mutex;
        // unwrap the Arc-built controller by rebuilding with a grid attached
        let mut steer = SafetyLimit::new("rover", "servo_angle");
        steer.allowed_pins = Some(vec![0]);
        steer.value_min = Some(-90);
        steer.value_max = Some(90);
        let mut drive = SafetyLimit::new("rover", "motor_speed");
        drive.allowed_pins = Some(vec![1]);
        drive.value_min = Some(-100);
        drive.value_max = Some(100);
        let mut stop = SafetyLimit::new("rover", "stop");
        stop.allowed_pins = Some(vec![1]);
        stop.value_min = Some(0);
        stop.value_max = Some(0);
        let movement = Arc::new(
            MovementController::new(
                "rover",
                Arc::new(SafetyGate::new(vec![steer, drive, stop])),
                Arc::new(LoggingActuatorSink),
            )
            .with_world_memory(Arc::clone(world)),
        );
        Arc::new(
            NavController::new(movement, ("steer".into(), 0), ("drive".into(), 1))
                .with_world_memory(Arc::clone(world))
                .with_grid(Arc::new(Mutex::new(OccupancyGrid::new(
                    0.0, 0.0, 1.0, 10, 10,
                )))),
        )
    }

    #[tokio::test]
    async fn nav_map_marks_then_navigate_plans_around() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let c = controller_with_grid(&world);
        let map = NavMapTool::new(Arc::clone(&c));
        // wall at x≈5 with a gap at the top
        for y in 0..8 {
            let r = map
                .execute(json!({ "action": "mark", "x": 5.5, "y": y as f64 + 0.5 }))
                .await
                .unwrap();
            assert!(r.success, "mark failed: {:?}", r.error);
        }
        let s = map.execute(json!({ "action": "status" })).await.unwrap();
        let v: Value = serde_json::from_str(&s.output).unwrap();
        assert_eq!(v["obstacles"], 8);

        // pose at the start, then a planned navigate detours around the wall
        world
            .observe("sensor.pos_x", json!({"value": 0.5}), 1, 1, "o")
            .unwrap();
        world
            .observe("sensor.pos_y", json!({"value": 0.5}), 1, 1, "o")
            .unwrap();
        world
            .observe("sensor.heading", json!({"value": 0.0}), 1, 1, "o")
            .unwrap();
        let r = NavigateTool::new(Arc::clone(&c))
            .execute(json!({ "x": 9.5, "y": 0.5 }))
            .await
            .unwrap();
        assert!(r.success, "navigate failed: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!(
            v["planned_waypoints"].as_u64().unwrap() >= 2,
            "expected a detour"
        );
    }

    #[tokio::test]
    async fn nav_map_scan_marks_obstacles_from_pose() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let c = controller_with_grid(&world);
        world
            .observe("sensor.pos_x", json!({"value": 0.5}), 1, 1, "o")
            .unwrap();
        world
            .observe("sensor.pos_y", json!({"value": 0.5}), 1, 1, "o")
            .unwrap();
        world
            .observe("sensor.heading", json!({"value": 0.0}), 1, 1, "o")
            .unwrap();
        let map = NavMapTool::new(Arc::clone(&c));
        let r = map
            .execute(json!({ "action": "scan", "beams": [[0.0, 4.0]] }))
            .await
            .unwrap();
        assert!(r.success, "scan failed: {:?}", r.error);
        assert!(c.obstacle_count() >= 1);
    }
}

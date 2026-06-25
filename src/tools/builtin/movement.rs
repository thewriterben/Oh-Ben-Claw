//! Movement tool — typed, safety-bounded actuation exposed to the agent (System 2).
//!
//! Wraps the Movement subsystem's [`MovementController`] so the LLM agent can
//! command a servo / motor / stop. Classed **physical, high-blast** so the
//! approval layer requires per-call approval and never auto-grants it to
//! `forever` (Subsystem Suite §7). The command is bounded by the Track 0
//! `SafetyGate` inside the controller and recorded into world memory as the
//! actuator's current state *before* it actuates.

use crate::movement::{MovementCommand, MovementController};
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

/// Tool: drive a physical actuator through the safety-bounded movement controller.
pub struct MovementTool {
    controller: Arc<MovementController>,
}

impl MovementTool {
    /// Build a tool over a shared movement controller.
    pub fn new(controller: Arc<MovementController>) -> Self {
        Self { controller }
    }
}

#[async_trait]
impl Tool for MovementTool {
    fn name(&self) -> &str {
        "move_actuator"
    }

    fn description(&self) -> &str {
        "Drive a physical actuator with a typed, safety-bounded command. Set \
         `type` to 'servo_angle' (with name, channel, degrees), 'motor_speed' \
         (with name, channel, speed in -1.0..1.0), or 'stop' (with name, \
         channel). Every command is bounded by the node's deterministic Track 0 \
         limits and recorded as the actuator's current state. Approval-gated; \
         physical action."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["type", "name", "channel"],
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["servo_angle", "motor_speed", "stop"],
                    "description": "Movement kind."
                },
                "name": {
                    "type": "string",
                    "description": "Stable actuator id (e.g. 'arm', 'gripper'). Becomes actuator.{name} in world memory."
                },
                "channel": {
                    "type": "integer",
                    "description": "Hardware channel for the actuator."
                },
                "degrees": {
                    "type": "number",
                    "description": "Absolute angle in degrees (required for 'servo_angle')."
                },
                "speed": {
                    "type": "number",
                    "minimum": -1.0,
                    "maximum": 1.0,
                    "description": "Signed throttle (required for 'motor_speed')."
                }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        // Physical actuation, high blast radius (drives a motor/servo): the
        // approval layer requires per-call approval and refuses `forever`.
        RiskClass::physical(true, BlastRadius::High)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let cmd: MovementCommand = match serde_json::from_value(args) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(format!("invalid movement command: {e}"))),
        };
        match self.controller.apply(&cmd, now_ms()).await {
            Ok(applied) => Ok(ToolResult::ok(
                serde_json::to_string(&applied).unwrap_or_else(|_| "{}".to_string()),
            )),
            Err(e) => Ok(ToolResult::err(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::world::WorldMemory;
    use crate::movement::LoggingActuatorSink;
    use crate::security::limits::{SafetyGate, SafetyLimit};

    fn tool() -> (MovementTool, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let mut limit = SafetyLimit::new("n1", "servo_angle");
        limit.allowed_pins = Some(vec![0]);
        limit.value_min = Some(0);
        limit.value_max = Some(180);
        let ctrl = Arc::new(
            MovementController::new("n1", Arc::new(SafetyGate::new(vec![limit])), Arc::new(LoggingActuatorSink))
                .with_world_memory(Arc::clone(&world)),
        );
        (MovementTool::new(ctrl), world)
    }

    #[test]
    fn classed_physical_high_blast_per_call_approval() {
        let (t, _) = tool();
        let rc = t.risk_class();
        assert!(rc.physical);
        assert_eq!(rc.blast, BlastRadius::High);
        assert!(rc.requires_per_call_approval());
    }

    #[tokio::test]
    async fn valid_servo_command_applies_and_records() {
        let (t, world) = tool();
        let r = t
            .execute(json!({ "type": "servo_angle", "name": "arm", "channel": 0, "degrees": 90 }))
            .await
            .unwrap();
        assert!(r.success, "expected ok, got {:?}", r.error);
        let fact = world.current("actuator.arm").unwrap().unwrap();
        assert!((fact.value["value"].as_f64().unwrap() - 90.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn out_of_range_is_soft_error() {
        let (t, world) = tool();
        let r = t
            .execute(json!({ "type": "servo_angle", "name": "arm", "channel": 0, "degrees": 250 }))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(world.current("actuator.arm").unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_command_is_soft_error() {
        let (t, _) = tool();
        let r = t.execute(json!({ "type": "fly", "name": "arm", "channel": 0 })).await.unwrap();
        assert!(!r.success);
    }
}

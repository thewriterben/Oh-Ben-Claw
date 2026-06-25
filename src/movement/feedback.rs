//! Closed-loop movement — feedback control (Subsystem Suite §6 Accelerate, L3).
//!
//! Open-loop movement commands a target and hopes; closed-loop *reads back* where
//! the actuator actually is (a feedback fact in world memory) and corrects toward
//! the target each tick. This is the depth step beyond gate-checked dispatch: a
//! proportional controller drives the error to zero, every commanded step still
//! passing through the Track 0 [`MovementController`] gate and recorded in memory.
//!
//! The feedback signal is just another world-memory entity (e.g. a
//! `sensor.{joint}_angle` reading produced by the sensing suite or the node), so
//! perception and actuation close the loop through the same shared memory the
//! rest of the system uses.

use crate::memory::world::WorldMemory;
use crate::movement::{MovementCommand, MovementController};
use serde_json::Value;
use std::sync::Arc;

/// Extract a scalar from a world-memory fact value (number or `{value: …}` object).
fn value_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Object(o) => o.get("value").and_then(value_of),
        _ => None,
    }
}

/// A bounded proportional controller. `correction = clamp(kp * error, ±max_step)`
/// where `error = target − measured`; within `tolerance` the loop is settled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PController {
    /// Proportional gain. Keep `≤ 1.0` to avoid overshoot with this simple law.
    pub kp: f64,
    /// Absolute error within which the loop is considered settled.
    pub tolerance: f64,
    /// Maximum magnitude of a single correction step (rate/jerk limit).
    pub max_step: f64,
}

impl Default for PController {
    fn default() -> Self {
        Self {
            kp: 0.5,
            tolerance: 1.0,
            max_step: 45.0,
        }
    }
}

impl PController {
    /// Whether the loop is settled (|error| ≤ tolerance).
    pub fn settled(&self, target: f64, measured: f64) -> bool {
        (target - measured).abs() <= self.tolerance
    }

    /// The correction to apply this tick (0.0 when settled).
    pub fn correction(&self, target: f64, measured: f64) -> f64 {
        let error = target - measured;
        if error.abs() <= self.tolerance {
            return 0.0;
        }
        (self.kp * error).clamp(-self.max_step, self.max_step)
    }
}

/// The result of one closed-loop control step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// Already within tolerance; nothing commanded.
    Settled { measured: f64 },
    /// A correction was commanded toward the target.
    Adjusted { measured: f64, commanded: f64 },
    /// No feedback fact available — cannot close the loop this tick.
    NoFeedback,
}

/// A closed-loop position controller for one servo actuator. Reads its measured
/// angle from a world-memory feedback entity and steps the gated
/// [`MovementController`] toward the target.
pub struct ClosedLoopServo {
    movement: Arc<MovementController>,
    world: Arc<WorldMemory>,
    feedback_entity: String,
    actuator_name: String,
    channel: i64,
    controller: PController,
}

impl ClosedLoopServo {
    /// Build a closed-loop servo over a gated movement controller. `feedback_entity`
    /// is the world-memory entity carrying the measured angle (e.g.
    /// `"sensor.arm_angle"`).
    pub fn new(
        movement: Arc<MovementController>,
        world: Arc<WorldMemory>,
        feedback_entity: impl Into<String>,
        actuator_name: impl Into<String>,
        channel: i64,
        controller: PController,
    ) -> Self {
        Self {
            movement,
            world,
            feedback_entity: feedback_entity.into(),
            actuator_name: actuator_name.into(),
            channel,
            controller,
        }
    }

    /// Read the current measured angle from the feedback entity, if available.
    pub fn measured(&self) -> anyhow::Result<Option<f64>> {
        Ok(self
            .world
            .current(&self.feedback_entity)?
            .and_then(|f| value_of(&f.value)))
    }

    /// One control step toward `target` degrees. Reads feedback; if not settled,
    /// commands a new absolute angle through the Track 0 gate (which may refuse
    /// it, propagated as an error). Idempotent w.r.t. a settled loop.
    pub async fn step(&self, target: f64, now_ms: u64) -> anyhow::Result<StepOutcome> {
        let Some(measured) = self.measured()? else {
            return Ok(StepOutcome::NoFeedback);
        };
        if self.controller.settled(target, measured) {
            return Ok(StepOutcome::Settled { measured });
        }
        let commanded = measured + self.controller.correction(target, measured);
        let cmd = MovementCommand::ServoAngle {
            name: self.actuator_name.clone(),
            channel: self.channel,
            degrees: commanded,
        };
        self.movement
            .apply(&cmd, now_ms)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(StepOutcome::Adjusted { measured, commanded })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::LoggingActuatorSink;
    use crate::security::limits::{SafetyGate, SafetyLimit};
    use serde_json::json;

    fn pcontroller() -> PController {
        PController { kp: 0.5, tolerance: 1.0, max_step: 30.0 }
    }

    #[test]
    fn correction_is_zero_within_tolerance() {
        let c = pcontroller();
        assert!(c.settled(90.0, 89.5));
        assert_eq!(c.correction(90.0, 89.5), 0.0);
    }

    #[test]
    fn correction_is_clamped_to_max_step() {
        let c = pcontroller();
        // error 100 * 0.5 = 50, clamped to 30
        assert_eq!(c.correction(100.0, 0.0), 30.0);
        // negative error clamps symmetrically
        assert_eq!(c.correction(0.0, 100.0), -30.0);
    }

    fn servo(world: &Arc<WorldMemory>) -> ClosedLoopServo {
        let mut limit = SafetyLimit::new("n1", "servo_angle");
        limit.allowed_pins = Some(vec![0]);
        limit.value_min = Some(0);
        limit.value_max = Some(180);
        let movement = Arc::new(
            MovementController::new("n1", Arc::new(SafetyGate::new(vec![limit])), Arc::new(LoggingActuatorSink))
                .with_world_memory(Arc::clone(world)),
        );
        ClosedLoopServo::new(
            movement,
            Arc::clone(world),
            "sensor.arm_angle",
            "arm",
            0,
            pcontroller(),
        )
    }

    #[tokio::test]
    async fn no_feedback_is_reported() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let s = servo(&world);
        assert_eq!(s.step(90.0, 1_000).await.unwrap(), StepOutcome::NoFeedback);
    }

    #[tokio::test]
    async fn settled_when_already_at_target() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world.observe("sensor.arm_angle", json!({"value": 90.0}), 1, 1, "sim").unwrap();
        let s = servo(&world);
        assert_eq!(s.step(90.0, 1_000).await.unwrap(), StepOutcome::Settled { measured: 90.0 });
    }

    #[tokio::test]
    async fn converges_to_target_over_steps() {
        // Simulated plant: a perfect actuator — after each commanded angle, the
        // feedback entity reports exactly the commanded value next tick.
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world.observe("sensor.arm_angle", json!({"value": 0.0}), 0, 0, "sim").unwrap();
        let s = servo(&world);

        let target = 90.0;
        let mut settled = false;
        let mut t = 1_000u64;
        for _ in 0..25 {
            match s.step(target, t).await.unwrap() {
                StepOutcome::Settled { .. } => {
                    settled = true;
                    break;
                }
                StepOutcome::Adjusted { commanded, .. } => {
                    // plant moves to the commanded angle
                    world
                        .observe("sensor.arm_angle", json!({"value": commanded}), t, t, "sim")
                        .unwrap();
                }
                StepOutcome::NoFeedback => panic!("feedback present"),
            }
            t += 1_000;
        }
        assert!(settled, "loop did not settle within step budget");
        let final_angle = s.measured().unwrap().unwrap();
        assert!((final_angle - target).abs() <= pcontroller().tolerance + 1e-9);

        // The commanded actuator state was recorded in world memory.
        assert!(world.current("actuator.arm").unwrap().is_some());
    }

    #[tokio::test]
    async fn refused_command_propagates_error() {
        // Target beyond the gate's max (180) forces a commanded angle the gate
        // refuses once the loop pushes past the limit.
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world.observe("sensor.arm_angle", json!({"value": 179.0}), 0, 0, "sim").unwrap();
        let s = servo(&world);
        // target 300 → commanded = 179 + clamp(0.5*121,30) = 209 > 180 → gate refuses
        assert!(s.step(300.0, 1_000).await.is_err());
    }
}

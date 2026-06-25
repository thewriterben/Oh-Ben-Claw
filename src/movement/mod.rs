//! Movement subsystem — typed, safety-bounded actuation.
//!
//! The act-side mirror of the vision subsystem, conformant with the Subsystem
//! Suite Contract:
//!
//! * **§7 Stay safe** — every command is bounded by the deterministic Track 0
//!   [`SafetyGate`] (angle/speed range, allowed channels, rate limit) *before*
//!   it reaches hardware. A refused command never actuates.
//! * **§3 Remember** — the commanded state of each actuator is recorded into
//!   bitemporal [`WorldMemory`] as an `actuator.{name}` fact (valid-time,
//!   non-destructive), so the brain knows where every actuator is and the reflex
//!   engine can react to it.
//! * **§1 Perceive/Act** — commands dispatch through a pluggable
//!   [`ActuatorSink`] (dry-run logging today; spine/firmware driver later) so the
//!   same caller works regardless of the physical backend.
//!
//! Where vision *perceives into* memory ([`crate::vision::clawcam_ingest`]),
//! movement *acts and records into* memory — closing the perceive→remember→
//! reflex→act loop on the actuation side.

use crate::memory::world::WorldMemory;
use crate::security::limits::{SafetyGate, SafetyViolation};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// A typed movement command for a named actuator on a hardware channel.
///
/// Wire shape uses an internal `type` tag so commands can arrive from config,
/// tool arguments, or reflex actions interchangeably.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MovementCommand {
    /// Drive a servo to an absolute angle in degrees.
    ServoAngle { name: String, channel: i64, degrees: f64 },
    /// Drive a motor at a signed throttle in `[-1.0, 1.0]`.
    MotorSpeed { name: String, channel: i64, speed: f64 },
    /// Stop an actuator (neutral / zero throttle).
    Stop { name: String, channel: i64 },
}

impl MovementCommand {
    /// Stable actuator id (the world-memory entity key segment).
    pub fn name(&self) -> &str {
        match self {
            MovementCommand::ServoAngle { name, .. }
            | MovementCommand::MotorSpeed { name, .. }
            | MovementCommand::Stop { name, .. } => name,
        }
    }

    /// Hardware channel (treated as the `pin` for the safety gate).
    pub fn channel(&self) -> i64 {
        match self {
            MovementCommand::ServoAngle { channel, .. }
            | MovementCommand::MotorSpeed { channel, .. }
            | MovementCommand::Stop { channel, .. } => *channel,
        }
    }

    /// Safety-gate tool key for this command kind.
    pub fn tool(&self) -> &'static str {
        match self {
            MovementCommand::ServoAngle { .. } => "servo_angle",
            MovementCommand::MotorSpeed { .. } => "motor_speed",
            MovementCommand::Stop { .. } => "stop",
        }
    }

    /// Integer value the deterministic gate bounds: servo → whole degrees,
    /// motor → percent throttle (`speed * 100`, clamped), stop → 0.
    pub fn safety_value(&self) -> i64 {
        match self {
            MovementCommand::ServoAngle { degrees, .. } => degrees.round() as i64,
            MovementCommand::MotorSpeed { speed, .. } => {
                (speed.clamp(-1.0, 1.0) * 100.0).round() as i64
            }
            MovementCommand::Stop { .. } => 0,
        }
    }

    /// The domain value recorded into world memory (degrees or throttle).
    fn domain_value(&self) -> f64 {
        match self {
            MovementCommand::ServoAngle { degrees, .. } => *degrees,
            MovementCommand::MotorSpeed { speed, .. } => speed.clamp(-1.0, 1.0),
            MovementCommand::Stop { .. } => 0.0,
        }
    }
}

/// A movement that passed the gate and (optionally) was recorded + dispatched.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppliedMovement {
    /// Node the actuator lives on (drives spine routing).
    pub node_id: String,
    pub name: String,
    pub tool: String,
    pub channel: i64,
    pub value: f64,
    pub at_ms: u64,
}

/// Why a movement was refused.
#[derive(Debug)]
pub enum MovementError {
    /// The deterministic safety gate refused the command.
    Safety(SafetyViolation),
    /// Recording the commanded state to world memory failed.
    Memory(String),
    /// The actuator sink failed to drive the hardware.
    Dispatch(String),
}

impl std::fmt::Display for MovementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MovementError::Safety(v) => write!(f, "{v}"),
            MovementError::Memory(e) => write!(f, "movement: world-memory error: {e}"),
            MovementError::Dispatch(e) => write!(f, "movement: actuator dispatch error: {e}"),
        }
    }
}

impl std::error::Error for MovementError {}

/// Drives the physical (or simulated) actuator. Pluggable so the same controller
/// works against a dry-run logger, the MQTT spine, or a direct firmware bridge.
#[async_trait]
pub trait ActuatorSink: Send + Sync {
    async fn drive(&self, applied: &AppliedMovement) -> anyhow::Result<()>;
}

/// Default sink: logs the commanded movement without touching hardware. Safe to
/// use until a real driver/spine sink is wired.
pub struct LoggingActuatorSink;

#[async_trait]
impl ActuatorSink for LoggingActuatorSink {
    async fn drive(&self, applied: &AppliedMovement) -> anyhow::Result<()> {
        tracing::info!(
            name = %applied.name,
            tool = %applied.tool,
            channel = applied.channel,
            value = applied.value,
            "movement (dry-run)"
        );
        Ok(())
    }
}

/// Drives a real movement node over the MQTT spine: invokes the node's typed
/// movement tool (`servo_angle` / `motor_speed` / `stop`), which the node bounds
/// again with its own firmware Track 0 limits. Best-effort — spine errors are
/// logged, not propagated, so one unreachable node never stalls the controller.
pub struct SpineActuatorSink {
    spine: Arc<crate::spine::SpineClient>,
}

impl SpineActuatorSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<crate::spine::SpineClient>) -> Self {
        Self { spine }
    }
}

#[async_trait]
impl ActuatorSink for SpineActuatorSink {
    async fn drive(&self, applied: &AppliedMovement) -> anyhow::Result<()> {
        let args = match applied.tool.as_str() {
            "servo_angle" => json!({ "channel": applied.channel, "degrees": applied.value }),
            "motor_speed" => json!({ "channel": applied.channel, "speed": applied.value }),
            _ => json!({ "channel": applied.channel }), // "stop"
        };
        if let Err(e) = self
            .spine
            .invoke_tool(&applied.node_id, &applied.tool, args)
            .await
        {
            tracing::warn!(
                node_id = %applied.node_id,
                tool = %applied.tool,
                error = %e,
                "movement over spine failed"
            );
        }
        Ok(())
    }
}

/// Orchestrates safety-bounded actuation: gate → remember → dispatch.
pub struct MovementController {
    node_id: String,
    gate: Arc<SafetyGate>,
    world: Option<Arc<WorldMemory>>,
    sink: Arc<dyn ActuatorSink>,
    source: String,
}

impl MovementController {
    /// Build a controller for `node_id` with a safety gate and actuator sink.
    pub fn new(node_id: impl Into<String>, gate: Arc<SafetyGate>, sink: Arc<dyn ActuatorSink>) -> Self {
        Self {
            node_id: node_id.into(),
            gate,
            world: None,
            sink,
            source: "movement".to_string(),
        }
    }

    /// Record commanded actuator state into world memory (enables §3 Remember).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Override the world-memory `source` label (default `"movement"`).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Gate-check the command and record the commanded state to world memory
    /// (synchronous; no hardware dispatch). Returns the applied movement, or a
    /// [`MovementError`] — a safety violation here means nothing was recorded or
    /// actuated.
    pub fn plan(&self, cmd: &MovementCommand, now_ms: u64) -> Result<AppliedMovement, MovementError> {
        self.gate
            .check(&self.node_id, cmd.tool(), cmd.channel(), cmd.safety_value(), now_ms)
            .map_err(MovementError::Safety)?;

        let applied = AppliedMovement {
            node_id: self.node_id.clone(),
            name: cmd.name().to_string(),
            tool: cmd.tool().to_string(),
            channel: cmd.channel(),
            value: cmd.domain_value(),
            at_ms: now_ms,
        };

        if let Some(world) = &self.world {
            let entity = format!("actuator.{}", applied.name);
            let value = json!({
                "tool": applied.tool,
                "channel": applied.channel,
                "value": applied.value,
            });
            world
                .observe(&entity, value, now_ms, now_ms, &self.source)
                .map_err(|e| MovementError::Memory(e.to_string()))?;
        }

        Ok(applied)
    }

    /// Full apply: [`plan`](Self::plan) (gate + remember) then dispatch to the
    /// actuator sink. The order guarantees an action is only driven *after* it
    /// passes the gate and is recorded.
    pub async fn apply(&self, cmd: &MovementCommand, now_ms: u64) -> Result<AppliedMovement, MovementError> {
        let applied = self.plan(cmd, now_ms)?;
        self.sink
            .drive(&applied)
            .await
            .map_err(|e| MovementError::Dispatch(e.to_string()))?;
        Ok(applied)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::limits::SafetyLimit;

    fn servo_limit(node: &str, channel: i64, min: i64, max: i64) -> SafetyLimit {
        let mut l = SafetyLimit::new(node, "servo_angle");
        l.allowed_pins = Some(vec![channel]);
        l.value_min = Some(min);
        l.value_max = Some(max);
        l
    }

    fn controller(limits: Vec<SafetyLimit>) -> (MovementController, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = MovementController::new(
            "n1",
            Arc::new(SafetyGate::new(limits)),
            Arc::new(LoggingActuatorSink),
        )
        .with_world_memory(Arc::clone(&world));
        (ctrl, world)
    }

    #[test]
    fn servo_within_bounds_is_recorded() {
        let (ctrl, world) = controller(vec![servo_limit("n1", 0, 0, 180)]);
        let cmd = MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 90.0 };
        let applied = ctrl.plan(&cmd, 1_000).unwrap();
        assert_eq!(applied.value, 90.0);
        let fact = world.current("actuator.arm").unwrap().unwrap();
        assert_eq!(fact.value["tool"], "servo_angle");
        assert!((fact.value["value"].as_f64().unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(fact.source, "movement");
    }

    #[test]
    fn servo_out_of_range_is_refused_and_not_recorded() {
        let (ctrl, world) = controller(vec![servo_limit("n1", 0, 0, 180)]);
        let cmd = MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 250.0 };
        let err = ctrl.plan(&cmd, 1_000).unwrap_err();
        assert!(matches!(err, MovementError::Safety(SafetyViolation::ValueOutOfRange { .. })));
        // Refused → nothing recorded.
        assert!(world.current("actuator.arm").unwrap().is_none());
    }

    #[test]
    fn disallowed_channel_is_refused() {
        let (ctrl, _world) = controller(vec![servo_limit("n1", 0, 0, 180)]);
        let cmd = MovementCommand::ServoAngle { name: "arm".into(), channel: 7, degrees: 45.0 };
        assert!(matches!(
            ctrl.plan(&cmd, 1).unwrap_err(),
            MovementError::Safety(SafetyViolation::PinNotAllowed { .. })
        ));
    }

    #[test]
    fn motor_speed_scales_to_percent_for_bounds() {
        let mut l = SafetyLimit::new("n1", "motor_speed");
        l.allowed_pins = Some(vec![1]);
        l.value_min = Some(-100);
        l.value_max = Some(100);
        let (ctrl, world) = controller(vec![l]);
        // 0.5 throttle → 50 percent, within [-100, 100].
        let cmd = MovementCommand::MotorSpeed { name: "drive".into(), channel: 1, speed: 0.5 };
        let applied = ctrl.plan(&cmd, 1).unwrap();
        assert!((applied.value - 0.5).abs() < 1e-9);
        assert!((world.current("actuator.drive").unwrap().unwrap().value["value"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rate_limit_blocks_rapid_commands() {
        let mut l = servo_limit("n1", 0, 0, 180);
        l.min_interval_ms = Some(500);
        let (ctrl, _world) = controller(vec![l]);
        let cmd = MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 30.0 };
        assert!(ctrl.plan(&cmd, 1_000).is_ok());
        assert!(matches!(
            ctrl.plan(&cmd, 1_200).unwrap_err(),
            MovementError::Safety(SafetyViolation::RateLimited { .. })
        ));
        assert!(ctrl.plan(&cmd, 1_600).is_ok()); // interval elapsed
    }

    #[test]
    fn no_rule_means_allowed_and_recorded() {
        // Gate with no limits → not governed here (left to the approval layer).
        let (ctrl, world) = controller(vec![]);
        let cmd = MovementCommand::Stop { name: "arm".into(), channel: 0 };
        ctrl.plan(&cmd, 1).unwrap();
        assert_eq!(world.current("actuator.arm").unwrap().unwrap().value["value"], 0.0);
    }

    #[test]
    fn successive_commands_update_current_state() {
        let (ctrl, world) = controller(vec![servo_limit("n1", 0, 0, 180)]);
        ctrl.plan(&MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 30.0 }, 1).unwrap();
        ctrl.plan(&MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 120.0 }, 2).unwrap();
        // Current belief is the latest; history retains both.
        let current = world.current("actuator.arm").unwrap().unwrap();
        assert!((current.value["value"].as_f64().unwrap() - 120.0).abs() < 1e-9);
        assert_eq!(world.history("actuator.arm").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn apply_drives_sink_after_gate_and_record() {
        let (ctrl, world) = controller(vec![servo_limit("n1", 0, 0, 180)]);
        let cmd = MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 60.0 };
        let applied = ctrl.apply(&cmd, 5).await.unwrap();
        assert_eq!(applied.tool, "servo_angle");
        assert_eq!(world.current("actuator.arm").unwrap().unwrap().value["value"], 60.0);
    }
}

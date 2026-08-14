//! The actuator sink that drives movement over the spine.
//!
//! This lived in `crate::movement` until 2026-08-08, and it was the entire
//! reason `movement` could not become a crate. `movement` owns the abstraction
//! — the [`ActuatorSink`](obc_movement::ActuatorSink) trait, the Track 0
//! gate, the world-memory record — and it named exactly one concrete backend:
//! `Arc<crate::SpineClient>`, in one field and one constructor
//! parameter, calling one method. Two lines of type, and a 741-line module
//! pinned to a 4611-line one.
//!
//! `scripts/extractability.py` had it as one blocking edge, `spine`, and one
//! free edge. It is now zero, and `navigation` — whose only blocking edge was
//! `movement` — moves with it.
//!
//! The direction is the point. The trait stays where the abstraction is; the
//! implementation lives with the dependency it needs. Same manoeuvre as
//! `obc_a2a::TaskExecutor` (declared in the protocol crate, implemented next to
//! the agent) and as `RiskClass` before obc-safety: an edge is turned around
//! rather than a module being made bigger.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::SpineClient;
use obc_movement::{ActuatorSink, AppliedMovement};

/// Drives a real movement node over the MQTT spine: invokes the node's typed
/// movement tool (`servo_angle` / `motor_speed` / `stop`), which the node bounds
/// again with its own firmware Track 0 limits. Best-effort — spine errors are
/// logged, not propagated, so one unreachable node never stalls the controller.
pub struct SpineActuatorSink {
    spine: Arc<SpineClient>,
}

impl SpineActuatorSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<SpineClient>) -> Self {
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

//! The action sink that routes reflex output over the spine.
//!
//! This lived in the agent's reflex module until 2026-08-12, and it was the only
//! thing keeping that module in this tree. `reflex.rs` names exactly four
//! things outside itself — `obc_movement`, `obc_memory`, `obc_observability`
//! and `crate::spine` — and the first three have been crates for days. All four
//! spine references were in this one struct: one field, one constructor
//! parameter, and two topic constants.
//!
//! Same manoeuvre and the same week: `SpineActuatorSink` left `movement` for
//! [`crate::spine::actuator`] on 2026-08-08 for exactly this reason, and
//! `movement` became a crate the next day with `navigation` behind it. Trait
//! where the abstraction is, implementation where the dependency is.
//!
//! Worth noting what this is *not*. Moving a file and re-exporting it under the
//! old name — the trick that made fourteen extractions call-site-free — does
//! not break an edge; `obc-tool-api` proved that four days ago by changing no
//! measured number at all. This does break one, because the dependency itself
//! moves rather than being renamed.

use std::sync::Arc;

use async_trait::async_trait;
use obc_movement::MovementCommand;
use serde_json::Value;

use obc_reflex::ActionSink;

use crate::spine::{SpineClient, TOPIC_PREFIX};

/// Routes reflex actions over the MQTT spine: a GPIO write becomes a `gpio_write`
/// tool call to the node (bounded there by the firmware Track 0 `SafetyGate`),
/// publishes go to the topic, and escalations are published to `obc/escalation`
/// for System 2 (the gateway/agent) to act on. Best-effort: spine errors are
/// logged, not propagated, so one unreachable node never stalls the reflex loop.
pub struct SpineActionSink {
    spine: Arc<SpineClient>,
}

impl SpineActionSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<SpineClient>) -> Self {
        Self { spine }
    }
}

#[async_trait]
impl ActionSink for SpineActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        let args = serde_json::json!({ "pin": pin, "value": value });
        if let Err(e) = self.spine.invoke_tool(node_id, "gpio_write", args).await {
            tracing::warn!(node_id, pin, value, error = %e, "reflex gpio_write over spine failed");
        }
        Ok(())
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        if let Err(e) = self.spine.publish(topic, payload).await {
            tracing::warn!(topic, error = %e, "reflex publish over spine failed");
        }
        Ok(())
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        let topic = format!("{}/escalation", TOPIC_PREFIX);
        let payload = serde_json::json!({ "reason": reason });
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(error = %e, "reflex escalation publish failed");
        }
        tracing::info!(reason, "reflex: escalated to System 2");
        Ok(())
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        // Publish the typed command to the movement topic; the movement node /
        // controller applies it under its own Track 0 bounds.
        let topic = format!("{}/movement", TOPIC_PREFIX);
        let payload = serde_json::to_value(command).unwrap_or(Value::Null);
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(actuator = command.name(), error = %e, "reflex movement publish failed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpineConfig;
    use serde_json::json;

    #[tokio::test]
    async fn spine_sink_is_best_effort_when_disconnected() {
        let spine = Arc::new(SpineClient::new(SpineConfig::default(), "test"));
        let sink = SpineActionSink::new(spine);
        // An unconnected spine makes the underlying calls fail, but the sink logs
        // and returns Ok so a reflex tick is never broken by a transient outage.
        assert!(sink.gpio_write("node-1", 18, 1).await.is_ok());
        assert!(sink.publish("obc/x", &json!({"a": 1})).await.is_ok());
        assert!(sink.escalate("why").await.is_ok());
    }
}

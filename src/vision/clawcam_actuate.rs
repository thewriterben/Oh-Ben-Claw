//! Close the loop — let OBC *command* ClawCam as an actuator.
//!
//! OBC reads ClawCam (detections, health, audio). This is the *act* half: an
//! [`ActionSink`] that intercepts reflex/foresight `Publish` actions whose topic
//! targets ClawCam (`clawcam/cmd/*`) and translates them into the gateway's gated
//! write tools (`capture_now`, `set_device_state`, `create_alert_rule`) over the
//! existing MCP bridge. Everything else passes through to an inner sink, so this
//! composes with the spine/movement/safing sinks already wired.
//!
//! Because it rides the same MCP bridge, every command still passes ClawCam's own
//! approval model (the scopes/plan-mode vocabulary shared with OBC) — OBC cannot
//! actuate a camera the gateway hasn't authorized.

use crate::agent::reflex::ActionSink;
use crate::mcp::client::McpClient;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Map a `clawcam/cmd/*` publish topic + payload to a ClawCam MCP `(tool, args)`
/// call. Returns `None` for any topic this sink does not own (so it passes through).
///
/// * `clawcam/cmd/capture` → `capture_now { device_id }`
/// * `clawcam/cmd/arm`     → `set_device_state { device_id, state }`
/// * `clawcam/cmd/alert_rule` → `create_alert_rule { …payload }`
pub fn map_command(topic: &str, payload: &Value) -> Option<(String, Value)> {
    let node = payload.get("node").or_else(|| payload.get("device_id")).cloned();
    match topic {
        "clawcam/cmd/capture" => Some(("capture_now".to_string(), json!({ "device_id": node }))),
        "clawcam/cmd/arm" => Some((
            "set_device_state".to_string(),
            json!({ "device_id": node, "state": payload.get("state") }),
        )),
        "clawcam/cmd/alert_rule" => Some(("create_alert_rule".to_string(), payload.clone())),
        _ => None,
    }
}

/// An [`ActionSink`] that forwards `clawcam/cmd/*` publishes to ClawCam write tools
/// over MCP, and delegates everything else to `inner`.
pub struct ClawCamActionSink {
    client: Arc<Mutex<McpClient>>,
    inner: Arc<dyn ActionSink>,
}

impl ClawCamActionSink {
    pub fn new(client: Arc<Mutex<McpClient>>, inner: Arc<dyn ActionSink>) -> Self {
        Self { client, inner }
    }
}

#[async_trait]
impl ActionSink for ClawCamActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        self.inner.gpio_write(node_id, pin, value).await
    }

    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        if let Some((tool, args)) = map_command(topic, payload) {
            let mut guard = self.client.lock().await;
            guard.call_tool(&tool, args).await?;
            Ok(())
        } else {
            self.inner.publish(topic, payload).await
        }
    }

    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        self.inner.escalate(reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_topic_maps_to_capture_now() {
        let (tool, args) = map_command("clawcam/cmd/capture", &json!({ "node": "cam-1" })).unwrap();
        assert_eq!(tool, "capture_now");
        assert_eq!(args["device_id"], "cam-1");
    }

    #[test]
    fn arm_topic_maps_to_set_device_state() {
        let (tool, args) =
            map_command("clawcam/cmd/arm", &json!({ "node": "cam-2", "state": "armed" })).unwrap();
        assert_eq!(tool, "set_device_state");
        assert_eq!(args["device_id"], "cam-2");
        assert_eq!(args["state"], "armed");
    }

    #[test]
    fn alert_rule_topic_passes_payload_through() {
        let payload = json!({ "name": "night-person", "subject": "person" });
        let (tool, args) = map_command("clawcam/cmd/alert_rule", &payload).unwrap();
        assert_eq!(tool, "create_alert_rule");
        assert_eq!(args["name"], "night-person");
    }

    #[test]
    fn unrelated_topic_is_not_owned() {
        assert!(map_command("obc/movement/drive", &json!({})).is_none());
        assert!(map_command("clawcam/telemetry", &json!({})).is_none());
    }

    #[test]
    fn device_id_alias_is_accepted() {
        let (_, args) = map_command("clawcam/cmd/capture", &json!({ "device_id": "cam-9" })).unwrap();
        assert_eq!(args["device_id"], "cam-9");
    }
}

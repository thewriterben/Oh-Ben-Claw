//! Oh-Ben-Claw Communication Bus
//!
//! This module implements the MQTT-based communication backbone that connects
//! the central brain agent with all distributed peripheral nodes.
//!
//! # Topic Hierarchy
//!
//! ```
//! obc/
//! ├── nodes/
//! │   ├── {node_id}/
//! │   │   ├── announce    # Node publishes its capabilities on connect
//! │   │   ├── heartbeat   # Node publishes a heartbeat every N seconds
//! │   │   └── status      # Node publishes its current status
//! ├── tools/
//! │   ├── {node_id}/
//! │   │   ├── call/{tool_name}   # Brain publishes a tool call request
//! │   │   └── result/{call_id}  # Node publishes the tool call result
//! └── broadcast/
//!     └── command    # Brain publishes a command to all nodes
//! ```
//!
//! # Node Announcement
//!
//! When a peripheral node connects to the MQTT broker, it publishes a JSON
//! payload to `obc/nodes/{node_id}/announce` describing its capabilities:
//!
//! ```json
//! {
//!   "node_id": "esp32-s3-kitchen",
//!   "board": "waveshare-esp32-s3-touch-lcd-2.1",
//!   "firmware_version": "0.1.0",
//!   "tools": [
//!     {
//!       "name": "camera_capture",
//!       "description": "Capture a JPEG image from the OV2640 camera module.",
//!       "parameters": { ... }
//!     },
//!     { "name": "audio_sample", ... },
//!     { "name": "sensor_read", ... },
//!     { "name": "gpio_read", ... },
//!     { "name": "gpio_write", ... }
//!   ]
//! }
//! ```
//!
//! The brain agent subscribes to `obc/nodes/+/announce` and dynamically
//! registers the announced tools into its unified tool registry.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The MQTT topic prefix for all Oh-Ben-Claw messages.
pub const TOPIC_PREFIX: &str = "obc";

/// Topic for node announcements.
pub fn topic_announce(node_id: &str) -> String {
    format!("{TOPIC_PREFIX}/nodes/{node_id}/announce")
}

/// Topic for node heartbeats.
pub fn topic_heartbeat(node_id: &str) -> String {
    format!("{TOPIC_PREFIX}/nodes/{node_id}/heartbeat")
}

/// Topic for tool call requests from the brain to a specific node.
pub fn topic_tool_call(node_id: &str, tool_name: &str) -> String {
    format!("{TOPIC_PREFIX}/tools/{node_id}/call/{tool_name}")
}

/// Topic for tool call results from a node back to the brain.
pub fn topic_tool_result(node_id: &str, call_id: &str) -> String {
    format!("{TOPIC_PREFIX}/tools/{node_id}/result/{call_id}")
}

/// Topic for broadcast commands from the brain to all nodes.
pub fn topic_broadcast() -> String {
    format!("{TOPIC_PREFIX}/broadcast/command")
}

// ── Node Announcement ────────────────────────────────────────────────────────

/// A description of a single tool exposed by a peripheral node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeToolSpec {
    /// The name of the tool (e.g., "camera_capture").
    pub name: String,
    /// A human-readable description of what the tool does.
    pub description: String,
    /// The JSON Schema for the tool's parameters.
    pub parameters: Value,
}

/// The announcement payload published by a peripheral node on connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAnnouncement {
    /// A unique identifier for this node (e.g., "esp32-s3-kitchen").
    pub node_id: String,
    /// The board type (e.g., "waveshare-esp32-s3-touch-lcd-2.1").
    pub board: String,
    /// The firmware version of the node.
    pub firmware_version: String,
    /// The list of tools this node exposes.
    pub tools: Vec<NodeToolSpec>,
    /// Optional metadata (e.g., location, description).
    #[serde(default)]
    pub metadata: Value,
}

// ── Tool Call Protocol ───────────────────────────────────────────────────────

/// A tool call request published by the brain to a peripheral node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// A unique identifier for this call, used to correlate the result.
    pub call_id: String,
    /// The name of the tool to invoke.
    pub tool_name: String,
    /// The arguments to pass to the tool.
    pub args: Value,
}

/// A tool call result published by a peripheral node back to the brain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// The call ID from the corresponding `ToolCallRequest`.
    pub call_id: String,
    /// Whether the tool call succeeded.
    pub ok: bool,
    /// The output of the tool call (if successful).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The error message (if the call failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Bus Configuration ────────────────────────────────────────────────────────

/// Configuration for the MQTT communication bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConfig {
    /// The type of bus to use. Currently only "mqtt" is supported.
    #[serde(default = "default_bus_kind")]
    pub kind: String,
    /// The hostname or IP address of the MQTT broker.
    #[serde(default = "default_mqtt_host")]
    pub host: String,
    /// The port of the MQTT broker.
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    /// Optional username for MQTT authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional password for MQTT authentication.
    #[serde(default)]
    pub password: Option<String>,
    /// Whether to use TLS for the MQTT connection.
    #[serde(default)]
    pub tls: bool,
    /// How long to wait for a tool call result before timing out (seconds).
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

fn default_bus_kind() -> String {
    "mqtt".to_string()
}

fn default_mqtt_host() -> String {
    "localhost".to_string()
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_tool_timeout_secs() -> u64 {
    30
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            kind: default_bus_kind(),
            host: default_mqtt_host(),
            port: default_mqtt_port(),
            username: None,
            password: None,
            tls: false,
            tool_timeout_secs: default_tool_timeout_secs(),
        }
    }
}

// ── Bus Client ───────────────────────────────────────────────────────────────

/// A client for the Oh-Ben-Claw MQTT communication bus.
///
/// The `BusClient` is used by the core agent to:
/// 1. Subscribe to node announcements and dynamically register their tools.
/// 2. Publish tool call requests to specific peripheral nodes.
/// 3. Receive tool call results from peripheral nodes.
///
/// It is also used by peripheral node agents to:
/// 1. Publish their capabilities on startup.
/// 2. Subscribe to tool call requests and execute them.
/// 3. Publish tool call results back to the brain.
pub struct BusClient {
    config: BusConfig,
    /// The MQTT client ID for this instance.
    client_id: String,
}

impl BusClient {
    /// Create a new `BusClient` from configuration.
    pub fn new(config: BusConfig, client_id: impl Into<String>) -> Self {
        Self {
            config,
            client_id: client_id.into(),
        }
    }

    /// Connect to the MQTT broker and return the client and event loop.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection to the MQTT broker fails.
    pub async fn connect(&self) -> Result<()> {
        // NOTE: Full MQTT connection logic is implemented in the `mqtt.rs` submodule.
        // This stub exists to define the public API surface.
        tracing::info!(
            host = %self.config.host,
            port = self.config.port,
            client_id = %self.client_id,
            "Connecting to MQTT bus"
        );
        Ok(())
    }

    /// Publish a node announcement to the bus.
    pub async fn announce(&self, announcement: &NodeAnnouncement) -> Result<()> {
        let topic = topic_announce(&announcement.node_id);
        let payload = serde_json::to_string(announcement)?;
        tracing::debug!(topic = %topic, "Publishing node announcement");
        // TODO: Publish via rumqttc client
        let _ = (topic, payload);
        Ok(())
    }

    /// Invoke a tool on a specific peripheral node via the bus.
    ///
    /// This method publishes a `ToolCallRequest` to the node's tool call topic
    /// and waits for a `ToolCallResult` on the corresponding result topic.
    pub async fn invoke_tool(
        &self,
        node_id: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolCallResult> {
        let call_id = uuid::Uuid::new_v4().to_string();
        let request = ToolCallRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.to_string(),
            args,
        };
        let request_topic = topic_tool_call(node_id, tool_name);
        let result_topic = topic_tool_result(node_id, &call_id);
        let payload = serde_json::to_string(&request)?;

        tracing::debug!(
            node_id = %node_id,
            tool_name = %tool_name,
            call_id = %call_id,
            "Invoking tool via MQTT bus"
        );

        // TODO: Publish request and await result via rumqttc client
        let _ = (request_topic, result_topic, payload);

        // Stub: return a placeholder result
        Ok(ToolCallResult {
            call_id,
            ok: true,
            output: Some(format!("Tool '{tool_name}' invoked on node '{node_id}' (stub)")),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_formats_are_correct() {
        assert_eq!(topic_announce("node-1"), "obc/nodes/node-1/announce");
        assert_eq!(topic_heartbeat("node-1"), "obc/nodes/node-1/heartbeat");
        assert_eq!(
            topic_tool_call("node-1", "camera_capture"),
            "obc/tools/node-1/call/camera_capture"
        );
        assert_eq!(
            topic_tool_result("node-1", "call-abc"),
            "obc/tools/node-1/result/call-abc"
        );
        assert_eq!(topic_broadcast(), "obc/broadcast/command");
    }

    #[test]
    fn node_announcement_serializes_correctly() {
        let announcement = NodeAnnouncement {
            node_id: "esp32-s3-kitchen".to_string(),
            board: "waveshare-esp32-s3-touch-lcd-2.1".to_string(),
            firmware_version: "0.1.0".to_string(),
            tools: vec![NodeToolSpec {
                name: "camera_capture".to_string(),
                description: "Capture a JPEG image.".to_string(),
                parameters: serde_json::json!({}),
            }],
            metadata: serde_json::json!({"location": "kitchen"}),
        };
        let json = serde_json::to_string(&announcement).unwrap();
        assert!(json.contains("esp32-s3-kitchen"));
        assert!(json.contains("camera_capture"));
    }

    #[test]
    fn bus_config_defaults_are_sensible() {
        let config = BusConfig::default();
        assert_eq!(config.kind, "mqtt");
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 1883);
        assert!(!config.tls);
        assert_eq!(config.tool_timeout_secs, 30);
    }
}

//! Oh-Ben-Claw Communication Spine
//!
//! This module implements the MQTT-based communication backbone that connects
//! the central brain agent with all distributed peripheral nodes.
//!
//! # Topic Hierarchy
//!
//! ```text
//! obc/
//! +-- nodes/
//! |   +-- {node_id}/
//! |   |   +-- announce    # Node publishes its capabilities on connect
//! |   |   +-- heartbeat   # Node publishes a heartbeat every N seconds
//! |   |   +-- status      # Node publishes its current status
//! +-- tools/
//! |   +-- {node_id}/
//! |   |   +-- call/{tool_name}   # Brain publishes a tool call request
//! |   |   +-- result/{call_id}  # Node publishes the tool call result
//! +-- broadcast/
//!     +-- command    # Brain publishes a command to all nodes
//! ```

pub mod p2p;

use crate::config::SpineConfig; // SpineConfig is defined in config::mod
use crate::tools::traits::{Tool, ToolResult};
use anyhow::{bail, Result};
use async_trait::async_trait;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex, RwLock};

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
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// The announcement payload published by a peripheral node on connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAnnouncement {
    pub node_id: String,
    pub board: String,
    pub firmware_version: String,
    pub tools: Vec<NodeToolSpec>,
    #[serde(default)]
    pub metadata: Value,
}

// ── Tool Call Protocol ───────────────────────────────────────────────────────

/// A tool call request published by the brain to a peripheral node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub args: Value,
}

/// A tool call result published by a peripheral node back to the brain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Pending Call Registry ────────────────────────────────────────────────────

/// A map from call_id to a one-shot sender waiting for the result.
type PendingCalls = Arc<Mutex<HashMap<String, oneshot::Sender<ToolCallResult>>>>;

/// A map from node_id to a list of that node's tool specs.
type NodeRegistry = Arc<RwLock<HashMap<String, NodeAnnouncement>>>;

// ── Spine Client ───────────────────────────────────────────────────────────────

/// A client for the Oh-Ben-Claw MQTT communication spine.
pub struct SpineClient {
    config: SpineConfig,
    client_id: String,
    mqtt_client: Option<AsyncClient>,
    pending_calls: PendingCalls,
    node_registry: NodeRegistry,
}

impl SpineClient {
    /// Create a new `SpineClient` from configuration.
    pub fn new(config: SpineConfig, client_id: impl Into<String>) -> Self {
        Self {
            config,
            client_id: client_id.into(),
            mqtt_client: None,
            pending_calls: Arc::new(Mutex::new(HashMap::new())),
            node_registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to the MQTT broker and spawn the event loop.
    ///
    /// Returns the `SpineClient` and a handle to the background event loop task.
    pub async fn connect(mut self) -> Result<Arc<Self>> {
        let mut opts = MqttOptions::new(&self.client_id, &self.config.host, self.config.port);
        opts.set_keep_alive(Duration::from_secs(30));
        opts.set_clean_session(true);

        if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            opts.set_credentials(user, pass);
        }

        let (client, mut event_loop) = AsyncClient::new(opts, 128);
        self.mqtt_client = Some(client.clone());

        // Subscribe to node announcements and tool results
        client
            .subscribe(format!("{TOPIC_PREFIX}/nodes/+/announce"), QoS::AtLeastOnce)
            .await?;
        client
            .subscribe(format!("{TOPIC_PREFIX}/tools/+/result/+"), QoS::AtLeastOnce)
            .await?;

        let pending_calls = Arc::clone(&self.pending_calls);
        let node_registry = Arc::clone(&self.node_registry);

        // Spawn the event loop handler
        tokio::spawn(async move {
            loop {
                match event_loop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let topic = publish.topic.clone();
                        let payload = publish.payload.clone();

                        if topic.contains("/announce") {
                            // Parse node announcement and register it
                            if let Ok(announcement) =
                                serde_json::from_slice::<NodeAnnouncement>(&payload)
                            {
                                let node_id = announcement.node_id.clone();
                                tracing::info!(
                                    node_id = %node_id,
                                    board = %announcement.board,
                                    tool_count = announcement.tools.len(),
                                    "Node announced on spine"
                                );
                                node_registry.write().await.insert(node_id, announcement);
                            }
                        } else if topic.contains("/result/") {
                            // Parse tool call result and wake the waiting caller
                            if let Ok(result) = serde_json::from_slice::<ToolCallResult>(&payload) {
                                let call_id = result.call_id.clone();
                                if let Some(sender) = pending_calls.lock().await.remove(&call_id) {
                                    let _ = sender.send(result);
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("MQTT event loop error: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });

        tracing::info!(
            host = %self.config.host,
            port = self.config.port,
            client_id = %self.client_id,
            "Connected to MQTT spine"
        );

        Ok(Arc::new(self))
    }

    /// Publish a node announcement to the spine.
    pub async fn announce(&self, announcement: &NodeAnnouncement) -> Result<()> {
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;
        let topic = topic_announce(&announcement.node_id);
        let payload = serde_json::to_vec(announcement)?;
        client
            .publish(topic, QoS::AtLeastOnce, true, payload)
            .await?;
        Ok(())
    }

    /// Invoke a tool on a specific peripheral node via the spine.
    pub async fn invoke_tool(
        &self,
        node_id: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolCallResult> {
        let client = self
            .mqtt_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Spine not connected"))?;

        let call_id = uuid::Uuid::new_v4().to_string();
        let request = ToolCallRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.to_string(),
            args,
        };

        let (tx, rx) = oneshot::channel();
        self.pending_calls.lock().await.insert(call_id.clone(), tx);

        let request_topic = topic_tool_call(node_id, tool_name);
        let payload = serde_json::to_vec(&request)?;
        client
            .publish(request_topic, QoS::AtLeastOnce, false, payload)
            .await?;

        let timeout = Duration::from_secs(self.config.tool_timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => bail!("Tool call channel dropped for call_id={}", call_id),
            Err(_) => {
                self.pending_calls.lock().await.remove(&call_id);
                bail!(
                    "Tool call timed out after {}s (node={}, tool={})",
                    self.config.tool_timeout_secs,
                    node_id,
                    tool_name
                )
            }
        }
    }

    /// Return all currently known peripheral nodes and their tool specs.
    pub async fn known_nodes(&self) -> HashMap<String, NodeAnnouncement> {
        self.node_registry.read().await.clone()
    }

    /// Build a list of `Box<dyn Tool>` from all currently known MQTT nodes.
    pub async fn build_mqtt_tools(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        let registry = self.node_registry.read().await;
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        for (node_id, announcement) in registry.iter() {
            for spec in &announcement.tools {
                tools.push(Box::new(MqttNodeTool {
                    node_id: node_id.clone(),
                    spec: spec.clone(),
                    spine: Arc::clone(self),
                }));
            }
        }
        tools
    }
}

// ── MQTT Node Tool ────────────────────────────────────────────────────────────

/// A tool that delegates execution to a peripheral node via the MQTT spine.
struct MqttNodeTool {
    node_id: String,
    spec: NodeToolSpec,
    spine: Arc<SpineClient>,
}

#[async_trait]
impl Tool for MqttNodeTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let result = self
            .spine
            .invoke_tool(&self.node_id, &self.spec.name, args)
            .await?;

        if result.ok {
            Ok(ToolResult::ok(result.output.unwrap_or_default()))
        } else {
            Ok(ToolResult::err(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
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
    fn spine_config_defaults_are_sensible() {
        let config = SpineConfig::default();
        assert_eq!(config.kind, "mqtt");
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 1883);
        assert!(!config.tls);
        assert_eq!(config.tool_timeout_secs, 30);
    }
}

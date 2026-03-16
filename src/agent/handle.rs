//! `AgentHandle` — a thread-safe, cloneable bridge to the running agent loop.
//!
//! The gateway route handlers and the CLI channel both need to call into the
//! agent from different async tasks. `AgentHandle` wraps `Arc<Agent>` and
//! provides a clean interface for:
//!
//! - Sending a user message and receiving the full `AgentResponse`
//! - Streaming tool-call and tool-result events via a broadcast channel
//! - Querying tool names, node count, and agent status
//!
//! # Design
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                     AgentHandle                         │
//! │                                                         │
//! │  Arc<Agent> ──► process() ──► AgentResponse             │
//! │                    │                                     │
//! │                    └──► broadcast(AgentEvent::*)         │
//! │                                                         │
//! │  Subscribers: GatewayState (SSE), CliChannel (terminal) │
//! └─────────────────────────────────────────────────────────┘
//! ```

use crate::agent::{Agent, AgentResponse};
use crate::config::ProviderConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

/// Capacity of the agent event broadcast channel.
const EVENT_CHANNEL_CAPACITY: usize = 512;

/// Events emitted by the agent during a `process()` call.
///
/// These are broadcast to all subscribers (gateway SSE stream, CLI, etc.)
/// in real time as the agent loop executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent started processing a user message.
    Started {
        session_id: String,
        user_message: String,
    },
    /// The agent is thinking (waiting for LLM response).
    Thinking { session_id: String, iteration: u32 },
    /// The agent dispatched a tool call.
    ToolCall {
        session_id: String,
        call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    /// A tool call completed.
    ToolResult {
        session_id: String,
        call_id: String,
        tool_name: String,
        success: bool,
        output: String,
        duration_ms: u64,
    },
    /// The agent produced a final text response.
    Response {
        session_id: String,
        content: String,
        tool_calls_made: u32,
    },
    /// The agent encountered an error.
    Error { session_id: String, message: String },
}

/// A thread-safe, cloneable handle to the running agent.
///
/// Cheap to clone — all clones share the same underlying `Arc<Agent>`
/// and broadcast sender.
#[derive(Clone)]
pub struct AgentHandle {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    event_tx: broadcast::Sender<AgentEvent>,
    /// Tracks whether the agent is currently processing a request.
    busy: Arc<Mutex<bool>>,
    /// Count of connected peripheral nodes (updated by the spine client).
    node_count: Arc<Mutex<usize>>,
    /// Public tunnel URL (set when a tunnel is active).
    tunnel_url: Arc<Mutex<Option<String>>>,
}

impl AgentHandle {
    /// Create a new `AgentHandle` wrapping the given agent.
    pub fn new(agent: Arc<Agent>, provider_config: ProviderConfig) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            agent,
            provider_config,
            event_tx,
            busy: Arc::new(Mutex::new(false)),
            node_count: Arc::new(Mutex::new(0)),
            tunnel_url: Arc::new(Mutex::new(None)),
        }
    }

    /// Subscribe to agent events.
    ///
    /// Returns a `broadcast::Receiver` that will receive all `AgentEvent`s
    /// emitted during future `process()` calls.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Process a user message and return the full `AgentResponse`.
    ///
    /// Broadcasts `AgentEvent::Started`, `AgentEvent::Thinking`,
    /// `AgentEvent::ToolCall`, `AgentEvent::ToolResult`, and
    /// `AgentEvent::Response` (or `AgentEvent::Error`) during execution.
    pub async fn process(&self, session_id: &str, user_message: &str) -> Result<AgentResponse> {
        // Mark as busy
        {
            let mut busy = self.busy.lock().await;
            *busy = true;
        }

        // Broadcast started event
        let _ = self.event_tx.send(AgentEvent::Started {
            session_id: session_id.to_string(),
            user_message: user_message.to_string(),
        });

        // Run the agent — it handles its own tool-call loop internally.
        // We wrap it to broadcast events around the call.
        let _ = self.event_tx.send(AgentEvent::Thinking {
            session_id: session_id.to_string(),
            iteration: 0,
        });

        let result = self
            .agent
            .process(session_id, user_message, &self.provider_config)
            .await;

        // Mark as not busy
        {
            let mut busy = self.busy.lock().await;
            *busy = false;
        }

        match &result {
            Ok(response) => {
                // Broadcast tool call records
                for (i, record) in response.tool_calls.iter().enumerate() {
                    let args_value: serde_json::Value = serde_json::from_str(&record.args)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    let _ = self.event_tx.send(AgentEvent::ToolCall {
                        session_id: session_id.to_string(),
                        call_id: format!("{}-{}", session_id, i),
                        tool_name: record.name.clone(),
                        args: args_value,
                    });
                    let _ = self.event_tx.send(AgentEvent::ToolResult {
                        session_id: session_id.to_string(),
                        call_id: format!("{}-{}", session_id, i),
                        tool_name: record.name.clone(),
                        success: !record.result.starts_with("Tool error:")
                            && !record.result.starts_with("Tool execution failed:"),
                        output: record.result.clone(),
                        duration_ms: record.duration_ms,
                    });
                }

                // Broadcast final response
                let _ = self.event_tx.send(AgentEvent::Response {
                    session_id: session_id.to_string(),
                    content: response.message.clone(),
                    tool_calls_made: response.tool_calls.len() as u32,
                });
            }
            Err(e) => {
                let _ = self.event_tx.send(AgentEvent::Error {
                    session_id: session_id.to_string(),
                    message: e.to_string(),
                });
            }
        }

        result
    }

    /// Returns true if the agent is currently processing a request.
    pub async fn is_busy(&self) -> bool {
        *self.busy.lock().await
    }

    /// Returns the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.agent
            .tool_names()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Returns the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.agent.tool_count()
    }

    /// Update the connected peripheral node count.
    pub async fn set_node_count(&self, count: usize) {
        *self.node_count.lock().await = count;
    }

    /// Get the current peripheral node count.
    pub async fn node_count(&self) -> usize {
        *self.node_count.lock().await
    }

    /// Set the active tunnel URL.
    pub async fn set_tunnel_url(&self, url: Option<String>) {
        *self.tunnel_url.lock().await = url;
    }

    /// Get the active tunnel URL.
    pub async fn tunnel_url(&self) -> Option<String> {
        self.tunnel_url.lock().await.clone()
    }

    /// Execute a tool directly by name with JSON args.
    ///
    /// Bypasses the agent loop — security policy is still evaluated.
    pub async fn execute_tool_direct(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<crate::tools::traits::ToolResult> {
        let call_id = uuid::Uuid::new_v4().to_string();

        // Broadcast the tool call event
        let _ = self.event_tx.send(AgentEvent::ToolCall {
            session_id: "direct".to_string(),
            call_id: call_id.clone(),
            tool_name: name.to_string(),
            args: args.clone(),
        });

        let start = std::time::Instant::now();
        let result = self.agent.execute_tool_direct(name, args).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Broadcast the result event
        match &result {
            Ok(tr) => {
                let _ = self.event_tx.send(AgentEvent::ToolResult {
                    session_id: "direct".to_string(),
                    call_id,
                    tool_name: name.to_string(),
                    success: tr.success,
                    output: tr.output.clone(),
                    duration_ms,
                });
            }
            Err(e) => {
                let _ = self.event_tx.send(AgentEvent::Error {
                    session_id: "direct".to_string(),
                    message: e.to_string(),
                });
            }
        }

        result
    }
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("tool_count", &self.agent.tool_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_serializes_started() {
        let ev = AgentEvent::Started {
            session_id: "s1".to_string(),
            user_message: "hello".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"started\""));
        assert!(json.contains("\"session_id\":\"s1\""));
    }

    #[test]
    fn agent_event_serializes_tool_call() {
        let ev = AgentEvent::ToolCall {
            session_id: "s1".to_string(),
            call_id: "c1".to_string(),
            tool_name: "shell".to_string(),
            args: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        assert!(json.contains("\"tool_name\":\"shell\""));
    }

    #[test]
    fn agent_event_serializes_response() {
        let ev = AgentEvent::Response {
            session_id: "s1".to_string(),
            content: "Done!".to_string(),
            tool_calls_made: 2,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"response\""));
        assert!(json.contains("\"tool_calls_made\":2"));
    }

    #[test]
    fn agent_event_serializes_error() {
        let ev = AgentEvent::Error {
            session_id: "s1".to_string(),
            message: "LLM timeout".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("LLM timeout"));
    }

    #[test]
    fn agent_event_roundtrips_all_variants() {
        let events = vec![
            AgentEvent::Started {
                session_id: "s1".to_string(),
                user_message: "hi".to_string(),
            },
            AgentEvent::Thinking {
                session_id: "s1".to_string(),
                iteration: 1,
            },
            AgentEvent::ToolCall {
                session_id: "s1".to_string(),
                call_id: "c1".to_string(),
                tool_name: "file_read".to_string(),
                args: serde_json::json!({"path": "/tmp/test.txt"}),
            },
            AgentEvent::ToolResult {
                session_id: "s1".to_string(),
                call_id: "c1".to_string(),
                tool_name: "file_read".to_string(),
                success: true,
                output: "hello world".to_string(),
                duration_ms: 12,
            },
            AgentEvent::Response {
                session_id: "s1".to_string(),
                content: "The file contains: hello world".to_string(),
                tool_calls_made: 1,
            },
            AgentEvent::Error {
                session_id: "s1".to_string(),
                message: "provider timeout".to_string(),
            },
        ];

        for ev in events {
            let json = serde_json::to_string(&ev).unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(v.get("type").is_some(), "Missing 'type' field in {json}");
        }
    }
}

//! Server-Sent Events (SSE) handler for real-time gateway events.
//!
//! Clients connect to `GET /events` and receive a stream of `GatewayEvent`
//! JSON objects as SSE messages. Client-to-server communication uses the
//! standard REST endpoints (`POST /api/v1/chat`, etc.).
//!
//! # Event Types
//!
//! | SSE event name | When emitted |
//! |---|---|
//! | `started` | Agent begins processing a user message |
//! | `thinking` | Agent is waiting for LLM response |
//! | `tool_call` | Agent dispatched a tool call |
//! | `tool_result` | A tool call completed |
//! | `message` | User or assistant chat message |
//! | `node_connected` | A peripheral node connected |
//! | `node_disconnected` | A peripheral node disconnected |
//! | `status` | System status update |
//! | `error` | An error occurred |

use super::{GatewayEvent, GatewayState};
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures_util::stream::Stream;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// `GET /events` — Subscribe to the real-time event stream via SSE.
///
/// The client receives a continuous stream of `GatewayEvent` JSON objects.
/// A keep-alive ping is sent every 15 seconds to prevent proxy timeouts.
///
/// # Example (JavaScript)
///
/// ```text
/// const es = new EventSource('/events');
/// es.addEventListener('message', e => console.log(JSON.parse(e.data)));
/// es.addEventListener('tool_call', e => console.log('Tool:', JSON.parse(e.data)));
/// es.addEventListener('tool_result', e => console.log('Result:', JSON.parse(e.data)));
/// ```
pub async fn sse_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Subscribe to the gateway broadcast channel
    let rx = state.event_tx.subscribe();
    let stream = event_stream(rx);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// Convert the broadcast receiver into an SSE stream.
///
/// Each `GatewayEvent` is serialized to JSON and emitted as an SSE event
/// with a named event type matching the variant name (snake_case).
fn event_stream(
    rx: tokio::sync::broadcast::Receiver<GatewayEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(event) => {
                let json = serde_json::to_string(&event).unwrap_or_default();
                // Use the `type` field from the serialized JSON as the SSE event name
                let event_type = match &event {
                    GatewayEvent::Message { .. } => "message",
                    GatewayEvent::Started { .. } => "started",
                    GatewayEvent::Thinking { .. } => "thinking",
                    GatewayEvent::ToolCall { .. } => "tool_call",
                    GatewayEvent::ToolResult { .. } => "tool_result",
                    GatewayEvent::NodeConnected { .. } => "node_connected",
                    GatewayEvent::NodeDisconnected { .. } => "node_disconnected",
                    GatewayEvent::Status { .. } => "status",
                    GatewayEvent::Error { .. } => "error",
                };
                Some(Ok(Event::default().event(event_type).data(json)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("SSE client lagged by {n} events — some events were dropped");
                // Emit a synthetic error event so the client knows it missed events
                let json = serde_json::json!({
                    "type": "error",
                    "message": format!("SSE client lagged: {n} events dropped")
                })
                .to_string();
                Some(Ok(Event::default().event("error").data(json)))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_event_roundtrips_json() {
        let events = vec![
            GatewayEvent::Status {
                agent_running: true,
                node_count: 2,
                tunnel_url: Some("https://abc.trycloudflare.com".to_string()),
            },
            GatewayEvent::Message {
                session_id: "s1".to_string(),
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            GatewayEvent::Started {
                session_id: "s1".to_string(),
                user_message: "Hello".to_string(),
            },
            GatewayEvent::Thinking {
                session_id: "s1".to_string(),
                iteration: 1,
            },
            GatewayEvent::ToolCall {
                session_id: "s1".to_string(),
                call_id: "c1".to_string(),
                name: "shell".to_string(),
                args: serde_json::json!({"command": "ls"}),
            },
            GatewayEvent::ToolResult {
                session_id: "s1".to_string(),
                call_id: "c1".to_string(),
                name: "shell".to_string(),
                success: true,
                result: "file.txt".to_string(),
            },
            GatewayEvent::NodeConnected {
                node_id: "n1".to_string(),
                board: "esp32-s3".to_string(),
                tools: vec!["gpio_read".to_string()],
            },
            GatewayEvent::NodeDisconnected {
                node_id: "n1".to_string(),
            },
            GatewayEvent::Error {
                message: "something went wrong".to_string(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            assert!(!json.is_empty());
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(v.get("type").is_some(), "Missing 'type' field in: {json}");
        }
    }

    #[test]
    fn event_type_names_are_snake_case() {
        let cases = vec![
            (
                GatewayEvent::Message {
                    session_id: "s".to_string(),
                    role: "user".to_string(),
                    content: "hi".to_string(),
                },
                "message",
            ),
            (
                GatewayEvent::Started {
                    session_id: "s".to_string(),
                    user_message: "hi".to_string(),
                },
                "started",
            ),
            (
                GatewayEvent::Thinking {
                    session_id: "s".to_string(),
                    iteration: 0,
                },
                "thinking",
            ),
            (
                GatewayEvent::Status {
                    agent_running: false,
                    node_count: 0,
                    tunnel_url: None,
                },
                "status",
            ),
            (
                GatewayEvent::Error {
                    message: "err".to_string(),
                },
                "error",
            ),
        ];

        for (event, expected_type) in cases {
            let json = serde_json::to_string(&event).unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                v["type"].as_str().unwrap(),
                expected_type,
                "Wrong type for {json}"
            );
        }
    }
}

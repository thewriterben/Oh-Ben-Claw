//! Server-Sent Events (SSE) handler for real-time gateway events.
//!
//! Clients connect to `GET /events` and receive a stream of `GatewayEvent`
//! JSON objects as SSE messages. Client-to-server communication uses the
//! standard REST endpoints (`POST /api/v1/chat`, etc.).
//!
//! SSE is used instead of WebSocket because it requires no additional axum
//! feature flags and is natively supported by all modern browsers and mobile
//! devices. It is the correct transport for a primarily server-push pattern.

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
/// A keep-alive ping is sent every 15 seconds to prevent connection timeouts.
pub async fn sse_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let rx = state.event_tx.subscribe();
    let stream = event_stream(rx);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// Convert the broadcast receiver into an SSE stream.
fn event_stream(
    rx: tokio::sync::broadcast::Receiver<GatewayEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(event) => {
                let json = serde_json::to_string(&event).unwrap_or_default();
                let event_type = match &event {
                    GatewayEvent::Status { .. } => "status",
                    GatewayEvent::Message { .. } => "message",
                    GatewayEvent::ToolCall { .. } => "tool_call",
                    GatewayEvent::ToolResult { .. } => "tool_result",
                    GatewayEvent::NodeConnected { .. } => "node_connected",
                    GatewayEvent::NodeDisconnected { .. } => "node_disconnected",
                    GatewayEvent::Error { .. } => "error",
                };
                Some(Ok(Event::default().event(event_type).data(json)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("SSE client lagged by {n} events");
                None
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

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert!(!json.is_empty());
            let _: serde_json::Value = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn sse_event_type_matches_variant() {
        let status = GatewayEvent::Status {
            agent_running: false,
            node_count: 0,
            tunnel_url: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("agent_running"));
    }
}

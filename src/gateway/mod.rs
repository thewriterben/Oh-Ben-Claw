//! Oh-Ben-Claw REST & WebSocket API Gateway
//!
//! An Axum-based HTTP server that exposes the agent, peripheral nodes, and
//! tool registry over a clean REST API and a WebSocket stream. This enables
//! mobile apps, web browsers, and third-party integrations to interact with
//! Oh-Ben-Claw over the network.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `GET` | `/` | Serve the PWA web client |
//! | `GET` | `/health` | Health check |
//! | `GET` | `/api/v1/status` | Agent and system status |
//! | `GET` | `/api/v1/sessions` | List conversation sessions |
//! | `POST` | `/api/v1/sessions` | Create a new session |
//! | `GET` | `/api/v1/sessions/{id}/messages` | Get messages for a session |
//! | `POST` | `/api/v1/chat` | Send a message and get a response |
//! | `GET` | `/api/v1/tools` | List all registered tools |
//! | `POST` | `/api/v1/tools/{name}` | Execute a tool directly |
//! | `GET` | `/api/v1/nodes` | List peripheral nodes |
//! | `GET` | `/api/v1/tunnel` | Get tunnel status and URL |
//! | `GET` | `/events` | SSE stream for real-time events |
//!
//! # Authentication
//!
//! If `gateway.api_token` is set in config, all API requests must include:
//! ```text
//! Authorization: Bearer <token>
//! ```
//!
//! # SSE Events
//!
//! The SSE stream at `/events` emits named JSON events:
//! ```json
//! {"type": "message", "role": "assistant", "content": "..."}
//! {"type": "tool_call", "name": "shell", "args": {...}}
//! {"type": "tool_result", "name": "shell", "result": "..."}
//! {"type": "node_connected", "node_id": "esp32-s3-01", "tools": [...]}
//! {"type": "node_disconnected", "node_id": "esp32-s3-01"}
//! {"type": "status", "agent": "running", "nodes": 2}
//! ```

pub mod routes;
pub mod ws;
pub mod pwa;
pub mod middleware;

use crate::config::GatewayConfig;
use anyhow::{Context, Result};
use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Events broadcast to all WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    /// A new assistant message.
    Message {
        session_id: String,
        role: String,
        content: String,
    },
    /// A tool call was dispatched.
    ToolCall {
        session_id: String,
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    /// A tool call result was received.
    ToolResult {
        session_id: String,
        call_id: String,
        name: String,
        success: bool,
        result: String,
    },
    /// A peripheral node connected.
    NodeConnected {
        node_id: String,
        board: String,
        tools: Vec<String>,
    },
    /// A peripheral node disconnected.
    NodeDisconnected { node_id: String },
    /// System status update.
    Status {
        agent_running: bool,
        node_count: usize,
        tunnel_url: Option<String>,
    },
    /// An error occurred.
    Error { message: String },
}

/// Shared state for the gateway.
#[derive(Debug, Clone)]
pub struct GatewayState {
    pub config: GatewayConfig,
    pub event_tx: broadcast::Sender<GatewayEvent>,
}

impl GatewayState {
    pub fn new(config: GatewayConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { config, event_tx }
    }

    /// Broadcast an event to all connected WebSocket clients.
    pub fn broadcast(&self, event: GatewayEvent) {
        // Ignore errors — no subscribers is fine
        let _ = self.event_tx.send(event);
    }
}

/// Build the Axum router with all routes and middleware.
pub fn build_router(state: Arc<GatewayState>) -> Router {
    let api_routes = Router::new()
        .route("/status", get(routes::get_status))
        .route("/sessions", get(routes::list_sessions))
        .route("/sessions", post(routes::create_session))
        .route("/sessions/{id}/messages", get(routes::get_messages))
        .route("/chat", post(routes::chat))
        .route("/tools", get(routes::list_tools))
        .route("/tools/{name}", post(routes::execute_tool))
        .route("/nodes", get(routes::list_nodes))
        .route("/tunnel", get(routes::get_tunnel_status));

    // Apply auth middleware to API routes if a token is configured
    let api_routes = if state.config.api_token.is_some() {
        api_routes.layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::require_auth,
        ))
    } else {
        api_routes
    };

    // Serve the PWA if enabled
    let pwa_routes = if state.config.serve_pwa {
        Router::new()
            .route("/", get(pwa::serve_index))
            .route("/manifest.json", get(pwa::serve_manifest))
            .route("/sw.js", get(pwa::serve_service_worker))
    } else {
        Router::new().route("/", get(routes::get_status))
    };

    // CORS
    let cors = build_cors(&state.config);

    Router::new()
        .nest("/api/v1", api_routes)
        .route("/events", get(ws::sse_handler))
        .merge(pwa_routes)
        .with_state(state.clone())
        .layer(cors)
}

fn build_cors(config: &GatewayConfig) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

    if config.cors_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(AllowOrigin::any())
            .allow_methods(AllowMethods::any())
            .allow_headers(AllowHeaders::any())
    } else {
        let origins: Vec<_> = config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(AllowMethods::any())
            .allow_headers(AllowHeaders::any())
    }
}

/// Start the gateway and return the bound address.
pub async fn start(config: GatewayConfig) -> Result<(Arc<GatewayState>, String)> {
    let bind_addr = format!("{}:{}", config.host, config.port);
    let state = Arc::new(GatewayState::new(config));
    let router = build_router(state.clone());

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind gateway to {bind_addr}"))?;

    let actual_addr = listener.local_addr()?.to_string();
    let url = format!("http://{actual_addr}");

    tracing::info!(addr = %actual_addr, "Gateway listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("Gateway error: {e}");
        }
    });

    Ok((state, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_state_new() {
        let config = GatewayConfig::default();
        let state = GatewayState::new(config.clone());
        assert_eq!(state.config.port, config.port);
    }

    #[test]
    fn gateway_event_serializes_correctly() {
        let event = GatewayEvent::Status {
            agent_running: true,
            node_count: 3,
            tunnel_url: Some("https://abc.trycloudflare.com".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"status\""));
        assert!(json.contains("\"agent_running\":true"));
        assert!(json.contains("\"node_count\":3"));
    }

    #[test]
    fn gateway_event_message_serializes() {
        let event = GatewayEvent::Message {
            session_id: "s1".to_string(),
            role: "assistant".to_string(),
            content: "Hello!".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        assert!(json.contains("\"role\":\"assistant\""));
    }

    #[test]
    fn gateway_event_node_connected_serializes() {
        let event = GatewayEvent::NodeConnected {
            node_id: "esp32-s3-01".to_string(),
            board: "esp32-s3".to_string(),
            tools: vec!["gpio_read".to_string(), "camera_capture".to_string()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"node_connected\""));
        assert!(json.contains("esp32-s3-01"));
    }

    #[test]
    fn build_router_does_not_panic() {
        let config = GatewayConfig::default();
        let state = Arc::new(GatewayState::new(config));
        let _router = build_router(state);
    }

    #[tokio::test]
    async fn gateway_binds_to_random_port() {
        let mut config = GatewayConfig::default();
        config.enabled = true;
        config.port = 0; // OS assigns a free port
        config.host = "127.0.0.1".to_string();
        let result = start(config).await;
        assert!(result.is_ok());
        let (_, url) = result.unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));
    }
}

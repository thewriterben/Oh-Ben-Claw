//! Gateway route handlers.

use super::GatewayState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

// ── Health / Status ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub version: &'static str,
    pub agent_running: bool,
    pub node_count: usize,
    pub tunnel_url: Option<String>,
    pub gateway_url: String,
}

/// `GET /api/v1/status` — System status.
pub async fn get_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        agent_running: false, // TODO: wire to Agent handle
        node_count: 0,        // TODO: wire to SpineClient
        tunnel_url: None,     // TODO: wire to TunnelManager
        gateway_url: format!("http://{}:{}", state.config.host, state.config.port),
    })
}

// ── Sessions ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: String,
    pub message_count: usize,
}

/// `GET /api/v1/sessions` — List conversation sessions.
pub async fn list_sessions(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // TODO: wire to MemoryStore
    Json(json!({ "sessions": Value::Array(vec![]) }))
}

/// `POST /api/v1/sessions` — Create a new session.
pub async fn create_session(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    (
        StatusCode::CREATED,
        Json(json!({ "session_id": id })),
    )
}

/// `GET /api/v1/sessions/{id}/messages` — Get messages for a session.
pub async fn get_messages(
    State(_state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // TODO: wire to MemoryStore
    Json(json!({ "session_id": id, "messages": Value::Array(vec![]) }))
}

// ── Chat ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub message: String,
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub message: String,
    pub tool_calls: Vec<Value>,
}

/// `POST /api/v1/chat` — Send a message and get a response.
pub async fn chat(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let session_id = req
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Broadcast the incoming user message
    state.broadcast(super::GatewayEvent::Message {
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: req.message.clone(),
    });

    // TODO: wire to Agent::run_turn()
    let response = format!(
        "Oh-Ben-Claw received: '{}'. (Agent loop not yet wired to gateway — coming in Phase 7)",
        req.message
    );

    state.broadcast(super::GatewayEvent::Message {
        session_id: session_id.clone(),
        role: "assistant".to_string(),
        content: response.clone(),
    });

    Json(ChatResponse {
        session_id,
        message: response,
        tool_calls: vec![],
    })
}

// ── Tools ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/tools` — List all registered tools.
pub async fn list_tools(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // TODO: wire to Agent tool registry
    Json(json!({ "tools": Value::Array(vec![]) }))
}

/// `POST /api/v1/tools/{name}` — Execute a tool directly.
pub async fn execute_tool(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    let call_id = uuid::Uuid::new_v4().to_string();

    state.broadcast(super::GatewayEvent::ToolCall {
        session_id: "direct".to_string(),
        call_id: call_id.clone(),
        name: name.clone(),
        args: args.clone(),
    });

    // TODO: wire to Agent::execute_tool()
    let result = format!("Tool '{name}' execution not yet wired to gateway.");

    state.broadcast(super::GatewayEvent::ToolResult {
        session_id: "direct".to_string(),
        call_id,
        name: name.clone(),
        success: false,
        result: result.clone(),
    });

    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "tool": name, "result": result })),
    )
}

// ── Nodes ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/nodes` — List connected peripheral nodes.
pub async fn list_nodes(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // TODO: wire to SpineClient node registry
    Json(json!({ "nodes": Value::Array(vec![]) }))
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

/// `GET /api/v1/tunnel` — Get tunnel status and URL.
pub async fn get_tunnel_status(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // TODO: wire to TunnelManager
    Json(json!({
        "status": "stopped",
        "url": Value::Null,
        "backend": "cloudflare"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_deserializes() {
        let json = r#"{"message": "hello", "session_id": "s1"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert_eq!(req.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn chat_response_serializes() {
        let resp = ChatResponse {
            session_id: "s1".to_string(),
            message: "hi".to_string(),
            tool_calls: vec![],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("session_id"));
        assert!(json.contains("tool_calls"));
    }

    #[test]
    fn status_response_serializes() {
        let resp = StatusResponse {
            version: "0.1.0",
            agent_running: true,
            node_count: 3,
            tunnel_url: Some("https://abc.trycloudflare.com".to_string()),
            gateway_url: "http://127.0.0.1:8080".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("agent_running"));
        assert!(json.contains("node_count"));
    }
}

//! Gateway route handlers — all wired to the live `AgentHandle`.

use super::{GatewayEvent, GatewayState};
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
    pub agent_busy: bool,
    pub tool_count: usize,
    pub node_count: usize,
    pub tunnel_url: Option<String>,
    pub gateway_url: String,
}

/// `GET /api/v1/status` — System status.
pub async fn get_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let (agent_running, agent_busy, tool_count, node_count, tunnel_url) =
        if let Some(handle) = &state.agent {
            (
                true,
                handle.is_busy().await,
                handle.tool_count(),
                handle.node_count().await,
                handle.tunnel_url().await,
            )
        } else {
            (false, false, 0, 0, None)
        };

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        agent_running,
        agent_busy,
        tool_count,
        node_count,
        tunnel_url,
        gateway_url: format!("http://{}:{}", state.config.host, state.config.port),
    })
}

// ── Sessions ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub message_count: usize,
}

/// `GET /api/v1/sessions` — List conversation sessions.
pub async fn list_sessions(State(_state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // TODO: wire to MemoryStore::list_sessions() in Phase 8
    Json(json!({ "sessions": Value::Array(vec![]) }))
}

/// `POST /api/v1/sessions` — Create a new session.
pub async fn create_session(State(_state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    (StatusCode::CREATED, Json(json!({ "session_id": id })))
}

/// `GET /api/v1/sessions/{id}/messages` — Get messages for a session.
pub async fn get_messages(
    State(_state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // TODO: wire to MemoryStore::load_messages() in Phase 8
    Json(json!({ "session_id": id, "messages": Value::Array(vec![]) }))
}

// ── Chat ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub message: String,
    pub tool_calls_made: usize,
    pub agent_available: bool,
}

/// `POST /api/v1/chat` — Send a message to the agent and get a response.
///
/// If an `AgentHandle` is attached, this calls `Agent::process()` and
/// broadcasts all intermediate events (tool calls, results) to the SSE stream.
/// If no agent is attached, returns a 503 Service Unavailable.
pub async fn chat(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let session_id = req
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let Some(handle) = &state.agent else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "Agent not available. Start the agent with `oh-ben-claw start --gateway`.",
                "session_id": session_id
            })),
        )
            .into_response();
    };

    // Broadcast user message to SSE subscribers
    state.broadcast(GatewayEvent::Message {
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: req.message.clone(),
    });

    // Subscribe to agent events BEFORE calling process() to avoid missing events
    let mut agent_events = handle.subscribe();
    let event_tx = state.event_tx.clone();
    let sid_clone = session_id.clone();

    // Spawn a task to forward AgentEvents → GatewayEvents on the SSE channel
    let forward_task = tokio::spawn(async move {
        loop {
            match agent_events.recv().await {
                Ok(ev) => {
                    use crate::agent::AgentEvent;
                    let gev = match ev {
                        AgentEvent::Started { session_id, user_message } => {
                            Some(GatewayEvent::Started { session_id, user_message })
                        }
                        AgentEvent::Thinking { session_id, iteration } => {
                            Some(GatewayEvent::Thinking { session_id, iteration })
                        }
                        AgentEvent::ToolCall { session_id, call_id, tool_name, args } => {
                            Some(GatewayEvent::ToolCall {
                                session_id,
                                call_id,
                                name: tool_name,
                                args,
                            })
                        }
                        AgentEvent::ToolResult {
                            session_id,
                            call_id,
                            tool_name,
                            success,
                            output,
                            ..
                        } => Some(GatewayEvent::ToolResult {
                            session_id,
                            call_id,
                            name: tool_name,
                            success,
                            result: output,
                        }),
                        AgentEvent::Response { session_id, content, .. } => {
                            Some(GatewayEvent::Message {
                                session_id,
                                role: "assistant".to_string(),
                                content,
                            })
                        }
                        AgentEvent::Error { session_id, message } => {
                            Some(GatewayEvent::Error {
                                message: format!("[{session_id}] {message}"),
                            })
                        }
                    };
                    if let Some(gev) = gev {
                        let _ = event_tx.send(gev);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        session_id = %sid_clone,
                        lagged = n,
                        "Gateway event forwarder lagged"
                    );
                }
            }
        }
    });

    // Run the agent
    match handle.process(&session_id, &req.message).await {
        Ok(response) => {
            forward_task.abort();

            // Broadcast the final assistant message
            state.broadcast(GatewayEvent::Message {
                session_id: session_id.clone(),
                role: "assistant".to_string(),
                content: response.message.clone(),
            });

            Json(ChatResponse {
                session_id,
                message: response.message,
                tool_calls_made: response.tool_calls.len(),
                agent_available: true,
            })
            .into_response()
        }
        Err(e) => {
            forward_task.abort();
            tracing::error!(error = %e, "Agent process error");
            state.broadcast(GatewayEvent::Error {
                message: e.to_string(),
            });
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "session_id": session_id })),
            )
                .into_response()
        }
    }
}

// ── Tools ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// `GET /api/v1/tools` — List all registered tools.
pub async fn list_tools(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let tools: Vec<Value> = if let Some(handle) = &state.agent {
        handle
            .tool_names()
            .into_iter()
            .map(|name| json!({ "name": name }))
            .collect()
    } else {
        vec![]
    };

    Json(json!({
        "tools": tools,
        "count": tools.len()
    }))
}

/// `POST /api/v1/tools/{name}` — Execute a tool directly via the agent.
pub async fn execute_tool(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    let Some(_handle) = &state.agent else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Agent not available" })),
        )
            .into_response();
    };

    let call_id = uuid::Uuid::new_v4().to_string();

    state.broadcast(GatewayEvent::ToolCall {
        session_id: "direct".to_string(),
        call_id: call_id.clone(),
        name: name.clone(),
        args: args.clone(),
    });

    // TODO: expose Agent::execute_tool() publicly in Phase 8 for direct invocation
    let result = format!(
        "Direct tool execution for '{name}' will be available in Phase 8. \
         Use POST /api/v1/chat to invoke tools via the agent loop."
    );

    state.broadcast(GatewayEvent::ToolResult {
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
        .into_response()
}

// ── Nodes ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/nodes` — List connected peripheral nodes.
pub async fn list_nodes(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let node_count = if let Some(handle) = &state.agent {
        handle.node_count().await
    } else {
        0
    };

    // TODO: expose per-node details from SpineClient in Phase 8
    Json(json!({
        "nodes": Value::Array(vec![]),
        "count": node_count
    }))
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

/// `GET /api/v1/tunnel` — Get tunnel status and URL.
pub async fn get_tunnel_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let tunnel_url = if let Some(handle) = &state.agent {
        handle.tunnel_url().await
    } else {
        None
    };

    let status = if tunnel_url.is_some() { "running" } else { "stopped" };

    Json(json!({
        "status": status,
        "url": tunnel_url,
        "backend": state.config.host
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
    fn chat_request_without_session_id() {
        let json = r#"{"message": "hello"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert!(req.session_id.is_none());
    }

    #[test]
    fn chat_response_serializes() {
        let resp = ChatResponse {
            session_id: "s1".to_string(),
            message: "hi".to_string(),
            tool_calls_made: 2,
            agent_available: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("session_id"));
        assert!(json.contains("tool_calls_made"));
        assert!(json.contains("agent_available"));
    }

    #[test]
    fn status_response_serializes() {
        let resp = StatusResponse {
            version: "0.1.0",
            agent_running: true,
            agent_busy: false,
            tool_count: 12,
            node_count: 3,
            tunnel_url: Some("https://abc.trycloudflare.com".to_string()),
            gateway_url: "http://127.0.0.1:8080".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("agent_running"));
        assert!(json.contains("tool_count"));
        assert!(json.contains("node_count"));
    }

    #[test]
    fn tool_info_serializes() {
        let info = ToolInfo {
            name: "shell".to_string(),
            description: "Execute shell commands".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"name\":\"shell\""));
    }
}

//! Gateway route handlers — all wired to the live `AgentHandle`, `MemoryStore`,
//! `Scheduler`, and `ObsContext`.

use super::{GatewayEvent, GatewayState};
use crate::scheduler::ScheduledTask;
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

// ── Metrics ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/metrics` — Observability metrics snapshot.
pub async fn get_metrics(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let metrics = if let Some(obs) = &state.obs {
        let snap = obs.snapshot();
        json!({
            "requests_total": snap.requests_total,
            "tool_calls_total": snap.tool_calls_total,
            "tool_errors_total": snap.tool_errors_total,
            "agent_turns_total": snap.agent_turns_total,
            "uptime_secs": snap.uptime_secs,
            "active_sessions": snap.active_sessions,
        })
    } else {
        json!({ "error": "Observability not initialized" })
    };

    Json(metrics)
}

// ── Sessions ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /api/v1/sessions` — List conversation sessions.
pub async fn list_sessions(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let sessions: Vec<Value> = if let Some(mem) = &state.memory {
        match mem.list_sessions() {
            Ok(sessions) => sessions
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "title": s.title,
                        "created_at": s.created_at.to_rfc3339(),
                        "updated_at": s.updated_at.to_rfc3339(),
                    })
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to list sessions");
                vec![]
            }
        }
    } else {
        vec![]
    };

    Json(json!({
        "sessions": sessions,
        "count": sessions.len()
    }))
}

/// `POST /api/v1/sessions` — Create a new session.
pub async fn create_session(
    State(state): State<Arc<GatewayState>>,
    body: Option<Json<Value>>,
) -> impl IntoResponse {
    let title = body
        .as_ref()
        .and_then(|b| b.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("New Session")
        .to_string();

    if let Some(mem) = &state.memory {
        match mem.create_session(&title) {
            Ok(id) => (
                StatusCode::CREATED,
                Json(json!({ "session_id": id, "title": title })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        (
            StatusCode::CREATED,
            Json(json!({ "session_id": id, "title": title })),
        )
            .into_response()
    }
}

/// `GET /api/v1/sessions/{id}/messages` — Get messages for a session.
pub async fn get_messages(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let messages: Vec<Value> = if let Some(mem) = &state.memory {
        match mem.load_messages(&id) {
            Ok(msgs) => msgs
                .into_iter()
                .map(|m| {
                    json!({
                        "role": format!("{:?}", m.role).to_lowercase(),
                        "content": m.content,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::error!(session_id = %id, error = %e, "Failed to load messages");
                vec![]
            }
        }
    } else {
        vec![]
    };

    Json(json!({
        "session_id": id,
        "messages": messages,
        "count": messages.len()
    }))
}

/// `DELETE /api/v1/sessions/{id}` — Delete a session and its messages.
pub async fn delete_session(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(mem) = &state.memory {
        match mem.delete_session(&id) {
            Ok(deleted) => {
                if deleted {
                    Json(json!({ "deleted": true, "session_id": id })).into_response()
                } else {
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "error": "Session not found", "session_id": id })),
                    )
                        .into_response()
                }
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Memory store not available" })),
        )
            .into_response()
    }
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

    // Track request in observability
    if let Some(obs) = &state.obs {
        obs.record_request();
    }

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

            // Track turn in observability
            if let Some(obs) = &state.obs {
                obs.record_agent_turn(response.tool_calls.len());
            }

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

    let count = tools.len();
    Json(json!({
        "tools": tools,
        "count": count
    }))
}

/// `POST /api/v1/tools/{name}` — Execute a tool directly by name.
pub async fn execute_tool(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    let Some(handle) = &state.agent else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Agent not available" })),
        )
            .into_response();
    };

    // Track in observability
    if let Some(obs) = &state.obs {
        obs.record_tool_call(&name);
    }

    match handle.execute_tool_direct(&name, args).await {
        Ok(result) => {
            if result.success {
                Json(json!({
                    "tool": name,
                    "success": true,
                    "output": result.output,
                }))
                .into_response()
            } else {
                // Track error in observability
                if let Some(obs) = &state.obs {
                    obs.record_tool_error(&name);
                }
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "tool": name,
                        "success": false,
                        "output": result.output,
                    })),
                )
                    .into_response()
            }
        }
        Err(e) => {
            if let Some(obs) = &state.obs {
                obs.record_tool_error(&name);
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "tool": name, "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ── Nodes ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/nodes` — List connected peripheral nodes.
pub async fn list_nodes(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let node_count = if let Some(handle) = &state.agent {
        handle.node_count().await
    } else {
        0
    };

    Json(json!({
        "nodes": Value::Array(vec![]),
        "count": node_count
    }))
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub value: String,
}

/// `GET /api/v1/scheduler/tasks` — List all scheduled tasks.
pub async fn list_tasks(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let Some(sched) = &state.scheduler else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Scheduler not initialized" })),
        )
            .into_response();
    };

    match sched.list_tasks() {
        Ok(tasks) => {
            let items: Vec<Value> = tasks
                .into_iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "prompt": t.prompt,
                        "session_id": t.session_id,
                        "kind": t.kind.to_storage_string(),
                        "enabled": t.enabled,
                        "last_run": t.last_run,
                        "next_run": t.next_run,
                        "run_count": t.run_count,
                        "created_at": t.created_at,
                    })
                })
                .collect();
            let count = items.len();
            Json(json!({ "tasks": items, "count": count })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/scheduler/tasks` — Create a new scheduled task.
pub async fn create_task(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let Some(sched) = &state.scheduler else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Scheduler not initialized" })),
        )
            .into_response();
    };

    let id = req
        .id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());

    let task = match req.kind.as_str() {
        "cron" => ScheduledTask::cron(
            &id, &req.name, &req.prompt, &session_id, &req.value,
        ),
        "interval" => {
            let secs: u64 = match req.value.parse() {
                Ok(s) => s,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "interval value must be a number of seconds" })),
                    )
                        .into_response();
                }
            };
            ScheduledTask::interval(&id, &req.name, &req.prompt, &session_id, secs)
        }
        "oneshot" => {
            let ts: u64 = match req.value.parse() {
                Ok(t) => t,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "oneshot value must be a Unix timestamp" })),
                    )
                        .into_response();
                }
            };
            ScheduledTask::one_shot(&id, &req.name, &req.prompt, &session_id, ts)
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Unknown task kind: {other}. Use cron, interval, or oneshot.") })),
            )
                .into_response();
        }
    };

    match sched.add_task(task) {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({ "task_id": id, "created": true })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/scheduler/tasks/{id}` — Delete a scheduled task.
pub async fn delete_task(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(sched) = &state.scheduler else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Scheduler not initialized" })),
        )
            .into_response();
    };

    match sched.remove_task(&id) {
        Ok(true) => Json(json!({ "deleted": true, "task_id": id })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Task not found", "task_id": id })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `PATCH /api/v1/scheduler/tasks/{id}` — Enable or disable a task.
pub async fn patch_task(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let Some(sched) = &state.scheduler else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Scheduler not initialized" })),
        )
            .into_response();
    };

    let enabled = match body.get("enabled").and_then(|v| v.as_bool()) {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Body must contain {\"enabled\": true|false}" })),
            )
                .into_response();
        }
    };

    match sched.set_enabled(&id, enabled) {
        Ok(true) => Json(json!({ "task_id": id, "enabled": enabled })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Task not found", "task_id": id })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
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

    #[test]
    fn create_task_request_deserializes() {
        let json = r#"{
            "name": "Daily Briefing",
            "prompt": "Give me a summary.",
            "kind": "cron",
            "value": "0 0 8 * * *"
        }"#;
        let req: CreateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Daily Briefing");
        assert_eq!(req.kind, "cron");
        assert_eq!(req.value, "0 0 8 * * *");
        assert!(req.id.is_none());
        assert!(req.session_id.is_none());
    }
}

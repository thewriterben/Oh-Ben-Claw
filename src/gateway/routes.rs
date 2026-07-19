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
        // Sync the active session count from memory before snapshotting
        if let Some(mem) = &state.memory {
            if let Ok(sessions) = mem.list_sessions() {
                obs.set_active_sessions(sessions.len());
            }
        }
        let snap = obs.snapshot();
        // Every registered counter (Phase 16 self-improvement, rollout
        // simulations, experience retrieval, …), not just the fixed set.
        let counters: serde_json::Map<String, Value> = obs
            .metrics
            .snapshot()
            .into_iter()
            .map(|m| (m.name, json!(m.value)))
            .collect();
        let mut metrics = json!({
            "requests_total": snap.requests_total,
            "tool_calls_total": snap.tool_calls_total,
            "tool_errors_total": snap.tool_errors_total,
            "agent_turns_total": snap.agent_turns_total,
            "uptime_secs": snap.uptime_secs,
            "active_sessions": snap.active_sessions,
            "counters": counters,
        });
        // Phase 15/9: live cost summary when tracking is enabled.
        if let Some(cost) = &state.cost {
            let s = cost.session_summary();
            metrics["cost"] = json!({
                "session_usd": s.session_cost_usd,
                "daily_usd": s.daily_cost_usd,
                "monthly_usd": s.monthly_cost_usd,
                "total_tokens_est": s.total_tokens,
                "requests": s.request_count,
            });
        }
        metrics
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
                        AgentEvent::Started {
                            session_id,
                            user_message,
                        } => Some(GatewayEvent::Started {
                            session_id,
                            user_message,
                        }),
                        AgentEvent::Thinking {
                            session_id,
                            iteration,
                        } => Some(GatewayEvent::Thinking {
                            session_id,
                            iteration,
                        }),
                        AgentEvent::ToolCall {
                            session_id,
                            call_id,
                            tool_name,
                            args,
                        } => Some(GatewayEvent::ToolCall {
                            session_id,
                            call_id,
                            name: tool_name,
                            args,
                        }),
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
                        AgentEvent::Response {
                            session_id,
                            content,
                            ..
                        } => Some(GatewayEvent::Message {
                            session_id,
                            role: "assistant".to_string(),
                            content,
                        }),
                        AgentEvent::Error {
                            session_id,
                            message,
                        } => Some(GatewayEvent::Error {
                            message: format!("[{session_id}] {message}"),
                        }),
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

/// `GET /api/v1/skills` — installed skills with rollout stage + run record.
pub async fn list_skills(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let Some(ops) = &state.skills else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Skill operations not available" })),
        )
            .into_response();
    };
    let forge = crate::skill_forge::SkillForge::new(ops.skill_dir.clone());
    let manifests = match forge.list_manifests() {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let skills: Vec<Value> = manifests
        .into_iter()
        .map(|m| {
            let rec = ops.tracker.record(&m.name).unwrap_or_default();
            let (clean, failures) = if rec.stage == m.stage {
                (rec.clean_runs, rec.failures)
            } else {
                (0, 0)
            };
            json!({
                "name": m.name,
                "description": m.description,
                "stage": m.stage.as_str(),
                "enabled": m.enabled,
                "tags": m.tags,
                "version": m.version,
                "clean_runs": clean,
                "failures": failures,
                "promotion_requires": ops.required_clean,
            })
        })
        .collect();
    let count = skills.len();
    Json(json!({ "skills": skills, "count": count })).into_response()
}

/// Shared body of the promote/demote endpoints.
async fn change_skill_stage(
    state: &GatewayState,
    name: &str,
    promote: bool,
) -> axum::response::Response {
    let Some(ops) = &state.skills else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Skill operations not available" })),
        )
            .into_response();
    };
    let forge = crate::skill_forge::SkillForge::new(ops.skill_dir.clone());
    let outcome = if promote {
        crate::skill_forge::rollout::promote(&forge, &ops.tracker, name, ops.required_clean)
    } else {
        crate::skill_forge::rollout::demote(&forge, &ops.tracker, name)
    };
    match outcome {
        Ok(stage) => {
            // Hot-reload the live agent so the stage change applies immediately.
            if let Some(handle) = &state.agent {
                handle.sync_skills(&forge);
            }
            Json(json!({ "skill": name, "stage": stage.as_str() })).into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({ "skill": name, "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/skills/{name}/promote` — one stage up, gated on the clean-run
/// record (Track 0 staged rollout).
pub async fn promote_skill(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    change_skill_stage(&state, &name, true).await
}

/// `POST /api/v1/skills/{name}/demote` — one stage down (always allowed).
pub async fn demote_skill(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    change_skill_stage(&state, &name, false).await
}

/// Response body for a tool that ran to completion but did not succeed.
///
/// Carries `error` as well as `output`. A refused or failed tool puts its reason in
/// [`crate::tools::traits::ToolResult::error`] and leaves `output` empty, so a body built
/// from `output` alone is `{"output":"","success":false}` — the caller can see *that*
/// something failed but never *why*. On the bench (2026-07-17) that turned a working
/// Track 0 refusal ("safety: pin 99 not in allow-list") into an hour of hunting a
/// delivery problem that did not exist.
fn tool_failure_body(name: &str, result: &crate::tools::traits::ToolResult) -> Value {
    json!({
        "tool": name,
        "success": false,
        "output": result.output,
        "error": result.error,
    })
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
                    Json(tool_failure_body(&name, &result)),
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

// ── Mesh ──────────────────────────────────────────────────────────────────────

/// `GET /api/v1/mesh/status` — Read-only LoRa mesh health snapshot.
///
/// Serves the same payload as the `mesh_status` agent tool (both call
/// [`crate::spine::mesh_supervisor::status_json`]), so a dashboard can poll it directly
/// with a plain GET — no agent attached, no POST body. Returns
/// `{ summary: { nodes, online, degraded, offline, escalated }, nodes: [ … ] }`, or a
/// `503` when world memory is not wired.
pub async fn get_mesh_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    match &state.world {
        Some(world) => Json(crate::spine::mesh_supervisor::status_json(world)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "World memory not available" })),
        )
            .into_response(),
    }
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

    let id = req.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let session_id = req.session_id.unwrap_or_else(|| "default".to_string());

    let task = match req.kind.as_str() {
        "cron" => ScheduledTask::cron(&id, &req.name, &req.prompt, &session_id, &req.value),
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

    let status = if tunnel_url.is_some() {
        "running"
    } else {
        "stopped"
    };

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
    fn a_failed_tool_response_says_why() {
        // The bench case: mesh_command refused by the node's Track 0 allow-list. The
        // reason lives in `error`; `output` is empty. A body built from `output` alone
        // told the operator only that something failed.
        use crate::tools::traits::ToolResult;
        let refused = ToolResult::err("safety: pin 99 not in allow-list");
        let body = tool_failure_body("mesh_command", &refused);

        assert_eq!(body["success"], json!(false));
        assert_eq!(body["tool"], json!("mesh_command"));
        assert_eq!(
            body["error"], json!("safety: pin 99 not in allow-list"),
            "the caller must be able to see why: {body}"
        );
    }

    #[test]
    fn an_approval_refusal_also_says_why() {
        // The other refusal an operator hits: the approval gate, not the node.
        use crate::tools::traits::ToolResult;
        let body = tool_failure_body(
            "mesh_command",
            &ToolResult::err("requires operator approval (autonomy is supervised)"),
        );
        assert!(
            body["error"].as_str().is_some_and(|e| e.contains("approval")),
            "an approval refusal is distinguishable from a node refusal: {body}"
        );
    }

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

// ── Multi-Agent Endpoints ─────────────────────────────────────────────────────

/// `GET /api/v1/agents` — List all sub-agents in the pool.
pub async fn list_agents(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let Some(pool) = &state.agent_pool else {
        return Json(json!({ "agents": [], "count": 0 })).into_response();
    };

    let agents = pool.list();
    let count = agents.len();
    Json(json!({ "agents": agents, "count": count })).into_response()
}

/// Request body for spawning a sub-agent.
#[derive(Debug, Deserialize)]
pub struct SpawnAgentRequest {
    pub name: String,
    pub role: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_iterations: Option<usize>,
}

/// `POST /api/v1/agents` — Spawn a new sub-agent.
pub async fn spawn_agent(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SpawnAgentRequest>,
) -> impl IntoResponse {
    let Some(pool) = &state.agent_pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Multi-agent pool not initialized" })),
        )
            .into_response();
    };

    let mut spec = crate::agent::SubAgentSpec::new(&body.name, &body.role);
    if let Some(prompt) = body.system_prompt {
        spec.system_prompt = prompt;
    }
    if !body.tools.is_empty() {
        spec.tools = body.tools;
    }
    if let Some(max_iter) = body.max_iterations {
        spec.max_iterations = max_iter;
    }

    match pool.spawn(spec) {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({ "name": body.name, "role": body.role, "status": "spawned" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/agents/{name}` — Stop a sub-agent.
pub async fn stop_agent(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(pool) = &state.agent_pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Multi-agent pool not initialized" })),
        )
            .into_response();
    };

    match pool.stop(&name) {
        Ok(()) => Json(json!({ "name": name, "status": "stopped" })).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Request body for delegating a task to a sub-agent.
#[derive(Debug, Deserialize)]
pub struct DelegateRequest {
    pub task: String,
    #[serde(default = "default_delegate_session")]
    pub session_id: String,
}

fn default_delegate_session() -> String {
    "gateway-delegate".to_string()
}

/// `POST /api/v1/agents/{name}/delegate` — Delegate a task to a named sub-agent.
pub async fn delegate_to_agent(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
    Json(body): Json<DelegateRequest>,
) -> impl IntoResponse {
    let Some(pool) = &state.agent_pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Multi-agent pool not initialized" })),
        )
            .into_response();
    };

    if let Some(obs) = &state.obs {
        obs.record_agent_turn(0);
    }

    match pool.delegate(&name, &body.task, &body.session_id).await {
        Ok(response) => Json(json!({
            "agent": name,
            "session_id": body.session_id,
            "response": response
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "agent": name })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/agents/{name}` — Get a single sub-agent's info.
pub async fn get_agent(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(pool) = &state.agent_pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Multi-agent pool not initialized" })),
        )
            .into_response();
    };

    let agents = pool.list();
    if let Some(agent) = agents.iter().find(|a| a.name == name) {
        Json(json!(agent)).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Agent not found", "name": name })),
        )
            .into_response()
    }
}

// ── Hardware registry (live catalog) ──────────────────────────────────────────

/// `GET /api/v1/registry` — The live hardware registry, serialized straight
/// from the running binary's `peripherals::registry` tables. Always current by
/// construction (no staleness possible), same JSON shape as the committed
/// `registry/registry.json` export — consumers (deployment generator,
/// Accelerapp) can refresh their bundled copy from a running fleet.
pub async fn get_registry() -> impl IntoResponse {
    match crate::peripherals::registry::registry_json() {
        Ok(body) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("registry serialization failed: {e}") })),
        )
            .into_response(),
    }
}

// ── Approvals (I4 Operate mode) ────────────────────────────────────────────────

/// `GET /api/v1/approvals` — Approval posture for remote consoles: per-tool
/// ask/approve/deny funnel, active session + forever grants, and the tail of
/// the decision audit log.
pub async fn get_approvals(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let Some(approval) = &state.approval else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Approval manager not attached" })),
        )
            .into_response();
    };

    // Physical-aware surface (Track 0): annotate every tool row with its
    // declared risk class so remote consoles can show what kind of action the
    // operator is granting — not just its name.
    let risk_of = |tool: &str| -> Value {
        let risk = state
            .agent
            .as_ref()
            .map(|h| h.tool_risk(tool))
            .unwrap_or_default();
        json!({
            "physical": risk.physical,
            "reversible": risk.reversible,
            "blast": format!("{:?}", risk.blast).to_lowercase(),
            "requires_per_call": risk.requires_per_call_approval(),
        })
    };

    let funnel: Vec<Value> = approval
        .funnel_summary()
        .into_iter()
        .map(|(tool, c)| {
            let risk = risk_of(&tool);
            json!({
                "tool": tool,
                "asked": c.asked,
                "approved_call": c.approved_call,
                "approved_session": c.approved_session,
                "approved_forever": c.approved_forever,
                "denied": c.denied,
                "plan_violations": c.plan_violations,
                "risk": risk,
            })
        })
        .collect();
    let forever: Vec<Value> = approval
        .forever_grants()
        .list()
        .into_iter()
        .map(|g| {
            let risk = risk_of(&g.tool_name);
            json!({ "tool": g.tool_name, "granted_at": g.granted_at, "risk": risk })
        })
        .collect();
    let audit_tail: Vec<Value> = approval
        .audit_log()
        .into_iter()
        .rev()
        .take(20)
        .map(|e| json!(e))
        .collect();

    Json(json!({
        "funnel": funnel,
        "session_grants": approval.session_grants(),
        "forever_grants": forever,
        "audit_tail": audit_tail,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ApprovalDecisionRequest {
    /// One of: `"call"` (this call class once), `"session"`, `"forever"`,
    /// `"deny"`, `"revoke"` (drop session + forever grants for the tool).
    pub decision: String,
}

/// `POST /api/v1/approvals/{tool}` — Record a remote operator decision for a
/// tool: grant (session/forever), deny, or revoke existing grants. Grants and
/// audit flow through the same `ApprovalManager` the agent consults, so the
/// next tool call sees the new posture immediately.
pub async fn post_approval_decision(
    State(state): State<Arc<GatewayState>>,
    Path(tool): Path<String>,
    Json(body): Json<ApprovalDecisionRequest>,
) -> impl IntoResponse {
    use crate::approval::{ApprovalRequest, ApprovalResponse};

    let Some(approval) = &state.approval else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Approval manager not attached" })),
        )
            .into_response();
    };

    let req = ApprovalRequest {
        tool_name: tool.clone(),
        arguments: json!({ "channel": "gateway-remote" }),
        risk: None,
    };
    match body.decision.as_str() {
        "call" => approval.record_external_decision(&req, ApprovalResponse::Yes),
        "session" => approval.record_external_decision(&req, ApprovalResponse::Always),
        "forever" => approval.record_external_decision(&req, ApprovalResponse::Forever),
        "deny" => approval.record_external_decision(&req, ApprovalResponse::No),
        "revoke" => {
            let session = approval.revoke_session(&tool);
            let forever = approval.forever_grants().revoke(&tool);
            return Json(json!({
                "tool": tool,
                "revoked_session": session,
                "revoked_forever": forever,
            }))
            .into_response();
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "unknown decision '{other}' (expected call|session|forever|deny|revoke)"
                    )
                })),
            )
                .into_response();
        }
    }

    Json(json!({
        "tool": tool,
        "decision": body.decision,
        "session_grants": approval.session_grants(),
    }))
    .into_response()
}

// ── Deployment scheme push (I4 Operate mode) ──────────────────────────────────

/// Maximum accepted scheme size (bytes) — a config TOML, not a firmware image.
const MAX_SCHEME_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub struct PushSchemeRequest {
    /// The scheme's TOML configuration (as produced by the deployment
    /// generator / `DeploymentPlanner`).
    pub toml: String,
    /// Optional label used in the saved filename.
    pub name: Option<String>,
}

/// `POST /api/v1/deployment/scheme` — Receive a generated deployment scheme.
///
/// The TOML is validated for well-formedness and staged to
/// `~/.oh-ben-claw/incoming/` for **operator review** — it is never applied
/// automatically. Returns the staged path.
pub async fn push_scheme(Json(body): Json<PushSchemeRequest>) -> impl IntoResponse {
    if body.toml.len() > MAX_SCHEME_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": format!("scheme exceeds {MAX_SCHEME_BYTES} bytes") })),
        )
            .into_response();
    }
    if let Err(e) = toml::from_str::<toml::Value>(&body.toml) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid TOML: {e}") })),
        )
            .into_response();
    }

    let label: String = body
        .name
        .as_deref()
        .unwrap_or("scheme")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    let dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".oh-ben-claw")
        .join("incoming");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not create incoming dir: {e}") })),
        )
            .into_response();
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{label}-{ts}.toml"));
    match std::fs::write(&path, &body.toml) {
        Ok(()) => Json(json!({
            "staged": path.to_string_lossy(),
            "note": "Staged for operator review — not applied automatically.",
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not write scheme: {e}") })),
        )
            .into_response(),
    }
}

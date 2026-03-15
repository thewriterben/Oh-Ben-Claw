//! Oh-Ben-Claw REST & SSE API Gateway
//!
//! An Axum-based HTTP server that exposes the agent, peripheral nodes, and
//! tool registry over a clean REST API and a Server-Sent Events stream.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `GET` | `/` | Serve the PWA web client |
//! | `GET` | `/health` | Health check |
//! | `GET` | `/api/v1/status` | Agent and system status |
//! | `GET` | `/api/v1/metrics` | Observability metrics snapshot |
//! | `GET` | `/api/v1/sessions` | List conversation sessions |
//! | `POST` | `/api/v1/sessions` | Create a new session |
//! | `GET` | `/api/v1/sessions/{id}/messages` | Get messages for a session |
//! | `DELETE` | `/api/v1/sessions/{id}` | Delete a session |
//! | `POST` | `/api/v1/chat` | Send a message and get a response |
//! | `GET` | `/api/v1/tools` | List all registered tools |
//! | `POST` | `/api/v1/tools/{name}` | Execute a tool directly |
//! | `GET` | `/api/v1/nodes` | List peripheral nodes |
//! | `GET` | `/api/v1/tunnel` | Get tunnel status and URL |
//! | `GET` | `/api/v1/scheduler/tasks` | List scheduled tasks |
//! | `POST` | `/api/v1/scheduler/tasks` | Create a scheduled task |
//! | `DELETE` | `/api/v1/scheduler/tasks/{id}` | Delete a scheduled task |
//! | `PATCH` | `/api/v1/scheduler/tasks/{id}` | Enable/disable a task |
//! | `GET` | `/events` | SSE stream for real-time agent events |

pub mod middleware;
pub mod pwa;
pub mod routes;
pub mod ws;

use crate::agent::AgentHandle;
use crate::config::GatewayConfig;
use crate::memory::MemoryStore;
use crate::observability::ObsContext;
use crate::scheduler::Scheduler;
use anyhow::{Context, Result};
use axum::{
    middleware as axum_middleware,
    routing::{delete, get, patch, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Events broadcast to all SSE subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    /// A new chat message (user or assistant).
    Message {
        session_id: String,
        role: String,
        content: String,
    },
    /// The agent started processing.
    Started {
        session_id: String,
        user_message: String,
    },
    /// The agent is thinking (waiting for LLM).
    Thinking { session_id: String, iteration: u32 },
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
    /// A scheduled task fired.
    ScheduledTask {
        task_id: String,
        task_name: String,
        session_id: String,
    },
    /// An error occurred.
    Error { message: String },
}

/// Shared state for the gateway, accessible from all route handlers.
#[derive(Clone)]
pub struct GatewayState {
    pub config: GatewayConfig,
    pub event_tx: broadcast::Sender<GatewayEvent>,
    /// Live handle to the running agent — `None` until the agent is started.
    pub agent: Option<AgentHandle>,
    /// Conversation memory store — `None` if not initialized.
    pub memory: Option<Arc<MemoryStore>>,
    /// Observability context — `None` if not initialized.
    pub obs: Option<Arc<ObsContext>>,
    /// Task scheduler — `None` if not initialized.
    pub scheduler: Option<Arc<Scheduler>>,
}

impl std::fmt::Debug for GatewayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayState")
            .field("config", &self.config)
            .field("agent_attached", &self.agent.is_some())
            .field("memory_attached", &self.memory.is_some())
            .field("obs_attached", &self.obs.is_some())
            .field("scheduler_attached", &self.scheduler.is_some())
            .finish()
    }
}

impl GatewayState {
    /// Create a new `GatewayState` without any subsystems attached.
    pub fn new(config: GatewayConfig) -> Self {
        let (event_tx, _) = broadcast::channel(512);
        Self {
            config,
            event_tx,
            agent: None,
            memory: None,
            obs: None,
            scheduler: None,
        }
    }

    /// Attach a live `AgentHandle`.
    pub fn with_agent(mut self, handle: AgentHandle) -> Self {
        self.agent = Some(handle);
        self
    }

    /// Attach a `MemoryStore`.
    pub fn with_memory(mut self, memory: Arc<MemoryStore>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Attach an `ObsContext`.
    pub fn with_obs(mut self, obs: Arc<ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    /// Attach a `Scheduler`.
    pub fn with_scheduler(mut self, scheduler: Arc<Scheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Broadcast an event to all connected SSE subscribers.
    pub fn broadcast(&self, event: GatewayEvent) {
        let _ = self.event_tx.send(event);
    }
}

/// Build the Axum router with all routes and middleware.
pub fn build_router(state: Arc<GatewayState>) -> Router {
    let api_routes = Router::new()
        .route("/status", get(routes::get_status))
        .route("/metrics", get(routes::get_metrics))
        .route("/sessions", get(routes::list_sessions))
        .route("/sessions", post(routes::create_session))
        .route("/sessions/{id}/messages", get(routes::get_messages))
        .route("/sessions/{id}", delete(routes::delete_session))
        .route("/chat", post(routes::chat))
        .route("/tools", get(routes::list_tools))
        .route("/tools/{name}", post(routes::execute_tool))
        .route("/nodes", get(routes::list_nodes))
        .route("/tunnel", get(routes::get_tunnel_status))
        .route("/scheduler/tasks", get(routes::list_tasks))
        .route("/scheduler/tasks", post(routes::create_task))
        .route("/scheduler/tasks/{id}", delete(routes::delete_task))
        .route("/scheduler/tasks/{id}", patch(routes::patch_task));

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
        .route("/health", get(health_handler))
        .merge(pwa_routes)
        .with_state(state.clone())
        .layer(cors)
}

async fn health_handler() -> impl axum::response::IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
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

/// Start the gateway and return the shared state and bound URL.
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

/// Start the gateway with all subsystems attached.
pub async fn start_with_agent(
    config: GatewayConfig,
    agent: AgentHandle,
) -> Result<(Arc<GatewayState>, String)> {
    start_full(config, agent, None, None, None).await
}

/// Start the gateway with all subsystems attached.
pub async fn start_full(
    config: GatewayConfig,
    agent: AgentHandle,
    memory: Option<Arc<MemoryStore>>,
    obs: Option<Arc<ObsContext>>,
    scheduler: Option<Arc<Scheduler>>,
) -> Result<(Arc<GatewayState>, String)> {
    let bind_addr = format!("{}:{}", config.host, config.port);

    let mut gs = GatewayState::new(config).with_agent(agent);
    if let Some(m) = memory {
        gs = gs.with_memory(m);
    }
    if let Some(o) = obs {
        gs = gs.with_obs(o);
    }
    if let Some(s) = scheduler {
        gs = gs.with_scheduler(s);
    }

    let state = Arc::new(gs);
    let router = build_router(state.clone());

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind gateway to {bind_addr}"))?;

    let actual_addr = listener.local_addr()?.to_string();
    let url = format!("http://{actual_addr}");

    tracing::info!(addr = %actual_addr, "Gateway listening (full subsystems attached)");

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
        assert!(state.agent.is_none());
        assert!(state.memory.is_none());
        assert!(state.obs.is_none());
        assert!(state.scheduler.is_none());
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
    fn gateway_event_scheduled_task_serializes() {
        let event = GatewayEvent::ScheduledTask {
            task_id: "daily-briefing".to_string(),
            task_name: "Daily Briefing".to_string(),
            session_id: "default".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"scheduled_task\""));
        assert!(json.contains("daily-briefing"));
    }

    #[test]
    fn gateway_state_with_builder_methods() {
        let config = GatewayConfig::default();
        let state = GatewayState::new(config);
        // Verify builder methods exist and work
        assert!(state.memory.is_none());
        assert!(state.obs.is_none());
        assert!(state.scheduler.is_none());
    }
}

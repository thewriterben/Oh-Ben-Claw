//! Feishu/Lark channel adapter — webhook event subscription.
//!
//! Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s Feishu
//! integration, which brings Oh-Ben-Claw to the popular Feishu/Lark enterprise
//! messaging platform.
//!
//! # Architecture
//!
//! ```text
//! Feishu Server → POST /feishu/webhook → handle_event() → Agent → Feishu REST API
//! ```
//!
//! # Setup
//!
//! 1. Go to [Feishu Open Platform](https://open.feishu.cn/) and create an app.
//! 2. Copy the **App ID** and **App Secret**.
//! 3. Enable the following permissions in the app:
//!    - `im:message` — Send and receive messages
//!    - `im:message:send_as_bot` — Send messages as a bot
//! 4. Under **Event Subscription**, configure the webhook URL to point to
//!    `https://<your-host>/feishu/webhook` and subscribe to `im.message.receive_v1`.
//! 5. Copy the **Verification Token** and **Encrypt Key** shown in the dashboard.
//! 6. Set the following in `config.toml`:
//!
//! ```toml
//! [channels.feishu]
//! app_id            = "cli_xxxxxxxxxxxxxx"
//! app_secret        = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
//! verification_token = "your-verification-token"   # optional but recommended
//! webhook_port      = 18790
//! ```
//!
//! # Message flow
//!
//! 1. Feishu sends a `POST /feishu/webhook` with the event payload.
//! 2. If a `verification_token` is configured, the signature is checked.
//! 3. The text content is extracted and forwarded to the Oh-Ben-Claw agent.
//! 4. The agent's reply is sent back via `POST /im/v1/messages`.

use crate::agent::Agent;
use crate::channels::utils::chunk_text;
use crate::config::{FeishuConfig, ProviderConfig};
use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";

// ── Feishu API types ──────────────────────────────────────────────────────────

/// Incoming webhook event envelope.
#[derive(Debug, Deserialize)]
struct FeishuEvent {
    /// Challenge used for webhook URL verification.
    challenge: Option<String>,
    /// Token for signature verification.
    token: Option<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
    event: Option<FeishuMessageEvent>,
}

/// Inner message event payload for `im.message.receive_v1`.
#[derive(Debug, Deserialize)]
struct FeishuMessageEvent {
    sender: Option<FeishuSender>,
    message: Option<FeishuMessage>,
}

#[derive(Debug, Deserialize)]
struct FeishuSender {
    sender_id: Option<FeishuUserId>,
}

#[derive(Debug, Deserialize)]
struct FeishuUserId {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuMessage {
    message_id: Option<String>,
    chat_id: Option<String>,
    content: Option<String>,
    message_type: Option<String>,
}

/// Parsed text body inside `FeishuMessage.content` (JSON-encoded string).
#[derive(Debug, Deserialize)]
struct FeishuTextContent {
    text: Option<String>,
}

/// Tenant access token response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    code: i32,
    tenant_access_token: Option<String>,
    expire: Option<u64>,
}

/// Feishu send-message request body.
#[derive(Debug, Serialize)]
struct SendMessageBody<'a> {
    receive_id: &'a str,
    msg_type: &'a str,
    content: String,
}

/// Challenge response for URL verification.
#[derive(Debug, Serialize)]
struct ChallengeResponse {
    challenge: String,
}

// ── Cached access token ───────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct TokenCache {
    token: String,
    /// `None` means the cache is empty / expired.
    expires_at: Option<std::time::Instant>,
}

impl TokenCache {
    fn is_expired(&self) -> bool {
        match self.expires_at {
            None => true,
            Some(at) => at <= std::time::Instant::now(),
        }
    }
}

// ── Shared channel state ──────────────────────────────────────────────────────

#[derive(Clone)]
struct FeishuState {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    app_id: String,
    app_secret: String,
    verification_token: Option<String>,
    http: reqwest::Client,
    token_cache: Arc<RwLock<TokenCache>>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// Feishu/Lark channel adapter.
pub struct FeishuChannel {
    state: FeishuState,
    webhook_port: u16,
}

impl FeishuChannel {
    /// Create a new `FeishuChannel`.
    ///
    /// Returns `None` when `app_id` or `app_secret` are not configured.
    pub fn new(
        config: &FeishuConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        let app_id = config
            .app_id
            .clone()
            .or_else(|| std::env::var("FEISHU_APP_ID").ok())?;
        let app_secret = config
            .app_secret
            .clone()
            .or_else(|| std::env::var("FEISHU_APP_SECRET").ok())?;

        Some(Self {
            state: FeishuState {
                agent,
                provider_config,
                app_id,
                app_secret,
                verification_token: config
                    .verification_token
                    .clone()
                    .or_else(|| std::env::var("FEISHU_VERIFICATION_TOKEN").ok()),
                http: reqwest::Client::new(),
                token_cache: Arc::new(RwLock::new(TokenCache::default())),
            },
            webhook_port: config.webhook_port.unwrap_or(18790),
        })
    }

    /// Start the Axum webhook server.
    ///
    /// Listens on `0.0.0.0:{webhook_port}` until the task is cancelled.
    /// Feishu must be configured to POST events to
    /// `https://<your-host>/feishu/webhook`.
    pub async fn run(&self) -> Result<()> {
        let bind_addr = format!("0.0.0.0:{}", self.webhook_port);
        tracing::info!(port = self.webhook_port, "Feishu webhook server starting");

        let state = self.state.clone();
        let router = Router::new()
            .route("/feishu/webhook", post(receive_event))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .context("Feishu webhook bind failed")?;

        tracing::info!(addr = %bind_addr, "Feishu webhook server listening");
        axum::serve(listener, router)
            .await
            .context("Feishu webhook server error")?;

        Ok(())
    }
}

// ── Axum handler ──────────────────────────────────────────────────────────────

/// POST /feishu/webhook — receive Feishu events.
async fn receive_event(
    State(state): State<FeishuState>,
    Json(payload): Json<FeishuEvent>,
) -> impl IntoResponse {
    // 1. URL verification handshake
    if let Some(challenge) = &payload.challenge {
        tracing::debug!("Feishu webhook URL verification");
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "challenge": challenge })),
        );
    }

    // 2. Verify token if configured
    if let Some(expected) = &state.verification_token {
        if payload.token.as_deref() != Some(expected.as_str()) {
            tracing::warn!("Feishu webhook: invalid verification token");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid token" })),
            );
        }
    }

    // 3. Only handle im.message.receive_v1 events
    if payload.event_type.as_deref() != Some("im.message.receive_v1") {
        return (StatusCode::OK, Json(serde_json::json!({})));
    }

    // 4. Extract message content
    let Some(event) = payload.event else {
        return (StatusCode::OK, Json(serde_json::json!({})));
    };
    let Some(message) = event.message else {
        return (StatusCode::OK, Json(serde_json::json!({})));
    };

    // Only process text messages
    if message.message_type.as_deref() != Some("text") {
        return (StatusCode::OK, Json(serde_json::json!({})));
    }

    let chat_id = match message.chat_id {
        Some(id) => id,
        None => return (StatusCode::OK, Json(serde_json::json!({}))),
    };

    let text = match message
        .content
        .as_deref()
        .and_then(|c| serde_json::from_str::<FeishuTextContent>(c).ok())
        .and_then(|t| t.text)
    {
        Some(t) => t.trim().to_string(),
        None => return (StatusCode::OK, Json(serde_json::json!({}))),
    };

    if text.is_empty() {
        return (StatusCode::OK, Json(serde_json::json!({})));
    }

    // 5. Process with the agent (spawn so we can return 200 immediately)
    let state_clone = state.clone();
    let chat_id_clone = chat_id.clone();
    tokio::spawn(async move {
        match state_clone
            .agent
            .process(&chat_id_clone, &text, &state_clone.provider_config)
            .await
        {
            Ok(response) => {
                for chunk in chunk_text(&response.message, 4000) {
                    if let Err(e) = send_message(&state_clone, &chat_id_clone, chunk).await {
                        tracing::error!("Feishu send error: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::error!("Feishu agent error: {e}");
                let _ = send_message(
                    &state_clone,
                    &chat_id_clone,
                    "Sorry, I encountered an error.",
                )
                .await;
            }
        }
    });

    (StatusCode::OK, Json(serde_json::json!({})))
}

// ── Feishu REST helpers ───────────────────────────────────────────────────────

/// Fetch (or return cached) tenant access token.
async fn get_token(state: &FeishuState) -> Result<String> {
    {
        let cache = state.token_cache.read().await;
        if !cache.is_expired() && !cache.token.is_empty() {
            return Ok(cache.token.clone());
        }
    }

    // Refresh
    let body = serde_json::json!({
        "app_id": state.app_id,
        "app_secret": state.app_secret,
    });

    let resp: TokenResponse = state
        .http
        .post(format!(
            "{FEISHU_API_BASE}/auth/v3/tenant_access_token/internal"
        ))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if resp.code != 0 {
        anyhow::bail!("Feishu token refresh failed (code {})", resp.code);
    }

    let token = resp
        .tenant_access_token
        .ok_or_else(|| anyhow::anyhow!("Missing tenant_access_token in response"))?;

    let expires_secs = resp.expire.unwrap_or(7200).saturating_sub(60);
    {
        let mut cache = state.token_cache.write().await;
        cache.token = token.clone();
        cache.expires_at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(expires_secs));
    }

    Ok(token)
}

/// Send a text message to a Feishu chat.
async fn send_message(state: &FeishuState, chat_id: &str, text: &str) -> Result<()> {
    let token = get_token(state).await?;

    let content = serde_json::json!({ "text": text }).to_string();
    let body = SendMessageBody {
        receive_id: chat_id,
        msg_type: "text",
        content,
    };

    let resp = state
        .http
        .post(format!("{FEISHU_API_BASE}/im/v1/messages"))
        .query(&[("receive_id_type", "chat_id")])
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .context("Feishu send_message HTTP error")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Feishu API error {status}: {body}");
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_text_content_parses() {
        let raw = r#"{"text":"hello world"}"#;
        let parsed: FeishuTextContent = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.text.unwrap(), "hello world");
    }

    #[test]
    fn feishu_event_challenge_deserializes() {
        let raw = r#"{"challenge":"abc123","token":"tok","type":"url_verification"}"#;
        let ev: FeishuEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(ev.challenge.unwrap(), "abc123");
    }

    #[test]
    fn token_cache_expired_when_default() {
        let cache = TokenCache::default();
        // Default has expires_at = None which means expired
        assert!(cache.is_expired());
    }
}

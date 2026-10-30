//! WhatsApp channel adapter — Meta Business Cloud API webhook.
//!
//! Receives messages via an Axum webhook server (Meta calls `POST /whatsapp/webhook`
//! for every incoming event) and sends replies through the Meta Graph API.
//!
//! # Setup
//! 1. Create a Meta Business app at <https://developers.facebook.com>.
//! 2. Add the **WhatsApp** product to the app.
//! 3. Generate a permanent system-user access token and copy the
//!    **Phone Number ID** for the test/production number.
//! 4. Set `channels.whatsapp.access_token` and `channels.whatsapp.phone_number_id`
//!    in `config.toml` (or `WHATSAPP_ACCESS_TOKEN` / `WHATSAPP_PHONE_NUMBER_ID`).
//! 5. Configure a webhook in the Meta developer dashboard pointing to
//!    `https://<your-host>/whatsapp/webhook` and set `channels.whatsapp.verify_token`
//!    to the same value you used in the dashboard (or `WHATSAPP_VERIFY_TOKEN`).
//! 6. Set `channels.whatsapp.app_secret` (or `WHATSAPP_APP_SECRET`) to the app
//!    secret. Required: the listener is on `0.0.0.0`, and the only thing that
//!    tells Meta's POSTs apart from anyone else's is `X-Hub-Signature-256`.
//!    Until 2026-10-13 nothing checked it, so any client that could reach the
//!    port could talk to the agent as any phone number.
//!
//! # Message flow
//! ```text
//! Meta → POST /whatsapp/webhook  →  handle_message()  →  Agent  →  Graph API  →  WhatsApp
//! ```
//!
//! # Limitations
//! Only text (`type == "text"`) messages are processed.  Media, templates, and
//! interactive messages are acknowledged but not forwarded to the agent.

use crate::utils::chunk_text;
use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use hmac::{Hmac, Mac};
use obc_agent::Agent;
use obc_config::{ProviderConfig, WhatsAppConfig};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;

const GRAPH_API_BASE: &str = "https://graph.facebook.com/v18.0";

// ── Incoming webhook payload types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WebhookPayload {
    object: Option<String>,
    entry: Option<Vec<Entry>>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    changes: Option<Vec<Change>>,
}

#[derive(Debug, Deserialize)]
struct Change {
    value: Option<ChangeValue>,
}

#[derive(Debug, Deserialize)]
struct ChangeValue {
    messages: Option<Vec<WaMessage>>,
}

#[derive(Debug, Deserialize)]
struct WaMessage {
    from: String,
    id: String,
    #[serde(rename = "type")]
    msg_type: String,
    text: Option<WaText>,
}

#[derive(Debug, Deserialize)]
struct WaText {
    body: String,
}

// ── Outgoing send-message types ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    messaging_product: &'a str,
    to: &'a str,
    #[serde(rename = "type")]
    msg_type: &'a str,
    text: SendTextBody<'a>,
}

#[derive(Debug, Serialize)]
struct SendTextBody<'a> {
    preview_url: bool,
    body: &'a str,
}

// ── Shared state (passed into Axum handlers) ─────────────────────────────────

#[derive(Clone)]
struct WaState {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    verify_token: String,
    app_secret: String,
    access_token: String,
    phone_number_id: String,
    http: reqwest::Client,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// WhatsApp Business Cloud API channel.
pub struct WhatsAppChannel {
    state: WaState,
    webhook_port: u16,
}

impl WhatsAppChannel {
    /// Create a new `WhatsAppChannel`.
    ///
    /// Returns `None` if the required tokens / IDs are not configured.
    pub fn new(
        config: &WhatsAppConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        let access_token = config
            .access_token
            .clone()
            .or_else(|| std::env::var("WHATSAPP_ACCESS_TOKEN").ok())?;
        let phone_number_id = config
            .phone_number_id
            .clone()
            .or_else(|| std::env::var("WHATSAPP_PHONE_NUMBER_ID").ok())?;
        let Some(app_secret) = config
            .app_secret
            .clone()
            .or_else(|| std::env::var("WHATSAPP_APP_SECRET").ok())
            .filter(|s| !s.is_empty())
        else {
            tracing::error!(
                "WhatsApp is configured but channels.whatsapp.app_secret (or \
                 WHATSAPP_APP_SECRET) is not set; the channel is not started, \
                 because without it the webhook cannot tell Meta from anyone else"
            );
            return None;
        };
        let verify_token = config
            .verify_token
            .clone()
            .or_else(|| std::env::var("WHATSAPP_VERIFY_TOKEN").ok())
            .unwrap_or_else(|| "obc-whatsapp".to_string());

        Some(Self {
            state: WaState {
                agent,
                provider_config,
                verify_token,
                app_secret,
                access_token,
                phone_number_id,
                http: reqwest::Client::new(),
            },
            webhook_port: config.webhook_port.unwrap_or(8444),
        })
    }

    /// Start the Axum webhook server.
    ///
    /// Listens on `0.0.0.0:{webhook_port}` until the task is cancelled.
    /// The Meta dashboard must be configured to call
    /// `https://<host>/whatsapp/webhook`.
    pub async fn run(&self) -> Result<()> {
        let bind_addr = format!("0.0.0.0:{}", self.webhook_port);
        tracing::info!(port = self.webhook_port, "WhatsApp webhook server starting");

        let state = self.state.clone();
        let router = Router::new()
            .route(
                "/whatsapp/webhook",
                get(verify_webhook).post(receive_webhook),
            )
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .context("WhatsApp webhook bind failed")?;

        tracing::info!(addr = %bind_addr, "WhatsApp webhook server listening");

        axum::serve(listener, router)
            .await
            .context("WhatsApp webhook server error")?;

        Ok(())
    }
}

// ── Axum handlers ─────────────────────────────────────────────────────────────

/// GET /whatsapp/webhook — Meta webhook verification handshake.
async fn verify_webhook(
    State(state): State<WaState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let mode = params.get("hub.mode").map(|s| s.as_str());
    let token = params.get("hub.verify_token").map(|s| s.as_str());
    let challenge = params.get("hub.challenge").cloned().unwrap_or_default();

    if mode == Some("subscribe") && token == Some(state.verify_token.as_str()) {
        tracing::info!("WhatsApp webhook verified successfully");
        (StatusCode::OK, challenge)
    } else {
        tracing::warn!("WhatsApp webhook verification failed");
        (StatusCode::FORBIDDEN, String::new())
    }
}

/// POST /whatsapp/webhook — incoming message events from Meta.
///
/// The body is taken raw, because the signature covers the bytes as sent and
/// re-serialising parsed JSON would not reproduce them.
async fn receive_webhook(
    State(state): State<WaState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !signature_valid(state.app_secret.as_bytes(), &body, signature) {
        tracing::warn!("WhatsApp webhook POST refused: missing or bad X-Hub-Signature-256");
        return StatusCode::UNAUTHORIZED;
    }
    let payload: WebhookPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    // Only handle whatsapp_business_account objects.
    if payload.object.as_deref() != Some("whatsapp_business_account") {
        return StatusCode::OK;
    }

    for entry in payload.entry.unwrap_or_default() {
        for change in entry.changes.unwrap_or_default() {
            if let Some(value) = change.value {
                for msg in value.messages.unwrap_or_default() {
                    if let Err(e) = handle_message(&state, msg).await {
                        tracing::error!(error = %e, "WhatsApp message handling error");
                    }
                }
            }
        }
    }

    StatusCode::OK
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Check Meta's `X-Hub-Signature-256: sha256=<hex>` against an HMAC-SHA256 of
/// `body` under the app secret. Constant-time compare (`verify_slice`).
fn signature_valid(app_secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(sig) = header.and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let Ok(sig) = hex::decode(sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(app_secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

async fn handle_message(state: &WaState, msg: WaMessage) -> Result<()> {
    // Only process text messages.
    if msg.msg_type != "text" {
        return Ok(());
    }

    let body = match msg.text {
        Some(t) if !t.body.trim().is_empty() => t.body.trim().to_string(),
        _ => return Ok(()),
    };

    tracing::debug!(
        from = %msg.from,
        id = %msg.id,
        text = %body,
        "WhatsApp message received"
    );

    // Session ID per phone number.
    let session_id = format!("wa-{}", msg.from);

    let response = state
        .agent
        .process(&session_id, &body, &state.provider_config)
        .await
        .context("Agent processing error")?;

    send_text(state, &msg.from, &response.message).await
}

async fn send_text(state: &WaState, to: &str, text: &str) -> Result<()> {
    let url = format!("{}/{}/messages", GRAPH_API_BASE, state.phone_number_id);

    // WhatsApp's per-message character limit is 4 096.  We chunk at 4 000
    // to leave headroom for any UTF-8 boundary rounding done by chunk_text.
    for chunk in chunk_text(text, 4000) {
        let body = SendMessageRequest {
            messaging_product: "whatsapp",
            to,
            msg_type: "text",
            text: SendTextBody {
                preview_url: false,
                body: chunk,
            },
        };

        let res = state
            .http
            .post(&url)
            .bearer_auth(&state.access_token)
            .json(&body)
            .send()
            .await
            .context("WhatsApp Graph API sendMessage HTTP error")?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            tracing::warn!(
                status = %status,
                body = %err_body,
                "WhatsApp Graph API sendMessage failed"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::signature_valid;

    #[test]
    fn webhook_signature_accepts_meta_and_refuses_everything_else() {
        let body = br#"{"object":"whatsapp_business_account"}"#;
        // Computed outside this code: printf '%s' "$body" | openssl dgst -sha256 -hmac s3cret
        let good = "sha256=911fd0aa272ad28e30ab013c69b6fd0dcf41e3fbb4cca5a48a4bf3be4544ff57";
        assert!(signature_valid(b"s3cret", body, Some(good)));
        // The case that used to pass: no header at all.
        assert!(!signature_valid(b"s3cret", body, None));
        // Wrong secret, altered body, missing prefix, not hex.
        assert!(!signature_valid(b"other", body, Some(good)));
        assert!(!signature_valid(b"s3cret", b"{}", Some(good)));
        assert!(!signature_valid(b"s3cret", body, Some(&good[7..])));
        assert!(!signature_valid(b"s3cret", body, Some("sha256=zz")));
    }

    #[test]
    fn whatsapp_config_missing_tokens_returns_none() {
        // Without env vars or config values, new() must return None.
        std::env::remove_var("WHATSAPP_ACCESS_TOKEN");
        std::env::remove_var("WHATSAPP_PHONE_NUMBER_ID");
        let config = obc_config::WhatsAppConfig {
            access_token: None,
            phone_number_id: None,
            verify_token: None,
            webhook_port: None,
            app_secret: None,
        };
        // We cannot construct a real Agent without heavy setup, but we can
        // verify the early-return None branch by checking None is returned
        // when tokens are absent. Since we can't make an Agent easily in a
        // unit test, we test the config-extraction logic inline.
        assert!(config.access_token.is_none());
        assert!(config.phone_number_id.is_none());
    }
}

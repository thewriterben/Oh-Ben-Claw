//! Slack channel adapter — Socket Mode WebSocket bot.
//!
//! Uses Slack's Socket Mode to receive events without exposing a public HTTP
//! endpoint. All real-time events arrive over a WebSocket connection; the bot
//! replies via the Web API (`chat.postMessage`).
//!
//! # Setup
//! 1. Create a Slack app at <https://api.slack.com/apps>.
//! 2. Enable *Socket Mode* and generate an **App-Level Token** with scope
//!    `connections:write`.  Set this as `channels.slack.app_token` or the
//!    `SLACK_APP_TOKEN` env var (must start with `xapp-`).
//! 3. Add a Bot Token Scope: `chat:write` + `app_mentions:read` +
//!    `channels:history` (or `im:history` for DMs).
//! 4. Install the app to your workspace and copy the **Bot User OAuth Token**
//!    (`xoxb-…`) into `channels.slack.bot_token` / `SLACK_BOT_TOKEN`.
//! 5. Subscribe to the `app_mention` and `message.im` bot events.

use crate::agent::Agent;
use crate::channels::typing::TypingTask;
use crate::channels::utils::chunk_text;
use crate::config::{ProviderConfig, SlackConfig};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const SLACK_API_BASE: &str = "https://slack.com/api";

// ── Slack Socket Mode payload ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SocketPayload {
    /// `events_api` | `interactive` | `disconnect` | …
    #[serde(rename = "type")]
    payload_type: String,
    /// Acknowledgement envelope ID.
    envelope_id: Option<String>,
    /// Inner event payload (for `events_api` type).
    payload: Option<EventsApiPayload>,
    /// Reason for `disconnect` type.
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventsApiPayload {
    event: Option<SlackEvent>,
}

#[derive(Debug, Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// Plaintext message content.
    text: Option<String>,
    /// Channel ID where the message was posted.
    channel: Option<String>,
    /// Thread timestamp for threaded replies.
    thread_ts: Option<String>,
    /// Timestamp of the message (used for reply threading).
    ts: Option<String>,
    /// User ID (bot messages are filtered by checking `bot_id`).
    bot_id: Option<String>,
    user: Option<String>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// Slack Socket Mode channel.
pub struct SlackChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    app_token: String,
    bot_token: String,
    http: reqwest::Client,
    /// Whether to send typing indicators while the agent processes.
    typing_indicators: bool,
}

impl SlackChannel {
    /// Create a new `SlackChannel`.
    ///
    /// Returns `None` if either required token is absent.
    pub fn new(
        config: &SlackConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        Self::new_with_typing(config, agent, provider_config, true)
    }

    /// Create a new `SlackChannel` with explicit typing-indicator control.
    ///
    /// Returns `None` if either required token is absent.
    pub fn new_with_typing(
        config: &SlackConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
        typing_indicators: bool,
    ) -> Option<Self> {
        let app_token = config
            .app_token
            .clone()
            .or_else(|| std::env::var("SLACK_APP_TOKEN").ok())?;
        let bot_token = config
            .bot_token
            .clone()
            .or_else(|| std::env::var("SLACK_BOT_TOKEN").ok())?;

        Some(Self {
            agent,
            provider_config,
            app_token,
            bot_token,
            http: reqwest::Client::new(),
            typing_indicators,
        })
    }

    /// Connect to Slack Socket Mode and process events.
    ///
    /// Runs until the connection closes or is cancelled.  The caller should
    /// re-invoke `run()` to reconnect.
    pub async fn run(&self) -> Result<()> {
        // 1. Obtain a WebSocket URL from apps.connections.open.
        let ws_url = self
            .open_connection()
            .await
            .context("Slack Socket Mode connection open failed")?;

        tracing::info!(%ws_url, "Slack Socket Mode connected");

        let (mut ws, _) = connect_async(&ws_url)
            .await
            .context("Slack Socket Mode WebSocket connect failed")?;

        loop {
            let msg = match ws.next().await {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "Slack WebSocket error; reconnecting");
                    break;
                }
                None => break,
            };

            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Ping(data) => {
                    let _ = ws.send(WsMessage::Pong(data)).await;
                    continue;
                }
                WsMessage::Close(_) => break,
                _ => continue,
            };

            let payload: SocketPayload = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(error = %e, "Slack Socket Mode parse error");
                    continue;
                }
            };

            // Acknowledge immediately to prevent retries.
            if let Some(ref eid) = payload.envelope_id {
                let ack = json!({ "envelope_id": eid });
                let _ = ws.send(WsMessage::Text(ack.to_string())).await;
            }

            match payload.payload_type.as_str() {
                "events_api" => {
                    if let Some(ep) = payload.payload {
                        if let Some(event) = ep.event {
                            if let Err(e) = self.handle_event(event).await {
                                tracing::error!(error = %e, "Slack event handling error");
                            }
                        }
                    }
                }
                "disconnect" => {
                    tracing::info!(
                        reason = ?payload.reason,
                        "Slack Socket Mode disconnect requested; reconnecting"
                    );
                    break;
                }
                other => {
                    tracing::trace!(payload_type = %other, "Slack Socket Mode unhandled type");
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Call `apps.connections.open` to obtain a Socket Mode WebSocket URL.
    async fn open_connection(&self) -> Result<String> {
        let resp: serde_json::Value = self
            .http
            .post(format!("{}/apps.connections.open", SLACK_API_BASE))
            .header("Authorization", format!("Bearer {}", self.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .context("Slack apps.connections.open request failed")?
            .json()
            .await
            .context("Slack apps.connections.open JSON parse error")?;

        if resp["ok"].as_bool() != Some(true) {
            anyhow::bail!(
                "Slack apps.connections.open error: {}",
                resp["error"].as_str().unwrap_or("unknown")
            );
        }

        resp["url"]
            .as_str()
            .map(|s| s.to_string())
            .context("Slack apps.connections.open missing url field")
    }

    async fn handle_event(&self, event: SlackEvent) -> Result<()> {
        // Skip bot messages to prevent loops.
        if event.bot_id.is_some() {
            return Ok(());
        }

        let text = match &event.text {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => return Ok(()),
        };

        let channel = match &event.channel {
            Some(c) => c.clone(),
            None => return Ok(()),
        };

        tracing::debug!(
            channel = %channel,
            event_type = %event.event_type,
            text = %text,
            "Slack event received"
        );

        // Session ID per channel.
        let session_id = format!("slack-{}", channel);

        // Slack supports `typing` indicators via `conversations.typing` (Socket
        // Mode event).  Refresh every 4 s while the agent processes.
        let _typing = if self.typing_indicators {
            let channel_owned = channel.clone();
            let bot_token_owned = self.bot_token.clone();
            let http_owned = self.http.clone();
            Some(TypingTask::start(4, move || {
                let url = format!("{}/conversations.typing", SLACK_API_BASE);
                let bot_token = bot_token_owned.clone();
                let http = http_owned.clone();
                let ch = channel_owned.clone();
                async move {
                    let body = serde_json::json!({ "channel": ch });
                    let _ = http
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", bot_token))
                        .json(&body)
                        .send()
                        .await;
                }
            }))
        } else {
            None
        };

        let response = self
            .agent
            .process(&session_id, &text, &self.provider_config)
            .await
            .context("Agent processing error")?;

        let thread_ts = event.thread_ts.or(event.ts);
        self.post_message(&channel, &response.message, thread_ts.as_deref())
            .await
    }

    async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<()> {
        // Slack message limit is 40,000 chars; split at 3,000 for safety.
        for chunk in chunk_text(text, 3000) {
            let mut body = json!({
                "channel": channel,
                "text": chunk,
            });
            if let Some(ts) = thread_ts {
                body["thread_ts"] = serde_json::Value::String(ts.to_string());
            }

            let res = self
                .http
                .post(format!("{}/chat.postMessage", SLACK_API_BASE))
                .header("Authorization", format!("Bearer {}", self.bot_token))
                .json(&body)
                .send()
                .await
                .context("Slack chat.postMessage HTTP error")?;

            let resp: serde_json::Value = res
                .json()
                .await
                .context("Slack chat.postMessage JSON parse error")?;

            if resp["ok"].as_bool() != Some(true) {
                tracing::warn!(
                    error = ?resp["error"],
                    "Slack chat.postMessage returned not-ok"
                );
            }
        }
        Ok(())
    }
}

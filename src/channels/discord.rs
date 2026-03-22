//! Discord channel adapter — Gateway WebSocket bot.
//!
//! Connects to the Discord Gateway via WebSocket, receives `MESSAGE_CREATE`
//! events, forwards user messages to the Oh-Ben-Claw agent, and replies in
//! the originating channel using the REST API.
//!
//! # Setup
//! 1. Create a bot application at <https://discord.com/developers/applications>.
//! 2. Enable *Message Content Intent* under **Bot → Privileged Gateway Intents**.
//! 3. Invite the bot with the `bot` scope and `Send Messages` + `Read Message
//!    History` permissions.
//! 4. Set `channels.discord.token` in `config.toml` (or `DISCORD_BOT_TOKEN`).
//!
//! # Limitations
//! The adapter handles the mandatory heartbeat and reconnect loop but does not
//! attempt a full voice-gateway or slash-command integration — plain text
//! messages only.

use crate::agent::Agent;
use crate::channels::typing::TypingTask;
use crate::channels::utils::chunk_text;
use crate::config::{DiscordConfig, ProviderConfig};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

// ── Discord Gateway constants ─────────────────────────────────────────────────

/// The intents bitmask: `GUILDS` (1) + `GUILD_MESSAGES` (512) +
/// `MESSAGE_CONTENT` (32768) + `DIRECT_MESSAGES` (4096).
const INTENTS: u64 = 1 | 512 | 32768 | 4096;

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

// ── Gateway payload types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GatewayPayload {
    /// Opcode
    op: u8,
    /// Event data (present for op 0)
    d: Option<serde_json::Value>,
    /// Sequence number (op 0 only)
    s: Option<i64>,
    /// Event name (op 0 only)
    t: Option<String>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// Discord Gateway bot channel.
pub struct DiscordChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    token: String,
    http: reqwest::Client,
    /// Whether to send typing indicators while the agent processes.
    typing_indicators: bool,
}

impl DiscordChannel {
    /// Create a new `DiscordChannel`.
    ///
    /// Returns `None` if no token is configured.
    pub fn new(
        config: &DiscordConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        Self::new_with_typing(config, agent, provider_config, true)
    }

    /// Create a new `DiscordChannel` with explicit typing-indicator control.
    ///
    /// Returns `None` if no token is configured.
    pub fn new_with_typing(
        config: &DiscordConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
        typing_indicators: bool,
    ) -> Option<Self> {
        let token = config
            .token
            .clone()
            .or_else(|| std::env::var("DISCORD_BOT_TOKEN").ok())?;

        Some(Self {
            agent,
            provider_config,
            token,
            http: reqwest::Client::new(),
            typing_indicators,
        })
    }

    /// Connect to the Discord Gateway and start the event loop.
    ///
    /// Handles the heartbeat, identifies, and processes `MESSAGE_CREATE`
    /// events until the task is cancelled.
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Discord channel connecting to Gateway");

        let (mut ws, _) = connect_async(GATEWAY_URL)
            .await
            .context("Discord Gateway WebSocket connect failed")?;

        let mut heartbeat_interval: Option<tokio::time::Interval> = None;
        let mut sequence: Option<i64> = None;
        let mut identified = false;

        loop {
            // Heartbeat tick — send if interval is set and due.
            let hb_pending = if let Some(ref mut iv) = heartbeat_interval {
                futures_util::future::Either::Left(iv.tick())
            } else {
                futures_util::future::Either::Right(futures_util::future::pending::<
                    tokio::time::Instant,
                >())
            };

            tokio::select! {
                _ = hb_pending => {
                    let hb = json!({ "op": 1, "d": sequence });
                    if ws.send(WsMessage::Text(hb.to_string())).await.is_err() {
                        tracing::warn!("Discord heartbeat send failed; reconnecting");
                        break;
                    }
                }

                msg = ws.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "Discord WebSocket error; reconnecting");
                            break;
                        }
                        None => break,
                    };

                    let text = match msg {
                        WsMessage::Text(t) => t,
                        WsMessage::Close(_) => {
                            tracing::info!("Discord Gateway closed; reconnecting");
                            break;
                        }
                        _ => continue,
                    };

                    let payload: GatewayPayload = match serde_json::from_str(&text) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::debug!(error = %e, "Discord Gateway parse error");
                            continue;
                        }
                    };

                    if let Some(s) = payload.s {
                        sequence = Some(s);
                    }

                    match payload.op {
                        // HELLO — start heartbeat and identify.
                        10 => {
                            let interval_ms = payload.d
                                .as_ref()
                                .and_then(|d| d["heartbeat_interval"].as_u64())
                                .unwrap_or(41_250);
                            heartbeat_interval =
                                Some(interval(Duration::from_millis(interval_ms)));

                            if !identified {
                                let identify = json!({
                                    "op": 2,
                                    "d": {
                                        "token": self.token,
                                        "intents": INTENTS,
                                        "properties": {
                                            "$os": "linux",
                                            "$browser": "oh-ben-claw",
                                            "$device": "oh-ben-claw"
                                        }
                                    }
                                });
                                let _ = ws.send(WsMessage::Text(identify.to_string())).await;
                                identified = true;
                            }
                        }
                        // HEARTBEAT ACK — nothing to do.
                        11 => {}
                        // DISPATCH — handle events.
                        0 => {
                            if let (Some(event_name), Some(data)) = (payload.t, payload.d) {
                                if event_name == "MESSAGE_CREATE" {
                                    if let Err(e) = self.handle_message_event(data).await {
                                        tracing::error!(error = %e, "Discord message handling error");
                                    }
                                }
                            }
                        }
                        op => {
                            tracing::trace!(op, "Discord Gateway unhandled opcode");
                        }
                    }
                }
            }
        }

        // Back-off before reconnecting (caller re-invokes run()).
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    async fn handle_message_event(&self, data: serde_json::Value) -> Result<()> {
        // Ignore messages sent by bots (including ourselves).
        if data["author"]["bot"].as_bool().unwrap_or(false) {
            return Ok(());
        }

        let content = match data["content"].as_str() {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => return Ok(()),
        };

        let channel_id = data["channel_id"]
            .as_str()
            .context("Discord message missing channel_id")?
            .to_string();

        let message_id = data["id"]
            .as_str()
            .context("Discord message missing id")?
            .to_string();

        let author_id = data["author"]["id"].as_str().unwrap_or("unknown");

        tracing::debug!(
            channel_id = %channel_id,
            author_id = %author_id,
            content = %content,
            "Discord message received"
        );

        // Built-in commands
        if content == "!help" {
            return self
                .send_reply(&channel_id, &message_id, "**Oh-Ben-Claw** — AI assistant\n`!help` show this help\n`!clear` clear session history")
                .await;
        }
        if content == "!clear" {
            let session_id = format!("discord-{}", channel_id);
            let _ = self.agent.clear_session(&session_id);
            return self
                .send_reply(&channel_id, &message_id, "Session history cleared.")
                .await;
        }

        // Session ID per-channel
        let session_id = format!("discord-{}", channel_id);

        // Start typing indicator — Discord's expires after ~10 s, refresh every 8 s.
        let _typing = if self.typing_indicators {
            let channel_id_owned = channel_id.clone();
            let token_owned = self.token.clone();
            let http_owned = self.http.clone();
            Some(TypingTask::start(8, move || {
                let url = format!("{}/channels/{}/typing", DISCORD_API_BASE, channel_id_owned);
                let token = token_owned.clone();
                let http = http_owned.clone();
                async move {
                    let _ = http
                        .post(&url)
                        .header("Authorization", format!("Bot {}", token))
                        .header("Content-Length", "0")
                        .send()
                        .await;
                }
            }))
        } else {
            None
        };

        let response = self
            .agent
            .process(&session_id, &content, &self.provider_config)
            .await
            .context("Agent processing error")?;

        self.send_reply(&channel_id, &message_id, &response.message)
            .await
    }

    async fn send_reply(&self, channel_id: &str, message_id: &str, text: &str) -> Result<()> {
        let url = format!("{}/channels/{}/messages", DISCORD_API_BASE, channel_id);

        // Discord message limit is 2000 characters.
        for chunk in chunk_text(text, 1900) {
            let body = json!({
                "content": chunk,
                "message_reference": { "message_id": message_id }
            });

            let res = self
                .http
                .post(&url)
                .header("Authorization", format!("Bot {}", self.token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .context("Discord sendMessage HTTP error")?;

            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                tracing::warn!(status = %status, body = %body, "Discord sendMessage failed");
            }
        }

        Ok(())
    }
}

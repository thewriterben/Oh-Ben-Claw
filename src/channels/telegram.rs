//! Telegram channel adapter — long-polling bot.
//!
//! Connects to the Telegram Bot API using long polling, forwards user messages
//! to the Oh-Ben-Claw agent, and replies in the originating chat.
//!
//! # Setup
//! 1. Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.
//! 2. Set `channels.telegram.token` in `config.toml` (or set `TELEGRAM_BOT_TOKEN`).
//!
//! # Limitations
//! Only text messages from private chats and groups are processed.  Media and
//! commands other than `/start`, `/help`, and `/clear` are ignored.

use crate::agent::Agent;
use crate::channels::typing::TypingTask;
use crate::channels::utils::chunk_text;
use crate::config::{ProviderConfig, TelegramConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Telegram API types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TgResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
}

#[derive(Debug, Deserialize, Clone)]
struct TgMessage {
    message_id: i64,
    chat: TgChat,
    from: Option<TgUser>,
    text: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct TgChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

#[derive(Debug, Deserialize, Clone)]
struct TgUser {
    id: i64,
    first_name: String,
    username: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    parse_mode: Option<&'a str>,
    reply_to_message_id: Option<i64>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// Telegram bot channel.
pub struct TelegramChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    token: String,
    api_base: String,
    http: reqwest::Client,
    /// Whether to send "typing…" indicators while the agent processes.
    typing_indicators: bool,
}

impl TelegramChannel {
    /// Create a new `TelegramChannel`.
    ///
    /// Returns `None` if no token is configured.
    pub fn new(
        config: &TelegramConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        Self::new_with_typing(config, agent, provider_config, true)
    }

    /// Create a new `TelegramChannel` with explicit typing-indicator control.
    ///
    /// Returns `None` if no token is configured.
    pub fn new_with_typing(
        config: &TelegramConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
        typing_indicators: bool,
    ) -> Option<Self> {
        let token = config
            .token
            .clone()
            .or_else(|| std::env::var("TELEGRAM_BOT_TOKEN").ok())?;

        Some(Self {
            agent,
            provider_config,
            api_base: format!("https://api.telegram.org/bot{}", token),
            token,
            http: reqwest::Client::new(),
            typing_indicators,
        })
    }

    /// Start the long-polling loop.
    ///
    /// Runs until the task is cancelled or a fatal error occurs.
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Telegram channel started (long-polling)");
        let mut offset: i64 = 0;

        loop {
            let updates = match self.get_updates(offset).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(error = %e, "Telegram getUpdates failed; retrying in 5 s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            for update in updates {
                offset = offset.max(update.update_id + 1);
                if let Some(msg) = update.message {
                    if let Err(e) = self.handle_message(msg).await {
                        tracing::error!(error = %e, "Failed to handle Telegram message");
                    }
                }
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    async fn get_updates(&self, offset: i64) -> Result<Vec<TgUpdate>> {
        let url = format!("{}/getUpdates", self.api_base);
        let resp: TgResponse<Vec<TgUpdate>> = self
            .http
            .get(&url)
            .query(&[
                ("offset", offset.to_string()),
                ("timeout", "30".into()),
                ("allowed_updates", "[\"message\"]".into()),
            ])
            .send()
            .await
            .context("Telegram getUpdates HTTP error")?
            .json()
            .await
            .context("Telegram getUpdates JSON parse error")?;

        if !resp.ok {
            anyhow::bail!(
                "Telegram API error: {}",
                resp.description.as_deref().unwrap_or("unknown")
            );
        }
        Ok(resp.result.unwrap_or_default())
    }

    async fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<()> {
        let url = format!("{}/sendMessage", self.api_base);
        // Split long messages to respect the 4096-char Telegram limit.
        for chunk in chunk_text(text, 4000) {
            let body = SendMessageRequest {
                chat_id,
                text: chunk,
                parse_mode: Some("Markdown"),
                reply_to_message_id: reply_to,
            };
            let resp: TgResponse<serde_json::Value> = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("Telegram sendMessage HTTP error")?
                .json()
                .await
                .context("Telegram sendMessage JSON parse error")?;

            if !resp.ok {
                tracing::warn!(
                    description = ?resp.description,
                    "Telegram sendMessage returned not-ok"
                );
            }
        }
        Ok(())
    }

    /// Send a `typing` chat action to Telegram.
    ///
    /// The indicator expires after ~5 seconds; callers should refresh it
    /// periodically while long operations are in progress.
    async fn send_chat_action(&self, chat_id: i64) {
        let url = format!("{}/sendChatAction", self.api_base);
        #[derive(serde::Serialize)]
        struct ChatActionBody {
            chat_id: i64,
            action: &'static str,
        }
        let _ = self
            .http
            .post(&url)
            .json(&ChatActionBody {
                chat_id,
                action: "typing",
            })
            .send()
            .await;
    }

    async fn handle_message(&self, msg: TgMessage) -> Result<()> {
        let text = match &msg.text {
            Some(t) => t.trim().to_string(),
            None => return Ok(()), // ignore non-text messages
        };

        tracing::debug!(
            chat_id = msg.chat.id,
            user_id = msg.from.as_ref().map(|u| u.id),
            text = %text,
            "Telegram message received"
        );

        // Built-in commands
        if text == "/start" || text == "/help" {
            return self
                .send_text(
                    msg.chat.id,
                    "👋 *Oh-Ben-Claw* is ready\\. Send me any message and I'll respond\\.\n\nCommands:\n• `/clear` — clear session history",
                    Some(msg.message_id),
                )
                .await;
        }
        if text == "/clear" {
            // Clear session history for this chat.
            let session_id = format!("tg-{}", msg.chat.id);
            let _ = self.agent.clear_session(&session_id);
            return self
                .send_text(
                    msg.chat.id,
                    "Session history cleared.",
                    Some(msg.message_id),
                )
                .await;
        }

        // Session ID per-chat
        let session_id = format!("tg-{}", msg.chat.id);

        // Start typing indicator while the agent processes the message.
        // Telegram's typing indicator expires after ~5 s, so we refresh it
        // every 4 s.  The task is dropped (cancelled) once we have a response.
        let _typing = if self.typing_indicators {
            let chat_id = msg.chat.id;
            let api_base = self.api_base.clone();
            let http = self.http.clone();
            Some(TypingTask::start(4, move || {
                let url = format!("{}/sendChatAction", api_base);
                let http = http.clone();
                async move {
                    #[derive(serde::Serialize)]
                    struct ChatActionBody {
                        chat_id: i64,
                        action: &'static str,
                    }
                    let _ = http
                        .post(&url)
                        .json(&ChatActionBody {
                            chat_id,
                            action: "typing",
                        })
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

        self.send_text(msg.chat.id, &response.message, Some(msg.message_id))
            .await
    }
}

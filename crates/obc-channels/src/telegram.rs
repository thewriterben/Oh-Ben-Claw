//! Telegram channel adapter — long-polling bot.
//!
//! Connects to the Telegram Bot API using long polling, forwards user messages
//! to the Oh-Ben-Claw agent, and replies in the originating chat.
//!
//! # Setup
//! 1. Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.
//! 2. Set `TELEGRAM_BOT_TOKEN` in the environment (or `channels.telegram.token`).
//! 3. Send the bot `/start`; the log names your user id as unlisted; put it in
//!    `channels.telegram.allowed_user_ids` and restart. Nobody else gets in.
//!
//! Since 2026-09-12 the adapter also carries the agent's *outbound* voice:
//! [`TelegramNotifyChannel`] delivers escalations and scheduled-task results to
//! every allowed user's private chat, which is what "and message me" means on
//! a phone.
//!
//! # Limitations
//! Only text messages from private chats and groups are processed.  Media and
//! commands other than `/start`, `/help`, and `/clear` are ignored. Replies are
//! sent as plain text: the previous `parse_mode = Markdown` made Telegram
//! reject any reply with an unbalanced `_` or `*`, silently.

// Wire-format types: fields mirror the platform's webhook payload and exist to
// document what arrives, even where this code does not read them. Deleting them
// would make the struct a worse description of the wire than the vendor's own docs.
// Scoped to this file deliberately — the crate root carries no blanket allow.
#![allow(dead_code)]
use crate::typing::TypingTask;
use crate::utils::chunk_text;
use anyhow::{Context, Result};
use async_trait::async_trait;
use obc_agent::notify::{Escalation, NotificationChannel};
use obc_agent::Agent;
use obc_config::{ProviderConfig, TelegramConfig};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Whether a sender may talk to the agent. An empty allowlist admits nobody.
pub fn allowed(allowed_user_ids: &[i64], user_id: Option<i64>) -> bool {
    match user_id {
        Some(id) => allowed_user_ids.contains(&id),
        None => false,
    }
}

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
    /// A voice note (ogg/opus) — transcribed when `transcribe_voice` is on.
    #[serde(default)]
    voice: Option<TgFile>,
    /// An audio file (mp3/m4a…) — treated like a voice note.
    #[serde(default)]
    audio: Option<TgFile>,
}

#[derive(Debug, Deserialize, Clone)]
struct TgFile {
    file_id: String,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TgFilePath {
    file_path: Option<String>,
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

/// `sendMessage` body. The two optionals are *omitted* when unset: Telegram
/// answers `"parse_mode": null` with `Bad Request: unsupported parse_mode`,
/// which is how the first allowlisted reply on the bench never arrived
/// (2026-09-12).
#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    allowed_user_ids: Vec<i64>,
    /// `OPENAI_API_BASE` when `transcribe_voice` is on — the speech endpoint
    /// (transcriptions in, optionally speech out).
    speech_base: Option<String>,
    voice_replies: bool,
    tts_voice: String,
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

        if config.allowed_user_ids.is_empty() {
            tracing::warn!(
                "Telegram: allowed_user_ids is empty, so every sender is refused; send the bot \
                 /start and add the user id the log names to [channels.telegram]"
            );
        }
        Some(Self {
            agent,
            provider_config,
            api_base: format!("https://api.telegram.org/bot{}", token),
            token,
            http: reqwest::Client::new(),
            allowed_user_ids: config.allowed_user_ids.clone(),
            speech_base: config
                .transcribe_voice
                .then(|| std::env::var("OPENAI_API_BASE").ok())
                .flatten()
                .map(|b| b.trim_end_matches('/').to_string()),
            voice_replies: config.voice_replies,
            tts_voice: config.tts_voice.clone(),
            typing_indicators,
        })
    }

    /// Fetch a Telegram file's bytes (`getFile`, then the file endpoint).
    async fn download(&self, file_id: &str) -> Result<(Vec<u8>, String)> {
        let meta: TgResponse<TgFilePath> = self
            .http
            .get(format!("{}/getFile", self.api_base))
            .query(&[("file_id", file_id)])
            .send()
            .await
            .context("Telegram getFile HTTP error")?
            .json()
            .await
            .context("Telegram getFile JSON parse error")?;
        let path = meta
            .result
            .and_then(|r| r.file_path)
            .ok_or_else(|| anyhow::anyhow!("Telegram getFile returned no file_path"))?;
        let bytes = self
            .http
            .get(format!(
                "https://api.telegram.org/file/bot{}/{}",
                self.token, path
            ))
            .send()
            .await
            .context("Telegram file download HTTP error")?
            .bytes()
            .await
            .context("Telegram file download read error")?
            .to_vec();
        let name = path.rsplit('/').next().unwrap_or("voice.ogg").to_string();
        Ok((bytes, name))
    }

    /// Transcribe audio bytes through the speech endpoint (Whisper-style).
    async fn transcribe(&self, bytes: Vec<u8>, file_name: &str, mime: &str) -> Result<String> {
        let base = self
            .speech_base
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no speech endpoint (OPENAI_API_BASE unset)"))?;
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name(file_name.to_string())
                    .mime_str(mime)
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new())),
            )
            .text("model", "whisper-1")
            .text("response_format", "text");
        let resp = self
            .http
            .post(format!("{base}/audio/transcriptions"))
            .bearer_auth(std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "local".into()))
            .multipart(form)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .context("transcription HTTP error")?;
        if !resp.status().is_success() {
            anyhow::bail!("transcription returned {}", resp.status());
        }
        Ok(resp.text().await.unwrap_or_default().trim().to_string())
    }

    /// Render `text` to mp3 through the speech endpoint and send it as audio.
    async fn send_voice_reply(&self, chat_id: i64, text: &str, reply_to: i64) -> Result<()> {
        let base = self
            .speech_base
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no speech endpoint (OPENAI_API_BASE unset)"))?;
        let spoken: String = text.chars().take(1500).collect();
        let resp = self
            .http
            .post(format!("{base}/audio/speech"))
            .bearer_auth(std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "local".into()))
            .json(&serde_json::json!({
                "model": "kokoro", "input": spoken, "voice": self.tts_voice, "response_format": "mp3"
            }))
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .context("speech HTTP error")?;
        if !resp.status().is_success() {
            anyhow::bail!("speech returned {}", resp.status());
        }
        let mp3 = resp.bytes().await?.to_vec();
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .text("reply_to_message_id", reply_to.to_string())
            .part(
                "audio",
                reqwest::multipart::Part::bytes(mp3)
                    .file_name("reply.mp3")
                    .mime_str("audio/mpeg")
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new())),
            );
        let resp: TgResponse<serde_json::Value> = self
            .http
            .post(format!("{}/sendAudio", self.api_base))
            .multipart(form)
            .send()
            .await
            .context("Telegram sendAudio HTTP error")?
            .json()
            .await
            .context("Telegram sendAudio JSON parse error")?;
        if !resp.ok {
            anyhow::bail!(
                "Telegram sendAudio refused: {}",
                resp.description.as_deref().unwrap_or("unknown")
            );
        }
        Ok(())
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
            // Plain text. With `parse_mode = Markdown` any reply holding an
            // unbalanced `_` or `*` (a path, a snake_case name, a bullet) came
            // back 400 "can't parse entities" and was dropped with a warning
            // the operator never saw.
            let body = SendMessageRequest {
                chat_id,
                text: chunk,
                parse_mode: None,
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
                anyhow::bail!(
                    "Telegram sendMessage refused: {}",
                    resp.description.as_deref().unwrap_or("unknown")
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
        // Text, or a voice note / audio message transcribed into text. Anything
        // else (stickers, photos) is ignored. The allowlist check below still
        // comes before any work on an unlisted sender's media.
        let spoken = msg.voice.as_ref().or(msg.audio.as_ref()).cloned();
        let mut was_voice = false;
        let text = match (&msg.text, spoken) {
            (Some(t), _) => t.trim().to_string(),
            (None, Some(file)) if self.speech_base.is_some() => {
                let user_id = msg.from.as_ref().map(|u| u.id);
                if !allowed(&self.allowed_user_ids, user_id) {
                    return self
                        .send_text(msg.chat.id, "This bot is private.", Some(msg.message_id))
                        .await;
                }
                let (bytes, name) = self.download(&file.file_id).await?;
                let mime = file
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "audio/ogg".to_string());
                let transcript = self.transcribe(bytes, &name, &mime).await?;
                tracing::info!(
                    chat_id = msg.chat.id,
                    seconds = ?file.duration,
                    chars = transcript.len(),
                    "Telegram voice note transcribed"
                );
                if transcript.is_empty() {
                    return self
                        .send_text(
                            msg.chat.id,
                            "I couldn't make out any words in that.",
                            Some(msg.message_id),
                        )
                        .await;
                }
                was_voice = true;
                transcript
            }
            _ => return Ok(()),
        };

        tracing::debug!(
            chat_id = msg.chat.id,
            user_id = msg.from.as_ref().map(|u| u.id),
            text = %text,
            "Telegram message received"
        );

        // The allowlist comes before anything else, commands included.
        let user_id = msg.from.as_ref().map(|u| u.id);
        if !allowed(&self.allowed_user_ids, user_id) {
            tracing::warn!(
                user_id = ?user_id,
                username = ?msg.from.as_ref().and_then(|u| u.username.clone()),
                first_name = ?msg.from.as_ref().map(|u| u.first_name.clone()),
                chat_id = msg.chat.id,
                "Telegram: message from an unlisted user refused; add the id to \
                 [channels.telegram] allowed_user_ids to let them in"
            );
            return self
                .send_text(msg.chat.id, "This bot is private.", Some(msg.message_id))
                .await;
        }

        // Built-in commands
        if text == "/start" || text == "/help" {
            return self
                .send_text(
                    msg.chat.id,
                    "Oh-Ben-Claw is ready. Send me any message and I'll respond.\n\nCommands:\n/clear - clear this chat's session history",
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
            .await?;
        if was_voice && self.voice_replies {
            if let Err(e) = self
                .send_voice_reply(msg.chat.id, &response.message, msg.message_id)
                .await
            {
                tracing::warn!(error = %e, "Telegram voice reply failed; the text reply was sent");
            }
        }
        Ok(())
    }
}

// ── Outbound: escalations and scheduled results ──────────────────────────────

/// The agent's outbound voice on Telegram: a [`NotificationChannel`] that
/// delivers escalations and scheduled-task results to every allowed user's
/// private chat (for a private chat, chat id == user id). Built from the same
/// `[channels.telegram]` block as the inbound adapter; `None` without a token,
/// with `notify = false`, or with nobody allowlisted.
pub struct TelegramNotifyChannel {
    api_base: String,
    chat_ids: Vec<i64>,
    http: reqwest::Client,
    /// `(start, end)` in minutes since local midnight; may wrap past midnight.
    quiet: Option<(u32, u32)>,
}

/// Parse `"HH:MM-HH:MM"` into minutes since midnight.
pub fn parse_quiet_hours(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.trim().split_once('-')?;
    let hm = |t: &str| -> Option<u32> {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60).then_some(h * 60 + m)
    };
    Some((hm(a)?, hm(b)?))
}

/// Whether `now` (minutes since local midnight) falls in the window; a window
/// whose end is before its start wraps past midnight (`23:00-07:00`).
pub fn in_quiet_hours(now: u32, window: (u32, u32)) -> bool {
    let (start, end) = window;
    if start == end {
        false
    } else if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

impl TelegramNotifyChannel {
    pub fn from_config(config: &TelegramConfig) -> Option<Self> {
        if !config.notify || config.allowed_user_ids.is_empty() {
            return None;
        }
        let token = config
            .token
            .clone()
            .or_else(|| std::env::var("TELEGRAM_BOT_TOKEN").ok())?;
        let quiet = config.quiet_hours.as_deref().and_then(|q| {
            let parsed = parse_quiet_hours(q);
            if parsed.is_none() {
                tracing::warn!(
                    quiet_hours = q,
                    "Telegram: quiet_hours must be HH:MM-HH:MM; ignored"
                );
            }
            parsed
        });
        Some(Self {
            api_base: format!("https://api.telegram.org/bot{token}"),
            chat_ids: config.allowed_user_ids.clone(),
            http: reqwest::Client::new(),
            quiet,
        })
    }

    fn quiet_now(&self) -> bool {
        use chrono::Timelike;
        let Some(window) = self.quiet else {
            return false;
        };
        let now = chrono::Local::now();
        in_quiet_hours(now.hour() * 60 + now.minute(), window)
    }

    pub fn chat_count(&self) -> usize {
        self.chat_ids.len()
    }

    /// The text sent for an escalation: the reason as it is (scheduled results
    /// already read `⏰ name: …`), prefixed for reflex escalations so a phone
    /// notification says where it came from.
    pub fn text_for(esc: &Escalation) -> String {
        if esc.reason.starts_with('⏰') {
            esc.reason.clone()
        } else {
            format!("OBC: {}", esc.reason)
        }
    }

    async fn send_plain(&self, chat_id: i64, text: &str) -> Result<()> {
        let url = format!("{}/sendMessage", self.api_base);
        for chunk in chunk_text(text, 4000) {
            let body = SendMessageRequest {
                chat_id,
                text: chunk,
                parse_mode: None,
                reply_to_message_id: None,
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
                anyhow::bail!(
                    "Telegram sendMessage refused: {}",
                    resp.description.as_deref().unwrap_or("unknown")
                );
            }
        }
        Ok(())
    }
}

#[async_trait]
impl NotificationChannel for TelegramNotifyChannel {
    fn name(&self) -> &str {
        "telegram"
    }
    async fn deliver(&self, esc: &Escalation) -> Result<()> {
        // Quiet hours hold back the routine; warnings and critical still ring.
        if esc.severity < obc_reflex::Severity::Warning && self.quiet_now() {
            tracing::info!(
                reason = %esc.reason.chars().take(80).collect::<String>(),
                "Telegram: routine notification held (quiet hours)"
            );
            return Ok(());
        }
        let text = Self::text_for(esc);
        let mut first_err = None;
        for chat_id in &self.chat_ids {
            if let Err(e) = self.send_plain(*chat_id, &text).await {
                tracing::warn!(chat_id, error = %e, "Telegram notification failed");
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod allowlist_tests {
    use super::*;

    #[test]
    fn a_plain_text_send_body_carries_no_null_fields() {
        let body = SendMessageRequest {
            chat_id: 42,
            text: "hi",
            parse_mode: None,
            reply_to_message_id: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"chat_id":42,"text":"hi"}"#);
        let body = SendMessageRequest {
            chat_id: 42,
            text: "hi",
            parse_mode: None,
            reply_to_message_id: Some(7),
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"chat_id":42,"text":"hi","reply_to_message_id":7}"#
        );
    }

    #[test]
    fn an_empty_allowlist_admits_nobody_and_a_listed_id_gets_in() {
        assert!(!allowed(&[], Some(42)));
        assert!(!allowed(&[42], None));
        assert!(allowed(&[42, 7], Some(7)));
        assert!(!allowed(&[42], Some(43)));
    }

    #[test]
    fn the_notify_channel_needs_a_token_and_someone_to_tell() {
        let mut cfg = TelegramConfig {
            token: Some("123:abc".into()),
            allowed_user_ids: vec![42],
            notify: true,
            notify_min_severity: None,
            quiet_hours: Some("23:00-07:00".into()),
            transcribe_voice: true,
            voice_replies: false,
            tts_voice: "af_bella".into(),
        };
        assert_eq!(
            TelegramNotifyChannel::from_config(&cfg)
                .unwrap()
                .chat_count(),
            1
        );
        cfg.notify = false;
        assert!(TelegramNotifyChannel::from_config(&cfg).is_none());
        cfg.notify = true;
        cfg.allowed_user_ids.clear();
        assert!(TelegramNotifyChannel::from_config(&cfg).is_none());
    }

    #[test]
    fn quiet_hours_parse_and_wrap_past_midnight() {
        assert_eq!(parse_quiet_hours("23:00-07:00"), Some((23 * 60, 7 * 60)));
        assert_eq!(
            parse_quiet_hours(" 22:30 - 06:15 "),
            Some((22 * 60 + 30, 6 * 60 + 15))
        );
        assert_eq!(parse_quiet_hours("25:00-07:00"), None);
        assert_eq!(parse_quiet_hours("nope"), None);
        let w = (23 * 60, 7 * 60);
        assert!(in_quiet_hours(23 * 60 + 30, w));
        assert!(in_quiet_hours(2 * 60, w));
        assert!(!in_quiet_hours(7 * 60, w));
        assert!(!in_quiet_hours(12 * 60, w));
        assert!(in_quiet_hours(13 * 60, (12 * 60, 14 * 60)));
        assert!(!in_quiet_hours(13 * 60, (14 * 60, 14 * 60)));
    }

    #[test]
    fn voice_notes_parse_out_of_an_update() {
        let raw = r#"{"message_id": 7, "chat": {"id": 42, "type": "private"},
            "from": {"id": 42, "first_name": "B"},
            "voice": {"file_id": "AwAC", "duration": 3, "mime_type": "audio/ogg"}}"#;
        let m: TgMessage = serde_json::from_str(raw).unwrap();
        assert!(m.text.is_none());
        assert_eq!(m.voice.as_ref().unwrap().file_id, "AwAC");
        assert_eq!(m.voice.as_ref().unwrap().duration, Some(3));
    }

    #[test]
    fn scheduled_results_go_out_as_they_are_and_escalations_get_a_prefix() {
        let sched = Escalation::new("⏰ Printer check: bed is 60C", 0);
        assert_eq!(
            TelegramNotifyChannel::text_for(&sched),
            "⏰ Printer check: bed is 60C"
        );
        let esc = Escalation::new("mesh node lost", 0);
        assert_eq!(TelegramNotifyChannel::text_for(&esc), "OBC: mesh node lost");
    }
}

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
//! 6. For approvals from Slack (2026-11-01): turn on *Interactivity* (Socket
//!    Mode carries the taps, so no Request URL is needed) and list the member
//!    IDs who may answer in `channels.slack.approver_users`. A gated call from
//!    a scheduled task or an A2A call then reaches each of them as a direct
//!    message with Approve / Deny buttons ([`SlackApprovalPrompter`]); a tap
//!    from anyone else is refused and audited.

// Wire-format types: fields mirror the platform's webhook payload and exist to
// document what arrives, even where this code does not read them. Deleting them
// would make the struct a worse description of the wire than the vendor's own docs.
// Scoped to this file deliberately — the crate root carries no blanket allow.
#![allow(dead_code)]
use crate::typing::TypingTask;
use crate::utils::{chunk_text, sender_allowed};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use obc_agent::Agent;
use obc_approval::{chat_code, ApprovalPrompter, ApprovalResponse, PendingApproval};
use obc_config::{ProviderConfig, SlackConfig};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const SLACK_API_BASE: &str = "https://slack.com/api";

// ── Slack Socket Mode payload ─────────────────────────────────────────────────

/// An `interactive` payload: a button tap (`block_actions`).
#[derive(Debug, Deserialize)]
struct Interaction {
    #[serde(rename = "type")]
    kind: String,
    user: SlackRef,
    #[serde(default)]
    channel: Option<SlackRef>,
    #[serde(default)]
    message: Option<TappedMessage>,
    #[serde(default)]
    actions: Vec<SlackAction>,
}

#[derive(Debug, Deserialize)]
struct SlackRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct TappedMessage {
    ts: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SlackAction {
    action_id: String,
    #[serde(default)]
    value: Option<String>,
}

/// Every approval button's `action_id` starts with this.
const APPROVAL_ACTION: &str = "obc_approval_";

#[derive(Debug, Deserialize)]
struct SocketPayload {
    /// `events_api` | `interactive` | `disconnect` | …
    #[serde(rename = "type")]
    payload_type: String,
    /// Acknowledgement envelope ID.
    envelope_id: Option<String>,
    /// Inner payload: an [`EventsApiPayload`] for `events_api`, an
    /// [`Interaction`] for `interactive`.
    payload: Option<serde_json::Value>,
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
    allowed_users: Vec<String>,
    /// Who may answer an approval request (`approver_users`).
    approvers: Vec<String>,
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
            allowed_users: config.allowed_users.clone(),
            approvers: config.approver_users.clone(),
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
                    let ep = payload
                        .payload
                        .and_then(|v| serde_json::from_value::<EventsApiPayload>(v).ok());
                    if let Some(ep) = ep {
                        if let Some(event) = ep.event {
                            if let Err(e) = self.handle_event(event).await {
                                tracing::error!(error = %e, "Slack event handling error");
                            }
                        }
                    }
                }
                "interactive" => match payload.payload.map(serde_json::from_value::<Interaction>) {
                    Some(Ok(tap)) => self.handle_tap(tap).await,
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "Slack interactive payload not understood")
                    }
                    None => {}
                },
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

        if !sender_allowed(
            "slack",
            &self.allowed_users,
            event.user.as_deref().unwrap_or(""),
        ) {
            return Ok(());
        }

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

    /// An approval button was tapped: answer the waiting call, then replace
    /// the buttons with the outcome; a tap that did nothing tells only the
    /// person who tapped, why.
    async fn handle_tap(&self, tap: Interaction) {
        let Some(action) = tap
            .actions
            .iter()
            .find(|a| a.action_id.starts_with(APPROVAL_ACTION))
        else {
            tracing::trace!(kind = %tap.kind, "Slack interaction without an approval button");
            return;
        };
        tracing::info!(user = %tap.user.id, "Slack approval button tapped");
        let answer = crate::approval::answer_tap(
            self.agent.approval_manager().map(|a| a.as_ref()),
            "slack",
            action.value.as_deref().unwrap_or_default(),
            &tap.user.id,
            &self.approvers,
        );
        let Some(channel) = tap.channel.map(|c| c.id) else {
            return;
        };
        let (method, body) = match (answer, tap.message) {
            (Ok(line), Some(msg)) => (
                "chat.update",
                answered_message(
                    &channel,
                    &msg.ts,
                    msg.text.as_deref().unwrap_or_default(),
                    &line,
                ),
            ),
            (Ok(_), None) => return,
            (Err(why), _) => (
                "chat.postEphemeral",
                json!({ "channel": channel, "user": tap.user.id, "text": why }),
            ),
        };
        if let Err(e) = slack_call(&self.http, &self.bot_token, method, &body).await {
            tracing::warn!(error = %e, method, "Slack approval reply failed");
        }
    }

    async fn post_message(&self, channel: &str, text: &str, thread_ts: Option<&str>) -> Result<()> {
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

/// POST `body` to Web API `method` as the bot; `ok: false` is an error.
async fn slack_call(
    http: &reqwest::Client,
    bot_token: &str,
    method: &str,
    body: &serde_json::Value,
) -> Result<()> {
    let resp: serde_json::Value = http
        .post(format!("{SLACK_API_BASE}/{method}"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .json(body)
        .send()
        .await
        .with_context(|| format!("Slack {method} HTTP error"))?
        .json()
        .await
        .with_context(|| format!("Slack {method} JSON parse error"))?;
    if resp["ok"].as_bool() != Some(true) {
        anyhow::bail!("Slack {method} returned {}", resp["error"]);
    }
    Ok(())
}

/// A section block's text holds 3,000 characters at most.
fn section(text: &str) -> serde_json::Value {
    let text: String = text.chars().take(2900).collect();
    json!({ "type": "section", "text": { "type": "plain_text", "text": text } })
}

/// The `chat.update` body that replaces a request's buttons with `line`.
fn answered_message(channel: &str, ts: &str, original: &str, line: &str) -> serde_json::Value {
    json!({
        "channel": channel,
        "ts": ts,
        "text": format!("{original}\n\n{line}"),
        "blocks": [
            section(original),
            { "type": "context", "elements": [{ "type": "plain_text", "text": line }] },
        ],
    })
}

/// Approval requests on Slack (2026-11-01): an [`ApprovalPrompter`] that
/// sends a waiting call to each of `approver_users` as a direct message from
/// the bot, with one button per answer. The tap comes back over
/// [`SlackChannel`]'s Socket Mode connection, so the inbound channel has to be
/// running, with Interactivity on, for an answer to land. `None` without a
/// bot token or without approvers.
#[derive(Clone)]
pub struct SlackApprovalPrompter {
    bot_token: String,
    approvers: Vec<String>,
    http: reqwest::Client,
}

impl SlackApprovalPrompter {
    pub fn from_config(config: &SlackConfig) -> Option<Self> {
        if config.approver_users.is_empty() {
            return None;
        }
        let bot_token = config
            .bot_token
            .clone()
            .or_else(|| std::env::var("SLACK_BOT_TOKEN").ok())?;
        Some(Self {
            bot_token,
            approvers: config.approver_users.clone(),
            http: reqwest::Client::new(),
        })
    }

    pub fn approver_count(&self) -> usize {
        self.approvers.len()
    }

    /// The `chat.postMessage` body for one approver (posting to a member ID
    /// lands in their DM with the app): the request and a row of buttons.
    fn message(pending: &PendingApproval, user: &str) -> serde_json::Value {
        let buttons: Vec<serde_json::Value> = pending
            .chat_choices()
            .iter()
            .map(|(decision, label)| {
                let mut b = json!({
                    "type": "button",
                    "text": { "type": "plain_text", "text": label },
                    "action_id": format!("{APPROVAL_ACTION}{}", chat_code(decision)),
                    "value": crate::approval::button_value(decision, &pending.id),
                });
                match decision {
                    ApprovalResponse::Yes => b["style"] = json!("primary"),
                    ApprovalResponse::No => b["style"] = json!("danger"),
                    _ => {}
                }
                b
            })
            .collect();
        let text = pending.prompt_text();
        json!({
            "channel": user,
            "text": text,
            "blocks": [section(&text), { "type": "actions", "elements": buttons }],
        })
    }
}

impl ApprovalPrompter for SlackApprovalPrompter {
    fn name(&self) -> &str {
        "slack"
    }

    fn prompt(&self, pending: &PendingApproval) {
        let this = self.clone();
        let bodies: Vec<serde_json::Value> = self
            .approvers
            .iter()
            .map(|user| Self::message(pending, user))
            .collect();
        tokio::spawn(async move {
            for body in bodies {
                if let Err(e) =
                    slack_call(&this.http, &this.bot_token, "chat.postMessage", &body).await
                {
                    tracing::warn!(
                        user = ?body["channel"],
                        error = %e,
                        "Slack approval request not delivered"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending() -> PendingApproval {
        PendingApproval {
            id: "0123456789abcdef0123456789abcdef".into(),
            tool_name: "open_gate".into(),
            arguments: json!({}),
            risk: None,
            origin: "an A2A call".into(),
            reason: "unattended".into(),
            expires_in_secs: 300,
            session_grant_secs: 0,
        }
    }

    #[test]
    fn a_tap_parses_out_of_a_socket_mode_envelope() {
        let raw = r#"{"type":"interactive","envelope_id":"e1","accepts_response_payload":false,
            "payload":{"type":"block_actions","user":{"id":"U1","name":"b"},
            "channel":{"id":"D1","name":"directmessage"},
            "message":{"ts":"1.2","text":"Approve?","type":"message"},
            "actions":[{"action_id":"obc_approval_y","block_id":"x","type":"button",
                        "value":"obc:y:0123456789abcdef0123456789abcdef"}]}}"#;
        let env: SocketPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(env.payload_type, "interactive");
        let tap: Interaction = serde_json::from_value(env.payload.unwrap()).unwrap();
        assert_eq!(tap.kind, "block_actions");
        assert_eq!(tap.user.id, "U1");
        assert_eq!(tap.channel.unwrap().id, "D1");
        assert_eq!(tap.message.unwrap().ts, "1.2");
        assert!(tap.actions[0].action_id.starts_with(APPROVAL_ACTION));

        // An events_api envelope still parses into its event.
        let raw = r#"{"type":"events_api","envelope_id":"e2","payload":{"event":{"type":"app_mention","user":"U1","text":"hi","channel":"C1","ts":"3.4"}}}"#;
        let env: SocketPayload = serde_json::from_str(raw).unwrap();
        let ep: EventsApiPayload = serde_json::from_value(env.payload.unwrap()).unwrap();
        assert!(ep.event.is_some());
    }

    #[test]
    fn an_approval_request_is_a_dm_with_approve_and_deny_only_when_no_window() {
        let cfg = SlackConfig {
            bot_token: Some("xoxb-1".into()),
            approver_users: vec!["U1".into()],
            ..SlackConfig::default()
        };
        assert_eq!(
            SlackApprovalPrompter::from_config(&cfg)
                .unwrap()
                .approver_count(),
            1
        );
        let body = SlackApprovalPrompter::message(&pending(), "U1");
        assert_eq!(body["channel"], "U1");
        let buttons = body["blocks"][1]["elements"].as_array().unwrap();
        let labels: Vec<&str> = buttons
            .iter()
            .map(|b| b["text"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["Approve", "Deny"]);
        assert_eq!(buttons[0]["style"], "primary");
        assert_eq!(buttons[1]["style"], "danger");
        for b in buttons {
            assert!(crate::approval::parse_button_value(b["value"].as_str().unwrap()).is_some());
        }

        let answered = answered_message("D1", "1.2", "Approve?", "⛔ Denied by slack:U1");
        assert_eq!(
            answered["blocks"].as_array().unwrap().len(),
            2,
            "no buttons left"
        );

        let cfg = SlackConfig {
            bot_token: Some("xoxb-1".into()),
            allowed_users: vec!["U1".into()],
            ..SlackConfig::default()
        };
        assert!(SlackApprovalPrompter::from_config(&cfg).is_none());
    }
}

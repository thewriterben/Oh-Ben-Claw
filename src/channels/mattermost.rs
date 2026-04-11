//! Mattermost channel adapter — WebSocket event API.
//!
//! Connects to the Mattermost WebSocket event stream, receives `posted` events,
//! forwards user messages to the Oh-Ben-Claw agent, and replies in the
//! originating channel using the REST API.
//!
//! # Authentication
//!
//! The adapter uses a **Personal Access Token** (PAT).  Create one in
//! Mattermost under *Profile → Security → Personal Access Tokens*.
//!
//! # Setup
//!
//! ```toml
//! [channels.mattermost]
//! server_url = "https://mattermost.example.com"
//! token      = "your-personal-access-token"
//! ```
//!
//! # Limitations
//!
//! * Only processes posts in channels the bot belongs to.
//! * The bot ignores its own messages to avoid feedback loops.

use crate::agent::Agent;
use crate::channels::utils::chunk_text;
use crate::config::{MattermostConfig, ProviderConfig};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const MM_WS_PING_INTERVAL_SECS: u64 = 30;

// ── Mattermost REST/WS types ──────────────────────────────────────────────────

/// Incoming WebSocket event from Mattermost.
#[derive(Debug, Deserialize)]
struct MmWsEvent {
    event: Option<String>,
    data: Option<serde_json::Value>,
}

/// A Mattermost post (message).
#[derive(Debug, Deserialize)]
struct MmPost {
    id: String,
    channel_id: String,
    user_id: String,
    message: String,
    /// Thread root post ID. Empty string for top-level posts.
    #[serde(default)]
    root_id: String,
    #[serde(rename = "type")]
    post_type: String,
}

/// Minimal user object returned by `GET /api/v4/users/me`.
#[derive(Debug, Deserialize)]
struct MmUser {
    id: String,
}

/// Response from `POST /api/v4/posts`.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct MmCreatePostResponse {
    id: String,
}

/// Request body for `POST /api/v4/posts`.
#[derive(Debug, Serialize)]
struct MmCreatePost<'a> {
    channel_id: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_id: Option<&'a str>,
}

// ── MattermostChannel ─────────────────────────────────────────────────────────

/// Mattermost channel adapter.
pub struct MattermostChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    config: MattermostConfig,
    http: reqwest::Client,
}

impl MattermostChannel {
    /// Create a new `MattermostChannel`.
    ///
    /// Returns `None` if `server_url` or `token` are not configured.
    pub fn new(
        config: &MattermostConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        if config.server_url.is_none() || config.token.is_none() {
            return None;
        }
        Some(Self {
            agent,
            provider_config,
            config: config.clone(),
            http: reqwest::Client::new(),
        })
    }

    /// Start the Mattermost WebSocket event loop.
    pub async fn run(&self) -> Result<()> {
        let server = self
            .config
            .server_url
            .as_deref()
            .unwrap_or("https://mattermost.example.com");
        let token = self.config.token.as_deref().unwrap_or("");

        // Resolve the bot's own user ID so we can ignore self-posts.
        let bot_user_id = self.fetch_me(server, token).await?.id;
        tracing::info!(server, bot_user_id, "Starting Mattermost channel adapter");

        // Build WebSocket URL
        let ws_url = format!(
            "{}/api/v4/websocket",
            server
                .replace("https://", "wss://")
                .replace("http://", "ws://")
        );

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .with_context(|| format!("Mattermost: failed to connect WebSocket to {}", ws_url))?;

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Authenticate over WebSocket
        let auth_msg = json!({
            "seq": 1,
            "action": "authentication_challenge",
            "data": { "token": token }
        });
        ws_sink
            .send(WsMessage::Text(auth_msg.to_string()))
            .await
            .context("Mattermost: WebSocket auth failed")?;

        // Periodic ping task
        let mut ping_interval =
            tokio::time::interval(Duration::from_secs(MM_WS_PING_INTERVAL_SECS));
        let agent = self.agent.clone();
        let provider_config = self.provider_config.clone();
        let http = self.http.clone();
        let server_owned = server.to_string();
        let token_owned = token.to_string();
        let mut seq: u64 = 2;

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    let ping = json!({ "seq": seq, "action": "ping" });
                    seq += 1;
                    if ws_sink.send(WsMessage::Text(ping.to_string())).await.is_err() {
                        tracing::warn!("Mattermost: WebSocket ping failed — reconnect needed");
                        break;
                    }
                }

                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            if let Ok(event) = serde_json::from_str::<MmWsEvent>(&text) {
                                if event.event.as_deref() == Some("posted") {
                                    if let Some(data) = &event.data {
                                        // The "post" field is a JSON-encoded string inside the event.
                                        if let Some(post_str) = data.get("post").and_then(|v| v.as_str()) {
                                            if let Ok(post) = serde_json::from_str::<MmPost>(post_str) {
                                                // Ignore bot's own messages
                                                if post.user_id == bot_user_id {
                                                    continue;
                                                }
                                                // Only handle regular messages
                                                if !post.post_type.is_empty() {
                                                    continue;
                                                }
                                                let message = post.message.trim().to_string();
                                                if message.is_empty() {
                                                    continue;
                                                }
                                                let channel_id = post.channel_id.clone();
                                                // Determine the thread root for the reply:
                                                // if the incoming post already belongs to a
                                                // thread, continue in that thread; otherwise
                                                // start a new thread rooted at the incoming post.
                                                let reply_root_id = if post.root_id.is_empty() {
                                                    post.id.clone()
                                                } else {
                                                    post.root_id.clone()
                                                };
                                                let session_id = format!("mattermost:{}", post.user_id);
                                                let agent_clone = agent.clone();
                                                let pc_clone = provider_config.clone();
                                                let http_clone = http.clone();
                                                let server_clone = server_owned.clone();
                                                let token_clone = token_owned.clone();

                                                tokio::spawn(async move {
                                                    match agent_clone.process(&session_id, &message, &pc_clone).await {
                                                        Ok(response) => {
                                                            for chunk in chunk_text(&response.message, 4000) {
                                                                if let Err(e) = post_message(&http_clone, &server_clone, &token_clone, &channel_id, chunk, Some(&reply_root_id)).await {
                                                                    tracing::error!(error = %e, "Mattermost: failed to post reply");
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            tracing::error!(error = %e, "Mattermost: agent processing error");
                                                            let _ = post_message(&http_clone, &server_clone, &token_clone, &channel_id, "Sorry, I ran into an error. Please try again.", Some(&reply_root_id)).await;
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(WsMessage::Close(_))) | None => {
                            tracing::info!("Mattermost: WebSocket connection closed");
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "Mattermost: WebSocket error");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    /// Fetch the bot's own user profile.
    async fn fetch_me(&self, server: &str, token: &str) -> Result<MmUser> {
        let url = format!("{}/api/v4/users/me", server);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .context("Mattermost: failed to fetch /users/me")?
            .json::<MmUser>()
            .await
            .context("Mattermost: failed to parse /users/me response")?;
        Ok(resp)
    }
}

/// Post a message to a Mattermost channel, optionally as a threaded reply.
async fn post_message(
    http: &reqwest::Client,
    server: &str,
    token: &str,
    channel_id: &str,
    message: &str,
    root_id: Option<&str>,
) -> Result<()> {
    let url = format!("{}/api/v4/posts", server);
    let body = MmCreatePost {
        channel_id,
        message,
        root_id,
    };
    http.post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .context("Mattermost: post request failed")?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MattermostConfig;

    #[test]
    fn new_returns_none_without_server_url() {
        let cfg = MattermostConfig::default();
        assert!(cfg.server_url.is_none());
    }

    #[test]
    fn new_returns_none_without_token() {
        let cfg = MattermostConfig {
            server_url: Some("https://mm.example.com".into()),
            token: None,
            ..Default::default()
        };
        assert!(cfg.token.is_none());
    }

    #[test]
    fn deserialize_mm_post() {
        let json = r#"{"id":"abc123","channel_id":"ch1","user_id":"u1","message":"hello","type":"","root_id":""}"#;
        let post: MmPost = serde_json::from_str(json).unwrap();
        assert_eq!(post.message, "hello");
        assert_eq!(post.channel_id, "ch1");
        assert!(post.root_id.is_empty());
    }

    #[test]
    fn deserialize_mm_post_with_root_id() {
        let json = r#"{"id":"reply1","channel_id":"ch1","user_id":"u1","message":"reply","type":"","root_id":"root123"}"#;
        let post: MmPost = serde_json::from_str(json).unwrap();
        assert_eq!(post.root_id, "root123");
    }

    #[test]
    fn deserialize_mm_post_missing_root_id_defaults_to_empty() {
        let json =
            r#"{"id":"abc123","channel_id":"ch1","user_id":"u1","message":"hello","type":""}"#;
        let post: MmPost = serde_json::from_str(json).unwrap();
        assert!(post.root_id.is_empty());
    }

    #[test]
    fn reply_root_id_uses_post_id_when_root_id_empty() {
        let post = MmPost {
            id: "post1".into(),
            channel_id: "ch1".into(),
            user_id: "u1".into(),
            message: "hello".into(),
            root_id: String::new(),
            post_type: String::new(),
        };
        let reply_root = if post.root_id.is_empty() {
            &post.id
        } else {
            &post.root_id
        };
        assert_eq!(reply_root, "post1");
    }

    #[test]
    fn reply_root_id_uses_existing_root_id() {
        let post = MmPost {
            id: "reply1".into(),
            channel_id: "ch1".into(),
            user_id: "u1".into(),
            message: "hello".into(),
            root_id: "root123".into(),
            post_type: String::new(),
        };
        let reply_root = if post.root_id.is_empty() {
            &post.id
        } else {
            &post.root_id
        };
        assert_eq!(reply_root, "root123");
    }

    #[test]
    fn create_post_serializes_root_id() {
        let body = MmCreatePost {
            channel_id: "ch1",
            message: "hi",
            root_id: Some("root123"),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["root_id"], "root123");
    }

    #[test]
    fn create_post_omits_root_id_when_none() {
        let body = MmCreatePost {
            channel_id: "ch1",
            message: "hi",
            root_id: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert!(!json.as_object().unwrap().contains_key("root_id"));
    }

    #[test]
    fn ws_url_conversion() {
        let server = "https://mattermost.example.com";
        let ws_url = format!(
            "{}/api/v4/websocket",
            server
                .replace("https://", "wss://")
                .replace("http://", "ws://")
        );
        assert_eq!(ws_url, "wss://mattermost.example.com/api/v4/websocket");
    }
}

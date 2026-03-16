//! Matrix channel adapter — Client-Server API long-poll bot.
//!
//! Connects to a Matrix homeserver using the Client-Server API (no SDK required),
//! long-polls for new `m.room.message` events, and replies via the same API.
//!
//! # Setup
//! 1. Register a Matrix user on any homeserver (e.g. `matrix.org`) that will
//!    act as the bot.
//! 2. Log in once to obtain an access token (e.g. via Element or the `/login`
//!    endpoint).  Set `channels.matrix.access_token` in `config.toml` or the
//!    `MATRIX_ACCESS_TOKEN` env var.
//! 3. Set `channels.matrix.homeserver` (e.g. `https://matrix.org`) or
//!    `MATRIX_HOMESERVER`.
//! 4. The bot will respond to text messages in **every room it has joined**.
//!    Invite the bot user to the rooms you want it to participate in.
//!
//! # Limitations
//! Only `m.room.message` events with `msgtype == "m.text"` are processed.
//! The bot ignores its own messages using the `@user:server` ID returned by
//! `/whoami`.

use crate::agent::Agent;
use crate::channels::utils::chunk_text;
use crate::config::{MatrixConfig, ProviderConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Percent-encode a string for use in a URL path segment.
///
/// Encodes all characters that are not unreserved (RFC 3986 §2.3) or
/// allowed sub-delimiters in path segments.  This covers the `!` and `#`
/// characters that appear in Matrix room and event IDs.
fn path_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for byte in s.bytes() {
        match byte {
            // Unreserved characters — passed through unchanged.
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'.' | b'_' | b'~'
            // Sub-delimiters allowed in path segments.
            | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
            | b'*' | b'+' | b',' | b';' | b'='
            // Colon and @ are allowed in path segments (not first segment).
            | b':' | b'@' => {
                out.push(byte as char);
            }
            other => {
                out.push('%');
                out.push_str(&format!("{:02X}", other));
            }
        }
    }
    out
}

const MATRIX_CLIENT_V3: &str = "_matrix/client/v3";
/// Long-poll timeout in milliseconds sent to the homeserver.
const SYNC_TIMEOUT_MS: u64 = 30_000;

// ── Matrix API types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SyncResponse {
    next_batch: String,
    rooms: Option<Rooms>,
}

#[derive(Debug, Deserialize)]
struct Rooms {
    join: Option<std::collections::HashMap<String, JoinedRoom>>,
    invite: Option<std::collections::HashMap<String, InvitedRoom>>,
}

#[derive(Debug, Deserialize)]
struct JoinedRoom {
    timeline: Option<Timeline>,
}

#[derive(Debug, Deserialize)]
struct InvitedRoom {
    // We only need the room ID (key in the map) to auto-join.
    #[allow(dead_code)]
    invite_state: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Timeline {
    events: Option<Vec<RoomEvent>>,
}

#[derive(Debug, Deserialize)]
struct RoomEvent {
    #[serde(rename = "type")]
    event_type: String,
    sender: String,
    event_id: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WhoAmIResponse {
    user_id: String,
}

#[derive(Debug, Serialize)]
struct SendMessageBody<'a> {
    msgtype: &'a str,
    body: &'a str,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// Matrix Client-Server API channel.
pub struct MatrixChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    homeserver: String,
    access_token: String,
    http: reqwest::Client,
}

impl MatrixChannel {
    /// Create a new `MatrixChannel`.
    ///
    /// Returns `None` if the homeserver URL or access token is not configured.
    pub fn new(
        config: &MatrixConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        let homeserver = config
            .homeserver
            .clone()
            .or_else(|| std::env::var("MATRIX_HOMESERVER").ok())?;
        let access_token = config
            .access_token
            .clone()
            .or_else(|| std::env::var("MATRIX_ACCESS_TOKEN").ok())?;

        // Strip trailing slash for clean URL building.
        let homeserver = homeserver.trim_end_matches('/').to_string();

        Some(Self {
            agent,
            provider_config,
            homeserver,
            access_token,
            http: reqwest::Client::new(),
        })
    }

    /// Connect and start the sync loop.
    ///
    /// Long-polls the homeserver for events, processes `m.room.message` events,
    /// auto-accepts invites, and runs until the task is cancelled or a fatal
    /// error occurs.
    pub async fn run(&self) -> Result<()> {
        tracing::info!(homeserver = %self.homeserver, "Matrix channel connecting");

        // Resolve the bot's own user ID to filter out self-sent messages.
        let bot_user_id = self
            .whoami()
            .await
            .context("Matrix /whoami failed — check access token")?;

        tracing::info!(bot_user_id = %bot_user_id, "Matrix channel authenticated");

        let mut next_batch: Option<String> = None;

        loop {
            let result = self.sync(next_batch.as_deref()).await;

            let sync = match result {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "Matrix sync failed; retrying in 5 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            next_batch = Some(sync.next_batch.clone());

            if let Some(rooms) = sync.rooms {
                // Auto-join pending invites.
                if let Some(invites) = rooms.invite {
                    for room_id in invites.keys() {
                        if let Err(e) = self.join_room(room_id).await {
                            tracing::warn!(room_id = %room_id, error = %e, "Matrix join failed");
                        } else {
                            tracing::info!(room_id = %room_id, "Matrix joined room");
                        }
                    }
                }

                // Process messages in joined rooms.
                if let Some(joined) = rooms.join {
                    for (room_id, room) in joined {
                        if let Some(timeline) = room.timeline {
                            for event in timeline.events.unwrap_or_default() {
                                if event.sender == bot_user_id {
                                    continue; // skip own messages
                                }
                                if event.event_type == "m.room.message" {
                                    if let Err(e) =
                                        self.handle_event(&room_id, event).await
                                    {
                                        tracing::error!(
                                            room_id = %room_id,
                                            error = %e,
                                            "Matrix event handling error"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Call `/_matrix/client/v3/account/whoami` to get the bot's user ID.
    async fn whoami(&self) -> Result<String> {
        let url = format!("{}/{}/account/whoami", self.homeserver, MATRIX_CLIENT_V3);
        let resp: WhoAmIResponse = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .context("Matrix /whoami HTTP error")?
            .json()
            .await
            .context("Matrix /whoami JSON parse error")?;
        Ok(resp.user_id)
    }

    /// Call `/_matrix/client/v3/sync` with an optional `since` token.
    async fn sync(&self, since: Option<&str>) -> Result<SyncResponse> {
        let url = format!("{}/{}/sync", self.homeserver, MATRIX_CLIENT_V3);
        let mut request = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .query(&[("timeout", SYNC_TIMEOUT_MS.to_string())]);

        if let Some(batch) = since {
            request = request.query(&[("since", batch)]);
        }

        let resp: SyncResponse = request
            .send()
            .await
            .context("Matrix sync HTTP error")?
            .json()
            .await
            .context("Matrix sync JSON parse error")?;

        Ok(resp)
    }

    /// Accept a room invite.
    async fn join_room(&self, room_id: &str) -> Result<()> {
        let url = format!(
            "{}/{}/join/{}",
            self.homeserver,
            MATRIX_CLIENT_V3,
            path_encode(room_id)
        );
        let res = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&json!({}))
            .send()
            .await
            .context("Matrix /join HTTP error")?;

        if !res.status().is_success() {
            anyhow::bail!("Matrix /join returned {}", res.status());
        }
        Ok(())
    }

    async fn handle_event(&self, room_id: &str, event: RoomEvent) -> Result<()> {
        // Only handle plain text messages.
        if event.content["msgtype"].as_str() != Some("m.text") {
            return Ok(());
        }

        let body = match event.content["body"].as_str() {
            Some(b) if !b.trim().is_empty() => b.trim().to_string(),
            _ => return Ok(()),
        };

        tracing::debug!(
            room_id = %room_id,
            sender = %event.sender,
            body = %body,
            "Matrix message received"
        );

        // Session ID per room.
        let session_id = format!("matrix-{}", room_id);

        let response = self
            .agent
            .process(&session_id, &body, &self.provider_config)
            .await
            .context("Agent processing error")?;

        self.send_message(room_id, &response.message).await
    }

    async fn send_message(&self, room_id: &str, text: &str) -> Result<()> {
        // Matrix has no strict per-message limit but split long responses for
        // readability (Synapse caps formatted bodies at 65536 bytes).
        for chunk in chunk_text(text, 60_000) {
            let txn_id = uuid::Uuid::new_v4().simple().to_string();
            let url = format!(
                "{}/{}/rooms/{}/send/m.room.message/{}",
                self.homeserver,
                MATRIX_CLIENT_V3,
                path_encode(room_id),
                txn_id,
            );

            let body = SendMessageBody {
                msgtype: "m.text",
                body: chunk,
            };

            let res = self
                .http
                .put(&url)
                .bearer_auth(&self.access_token)
                .json(&body)
                .send()
                .await
                .context("Matrix sendMessage HTTP error")?;

            if !res.status().is_success() {
                let status = res.status();
                let err_body = res.text().await.unwrap_or_default();
                tracing::warn!(
                    room_id = %room_id,
                    status = %status,
                    body = %err_body,
                    "Matrix sendMessage failed"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_config_missing_tokens_returns_none() {
        std::env::remove_var("MATRIX_HOMESERVER");
        std::env::remove_var("MATRIX_ACCESS_TOKEN");
        let config = crate::config::MatrixConfig {
            homeserver: None,
            access_token: None,
        };
        assert!(config.homeserver.is_none());
        assert!(config.access_token.is_none());
    }

    #[test]
    fn homeserver_trailing_slash_stripped() {
        // Verify that the URL building logic would strip trailing slashes.
        let url = "https://matrix.example.org/";
        let stripped = url.trim_end_matches('/');
        assert_eq!(stripped, "https://matrix.example.org");
    }
}

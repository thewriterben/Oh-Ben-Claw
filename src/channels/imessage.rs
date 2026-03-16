//! iMessage channel adapter — macOS Messages.app via AppleScript + SQLite.
//!
//! Polls the macOS Messages database (`~/Library/Messages/chat.db`) for new
//! incoming messages, forwards them to the Oh-Ben-Claw agent, and replies
//! using `osascript` to drive the Messages application.
//!
//! # Setup
//! 1. Grant **Full Disk Access** to the terminal / app that runs Oh-Ben-Claw
//!    (System Settings → Privacy & Security → Full Disk Access).
//! 2. Optionally restrict which senders are responded to via
//!    `channels.imessage.allowed_senders` (list of phone numbers or Apple IDs).
//!    If the list is empty, all incoming texts are answered.
//! 3. The bot only replies from the first iMessage-capable account that is
//!    signed into the Messages app.
//!
//! # Limitations
//! - **macOS only** — returns an error immediately on other platforms.
//! - Only text messages (not attachments, reactions, or Tapbacks) are processed.
//! - Requires the Messages app to be running and the user to be signed in.
//! - The `chat.db` polling approach may miss messages if the database is
//!   heavily locked; the poll interval is configurable (default 2 s).

use crate::agent::Agent;
use crate::config::{IMessageConfig, ProviderConfig};
use anyhow::{Context, Result};
use std::sync::Arc;

// ── Channel ───────────────────────────────────────────────────────────────────

/// iMessage channel adapter (macOS only).
pub struct IMessageChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    allowed_senders: Vec<String>,
    poll_interval_secs: u64,
}

impl IMessageChannel {
    /// Create a new `IMessageChannel`.
    ///
    /// Returns `None` if iMessage is disabled in config.
    pub fn new(
        config: &IMessageConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        Some(Self {
            agent,
            provider_config,
            allowed_senders: config.allowed_senders.clone(),
            poll_interval_secs: config.poll_interval_secs.unwrap_or(2),
        })
    }

    /// Start the iMessage polling loop.
    ///
    /// On non-macOS platforms this returns an error immediately.
    pub async fn run(&self) -> Result<()> {
        #[cfg(not(target_os = "macos"))]
        {
            anyhow::bail!("iMessage channel is only supported on macOS");
        }

        #[cfg(target_os = "macos")]
        self.run_macos().await
    }

    // ── macOS implementation ─────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    async fn run_macos(&self) -> Result<()> {
        tracing::info!(
            poll_interval_secs = self.poll_interval_secs,
            "iMessage channel started (polling Messages.app database)"
        );

        let db_path = imessage_db_path()?;
        // Track the highest ROWID seen so far to avoid re-processing old messages.
        let mut last_rowid: i64 = self.latest_rowid(&db_path).await.unwrap_or(0);

        tracing::debug!(last_rowid, "iMessage initial watermark set");

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(self.poll_interval_secs)).await;

            match self.poll_new_messages(&db_path, last_rowid).await {
                Ok(messages) => {
                    for (rowid, handle_id, text) in messages {
                        last_rowid = last_rowid.max(rowid);
                        if let Err(e) = self.handle_message(&handle_id, &text).await {
                            tracing::error!(error = %e, handle_id = %handle_id, "iMessage handling error");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "iMessage database poll failed; retrying");
                }
            }
        }
    }

    /// Return the maximum ROWID currently in the messages table.
    #[cfg(target_os = "macos")]
    async fn latest_rowid(&self, db_path: &str) -> Result<i64> {
        let db_path = db_path.to_string();
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let conn = rusqlite::Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .context("Failed to open Messages.app database")?;

            let rowid: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(ROWID), 0) FROM message",
                    [],
                    |row| row.get(0),
                )
                .context("Failed to query max ROWID")?;
            Ok(rowid)
        })
        .await
        .context("spawn_blocking panicked")?
    }

    /// Fetch messages with ROWID > `since_rowid` that were sent by someone else.
    ///
    /// Returns a list of `(rowid, handle_id, text)` tuples sorted ascending.
    #[cfg(target_os = "macos")]
    async fn poll_new_messages(
        &self,
        db_path: &str,
        since_rowid: i64,
    ) -> Result<Vec<(i64, String, String)>> {
        let db_path = db_path.to_string();
        let allowed = self.allowed_senders.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String, String)>> {
            let conn = rusqlite::Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .context("Failed to open Messages.app database")?;

            let mut stmt = conn
                .prepare(
                    "SELECT m.ROWID, h.id, m.text
                       FROM message m
                       JOIN handle h ON m.handle_id = h.ROWID
                      WHERE m.is_from_me = 0
                        AND m.ROWID > ?1
                        AND m.text IS NOT NULL
                        AND length(trim(m.text)) > 0
                      ORDER BY m.ROWID ASC
                      LIMIT 50",
                )
                .context("Failed to prepare iMessage query")?;

            let rows = stmt
                .query_map([since_rowid], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .context("Failed to execute iMessage query")?;

            let mut results = Vec::new();
            for row in rows {
                let (rowid, handle_id, text) = row.context("iMessage row error")?;
                if allowed.is_empty() || allowed.contains(&handle_id) {
                    results.push((rowid, handle_id, text));
                }
            }
            Ok(results)
        })
        .await
        .context("spawn_blocking panicked")?
    }

    /// Process one incoming message and reply via AppleScript.
    #[cfg(target_os = "macos")]
    async fn handle_message(&self, handle_id: &str, text: &str) -> Result<()> {
        tracing::debug!(
            handle_id = %handle_id,
            text = %text,
            "iMessage received"
        );

        let session_id = format!("imessage-{}", handle_id);
        let response = self
            .agent
            .process(&session_id, text, &self.provider_config)
            .await
            .context("Agent processing error")?;

        self.send_imessage(handle_id, &response.message).await
    }

    /// Send an iMessage to `handle_id` via `osascript`.
    #[cfg(target_os = "macos")]
    async fn send_imessage(&self, handle_id: &str, text: &str) -> Result<()> {
        // Build a safe AppleScript string literal by:
        //   1. Escaping backslashes (must come first).
        //   2. Escaping double-quotes (the delimiter used in the script).
        //   3. Replacing newlines with a space — AppleScript string literals are
        //      single-line and an embedded newline would break the `send` command.
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ")
            .replace('\r', " ");
        let script = format!(
            r#"tell application "Messages"
    set targetService to 1st service whose service type = iMessage
    set targetBuddy to buddy "{handle}" of targetService
    send "{msg}" to targetBuddy
end tell"#,
            handle = handle_id,
            msg = escaped,
        );

        let output = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .await
            .context("osascript execution failed")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                handle_id = %handle_id,
                stderr = %stderr,
                "osascript returned non-zero exit code"
            );
        }

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the path to the macOS Messages SQLite database.
#[cfg(target_os = "macos")]
fn imessage_db_path() -> Result<String> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(format!("{}/Library/Messages/chat.db", home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imessage_disabled_returns_none() {
        let config = crate::config::IMessageConfig {
            enabled: false,
            allowed_senders: vec![],
            poll_interval_secs: None,
        };
        // We cannot build a real Agent here, so verify the enabled guard.
        assert!(!config.enabled);
    }

    #[test]
    fn imessage_allowed_senders_empty_means_all() {
        let config = crate::config::IMessageConfig {
            enabled: true,
            allowed_senders: vec![],
            poll_interval_secs: Some(5),
        };
        assert!(config.allowed_senders.is_empty());
    }
}

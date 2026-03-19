//! Oh-Ben-Claw memory subsystem — SQLite-backed conversation history.
//!
//! The memory subsystem stores conversation history in a local SQLite database
//! at `~/.oh-ben-claw/memory.db`. Each conversation is identified by a
//! `session_id`, allowing multiple parallel sessions to coexist.
//!
//! ## Additional memory backends (Phase 11 — Pycoclaw/Mimiclaw parity)
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`personality`] | SOUL.md (agent personality) + USER.md (user profile) |
//! | [`heartbeat`] | HEARTBEAT.md proactive task dispatch |
//! | [`journal`] | YYYY-MM-DD.md daily journal notes |

pub mod heartbeat;
pub mod journal;
pub mod personality;

pub use heartbeat::HeartbeatStore;
pub use journal::DailyJournal;
pub use personality::PersonalityStore;

use crate::providers::{ChatMessage, ChatRole};
use anyhow::Result;
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

// ── Data Types ───────────────────────────────────────────────────────────────

/// A single stored message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Memory Store ─────────────────────────────────────────────────────────────

/// The memory store — a SQLite-backed conversation history.
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    /// Open (or create) the memory store at the default location.
    pub fn open() -> Result<Self> {
        let path = Self::default_db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        tracing::info!("Memory store opened at {:?}", path);
        Ok(store)
    }

    /// Open an in-memory store (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Get the default database path.
    pub fn default_db_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
        Ok(dirs.data_dir().join("memory.db"))
    }

    /// Run database migrations to create the schema.
    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;

            CREATE TABLE IF NOT EXISTS sessions (
                id         TEXT PRIMARY KEY,
                title      TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS messages (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role       TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
                content    TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session_id
                ON messages (session_id, id);
            ",
        )?;
        Ok(())
    }

    // ── Session Management ────────────────────────────────────────────────────

    /// Create a new session and return its ID.
    pub fn create_session(&self, title: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title) VALUES (?1, ?2)",
            params![id, title],
        )?;
        Ok(id)
    }

    /// Get or create the "default" session.
    pub fn default_session(&self) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = 'default'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);

        if !exists {
            conn.execute(
                "INSERT INTO sessions (id, title) VALUES ('default', 'Default Session')",
                [],
            )?;
        }
        Ok("default".to_string())
    }

    /// List all sessions, most recently updated first.
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC",
        )?;
        let sessions = stmt
            .query_map([], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row
                        .get::<_, String>(2)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                    updated_at: row
                        .get::<_, String>(3)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(sessions)
    }

    // ── Message Management ────────────────────────────────────────────────────

    /// Append a message to a session.
    pub fn append_message(&self, session_id: &str, role: ChatRole, content: &str) -> Result<i64> {
        let role_str = match role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content) VALUES (?1, ?2, ?3)",
            params![session_id, role_str, content],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;
        Ok(id)
    }

    /// Load all messages for a session, in chronological order.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT role, content FROM messages WHERE session_id = ?1 ORDER BY id ASC")?;
        let messages = stmt
            .query_map(params![session_id], |row| {
                let role_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                Ok((role_str, content))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(messages
            .into_iter()
            .map(|(role_str, content)| ChatMessage {
                role: match role_str.as_str() {
                    "system" => ChatRole::System,
                    "assistant" => ChatRole::Assistant,
                    _ => ChatRole::User,
                },
                content,
            })
            .collect())
    }

    /// Load the last N messages for a session (for context window management).
    pub fn load_recent_messages(&self, session_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT role, content FROM (
                SELECT role, content, id FROM messages WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id ASC",
        )?;
        let messages = stmt
            .query_map(params![session_id, limit as i64], |row| {
                let role_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                Ok((role_str, content))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(messages
            .into_iter()
            .map(|(role_str, content)| ChatMessage {
                role: match role_str.as_str() {
                    "system" => ChatRole::System,
                    "assistant" => ChatRole::Assistant,
                    _ => ChatRole::User,
                },
                content,
            })
            .collect())
    }

    /// Delete a session and all its messages.
    ///
    /// Returns `true` if the session existed and was deleted, `false` if not found.
    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        // Messages are deleted via ON DELETE CASCADE on the foreign key
        let rows = conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        Ok(rows > 0)
    }

    /// Delete all messages in a session (clear history).
    pub fn clear_session(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Count the number of messages in a session.
    pub fn message_count(&self, session_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> MemoryStore {
        MemoryStore::open_in_memory().unwrap()
    }

    #[test]
    fn create_session_and_append_messages() {
        let store = make_store();
        let session_id = store.create_session("Test Session").unwrap();
        store
            .append_message(&session_id, ChatRole::User, "Hello!")
            .unwrap();
        store
            .append_message(&session_id, ChatRole::Assistant, "Hi there!")
            .unwrap();

        let messages = store.load_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Hello!");
        assert_eq!(messages[1].content, "Hi there!");
        assert_eq!(messages[0].role, ChatRole::User);
        assert_eq!(messages[1].role, ChatRole::Assistant);
    }

    #[test]
    fn default_session_is_idempotent() {
        let store = make_store();
        let id1 = store.default_session().unwrap();
        let id2 = store.default_session().unwrap();
        assert_eq!(id1, id2);
        assert_eq!(id1, "default");
    }

    #[test]
    fn load_recent_messages_respects_limit() {
        let store = make_store();
        let session_id = store.create_session("Test").unwrap();
        for i in 0..10 {
            store
                .append_message(&session_id, ChatRole::User, &format!("msg {}", i))
                .unwrap();
        }
        let recent = store.load_recent_messages(&session_id, 3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "msg 7");
        assert_eq!(recent[2].content, "msg 9");
    }

    #[test]
    fn clear_session_removes_messages() {
        let store = make_store();
        let session_id = store.create_session("Test").unwrap();
        store
            .append_message(&session_id, ChatRole::User, "Hello!")
            .unwrap();
        store.clear_session(&session_id).unwrap();
        let messages = store.load_messages(&session_id).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn message_count_is_accurate() {
        let store = make_store();
        let session_id = store.create_session("Test").unwrap();
        assert_eq!(store.message_count(&session_id).unwrap(), 0);
        store
            .append_message(&session_id, ChatRole::User, "Hello!")
            .unwrap();
        assert_eq!(store.message_count(&session_id).unwrap(), 1);
    }
}

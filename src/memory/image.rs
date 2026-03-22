//! Image memory — multimodal image storage and retrieval.
//!
//! Introduced as part of Oh-Ben-Claw Phase 12 (OpenClaw 3.13 parity),
//! image memory lets the agent store and later retrieve images alongside
//! descriptive text.  This mirrors the "Image Memory" feature shipped
//! in OpenClaw 3.13 (March 2026), which allows agents to accumulate visual
//! context across sessions.
//!
//! # Storage model
//!
//! Images are stored in a SQLite table (`image_memory`) with:
//!
//! - A unique UUID `id`.
//! - Base-64–encoded image data (`data`) and MIME type (`mime_type`).
//! - A human-readable `description` used for semantic search.
//! - Optional `tags` (comma-separated) for quick filtering.
//! - A `session_id` so memories can be scoped to a conversation.
//! - A `created_at` Unix timestamp.
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::memory::image::ImageMemoryStore;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let store = ImageMemoryStore::open_in_memory()?;
//! let id = store.store(
//!     "screenshot.png",
//!     "image/png",
//!     b"<raw bytes>",
//!     "Screenshot of the Oh-Ben-Claw dashboard",
//!     &["screenshot", "dashboard"],
//!     Some("session-abc"),
//! )?;
//! let results = store.search("dashboard", 5)?;
//! println!("Found {} images", results.len());
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

// ── Data types ────────────────────────────────────────────────────────────────

/// A stored image entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEntry {
    /// Unique UUID identifier.
    pub id: String,
    /// MIME type (e.g. `"image/png"`, `"image/jpeg"`).
    pub mime_type: String,
    /// Base64-encoded image data.
    pub data: String,
    /// Human-readable description (used for search).
    pub description: String,
    /// Optional tags for filtering.
    pub tags: Vec<String>,
    /// Session ID this image is associated with (empty string = global).
    pub session_id: String,
    /// Unix timestamp (seconds) when the image was stored.
    pub created_at: i64,
    /// Original file name, if known.
    pub file_name: String,
}

impl ImageEntry {
    /// Decode the stored base64 data back to raw bytes.
    pub fn decode_bytes(&self) -> Result<Vec<u8>> {
        BASE64.decode(&self.data).context("base64 decode")
    }

    /// Returns the estimated size of the image in bytes.
    pub fn estimated_bytes(&self) -> usize {
        // base64 overhead is roughly 3/4 of the encoded length
        self.data.len() * 3 / 4
    }

    /// Returns `true` if the entry carries any of the given tags
    /// (case-insensitive).
    pub fn has_any_tag(&self, tags: &[&str]) -> bool {
        let lower: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        self.tags.iter().any(|t| lower.contains(&t.to_lowercase()))
    }
}

// ── ImageMemoryStore ──────────────────────────────────────────────────────────

/// SQLite-backed store for multimodal image memories.
pub struct ImageMemoryStore {
    conn: Mutex<Connection>,
}

impl ImageMemoryStore {
    /// Open (or create) the image memory database at the given path.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Open image memory DB at {:?}", path))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init()?;
        Ok(store)
    }

    /// Open an in-memory database (useful for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Open in-memory image DB")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS image_memory (
                id          TEXT PRIMARY KEY,
                mime_type   TEXT NOT NULL,
                data        TEXT NOT NULL,
                description TEXT NOT NULL,
                tags        TEXT NOT NULL DEFAULT '',
                session_id  TEXT NOT NULL DEFAULT '',
                created_at  INTEGER NOT NULL,
                file_name   TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_im_session ON image_memory(session_id);
            CREATE INDEX IF NOT EXISTS idx_im_created ON image_memory(created_at DESC);",
        )
        .context("Initialize image_memory table")?;
        Ok(())
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Store an image and return its UUID.
    ///
    /// # Parameters
    ///
    /// - `file_name` — original file name (e.g. `"photo.jpg"`); may be empty.
    /// - `mime_type` — MIME type (e.g. `"image/jpeg"`).
    /// - `data` — raw image bytes.
    /// - `description` — human-readable description for semantic search.
    /// - `tags` — optional tags for filtering.
    /// - `session_id` — conversation session ID; pass `None` for global scope.
    pub fn store(
        &self,
        file_name: &str,
        mime_type: &str,
        data: &[u8],
        description: &str,
        tags: &[&str],
        session_id: Option<&str>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let encoded = BASE64.encode(data);
        let tags_str = tags.join(",");
        let session = session_id.unwrap_or("").to_string();
        let created_at = now_unix();

        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute(
            "INSERT INTO image_memory (id, mime_type, data, description, tags, session_id, created_at, file_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, mime_type, encoded, description, tags_str, session, created_at, file_name],
        )
        .context("Insert image into image_memory")?;

        tracing::debug!(id = %id, mime = %mime_type, bytes = data.len(), "Image stored in memory");
        Ok(id)
    }

    /// Delete an image entry by its UUID.  Returns `true` if a row was deleted.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let rows = conn
            .execute("DELETE FROM image_memory WHERE id = ?1", params![id])
            .context("Delete image from image_memory")?;
        Ok(rows > 0)
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Retrieve an image entry by its UUID.
    pub fn get(&self, id: &str) -> Result<Option<ImageEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, mime_type, data, description, tags, session_id, created_at, file_name
             FROM image_memory WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_entry(row)?))
        } else {
            Ok(None)
        }
    }

    /// Search images whose description or tags contain `query` (case-insensitive).
    ///
    /// Results are ordered by recency (newest first).  `limit` caps the result count.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ImageEntry>> {
        let pattern = format!("%{}%", query.to_lowercase());
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, mime_type, data, description, tags, session_id, created_at, file_name
             FROM image_memory
             WHERE lower(description) LIKE ?1 OR lower(tags) LIKE ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let entries = stmt
            .query_map(params![pattern, limit as i64], |row| {
                row_to_entry(row).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// List all images for a given session, ordered by recency.
    pub fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<ImageEntry>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, mime_type, data, description, tags, session_id, created_at, file_name
             FROM image_memory
             WHERE session_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let entries = stmt
            .query_map(params![session_id, limit as i64], |row| {
                row_to_entry(row).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Return the total number of stored images.
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM image_memory", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

// ── Row helper ────────────────────────────────────────────────────────────────

fn row_to_entry(row: &rusqlite::Row) -> anyhow::Result<ImageEntry> {
    let tags_str: String = row.get(4)?;
    let tags = if tags_str.is_empty() {
        vec![]
    } else {
        tags_str.split(',').map(|s| s.to_string()).collect()
    };
    Ok(ImageEntry {
        id: row.get(0)?,
        mime_type: row.get(1)?,
        data: row.get(2)?,
        description: row.get(3)?,
        tags,
        session_id: row.get(5)?,
        created_at: row.get(6)?,
        file_name: row.get(7)?,
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> ImageMemoryStore {
        ImageMemoryStore::open_in_memory().expect("open in-memory image store")
    }

    #[test]
    fn store_and_retrieve_by_id() {
        let store = open();
        let id = store
            .store(
                "photo.jpg",
                "image/jpeg",
                b"\xFF\xD8\xFF",
                "A cat sitting on a keyboard",
                &["cat", "keyboard"],
                Some("session-1"),
            )
            .unwrap();
        assert!(!id.is_empty());

        let entry = store.get(&id).unwrap().expect("entry should exist");
        assert_eq!(entry.mime_type, "image/jpeg");
        assert_eq!(entry.description, "A cat sitting on a keyboard");
        assert_eq!(entry.session_id, "session-1");
        assert_eq!(entry.tags, vec!["cat", "keyboard"]);
        assert_eq!(entry.file_name, "photo.jpg");
    }

    #[test]
    fn decode_bytes_round_trips() {
        let store = open();
        let raw = b"FAKEPNGDATA\x89PNG";
        let id = store
            .store("img.png", "image/png", raw, "fake png", &[], None)
            .unwrap();
        let entry = store.get(&id).unwrap().unwrap();
        assert_eq!(entry.decode_bytes().unwrap(), raw);
    }

    #[test]
    fn estimated_bytes_is_reasonable() {
        let store = open();
        let raw = vec![0u8; 300];
        let id = store
            .store("blank.png", "image/png", &raw, "blank image", &[], None)
            .unwrap();
        let entry = store.get(&id).unwrap().unwrap();
        // base64 of 300 bytes is 400 chars; estimated_bytes ≈ 300
        let est = entry.estimated_bytes();
        assert!(est >= 200 && est <= 400, "estimated_bytes = {est}");
    }

    #[test]
    fn count_reflects_stored_images() {
        let store = open();
        assert_eq!(store.count().unwrap(), 0);
        store
            .store("a.jpg", "image/jpeg", b"A", "img A", &[], None)
            .unwrap();
        store
            .store("b.jpg", "image/jpeg", b"B", "img B", &[], None)
            .unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn search_by_description() {
        let store = open();
        store
            .store(
                "chart.png",
                "image/png",
                b"X",
                "bar chart of sales data",
                &[],
                None,
            )
            .unwrap();
        store
            .store("photo.jpg", "image/jpeg", b"Y", "holiday photo", &[], None)
            .unwrap();

        let results = store.search("sales", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "bar chart of sales data");
    }

    #[test]
    fn search_by_tag() {
        let store = open();
        store
            .store("a.png", "image/png", b"A", "image A", &["dashboard"], None)
            .unwrap();
        store
            .store("b.png", "image/png", b"B", "image B", &["screenshot"], None)
            .unwrap();

        let results = store.search("dashboard", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_name, "a.png");
    }

    #[test]
    fn search_no_match_returns_empty() {
        let store = open();
        store
            .store("a.png", "image/png", b"A", "image A", &[], None)
            .unwrap();
        let results = store.search("nonexistent_xyz", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn list_by_session() {
        let store = open();
        store
            .store(
                "a.jpg",
                "image/jpeg",
                b"A",
                "session A image 1",
                &[],
                Some("sess-A"),
            )
            .unwrap();
        store
            .store(
                "b.jpg",
                "image/jpeg",
                b"B",
                "session A image 2",
                &[],
                Some("sess-A"),
            )
            .unwrap();
        store
            .store(
                "c.jpg",
                "image/jpeg",
                b"C",
                "session B image",
                &[],
                Some("sess-B"),
            )
            .unwrap();

        let sess_a = store.list_by_session("sess-A", 10).unwrap();
        assert_eq!(sess_a.len(), 2);

        let sess_b = store.list_by_session("sess-B", 10).unwrap();
        assert_eq!(sess_b.len(), 1);

        let sess_none = store.list_by_session("sess-C", 10).unwrap();
        assert!(sess_none.is_empty());
    }

    #[test]
    fn delete_removes_entry() {
        let store = open();
        let id = store
            .store("x.jpg", "image/jpeg", b"X", "deletable image", &[], None)
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);

        let deleted = store.delete(&id).unwrap();
        assert!(deleted);
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.get(&id).unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let store = open();
        let deleted = store.delete("nonexistent-id").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let store = open();
        let result = store.get("nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn has_any_tag_works() {
        let mut entry = ImageEntry {
            id: "x".to_string(),
            mime_type: "image/png".to_string(),
            data: String::new(),
            description: String::new(),
            tags: vec!["cat".to_string(), "kitten".to_string()],
            session_id: String::new(),
            created_at: 0,
            file_name: String::new(),
        };
        assert!(entry.has_any_tag(&["cat"]));
        assert!(entry.has_any_tag(&["KITTEN"]));
        assert!(entry.has_any_tag(&["dog", "cat"]));
        assert!(!entry.has_any_tag(&["dog"]));
        // Empty tags on entry
        entry.tags.clear();
        assert!(!entry.has_any_tag(&["cat"]));
    }

    #[test]
    fn store_without_session_uses_empty_string() {
        let store = open();
        let id = store
            .store("g.jpg", "image/jpeg", b"G", "global image", &[], None)
            .unwrap();
        let entry = store.get(&id).unwrap().unwrap();
        assert_eq!(entry.session_id, "");
    }

    #[test]
    fn search_is_case_insensitive() {
        let store = open();
        store
            .store(
                "a.png",
                "image/png",
                b"A",
                "Dashboard Screenshot",
                &[],
                None,
            )
            .unwrap();
        let results = store.search("dashboard", 10).unwrap();
        assert_eq!(results.len(), 1);
        let results = store.search("DASHBOARD", 10).unwrap();
        assert_eq!(results.len(), 1);
    }
}

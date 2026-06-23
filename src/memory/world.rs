//! World memory — a temporal model of the physical environment (Phase 18).
//!
//! Where the conversation [`MemoryStore`](super::MemoryStore) remembers what was
//! *said*, world memory remembers what is *true of the world*: the state of
//! rooms, devices, sensors, and subjects over time. Subsystem suites (vision,
//! sensing, movement) write observations here; the agent queries it to ground
//! decisions in real, time-valid state instead of stuffing raw logs into the
//! prompt.
//!
//! # Temporal model
//!
//! Each [`Fact`] carries a **valid-time** interval (`valid_from`..`valid_to`)
//! and a **transaction-time** stamp (`ingested_at`):
//! - `valid_from`/`valid_to` — when the fact was true in the world. `valid_to =
//!   None` means "still believed true now".
//! - `ingested_at` — when we recorded it.
//!
//! Writes are **non-destructive**: [`WorldMemory::observe`] never deletes; it
//! closes the entity's currently-open fact (sets its `valid_to`) and appends the
//! new one. This gives `current`/`at`/`history` queries and an auditable trail —
//! the foundation for full bitemporal as-of-transaction-time queries later.
//!
//! Observations for an entity are expected in non-decreasing `valid_from` order.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

/// A time-valid fact about an entity in the world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    /// Row id.
    pub id: i64,
    /// The thing this fact is about (e.g. `"living_room.temp"`, `"front_door.lock"`, `"subject:deer-7"`).
    pub entity: String,
    /// The fact's value (any JSON: a number, string, object…).
    pub value: Value,
    /// When the fact became true (ms since epoch).
    pub valid_from: u64,
    /// When it stopped being true; `None` = still believed true.
    pub valid_to: Option<u64>,
    /// When we recorded it (transaction time, ms since epoch).
    pub ingested_at: u64,
    /// Who reported it (node id / tool / inference).
    pub source: String,
}

/// SQLite-backed temporal store of world [`Fact`]s.
pub struct WorldMemory {
    conn: Mutex<Connection>,
}

impl WorldMemory {
    /// Open (or create) a world-memory database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;

            CREATE TABLE IF NOT EXISTS world_facts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                entity      TEXT NOT NULL,
                value_json  TEXT NOT NULL,
                valid_from  INTEGER NOT NULL,
                valid_to    INTEGER,
                ingested_at INTEGER NOT NULL,
                source      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_world_entity ON world_facts(entity);
            CREATE INDEX IF NOT EXISTS idx_world_valid ON world_facts(entity, valid_from);
            ",
        )?;
        Ok(())
    }

    /// Record a new observation about `entity`, valid from `valid_from`.
    ///
    /// Non-destructive: the entity's currently-open fact (if any) is closed by
    /// setting its `valid_to = valid_from`, and the new fact is appended open
    /// (`valid_to = None`). Returns the inserted [`Fact`].
    pub fn observe(
        &self,
        entity: &str,
        value: Value,
        valid_from: u64,
        ingested_at: u64,
        source: &str,
    ) -> Result<Fact> {
        let value_json = serde_json::to_string(&value)?;
        let conn = self.conn.lock().unwrap();

        // Close the entity's open fact, if any (only those that started at or
        // before this observation — avoids negative intervals on out-of-order data).
        conn.execute(
            "UPDATE world_facts SET valid_to = ?1
             WHERE entity = ?2 AND valid_to IS NULL AND valid_from <= ?1",
            params![valid_from as i64, entity],
        )?;

        conn.execute(
            "INSERT INTO world_facts (entity, value_json, valid_from, valid_to, ingested_at, source)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![entity, value_json, valid_from as i64, ingested_at as i64, source],
        )?;
        let id = conn.last_insert_rowid();

        Ok(Fact {
            id,
            entity: entity.to_string(),
            value,
            valid_from,
            valid_to: None,
            ingested_at,
            source: source.to_string(),
        })
    }

    fn row_to_fact(
        id: i64,
        entity: String,
        value_json: String,
        valid_from: i64,
        valid_to: Option<i64>,
        ingested_at: i64,
        source: String,
    ) -> Fact {
        Fact {
            id,
            entity,
            value: serde_json::from_str(&value_json).unwrap_or(Value::Null),
            valid_from: valid_from as u64,
            valid_to: valid_to.map(|v| v as u64),
            ingested_at: ingested_at as u64,
            source,
        }
    }

    fn query_one(&self, sql: &str, sql_params: impl rusqlite::Params) -> Result<Option<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query_map(sql_params, |row| {
            Ok(Self::row_to_fact(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    const COLS: &'static str =
        "id, entity, value_json, valid_from, valid_to, ingested_at, source";

    /// The currently-believed fact for `entity` (the open one), if any.
    pub fn current(&self, entity: &str) -> Result<Option<Fact>> {
        let sql = format!(
            "SELECT {} FROM world_facts WHERE entity = ?1 AND valid_to IS NULL
             ORDER BY valid_from DESC LIMIT 1",
            Self::COLS
        );
        self.query_one(&sql, params![entity])
    }

    /// The fact about `entity` that was valid at time `ts`, if any.
    pub fn at(&self, entity: &str, ts: u64) -> Result<Option<Fact>> {
        let sql = format!(
            "SELECT {} FROM world_facts
             WHERE entity = ?1 AND valid_from <= ?2 AND (valid_to IS NULL OR ?2 < valid_to)
             ORDER BY valid_from DESC LIMIT 1",
            Self::COLS
        );
        self.query_one(&sql, params![entity, ts as i64])
    }

    /// The full history of facts for `entity`, oldest first.
    pub fn history(&self, entity: &str) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM world_facts WHERE entity = ?1 ORDER BY valid_from ASC, id ASC",
            Self::COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let facts = stmt
            .query_map(params![entity], |row| {
                Ok(Self::row_to_fact(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// All distinct entities known to the store.
    pub fn entities(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT entity FROM world_facts ORDER BY entity")?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    /// Total fact count (including closed/historical facts).
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM world_facts", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn observe_and_current() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("living_room.temp", json!(21.5), 1_000, 1_000, "node-1").unwrap();
        let f = w.current("living_room.temp").unwrap().unwrap();
        assert_eq!(f.value, json!(21.5));
        assert_eq!(f.valid_to, None);
        assert_eq!(f.source, "node-1");
    }

    #[test]
    fn second_observation_closes_the_first() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("front_door.lock", json!("locked"), 1_000, 1_000, "n").unwrap();
        w.observe("front_door.lock", json!("unlocked"), 2_000, 2_000, "n").unwrap();

        // current is the latest, still open
        let cur = w.current("front_door.lock").unwrap().unwrap();
        assert_eq!(cur.value, json!("unlocked"));
        assert_eq!(cur.valid_to, None);

        // history has both; the first is now closed at 2000 (non-destructive)
        let hist = w.history("front_door.lock").unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].value, json!("locked"));
        assert_eq!(hist[0].valid_to, Some(2_000));
        assert_eq!(w.count().unwrap(), 2);
    }

    #[test]
    fn at_returns_time_correct_fact() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("room.occupied", json!(false), 0, 0, "pir").unwrap();
        w.observe("room.occupied", json!(true), 1_000, 1_000, "pir").unwrap();
        w.observe("room.occupied", json!(false), 2_000, 2_000, "pir").unwrap();

        assert_eq!(w.at("room.occupied", 500).unwrap().unwrap().value, json!(false));
        assert_eq!(w.at("room.occupied", 1_500).unwrap().unwrap().value, json!(true));
        assert_eq!(w.at("room.occupied", 2_500).unwrap().unwrap().value, json!(false));
        // a fact that starts later is not yet valid earlier
        w.observe("later.entity", json!(1), 5_000, 5_000, "s").unwrap();
        assert!(w.at("later.entity", 4_999).unwrap().is_none());
        assert!(w.at("later.entity", 5_000).unwrap().is_some());
        // unknown entity
        assert!(w.at("nope", 1_000).unwrap().is_none());
    }

    #[test]
    fn entities_lists_distinct() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("a", json!(1), 1, 1, "s").unwrap();
        w.observe("a", json!(2), 2, 2, "s").unwrap();
        w.observe("b", json!(1), 1, 1, "s").unwrap();
        let mut es = w.entities().unwrap();
        es.sort();
        assert_eq!(es, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn current_unknown_entity_is_none() {
        let w = WorldMemory::open_in_memory().unwrap();
        assert!(w.current("ghost").unwrap().is_none());
    }
}

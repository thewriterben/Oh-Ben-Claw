//! Trajectory store — experiential records of agent runs (Phase 16).
//!
//! Every agent run can be captured as an [`Episode`]: the objective, the
//! ordered tool calls it made (with results and success), and the outcome.
//! Successful episodes are the raw material the skill synthesizer
//! ([`crate::skill_forge::synthesis`]) distils into reusable, verified skills,
//! so the agent gets better at recurring tasks over time.
//!
//! Storage is a local SQLite (WAL) database, mirroring the conversation
//! `MemoryStore`. Each episode is one row; the steps are stored as a JSON blob
//! plus promoted columns (`outcome`, `ts_ms`) for querying.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

/// How an agent run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The run produced a final answer.
    Success,
    /// The run failed to complete.
    Failure,
    /// The run was aborted (e.g. max iterations, cancellation).
    Aborted,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
            Outcome::Aborted => "aborted",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "success" => Outcome::Success,
            "aborted" => Outcome::Aborted,
            _ => Outcome::Failure,
        }
    }
}

/// One tool call within an episode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeStep {
    /// Tool name.
    pub tool: String,
    /// Arguments passed to the tool.
    pub args: Value,
    /// The tool's result text.
    pub result: String,
    /// Whether the call succeeded.
    pub ok: bool,
}

/// A captured agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Unique id.
    pub id: String,
    /// Conversation session this run belonged to.
    pub session_id: String,
    /// The user objective / prompt that drove the run.
    pub objective: String,
    /// Ordered tool calls made during the run.
    pub steps: Vec<EpisodeStep>,
    /// How the run ended.
    pub outcome: Outcome,
    /// Wall-clock timestamp (ms since epoch).
    pub ts_ms: u64,
}

/// SQLite-backed store of agent [`Episode`]s.
pub struct TrajectoryStore {
    conn: Mutex<Connection>,
}

impl TrajectoryStore {
    /// Open (or create) a trajectory database at `path`.
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

            CREATE TABLE IF NOT EXISTS episodes (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL,
                objective   TEXT NOT NULL,
                steps_json  TEXT NOT NULL,
                outcome     TEXT NOT NULL,
                ts_ms       INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_episodes_ts ON episodes(ts_ms);
            CREATE INDEX IF NOT EXISTS idx_episodes_outcome ON episodes(outcome);
            ",
        )?;
        Ok(())
    }

    /// Persist an episode.
    pub fn record(&self, ep: &Episode) -> Result<()> {
        let steps_json = serde_json::to_string(&ep.steps)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO episodes (id, session_id, objective, steps_json, outcome, ts_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ep.id,
                ep.session_id,
                ep.objective,
                steps_json,
                ep.outcome.as_str(),
                ep.ts_ms as i64,
            ],
        )?;
        Ok(())
    }

    fn row_to_episode(
        id: String,
        session_id: String,
        objective: String,
        steps_json: String,
        outcome: String,
        ts_ms: i64,
    ) -> Episode {
        Episode {
            id,
            session_id,
            objective,
            steps: serde_json::from_str(&steps_json).unwrap_or_default(),
            outcome: Outcome::from_str(&outcome),
            ts_ms: ts_ms as u64,
        }
    }

    /// Fetch an episode by id.
    pub fn get(&self, id: &str) -> Result<Option<Episode>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, objective, steps_json, outcome, ts_ms FROM episodes WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Self::row_to_episode(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn query<P: rusqlite::Params>(&self, sql: &str, sql_params: P) -> Result<Vec<Episode>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let eps = stmt
            .query_map(sql_params, |row| {
                Ok(Self::row_to_episode(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(eps)
    }

    /// All successful episodes recorded at or after `since_ts_ms`, newest first.
    pub fn successful_since(&self, since_ts_ms: u64) -> Result<Vec<Episode>> {
        self.query(
            "SELECT id, session_id, objective, steps_json, outcome, ts_ms FROM episodes
             WHERE outcome = 'success' AND ts_ms >= ?1 ORDER BY ts_ms DESC",
            params![since_ts_ms as i64],
        )
    }

    /// The most recent `limit` episodes, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Episode>> {
        self.query(
            "SELECT id, session_id, objective, steps_json, outcome, ts_ms FROM episodes
             ORDER BY ts_ms DESC LIMIT ?1",
            params![limit as i64],
        )
    }

    /// Successful episodes whose objective resembles `objective` (substring
    /// match, case-insensitive) — a lightweight stand-in for semantic retrieval.
    pub fn similar(&self, objective: &str, k: usize) -> Result<Vec<Episode>> {
        let needle = format!("%{}%", objective.to_lowercase());
        self.query(
            "SELECT id, session_id, objective, steps_json, outcome, ts_ms FROM episodes
             WHERE outcome = 'success' AND lower(objective) LIKE ?1
             ORDER BY ts_ms DESC LIMIT ?2",
            params![needle, k as i64],
        )
    }

    /// Total episode count.
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ep(id: &str, objective: &str, outcome: Outcome, ts: u64) -> Episode {
        Episode {
            id: id.to_string(),
            session_id: "s1".to_string(),
            objective: objective.to_string(),
            steps: vec![EpisodeStep {
                tool: "gpio_write".to_string(),
                args: json!({"pin": 17, "value": 1}),
                result: "done".to_string(),
                ok: true,
            }],
            outcome,
            ts_ms: ts,
        }
    }

    #[test]
    fn record_and_get_roundtrip() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        let e = ep("e1", "turn on the fan", Outcome::Success, 1_000);
        s.record(&e).unwrap();
        let got = s.get("e1").unwrap().unwrap();
        assert_eq!(got.objective, "turn on the fan");
        assert_eq!(got.outcome, Outcome::Success);
        assert_eq!(got.steps.len(), 1);
        assert_eq!(got.steps[0].tool, "gpio_write");
        assert_eq!(got.steps[0].args["pin"], 17);
    }

    #[test]
    fn successful_since_filters_outcome_and_time() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("ok-old", "a", Outcome::Success, 100)).unwrap();
        s.record(&ep("ok-new", "b", Outcome::Success, 2_000)).unwrap();
        s.record(&ep("fail", "c", Outcome::Failure, 2_000)).unwrap();
        let res = s.successful_since(1_000).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, "ok-new");
    }

    #[test]
    fn similar_matches_objective_substring() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("e1", "Run the morning routine", Outcome::Success, 1)).unwrap();
        s.record(&ep("e2", "shut down for the night", Outcome::Success, 2)).unwrap();
        let hits = s.similar("morning", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "e1");
    }

    #[test]
    fn count_and_recent() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        for i in 0..5 {
            s.record(&ep(&format!("e{i}"), "x", Outcome::Success, i as u64))
                .unwrap();
        }
        assert_eq!(s.count().unwrap(), 5);
        let recent = s.recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].id, "e4"); // newest first
    }
}

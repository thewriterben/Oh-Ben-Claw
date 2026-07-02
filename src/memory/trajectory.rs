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
    /// Wall-clock duration of the run (ms), when measured (Phase 16 P4).
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Rough token estimate for the run (chars/4 heuristic), when measured.
    #[serde(default)]
    pub tokens_est: Option<u64>,
}

/// Produces vector embeddings for episode objectives (Phase 16 retrieval).
///
/// Implementations must be cheap enough for the record path or interior-
/// buffered; the built-in backend (`semantic` cargo feature) runs a local
/// ONNX model via fastembed — no network at inference time.
pub trait Embedder: Send + Sync {
    /// Embed one text into a dense vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// SQLite-backed store of agent [`Episode`]s.
pub struct TrajectoryStore {
    conn: Mutex<Connection>,
    /// Whether the FTS5 side-table is available (bundled SQLite normally: yes).
    fts: bool,
    /// Optional embedder: when set, objectives are embedded at record time and
    /// a dense leg joins the hybrid retrieval fusion.
    embedder: Option<Box<dyn Embedder>>,
}

impl TrajectoryStore {
    /// Open (or create) a trajectory database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        let mut store = Self {
            conn: Mutex::new(conn),
            fts: false,
            embedder: None,
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self {
            conn: Mutex::new(conn),
            fts: false,
            embedder: None,
        };
        store.migrate()?;
        Ok(store)
    }

    /// Attach an embedder: objectives are embedded at record time and a dense
    /// cosine leg joins the retrieval fusion. Call before sharing the store.
    pub fn with_embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    fn migrate(&mut self) -> Result<()> {
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

            CREATE TABLE IF NOT EXISTS episode_vecs (
                id  TEXT PRIMARY KEY,
                dim INTEGER NOT NULL,
                vec BLOB NOT NULL
            );
            ",
        )?;
        // Additive Phase 16 P4 columns (ignore \"duplicate column\" on re-open).
        for ddl in [
            "ALTER TABLE episodes ADD COLUMN duration_ms INTEGER",
            "ALTER TABLE episodes ADD COLUMN tokens_est INTEGER",
        ] {
            let _ = conn.execute(ddl, []);
        }
        // FTS5 lexical side-table (BM25 leg of hybrid retrieval). The bundled
        // SQLite ships FTS5; degrade gracefully if a system build doesn't.
        self.fts = conn
            .execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts
                 USING fts5(id UNINDEXED, objective)",
            )
            .is_ok();
        if !self.fts {
            tracing::warn!("SQLite FTS5 unavailable — retrieval runs without the BM25 leg");
        }
        Ok(())
    }

    /// Persist an episode (and keep the FTS/vector side-tables in sync).
    pub fn record(&self, ep: &Episode) -> Result<()> {
        let steps_json = serde_json::to_string(&ep.steps)?;
        // Embed outside the connection lock (the model call can be slow).
        let embedding = self.embedder.as_ref().and_then(|e| {
            e.embed(&ep.objective)
                .map_err(|err| tracing::warn!(error = %err, "episode embedding failed"))
                .ok()
        });
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO episodes
             (id, session_id, objective, steps_json, outcome, ts_ms, duration_ms, tokens_est)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ep.id,
                ep.session_id,
                ep.objective,
                steps_json,
                ep.outcome.as_str(),
                ep.ts_ms as i64,
                ep.duration_ms.map(|v| v as i64),
                ep.tokens_est.map(|v| v as i64),
            ],
        )?;
        if self.fts {
            let _ = conn.execute("DELETE FROM episodes_fts WHERE id = ?1", params![ep.id]);
            let _ = conn.execute(
                "INSERT INTO episodes_fts (id, objective) VALUES (?1, ?2)",
                params![ep.id, ep.objective],
            );
        }
        if let Some(vec) = embedding {
            let blob: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO episode_vecs (id, dim, vec) VALUES (?1, ?2, ?3)",
                params![ep.id, vec.len() as i64, blob],
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn row_to_episode(
        id: String,
        session_id: String,
        objective: String,
        steps_json: String,
        outcome: String,
        ts_ms: i64,
        duration_ms: Option<i64>,
        tokens_est: Option<i64>,
    ) -> Episode {
        Episode {
            id,
            session_id,
            objective,
            steps: serde_json::from_str(&steps_json).unwrap_or_default(),
            outcome: Outcome::from_str(&outcome),
            ts_ms: ts_ms as u64,
            duration_ms: duration_ms.map(|v| v as u64),
            tokens_est: tokens_est.map(|v| v as u64),
        }
    }

    const COLS: &'static str =
        "id, session_id, objective, steps_json, outcome, ts_ms, duration_ms, tokens_est";

    /// Fetch an episode by id.
    pub fn get(&self, id: &str) -> Result<Option<Episode>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM episodes WHERE id = ?1",
            Self::COLS
        ))?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Self::row_to_episode(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
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
                    row.get(6)?,
                    row.get(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(eps)
    }

    /// All successful episodes recorded at or after `since_ts_ms`, newest first.
    pub fn successful_since(&self, since_ts_ms: u64) -> Result<Vec<Episode>> {
        self.query(
            &format!(
                "SELECT {} FROM episodes
                 WHERE outcome = 'success' AND ts_ms >= ?1 ORDER BY ts_ms DESC",
                Self::COLS
            ),
            params![since_ts_ms as i64],
        )
    }

    /// The most recent `limit` episodes, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Episode>> {
        self.query(
            &format!(
                "SELECT {} FROM episodes ORDER BY ts_ms DESC LIMIT ?1",
                Self::COLS
            ),
            params![limit as i64],
        )
    }

    /// Successful episodes whose objective resembles `objective`, ranked by
    /// **hybrid retrieval with Reciprocal Rank Fusion** over up to three legs
    /// (2026 hybrid-search practice — legs are complementary, fusion is
    /// rank-based so no score normalization is needed):
    ///
    /// 1. token-overlap ([`lexical_score`], threshold 0.2) — exact anchors;
    /// 2. SQLite FTS5/BM25 (when available) — weighted lexical recall;
    /// 3. dense cosine over locally-embedded objectives (when an [`Embedder`]
    ///    is attached) — paraphrase recall.
    ///
    /// Only episodes surfaced by at least one leg are returned, best first;
    /// ties break newest-first. Fully deterministic given the same store.
    pub fn similar(&self, objective: &str, k: usize) -> Result<Vec<Episode>> {
        const MIN_SCORE: f32 = 0.2;
        const MIN_COSINE: f32 = 0.3;
        const RRF_K: f32 = 60.0;
        const LEG_DEPTH: usize = 50;

        // Bounded window of recent successes = the candidate universe.
        let candidates = self.query(
            &format!(
                "SELECT {} FROM episodes
                 WHERE outcome = 'success' ORDER BY ts_ms DESC LIMIT 1000",
                Self::COLS
            ),
            [],
        )?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let index_of: std::collections::HashMap<&str, usize> = candidates
            .iter()
            .enumerate()
            .map(|(i, ep)| (ep.id.as_str(), i))
            .collect();

        let mut legs: Vec<Vec<usize>> = Vec::new();

        // Leg 1 — token overlap (always available).
        {
            let mut scored: Vec<(f32, usize)> = candidates
                .iter()
                .enumerate()
                .filter_map(|(i, ep)| {
                    let s = lexical_score(objective, &ep.objective);
                    (s >= MIN_SCORE).then_some((s, i))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(candidates[b.1].ts_ms.cmp(&candidates[a.1].ts_ms))
            });
            legs.push(scored.into_iter().take(LEG_DEPTH).map(|(_, i)| i).collect());
        }

        // Leg 2 — FTS5/BM25 (tokens OR-joined; FTS syntax kept out of reach).
        if self.fts {
            let match_expr = tokens(objective)
                .into_iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(" OR ");
            if !match_expr.is_empty() {
                let conn = self.conn.lock().unwrap();
                let ids: Vec<String> = conn
                    .prepare(
                        "SELECT id FROM episodes_fts WHERE episodes_fts MATCH ?1
                         ORDER BY bm25(episodes_fts) LIMIT ?2",
                    )
                    .and_then(|mut stmt| {
                        stmt.query_map(params![match_expr, LEG_DEPTH as i64], |row| row.get(0))?
                            .collect::<rusqlite::Result<Vec<String>>>()
                    })
                    .unwrap_or_default();
                drop(conn);
                let leg: Vec<usize> = ids
                    .iter()
                    .filter_map(|id| index_of.get(id.as_str()).copied())
                    .collect();
                if !leg.is_empty() {
                    legs.push(leg);
                }
            }
        }

        // Leg 3 — dense cosine (only when an embedder is attached).
        if let Some(embedder) = &self.embedder {
            if let Ok(qv) = embedder.embed(objective) {
                let conn = self.conn.lock().unwrap();
                let rows: Vec<(String, Vec<u8>)> = conn
                    .prepare("SELECT id, vec FROM episode_vecs")
                    .and_then(|mut stmt| {
                        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap_or_default();
                drop(conn);
                let mut scored: Vec<(f32, usize)> = rows
                    .into_iter()
                    .filter_map(|(id, blob)| {
                        let i = *index_of.get(id.as_str())?;
                        let v: Vec<f32> = blob
                            .chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        let s = cosine(&qv, &v);
                        (s >= MIN_COSINE).then_some((s, i))
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    b.0.partial_cmp(&a.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(candidates[b.1].ts_ms.cmp(&candidates[a.1].ts_ms))
                });
                let leg: Vec<usize> =
                    scored.into_iter().take(LEG_DEPTH).map(|(_, i)| i).collect();
                if !leg.is_empty() {
                    legs.push(leg);
                }
            }
        }

        // Reciprocal Rank Fusion across whichever legs produced results.
        let mut fused: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
        for leg in &legs {
            for (rank, &i) in leg.iter().enumerate() {
                *fused.entry(i).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
            }
        }
        let mut ranked: Vec<(f32, usize)> = fused.into_iter().map(|(i, s)| (s, i)).collect();
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(candidates[b.1].ts_ms.cmp(&candidates[a.1].ts_ms))
                .then(candidates[a.1].id.cmp(&candidates[b.1].id))
        });
        Ok(ranked
            .into_iter()
            .take(k)
            .map(|(_, i)| candidates[i].clone())
            .collect())
    }

    /// Total episode count.
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Efficiency comparison for the Phase 16 metric "token/latency reduction
    /// on repeated routine tasks": successful runs that invoked at least one
    /// `learned_*` skill vs. those that did not, over the most recent 1 000
    /// successes with measurements.
    pub fn efficiency_stats(&self) -> Result<EfficiencyStats> {
        let eps = self.query(
            &format!(
                "SELECT {} FROM episodes
                 WHERE outcome = 'success' AND duration_ms IS NOT NULL
                 ORDER BY ts_ms DESC LIMIT 1000",
                Self::COLS
            ),
            [],
        )?;
        let mut with = EfficiencyBucket::default();
        let mut without = EfficiencyBucket::default();
        for ep in &eps {
            let bucket = if ep.steps.iter().any(|s| s.tool.starts_with("learned_")) {
                &mut with
            } else {
                &mut without
            };
            bucket.runs += 1;
            bucket.total_ms += ep.duration_ms.unwrap_or(0);
            bucket.total_tokens_est += ep.tokens_est.unwrap_or(0);
        }
        Ok(EfficiencyStats { with_learned: with, without_learned: without })
    }
}

/// Aggregates for one side of the [`EfficiencyStats`] comparison.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EfficiencyBucket {
    pub runs: u64,
    pub total_ms: u64,
    pub total_tokens_est: u64,
}

impl EfficiencyBucket {
    /// Mean duration per run (ms), 0 when empty.
    pub fn avg_ms(&self) -> u64 {
        self.total_ms.checked_div(self.runs).unwrap_or(0)
    }
    /// Mean estimated tokens per run, 0 when empty.
    pub fn avg_tokens_est(&self) -> u64 {
        self.total_tokens_est.checked_div(self.runs).unwrap_or(0)
    }
}

/// Runs that used a learned skill vs. runs that did not (Phase 16 metric).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EfficiencyStats {
    pub with_learned: EfficiencyBucket,
    pub without_learned: EfficiencyBucket,
}

/// English function words that carry no task meaning (kept deliberately small).
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "was", "has", "had", "its", "not", "with", "this", "that",
    "from", "into", "onto", "then", "than", "will", "can", "you", "your", "our", "all",
    "please", "some", "any",
];

/// Lowercased alphanumeric tokens of length ≥ 3, minus stopwords.
fn tokens(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3 && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// Cosine similarity between two dense vectors (0 on dimension mismatch).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Deterministic similarity between two objectives: cosine-style token overlap
/// `|A ∩ B| / sqrt(|A| · |B|)` in `[0, 1]`. Zero when either side has no tokens.
pub fn lexical_score(a: &str, b: &str) -> f32 {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let common = ta.intersection(&tb).count() as f32;
    common / ((ta.len() as f32) * (tb.len() as f32)).sqrt()
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
            duration_ms: None,
            tokens_est: None,
        }
    }

    /// Deterministic mock: known strings map to fixed vectors.
    struct MockEmbedder;
    impl Embedder for MockEmbedder {
        fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            // "door"-ish texts cluster on axis 0; "weather"-ish on axis 1.
            Ok(if text.contains("door") || text.contains("entrance") {
                vec![1.0, 0.0]
            } else if text.contains("weather") || text.contains("forecast") {
                vec![0.0, 1.0]
            } else {
                vec![0.6, 0.6]
            })
        }
    }

    #[test]
    fn dense_leg_finds_paraphrases_with_zero_token_overlap() {
        let s = TrajectoryStore::open_in_memory()
            .unwrap()
            .with_embedder(Box::new(MockEmbedder));
        s.record(&ep("d1", "unlock the entrance", Outcome::Success, 1)).unwrap();
        s.record(&ep("w1", "fetch the forecast", Outcome::Success, 2)).unwrap();

        // "open the door" shares NO ≥3-char content tokens with either
        // objective — only the dense leg can surface the right episode.
        let hits = s.similar("open the door", 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "d1", "paraphrase retrieved via embeddings");
    }

    #[test]
    fn fts_leg_matches_and_hybrid_stays_deterministic() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("a", "calibrate the pan tilt camera mount", Outcome::Success, 1)).unwrap();
        s.record(&ep("b", "water the tomato plants", Outcome::Success, 2)).unwrap();
        let first = s.similar("calibrate camera", 5).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, "a");
        // Same store, same query → identical ranking (RRF is rank-based).
        let second = s.similar("calibrate camera", 5).unwrap();
        assert_eq!(
            first.iter().map(|e| &e.id).collect::<Vec<_>>(),
            second.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fts_query_syntax_is_neutralized() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("a", "check the weather", Outcome::Success, 1)).unwrap();
        // FTS5 operators / broken quotes in the objective must not error.
        let hits = s.similar("weather\" OR (NEAR *", 5).unwrap();
        assert_eq!(hits.len(), 1, "sanitized query still matches by token");
    }

    #[test]
    fn efficiency_stats_split_learned_vs_not() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        let mut learned = ep("l1", "routine task", Outcome::Success, 1);
        learned.steps[0].tool = "learned_routine".to_string();
        learned.duration_ms = Some(100);
        learned.tokens_est = Some(50);
        s.record(&learned).unwrap();

        let mut plain = ep("p1", "routine task", Outcome::Success, 2);
        plain.duration_ms = Some(300);
        plain.tokens_est = Some(150);
        s.record(&plain).unwrap();

        // No measurement → excluded from stats entirely.
        s.record(&ep("unmeasured", "x", Outcome::Success, 3)).unwrap();

        let stats = s.efficiency_stats().unwrap();
        assert_eq!(stats.with_learned.runs, 1);
        assert_eq!(stats.with_learned.avg_ms(), 100);
        assert_eq!(stats.with_learned.avg_tokens_est(), 50);
        assert_eq!(stats.without_learned.runs, 1);
        assert_eq!(stats.without_learned.avg_ms(), 300);
    }

    #[test]
    fn duration_and_tokens_roundtrip() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        let mut e = ep("e1", "x", Outcome::Success, 1);
        e.duration_ms = Some(1234);
        e.tokens_est = Some(42);
        s.record(&e).unwrap();
        let got = s.get("e1").unwrap().unwrap();
        assert_eq!(got.duration_ms, Some(1234));
        assert_eq!(got.tokens_est, Some(42));
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
    fn similar_ranks_by_overlap_and_filters_unrelated() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("close", "check the weather in Oslo", Outcome::Success, 1)).unwrap();
        s.record(&ep("closer", "check the weather", Outcome::Success, 2)).unwrap();
        s.record(&ep("far", "unlock the front door", Outcome::Success, 3)).unwrap();
        let hits = s.similar("check the weather", 10).unwrap();
        assert_eq!(hits.len(), 2, "unrelated episode filtered out");
        assert_eq!(hits[0].id, "closer", "exact-overlap objective ranks first");
        assert_eq!(hits[1].id, "close");
    }

    #[test]
    fn similar_excludes_failures() {
        let s = TrajectoryStore::open_in_memory().unwrap();
        s.record(&ep("f", "check the weather", Outcome::Failure, 1)).unwrap();
        assert!(s.similar("check the weather", 10).unwrap().is_empty());
    }

    #[test]
    fn lexical_score_bounds_and_behavior() {
        assert!((lexical_score("check the weather", "check the weather") - 1.0).abs() < 1e-5);
        assert_eq!(lexical_score("", "check"), 0.0);
        assert_eq!(lexical_score("a b", "c d"), 0.0); // all tokens < 3 chars
        let related = lexical_score("turn on the fan", "turn off the fan");
        let unrelated = lexical_score("turn on the fan", "photograph the bird feeder");
        assert!(related > unrelated);
        assert!(unrelated < 0.2);
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

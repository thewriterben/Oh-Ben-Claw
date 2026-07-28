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

/// How a fact came to be believed — the epistemic class a consumer gates on.
///
/// Distinct from [`Fact::source`], which is a descriptive label ("which component wrote
/// this"). Source answers *who typed it*; origin answers *what kind of claim it is*, and
/// only the latter is a basis for deciding whether to act.
///
/// The two are not interchangeable, and assuming they were is what motivated this type.
/// `sensing` and `power` write with their own framework source constants even when the
/// reading arrived from an agent tool call — a trusted writer relaying untrusted content
/// — so a fact sourced `"power"` may be an assertion, not an observation. Origin must be
/// set where the content enters the system and travel with the reading.
///
/// **Not a total order.** "May I treat this as evidence about the world?" ranks
/// `Observed` above `Asserted`; "does this carry authority to act?" ranks `Instructed`
/// highest. So consumers declare the *set* of origins they accept, never a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// A sensor, radio, or driver reported it — the world said so.
    Observed,
    /// The framework computed it from other facts (health rollups, derived modes).
    Derived,
    /// An agent concluded it. True or not, it is a claim, not a reading.
    Asserted,
    /// A human said so — authoritative for intent, not evidence about the world.
    Instructed,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Observed => "observed",
            Origin::Derived => "derived",
            Origin::Asserted => "asserted",
            Origin::Instructed => "instructed",
        }
    }

    /// Parse a stored origin. Unrecognised values read as [`Origin::Asserted`] — the
    /// least-trusted class — so an unknown or corrupted label can never be mistaken for
    /// evidence. Fail-closed is the whole point of the type.
    pub fn parse(s: &str) -> Self {
        match s {
            "observed" => Origin::Observed,
            "derived" => Origin::Derived,
            "instructed" => Origin::Instructed,
            _ => Origin::Asserted,
        }
    }
}

/// The set of [`Origin`]s a consumer will act on.
///
/// A **set**, not a threshold, because trust here is not a single ordering. "May I treat
/// this as evidence about the world?" ranks `Observed` above `Asserted`; "does this carry
/// authority to act?" ranks `Instructed` near the top while leaving it useless as
/// evidence. A consumer that collapsed both questions into one level would be wrong about
/// one of them.
///
/// Every consumer that acts on world memory should hold one of these and say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginSet(u8);

impl OriginSet {
    const fn bit(o: Origin) -> u8 {
        match o {
            Origin::Observed => 1,
            Origin::Derived => 2,
            Origin::Asserted => 4,
            Origin::Instructed => 8,
        }
    }

    /// Nothing is accepted. A useful base to build from, and the safe thing to fall back
    /// to if a consumer's policy is somehow unset.
    pub const NONE: Self = Self(0);

    /// What the world reported, plus what the framework computed from it — the classes
    /// that constitute *evidence*. Excludes `Asserted` (an agent's claim) and
    /// `Instructed` (a human's intent): neither is a reading, however true it may be.
    pub const EVIDENCE: Self = Self(Self::bit(Origin::Observed) | Self::bit(Origin::Derived));

    /// Everything, including agent assertions. Appropriate for read-only surfaces that
    /// present state to a human, never for anything that actuates.
    pub const ALL: Self = Self(
        Self::bit(Origin::Observed)
            | Self::bit(Origin::Derived)
            | Self::bit(Origin::Asserted)
            | Self::bit(Origin::Instructed),
    );

    pub const fn accepts(self, o: Origin) -> bool {
        self.0 & Self::bit(o) != 0
    }

    pub const fn with(self, o: Origin) -> Self {
        Self(self.0 | Self::bit(o))
    }

    pub const fn without(self, o: Origin) -> Self {
        Self(self.0 & !Self::bit(o))
    }
}

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
    /// Who reported it (node id / tool / inference) — a descriptive label, not a trust
    /// signal. See [`Fact::origin`].
    pub source: String,
    /// What kind of claim this is. The field consumers gate on.
    pub origin: Origin,
    /// The facts this belief was computed from — Doyle's JTMS *in-list*.
    ///
    /// Three states, and the difference between the first two is the whole point:
    /// - `None` — **unknown support.** We did not record what this was computed from.
    ///   Every row written before this column existed is in this state, as is every
    ///   caller of [`WorldMemory::observe`] that has not been taught to declare its
    ///   inputs. Invalidation must never treat unknown support as *no* support: a
    ///   sweep that retracted these would empty the store.
    /// - `Some([])` — **explicitly self-standing.** A premise. Nothing upstream can
    ///   undercut it.
    /// - `Some([ids…])` — **supported by these facts.** If they all go away, this
    ///   belief is unsupported (which is not the same as false — see
    ///   [`WorldMemory::dependents`]).
    ///
    /// Note that [`Origin::Derived`] alone does *not* imply support is recorded, because
    /// `Derived` is also the conservative default for callers that have not thought
    /// about provenance at all. Origin says what kind of claim this is; `derived_from`
    /// says what it rests on. Only the latter can be walked.
    pub derived_from: Option<Vec<i64>>,
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

        // ── origin column (added 2026-07-19) ────────────────────────────────────
        // SQLite has no `ADD COLUMN IF NOT EXISTS`, so check before altering.
        let has_origin = conn
            .prepare("SELECT 1 FROM pragma_table_info('world_facts') WHERE name = 'origin'")?
            .exists([])?;
        if !has_origin {
            conn.execute_batch(
                "ALTER TABLE world_facts ADD COLUMN origin TEXT NOT NULL DEFAULT 'asserted';",
            )?;
            // Backfill by source. This is a one-time best-effort reading of history, NOT
            // the ongoing classification rule — origin is set at the write boundary from
            // here on, because a source label cannot tell you whether a trusted writer
            // was relaying agent-supplied content.
            //
            // Anything unrecognised keeps the `asserted` default: for pre-existing rows
            // we genuinely do not know, and guessing upward would launder history into
            // evidence.
            conn.execute_batch(
                "
                UPDATE world_facts SET origin = 'observed'
                    WHERE source IN ('lora-gateway', 'sensing', 'power', 'audio', 'vision',
                                     'clawcam', 'gnss', 'fusion', 'movement', 'navigation');
                UPDATE world_facts SET origin = 'derived'
                    WHERE source IN ('mesh-supervisor', 'notifier', 'system2', 'foresight',
                                     'site_anchor', 'siteplan', 'mission', 'fleet');
                UPDATE world_facts SET origin = 'asserted' WHERE source = 'agent';
                ",
            )?;
        }

        // ── derived_from column (added 2026-07-28) ─────────────────────────────
        // A JSON array of `world_facts.id` — the JTMS in-list. Deliberately NULL for
        // every existing row and NOT backfilled: unlike `origin`, support genuinely
        // cannot be guessed from a source label, and a wrong in-list is worse than an
        // absent one because invalidation would walk it. NULL means "unknown support"
        // and is inert to every sweep.
        let has_derived_from = conn
            .prepare("SELECT 1 FROM pragma_table_info('world_facts') WHERE name = 'derived_from'")?
            .exists([])?;
        if !has_derived_from {
            conn.execute_batch("ALTER TABLE world_facts ADD COLUMN derived_from TEXT;")?;
        }
        Ok(())
    }

    /// Record a new observation about `entity`, valid from `valid_from`.
    ///
    /// Non-destructive: the entity's currently-open fact (if any) is closed by
    /// setting its `valid_to = valid_from`, and the new fact is appended open
    /// (`valid_to = None`). Returns the inserted [`Fact`].
    /// Record an observation, defaulting to [`Origin::Derived`].
    ///
    /// `Derived` is the conservative default: honest for the framework components that
    /// make up most callers, and — critically — *not* `Observed`, so a caller that has
    /// not thought about provenance can never have its writes mistaken for evidence
    /// about the world. Callers that know better use [`WorldMemory::observe_as`].
    pub fn observe(
        &self,
        entity: &str,
        value: Value,
        valid_from: u64,
        ingested_at: u64,
        source: &str,
    ) -> Result<Fact> {
        self.observe_as(entity, value, valid_from, ingested_at, source, Origin::Derived)
    }

    /// Record an observation with an explicit [`Origin`].
    ///
    /// Origin must be decided where the content *enters* the system — at the boundary
    /// that knows whether this came off a wire or out of a tool call — and then travel
    /// with it. It cannot be reconstructed downstream from `source`, because a trusted
    /// component relaying agent-supplied content writes under its own source label.
    #[allow(clippy::too_many_arguments)]
    pub fn observe_as(
        &self,
        entity: &str,
        value: Value,
        valid_from: u64,
        ingested_at: u64,
        source: &str,
        origin: Origin,
    ) -> Result<Fact> {
        self.insert(entity, value, valid_from, ingested_at, source, origin, None)
    }

    /// Record a belief the framework computed, declaring what it was computed *from*.
    ///
    /// `derived_from` is the JTMS in-list: the ids of the facts this one rests on. Pass
    /// an empty slice to say "this is a premise, nothing undercuts it" — that is a
    /// different and much stronger claim than [`WorldMemory::observe`], which records no
    /// support at all.
    ///
    /// Callers already hold their inputs at the moment they compute a rollup; this is
    /// the API that stops them throwing that away. Once support is recorded, the
    /// question "what did I believe *because of* that source?" becomes a walk instead of
    /// an archaeology exercise — see [`WorldMemory::dependents`].
    #[allow(clippy::too_many_arguments)]
    pub fn observe_derived_from(
        &self,
        entity: &str,
        value: Value,
        valid_from: u64,
        ingested_at: u64,
        source: &str,
        derived_from: &[i64],
    ) -> Result<Fact> {
        self.insert(
            entity,
            value,
            valid_from,
            ingested_at,
            source,
            Origin::Derived,
            Some(derived_from.to_vec()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &self,
        entity: &str,
        value: Value,
        valid_from: u64,
        ingested_at: u64,
        source: &str,
        origin: Origin,
        derived_from: Option<Vec<i64>>,
    ) -> Result<Fact> {
        let value_json = serde_json::to_string(&value)?;
        let derived_json = match &derived_from {
            Some(ids) => Some(serde_json::to_string(ids)?),
            None => None,
        };
        let conn = self.conn.lock().unwrap();

        // Close the entity's open fact, if any (only those that started at or
        // before this observation — avoids negative intervals on out-of-order data).
        conn.execute(
            "UPDATE world_facts SET valid_to = ?1
             WHERE entity = ?2 AND valid_to IS NULL AND valid_from <= ?1",
            params![valid_from as i64, entity],
        )?;

        conn.execute(
            "INSERT INTO world_facts (entity, value_json, valid_from, valid_to, ingested_at, source, origin, derived_from)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)",
            params![
                entity,
                value_json,
                valid_from as i64,
                ingested_at as i64,
                source,
                origin.as_str(),
                derived_json
            ],
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
            origin,
            derived_from,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn row_to_fact(
        id: i64,
        entity: String,
        value_json: String,
        valid_from: i64,
        valid_to: Option<i64>,
        ingested_at: i64,
        source: String,
        origin: String,
        derived_from: Option<String>,
    ) -> Fact {
        Fact {
            id,
            entity,
            value: serde_json::from_str(&value_json).unwrap_or(Value::Null),
            valid_from: valid_from as u64,
            valid_to: valid_to.map(|v| v as u64),
            ingested_at: ingested_at as u64,
            source,
            origin: Origin::parse(&origin),
            // Unparseable support reads as unknown, never as empty. `Some([])` is the
            // assertion "nothing can undercut this"; a corrupted row must not make it.
            derived_from: derived_from.and_then(|s| serde_json::from_str::<Vec<i64>>(&s).ok()),
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
                row.get(7)?,
                row.get(8)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    const COLS: &'static str =
        "id, entity, value_json, valid_from, valid_to, ingested_at, source, origin, derived_from";

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
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// The facts whose in-list names `id` — one step of the JTMS dependency walk.
    ///
    /// Answers "what did I believe *because of* this?" for a single fact. Rows with
    /// unknown support (`derived_from IS NULL`) are invisible here by construction:
    /// they might depend on `id` and we have no way to know, so they are never claimed
    /// as dependents. That is the fail-closed direction — a sweep built on this will
    /// under-retract rather than empty the store.
    ///
    /// Returns both open and closed facts; callers filter. Not transitive: propagation
    /// is a separate decision (a fact with several supports may survive one dying), and
    /// putting that policy here would bake in `a·b` semantics for everyone.
    pub fn dependents(&self, id: i64) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        // json_each over a NULL column yields no rows, so the NULL case needs no guard.
        let sql = format!(
            "SELECT {} FROM world_facts f
             WHERE EXISTS (
                 SELECT 1 FROM json_each(f.derived_from) WHERE json_each.value = ?1
             )
             ORDER BY f.id ASC",
            Self::COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let facts = stmt
            .query_map(params![id], |row| {
                Ok(Self::row_to_fact(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// How many facts record their support at all.
    ///
    /// Returns `(with_support, total)`. Useful as a migration dial: the gap is the set
    /// of beliefs no invalidation sweep can reason about, and it only shrinks as write
    /// sites are taught to declare their inputs.
    pub fn support_coverage(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let with: i64 = conn.query_row(
            "SELECT COUNT(*) FROM world_facts WHERE derived_from IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM world_facts", [], |r| r.get(0))?;
        Ok((with as usize, total as usize))
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
    fn observe_defaults_to_derived_and_observe_as_is_explicit() {
        let w = WorldMemory::open_in_memory().unwrap();
        // A caller that has not thought about provenance must never produce evidence.
        w.observe("a.b", json!(1), 1_000, 1_000, "whoever").unwrap();
        assert_eq!(w.current("a.b").unwrap().unwrap().origin, Origin::Derived);

        w.observe_as("c.d", json!(2), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        assert_eq!(w.current("c.d").unwrap().unwrap().origin, Origin::Observed);
    }

    #[test]
    fn unknown_support_and_no_support_are_different_states() {
        // The distinction the whole column exists for. `observe` records nothing about
        // support (None); `observe_derived_from(&[])` asserts there is none (Some([])).
        // Collapsing them would make an invalidation sweep either inert or catastrophic.
        let w = WorldMemory::open_in_memory().unwrap();

        w.observe("unknown.support", json!(1), 1_000, 1_000, "legacy").unwrap();
        assert_eq!(w.current("unknown.support").unwrap().unwrap().derived_from, None);

        w.observe_derived_from("premise", json!(1), 1_000, 1_000, "rule", &[])
            .unwrap();
        assert_eq!(
            w.current("premise").unwrap().unwrap().derived_from,
            Some(vec![]),
            "an empty in-list is a claim, not an absence"
        );

        // observe_as still records nothing — an explicit origin is not a claim about support.
        w.observe_as("obs", json!(1), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        assert_eq!(w.current("obs").unwrap().unwrap().derived_from, None);
    }

    #[test]
    fn dependents_walks_back_to_the_source_that_died() {
        // The mesh.escalated_count shape: a rollup computed from two node readings.
        // When one reading's source goes away we must be able to find the rollup.
        let w = WorldMemory::open_in_memory().unwrap();
        let n1 = w
            .observe_as("mesh.n1.rssi", json!(-90), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        let n2 = w
            .observe_as("mesh.n2.rssi", json!(-70), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        let roll = w
            .observe_derived_from(
                "mesh.escalated_count",
                json!(2),
                1_100,
                1_100,
                "mesh-supervisor",
                &[n1.id, n2.id],
            )
            .unwrap();
        assert_eq!(roll.origin, Origin::Derived);

        let deps = w.dependents(n1.id).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].id, roll.id);
        assert_eq!(deps[0].entity, "mesh.escalated_count");
        // Support survives the round trip through the query, not just the insert.
        assert_eq!(deps[0].derived_from, Some(vec![n1.id, n2.id]));
        // Both supports find it — the caller decides whether one death is enough.
        assert_eq!(w.dependents(n2.id).unwrap().len(), 1);
        // Nothing rests on the rollup itself.
        assert!(w.dependents(roll.id).unwrap().is_empty());
    }

    #[test]
    fn unknown_support_is_never_claimed_as_a_dependent() {
        // Fail-closed. A row that might depend on n1 but never said so must not be
        // swept: under-retracting is recoverable, retracting the store is not.
        let w = WorldMemory::open_in_memory().unwrap();
        let n1 = w.observe("mesh.n1.rssi", json!(-90), 1_000, 1_000, "lora").unwrap();
        w.observe("probably.related", json!(true), 1_100, 1_100, "mesh-supervisor")
            .unwrap();
        assert!(w.dependents(n1.id).unwrap().is_empty());

        let (with, total) = w.support_coverage().unwrap();
        assert_eq!((with, total), (0, 2), "coverage names the blind spot");
    }

    #[test]
    fn a_corrupted_in_list_reads_as_unknown_not_as_empty() {
        // Same fail-closed rule as Origin::parse: garbage must never be promoted into
        // the stronger claim ("nothing undercuts this").
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("x", json!(1), 1_000, 1_000, "s").unwrap();
        {
            let conn = w.conn.lock().unwrap();
            conn.execute("UPDATE world_facts SET derived_from = 'not json'", [])
                .unwrap();
        }
        assert_eq!(w.current("x").unwrap().unwrap().derived_from, None);
    }

    #[test]
    fn origin_survives_the_round_trip_through_every_query() {
        // current/at/history all share one column list; a mismatch there would silently
        // shift every field by one, so check the value as well as the origin.
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as("x.y", json!({"v": 1}), 1_000, 1_000, "s1", Origin::Observed).unwrap();
        w.observe_as("x.y", json!({"v": 2}), 2_000, 2_000, "s2", Origin::Asserted).unwrap();

        let cur = w.current("x.y").unwrap().unwrap();
        assert_eq!(cur.origin, Origin::Asserted);
        assert_eq!(cur.value, json!({"v": 2}), "columns did not shift");
        assert_eq!(cur.source, "s2");

        let past = w.at("x.y", 1_500).unwrap().unwrap();
        assert_eq!(past.origin, Origin::Observed);
        assert_eq!(past.value, json!({"v": 1}));

        let hist = w.history("x.y").unwrap();
        assert_eq!(
            hist.iter().map(|f| f.origin).collect::<Vec<_>>(),
            vec![Origin::Observed, Origin::Asserted]
        );
    }

    #[test]
    fn an_unrecognised_stored_origin_reads_as_asserted() {
        // Fail-closed: a corrupted or future label must not be mistaken for evidence.
        assert_eq!(Origin::parse("observed"), Origin::Observed);
        assert_eq!(Origin::parse("derived"), Origin::Derived);
        assert_eq!(Origin::parse("instructed"), Origin::Instructed);
        assert_eq!(Origin::parse("asserted"), Origin::Asserted);
        assert_eq!(Origin::parse(""), Origin::Asserted);
        assert_eq!(Origin::parse("OBSERVED"), Origin::Asserted, "no case-insensitive uplift");
        assert_eq!(Origin::parse("trusted"), Origin::Asserted);
    }

    #[test]
    fn migration_adds_origin_to_a_pre_existing_db_and_backfills_by_source() {
        // A database written before the column existed — exactly the bench db. Build the
        // old schema by hand, insert rows, then let migrate() upgrade it in place.
        let dir = std::env::temp_dir().join(format!("obc-world-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE world_facts (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    entity TEXT NOT NULL, value_json TEXT NOT NULL,
                    valid_from INTEGER NOT NULL, valid_to INTEGER,
                    ingested_at INTEGER NOT NULL, source TEXT NOT NULL);
                 INSERT INTO world_facts (entity,value_json,valid_from,valid_to,ingested_at,source)
                 VALUES ('mesh.n1','{\"a\":1}',1,NULL,1,'lora-gateway'),
                        ('mesh.n1.health','{\"a\":2}',1,NULL,1,'mesh-supervisor'),
                        ('mesh.escalation_status','\"note\"',1,NULL,1,'agent'),
                        ('odd.thing','{\"a\":3}',1,NULL,1,'some-retired-subsystem');",
            )
            .unwrap();
        }

        let w = WorldMemory::open(&path).unwrap();
        assert_eq!(w.current("mesh.n1").unwrap().unwrap().origin, Origin::Observed);
        assert_eq!(w.current("mesh.n1.health").unwrap().unwrap().origin, Origin::Derived);
        // The phantom note from the 2026-07-17 incident, classified correctly in hindsight.
        assert_eq!(
            w.current("mesh.escalation_status").unwrap().unwrap().origin,
            Origin::Asserted
        );
        // Unknown source keeps the fail-closed default rather than guessing upward.
        assert_eq!(w.current("odd.thing").unwrap().unwrap().origin, Origin::Asserted);

        // Re-opening must not re-run the ALTER (it would error) or re-backfill.
        drop(w);
        let w2 = WorldMemory::open(&path).unwrap();
        w2.observe_as("new.fact", json!(1), 2, 2, "x", Origin::Instructed).unwrap();
        assert_eq!(w2.current("new.fact").unwrap().unwrap().origin, Origin::Instructed);
        assert_eq!(w2.current("mesh.n1").unwrap().unwrap().origin, Origin::Observed);

        drop(w2);
        let _ = std::fs::remove_file(&path);
    }

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
    fn migration_adds_derived_from_as_unknown_and_never_guesses_it() {
        // Two shapes the bench db can be in: pre-origin, and post-origin/pre-support.
        // Both must land on "unknown support" for every existing row — a backfilled
        // in-list would be walked by invalidation, so a wrong guess is worse than none.
        let dir = std::env::temp_dir().join(format!("obc-world-derived-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        for (name, extra_col) in [("pre-origin.db", ""), ("post-origin.db", ", origin TEXT NOT NULL DEFAULT 'asserted'")] {
            let path = dir.join(name);
            let _ = std::fs::remove_file(&path);
            {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch(&format!(
                    "CREATE TABLE world_facts (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        entity TEXT NOT NULL, value_json TEXT NOT NULL,
                        valid_from INTEGER NOT NULL, valid_to INTEGER,
                        ingested_at INTEGER NOT NULL, source TEXT NOT NULL{extra_col});
                     INSERT INTO world_facts (entity,value_json,valid_from,valid_to,ingested_at,source)
                     VALUES ('mesh.escalated_count','2',1,NULL,1,'mesh-supervisor');"
                ))
                .unwrap();
            }

            let w = WorldMemory::open(&path).unwrap();
            let f = w.current("mesh.escalated_count").unwrap().unwrap();
            assert_eq!(f.value, json!(2), "columns did not shift");
            assert_eq!(f.derived_from, None, "{name}: support must not be invented");
            // And so the fact that motivated all this is invisible to a sweep until the
            // supervisor is taught to declare its inputs. That is the honest state.
            assert!(w.dependents(f.id).unwrap().is_empty());
            assert_eq!(w.support_coverage().unwrap(), (0, 1));

            // Re-opening must not re-run the ALTER (it would error).
            drop(w);
            let w2 = WorldMemory::open(&path).unwrap();
            let n = w2.observe_as("s", json!(1), 2, 2, "lora", Origin::Observed).unwrap();
            w2.observe_derived_from("roll", json!(1), 3, 3, "sup", &[n.id]).unwrap();
            assert_eq!(w2.dependents(n.id).unwrap().len(), 1);
            assert_eq!(w2.support_coverage().unwrap(), (1, 3));
        }
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

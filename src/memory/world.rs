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
use rusqlite::{params, Connection, OptionalExtension};
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
    /// Why this fact stopped being believed, when it has.
    ///
    /// `None` on an open fact, and `None` on a closed one means the ordinary case: a
    /// newer observation of the same entity superseded it. Anything else is a
    /// *withdrawal* — see [`Closure`] — and the string says what withdrew it.
    ///
    /// The two look identical in the row (`valid_to` is set either way) and mean opposite
    /// things. Superseded: we know something newer. Withdrawn: we no longer have grounds
    /// for this, and nothing has replaced it.
    pub closed_reason: Option<String>,
}

/// Why a fact's valid-time interval was closed.
///
/// The variants exist to be read, not just written: an operator looking at a store where
/// half the mesh facts went quiet needs to know whether the mesh changed or whether its
/// author went away, and those are the same row shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closure {
    /// Still believed.
    Open,
    /// A newer observation of the same entity replaced it. The ordinary path, and the
    /// only one that existed before withdrawal was possible.
    Superseded,
    /// Withdrawn because its author stopped reporting. Names the source.
    SourceStopped(String),
    /// Withdrawn because something it was derived from is no longer believed — the
    /// undercutting case. Names the source whose retirement started the walk.
    Unsupported(String),
    /// Withdrawn because a declared retention policy says a belief of this kind is no
    /// longer current. Names the policy's entity prefix.
    ///
    /// Distinct from the other two on purpose. Those are consequences of the world
    /// changing; this is a consequence of a rule someone wrote, and an operator looking
    /// at a withdrawal should be able to tell "the camera went away" from "we decided
    /// notes like this stop counting after a week".
    Expired(String),
}

impl Closure {
    /// The tag stored in `closed_reason`. `Open` and `Superseded` store nothing.
    pub fn as_tag(&self) -> Option<String> {
        match self {
            Closure::Open | Closure::Superseded => None,
            Closure::SourceStopped(s) => Some(format!("source-stopped:{s}")),
            Closure::Unsupported(s) => Some(format!("unsupported:{s}")),
            Closure::Expired(p) => Some(format!("expired:{p}")),
        }
    }

    /// Read a fact's closure state.
    ///
    /// An unrecognised tag reads as `Superseded` rather than as a withdrawal: the
    /// conservative direction is to under-report withdrawals, because a spurious
    /// "we lost our grounds for this" is the kind of thing an operator acts on.
    pub fn of(fact: &Fact) -> Self {
        if fact.valid_to.is_none() {
            return Closure::Open;
        }
        match fact.closed_reason.as_deref() {
            Some(t) => match t.split_once(':') {
                Some(("source-stopped", s)) => Closure::SourceStopped(s.to_string()),
                Some(("unsupported", s)) => Closure::Unsupported(s.to_string()),
                Some(("expired", p)) => Closure::Expired(p.to_string()),
                _ => Closure::Superseded,
            },
            None => Closure::Superseded,
        }
    }

    /// Whether this closure was a withdrawal rather than a supersession.
    pub fn is_withdrawal(&self) -> bool {
        matches!(
            self,
            Closure::SourceStopped(_) | Closure::Unsupported(_) | Closure::Expired(_)
        )
    }
}

/// Whether a belief's justification still stands, asked at read time.
///
/// See [`WorldMemory::support_status`] for why supersession is evaluated lazily rather
/// than swept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// No in-list was recorded. Says nothing either way — most of the store is here, and
    /// treating it as ungrounded would condemn everything written before support was.
    Unknown,
    /// An explicitly empty in-list: a premise. Nothing upstream can undercut it.
    SelfStanding,
    /// Every fact in the in-list is still believed.
    Grounded,
    /// At least one supporting fact is no longer believed — superseded, withdrawn, or
    /// gone. Names them, because "this is stale" is much less useful than "this is stale
    /// because #4592 moved".
    Ungrounded { missing: Vec<i64> },
}

impl Support {
    /// Whether the justification has definitely failed.
    ///
    /// `Unknown` is *not* a failure: absence of a recorded in-list is absence of
    /// evidence, and the fail-closed direction here is to leave it alone.
    pub fn has_failed(&self) -> bool {
        matches!(self, Support::Ungrounded { .. })
    }
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

        // ── closed_reason column (added 2026-07-28) ────────────────────────────
        // Why an interval was closed, not just when. NULL is the ordinary case and the
        // only one that existed until now: superseded by a newer observation of the same
        // entity. A withdrawal is a different event wearing the same clothes — the row
        // looks identical, `valid_to` is set either way — and telling them apart is the
        // difference between "we know something newer" and "we no longer have grounds
        // for this". Not backfilled, for the same reason as `derived_from`: every
        // pre-existing closure genuinely was a supersession.
        let has_closed_reason = conn
            .prepare("SELECT 1 FROM pragma_table_info('world_facts') WHERE name = 'closed_reason'")?
            .exists([])?;
        if !has_closed_reason {
            conn.execute_batch("ALTER TABLE world_facts ADD COLUMN closed_reason TEXT;")?;
        }

        // Partial index over the support graph. `dependents` cannot use an index for the
        // `json_each` membership test itself, but it does not need to look at rows with
        // no in-list at all — they are excluded by definition. On a real store that is
        // almost everything: the bench store carries support on a few facts out of
        // 23,000, so restricting the scan to indexed rows is close to a 10,000× cut in
        // rows visited per walk step. That ratio matters because a sweep walks once per
        // frontier fact, at startup, before the gateway binds.
        //
        // Created after the column so a store that predates it gets both in one open.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_world_support
                 ON world_facts(id) WHERE derived_from IS NOT NULL;",
        )?;
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
            closed_reason: None,
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
        closed_reason: Option<String>,
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
            closed_reason,
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
                row.get(9)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    const COLS: &'static str = "id, entity, value_json, valid_from, valid_to, ingested_at, \
                                source, origin, derived_from, closed_reason";

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
                    row.get(9)?,
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
        // The `IS NOT NULL` predicate is not redundant with `json_each` returning no
        // rows for NULL — it is what lets SQLite use the partial index and skip the rows
        // that could never match. Removing it is silently correct and quadratically slow.
        let sql = format!(
            "SELECT {} FROM world_facts f
             WHERE f.derived_from IS NOT NULL AND EXISTS (
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
                    row.get(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// A single fact by row id, open or closed.
    pub fn fact_by_id(&self, id: i64) -> Result<Option<Fact>> {
        let sql = format!("SELECT {} FROM world_facts WHERE id = ?1", Self::COLS);
        self.query_one(&sql, params![id])
    }

    /// Every currently-believed fact written under `source`.
    pub fn open_facts_by_source(&self, source: &str) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM world_facts WHERE source = ?1 AND valid_to IS NULL ORDER BY id ASC",
            Self::COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let facts = stmt
            .query_map(params![source], |row| {
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
                    row.get(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// Currently-believed facts whose entity starts with `prefix`.
    ///
    /// `LIKE` with an escaped prefix rather than string matching in Rust: a retention
    /// policy scans this on a schedule, and pulling every open fact back to filter it
    /// here would make the cost of a narrow policy the same as a broad one.
    pub fn open_facts_with_prefix(&self, prefix: &str) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        // `_` and `%` in an entity prefix are literal, not wildcards. Without the escape
        // a policy for `mesh_` would also match `mesha`, `meshb`, and so on.
        let pattern = format!(
            "{}%",
            prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        );
        let sql = format!(
            "SELECT {} FROM world_facts
             WHERE valid_to IS NULL AND entity LIKE ?1 ESCAPE '\\'
             ORDER BY id ASC",
            Self::COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let facts = stmt
            .query_map(params![pattern], |row| {
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
                    row.get(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    /// Whether any fact for `entity` — open or closed — already carries
    /// `value_json.<field> == value`.
    ///
    /// The idempotence check for an event stream. A poll that re-reads the same source
    /// returns the same events, and re-recording them is not new evidence: it inflates
    /// counters, resets the age of a belief that has not changed, and makes a reflex
    /// keyed on freshness fire forever on data from weeks ago. The ClawCam poll did all
    /// three — 50 distinct events written 380 times each.
    ///
    /// Searches history, not just the open fact, because supersession means yesterday's
    /// event is closed by today's and would otherwise look unseen on the next poll.
    pub fn has_value_field(&self, entity: &str, field: &str, value: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM world_facts
                 WHERE entity = ?1 AND json_extract(value_json, '$.' || ?2) = ?3
                 LIMIT 1",
                params![entity, field, value],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Distinct sources with at least one currently-believed fact.
    pub fn open_sources(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source FROM world_facts WHERE valid_to IS NULL ORDER BY source",
        )?;
        let out = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// When `source` last wrote anything (max `ingested_at`), if ever.
    ///
    /// The scrape-timestamp analogue: this is what makes "has not reported since T" a
    /// question the store can answer, rather than something a caller has to remember.
    pub fn last_write_by_source(&self, source: &str) -> Result<Option<u64>> {
        let conn = self.conn.lock().unwrap();
        let ts: Option<i64> = conn.query_row(
            "SELECT MAX(ingested_at) FROM world_facts WHERE source = ?1",
            params![source],
            |r| r.get(0),
        )?;
        Ok(ts.map(|t| t as u64))
    }

    /// Stop believing fact `id` as of `at_ms`, without deleting it.
    ///
    /// Closes the valid-time interval, exactly as a superseding observation would. The
    /// row and its history remain queryable through [`WorldMemory::at`] and
    /// [`WorldMemory::history`]; only `current` stops returning it.
    ///
    /// Returns `false` if the fact does not exist or was already closed, so a caller can
    /// tell "I retracted this" from "someone else already had".
    ///
    /// `why` records *which kind* of closure this was. [`Closure::Superseded`] stores
    /// nothing, because that is what an unmarked closed row already means; a withdrawal
    /// stores a tag, so the two can be told apart afterwards. They look identical
    /// otherwise, and mean opposite things.
    pub fn close_fact(&self, id: i64, at_ms: u64, why: &Closure) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE world_facts SET valid_to = ?1, closed_reason = ?2
             WHERE id = ?3 AND valid_to IS NULL",
            params![at_ms as i64, why.as_tag(), id],
        )?;
        Ok(n > 0)
    }

    /// Whether a fact's justification still stands — evaluated now, not when it was
    /// written.
    ///
    /// # Why this is a query and not a sweep
    ///
    /// A justification fails for two different reasons, and only one of them is an event
    /// worth reacting to. Its author can stop reporting — that is rare, discrete, and
    /// [`liveness::stopped`](crate::memory::liveness::stopped) handles it eagerly. Or a
    /// supporting fact can simply be *superseded* by a newer value, which happens on
    /// every sensor tick.
    ///
    /// Propagating eagerly through supersession would be the pure JTMS reading, and it
    /// would be unworkable: every reading a fused pose was computed from is replaced
    /// seconds later, so the pose would be retracted and immediately recomputed, forever.
    /// The store would fill with churn that says nothing.
    ///
    /// So supersession is evaluated lazily. The dependent stays open and a consumer that
    /// cares asks whether it is still grounded. This is the STALE Type II case — a
    /// belief invalidated not by contradiction but by something *underneath* it having
    /// moved — and the honest position is that OBC can now answer the question rather
    /// than that it eagerly acts on it.
    pub fn support_status(&self, fact: &Fact) -> Result<Support> {
        let Some(ids) = &fact.derived_from else {
            return Ok(Support::Unknown);
        };
        if ids.is_empty() {
            return Ok(Support::SelfStanding);
        }
        let mut missing = Vec::new();
        for id in ids {
            match self.fact_by_id(*id)? {
                // Closed: superseded, or withdrawn. Either way it is not believed now,
                // and a conjunctive in-list needs every member.
                Some(f) if f.valid_to.is_some() => missing.push(*id),
                // Gone entirely. Nothing deletes facts today, so this means a store that
                // has been edited by hand — treat it as missing rather than as fine.
                None => missing.push(*id),
                Some(_) => {}
            }
        }
        if missing.is_empty() {
            Ok(Support::Grounded)
        } else {
            Ok(Support::Ungrounded { missing })
        }
    }

    /// Open facts whose justification no longer stands.
    ///
    /// The lazy counterpart to [`WorldMemory::withdrawn_since`]: these are beliefs still
    /// being served by `current` whose grounds have moved underneath them. Nothing
    /// retracts them automatically — see [`WorldMemory::support_status`] — so this is
    /// the query that makes them visible rather than silently stale.
    pub fn ungrounded(&self) -> Result<Vec<(Fact, Vec<i64>)>> {
        let sql = format!(
            "SELECT {} FROM world_facts WHERE valid_to IS NULL AND derived_from IS NOT NULL
             ORDER BY id ASC",
            Self::COLS
        );
        let open: Vec<Fact> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(&sql)?;
            let v = stmt
                .query_map([], |row| {
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
                        row.get(9)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let mut out = Vec::new();
        for f in open {
            if let Support::Ungrounded { missing } = self.support_status(&f)? {
                out.push((f, missing));
            }
        }
        Ok(out)
    }

    /// Facts withdrawn (not superseded) at or after `since_ms`, newest first.
    ///
    /// The operator's question after a source disappears: what did we stop believing,
    /// and why. Superseded rows are excluded — an entity changing value is ordinary and
    /// would bury the signal.
    pub fn withdrawn_since(&self, since_ms: u64) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM world_facts
             WHERE closed_reason IS NOT NULL AND valid_to >= ?1
             ORDER BY valid_to DESC, id DESC",
            Self::COLS
        );
        let mut stmt = conn.prepare(&sql)?;
        let facts = stmt
            .query_map(params![since_ms as i64], |row| {
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
                    row.get(9)?,
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
    fn an_unrecognised_closed_reason_reads_as_superseded() {
        // Fail-quiet in the direction that does not raise a false alarm. A spurious
        // "we lost our grounds for this" is the kind of thing an operator acts on, so a
        // tag we cannot parse degrades to the ordinary case rather than inventing a
        // withdrawal.
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("x", json!(1), 1_000, 1_000, "s").unwrap();
        w.observe("x", json!(2), 2_000, 2_000, "s").unwrap();
        {
            let conn = w.conn.lock().unwrap();
            conn.execute(
                "UPDATE world_facts SET closed_reason = 'who knows' WHERE valid_to IS NOT NULL",
                [],
            )
            .unwrap();
        }
        let hist = w.history("x").unwrap();
        assert_eq!(Closure::of(&hist[0]), Closure::Superseded);
        assert!(!Closure::of(&hist[0]).is_withdrawal());
        assert_eq!(Closure::of(&hist[1]), Closure::Open, "the live one");
        // An unparseable tag still counts as *marked*, so it stays visible to the
        // operator query rather than vanishing quietly.
        assert_eq!(w.withdrawn_since(0).unwrap().len(), 1);
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

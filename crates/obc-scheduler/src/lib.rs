//! Open Body Control — the task scheduler
//!
//! Provides cron-based and interval-based task scheduling for the agent loop.
//! Scheduled tasks are stored in SQLite and survive restarts.
//!
//! # Design
//!
//! The scheduler runs as a background tokio task. It wakes up every 30 seconds,
//! evaluates all active scheduled tasks against the current time, and dispatches
//! any due tasks to the `AgentHandle` for processing.
//!
//! # Task Types
//!
//! - **Cron tasks** — run at times matching a cron expression (6-field: sec min hr dom mon dow),
//!   read in the task's zone ([`Tz`]: UTC or the machine's local zone)
//! - **Interval tasks** — run every N seconds
//! - **One-shot tasks** — run once at a specific Unix timestamp
//!
//! A task carries a `prompt` for the agent and/or a `tool` (any registered tool,
//! learned skills included) with fixed arguments. [`nl::parse_when`] turns
//! "every weekday at 8" into a [`TaskKind`] without a model call.
//!
//! # Usage
//!
//! ```rust,no_run
//! use obc_scheduler::{Scheduler, ScheduledTask, TaskKind};
//!
//! let scheduler = Scheduler::new(":memory:").unwrap();
//!
//! // Schedule a daily briefing at 08:00
//! scheduler.add_task(ScheduledTask {
//!     id: "daily-briefing".to_string(),
//!     name: "Daily Briefing".to_string(),
//!     prompt: "Give me a brief summary of today's priorities.".to_string(),
//!     session_id: "default".to_string(),
//!     kind: TaskKind::Cron("0 0 8 * * *".to_string()),
//!     enabled: true,
//!     last_run: None,
//!     next_run: None,
//!     run_count: 0,
//!     created_at: 0,
//!     ..ScheduledTask::default()
//! }).unwrap();
//! ```

pub mod nl;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use cron::Schedule;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Zone ──────────────────────────────────────────────────────────────────────

/// The zone a cron expression or a phrase is read in. `Local` is the machine's
/// zone (what an operator means by "8"); `Utc` is what every task was before
/// 2026-09-11 and remains the storage default for old rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tz {
    #[default]
    Utc,
    Local,
}

impl Tz {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "utc" | "z" | "" => Some(Tz::Utc),
            "local" => Some(Tz::Local),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Tz::Utc => "utc",
            Tz::Local => "local",
        }
    }
    /// Wall-clock parts of a Unix timestamp in this zone.
    pub fn naive(self, ts: u64) -> Option<NaiveDateTime> {
        match self {
            Tz::Utc => Utc
                .timestamp_opt(ts as i64, 0)
                .single()
                .map(|d| d.naive_utc()),
            Tz::Local => Local
                .timestamp_opt(ts as i64, 0)
                .single()
                .map(|d| d.naive_local()),
        }
    }
    /// Unix timestamp of a wall-clock time in this zone (`None` in a DST gap).
    pub fn to_ts(self, naive: NaiveDateTime) -> Option<u64> {
        let ts = match self {
            Tz::Utc => Utc.from_local_datetime(&naive).single()?.timestamp(),
            Tz::Local => Local.from_local_datetime(&naive).earliest()?.timestamp(),
        };
        u64::try_from(ts).ok()
    }
    /// `2026-09-11 08:00 local` — for listings and tool replies.
    pub fn render(self, ts: u64) -> String {
        match self.naive(ts) {
            Some(n) => format!("{} {}", n.format("%Y-%m-%d %H:%M"), self.as_str()),
            None => format!("t={ts}"),
        }
    }
}

// ── Task Types ────────────────────────────────────────────────────────────────

/// How a scheduled task is triggered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TaskKind {
    /// Run at times matching a 6-field cron expression.
    /// Format: `sec min hr dom mon dow`
    Cron(String),
    /// Run every N seconds.
    Interval(u64),
    /// Run once at the given Unix timestamp (seconds).
    OneShot(u64),
}

impl TaskKind {
    /// Serialize to a string for storage.
    pub fn to_storage_string(&self) -> String {
        match self {
            TaskKind::Cron(expr) => format!("cron:{}", expr),
            TaskKind::Interval(secs) => format!("interval:{}", secs),
            TaskKind::OneShot(ts) => format!("oneshot:{}", ts),
        }
    }

    /// Deserialize from a storage string.
    pub fn from_storage_string(s: &str) -> Result<Self> {
        if let Some(expr) = s.strip_prefix("cron:") {
            Ok(TaskKind::Cron(expr.to_string()))
        } else if let Some(secs) = s.strip_prefix("interval:") {
            Ok(TaskKind::Interval(secs.parse()?))
        } else if let Some(ts) = s.strip_prefix("oneshot:") {
            Ok(TaskKind::OneShot(ts.parse()?))
        } else {
            anyhow::bail!("Unknown task kind format: {}", s)
        }
    }

    /// Compute the next run time after `after_ts` (Unix seconds), cron read in UTC.
    pub fn next_run_after(&self, after_ts: u64) -> Option<u64> {
        self.next_run_after_in(after_ts, Tz::Utc)
    }

    /// Compute the next run time after `after_ts` (Unix seconds), cron read in `tz`.
    pub fn next_run_after_in(&self, after_ts: u64, tz: Tz) -> Option<u64> {
        match self {
            TaskKind::Cron(expr) => {
                let schedule = Schedule::from_str(expr).ok()?;
                match tz {
                    Tz::Utc => {
                        let after: DateTime<Utc> =
                            Utc.timestamp_opt(after_ts as i64, 0).single()?;
                        schedule
                            .after(&after)
                            .next()
                            .map(|dt| dt.timestamp() as u64)
                    }
                    Tz::Local => {
                        let after: DateTime<Local> =
                            Local.timestamp_opt(after_ts as i64, 0).single()?;
                        schedule
                            .after(&after)
                            .next()
                            .map(|dt| dt.timestamp() as u64)
                    }
                }
            }
            TaskKind::Interval(secs) => Some(after_ts + secs),
            TaskKind::OneShot(ts) => {
                if *ts > after_ts {
                    Some(*ts)
                } else {
                    None // Already past
                }
            }
        }
    }

    /// Whether this kind can ever fire. A bad cron expression used to be accepted
    /// and stored with `next_run = NULL`, a task that never ran and never said why.
    pub fn validate(&self) -> std::result::Result<(), String> {
        match self {
            TaskKind::Cron(expr) => Schedule::from_str(expr).map(|_| ()).map_err(|e| {
                format!("bad cron expression '{expr}' (6 fields: sec min hour dom mon dow): {e}")
            }),
            TaskKind::Interval(secs) if *secs == 0 => Err("interval must be > 0 seconds".into()),
            TaskKind::Interval(_) => Ok(()),
            TaskKind::OneShot(ts) if *ts <= now_ts() => {
                Err(format!("one-shot time {ts} is already past"))
            }
            TaskKind::OneShot(_) => Ok(()),
        }
    }

    /// Whether this task should repeat.
    pub fn repeats(&self) -> bool {
        !matches!(self, TaskKind::OneShot(_))
    }
}

// ── Scheduled Task ────────────────────────────────────────────────────────────

/// A scheduled task record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduledTask {
    /// Unique identifier for this task.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The prompt to send to the agent when this task fires. May be empty when
    /// `tool` is set: then the tool runs on its own and its output is the result.
    pub prompt: String,
    /// The session ID to use for the agent call.
    pub session_id: String,
    /// How this task is triggered.
    #[serde(default = "default_kind")]
    pub kind: TaskKind,
    /// Whether this task is active.
    pub enabled: bool,
    /// Unix timestamp of the last run (seconds), if any.
    pub last_run: Option<u64>,
    /// Unix timestamp of the next scheduled run (seconds), if any.
    pub next_run: Option<u64>,
    /// Total number of times this task has run.
    pub run_count: u64,
    /// Unix timestamp when this task was created.
    pub created_at: u64,
    /// Zone the cron expression is read in. Rows written before 2026-09-11 are UTC.
    #[serde(default)]
    pub tz: Tz,
    /// A tool (or learned skill) to run when the task fires, before the prompt.
    #[serde(default)]
    pub tool: Option<String>,
    /// Fixed arguments for `tool`.
    #[serde(default)]
    pub tool_args: Option<serde_json::Value>,
    /// The operator's own words for the schedule, kept for listings.
    #[serde(default)]
    pub phrase: Option<String>,
}

fn default_kind() -> TaskKind {
    TaskKind::Interval(3600)
}

impl Default for TaskKind {
    fn default() -> Self {
        default_kind()
    }
}

impl ScheduledTask {
    /// Read the cron expression in `tz` from now on (recomputes `next_run`).
    pub fn with_tz(mut self, tz: Tz) -> Self {
        self.tz = tz;
        if self.kind.repeats() {
            self.next_run = self.kind.next_run_after_in(now_ts(), tz);
        }
        self
    }

    /// Run `tool` with `args` when the task fires.
    pub fn with_tool(mut self, tool: impl Into<String>, args: Option<serde_json::Value>) -> Self {
        self.tool = Some(tool.into());
        self.tool_args = args;
        self
    }

    /// Keep the phrase the schedule came from.
    pub fn with_phrase(mut self, phrase: impl Into<String>) -> Self {
        self.phrase = Some(phrase.into());
        self
    }

    /// Build from an already-parsed kind (the tool and the gateway both do this).
    pub fn from_kind(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        kind: TaskKind,
        tz: Tz,
    ) -> Self {
        let now = now_ts();
        let next_run = kind.next_run_after_in(now, tz);
        Self {
            id: id.into(),
            name: name.into(),
            prompt: prompt.into(),
            session_id: session_id.into(),
            kind,
            enabled: true,
            last_run: None,
            next_run,
            run_count: 0,
            created_at: now,
            tz,
            ..Self::default()
        }
    }

    /// Create a new cron-scheduled task.
    pub fn cron(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        cron_expr: impl Into<String>,
    ) -> Self {
        Self::from_kind(
            id,
            name,
            prompt,
            session_id,
            TaskKind::Cron(cron_expr.into()),
            Tz::Utc,
        )
    }

    /// Create a new interval-scheduled task.
    pub fn interval(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        interval_secs: u64,
    ) -> Self {
        Self::from_kind(
            id,
            name,
            prompt,
            session_id,
            TaskKind::Interval(interval_secs),
            Tz::Utc,
        )
    }

    /// Create a one-shot task that runs at a specific Unix timestamp.
    pub fn one_shot(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        run_at: u64,
    ) -> Self {
        let mut t = Self::from_kind(
            id,
            name,
            prompt,
            session_id,
            TaskKind::OneShot(run_at),
            Tz::Utc,
        );
        t.next_run = Some(run_at);
        t
    }

    /// Whether this task is due to run at the given timestamp.
    pub fn is_due(&self, now: u64) -> bool {
        if !self.enabled {
            return false;
        }
        match self.next_run {
            Some(next) => next <= now,
            None => false,
        }
    }

    /// Advance the task after a successful run.
    pub fn advance(&mut self) {
        let now = now_ts();
        self.last_run = Some(now);
        self.run_count += 1;
        if self.kind.repeats() {
            self.next_run = self.kind.next_run_after_in(now, self.tz);
        } else {
            self.next_run = None;
            self.enabled = false;
        }
    }
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

/// The Oh-Ben-Claw task scheduler.
///
/// Backed by SQLite for persistence across restarts.
pub struct Scheduler {
    conn: Arc<Mutex<Connection>>,
}

impl Scheduler {
    /// Create a new scheduler backed by the given SQLite path.
    ///
    /// Use `":memory:"` for an in-memory scheduler (tests, ephemeral use).
    pub fn new(db_path: &str) -> Result<Arc<Self>> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open scheduler database at {}", db_path))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS scheduled_tasks (
                 id          TEXT PRIMARY KEY,
                 name        TEXT NOT NULL,
                 prompt      TEXT NOT NULL,
                 session_id  TEXT NOT NULL,
                 kind        TEXT NOT NULL,
                 enabled     INTEGER NOT NULL DEFAULT 1,
                 last_run    INTEGER,
                 next_run    INTEGER,
                 run_count   INTEGER NOT NULL DEFAULT 0,
                 created_at  INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_next_run ON scheduled_tasks(next_run)
             WHERE enabled = 1;",
        )
        .context("Failed to initialize scheduler database schema")?;

        // 2026-09-11: zone, tool, tool_args, phrase. Added in place so an
        // existing scheduler.db keeps its rows (all UTC, prompt-only).
        let have: Vec<String> = conn
            .prepare("PRAGMA table_info(scheduled_tasks)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<std::result::Result<_, _>>()?;
        for (col, ddl) in [
            (
                "tz",
                "ALTER TABLE scheduled_tasks ADD COLUMN tz TEXT NOT NULL DEFAULT 'utc'",
            ),
            ("tool", "ALTER TABLE scheduled_tasks ADD COLUMN tool TEXT"),
            (
                "tool_args",
                "ALTER TABLE scheduled_tasks ADD COLUMN tool_args TEXT",
            ),
            (
                "phrase",
                "ALTER TABLE scheduled_tasks ADD COLUMN phrase TEXT",
            ),
        ] {
            if !have.iter().any(|c| c == col) {
                conn.execute_batch(ddl)
                    .with_context(|| format!("Failed to add column {col}"))?;
            }
        }

        Ok(Arc::new(Self {
            conn: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Add or replace a scheduled task.
    pub fn add_task(&self, task: ScheduledTask) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO scheduled_tasks
             (id, name, prompt, session_id, kind, enabled, last_run, next_run, run_count, created_at,
              tz, tool, tool_args, phrase)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                task.id,
                task.name,
                task.prompt,
                task.session_id,
                task.kind.to_storage_string(),
                task.enabled as i64,
                task.last_run.map(|t| t as i64),
                task.next_run.map(|t| t as i64),
                task.run_count as i64,
                task.created_at as i64,
                task.tz.as_str(),
                task.tool,
                task.tool_args.as_ref().map(|v| v.to_string()),
                task.phrase,
            ],
        )
        .context("Failed to insert scheduled task")?;
        Ok(())
    }

    /// Find a task by id, or by exact (case-insensitive) name.
    pub fn find_task(&self, id_or_name: &str) -> Result<Option<ScheduledTask>> {
        if let Some(t) = self.get_task(id_or_name)? {
            return Ok(Some(t));
        }
        let want = id_or_name.trim().to_ascii_lowercase();
        Ok(self
            .list_tasks()?
            .into_iter()
            .find(|t| t.name.trim().to_ascii_lowercase() == want))
    }

    /// Remove a scheduled task by ID.
    pub fn remove_task(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])
            .context("Failed to remove scheduled task")?;
        Ok(rows > 0)
    }

    /// Enable or disable a task.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE scheduled_tasks SET enabled = ?1 WHERE id = ?2",
                params![enabled as i64, id],
            )
            .context("Failed to update task enabled state")?;
        Ok(rows > 0)
    }

    /// Get a task by ID.
    pub fn get_task(&self, id: &str) -> Result<Option<ScheduledTask>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM scheduled_tasks WHERE id = ?1"
        ))?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_task(row)?))
        } else {
            Ok(None)
        }
    }

    /// List all tasks.
    pub fn list_tasks(&self) -> Result<Vec<ScheduledTask>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM scheduled_tasks ORDER BY created_at ASC"
        ))?;
        let tasks = stmt
            .query_map([], |row| {
                row_to_task(row).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to list scheduled tasks")?;
        Ok(tasks)
    }

    /// Return all tasks that are due to run at or before `now`.
    pub fn due_tasks(&self, now: u64) -> Result<Vec<ScheduledTask>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM scheduled_tasks
             WHERE enabled = 1 AND next_run IS NOT NULL AND next_run <= ?1
             ORDER BY next_run ASC"
        ))?;
        let tasks = stmt
            .query_map(params![now as i64], |row| {
                row_to_task(row).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query due tasks")?;
        Ok(tasks)
    }

    /// Mark a task as having run — updates last_run, run_count, next_run.
    pub fn mark_run(&self, id: &str) -> Result<Option<ScheduledTask>> {
        let mut task = match self.get_task(id)? {
            Some(t) => t,
            None => return Ok(None),
        };
        task.advance();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE scheduled_tasks
             SET last_run = ?1, next_run = ?2, run_count = ?3, enabled = ?4
             WHERE id = ?5",
            params![
                task.last_run.map(|t| t as i64),
                task.next_run.map(|t| t as i64),
                task.run_count as i64,
                task.enabled as i64,
                id,
            ],
        )
        .context("Failed to mark task as run")?;
        Ok(Some(task))
    }

    /// Total number of tasks (enabled + disabled).
    pub fn task_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM scheduled_tasks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Number of enabled tasks.
    pub fn enabled_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM scheduled_tasks WHERE enabled = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const COLUMNS: &str = "id, name, prompt, session_id, kind, enabled, last_run, next_run, \
                       run_count, created_at, tz, tool, tool_args, phrase";

fn row_to_task(row: &rusqlite::Row<'_>) -> Result<ScheduledTask> {
    let kind_str: String = row.get(4)?;
    let kind = TaskKind::from_storage_string(&kind_str)?;
    let tz: String = row.get(10)?;
    let tool_args: Option<String> = row.get(12)?;
    Ok(ScheduledTask {
        id: row.get(0)?,
        name: row.get(1)?,
        prompt: row.get(2)?,
        session_id: row.get(3)?,
        kind,
        enabled: row.get::<_, i64>(5)? != 0,
        last_run: row.get::<_, Option<i64>>(6)?.map(|t| t as u64),
        next_run: row.get::<_, Option<i64>>(7)?.map(|t| t as u64),
        run_count: row.get::<_, i64>(8)? as u64,
        created_at: row.get::<_, i64>(9)? as u64,
        tz: Tz::parse(&tz).unwrap_or_default(),
        tool: row.get(11)?,
        tool_args: tool_args.and_then(|s| serde_json::from_str(&s).ok()),
        phrase: row.get(13)?,
    })
}

// ── Scheduler Runner ──────────────────────────────────────────────────────────

/// A dispatch record for a due task.
#[derive(Debug, Clone)]
pub struct TaskDispatch {
    pub task_id: String,
    pub task_name: String,
    pub prompt: String,
    pub session_id: String,
    /// Tool (or learned skill) to run first, with its fixed arguments.
    pub tool: Option<String>,
    pub tool_args: Option<serde_json::Value>,
}

/// Run the scheduler loop, dispatching due tasks via the provided callback.
///
/// This function runs indefinitely and should be spawned as a background task.
/// The callback receives a `TaskDispatch` for each due task.
pub async fn run_scheduler_loop<F, Fut>(
    scheduler: Arc<Scheduler>,
    tick_interval_secs: u64,
    on_dispatch: F,
) where
    F: Fn(TaskDispatch) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(tick_interval_secs));

    loop {
        interval.tick().await;
        let now = now_ts();

        let due = match scheduler.due_tasks(now) {
            Ok(tasks) => tasks,
            Err(e) => {
                tracing::error!(error = %e, "Failed to query due tasks");
                continue;
            }
        };

        for task in due {
            tracing::info!(
                task_id = %task.id,
                task_name = %task.name,
                session_id = %task.session_id,
                "Dispatching scheduled task"
            );

            let dispatch = TaskDispatch {
                task_id: task.id.clone(),
                task_name: task.name.clone(),
                prompt: task.prompt.clone(),
                session_id: task.session_id.clone(),
                tool: task.tool.clone(),
                tool_args: task.tool_args.clone(),
            };

            // Advance *before* running: a slow or failing dispatch must not make
            // the next tick find the same task still due and run it twice.
            if let Err(e) = scheduler.mark_run(&task.id) {
                tracing::error!(
                    task_id = %task.id,
                    error = %e,
                    "Failed to mark task as run"
                );
            }

            on_dispatch(dispatch).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scheduler() -> Arc<Scheduler> {
        Scheduler::new(":memory:").unwrap()
    }

    #[test]
    fn add_and_get_task() {
        let sched = make_scheduler();
        let task = ScheduledTask::interval("t1", "Test", "hello", "default", 300);
        sched.add_task(task.clone()).unwrap();
        let got = sched.get_task("t1").unwrap().unwrap();
        assert_eq!(got.id, "t1");
        assert_eq!(got.name, "Test");
        assert_eq!(got.prompt, "hello");
        assert!(got.enabled);
        assert_eq!(got.run_count, 0);
    }

    #[test]
    fn remove_task() {
        let sched = make_scheduler();
        sched
            .add_task(ScheduledTask::interval("t1", "T", "p", "s", 60))
            .unwrap();
        assert!(sched.remove_task("t1").unwrap());
        assert!(sched.get_task("t1").unwrap().is_none());
        assert!(!sched.remove_task("t1").unwrap()); // Already gone
    }

    #[test]
    fn set_enabled() {
        let sched = make_scheduler();
        sched
            .add_task(ScheduledTask::interval("t1", "T", "p", "s", 60))
            .unwrap();
        sched.set_enabled("t1", false).unwrap();
        let task = sched.get_task("t1").unwrap().unwrap();
        assert!(!task.enabled);
    }

    #[test]
    fn due_tasks_returns_overdue() {
        let sched = make_scheduler();
        // Create a task with next_run in the past
        let mut task = ScheduledTask::interval("t1", "T", "p", "s", 300);
        task.next_run = Some(1_000_000); // Far in the past
        sched.add_task(task).unwrap();

        let due = sched.due_tasks(now_ts()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "t1");
    }

    #[test]
    fn due_tasks_excludes_future() {
        let sched = make_scheduler();
        let mut task = ScheduledTask::interval("t1", "T", "p", "s", 300);
        task.next_run = Some(now_ts() + 9999); // Far in the future
        sched.add_task(task).unwrap();

        let due = sched.due_tasks(now_ts()).unwrap();
        assert!(due.is_empty());
    }

    #[test]
    fn mark_run_advances_interval_task() {
        let sched = make_scheduler();
        let mut task = ScheduledTask::interval("t1", "T", "p", "s", 300);
        task.next_run = Some(1_000_000);
        sched.add_task(task).unwrap();

        let updated = sched.mark_run("t1").unwrap().unwrap();
        assert_eq!(updated.run_count, 1);
        assert!(updated.last_run.is_some());
        assert!(updated.next_run.is_some());
        assert!(updated.enabled); // Interval tasks stay enabled
    }

    #[test]
    fn mark_run_disables_one_shot() {
        let sched = make_scheduler();
        let task = ScheduledTask::one_shot("t1", "T", "p", "s", 1_000_000);
        sched.add_task(task).unwrap();

        let updated = sched.mark_run("t1").unwrap().unwrap();
        assert!(!updated.enabled); // One-shot disables after running
        assert!(updated.next_run.is_none());
    }

    #[test]
    fn task_count_and_enabled_count() {
        let sched = make_scheduler();
        sched
            .add_task(ScheduledTask::interval("t1", "T", "p", "s", 60))
            .unwrap();
        sched
            .add_task(ScheduledTask::interval("t2", "T", "p", "s", 60))
            .unwrap();
        sched.set_enabled("t2", false).unwrap();
        assert_eq!(sched.task_count().unwrap(), 2);
        assert_eq!(sched.enabled_count().unwrap(), 1);
    }

    #[test]
    fn task_kind_storage_roundtrip() {
        let kinds = vec![
            TaskKind::Cron("0 0 8 * * *".to_string()),
            TaskKind::Interval(300),
            TaskKind::OneShot(1_700_000_000),
        ];
        for kind in kinds {
            let s = kind.to_storage_string();
            let restored = TaskKind::from_storage_string(&s).unwrap();
            assert_eq!(kind, restored);
        }
    }

    #[test]
    fn task_kind_repeats() {
        assert!(TaskKind::Cron("0 0 8 * * *".to_string()).repeats());
        assert!(TaskKind::Interval(300).repeats());
        assert!(!TaskKind::OneShot(1_700_000_000).repeats());
    }

    #[test]
    fn interval_next_run_after() {
        let kind = TaskKind::Interval(300);
        let now = 1_700_000_000u64;
        assert_eq!(kind.next_run_after(now), Some(now + 300));
    }

    #[test]
    fn one_shot_next_run_after_past_returns_none() {
        let kind = TaskKind::OneShot(1_000_000);
        assert!(kind.next_run_after(2_000_000).is_none());
    }

    #[test]
    fn one_shot_next_run_after_future_returns_ts() {
        let kind = TaskKind::OneShot(2_000_000);
        assert_eq!(kind.next_run_after(1_000_000), Some(2_000_000));
    }

    #[test]
    fn scheduled_task_is_due() {
        let mut task = ScheduledTask::interval("t1", "T", "p", "s", 300);
        task.next_run = Some(1_000_000);
        assert!(task.is_due(2_000_000));
        assert!(!task.is_due(500_000));
    }

    #[test]
    fn scheduled_task_disabled_not_due() {
        let mut task = ScheduledTask::interval("t1", "T", "p", "s", 300);
        task.next_run = Some(1_000_000);
        task.enabled = false;
        assert!(!task.is_due(2_000_000));
    }

    #[test]
    fn list_tasks_returns_all() {
        let sched = make_scheduler();
        for i in 0..5 {
            sched
                .add_task(ScheduledTask::interval(format!("t{i}"), "T", "p", "s", 60))
                .unwrap();
        }
        assert_eq!(sched.list_tasks().unwrap().len(), 5);
    }
}

#[cfg(test)]
mod tests_2026_09_11 {
    use super::*;

    #[test]
    fn old_schema_gains_the_new_columns_and_keeps_rows() {
        let dir = std::env::temp_dir().join(format!("obc-sched-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE scheduled_tasks (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL, prompt TEXT NOT NULL,
                     session_id TEXT NOT NULL, kind TEXT NOT NULL,
                     enabled INTEGER NOT NULL DEFAULT 1, last_run INTEGER, next_run INTEGER,
                     run_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
                 INSERT INTO scheduled_tasks VALUES
                   ('old', 'Old task', 'say hi', 'default', 'cron:0 0 8 * * *', 1, NULL, 1, 0, 0);",
            )
            .unwrap();
        }
        let sched = Scheduler::new(path.to_str().unwrap()).unwrap();
        let old = sched.get_task("old").unwrap().unwrap();
        assert_eq!(old.tz, Tz::Utc);
        assert_eq!(old.tool, None);
        assert_eq!(old.phrase, None);
        assert_eq!(old.kind, TaskKind::Cron("0 0 8 * * *".into()));
        // and a new-style row round-trips
        let t = ScheduledTask::from_kind(
            "new",
            "Printer check",
            "check the printer",
            "scheduled-printer",
            TaskKind::Cron("0 0 8 * * Mon-Fri".into()),
            Tz::Local,
        )
        .with_tool("printer_status", Some(serde_json::json!({"verbose": true})))
        .with_phrase("every weekday at 8");
        sched.add_task(t).unwrap();
        let got = sched.find_task("printer check").unwrap().unwrap();
        assert_eq!(got.id, "new");
        assert_eq!(got.tz, Tz::Local);
        assert_eq!(got.tool.as_deref(), Some("printer_status"));
        assert_eq!(got.tool_args, Some(serde_json::json!({"verbose": true})));
        assert_eq!(got.phrase.as_deref(), Some("every weekday at 8"));
        assert!(got.next_run.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_zone_cron_fires_at_local_eight() {
        let kind = TaskKind::Cron("0 0 8 * * *".into());
        let now = now_ts();
        let next = kind.next_run_after_in(now, Tz::Local).unwrap();
        let wall = Tz::Local.naive(next).unwrap();
        assert_eq!(wall.format("%H:%M").to_string(), "08:00");
        assert!(next > now && next - now <= 86_400 + 3_600);
        let next_utc = kind.next_run_after_in(now, Tz::Utc).unwrap();
        let wall_utc = Tz::Utc.naive(next_utc).unwrap();
        assert_eq!(wall_utc.format("%H:%M").to_string(), "08:00");
    }

    #[test]
    fn validate_rejects_what_would_never_run() {
        assert!(TaskKind::Cron("0 0 8 * * Mon-Fri".into())
            .validate()
            .is_ok());
        assert!(TaskKind::Cron("0 8 * * 1-5".into()).validate().is_err());
        assert!(TaskKind::Cron("every weekday".into()).validate().is_err());
        assert!(TaskKind::Interval(0).validate().is_err());
        assert!(TaskKind::OneShot(1).validate().is_err());
        assert!(TaskKind::OneShot(now_ts() + 60).validate().is_ok());
    }

    #[tokio::test]
    async fn the_loop_dispatches_a_due_task_once_with_its_tool() {
        let sched = Scheduler::new(":memory:").unwrap();
        let mut t = ScheduledTask::one_shot("once", "Once", "", "s", now_ts() - 1);
        t = t.with_tool("device_health", Some(serde_json::json!({"node": "all"})));
        sched.add_task(t).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<TaskDispatch>(4);
        let s2 = Arc::clone(&sched);
        let h = tokio::spawn(async move {
            run_scheduler_loop(s2, 1, move |d| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(d).await;
                }
            })
            .await;
        });
        let d = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("dispatched within 5 s")
            .unwrap();
        assert_eq!(d.task_id, "once");
        assert_eq!(d.tool.as_deref(), Some("device_health"));
        assert_eq!(d.tool_args, Some(serde_json::json!({"node": "all"})));
        // one-shot: disabled after the run, so a second tick dispatches nothing
        let again = tokio::time::timeout(std::time::Duration::from_millis(2_500), rx.recv()).await;
        assert!(again.is_err(), "dispatched twice");
        let t = sched.get_task("once").unwrap().unwrap();
        assert!(!t.enabled);
        assert_eq!(t.run_count, 1);
        h.abort();
    }
}

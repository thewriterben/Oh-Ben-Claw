//! Oh-Ben-Claw Task Scheduler
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
//! - **Cron tasks** — run at times matching a cron expression (6-field: sec min hr dom mon dow)
//! - **Interval tasks** — run every N seconds
//! - **One-shot tasks** — run once at a specific Unix timestamp
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::scheduler::{Scheduler, ScheduledTask, TaskKind};
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
//! }).unwrap();
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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

    /// Compute the next run time after `after_ts` (Unix seconds).
    pub fn next_run_after(&self, after_ts: u64) -> Option<u64> {
        match self {
            TaskKind::Cron(expr) => {
                let schedule = Schedule::from_str(expr).ok()?;
                let after: DateTime<Utc> = Utc.timestamp_opt(after_ts as i64, 0).single()?;
                schedule.after(&after).next().map(|dt| dt.timestamp() as u64)
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

    /// Whether this task should repeat.
    pub fn repeats(&self) -> bool {
        !matches!(self, TaskKind::OneShot(_))
    }
}

// ── Scheduled Task ────────────────────────────────────────────────────────────

/// A scheduled task record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique identifier for this task.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The prompt to send to the agent when this task fires.
    pub prompt: String,
    /// The session ID to use for the agent call.
    pub session_id: String,
    /// How this task is triggered.
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
}

impl ScheduledTask {
    /// Create a new cron-scheduled task.
    pub fn cron(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        cron_expr: impl Into<String>,
    ) -> Self {
        let now = now_ts();
        let kind = TaskKind::Cron(cron_expr.into());
        let next_run = kind.next_run_after(now);
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
        }
    }

    /// Create a new interval-scheduled task.
    pub fn interval(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        interval_secs: u64,
    ) -> Self {
        let now = now_ts();
        let kind = TaskKind::Interval(interval_secs);
        let next_run = kind.next_run_after(now);
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
        }
    }

    /// Create a one-shot task that runs at a specific Unix timestamp.
    pub fn one_shot(
        id: impl Into<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        session_id: impl Into<String>,
        run_at: u64,
    ) -> Self {
        let now = now_ts();
        Self {
            id: id.into(),
            name: name.into(),
            prompt: prompt.into(),
            session_id: session_id.into(),
            kind: TaskKind::OneShot(run_at),
            enabled: true,
            last_run: None,
            next_run: Some(run_at),
            run_count: 0,
            created_at: now,
        }
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
            self.next_run = self.kind.next_run_after(now);
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

        Ok(Arc::new(Self {
            conn: Arc::new(Mutex::new(conn)),
        }))
    }

    /// Add or replace a scheduled task.
    pub fn add_task(&self, task: ScheduledTask) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO scheduled_tasks
             (id, name, prompt, session_id, kind, enabled, last_run, next_run, run_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            ],
        )
        .context("Failed to insert scheduled task")?;
        Ok(())
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
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, session_id, kind, enabled, last_run, next_run, run_count, created_at
             FROM scheduled_tasks WHERE id = ?1",
        )?;
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
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, session_id, kind, enabled, last_run, next_run, run_count, created_at
             FROM scheduled_tasks ORDER BY created_at ASC",
        )?;
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
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, session_id, kind, enabled, last_run, next_run, run_count, created_at
             FROM scheduled_tasks
             WHERE enabled = 1 AND next_run IS NOT NULL AND next_run <= ?1
             ORDER BY next_run ASC",
        )?;
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

fn row_to_task(row: &rusqlite::Row<'_>) -> Result<ScheduledTask> {
    let kind_str: String = row.get(4)?;
    let kind = TaskKind::from_storage_string(&kind_str)?;
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
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(tick_interval_secs));

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
            };

            on_dispatch(dispatch).await;

            if let Err(e) = scheduler.mark_run(&task.id) {
                tracing::error!(
                    task_id = %task.id,
                    error = %e,
                    "Failed to mark task as run"
                );
            }
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
                .add_task(ScheduledTask::interval(
                    format!("t{i}"),
                    "T",
                    "p",
                    "s",
                    60,
                ))
                .unwrap();
        }
        assert_eq!(sched.list_tasks().unwrap().len(), 5);
    }
}

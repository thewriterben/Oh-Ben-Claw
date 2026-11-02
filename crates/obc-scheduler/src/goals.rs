//! Standing goals (personal-agents item 4, 2026-11-02).
//!
//! A goal is a task that keeps going until it is met: "keep the greenhouse
//! between 18 and 24 °C this week", "tell me when the LoRa node's battery
//! trend says it dies within 48 h". The record lives here, next to the
//! scheduled task that ticks it; the task has the goal's id, so the scheduler
//! loop fires it like any other and the dispatch tells the two apart by the
//! [`GOAL_PREFIX`]. What a tick does (one unattended agent turn, the progress
//! note in world memory, the report to the owner) is in `obc_agent::goals`.

use crate::{now_ts, ScheduledTask, Scheduler, TaskKind, Tz};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// Prefix of a goal's id, and so of the scheduled task that ticks it.
pub const GOAL_PREFIX: &str = "goal-";

/// Whether a scheduled task is a goal's tick.
pub fn is_goal_task(task_id: &str) -> bool {
    task_id.starts_with(GOAL_PREFIX)
}

/// Where a goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    /// Still ticking.
    Active,
    /// The done condition was met.
    Done,
    /// Its window (`until`) ran out before it was met.
    Expired,
    /// Closed by the operator.
    Cancelled,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Done => "done",
            GoalStatus::Expired => "expired",
            GoalStatus::Cancelled => "cancelled",
        }
    }

    /// Unknown labels read as `Cancelled`: a row nobody can account for must
    /// not keep running unattended turns.
    pub fn parse(s: &str) -> Self {
        match s {
            "active" => GoalStatus::Active,
            "done" => GoalStatus::Done,
            "expired" => GoalStatus::Expired,
            _ => GoalStatus::Cancelled,
        }
    }
}

/// One standing goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    /// `goal-<slug>`; also the id of the task that ticks it.
    pub id: String,
    /// What to keep true or watch for, in the operator's words.
    pub text: String,
    /// When it counts as met, in words the tick turn is asked to judge.
    pub done_when: String,
    /// Where reports go: `telegram:<chat>`, `slack:<channel>`, or empty for
    /// the notifier's channels. Taken from the session that set the goal,
    /// never from a tool argument.
    pub owner: String,
    pub status: GoalStatus,
    /// The latest progress note, written by the last tick.
    pub note: Option<String>,
    /// Ticks run so far.
    pub ticks: u64,
    /// Unix seconds after which the goal closes as [`GoalStatus::Expired`].
    pub until: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Goal {
    /// A new active goal with id `goal-<slug of text>`.
    pub fn new(text: &str, done_when: &str, owner: &str) -> Self {
        let now = now_ts();
        Self {
            id: format!("{GOAL_PREFIX}{}", slug(text)),
            text: text.trim().to_string(),
            done_when: done_when.trim().to_string(),
            owner: owner.trim().to_string(),
            status: GoalStatus::Active,
            note: None,
            ticks: 0,
            until: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = if is_goal_task(id) {
            id.to_string()
        } else {
            format!("{GOAL_PREFIX}{}", slug(id))
        };
        self
    }

    pub fn with_until(mut self, until: Option<u64>) -> Self {
        self.until = until;
        self
    }

    /// The scheduled task that ticks this goal: same id, its own session.
    pub fn task(&self, kind: TaskKind, tz: Tz) -> ScheduledTask {
        let name: String = self.text.chars().take(60).collect();
        ScheduledTask::from_kind(&self.id, &name, &self.text, &self.id, kind, tz)
    }
}

pub(crate) const DDL: &str = "CREATE TABLE IF NOT EXISTS goals (
     id          TEXT PRIMARY KEY,
     text        TEXT NOT NULL,
     done_when   TEXT NOT NULL,
     owner       TEXT NOT NULL,
     status      TEXT NOT NULL,
     note        TEXT,
     ticks       INTEGER NOT NULL DEFAULT 0,
     until       INTEGER,
     created_at  INTEGER NOT NULL,
     updated_at  INTEGER NOT NULL
 );";

const COLS: &str = "id, text, done_when, owner, status, note, ticks, until, created_at, updated_at";

fn row_to_goal(row: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: row.get(0)?,
        text: row.get(1)?,
        done_when: row.get(2)?,
        owner: row.get(3)?,
        status: GoalStatus::parse(&row.get::<_, String>(4)?),
        note: row.get(5)?,
        ticks: row.get::<_, i64>(6)? as u64,
        until: row.get::<_, Option<i64>>(7)?.map(|t| t as u64),
        created_at: row.get::<_, i64>(8)? as u64,
        updated_at: row.get::<_, i64>(9)? as u64,
    })
}

fn query_goals(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Goal>> {
    let mut stmt = conn.prepare(sql)?;
    let goals = stmt
        .query_map(p, row_to_goal)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to read goals")?;
    Ok(goals)
}

impl Scheduler {
    /// Store a goal (replacing one with the same id) and the task that ticks it.
    pub fn add_goal(&self, goal: &Goal, kind: TaskKind, tz: Tz) -> Result<ScheduledTask> {
        self.add_goal_tainted(goal, kind, tz, None)
    }

    /// [`add_goal`](Self::add_goal), with the task stamped as set after
    /// reading the untrusted sources `tainted_by` (see
    /// [`ScheduledTask::tainted_by`]).
    pub fn add_goal_tainted(
        &self,
        goal: &Goal,
        kind: TaskKind,
        tz: Tz,
        tainted_by: Option<String>,
    ) -> Result<ScheduledTask> {
        kind.validate().map_err(|e| anyhow::anyhow!(e))?;
        let mut task = goal.task(kind, tz);
        task.tainted_by = tainted_by;
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                &format!("INSERT OR REPLACE INTO goals ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"),
                params![
                    goal.id,
                    goal.text,
                    goal.done_when,
                    goal.owner,
                    goal.status.as_str(),
                    goal.note,
                    goal.ticks as i64,
                    goal.until.map(|t| t as i64),
                    goal.created_at as i64,
                    goal.updated_at as i64,
                ],
            )
            .context("Failed to store goal")?;
        }
        self.add_task(task.clone())?;
        Ok(task)
    }

    pub fn get_goal(&self, id: &str) -> Result<Option<Goal>> {
        let conn = self.conn.lock().unwrap();
        Ok(query_goals(
            &conn,
            &format!("SELECT {COLS} FROM goals WHERE id = ?1"),
            params![id],
        )?
        .pop())
    }

    /// Find a goal by id, or by its text (case-insensitive).
    pub fn find_goal(&self, id_or_text: &str) -> Result<Option<Goal>> {
        let key = id_or_text.trim();
        if let Some(g) = self.get_goal(key)? {
            return Ok(Some(g));
        }
        if let Some(g) = self.get_goal(&format!("{GOAL_PREFIX}{}", slug(key)))? {
            return Ok(Some(g));
        }
        let want = key.to_lowercase();
        Ok(self
            .list_goals()?
            .into_iter()
            .find(|g| g.text.to_lowercase() == want))
    }

    /// Every goal, open ones first, newest first within each.
    pub fn list_goals(&self) -> Result<Vec<Goal>> {
        let conn = self.conn.lock().unwrap();
        query_goals(
            &conn,
            &format!(
                "SELECT {COLS} FROM goals
                 ORDER BY (status = 'active') DESC, created_at DESC"
            ),
            [],
        )
    }

    /// Record a tick's progress note. `false` when no such goal.
    pub fn record_goal_progress(&self, id: &str, note: &str, at: u64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE goals SET note = ?1, ticks = ticks + 1, updated_at = ?2 WHERE id = ?3",
                params![note, at as i64, id],
            )
            .context("Failed to record goal progress")?;
        Ok(n > 0)
    }

    /// Close a goal and stop the task that ticks it. History stays: the row,
    /// its last note, and the task (disabled) remain for listing.
    pub fn close_goal(&self, id: &str, status: GoalStatus, at: u64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE goals SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), at as i64, id],
            )
            .context("Failed to close goal")?;
        conn.execute(
            "UPDATE scheduled_tasks SET enabled = 0 WHERE id = ?1",
            params![id],
        )
        .context("Failed to stop the goal's task")?;
        Ok(n > 0)
    }
}

fn slug(text: &str) -> String {
    let mut s = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_end_matches('-').chars().take(40).collect();
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        "goal".to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_goal_and_its_task_share_an_id_and_closing_stops_the_task() {
        let s = Scheduler::new(":memory:").unwrap();
        let g = Goal::new(
            "Keep the greenhouse between 18 and 24 °C this week",
            "the week is over",
            "telegram:42",
        );
        assert_eq!(g.id, "goal-keep-the-greenhouse-between-18-and-24-c");
        let task = s.add_goal(&g, TaskKind::Interval(900), Tz::Utc).unwrap();
        assert_eq!(task.id, g.id);
        assert_eq!(task.session_id, g.id);
        assert!(s.get_task(&g.id).unwrap().unwrap().enabled);

        assert!(s.record_goal_progress(&g.id, "21 °C, fine", 100).unwrap());
        let got = s.find_goal(&g.text.to_uppercase()).unwrap().unwrap();
        assert_eq!(got.note.as_deref(), Some("21 °C, fine"));
        assert_eq!(got.ticks, 1);
        assert_eq!(got.owner, "telegram:42");

        assert!(s.close_goal(&g.id, GoalStatus::Done, 200).unwrap());
        assert_eq!(s.get_goal(&g.id).unwrap().unwrap().status, GoalStatus::Done);
        assert!(!s.get_task(&g.id).unwrap().unwrap().enabled);
        assert!(!s.close_goal("goal-nope", GoalStatus::Done, 200).unwrap());
    }

    #[test]
    fn statuses_round_trip_and_unknown_reads_as_cancelled() {
        for st in [
            GoalStatus::Active,
            GoalStatus::Done,
            GoalStatus::Expired,
            GoalStatus::Cancelled,
        ] {
            assert_eq!(GoalStatus::parse(st.as_str()), st);
        }
        assert_eq!(GoalStatus::parse("weird"), GoalStatus::Cancelled);
        assert!(is_goal_task("goal-x") && !is_goal_task("printer"));
        assert_eq!(Goal::new("", "", "").id, "goal-goal");
        assert_eq!(Goal::new("x", "", "").with_id("goal-a").id, "goal-a");
        assert_eq!(Goal::new("x", "", "").with_id("My Goal").id, "goal-my-goal");
    }
}

//! Standing goals (personal-agents item 4, 2026-11-02): what a tick does.
//!
//! The record and the task that ticks it live in `obc_scheduler::goals`; the
//! scheduler loop fires the task like any other and [`crate::scheduled::dispatch`]
//! hands it here because a goal row has its id. One tick is one unattended
//! turn, given the goal, its done condition and the notes earlier ticks left.
//! The turn ends with a progress note and two verdicts (changed? met?); the
//! note goes to world memory as `goal.<slug>.progress` (origin `Asserted`,
//! source `goal:<id>`: it is the agent's reading of the situation, not a
//! sensor's) and onto the goal row.
//!
//! The owner hears about it only when the turn says something changed, when
//! the goal is met (and closes), or when its window runs out. Owner is the
//! chat the goal was set from (`telegram:<chat>`, `slack:<channel>`); with
//! none, or when that chat cannot be reached, the report goes through the
//! [`Notifier`] like a scheduled task's.
//!
//! Gated tools inside a tick are an unattended call like any scheduled task's:
//! `Full` autonomy does not cover them, and a call that needs approval waits
//! for an answer in chat, with the prompt naming the goal.

use crate::handle::AgentHandle;
use crate::notify::Notifier;
use async_trait::async_trait;
use obc_memory::world::{Closure, Origin, WorldMemory};
use obc_scheduler::{nl, Goal, GoalStatus, Scheduler, TaskDispatch, TaskKind, Tz, GOAL_PREFIX};
use obc_tool_api::{BlastRadius, RiskClass, Tool, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

/// Sends a goal's report to the chat that owns it.
#[async_trait]
pub trait GoalReporter: Send + Sync {
    /// Deliver `text` to `owner` (`telegram:<chat>`, `slack:<channel>`). An
    /// error means it did not arrive; the tick then falls back to the notifier.
    async fn report(&self, owner: &str, text: &str) -> anyhow::Result<()>;
}

/// The owner a goal set from `session` reports to: the chat behind a Telegram
/// or Slack session, or empty (the notifier's channels) for anything else.
pub fn owner_for_session(session: &str) -> String {
    if let Some(chat) = session.strip_prefix("tg-") {
        return format!("telegram:{chat}");
    }
    if let Some(channel) = session.strip_prefix("slack-") {
        return format!("slack:{channel}");
    }
    String::new()
}

/// The world-memory entity a goal's progress notes are written under.
pub fn progress_entity(goal_id: &str) -> String {
    format!(
        "goal.{}.progress",
        goal_id.strip_prefix(GOAL_PREFIX).unwrap_or(goal_id)
    )
}

/// What ticks need beyond the agent: the goal store, world memory for the
/// notes, and the way to the owner.
pub struct Goals {
    scheduler: Arc<Scheduler>,
    world: Option<Arc<WorldMemory>>,
    reporter: Option<Arc<dyn GoalReporter>>,
    history: usize,
}

impl Goals {
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self {
            scheduler,
            world: None,
            reporter: None,
            history: 5,
        }
    }

    pub fn with_world(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    pub fn with_reporter(mut self, reporter: Arc<dyn GoalReporter>) -> Self {
        self.reporter = Some(reporter);
        self
    }

    /// How many earlier notes a tick is shown.
    pub fn with_history(mut self, n: usize) -> Self {
        self.history = n;
        self
    }

    pub fn scheduler(&self) -> &Arc<Scheduler> {
        &self.scheduler
    }

    /// Whether the scheduled task `task_id` ticks a goal. Asked of the goal
    /// table, not of the id alone: a reminder someone named "goal-…" stays a
    /// reminder.
    pub fn is_goal(&self, task_id: &str) -> bool {
        obc_scheduler::is_goal_task(task_id)
            && matches!(self.scheduler.get_goal(task_id), Ok(Some(_)))
    }

    /// The notes earlier ticks left, oldest first, at most `history` of them.
    fn notes(&self, goal: &Goal) -> Vec<String> {
        let from_world = self.world.as_ref().and_then(|w| {
            w.history(&progress_entity(&goal.id)).ok().map(|facts| {
                facts
                    .iter()
                    .filter_map(|f| f.value.get("note").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        });
        let mut notes = match from_world {
            Some(n) if !n.is_empty() => n,
            _ => goal.note.iter().cloned().collect(),
        };
        let skip = notes.len().saturating_sub(self.history);
        notes.drain(..skip);
        notes
    }

    fn record(&self, goal: &Goal, reply: &Reply, changed: bool, done: bool, now: u64) {
        if let Some(w) = &self.world {
            let now_ms = now * 1000;
            let value = json!({ "note": reply.note, "changed": changed, "done": done });
            if let Err(e) = w.observe_as(
                &progress_entity(&goal.id),
                value,
                now_ms,
                now_ms,
                &format!("goal:{}", goal.id),
                Origin::Asserted,
            ) {
                tracing::warn!(goal = %goal.id, error = %e, "goal progress note not written to world memory");
            }
        }
        if let Err(e) = self
            .scheduler
            .record_goal_progress(&goal.id, &reply.note, now)
        {
            tracing::warn!(goal = %goal.id, error = %e, "goal progress not recorded");
        }
    }

    /// Close the goal (which stops its task) and withdraw its progress note
    /// from world memory, so a finished goal stops showing as current state.
    fn close(&self, goal: &Goal, status: GoalStatus, now: u64) {
        if let Err(e) = self.scheduler.close_goal(&goal.id, status, now) {
            tracing::warn!(goal = %goal.id, error = %e, "goal not closed");
        }
        if let Some(w) = &self.world {
            let why = Closure::SourceStopped(format!("goal:{}", goal.id));
            if let Err(e) = w.withdraw_entity(&progress_entity(&goal.id), now * 1000, &why) {
                tracing::warn!(goal = %goal.id, error = %e, "goal progress note not withdrawn");
            }
        }
    }

    /// Close a goal by id or text, as the operator asked. `None` when no such
    /// goal is open.
    pub fn cancel(&self, id_or_text: &str) -> anyhow::Result<Option<Goal>> {
        match self.scheduler.find_goal(id_or_text)? {
            Some(g) if g.status == GoalStatus::Active => {
                self.close(&g, GoalStatus::Cancelled, now_secs());
                Ok(Some(g))
            }
            _ => Ok(None),
        }
    }

    /// Send `text` to the goal's owner, or through the notifier when it has
    /// none or cannot be reached. `true` when it went somewhere.
    async fn report(&self, goal: &Goal, text: &str, notifier: Option<&Arc<Notifier>>) -> bool {
        if !goal.owner.is_empty() {
            if let Some(r) = &self.reporter {
                match r.report(&goal.owner, text).await {
                    Ok(()) => return true,
                    Err(e) => {
                        tracing::warn!(goal = %goal.id, owner = %goal.owner, error = %e, "goal report did not reach its owner; using the notifier");
                    }
                }
            }
        }
        if let Some(n) = notifier {
            n.deliver_summary(text.to_string(), now_secs() * 1000).await;
            return true;
        }
        false
    }
}

/// The message a tick's turn starts with.
pub fn tick_message(goal: &Goal, notes: &[String]) -> String {
    // Same lesson as `scheduled::turn_message`: no word of scheduling, or the
    // local model answers that the goal is already set and does nothing.
    let mut m = format!("[Standing goal] Keep watch on this: \"{}\".\n", goal.text);
    if goal.done_when.is_empty() {
        m.push_str("It has no end condition; keep watching.\n");
    } else {
        m.push_str(&format!("It is met when: {}.\n", goal.done_when));
    }
    if notes.is_empty() {
        m.push_str("This is the first check.\n");
    } else {
        m.push_str("Notes from earlier checks, oldest first:\n");
        for n in notes {
            m.push_str(&format!("- {n}\n"));
        }
    }
    m.push_str(
        "Check where it stands now. Use tools where they help; if something cannot be checked \
         from here, say so. Do not set any reminder or goal. End your reply with three lines:\n\
         NOTE: <one line: where it stands now>\n\
         CHANGED: yes or no (has anything the operator would want to hear about changed since \
         the last note?)\n\
         DONE: yes or no (is it met?)",
    );
    m
}

/// What a tick's reply said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub note: String,
    /// `None` when the reply had no CHANGED line; the tick then compares notes.
    pub changed: Option<bool>,
    pub done: bool,
}

/// Read the NOTE / CHANGED / DONE lines out of a tick's reply. Tolerant of
/// bullets, bold and case. With no NOTE line the whole reply, as one line, is
/// the note.
pub fn parse_reply(text: &str) -> Reply {
    let mut note = None;
    let mut changed = None;
    let mut done = false;
    let mut rest = Vec::new();
    for line in text.lines() {
        let clean: String = line.chars().filter(|c| !matches!(c, '*' | '_')).collect();
        let clean = clean.trim().trim_start_matches(['-', '#', '>', ' ']);
        let field = clean
            .split_once(':')
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()));
        match field {
            Some((k, v)) if k == "note" => note = Some(v.to_string()),
            Some((k, v)) if k == "changed" => changed = Some(is_yes(v)),
            Some((k, v)) if k == "done" => done = is_yes(v),
            _ => rest.push(clean.to_string()),
        }
    }
    let note = note
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| one_line(&rest.join(" "), 400));
    Reply {
        note: if note.is_empty() {
            "(no note)".to_string()
        } else {
            note
        },
        changed,
        done,
    }
}

fn is_yes(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    v.starts_with("yes") || v.starts_with("true") || v == "y"
}

fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    } else {
        s
    }
}

fn normalise(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickOutcome {
    pub goal_id: String,
    /// The note recorded, or why there is none.
    pub note: String,
    pub changed: bool,
    pub done: bool,
    /// Whether a report was sent.
    pub reported: bool,
    /// The goal's status after the tick.
    pub status: GoalStatus,
    /// `false` when the turn failed or the goal could not be read.
    pub ok: bool,
}

/// Run one tick of the goal `d` fires. See the module docs.
pub async fn tick(
    handle: &AgentHandle,
    goals: &Goals,
    d: TaskDispatch,
    notifier: Option<Arc<Notifier>>,
) -> TickOutcome {
    let origin = format!("standing goal '{}'", d.task_name);
    crate::with_unattended_origin(origin, run_tick(handle, goals, d, notifier)).await
}

async fn run_tick(
    handle: &AgentHandle,
    goals: &Goals,
    d: TaskDispatch,
    notifier: Option<Arc<Notifier>>,
) -> TickOutcome {
    let now = now_secs();
    let mut out = TickOutcome {
        goal_id: d.task_id.clone(),
        note: String::new(),
        changed: false,
        done: false,
        reported: false,
        status: GoalStatus::Cancelled,
        ok: true,
    };
    let goal = match goals.scheduler.get_goal(&d.task_id) {
        Ok(Some(g)) => g,
        Ok(None) => {
            let _ = goals.scheduler.set_enabled(&d.task_id, false);
            out.ok = false;
            out.note = "no such goal; its task is stopped".to_string();
            return out;
        }
        Err(e) => {
            tracing::warn!(goal = %d.task_id, error = %e, "goal could not be read");
            out.ok = false;
            out.note = format!("the goal could not be read: {e}");
            return out;
        }
    };
    out.status = goal.status;
    if goal.status != GoalStatus::Active {
        let _ = goals.scheduler.set_enabled(&goal.id, false);
        out.note = goal.note.clone().unwrap_or_default();
        return out;
    }
    let n = notifier.as_ref();

    if goal.until.is_some_and(|u| now >= u) {
        goals.close(&goal, GoalStatus::Expired, now);
        out.status = GoalStatus::Expired;
        out.note = goal.note.clone().unwrap_or_default();
        let last = goal.note.as_deref().unwrap_or("no check had run");
        let text = format!("🎯 Window ended — {}. Last note: {last}", goal.text);
        out.reported = goals.report(&goal, &text, n).await;
        tracing::info!(goal = %goal.id, "standing goal expired");
        return out;
    }

    let message = tick_message(&goal, &goals.notes(&goal));
    let reply = match handle.process_unattended(&d.session_id, &message).await {
        Ok(r) => r.message,
        Err(e) => {
            // No note and no report: a failed check is not news about the
            // goal, and every 30 minutes of "failed" would bury the one that is.
            tracing::warn!(goal = %goal.id, error = %e, "standing goal check failed");
            out.ok = false;
            out.note = format!("the check failed: {e}");
            return out;
        }
    };
    let parsed = parse_reply(&reply);
    let changed = parsed
        .changed
        .unwrap_or_else(|| goal.note.as_deref().map(normalise) != Some(normalise(&parsed.note)));
    // A goal with no end condition is never met by the model's say-so.
    let done = parsed.done && !goal.done_when.is_empty();
    goals.record(&goal, &parsed, changed, done, now);
    out.note = parsed.note.clone();
    out.changed = changed;
    out.done = done;

    if done {
        goals.close(&goal, GoalStatus::Done, now);
        out.status = GoalStatus::Done;
        let text = format!("🎯 Done — {}: {}", goal.text, parsed.note);
        out.reported = goals.report(&goal, &text, n).await;
    } else if changed {
        let text = format!("🎯 {}: {}", goal.text, parsed.note);
        out.reported = goals.report(&goal, &text, n).await;
    }
    tracing::info!(
        goal = %goal.id,
        changed,
        done,
        reported = out.reported,
        note = %one_line(&parsed.note, 160),
        "standing goal checked"
    );
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The `goal` tool: set, list and close standing goals.
pub struct GoalTool {
    goals: Arc<Goals>,
    tz: Tz,
    default_every: String,
}

impl GoalTool {
    pub fn new(goals: Arc<Goals>, tz: Tz, default_every: &str) -> Self {
        Self {
            goals,
            tz,
            default_every: default_every.to_string(),
        }
    }

    fn set(&self, args: &Value) -> ToolResult {
        let Some(text) = str_arg(args, "goal").or_else(|| str_arg(args, "text")) else {
            return ToolResult::err("set needs `goal`: what to keep true or watch for");
        };
        let done_when = str_arg(args, "done_when").unwrap_or_default();
        let now = now_secs();
        let every = str_arg(args, "every").unwrap_or_else(|| self.default_every.clone());
        let kind = match nl::parse_when(&every, now, self.tz) {
            Ok(p) if !matches!(p.kind, TaskKind::OneShot(_)) => p.kind,
            Ok(_) => {
                return ToolResult::err(format!(
                    "`every` must repeat (\"every 30 minutes\"), got \"{every}\""
                ))
            }
            Err(e) => return ToolResult::err(format!("`every`: {e}. Forms: {}", nl::FORMS)),
        };
        let until = match str_arg(args, "until") {
            None => None,
            Some(phrase) => match nl::parse_when(&phrase, now, self.tz) {
                Ok(p) => match p.kind {
                    TaskKind::OneShot(ts) => Some(ts),
                    _ => {
                        return ToolResult::err(format!(
                            "`until` must be one time (\"on sunday at 18:00\", \"in 7 days\"), got \"{phrase}\""
                        ))
                    }
                },
                Err(e) => return ToolResult::err(format!("`until`: {e}. Forms: {}", nl::FORMS)),
            },
        };
        // The owner is the chat this turn came from, never an argument: a
        // goal set from a web page's instructions must not report to a chat
        // the page names.
        let owner = crate::current_session()
            .map(|s| owner_for_session(&s))
            .unwrap_or_default();
        let goal = Goal::new(&text, &done_when, &owner).with_until(until);
        let scheduler = self.goals.scheduler();
        if let Ok(Some(old)) = scheduler.get_goal(&goal.id) {
            if old.status == GoalStatus::Active {
                return ToolResult::err(format!(
                    "a goal like that is already open ({}); close it first",
                    old.id
                ));
            }
        }
        match scheduler.add_goal(&goal, kind.clone(), self.tz) {
            Ok(_) => {
                let mut msg = format!(
                    "Goal set ({}): {}. Checked {}",
                    goal.id,
                    goal.text,
                    nl::describe(&kind, self.tz)
                );
                if !goal.done_when.is_empty() {
                    msg.push_str(&format!("; closes when {}", goal.done_when));
                }
                if let Some(u) = until {
                    msg.push_str(&format!(
                        "; ends {}",
                        nl::describe(&TaskKind::OneShot(u), self.tz)
                    ));
                }
                msg.push_str(". Reports come here only when something changes.");
                ToolResult::ok(msg)
            }
            Err(e) => ToolResult::err(format!("goal not stored: {e}")),
        }
    }

    fn list(&self) -> ToolResult {
        match self.goals.scheduler().list_goals() {
            Ok(goals) if goals.is_empty() => ToolResult::ok("No goals."),
            Ok(goals) => {
                let lines: Vec<String> = goals
                    .iter()
                    .map(|g| {
                        format!(
                            "{} [{}] {} — {} check(s); last: {}",
                            g.id,
                            g.status.as_str(),
                            g.text,
                            g.ticks,
                            g.note.as_deref().unwrap_or("none yet")
                        )
                    })
                    .collect();
                ToolResult::ok(lines.join("\n"))
            }
            Err(e) => ToolResult::err(format!("goals could not be read: {e}")),
        }
    }

    fn close(&self, args: &Value) -> ToolResult {
        let Some(key) = str_arg(args, "goal").or_else(|| str_arg(args, "id")) else {
            return ToolResult::err("close needs `goal`: its id or its text");
        };
        match self.goals.cancel(&key) {
            Ok(Some(g)) => ToolResult::ok(format!("Closed {}: {}", g.id, g.text)),
            Ok(None) => ToolResult::err(format!("no open goal matches \"{key}\"")),
            Err(e) => ToolResult::err(format!("goal not closed: {e}")),
        }
    }
}

#[async_trait]
impl Tool for GoalTool {
    fn name(&self) -> &str {
        "goal"
    }

    fn description(&self) -> &str {
        "Standing goals: something to keep true or watch for, checked on a cadence until it is \
         met (\"keep the greenhouse between 18 and 24 °C this week\", \"tell me when the LoRa \
         node's battery will die within 48 h\"). action=set with goal, optional done_when, \
         every (\"every 30 minutes\") and until (\"on sunday at 18:00\"); action=list; \
         action=close with goal. Reports go to this chat, only when something changes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["set", "list", "close"] },
                "goal": { "type": "string", "description": "What to keep true or watch for (set), or the goal's id or text (close)." },
                "done_when": { "type": "string", "description": "When it counts as met; the goal then closes. Leave out to keep watching." },
                "every": { "type": "string", "description": "How often to check, a repeating phrase. Default from config." },
                "until": { "type": "string", "description": "One time after which the goal ends unmet." }
            },
            "required": ["action"]
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        Ok(match str_arg(&args, "action").as_deref() {
            Some("set") => self.set(&args),
            Some("list") => self.list(),
            Some("close") => self.close(&args),
            other => ToolResult::err(format!("unknown action {other:?}; use set, list or close")),
        })
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_parse_with_bullets_bold_and_case() {
        let r = parse_reply(
            "Checked the sensor.\n- **NOTE:** 21.4 °C, inside the band\n* changed: No\nDone: no",
        );
        assert_eq!(r.note, "21.4 °C, inside the band");
        assert_eq!(r.changed, Some(false));
        assert!(!r.done);
        let r = parse_reply(
            "NOTE: battery 3.41 V, falling 0.02 V/h\nCHANGED: yes\nDONE: Yes, under 48 h",
        );
        assert_eq!(r.changed, Some(true));
        assert!(r.done);
        // No fields at all: the reply is the note, and nothing is claimed.
        let r = parse_reply("It is 22 degrees\nin there.");
        assert_eq!(r.note, "It is 22 degrees in there.");
        assert_eq!(r.changed, None);
        assert!(!r.done);
        assert_eq!(parse_reply("").note, "(no note)");
    }

    #[test]
    fn owners_come_from_chat_sessions_only() {
        assert_eq!(owner_for_session("tg-42"), "telegram:42");
        assert_eq!(owner_for_session("slack-C07"), "slack:C07");
        assert_eq!(owner_for_session("console"), "");
        assert_eq!(owner_for_session("goal-x"), "");
        assert_eq!(progress_entity("goal-deer"), "goal.deer.progress");
    }

    #[test]
    fn the_tick_message_carries_the_goal_and_earlier_notes() {
        let g = Goal::new("Log every deer on the trail cam", "it is Sunday", "");
        let m = tick_message(&g, &[]);
        assert!(m.contains("\"Log every deer on the trail cam\""));
        assert!(m.contains("It is met when: it is Sunday."));
        assert!(m.contains("This is the first check."));
        assert!(!m.to_ascii_lowercase().contains("schedul"));
        let m = tick_message(&g, &["one deer at 06:10".into(), "none since".into()]);
        assert!(m.contains("oldest first:\n- one deer at 06:10\n- none since\n"));
        let open = Goal::new("Watch the gate", "", "");
        assert!(tick_message(&open, &[]).contains("no end condition"));
    }
}

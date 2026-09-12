//! `schedule` tool — timers the agent sets for itself (parity plan Stage 2,
//! item 6, 2026-09-11). "Every weekday at 8 check the printer and message me"
//! becomes: the model calls `schedule` with `when = "every weekday at 8"`, a
//! `prompt` describing the job, and optionally a `tool` to run first. The
//! phrase is converted by `obc_scheduler::nl` with no second model call; a
//! 6-field cron expression is accepted for anything the grammar lacks. Tasks
//! persist in `scheduler.db` and are fired by the scheduler loop; the result of
//! each run is delivered through the notifier.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_scheduler::{nl, ScheduledTask, Scheduler, TaskKind, Tz};
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: create, list, pause, resume and remove scheduled tasks.
pub struct ScheduleTool {
    scheduler: Arc<Scheduler>,
    tz: Tz,
}

impl ScheduleTool {
    pub fn new(scheduler: Arc<Scheduler>, tz: Tz) -> Self {
        Self { scheduler, tz }
    }

    fn now_ts() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn render(&self, t: &ScheduledTask) -> String {
        let when = match &t.phrase {
            Some(p) => format!("\"{p}\" → {}", nl::describe(&t.kind, t.tz)),
            None => nl::describe(&t.kind, t.tz),
        };
        let next = match t.next_run {
            Some(ts) => self.tz.render(ts),
            None => "never".to_string(),
        };
        let what = match (&t.tool, t.prompt.trim().is_empty()) {
            (Some(tool), true) => format!("tool `{tool}`"),
            (Some(tool), false) => format!("tool `{tool}` then: {}", t.prompt.trim()),
            (None, _) => t.prompt.trim().to_string(),
        };
        format!(
            "- {} [{}] {}{} — next {}; runs {}; session {}\n    {}",
            t.name,
            t.id,
            when,
            if t.enabled { "" } else { " (paused)" },
            next,
            t.run_count,
            t.session_id,
            what
        )
    }

    fn create(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let name = str_arg(args, "name")
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' (a short label for the task)"))?;
        let prompt = str_arg(args, "prompt").unwrap_or_default();
        let tool = str_arg(args, "tool");
        if prompt.is_empty() && tool.is_none() {
            return Ok(ToolResult::err(
                "Give a 'prompt' (what to do when it fires) and/or a 'tool' to run.",
            ));
        }
        let tool_args = args.get("tool_args").cloned().filter(|v| !v.is_null());
        if tool.is_none() && tool_args.is_some() {
            return Ok(ToolResult::err("'tool_args' needs a 'tool'."));
        }

        let phrase = str_arg(args, "when");
        let (kind, description) = if let Some(p) = &phrase {
            match nl::parse_when(p, Self::now_ts(), self.tz) {
                Ok(parsed) => (parsed.kind, parsed.description),
                Err(e) => return Ok(ToolResult::err(e)),
            }
        } else if let Some(expr) = str_arg(args, "cron") {
            let kind = TaskKind::Cron(expr);
            (kind.clone(), nl::describe(&kind, self.tz))
        } else if let Some(secs) = args.get("every_secs").and_then(|v| v.as_u64()) {
            let kind = TaskKind::Interval(secs);
            (kind.clone(), nl::describe(&kind, self.tz))
        } else {
            return Ok(ToolResult::err(format!(
                "Say when: 'when' in words ({}), or 'cron' (6 fields: sec min hour dom mon dow, read in {}), or 'every_secs'.",
                nl::FORMS,
                self.tz.as_str()
            )));
        };
        if let Err(e) = kind.validate() {
            return Ok(ToolResult::err(e));
        }

        let id = str_arg(args, "id").unwrap_or_else(|| slug(&name));
        let session =
            str_arg(args, "session").unwrap_or_else(|| format!("scheduled-{}", slug(&name)));
        let mut task = ScheduledTask::from_kind(&id, &name, &prompt, &session, kind, self.tz);
        if let Some(t) = tool {
            task = task.with_tool(t, tool_args);
        }
        if let Some(p) = phrase {
            task = task.with_phrase(p);
        }
        let Some(next) = task.next_run else {
            return Ok(ToolResult::err("that schedule has no next run"));
        };
        let replaced = self.scheduler.get_task(&id)?.is_some();
        self.scheduler.add_task(task)?;
        Ok(ToolResult::ok(format!(
            "{} task '{}' (id {}): {} — first run {}. It runs in session {}; each result is \
             delivered to the operator.",
            if replaced { "Replaced" } else { "Scheduled" },
            name,
            id,
            description,
            self.tz.render(next),
            session
        )))
    }

    fn list(&self) -> anyhow::Result<ToolResult> {
        let tasks = self.scheduler.list_tasks()?;
        if tasks.is_empty() {
            return Ok(ToolResult::ok("No scheduled tasks.".to_string()));
        }
        let mut out = format!(
            "{} scheduled task(s) (times in {}):\n",
            tasks.len(),
            self.tz.as_str()
        );
        for t in &tasks {
            out.push_str(&self.render(t));
            out.push('\n');
        }
        Ok(ToolResult::ok(out.trim_end().to_string()))
    }

    fn lookup(&self, args: &Value) -> anyhow::Result<Result<ScheduledTask, ToolResult>> {
        let key = str_arg(args, "id")
            .or_else(|| str_arg(args, "name"))
            .ok_or_else(|| anyhow::anyhow!("Missing 'id' or 'name'"))?;
        Ok(match self.scheduler.find_task(&key)? {
            Some(t) => Ok(t),
            None => Err(ToolResult::err(format!(
                "No scheduled task called '{key}'."
            ))),
        })
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn slug(name: &str) -> String {
    let mut s = String::new();
    for ch in name.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        "task".to_string()
    } else {
        s.chars().take(40).collect()
    }
}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Set, list, pause, resume or remove timed tasks that fire later without the operator \
         asking again — reminders, recurring checks, follow-ups. Use it when the operator says \
         'every …', 'in 20 minutes', 'tomorrow at 9', 'remind me', 'each weekday'. Put the \
         schedule in 'when' in plain words; put what to do in 'prompt' (an instruction to \
         yourself, e.g. 'check the printer status and report anything off') and, when a \
         specific tool should run first, name it in 'tool' with 'tool_args'. Each run's \
         result is delivered to the operator."
    }

    fn risk_class(&self) -> RiskClass {
        // Creating a timer is a side effect that acts later, and a replayed
        // `create` re-arms a finished one-shot or resurrects a deleted task —
        // which is exactly what the self-improvement pass did on the bench
        // (2026-09-11) while verifying three skills it had learned from
        // successful `schedule` turns. Not reversible, so the forge quarantines
        // such recipes for operator promotion instead of auto-installing them.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "remove", "pause", "resume"],
                    "description": "What to do (default create)."
                },
                "name": { "type": "string", "description": "Short label, e.g. 'Printer check'. Also used to remove/pause/resume." },
                "when": {
                    "type": "string",
                    "description": format!("When, in words: {}. Read in the {} zone.", nl::FORMS, self.tz.as_str())
                },
                "cron": { "type": "string", "description": "Instead of 'when': a 6-field cron expression (sec min hour dom mon dow)." },
                "every_secs": { "type": "integer", "description": "Instead of 'when': run every N seconds." },
                "prompt": { "type": "string", "description": "What to do each time it fires, written as an instruction to yourself." },
                "tool": { "type": "string", "description": "Optional tool or learned skill to run first; its output is given to the prompt." },
                "tool_args": { "type": "object", "description": "Arguments for 'tool'." },
                "session": { "type": "string", "description": "Session to run in (default scheduled-<name>)." },
                "id": { "type": "string", "description": "Task id (create: to replace an existing one; remove/pause/resume: instead of name)." }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = str_arg(&args, "action").unwrap_or_else(|| "create".to_string());
        match action.as_str() {
            "create" | "add" | "set" => self.create(&args),
            "list" => self.list(),
            "remove" | "delete" | "cancel" => {
                let t = match self.lookup(&args)? {
                    Ok(t) => t,
                    Err(r) => return Ok(r),
                };
                self.scheduler.remove_task(&t.id)?;
                Ok(ToolResult::ok(format!(
                    "Removed scheduled task '{}' ({}).",
                    t.name, t.id
                )))
            }
            "pause" | "disable" | "resume" | "enable" => {
                let on = matches!(action.as_str(), "resume" | "enable");
                let t = match self.lookup(&args)? {
                    Ok(t) => t,
                    Err(r) => return Ok(r),
                };
                if on {
                    // Re-arm from now so a task paused past its next_run does not fire at once.
                    let mut t2 = t.clone();
                    t2.enabled = true;
                    if t2.kind.repeats() {
                        t2.next_run = t2.kind.next_run_after_in(Self::now_ts(), t2.tz);
                    }
                    self.scheduler.add_task(t2)?;
                } else {
                    self.scheduler.set_enabled(&t.id, false)?;
                }
                Ok(ToolResult::ok(format!(
                    "{} scheduled task '{}' ({}).",
                    if on { "Resumed" } else { "Paused" },
                    t.name,
                    t.id
                )))
            }
            other => Ok(ToolResult::err(format!(
                "Unknown action '{other}'. Use create, list, remove, pause or resume."
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message a result carries: `output` on success, `error` on failure.
    fn text(r: &ToolResult) -> String {
        if r.success {
            r.output.clone()
        } else {
            r.error.clone().unwrap_or_default()
        }
    }

    fn tool() -> ScheduleTool {
        ScheduleTool::new(Scheduler::new(":memory:").unwrap(), Tz::Utc)
    }

    #[tokio::test]
    async fn creates_from_words_lists_and_removes() {
        let t = tool();
        let r = t
            .execute(json!({
                "name": "Printer check",
                "when": "every weekday at 8",
                "prompt": "check the printer and report anything off",
                "tool": "device_health",
                "tool_args": {"node": "printer"}
            }))
            .await
            .unwrap();
        assert!(r.success, "{}", text(&r));
        assert!(text(&r).contains("weekdays at 08:00"), "{}", text(&r));
        assert!(
            text(&r).contains("session scheduled-printer-check"),
            "{}",
            text(&r)
        );
        let stored = t.scheduler.get_task("printer-check").unwrap().unwrap();
        assert_eq!(stored.kind, TaskKind::Cron("0 0 8 * * Mon-Fri".into()));
        assert_eq!(stored.tool.as_deref(), Some("device_health"));
        assert_eq!(stored.phrase.as_deref(), Some("every weekday at 8"));

        let r = t.execute(json!({"action": "list"})).await.unwrap();
        assert!(
            text(&r).contains("Printer check [printer-check]"),
            "{}",
            text(&r)
        );
        assert!(r
            .output
            .contains("tool `device_health` then: check the printer"));

        let r = t
            .execute(json!({"action": "pause", "name": "printer check"}))
            .await
            .unwrap();
        assert!(r.success && text(&r).starts_with("Paused"), "{}", text(&r));
        assert!(
            !t.scheduler
                .get_task("printer-check")
                .unwrap()
                .unwrap()
                .enabled
        );
        let r = t
            .execute(json!({"action": "resume", "id": "printer-check"}))
            .await
            .unwrap();
        assert!(r.success && text(&r).starts_with("Resumed"), "{}", text(&r));

        let r = t
            .execute(json!({"action": "remove", "name": "Printer check"}))
            .await
            .unwrap();
        assert!(r.success, "{}", text(&r));
        assert_eq!(t.scheduler.task_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn one_shot_cron_and_errors() {
        let t = tool();
        let r = t
            .execute(
                json!({"name": "Nudge", "when": "in 20 minutes", "prompt": "remind me to stretch"}),
            )
            .await
            .unwrap();
        assert!(
            r.success && text(&r).contains("in 20 minutes"),
            "{}",
            text(&r)
        );

        let r = t
            .execute(json!({"name": "Backup", "cron": "0 30 2 * * *", "prompt": "run the backup"}))
            .await
            .unwrap();
        assert!(
            r.success && text(&r).contains("cron `0 30 2 * * *`"),
            "{}",
            text(&r)
        );

        let r = t
            .execute(json!({"name": "Bad", "cron": "30 2 * *", "prompt": "x"}))
            .await
            .unwrap();
        assert!(
            !r.success && text(&r).contains("bad cron expression"),
            "{}",
            text(&r)
        );

        let r = t
            .execute(json!({"name": "Vague", "when": "whenever", "prompt": "x"}))
            .await
            .unwrap();
        assert!(
            !r.success && text(&r).contains("accepted forms"),
            "{}",
            text(&r)
        );

        let r = t
            .execute(json!({"name": "Nothing", "when": "in 5 minutes"}))
            .await
            .unwrap();
        assert!(!r.success && text(&r).contains("'prompt'"), "{}", text(&r));

        let r = t
            .execute(json!({"action": "remove", "name": "ghost"}))
            .await
            .unwrap();
        assert!(
            !r.success && text(&r).contains("No scheduled task"),
            "{}",
            text(&r)
        );

        assert!(t
            .execute(json!({"when": "in 5 minutes", "prompt": "x"}))
            .await
            .is_err());
    }

    #[test]
    fn the_tool_is_not_safe_to_replay() {
        let r = tool().risk_class();
        assert!(!r.reversible && !r.physical);
        assert!(!matches!(r.blast, BlastRadius::None));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Printer check"), "printer-check");
        assert_eq!(slug("  ??? "), "task");
        assert_eq!(slug("A/B: test!"), "a-b-test");
    }
}

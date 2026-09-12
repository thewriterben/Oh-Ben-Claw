//! Running a scheduled task (parity plan Stage 2, item 6, 2026-09-11).
//!
//! `obc_scheduler::run_scheduler_loop` had no caller until now: tasks were stored
//! and listed, never fired. This is the dispatch it hands each due task to.
//!
//! A task may carry a **tool** (any registered tool, learned skills included)
//! and/or a **prompt**. The tool runs first, directly, under the same policy as
//! any tool call; its output is handed to the prompt as context. The prompt is
//! an ordinary agent turn in the task's own session, so it shows up in the
//! Command Center like any conversation and the router treats `scheduled-*`
//! sessions as routine (local brain). Whatever came out — the reply, or the
//! bare tool output when there is no prompt — is delivered through the
//! [`Notifier`] when notifications are on (world-memory log, webhook, speech),
//! which is what "and message me" means on this bench.

use crate::handle::AgentHandle;
use crate::notify::Notifier;
use obc_scheduler::TaskDispatch;
use std::sync::Arc;

/// Prefix of the session a scheduled task runs in when none was named.
pub const SESSION_PREFIX: &str = "scheduled-";

/// The outcome of one scheduled run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub task_id: String,
    pub task_name: String,
    /// What was produced: the reply, or the tool output when there was no prompt.
    pub text: String,
    /// `false` when the tool or the turn failed; `text` then holds the error.
    pub ok: bool,
}

/// The message a scheduled prompt turn starts with, so the model knows this is
/// a timer firing and not the operator typing.
pub fn turn_message(task_name: &str, prompt: &str, tool_report: Option<&str>) -> String {
    let mut m = format!(
        "[Scheduled task \"{task_name}\" fired. Do what it says below, then answer with \
         the result in a few sentences; that answer is delivered to the operator.]\n{prompt}"
    );
    if let Some(r) = tool_report {
        m.push_str("\n\n");
        m.push_str(r);
    }
    m
}

/// Run one dispatch to completion: tool, then prompt, then delivery.
pub async fn run_scheduled(
    handle: &AgentHandle,
    d: TaskDispatch,
    notifier: Option<Arc<Notifier>>,
) -> Outcome {
    let started = std::time::Instant::now();
    let mut ok = true;
    let mut tool_report: Option<String> = None;

    if let Some(tool) = d.tool.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let args = d.tool_args.clone().unwrap_or_else(|| serde_json::json!({}));
        match handle.execute_tool_direct(tool, args).await {
            Ok(r) if r.success => {
                tool_report = Some(format!("Result of `{tool}`:\n{}", r.output.trim()));
            }
            Ok(r) => {
                ok = false;
                let why = r.error.unwrap_or(r.output);
                tool_report = Some(format!("`{tool}` failed: {}", why.trim()));
            }
            Err(e) => {
                ok = false;
                tool_report = Some(format!("`{tool}` could not run: {e}"));
            }
        }
    }

    let prompt = d.prompt.trim();
    let text = if prompt.is_empty() {
        tool_report
            .clone()
            .unwrap_or_else(|| "scheduled task had neither a prompt nor a tool".to_string())
    } else {
        let message = turn_message(&d.task_name, prompt, tool_report.as_deref());
        match handle.process(&d.session_id, &message).await {
            Ok(resp) => resp.message,
            Err(e) => {
                ok = false;
                format!("the scheduled turn failed: {e}")
            }
        }
    };

    let elapsed = started.elapsed().as_secs_f32();
    if ok {
        tracing::info!(
            task_id = %d.task_id,
            task = %d.task_name,
            session = %d.session_id,
            secs = format!("{elapsed:.1}"),
            result = %preview(&text),
            "scheduled task ran"
        );
    } else {
        tracing::warn!(
            task_id = %d.task_id,
            task = %d.task_name,
            secs = format!("{elapsed:.1}"),
            result = %preview(&text),
            "scheduled task failed"
        );
    }

    if let Some(n) = notifier {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let head = if ok { "⏰" } else { "⏰ failed" };
        n.deliver_summary(format!("{head} {}: {}", d.task_name, text.trim()), now_ms)
            .await;
    }

    Outcome {
        task_id: d.task_id,
        task_name: d.task_name,
        text,
        ok,
    }
}

fn preview(s: &str) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 160 {
        let cut: String = one_line.chars().take(157).collect();
        format!("{cut}…")
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_message_names_the_task_and_carries_the_tool_report() {
        let m = turn_message(
            "Printer check",
            "check the printer",
            Some("Result of `x`:\nok"),
        );
        assert!(m.starts_with("[Scheduled task \"Printer check\" fired."));
        assert!(m.contains("\ncheck the printer\n\nResult of `x`:\nok"));
        let m = turn_message("t", "p", None);
        assert!(m.ends_with("\np"));
    }

    #[test]
    fn preview_is_one_short_line() {
        assert_eq!(preview("a\n  b\tc"), "a b c");
        assert_eq!(preview(&"x".repeat(200)).chars().count(), 158);
    }
}

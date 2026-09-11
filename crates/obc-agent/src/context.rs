//! Context management: what the model is shown, in what order, and when the
//! history is folded into a summary.
//!
//! Three problems this closes (2026-09-11, parity plan item 2):
//!
//! 1. **The prefix was never cacheable.** The world-state block — regenerated
//!    every turn — sat right after the system prompt, so every provider saw a
//!    different prefix on every turn. Ollama's prompt cache and Anthropic's
//!    prompt caching both work on the longest identical prefix; a changing
//!    second message defeats both. The order is now: system prompt, history,
//!    then the ephemeral blocks (experience, world state), then the latest
//!    user message. Only the tail changes turn to turn.
//! 2. **History was raw and unbounded in tokens.** The last fifty messages
//!    went in verbatim, whatever their size. Now, when the estimated history
//!    exceeds `compaction_threshold × context_tokens`, the older part is
//!    summarised by the model into one persisted `[Conversation summary]`
//!    message and the assembled history is that summary plus what came after
//!    it. The summary carries a cursor (the id of the last message it covers),
//!    so it is placed first even though it was written last.
//! 3. **Tool outputs piled up within a turn.** Ten iterations of a chatty
//!    tool could fill the window. Older tool results are stubbed once they are
//!    behind the last two, the way Hermes does it.
//!
//! Everything here is a pure function over messages so it is tested without a
//! model; `Agent` wires it.

use obc_memory::{ChatMessage, ChatRole, StoredMessage};

/// Prefix of a persisted summary message. Everything after `through #<id>` is
/// the summary text.
pub const SUMMARY_MARKER: &str = "[Conversation summary through #";

/// Prefix the agent loop gives a tool result it feeds back to the model.
pub const TOOL_RESULT_PREFIX: &str = "[Tool result for ";

/// Replacement for a stubbed tool output.
pub const STUB: &str = "[Old tool output cleared to save context space]";

/// Rough token count: chars/4 plus a few per message for role framing. A
/// relative signal for thresholds, not billing-grade accounting.
pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(|m| m.content.len() / 4 + 4).sum()
}

/// Build the persisted summary message content.
pub fn summary_message(through_id: i64, summary: &str) -> String {
    format!("{SUMMARY_MARKER}{through_id}]\n{}", summary.trim())
}

/// Parse a summary message's cursor, if the message is one.
pub fn summary_cursor(content: &str) -> Option<i64> {
    let rest = content.strip_prefix(SUMMARY_MARKER)?;
    let end = rest.find(']')?;
    rest[..end].parse().ok()
}

/// Turn stored messages (oldest first) into the history the model sees:
/// the latest summary, if any, followed by every non-summary message after
/// its cursor. Older summaries and the messages they covered are dropped.
pub fn assemble_history(stored: &[StoredMessage]) -> Vec<ChatMessage> {
    let latest_summary = stored
        .iter()
        .rev()
        .find(|m| m.role == "system" && summary_cursor(&m.content).is_some());
    let (cursor, mut out) = match latest_summary {
        Some(s) => (
            summary_cursor(&s.content).unwrap_or(-1),
            vec![ChatMessage {
                role: ChatRole::System,
                content: s.content.clone(),
            }],
        ),
        None => (-1, Vec::new()),
    };
    for m in stored {
        if m.id <= cursor {
            continue;
        }
        if m.role == "system" && summary_cursor(&m.content).is_some() {
            continue;
        }
        let role = match m.role.as_str() {
            "system" => ChatRole::System,
            "assistant" => ChatRole::Assistant,
            _ => ChatRole::User,
        };
        out.push(ChatMessage {
            role,
            content: m.content.clone(),
        });
    }
    out
}

/// Which stored messages a compaction would fold: everything after the
/// current cursor except the last `keep_tail`. `None` when there is nothing
/// worth folding (fewer than two messages would be summarised).
pub fn compaction_range(stored: &[StoredMessage], keep_tail: usize) -> Option<(usize, usize)> {
    let cursor = stored
        .iter()
        .rev()
        .find(|m| m.role == "system" && summary_cursor(&m.content).is_some())
        .and_then(|s| summary_cursor(&s.content))
        .unwrap_or(-1);
    let candidates: Vec<usize> = stored
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.id > cursor && !(m.role == "system" && summary_cursor(&m.content).is_some())
        })
        .map(|(i, _)| i)
        .collect();
    if candidates.len() < keep_tail + 2 {
        return None;
    }
    let end = candidates.len() - keep_tail;
    Some((candidates[0], candidates[end - 1]))
}

/// The messages handed to the summariser: the previous summary (if any) so
/// nothing is lost across compactions, then the folded span.
pub fn summarizer_input(
    previous_summary: Option<&str>,
    span: &[&StoredMessage],
) -> Vec<ChatMessage> {
    let mut transcript = String::new();
    if let Some(prev) = previous_summary {
        transcript.push_str("Earlier summary:\n");
        transcript.push_str(prev.trim());
        transcript.push_str("\n\n");
    }
    transcript.push_str("Conversation to fold in:\n");
    for m in span {
        transcript.push_str(&format!("{}: {}\n", m.role, m.content.trim()));
    }
    vec![
        ChatMessage {
            role: ChatRole::System,
            content: "You maintain a running summary of a conversation between an operator and \
                      an embodied agent so the agent can continue it later with less context. \
                      Write a compact summary in plain prose: what was asked, what was done \
                      and with which tools, decisions taken, facts learned, and anything still \
                      open. Keep names, numbers, paths and identifiers exact. No preamble."
                .to_string(),
        },
        ChatMessage {
            role: ChatRole::User,
            content: transcript,
        },
    ]
}

/// Within a turn: stub every tool result except the last `keep_last`, when it
/// is long enough to matter. The model already acted on them; what it needs
/// is that they happened, not their bytes.
pub fn stub_old_tool_outputs(messages: &mut [ChatMessage], keep_last: usize) -> usize {
    let idx: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == ChatRole::User && m.content.starts_with(TOOL_RESULT_PREFIX))
        .map(|(i, _)| i)
        .collect();
    let mut stubbed = 0;
    if idx.len() <= keep_last {
        return 0;
    }
    for &i in &idx[..idx.len() - keep_last] {
        let m = &mut messages[i];
        if m.content.len() > 200 && !m.content.ends_with(STUB) {
            // Keep the "[Tool result for name (id=…)]:" header so the loop's
            // bookkeeping still reads; drop the payload.
            let header_end = m.content.find("]:").map(|p| p + 2).unwrap_or(0);
            let header = m.content[..header_end].to_string();
            m.content = format!("{header} {STUB}");
            stubbed += 1;
        }
    }
    stubbed
}

/// Place the ephemeral blocks: after the history, before the final user
/// message, so the stable prefix ends with the last *stored* exchange.
pub fn with_ephemeral_tail(
    mut messages: Vec<ChatMessage>,
    ephemeral: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    if ephemeral.is_empty() {
        return messages;
    }
    let last_is_user = messages.last().is_some_and(|m| m.role == ChatRole::User);
    if last_is_user {
        let last = messages.pop().expect("checked non-empty");
        messages.extend(ephemeral);
        messages.push(last);
    } else {
        messages.extend(ephemeral);
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn stored(id: i64, role: &str, content: &str) -> StoredMessage {
        StoredMessage {
            id,
            session_id: "s".into(),
            role: role.into(),
            content: content.into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn a_summary_replaces_what_it_covers_and_comes_first() {
        let s = vec![
            stored(1, "user", "first"),
            stored(2, "assistant", "reply"),
            stored(3, "user", "second"),
            stored(4, "assistant", "reply2"),
            stored(5, "user", "third"),
            stored(
                6,
                "system",
                &summary_message(4, "we talked about first and second"),
            ),
            stored(7, "assistant", "reply3"),
        ];
        let h = assemble_history(&s);
        let roles: Vec<_> = h.iter().map(|m| (m.role, m.content.as_str())).collect();
        assert_eq!(roles[0].0, ChatRole::System);
        assert!(roles[0].1.starts_with(SUMMARY_MARKER));
        assert_eq!(
            roles[1..].iter().map(|(_, c)| *c).collect::<Vec<_>>(),
            vec!["third", "reply3"],
            "messages after the cursor survive, in order, without the summary row itself"
        );
    }

    #[test]
    fn a_newer_summary_supersedes_an_older_one() {
        let s = vec![
            stored(1, "user", "a"),
            stored(2, "system", &summary_message(1, "one")),
            stored(3, "user", "b"),
            stored(4, "system", &summary_message(3, "two")),
            stored(5, "user", "c"),
        ];
        let h = assemble_history(&s);
        assert_eq!(h.len(), 2);
        assert!(h[0].content.contains("two"));
        assert_eq!(h[1].content, "c");
    }

    #[test]
    fn compaction_range_keeps_the_tail_and_respects_the_cursor() {
        let mut s: Vec<StoredMessage> = (1..=10)
            .map(|i| stored(i, if i % 2 == 1 { "user" } else { "assistant" }, "x"))
            .collect();
        assert_eq!(
            compaction_range(&s, 4),
            Some((0, 5)),
            "fold ids 1..=6, keep 7..=10"
        );
        s.push(stored(11, "system", &summary_message(6, "folded")));
        s.push(stored(12, "user", "y"));
        // After the cursor there are 5 candidates (7,8,9,10,12); keep 4 → 1 to fold: below the minimum of 2.
        assert_eq!(compaction_range(&s, 4), None);
        s.push(stored(13, "assistant", "z"));
        assert_eq!(compaction_range(&s, 4), Some((6, 7)), "fold ids 7 and 8");
    }

    #[test]
    fn old_tool_outputs_are_stubbed_but_the_last_two_are_kept() {
        let big = "x".repeat(400);
        let mut m = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "hi".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "[Tool calls: shell]".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: format!("[Tool result for shell (id=1)]: {big}"),
            },
            ChatMessage {
                role: ChatRole::User,
                content: format!("[Tool result for file (id=2)]: {big}"),
            },
            ChatMessage {
                role: ChatRole::User,
                content: format!("[Tool result for http (id=3)]: {big}"),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "[Tool result for tiny (id=4)]: ok".into(),
            },
        ];
        assert_eq!(stub_old_tool_outputs(&mut m, 2), 2);
        assert_eq!(
            m[2].content,
            format!("[Tool result for shell (id=1)]: {STUB}")
        );
        assert_eq!(
            m[3].content,
            format!("[Tool result for file (id=2)]: {STUB}")
        );
        assert!(
            m[4].content.ends_with(&big),
            "the last two results are untouched"
        );
        assert_eq!(stub_old_tool_outputs(&mut m, 2), 0, "idempotent");
    }

    #[test]
    fn ephemeral_blocks_go_before_the_final_user_message() {
        let msgs = vec![
            ChatMessage {
                role: ChatRole::System,
                content: "prompt".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "earlier".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "reply".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "now".into(),
            },
        ];
        let out = with_ephemeral_tail(
            msgs,
            vec![ChatMessage {
                role: ChatRole::System,
                content: "## World state".into(),
            }],
        );
        let order: Vec<&str> = out.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            order,
            vec!["prompt", "earlier", "reply", "## World state", "now"]
        );
    }

    #[test]
    fn token_estimate_is_monotone_in_content() {
        let a = vec![ChatMessage {
            role: ChatRole::User,
            content: "x".repeat(400),
        }];
        let b = vec![ChatMessage {
            role: ChatRole::User,
            content: "x".repeat(800),
        }];
        assert!(estimate_tokens(&b) > estimate_tokens(&a));
        assert_eq!(estimate_tokens(&a), 104);
    }
}

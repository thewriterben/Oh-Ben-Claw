//! Approve from the chat you're already in (2026-11-01).
//!
//! A scheduled task or an A2A call that reaches a gated tool waits in
//! [`ApprovalManager::wait_for_decision`] while every attached
//! [`ApprovalPrompter`](obc_approval::ApprovalPrompter) shows the call with
//! buttons. Since 2026-11-02 an attended turn sent by an approver does the
//! same, asked only in the chat it came from
//! ([`obc_agent::with_present_approver`]); each channel keeps reading taps
//! while its turns run. The Telegram and Slack prompters live with their channels; this
//! module holds what they share: what a button carries, and what a tap on one
//! does. Who may tap is the channel's own approver list, not its chat
//! allowlist: talking to the agent is not the right to open its gates.

use obc_approval::{chat_code, parse_chat_code, ApprovalManager, ApprovalResponse};

/// What an approval button carries: `obc:<code>:<id>`, 38 bytes, well inside
/// Telegram's 64-byte `callback_data`.
pub fn button_value(decision: &ApprovalResponse, id: &str) -> String {
    format!("obc:{}:{id}", chat_code(decision))
}

/// The answer and the pending id out of a button value; `None` for anything
/// this agent did not send, a forever grant among them.
pub fn parse_button_value(value: &str) -> Option<(ApprovalResponse, &str)> {
    let (code, id) = value.strip_prefix("obc:")?.split_once(':')?;
    let decision = parse_chat_code(code)?;
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some((decision, id))
}

/// Apply a tap: `user` pressed the button carrying `value` in `channel`
/// (`"telegram"`, `"slack"`). On success, the line to put under the request
/// in place of its buttons; otherwise why the tap did nothing, for the person
/// who pressed it. A tap from someone not in `approvers` is refused and
/// audited by [`ApprovalManager::resolve_pending`], and the call keeps waiting.
pub fn answer_tap(
    approval: Option<&ApprovalManager>,
    channel: &str,
    value: &str,
    user: &str,
    approvers: &[String],
) -> Result<String, String> {
    let (decision, id) =
        parse_button_value(value).ok_or("not an approval button from this agent")?;
    let approval = approval.ok_or("approvals are not enabled on this agent")?;
    let pending = approval
        .resolve_pending(id, decision.clone(), channel, user, approvers)
        .map_err(|e| e.to_string())?;
    let by = format!("{channel}:{user}");
    Ok(match decision {
        ApprovalResponse::No => format!("⛔ Denied by {by}"),
        ApprovalResponse::Always if pending.to.is_some() => format!(
            "✅ Approved by {by}; {} allowed for this session",
            pending.tool_name
        ),
        ApprovalResponse::Always => {
            let window = pending
                .chat_choices()
                .into_iter()
                .find(|(d, _)| *d == ApprovalResponse::Always)
                .map(|(_, label)| label.trim_start_matches("Allow ").to_string())
                .unwrap_or_default();
            format!(
                "✅ Approved by {by}; {} allowed unattended for {window}",
                pending.tool_name
            )
        }
        _ => format!("✅ Approved by {by} (this call)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_approval::{ApprovalRequest, AutonomyConfig, ForeverGrants, WaitOutcome};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn a_button_value_round_trips_and_fits_telegram() {
        let id = "0123456789abcdef0123456789abcdef";
        for d in [
            ApprovalResponse::Yes,
            ApprovalResponse::Always,
            ApprovalResponse::No,
        ] {
            let v = button_value(&d, id);
            assert!(v.len() <= 64, "{v}");
            assert_eq!(parse_button_value(&v), Some((d, id)));
        }
        // Not ours, or claiming what a chat never offers.
        assert_eq!(parse_button_value(&format!("obc:f:{id}")), None);
        assert_eq!(parse_button_value(&format!("xyz:y:{id}")), None);
        assert_eq!(parse_button_value("obc:y:short"), None);
        assert_eq!(
            parse_button_value("obc:y:../../etc/passwd/aaaaaaaaaaaaaaaaaa"),
            None
        );
    }

    /// A call waiting the way a scheduled task's does, and its pending id.
    async fn waiting(mgr: &Arc<ApprovalManager>) -> (tokio::task::JoinHandle<WaitOutcome>, String) {
        let m = Arc::clone(mgr);
        let call = tokio::spawn(async move {
            let req = ApprovalRequest {
                tool_name: "open_gate".into(),
                arguments: json!({}),
                risk: None,
            };
            m.wait_for_decision(&req, "scheduled task 'gate'", "r")
                .await
        });
        for _ in 0..400 {
            if let Some(p) = mgr.pending().first() {
                return (call, p.id.clone());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("nothing pending");
    }

    fn mgr() -> Arc<ApprovalManager> {
        let grants = std::env::temp_dir().join(format!(
            "obc-channels-approval-{}.json",
            uuid::Uuid::new_v4()
        ));
        Arc::new(ApprovalManager::with_grants(
            &AutonomyConfig::default(),
            ForeverGrants::load(grants),
            true,
        ))
    }

    #[tokio::test]
    async fn only_an_approver_can_answer_and_only_once() {
        let mgr = mgr();
        let approvers = vec!["42".to_string()];
        let (call, id) = waiting(&mgr).await;
        let yes = button_value(&ApprovalResponse::Yes, &id);

        let stranger = answer_tap(Some(&mgr), "telegram", &yes, "99", &approvers);
        assert_eq!(
            stranger.unwrap_err(),
            "you are not on this channel's approver list"
        );
        assert_eq!(mgr.pending().len(), 1, "a stranger's tap answered the call");
        assert!(mgr.audit_log().last().unwrap().refused.is_some());

        let line = answer_tap(Some(&mgr), "telegram", &yes, "42", &approvers).unwrap();
        assert_eq!(line, "✅ Approved by telegram:42 (this call)");
        assert!(matches!(call.await.unwrap(), WaitOutcome::Approved { .. }));

        let again = answer_tap(Some(&mgr), "telegram", &yes, "42", &approvers);
        assert!(again.unwrap_err().contains("no longer pending"));
    }

    #[tokio::test]
    async fn deny_and_allow_say_what_they_did() {
        let approvers = vec!["U1".to_string()];
        let mgr = mgr();
        let (call, id) = waiting(&mgr).await;
        let no = button_value(&ApprovalResponse::No, &id);
        let line = answer_tap(Some(&mgr), "slack", &no, "U1", &approvers).unwrap();
        assert_eq!(line, "⛔ Denied by slack:U1");
        assert!(matches!(call.await.unwrap(), WaitOutcome::Denied { .. }));

        let (call, id) = waiting(&mgr).await;
        let allow = button_value(&ApprovalResponse::Always, &id);
        let line = answer_tap(Some(&mgr), "slack", &allow, "U1", &approvers).unwrap();
        assert_eq!(
            line,
            "✅ Approved by slack:U1; open_gate allowed unattended for 30 min"
        );
        call.await.unwrap();

        // A manager-less agent and a foreign button both say so.
        assert!(answer_tap(None, "slack", &allow, "U1", &approvers)
            .unwrap_err()
            .contains("not enabled"));
        assert!(answer_tap(Some(&mgr), "slack", "hello", "U1", &approvers)
            .unwrap_err()
            .contains("not an approval button"));
    }
}

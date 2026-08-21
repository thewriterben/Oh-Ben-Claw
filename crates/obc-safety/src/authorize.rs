//! Track 0: the join between what a tool declares and what the gate does.
//!
//! [`track0_authorize`] is the one function that reads a tool's [`RiskClass`]
//! and turns it into an outcome: consult the deterministic [`SafetyGate`],
//! append the decision to the tamper-evident [`ActionAuditor`] chain, and refuse
//! the call if the gate refuses it.
//!
//! # Why it lives here
//!
//! It lived in `obc-agent` until 2026-08-19, which meant the claim this whole
//! layer rests on — *a tool declares that it actuates, and the gate believes it*
//! — could not be executed anywhere the agent was absent. OBC-Prime vendors
//! `obc-safety`, `obc-tool-api` and `obc-tools`, so it had both halves and no
//! wire: a reader could see a tool declare `risk_class`, and see a `SafetyGate`
//! refuse a pin, and had to take on faith that the first reaches the second.
//! That gap is written into OBC-Prime's own README.
//!
//! The function never needed the agent. It touches [`SafetyGate`],
//! [`ActionAuditor`], [`Decision`] and [`RiskClass`] — all four defined in this
//! crate — plus `serde_json` and `tracing`, which this crate already depends on.
//! Moving it added no edge in either direction; it turned one around, which is
//! the same manoeuvre that released `obc-approval`, `obc-reflex` and the spine
//! sinks before it.
//!
//! # What it does not do
//!
//! It does not ask a human. Approval, autonomy level and behavioural trust are
//! `obc-approval`'s, and the agent calls that separately. This is the
//! deterministic half: a table of limits, and a record that cannot be edited
//! after the fact.

use crate::audit::{ActionAuditor, Decision};
use crate::limits::SafetyGate;
use crate::risk::RiskClass;
use serde_json::Value;
use std::sync::Mutex;

/// Authorize a single tool call against the Track 0 safety layer.
///
/// Non-physical tools pass through untouched (returns `Ok`). For physical tools,
/// the deterministic [`SafetyGate`] (when configured) is consulted using the
/// action's `node_id`/`pin`/`value`, and the resulting decision is appended to
/// the tamper-evident audit log (when configured). Auditing never blocks the
/// action path. Returns `Err(reason)` only when the gate refuses the action.
///
/// The first line is the reason `Tool::risk_class` matters: a tool that reports
/// itself non-physical is not gated, not limit-checked, and not audited. That is
/// what `scripts/check_physical_tools.py` exists to prevent.
pub fn track0_authorize(
    safety: Option<&SafetyGate>,
    auditor: Option<&Mutex<ActionAuditor>>,
    tool: &str,
    risk: RiskClass,
    args: &Value,
) -> Result<(), String> {
    if !risk.physical {
        return Ok(());
    }

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("local");
    let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0);
    let value = args.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
    let now = now_ms();

    let decision = match safety {
        // A rule covers this node+tool: the action is bounded, and the record
        // says which way it went.
        Some(gate) if gate.covers(node_id, tool) => {
            match gate.check(node_id, tool, pin, value, now) {
                Ok(()) => Decision::Allowed,
                Err(violation) => Decision::Denied(violation.to_string()),
            }
        }
        // A gate exists and has nothing to say about this node+tool. The
        // approval layer still governs, so this stays an allow — but it is not
        // the same allow as the one above, and it no longer records as one.
        Some(_) => {
            warn_uncovered(node_id, tool, "no limit rule covers it");
            Decision::AllowedUncovered(format!("no limit rule for {node_id}/{tool}"))
        }
        // No deterministic gate configured at all. `Agent::safety` is an
        // `Option`, so this is a live configuration, not a theoretical one.
        None => {
            warn_uncovered(node_id, tool, "no safety gate is configured");
            Decision::AllowedUncovered("no safety gate configured".to_string())
        }
    };

    if let Some(auditor) = auditor {
        let mut a = auditor.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = a.record(now, node_id, tool, args, risk, decision.clone()) {
            tracing::warn!(error = %e, "Track 0 action audit write failed");
        }
    }

    match decision {
        Decision::Denied(reason) => Err(reason),
        _ => Ok(()),
    }
}

/// Warn the first time a physical action on this node+tool goes ungated.
///
/// Once per `(node, tool)` per process, deliberately. An actuator fires on a
/// timer; a line per actuation would be a log nobody reads, which is the same
/// as silence with more noise. Once is a signal an operator can act on: it
/// means a physical tool is live on a node whose limits nobody wrote.
///
/// Before this, that path emitted nothing at all. Adding an actuator node and
/// forgetting its `[[safety.limits]]` entry produced a working robot, an
/// audit line that read `allowed`, and no way to tell.
fn warn_uncovered(node_id: &str, tool: &str, why: &str) {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static SEEN: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();

    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = seen.lock().unwrap_or_else(|e| e.into_inner());
    if guard.insert((node_id.to_string(), tool.to_string())) {
        tracing::warn!(
            node_id,
            tool,
            "Track 0: physical action allowed ungated — {why}. \
             The approval layer still applies; the deterministic limit check does not."
        );
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{SafetyGate, SafetyLimit};
    use crate::risk::BlastRadius;
    use serde_json::json;

    // These three moved here from `obc-agent` with the function on 2026-08-19.
    // They never needed the agent either: a gate, a risk class and a JSON blob.

    /// A gate with one rule, so `covers` is true for `local/gpio_write` and
    /// false for anything else.
    fn one_rule() -> SafetyGate {
        SafetyGate::new(vec![SafetyLimit {
            node_id: "local".into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![17]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: None,
        }])
    }

    /// Pull the decision back out of a fresh audit log.
    ///
    /// Read from the file rather than from memory on purpose: the claim in
    /// `docs/SAFETY-CASE.md` §3.3 is about what an auditor finds on disk.
    fn recorded(
        tag: &str,
        gate: Option<&SafetyGate>,
        args: &serde_json::Value,
    ) -> crate::audit::Decision {
        let dir = std::env::temp_dir().join(format!("obc-authz-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        let auditor = Mutex::new(ActionAuditor::open(b"k".to_vec(), path.clone()).unwrap());
        let risk = RiskClass::physical(false, BlastRadius::High);
        let _ = track0_authorize(gate, Some(&auditor), "gpio_write", risk, args);
        let text = std::fs::read_to_string(&path).expect("an audit file");
        let line = text.lines().next().expect("exactly one audit record");
        let rec: crate::audit::ActionRecord = serde_json::from_str(line).unwrap();
        rec.decision
    }

    // ── Coverage is not a pass ───────────────────────────────────────────────
    // `SafetyGate::check` returns Ok both when a rule bounded the action and
    // when no rule existed. Until 2026-08-21 both recorded as `Allowed`, so the
    // evidence log could not answer the question a safety case is for: was this
    // actuation actually checked against a limit?

    #[test]
    fn an_action_a_rule_covers_is_recorded_as_checked() {
        let gate = one_rule();
        let d = recorded(
            "covered",
            Some(&gate),
            &json!({"node_id": "local", "pin": 17, "value": 1}),
        );
        assert_eq!(d, Decision::Allowed, "a bounded action records as Allowed");
    }

    #[test]
    fn an_action_no_rule_covers_is_not_recorded_as_checked() {
        let gate = one_rule();
        // Same gate, a node it has never heard of. Allowed either way — the
        // point is that the record no longer claims it was checked.
        let d = recorded(
            "uncovered",
            Some(&gate),
            &json!({"node_id": "arm-2", "pin": 99, "value": 1}),
        );
        match d {
            Decision::AllowedUncovered(why) => assert!(
                why.contains("arm-2"),
                "the record should name what was uncovered: {why}"
            ),
            other => panic!("an ungated actuation recorded as {other:?}"),
        }
    }

    #[test]
    fn with_no_gate_at_all_the_record_says_so() {
        let d = recorded("nogate", None, &json!({"pin": 4, "value": 1}));
        assert_eq!(
            d,
            Decision::AllowedUncovered("no safety gate configured".to_string())
        );
    }

    #[test]
    fn an_ungated_action_is_still_allowed() {
        // This change is about the record, not the decision. A flat flip to
        // deny-on-unmatched would stop every physical tool on every node with
        // no explicit rule, which is a different change and not this one.
        let gate = one_rule();
        let r = track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            RiskClass::physical(false, BlastRadius::High),
            &json!({"node_id": "arm-2", "pin": 99, "value": 1}),
        );
        assert!(r.is_ok(), "uncovered still means allowed: {r:?}");
    }

    #[test]
    fn track0_passes_nonphysical_tools() {
        // A normal (non-physical) tool is never gated.
        let r = track0_authorize(None, None, "shell", RiskClass::safe(), &json!({}));
        assert!(r.is_ok());
    }

    #[test]
    fn track0_gate_allows_in_policy_and_refuses_out_of_policy() {
        let gate = SafetyGate::new(vec![SafetyLimit {
            node_id: "local".into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![17]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: None,
        }]);
        let risk = RiskClass::physical(false, BlastRadius::High);

        // In-policy pin/value is allowed.
        assert!(track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            risk,
            &json!({"pin": 17, "value": 1})
        )
        .is_ok());

        // Out-of-policy pin is refused (and the reason is surfaced).
        let denied = track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            risk,
            &json!({"pin": 99, "value": 1}),
        );
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("pin"));
    }

    #[test]
    fn track0_without_gate_allows_physical() {
        // No gate configured → deterministic layer is permissive (approval governs).
        let r = track0_authorize(
            None,
            None,
            "gpio_write",
            RiskClass::physical(false, BlastRadius::High),
            &json!({"pin": 99, "value": 1}),
        );
        assert!(r.is_ok());
    }

    /// The reason `check_physical_tools.py` exists, asserted rather than argued.
    ///
    /// A tool that declares itself non-physical is not merely un-limited: the
    /// function returns before the gate is consulted at all, so a pin the gate
    /// would refuse goes through. Same gate, same pin, same call — the only
    /// difference is what the tool said about itself.
    #[test]
    fn a_tool_that_understates_its_risk_is_never_gated() {
        let gate = SafetyGate::new(vec![SafetyLimit {
            node_id: "local".into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![17]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: None,
        }]);
        let args = json!({"pin": 99, "value": 1});

        // Declared physical: refused.
        assert!(track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            RiskClass::physical(false, BlastRadius::High),
            &args
        )
        .is_err());

        // Same call, declared safe: not refused, because it was never checked.
        assert!(
            track0_authorize(Some(&gate), None, "gpio_write", RiskClass::safe(), &args).is_ok()
        );
    }
}

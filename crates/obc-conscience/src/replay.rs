//! Deterministic replay of conscience decisions — after-the-fact review.
//!
//! Both gates are **pure functions of (input, config)**: perception is
//! [`crate::Conscience::may_perceive_label`] over `(label, confidence)`, reach is
//! [`crate::ReachGate::check`] over `(tool, host)`. Nothing about a decision
//! depends on wall-clock, ordering, or hidden state. That is what makes replay
//! possible: given the inputs a decision was made on, you can re-run it and get
//! the same verdict — every time, from anyone's machine.
//!
//! Two things this buys an auditor:
//!
//! 1. **Determinism as a checkable property.** Replay a decision log under the
//!    *same* config and every verdict must match. A mismatch there is a bug — a
//!    gate that isn't a pure function — and this is how you'd catch it.
//! 2. **Drift attribution.** Replay under the *current* config and a mismatch
//!    means the policy changed between then and now: "this human was refused in
//!    June; under today's registry they'd be allowed." Each record can carry a
//!    fingerprint of the config in effect when it was made, so replay can say
//!    *whether the policy moved*, not just that the answer did.
//!
//! The record type ([`DecisionRecord`]) captures the **full input** — including
//! the perception confidence, which the refusal-only audit entry does not — so a
//! decision log written from these is sufficient to replay. Wiring the runtime
//! to persist such a log is the remaining step; this module is the mechanism and
//! is exercised on synthetic logs in its tests.

use crate::{Conscience, ConscienceConfig, PerceptionDecision, ReachDecision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The full input a conscience decision was made on — enough to replay it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionInput {
    /// A perception decision: classify `label` and gate on the class, with the
    /// detector's `confidence` (the input the refusal audit entry omits).
    Perception { label: String, confidence: f32 },
    /// An egress reach decision: is `tool` allowed to reach `host`?
    Reach { tool: String, host: String },
}

/// The canonical outcome of a decision: allowed or not, and the reason when not.
/// Comparison for replay is on `allowed` — the safety-relevant bit — while the
/// reason is kept for display and stricter diffing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One recorded decision: when, on what input, what was decided, and (optionally)
/// a fingerprint of the config in force at the time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub ts: u64,
    pub input: DecisionInput,
    pub verdict: Verdict,
    /// Fingerprint of the [`ConscienceConfig`] in effect when this was decided.
    /// Lets replay distinguish "policy changed" from "gate is non-deterministic".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_fingerprint: Option<String>,
}

/// Evaluate an input against a live conscience — the pure decision function that
/// replay re-runs. This is the *same* call the runtime gates make, so a verdict
/// from here is byte-for-byte the verdict the gate would produce.
pub fn evaluate(input: &DecisionInput, conscience: &Conscience) -> Verdict {
    match input {
        DecisionInput::Perception { label, confidence } => {
            match conscience.may_perceive_label(label, *confidence) {
                PerceptionDecision::Allow { .. } => Verdict {
                    allowed: true,
                    reason: None,
                },
                PerceptionDecision::Refuse(r) => Verdict {
                    allowed: false,
                    reason: Some(r.to_string()),
                },
            }
        }
        // `may_reach`, not `reach.check` directly: it honors the `enabled` flag
        // the same way `may_perceive_label` does, so a disabled conscience
        // replays as Allow on BOTH gates — matching the runtime, which only
        // attaches the reach gate when conscience is enabled.
        DecisionInput::Reach { tool, host } => match conscience.may_reach(tool, host) {
            ReachDecision::Allow { .. } => Verdict {
                allowed: true,
                reason: None,
            },
            ReachDecision::Refuse(r) => Verdict {
                allowed: false,
                reason: Some(r.to_string()),
            },
        },
    }
}

/// Make a decision record *now*: evaluate `input` under `conscience` and stamp it
/// with `ts` and the fingerprint of `config`. This is what a runtime would append
/// to a decision log so the decision can be replayed later.
pub fn record(
    ts: u64,
    input: DecisionInput,
    conscience: &Conscience,
    config: &ConscienceConfig,
) -> DecisionRecord {
    DecisionRecord {
        verdict: evaluate(&input, conscience),
        ts,
        input,
        config_fingerprint: Some(fingerprint(config)),
    }
}

/// A recorded verdict that no longer matches what the current policy produces.
#[derive(Debug, Clone)]
pub struct Mismatch {
    pub record: DecisionRecord,
    /// What the decision function produces now.
    pub recomputed: Verdict,
    /// The record carried a config fingerprint and it differs from the one being
    /// replayed against — the mismatch is explained by a policy change, not a
    /// non-deterministic gate. `false` when fingerprints match (or none was
    /// recorded), which for a same-config replay means a real determinism bug.
    pub config_drift: bool,
}

/// The result of replaying a decision log.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub total: u64,
    pub matched: u64,
    pub mismatches: Vec<Mismatch>,
    /// Fingerprint of the config the log was replayed against.
    pub current_fingerprint: String,
}

impl ReplayReport {
    /// True when every recorded verdict reproduced — the log is consistent with
    /// the config it was replayed against.
    pub fn all_matched(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// Mismatches NOT explained by a config change — i.e. the record claimed the
    /// same policy yet a different verdict. These are the alarming ones: a pure
    /// function disagreeing with itself.
    pub fn undrifted_mismatches(&self) -> usize {
        self.mismatches.iter().filter(|m| !m.config_drift).count()
    }
}

impl std::fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Conscience decision replay: {}/{} reproduced (config {})",
            self.matched, self.total, self.current_fingerprint
        )?;
        if self.mismatches.is_empty() {
            writeln!(f, "  ✓ every recorded decision replays identically.")?;
            return Ok(());
        }
        let drift = self.mismatches.len() - self.undrifted_mismatches();
        writeln!(
            f,
            "  {} mismatch(es): {} from policy drift, {} UNEXPLAINED (same config, \
             different verdict — investigate).",
            self.mismatches.len(),
            drift,
            self.undrifted_mismatches()
        )?;
        for m in &self.mismatches {
            let tag = if m.config_drift { "drift" } else { "‼ BUG" };
            writeln!(
                f,
                "  [{}] {:?}: recorded allowed={}, now allowed={}",
                tag,
                m.input_summary(),
                m.record.verdict.allowed,
                m.recomputed.allowed
            )?;
        }
        Ok(())
    }
}

impl Mismatch {
    fn input_summary(&self) -> String {
        match &self.record.input {
            DecisionInput::Perception { label, confidence } => {
                format!("perceive '{label}'@{confidence:.2}")
            }
            DecisionInput::Reach { tool, host } => format!("{tool}→{host}"),
        }
    }
}

/// Replay a decision log against a conscience + its config. Each record's input is
/// re-evaluated; a record "matches" when the recomputed `allowed` equals the
/// recorded one. Mismatches are flagged as config drift when the record's
/// fingerprint differs from the current one.
pub fn replay(
    records: &[DecisionRecord],
    conscience: &Conscience,
    config: &ConscienceConfig,
) -> ReplayReport {
    let current = fingerprint(config);
    let mut matched = 0u64;
    let mut mismatches = Vec::new();
    for rec in records {
        let recomputed = evaluate(&rec.input, conscience);
        if recomputed.allowed == rec.verdict.allowed {
            matched += 1;
        } else {
            let config_drift = rec
                .config_fingerprint
                .as_ref()
                .is_some_and(|fp| fp != &current);
            mismatches.push(Mismatch {
                record: rec.clone(),
                recomputed,
                config_drift,
            });
        }
    }
    ReplayReport {
        total: records.len() as u64,
        matched,
        mismatches,
        current_fingerprint: current,
    }
}

/// A stable, dependency-free fingerprint of a conscience config: FNV-1a over the
/// config's **canonical** JSON (object keys sorted recursively, so a `HashMap`'s
/// arbitrary iteration order can't change the hash). Same config → same
/// fingerprint on any machine, any run; a single changed rule → a different one.
pub fn fingerprint(config: &ConscienceConfig) -> String {
    let value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
    let canonical = canonical_json(&value);
    format!("cfg-{:016x}", fnv1a64(canonical.as_bytes()))
}

/// Serialize a JSON value with object keys sorted recursively — a canonical form
/// so semantically-equal configs hash equal regardless of map iteration order.
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = map.iter().collect();
            let inner: Vec<String> = sorted
                .iter()
                .map(|(k, val)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(val)
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// FNV-1a 64-bit — a small, fast, stable non-cryptographic hash. Fine for a
/// config fingerprint (collision resistance is not a security property here; this
/// detects change, it does not authenticate — that is the audit chain's job).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x00000100000001B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConsentRule, Transmit};

    /// A conscience that allows wildlife perception and denies humans (default),
    /// with a config we can fingerprint. Confidence gating is the global
    /// `confidence_threshold` (default 0.6), so a low-confidence deer is refused.
    fn wildlife_conscience() -> (Conscience, ConscienceConfig) {
        let cfg = ConscienceConfig {
            enabled: true,
            subjects: vec![ConsentRule::allow("wildlife", 30, Transmit::WeightsOnly)],
            ..Default::default()
        };
        (Conscience::new(&cfg), cfg)
    }

    fn perceive(label: &str, conf: f32) -> DecisionInput {
        DecisionInput::Perception {
            label: label.to_string(),
            confidence: conf,
        }
    }

    #[test]
    fn same_config_replays_identically() {
        let (c, cfg) = wildlife_conscience();
        // Build records from live decisions, then replay under the same config.
        let inputs = vec![
            perceive("deer", 0.9),   // allowed (wildlife, conf ok)
            perceive("person", 0.9), // refused (human, default-deny)
            perceive("deer", 0.1),   // refused (below min_confidence)
            perceive("drone", 0.9),  // refused (unrecognized → fail closed)
        ];
        let records: Vec<DecisionRecord> = inputs
            .into_iter()
            .map(|i| record(1_000, i, &c, &cfg))
            .collect();

        let report = replay(&records, &c, &cfg);
        assert!(
            report.all_matched(),
            "pure gate must reproduce every verdict"
        );
        assert_eq!(report.matched, 4);
        assert_eq!(report.undrifted_mismatches(), 0);
    }

    #[test]
    fn a_recorded_verdict_matches_the_expected_decision() {
        let (c, cfg) = wildlife_conscience();
        let r = record(1, perceive("deer", 0.9), &c, &cfg);
        assert!(r.verdict.allowed);
        let r = record(1, perceive("person", 0.9), &c, &cfg);
        assert!(!r.verdict.allowed);
        assert!(r.verdict.reason.is_some());
    }

    #[test]
    fn config_change_is_flagged_as_drift_not_a_bug() {
        // Record under a wildlife-allowing policy...
        let (c1, cfg1) = wildlife_conscience();
        let rec = record(1_000, perceive("deer", 0.9), &c1, &cfg1);
        assert!(rec.verdict.allowed);

        // ...then replay under a policy that allows nothing (deer now refused).
        let cfg2 = ConscienceConfig {
            enabled: true, // no subjects → default-deny everything
            ..Default::default()
        };
        let c2 = Conscience::new(&cfg2);

        let report = replay(&[rec], &c2, &cfg2);
        assert_eq!(report.matched, 0);
        assert_eq!(report.mismatches.len(), 1);
        assert!(
            report.mismatches[0].config_drift,
            "different fingerprint → mismatch attributed to policy drift"
        );
        assert_eq!(report.undrifted_mismatches(), 0, "not a determinism bug");
    }

    #[test]
    fn a_verdict_that_flips_under_the_same_fingerprint_is_an_unexplained_bug() {
        // Simulate a corrupted/incorrect log: a record claims 'person' was ALLOWED
        // under the current config's fingerprint. The gate says refused → an
        // unexplained mismatch (same policy, different verdict).
        let (c, cfg) = wildlife_conscience();
        let bad = DecisionRecord {
            ts: 1,
            input: perceive("person", 0.9),
            verdict: Verdict {
                allowed: true,
                reason: None,
            }, // wrong
            config_fingerprint: Some(fingerprint(&cfg)), // claims same policy
        };
        let report = replay(&[bad], &c, &cfg);
        assert_eq!(report.mismatches.len(), 1);
        assert!(!report.mismatches[0].config_drift);
        assert_eq!(
            report.undrifted_mismatches(),
            1,
            "flagged for investigation"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_change_sensitive() {
        let (_, cfg) = wildlife_conscience();
        let a = fingerprint(&cfg);
        let b = fingerprint(&cfg);
        assert_eq!(a, b, "same config → same fingerprint");

        let mut cfg2 = cfg.clone();
        cfg2.confidence_threshold = 0.99; // one setting changed
        assert_ne!(
            a,
            fingerprint(&cfg2),
            "a changed setting changes the fingerprint"
        );
    }

    #[test]
    fn reach_decisions_replay_too() {
        use crate::{HostRule, ReachScope, ToolReach};
        let cfg = ConscienceConfig {
            enabled: true,
            hosts: vec![HostRule {
                host: "api.allowed.example".to_string(),
                purpose: "test".to_string(),
                credential: None,
            }],
            tools: vec![ToolReach {
                tool: "http".to_string(),
                scope: ReachScope::Egress,
            }],
            ..Default::default()
        };
        let c = Conscience::new(&cfg);

        let recs = vec![
            record(
                1,
                DecisionInput::Reach {
                    tool: "http".into(),
                    host: "api.allowed.example".into(),
                },
                &c,
                &cfg,
            ),
            record(
                2,
                DecisionInput::Reach {
                    tool: "http".into(),
                    host: "evil.example".into(),
                },
                &c,
                &cfg,
            ),
        ];
        assert!(recs[0].verdict.allowed);
        assert!(!recs[1].verdict.allowed);
        assert!(replay(&recs, &c, &cfg).all_matched());
    }

    #[test]
    fn records_round_trip_through_json() {
        let (c, cfg) = wildlife_conscience();
        let recs = vec![
            record(1, perceive("deer", 0.9), &c, &cfg),
            record(
                2,
                DecisionInput::Reach {
                    tool: "http".into(),
                    host: "h".into(),
                },
                &c,
                &cfg,
            ),
        ];
        let json = serde_json::to_string(&recs).unwrap();
        let back: Vec<DecisionRecord> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 2);
        // Replays identically after a round-trip through the log format.
        assert!(replay(&back, &c, &cfg).all_matched());
    }
}

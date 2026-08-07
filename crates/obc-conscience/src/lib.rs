//! Open Body Control — Conscience layer.
//!
//! Track 0 ([`obc_safety`](../obc_safety)) answers one question completely: can
//! the model move a motor it shouldn't? No — a deterministic limit table on the
//! host and mirrored on the MCU refuses out-of-range actuation regardless of
//! how the model was reasoned into asking.
//!
//! Conscience answers the two questions Track 0 is silent on:
//!
//! - **Can the model *watch* a subject it shouldn't?** ([`consent`]) — a
//!   consent registry, default-deny for humans, checked before any frame
//!   reaches the reasoner.
//! - **Can the model *reach* a system it shouldn't?** ([`reach`]) — an egress
//!   allowlist, default-deny, credentials by name, scoped and never standing.
//!
//! Same conviction as the safety layer: the model proposes what to observe and
//! what to touch; deterministic code disposes. The model is not the privacy
//! function and not the access-control function, for the same reason it is not
//! the safety function — threat "model wrong" and threat "model/host
//! compromised" are the same problem, so enforcement lives in neither.
//!
//! Spec: `OBC-Prime/docs/CONSCIENCE.md`. Like Track 0, refusals are
//! first-class: every decision here is meant to be written to the same
//! tamper-evident audit log (`obc_safety::audit`) by the caller — a conscience
//! that isn't audited is just a promise.
//!
//! ## Honest limits (see spec §4/§6)
//! The perception classifier ([`classifier`]) is a heuristic label→class map,
//! fail-closed on any label it doesn't recognize; it is not a proof and a
//! determined adversary or an unlucky angle can still produce a wrong call. It
//! cannot stop an operator with physical control from
//! surveilling — it makes that act explicit, effortful, and logged, not
//! impossible. Semantic consent ("this person agreed, that guest didn't") is not
//! expressible by *class* — it is handled by the [`multiparty`] layer, which
//! gates on an affirmative consent token the subject presents rather than a
//! recognized identity (opt-in, fail-closed). These are labeled, not hidden.

pub mod classifier;
pub mod consent;
pub mod detector_eval;
pub mod multiparty;
pub mod reach;
pub mod replay;

pub use classifier::{Classification, ClassifierConfig, SubjectClassifier};
pub use consent::{ConsentRule, PerceptionDecision, PerceptionGate, PerceptionRefusal, Transmit};
pub use detector_eval::{measure as measure_false_negatives, ClassStats, EvalFrame, FnReport};
pub use multiparty::{
    decide_frame, ConsentGrant, ConsentLedger, ConsentPolicy, FrameConsent, SubjectPresence,
};
pub use reach::{HostRule, ReachDecision, ReachGate, ReachRefusal, ReachScope, ToolReach};
pub use replay::{
    fingerprint as config_fingerprint, replay as replay_decisions, DecisionInput, DecisionRecord,
    Mismatch, ReplayReport, Verdict,
};

use serde::{Deserialize, Serialize};

/// Default classifier-confidence threshold below which perception fails closed.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.60;

/// Conscience configuration (root `[conscience]` config section).
///
/// Like `[safety]`, unknown keys are rejected: a typo here must not silently
/// remove a privacy or access control while startup logs the gate as active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConscienceConfig {
    /// Enable the conscience gates. When off, NEITHER gate constrains — a
    /// deployment that never configures conscience has no gate, exactly the
    /// caveat the spec names. Reference Bodies ship it armed.
    #[serde(default)]
    pub enabled: bool,
    /// Consent registry (perception gate). Empty ⇒ deny all perception.
    #[serde(default)]
    pub subjects: Vec<ConsentRule>,
    /// Egress allowlist (reach gate). Empty ⇒ deny all egress.
    #[serde(default)]
    pub hosts: Vec<HostRule>,
    /// Per-tool reach scopes. Unlisted tools default to no reach.
    #[serde(default)]
    pub tools: Vec<ToolReach>,
    /// Classifier-confidence threshold for the perception gate.
    #[serde(default = "default_threshold")]
    pub confidence_threshold: f32,
    /// Perception classifier: maps raw detector labels onto broad consent
    /// classes, failing closed on any label it does not recognize.
    #[serde(default)]
    pub classifier: ClassifierConfig,
}

fn default_threshold() -> f32 {
    DEFAULT_CONFIDENCE_THRESHOLD
}

impl Default for ConscienceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            subjects: Vec::new(),
            hosts: Vec::new(),
            tools: Vec::new(),
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            classifier: ClassifierConfig::default(),
        }
    }
}

/// The assembled conscience: a perception gate + a reach gate.
#[derive(Debug, Clone)]
pub struct Conscience {
    pub enabled: bool,
    pub perception: PerceptionGate,
    pub reach: ReachGate,
    pub classifier: SubjectClassifier,
}

impl Conscience {
    /// Build from config.
    pub fn new(config: &ConscienceConfig) -> Self {
        Self {
            enabled: config.enabled,
            perception: PerceptionGate::new(config.subjects.clone(), config.confidence_threshold),
            reach: ReachGate::new(config.hosts.clone(), config.tools.clone()),
            classifier: SubjectClassifier::from_config(&config.classifier),
        }
    }

    /// May a detected subject be perceived? When conscience is disabled the
    /// gate does not constrain (returns Allow) — the deployment took that
    /// choice explicitly; the spec's strong recommendation is to leave it on.
    pub fn may_perceive(&self, class: &str, confidence: f32) -> PerceptionDecision {
        if !self.enabled {
            return PerceptionDecision::Allow {
                retain_days: 0,
                transmit: Transmit::None,
            };
        }
        self.perception.check(class, confidence)
    }

    /// May a detected subject be perceived, given the detector's RAW label?
    /// Classifies the label into a consent class first, fail-closed: a label the
    /// classifier does not recognize is refused outright (never mapped onto a
    /// permitted class, even on a permissive body). This is the entry point the
    /// perception ingest boundary should use — the gap the spec §6 named as the
    /// classifier being unbuilt is now closed here.
    pub fn may_perceive_label(&self, label: &str, confidence: f32) -> PerceptionDecision {
        if !self.enabled {
            return PerceptionDecision::Allow {
                retain_days: 0,
                transmit: Transmit::None,
            };
        }
        let c = self.classifier.classify(label);
        if !c.recognized {
            return PerceptionDecision::Refuse(PerceptionRefusal::Unrecognized {
                label: label.to_string(),
            });
        }
        self.perception.check(&c.class, confidence)
    }

    /// May `tool` reach `host`?
    pub fn may_reach(&self, tool: &str, host: &str) -> ReachDecision {
        if !self.enabled {
            return ReachDecision::Allow { credential: None };
        }
        self.reach.check(tool, host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_unknown_config_keys() {
        // A typo in the safety-adjacent config must fail loudly, not silently disarm.
        let bad = r#"{"enabled": true, "subjekts": []}"#;
        assert!(serde_json::from_str::<ConscienceConfig>(bad).is_err());
    }

    #[test]
    fn armed_conscience_denies_humans_and_unlisted_egress() {
        let cfg = ConscienceConfig {
            enabled: true,
            subjects: vec![
                ConsentRule::allow("wildlife", 30, Transmit::WeightsOnly),
                ConsentRule::deny("human"),
            ],
            hosts: vec![HostRule {
                host: "api.anthropic.com".into(),
                purpose: "brain".into(),
                credential: Some("brain-key".into()),
            }],
            tools: vec![ToolReach {
                tool: "brain".into(),
                scope: ReachScope::Egress,
            }],
            confidence_threshold: 0.6,
            classifier: Default::default(),
        };
        let c = Conscience::new(&cfg);
        assert!(c.may_perceive("wildlife", 0.9).is_allowed());
        assert!(!c.may_perceive("human", 0.99).is_allowed());
        assert!(c.may_reach("brain", "api.anthropic.com").is_allowed());
        assert!(!c.may_reach("brain", "evil.example.com").is_allowed());
    }

    #[test]
    fn disabled_conscience_does_not_constrain() {
        let c = Conscience::new(&ConscienceConfig::default());
        assert!(c.may_perceive("human", 0.99).is_allowed());
        assert!(c.may_reach("anything", "anywhere").is_allowed());
    }

    #[test]
    fn config_roundtrips_json() {
        let cfg = ConscienceConfig {
            enabled: true,
            subjects: vec![ConsentRule::deny("human")],
            ..Default::default()
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: ConscienceConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, back);
    }
}

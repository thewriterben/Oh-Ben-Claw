//! Vision-driven reflexes & foresight — make ClawCam detections *act*.
//!
//! Detections folded into world memory (`vision.subject.{species}` by
//! [`super::clawcam_ingest`], with a rolling `vision.count.{species}` for trends)
//! are inert until something keys on them. This module authors two rule libraries
//! that plug straight into the existing engines:
//!
//!  * **Reflexes (System 1)** — a verified sighting of an *alert subject* (e.g. a
//!    person) escalates immediately, and can optionally fire a `clawcam/cmd/capture`
//!    publish (handled by [`super::clawcam_actuate`]) to grab a fresh frame.
//!  * **Foresight (Track 1)** — the per-subject sighting **rate** is trended, so a
//!    *rising* intrusion rate escalates *before* it peaks.
//!
//! Both reuse the exact rule types the safing library uses, so they merge into the
//! live reflex/foresight controllers with no engine changes and remain bounded by
//! Track 0 / the escalation budget.

use crate::agent::reflex::{Action, Cmp, Condition, ReflexRule};
use crate::foresight::ForesightRule;
use serde_json::json;

/// How to turn detections into behavior.
#[derive(Debug, Clone)]
pub struct VisionRuleOptions {
    /// Subjects that warrant an alert (e.g. `["person", "vehicle"]`). Each maps to
    /// the `vision.subject.{subject}` entity.
    pub alert_subjects: Vec<String>,
    /// Review state a sighting must carry to count as confirmed. Default `verified`.
    pub require_state: String,
    /// Minimum ms between re-fires of a given rule.
    pub debounce_ms: u64,
    /// If set, alert reflexes also publish a `clawcam/cmd/capture` for this camera
    /// node (a static reflex can't know which camera fired, so capture targets a
    /// configured default). `None` disables capture-on-alert.
    pub capture_node: Option<String>,
    /// Foresight: escalate when a subject's sighting count is predicted within
    /// `horizon_ms` to reach `rate_threshold` more sightings.
    pub rate_threshold: f64,
    pub horizon_ms: u64,
}

impl Default for VisionRuleOptions {
    fn default() -> Self {
        Self {
            alert_subjects: vec!["person".to_string()],
            require_state: "verified".to_string(),
            debounce_ms: 10_000,
            capture_node: None,
            rate_threshold: 5.0,
            horizon_ms: 60_000,
        }
    }
}

/// Reflex rules: a confirmed sighting of an alert subject escalates (and optionally
/// captures). Merge into the live `ReflexEngine` alongside the safing rules.
pub fn vision_security_rules(opts: &VisionRuleOptions) -> Vec<ReflexRule> {
    let mut rules = Vec::new();
    for subject in &opts.alert_subjects {
        let entity = format!("vision.subject.{subject}");
        let when = Condition::State {
            entity: entity.clone(),
            field: Some("review_state".to_string()),
            equals: opts.require_state.clone(),
        };
        rules.push(ReflexRule {
            id: format!("vision-alert-{subject}"),
            when: when.clone(),
            then: Action::Escalate {
                reason: format!("{subject} detected ({}) on a camera", opts.require_state),
            },
            debounce_ms: opts.debounce_ms,
            max_rate_hz: None,
        });
        if let Some(node) = &opts.capture_node {
            rules.push(ReflexRule {
                id: format!("vision-capture-{subject}"),
                when,
                then: Action::Publish {
                    topic: "clawcam/cmd/capture".to_string(),
                    payload: json!({ "node": node, "reason": subject }),
                },
                debounce_ms: opts.debounce_ms,
                max_rate_hz: None,
            });
        }
    }
    rules
}

/// Foresight rules: escalate when a subject's sighting **rate** is climbing —
/// predicted to add `rate_threshold` sightings within `horizon_ms`. Merge into the
/// `ForesightEngine`.
pub fn vision_foresight_rules(opts: &VisionRuleOptions) -> Vec<ForesightRule> {
    opts.alert_subjects
        .iter()
        .map(|subject| {
            let current_floor = opts.rate_threshold; // relative-rise handled by the trend
            ForesightRule {
                id: format!("vision-rate-{subject}"),
                entity: format!("vision.count.{subject}"),
                op: Cmp::Ge,
                threshold: current_floor,
                horizon_ms: opts.horizon_ms,
                then: Action::Escalate {
                    reason: format!("{subject} detection rate rising"),
                },
                debounce_ms: opts.debounce_ms,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_subject_yields_an_escalating_reflex() {
        let opts = VisionRuleOptions {
            alert_subjects: vec!["person".into()],
            ..Default::default()
        };
        let rules = vision_security_rules(&opts);
        assert_eq!(rules.len(), 1, "no capture node ⇒ just the escalate rule");
        let r = &rules[0];
        assert_eq!(r.id, "vision-alert-person");
        assert!(matches!(r.then, Action::Escalate { .. }));
        match &r.when {
            Condition::State { entity, field, equals } => {
                assert_eq!(entity, "vision.subject.person");
                assert_eq!(field.as_deref(), Some("review_state"));
                assert_eq!(equals, "verified");
            }
            other => panic!("expected a State condition, got {other:?}"),
        }
    }

    #[test]
    fn capture_node_adds_a_capture_publish_rule() {
        let opts = VisionRuleOptions {
            alert_subjects: vec!["person".into()],
            capture_node: Some("cam-door".into()),
            ..Default::default()
        };
        let rules = vision_security_rules(&opts);
        assert_eq!(rules.len(), 2, "escalate + capture");
        let cap = rules.iter().find(|r| r.id == "vision-capture-person").unwrap();
        match &cap.then {
            Action::Publish { topic, payload } => {
                assert_eq!(topic, "clawcam/cmd/capture");
                assert_eq!(payload["node"], "cam-door");
            }
            other => panic!("expected a Publish, got {other:?}"),
        }
    }

    #[test]
    fn foresight_rule_trends_the_sighting_count() {
        let opts = VisionRuleOptions {
            alert_subjects: vec!["vehicle".into()],
            ..Default::default()
        };
        let rules = vision_foresight_rules(&opts);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].entity, "vision.count.vehicle");
        assert!(matches!(rules[0].op, Cmp::Ge));
    }
}

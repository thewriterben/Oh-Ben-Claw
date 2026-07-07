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

// ── Analytics-driven reflexes ("today is weird") ─────────────────────────────

/// How to turn the `clawcam.analytics.*` facts (folded in by
/// [`super::clawcam_analytics`]) into behavior.
#[derive(Debug, Clone)]
pub struct AnalyticsRuleOptions {
    /// |z| at/above which the latest day's detection count escalates: a drop
    /// (`z <= -z_alert`) reads as a possibly knocked-over/obstructed camera or
    /// dead PIR; a spike (`z >= z_alert`) as a surge worth System 2's attention.
    pub z_alert: f64,
    /// Minimum ms between re-fires. Analytics facts change on a daily scale, so
    /// this defaults to hours, not seconds.
    pub debounce_ms: u64,
}

impl Default for AnalyticsRuleOptions {
    fn default() -> Self {
        Self {
            z_alert: 2.0,
            debounce_ms: 21_600_000, // 6 h
        }
    }
}

/// Reflex rules keyed on the analytics facts: an unusually **quiet** day and an
/// unusually **busy** day both escalate (with drop framed as a possible camera
/// fault — the actionable reading), and a **miscalibrated** model escalates so
/// alert thresholds get retuned before they mislead. Merge into the live
/// `ReflexEngine` alongside the safing and vision-security rules.
pub fn vision_analytics_rules(opts: &AnalyticsRuleOptions) -> Vec<ReflexRule> {
    let z = opts.z_alert.abs();
    vec![
        ReflexRule {
            id: "vision-anomaly-drop".to_string(),
            when: Condition::Sensor {
                entity: super::clawcam_analytics::ANOMALY_ENTITY.to_string(),
                op: Cmp::Le,
                value: -z,
            },
            then: Action::Escalate {
                reason: format!(
                    "Unusually quiet day on the cameras (z <= -{z}) — a drop is the \
                     signature of a knocked-over / obstructed camera or a dead PIR. \
                     Triage: (1) `get_anomaly_report` to confirm the day and how deep the \
                     drop; (2) `get_node_health` — a camera offline or on low battery \
                     explains a silent feed; (3) `get_site_report` to see if it's \
                     site-wide or one subject. Read-only tools; any node action stays \
                     Track-0 gated."
                ),
            },
            debounce_ms: opts.debounce_ms,
            max_rate_hz: None,
        },
        ReflexRule {
            id: "vision-anomaly-spike".to_string(),
            when: Condition::Sensor {
                entity: super::clawcam_analytics::ANOMALY_ENTITY.to_string(),
                op: Cmp::Ge,
                value: z,
            },
            then: Action::Escalate {
                reason: format!(
                    "Unusually busy day on the cameras (z >= {z}) — an activity surge. \
                     Triage: (1) `get_anomaly_report` for the day and its magnitude; (2) \
                     `get_site_report` for which species and whether it's rising; (3) a \
                     genuine surge may warrant an operator alert. Read-only tools; any \
                     node action stays Track-0 gated."
                ),
            },
            debounce_ms: opts.debounce_ms,
            max_rate_hz: None,
        },
        ReflexRule {
            id: "vision-calibration-drift".to_string(),
            when: Condition::State {
                entity: super::clawcam_analytics::CALIBRATION_ENTITY.to_string(),
                field: Some("well_calibrated".to_string()),
                equals: "false".to_string(),
            },
            then: Action::Escalate {
                reason: "Camera model confidence disagrees with human review \
                         (miscalibrated). Triage: (1) `get_calibration_report` for the \
                         suggested accept threshold and current precision; (2) retune \
                         detection / alert thresholds toward that threshold so alerts \
                         stop misleading; (3) `get_review_queue` to clear the ambiguous \
                         backlog driving the disagreement. Read-only tools; any threshold \
                         change stays operator-gated."
                    .to_string(),
            },
            debounce_ms: opts.debounce_ms,
            max_rate_hz: None,
        },
    ]
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

    #[test]
    fn analytics_rules_cover_drop_spike_and_calibration() {
        let rules = vision_analytics_rules(&AnalyticsRuleOptions::default());
        assert_eq!(rules.len(), 3);
        // Drop rule: z <= -2.0 on the anomaly fact, escalating.
        let drop = rules.iter().find(|r| r.id == "vision-anomaly-drop").unwrap();
        match &drop.when {
            Condition::Sensor { entity, op, value } => {
                assert_eq!(entity, super::super::clawcam_analytics::ANOMALY_ENTITY);
                assert!(matches!(op, Cmp::Le));
                assert!((value + 2.0).abs() < 1e-9);
            }
            other => panic!("expected a Sensor condition, got {other:?}"),
        }
        assert!(matches!(drop.then, Action::Escalate { .. }));
        // Spike rule mirrors it at +z.
        let spike = rules.iter().find(|r| r.id == "vision-anomaly-spike").unwrap();
        match &spike.when {
            Condition::Sensor { op, value, .. } => {
                assert!(matches!(op, Cmp::Ge));
                assert!((value - 2.0).abs() < 1e-9);
            }
            other => panic!("expected a Sensor condition, got {other:?}"),
        }
        // Calibration rule matches the string "false" (the State contract).
        let cal = rules.iter().find(|r| r.id == "vision-calibration-drift").unwrap();
        match &cal.when {
            Condition::State { entity, field, equals } => {
                assert_eq!(entity, super::super::clawcam_analytics::CALIBRATION_ENTITY);
                assert_eq!(field.as_deref(), Some("well_calibrated"));
                assert_eq!(equals, "false");
            }
            other => panic!("expected a State condition, got {other:?}"),
        }
    }

    #[test]
    fn analytics_reasons_are_self_guiding_triage_directives() {
        // Every analytics wake should name the read-only tools the agent runs next and
        // reaffirm the safety gate — the mesh-playbook standard, so a wake is actionable.
        let rules = vision_analytics_rules(&AnalyticsRuleOptions::default());
        let reason = |id: &str| match &rules.iter().find(|r| r.id == id).unwrap().then {
            Action::Escalate { reason } => reason.clone(),
            other => panic!("expected an escalate action, got {other:?}"),
        };

        let drop = reason("vision-anomaly-drop");
        assert!(drop.contains("get_anomaly_report"), "drop names the anomaly tool");
        assert!(drop.contains("get_node_health"), "drop points at camera health");
        assert!(drop.contains("Track-0"), "drop reaffirms the safety gate");

        let spike = reason("vision-anomaly-spike");
        assert!(spike.contains("get_anomaly_report"), "spike names the anomaly tool");
        assert!(spike.contains("get_site_report"), "spike points at the fuller picture");

        let cal = reason("vision-calibration-drift");
        assert!(cal.contains("get_calibration_report"), "calibration names its report");
        assert!(cal.contains("get_review_queue"), "calibration points at the review backlog");
        assert!(cal.contains("operator-gated"), "calibration keeps threshold changes gated");
    }

    #[test]
    fn analytics_z_alert_is_symmetric_even_if_negative_configured() {
        let rules = vision_analytics_rules(&AnalyticsRuleOptions {
            z_alert: -3.0,
            ..Default::default()
        });
        let drop = rules.iter().find(|r| r.id == "vision-anomaly-drop").unwrap();
        let spike = rules.iter().find(|r| r.id == "vision-anomaly-spike").unwrap();
        match (&drop.when, &spike.when) {
            (
                Condition::Sensor { value: d, .. },
                Condition::Sensor { value: s, .. },
            ) => {
                assert!((d + 3.0).abs() < 1e-9);
                assert!((s - 3.0).abs() < 1e-9);
            }
            other => panic!("expected Sensor conditions, got {other:?}"),
        }
    }
}

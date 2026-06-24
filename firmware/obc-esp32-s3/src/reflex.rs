//! On-MCU reflex mirror (Phase 18 — System 1 at the edge).
//!
//! A faithful, dependency-light port of the host
//! `oh_ben_claw::agent::reflex` engine so a node keeps reacting within
//! milliseconds even when the spine/brain is unreachable. Rules are pushed from
//! the host (retained `obc/nodes/{id}/reflex_rules`, or the `set_reflex_rules`
//! serial command today); fired reflexes are reported on
//! `obc/nodes/{id}/reflex`. Local `gpio_write` actions stay bounded by the
//! Track 0 on-MCU safety gate (`safety_check_gpio_write`).
//!
//! This module is pure (`std` + `serde` only, no `esp-idf`) and wire-compatible
//! with the host `ReflexRule`/`Condition`/`Action` JSON, so a rule authored
//! against world memory validates identically here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Numeric comparison operator (mirror of the host `Cmp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl Cmp {
    fn test(self, a: f64, b: f64) -> bool {
        const EPS: f64 = 1e-9;
        match self {
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Eq => (a - b).abs() < EPS,
            Cmp::Ne => (a - b).abs() >= EPS,
        }
    }
}

/// A condition over a snapshot of `entity -> numeric value`. A missing entity
/// makes the leaf condition false (mirror of the host `Condition`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Condition {
    Sensor { entity: String, op: Cmp, value: f64 },
    GpioEq { entity: String, value: i64 },
    And { all: Vec<Condition> },
    Or { any: Vec<Condition> },
}

impl Condition {
    /// Evaluate against a snapshot of entity → numeric value.
    pub fn eval(&self, snapshot: &HashMap<String, f64>) -> bool {
        match self {
            Condition::Sensor { entity, op, value } => {
                snapshot.get(entity).map_or(false, |v| op.test(*v, *value))
            }
            Condition::GpioEq { entity, value } => snapshot
                .get(entity)
                .map_or(false, |v| Cmp::Eq.test(*v, *value as f64)),
            Condition::And { all } => all.iter().all(|c| c.eval(snapshot)),
            Condition::Or { any } => any.iter().any(|c| c.eval(snapshot)),
        }
    }
}

/// The action a fired reflex performs. The node honours `gpio_write` (driven
/// locally through the Track 0 safety gate) and `escalate` (reported upward);
/// host-only variants such as `publish` deserialize to [`Action::Unsupported`]
/// and are reported but not acted upon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Action {
    GpioWrite {
        /// Target node id (informational on-device; this node acts on its own pins).
        #[serde(default)]
        node_id: String,
        pin: i64,
        value: i64,
    },
    Escalate {
        reason: String,
    },
    /// Any action variant this firmware does not implement (e.g. host `publish`).
    #[serde(other)]
    Unsupported,
}

/// A reflex rule: when `when` holds, perform `then`, subject to debounce/rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflexRule {
    pub id: String,
    pub when: Condition,
    pub then: Action,
    #[serde(default)]
    pub debounce_ms: u64,
    #[serde(default)]
    pub max_rate_hz: Option<f64>,
}

impl ReflexRule {
    fn min_interval_ms(&self) -> u64 {
        let rate_ms = self
            .max_rate_hz
            .filter(|hz| *hz > 0.0)
            .map(|hz| (1000.0 / hz).ceil() as u64)
            .unwrap_or(0);
        self.debounce_ms.max(rate_ms)
    }
}

/// A reflex that fired this tick.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FiredReflex {
    pub rule_id: String,
    pub action: Action,
}

/// Evaluates [`ReflexRule`]s against sensor snapshots with per-rule debounce.
///
/// Single-threaded on the node, so fire-time state is a plain map updated via
/// `&mut self` (the host uses interior mutability for shared access).
#[derive(Debug, Default)]
pub struct ReflexEngine {
    rules: Vec<ReflexRule>,
    last_fire: HashMap<String, u64>,
}

impl ReflexEngine {
    #[allow(dead_code)] // standard constructor; used by tests (main uses Default + set_rules)
    pub fn new(rules: Vec<ReflexRule>) -> Self {
        Self {
            rules,
            last_fire: HashMap::new(),
        }
    }

    /// Replace the rule set (e.g. on a fresh push from the host). Clears
    /// debounce state so newly pushed rules can fire immediately.
    pub fn set_rules(&mut self, rules: Vec<ReflexRule>) {
        self.rules = rules;
        self.last_fire.clear();
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate all rules against `snapshot` at `now_ms`, honouring debounce/
    /// rate, recording fire times. Returns the reflexes that fired.
    pub fn evaluate(&mut self, snapshot: &HashMap<String, f64>, now_ms: u64) -> Vec<FiredReflex> {
        let mut fired = Vec::new();
        for rule in &self.rules {
            if !rule.when.eval(snapshot) {
                continue;
            }
            let min_interval = rule.min_interval_ms();
            if min_interval > 0 {
                if let Some(&last) = self.last_fire.get(&rule.id) {
                    if now_ms.saturating_sub(last) < min_interval {
                        continue;
                    }
                }
            }
            self.last_fire.insert(rule.id.clone(), now_ms);
            fired.push(FiredReflex {
                rule_id: rule.id.clone(),
                action: rule.then.clone(),
            });
        }
        fired
    }
}

// ── Tests (mirror the host reflex unit tests) ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn rule(id: &str, when: Condition, then: Action, debounce_ms: u64) -> ReflexRule {
        ReflexRule { id: id.to_string(), when, then, debounce_ms, max_rate_hz: None }
    }

    #[test]
    fn sensor_threshold_fires_and_missing_entity_is_false() {
        let cond = Condition::Sensor { entity: "sensor.temp".into(), op: Cmp::Gt, value: 28.0 };
        assert!(cond.eval(&snap(&[("sensor.temp", 30.0)])));
        assert!(!cond.eval(&snap(&[("sensor.temp", 20.0)])));
        assert!(!cond.eval(&snap(&[]))); // missing entity → false
    }

    #[test]
    fn and_or_compose() {
        let c = Condition::And {
            all: vec![
                Condition::Sensor { entity: "a".into(), op: Cmp::Ge, value: 1.0 },
                Condition::Or {
                    any: vec![
                        Condition::Sensor { entity: "b".into(), op: Cmp::Lt, value: 0.0 },
                        Condition::GpioEq { entity: "c".into(), value: 1 },
                    ],
                },
            ],
        };
        assert!(c.eval(&snap(&[("a", 2.0), ("c", 1.0)])));
        assert!(!c.eval(&snap(&[("a", 2.0), ("c", 0.0)])));
    }

    #[test]
    fn debounce_blocks_rapid_refire() {
        let mut eng = ReflexEngine::new(vec![rule(
            "r1",
            Condition::Sensor { entity: "sensor.temp".into(), op: Cmp::Gt, value: 28.0 },
            Action::Escalate { reason: "hot".into() },
            500,
        )]);
        let s = snap(&[("sensor.temp", 30.0)]);
        assert_eq!(eng.evaluate(&s, 1_000).len(), 1);
        assert_eq!(eng.evaluate(&s, 1_200).len(), 0); // within 500ms debounce
        assert_eq!(eng.evaluate(&s, 1_600).len(), 1); // debounce elapsed
    }

    #[test]
    fn gpio_write_action_round_trips_host_json() {
        // Host emits {"type":"gpio_write","node_id":"n","pin":2,"value":1}.
        let a: Action = serde_json::from_str(
            r#"{"type":"gpio_write","node_id":"node-1","pin":2,"value":1}"#,
        )
        .unwrap();
        assert_eq!(a, Action::GpioWrite { node_id: "node-1".into(), pin: 2, value: 1 });
    }

    #[test]
    fn host_only_action_becomes_unsupported() {
        let a: Action =
            serde_json::from_str(r#"{"type":"publish","topic":"x","payload":{}}"#).unwrap();
        assert_eq!(a, Action::Unsupported);
    }

    #[test]
    fn full_rule_deserializes_and_fires() {
        let rules: Vec<ReflexRule> = serde_json::from_str(
            r#"[{"id":"overheat","when":{"type":"sensor","entity":"sensor.temp","op":"gt","value":60.0},
                 "then":{"type":"gpio_write","node_id":"self","pin":7,"value":1},"debounce_ms":1000}]"#,
        )
        .unwrap();
        let mut eng = ReflexEngine::new(rules);
        assert_eq!(eng.rule_count(), 1);
        let fired = eng.evaluate(&snap(&[("sensor.temp", 75.0)]), 0);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "overheat");
        assert!(matches!(fired[0].action, Action::GpioWrite { pin: 7, value: 1, .. }));
    }
}

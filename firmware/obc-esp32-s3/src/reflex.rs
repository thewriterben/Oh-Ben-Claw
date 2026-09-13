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
//!
//! ## Descending modulation (the spinal tier)
//!
//! The brain does not name actuators to this node; it *modulates* the reflexes
//! that do. A rule may bind a threshold to a **slot** instead of a literal
//! (`Condition::SensorSlot`), and the `descend` command sets slot *levels* in
//! `[0, 1]` — a short list of `(slot, level)` pairs. The rule owns the physical
//! range (`min`, `max`); the message carries only where in it to sit. Levels
//! live in RAM and are lost on reboot, which returns every rule to its own
//! `default` — the safe posture. Whatever a modulated rule then actuates still
//! passes the Track 0 safety gate; modulation moves thresholds, never limits.
//!
//! The shape (sparse, graded, one slot per behaviour rather than per actuator)
//! is what the fly's descending population looks like when measured:
//! `experiments/lif-fly/RESULTS.md`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Slots a node holds. Sixteen is more than the bench uses. A `descend` is
/// sparse by design — a behaviour touches a few slots — and the full table
/// does *not* fit one LoRa frame (279 bytes); half of it does, with the auth
/// tag, and `tests/spine_payload_budget.rs` measures exactly how many.
pub const MAX_SLOTS: usize = 16;

/// The node's descending modulation table: one optional level per slot.
#[derive(Debug, Clone, PartialEq)]
pub struct Modulations {
    levels: [Option<f64>; MAX_SLOTS],
}

impl Default for Modulations {
    fn default() -> Self {
        Self {
            levels: [None; MAX_SLOTS],
        }
    }
}

impl Modulations {
    /// The level set for `slot`, if any.
    pub fn level(&self, slot: u8) -> Option<f64> {
        self.levels.get(slot as usize).copied().flatten()
    }

    /// Apply a descending message. All-or-nothing: every pair is checked
    /// before any is written, so a bad slot in the list changes nothing.
    /// Returns how many slots were set.
    pub fn apply(&mut self, pairs: &[(u8, f64)]) -> Result<usize, String> {
        for (slot, level) in pairs {
            if *slot as usize >= MAX_SLOTS {
                return Err(format!("slot {slot} out of range (max {})", MAX_SLOTS - 1));
            }
            if !(*level >= 0.0 && *level <= 1.0) {
                return Err(format!("slot {slot}: level {level} not in [0, 1]"));
            }
        }
        for (slot, level) in pairs {
            self.levels[*slot as usize] = Some(*level);
        }
        Ok(pairs.len())
    }

    /// Drop every level: every rule returns to its own default.
    pub fn clear(&mut self) {
        self.levels = [None; MAX_SLOTS];
    }

    /// The slots currently set, ascending.
    pub fn active(&self) -> Vec<(u8, f64)> {
        self.levels
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.map(|l| (i as u8, l)))
            .collect()
    }
}

/// Where a slot-bound threshold sits: `min + level·(max − min)`, with the
/// rule's own `default` level when the slot is unset.
fn slot_threshold(mods: &Modulations, slot: u8, min: f64, max: f64, default: f64) -> f64 {
    let level = mods.level(slot).unwrap_or(default).clamp(0.0, 1.0);
    min + level * (max - min)
}

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
    Sensor {
        entity: String,
        op: Cmp,
        value: f64,
    },
    /// Like `Sensor`, but the threshold is `min + level·(max − min)` where
    /// `level` is the node's current level for `slot` (or `default`). The
    /// descending path moves this threshold; nothing else does.
    SensorSlot {
        entity: String,
        op: Cmp,
        slot: u8,
        min: f64,
        max: f64,
        default: f64,
    },
    GpioEq {
        entity: String,
        value: i64,
    },
    And {
        all: Vec<Condition>,
    },
    Or {
        any: Vec<Condition>,
    },
}

impl Condition {
    /// Evaluate against a snapshot of entity → numeric value, with the node's
    /// current modulation levels resolving any slot-bound thresholds.
    pub fn eval(&self, snapshot: &HashMap<String, f64>, mods: &Modulations) -> bool {
        match self {
            Condition::Sensor { entity, op, value } => {
                snapshot.get(entity).is_some_and(|v| op.test(*v, *value))
            }
            Condition::SensorSlot {
                entity,
                op,
                slot,
                min,
                max,
                default,
            } => {
                let threshold = slot_threshold(mods, *slot, *min, *max, *default);
                snapshot.get(entity).is_some_and(|v| op.test(*v, threshold))
            }
            Condition::GpioEq { entity, value } => snapshot
                .get(entity)
                .is_some_and(|v| Cmp::Eq.test(*v, *value as f64)),
            Condition::And { all } => all.iter().all(|c| c.eval(snapshot, mods)),
            Condition::Or { any } => any.iter().any(|c| c.eval(snapshot, mods)),
        }
    }

    /// Reject a slot the node cannot hold or a range that cannot be sat in.
    /// Checked when rules are pushed, so a bad rule is refused at the door
    /// rather than silently never modulating.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Condition::SensorSlot {
                slot,
                min,
                max,
                default,
                ..
            } => {
                if *slot as usize >= MAX_SLOTS {
                    return Err(format!("slot {slot} out of range (max {})", MAX_SLOTS - 1));
                }
                if !(min.is_finite() && max.is_finite()) {
                    return Err(format!("slot {slot}: min/max must be finite"));
                }
                if !(*default >= 0.0 && *default <= 1.0) {
                    return Err(format!(
                        "slot {slot}: default level {default} not in [0, 1]"
                    ));
                }
                Ok(())
            }
            Condition::And { all } => all.iter().try_for_each(Condition::validate),
            Condition::Or { any } => any.iter().try_for_each(Condition::validate),
            Condition::Sensor { .. } | Condition::GpioEq { .. } => Ok(()),
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
    /// Descending modulation levels; RAM only, reboot returns rules to defaults.
    mods: Modulations,
}

impl ReflexEngine {
    #[allow(dead_code)] // standard constructor; used by tests (main uses Default + set_rules)
    pub fn new(rules: Vec<ReflexRule>) -> Self {
        Self {
            rules,
            last_fire: HashMap::new(),
            mods: Modulations::default(),
        }
    }

    /// Replace the rule set (e.g. on a fresh push from the host). Clears
    /// debounce state so newly pushed rules can fire immediately. Refuses the
    /// whole set if any rule binds a slot this node cannot hold; modulation
    /// levels already set are kept — they belong to slots, not rules.
    pub fn set_rules(&mut self, rules: Vec<ReflexRule>) -> Result<(), String> {
        for r in &rules {
            r.when
                .validate()
                .map_err(|e| format!("rule {}: {e}", r.id))?;
        }
        self.rules = rules;
        self.last_fire.clear();
        Ok(())
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Apply a descending message (`(slot, level)` pairs). All-or-nothing.
    pub fn descend(&mut self, pairs: &[(u8, f64)]) -> Result<usize, String> {
        self.mods.apply(pairs)
    }

    /// The node's current modulation table.
    pub fn modulations(&self) -> &Modulations {
        &self.mods
    }

    /// Drop every level: every slot-bound rule returns to its own default.
    pub fn clear_modulations(&mut self) {
        self.mods.clear();
    }

    /// Bench/one-shot evaluation: return every rule whose condition matches
    /// `snapshot`, WITHOUT reading or mutating the live debounce state. Used by
    /// the `reflex_tick` command so a manual tick reports what a snapshot *would*
    /// trigger without contending with the autonomous loop (which advances
    /// `last_fire` on the real uptime clock). Debounce/rate are intentionally
    /// ignored here — a single isolated tick has no history to suppress.
    pub fn evaluate_scratch(&self, snapshot: &HashMap<String, f64>) -> Vec<FiredReflex> {
        self.rules
            .iter()
            .filter(|rule| rule.when.eval(snapshot, &self.mods))
            .map(|rule| FiredReflex {
                rule_id: rule.id.clone(),
                action: rule.then.clone(),
            })
            .collect()
    }

    /// Evaluate all rules against `snapshot` at `now_ms`, honouring debounce/
    /// rate, recording fire times. Returns the reflexes that fired.
    pub fn evaluate(&mut self, snapshot: &HashMap<String, f64>, now_ms: u64) -> Vec<FiredReflex> {
        let mut fired = Vec::new();
        for rule in &self.rules {
            if !rule.when.eval(snapshot, &self.mods) {
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

    #[test]
    fn scratch_eval_ignores_debounce_history() {
        let mut eng = ReflexEngine::new(vec![rule(
            "batt-crit",
            Condition::Sensor {
                entity: "sensor.battery_soc".into(),
                op: Cmp::Le,
                value: 10.0,
            },
            Action::Escalate {
                reason: "critical".into(),
            },
            5_000,
        )]);
        // Stateful path: fires once, then is debounced on an immediate re-tick.
        assert_eq!(
            eng.evaluate(&snap(&[("sensor.battery_soc", 6.0)]), 1_000)
                .len(),
            1
        );
        assert!(eng
            .evaluate(&snap(&[("sensor.battery_soc", 6.0)]), 1_100)
            .is_empty());
        // Scratch path ignores that history and still reports the live match…
        assert_eq!(
            eng.evaluate_scratch(&snap(&[("sensor.battery_soc", 6.0)]))
                .len(),
            1
        );
        // …and a healthy reading fires nothing.
        assert!(eng
            .evaluate_scratch(&snap(&[("sensor.battery_soc", 85.0)]))
            .is_empty());
    }

    fn snap(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn rule(id: &str, when: Condition, then: Action, debounce_ms: u64) -> ReflexRule {
        ReflexRule {
            id: id.to_string(),
            when,
            then,
            debounce_ms,
            max_rate_hz: None,
        }
    }

    #[test]
    fn sensor_threshold_fires_and_missing_entity_is_false() {
        let cond = Condition::Sensor {
            entity: "sensor.temp".into(),
            op: Cmp::Gt,
            value: 28.0,
        };
        let m = Modulations::default();
        assert!(cond.eval(&snap(&[("sensor.temp", 30.0)]), &m));
        assert!(!cond.eval(&snap(&[("sensor.temp", 20.0)]), &m));
        assert!(!cond.eval(&snap(&[]), &m)); // missing entity → false
    }

    // ── Descending modulation ───────────────────────────────────────────

    fn slot_rule() -> ReflexRule {
        // "Too hot" sits somewhere between 20 °C and 60 °C; the brain decides
        // where, the rule decides the range, default is the middle (40 °C).
        rule(
            "vent",
            Condition::SensorSlot {
                entity: "sensor.temp".into(),
                op: Cmp::Gt,
                slot: 3,
                min: 20.0,
                max: 60.0,
                default: 0.5,
            },
            Action::GpioWrite {
                node_id: "self".into(),
                pin: 7,
                value: 1,
            },
            0,
        )
    }

    #[test]
    fn an_unset_slot_uses_the_rules_default_level() {
        let mut eng = ReflexEngine::new(vec![slot_rule()]);
        // Threshold = 20 + 0.5·40 = 40.
        assert!(eng.evaluate(&snap(&[("sensor.temp", 45.0)]), 0).len() == 1);
        assert!(eng.evaluate(&snap(&[("sensor.temp", 35.0)]), 1).is_empty());
    }

    #[test]
    fn a_descending_level_moves_the_threshold_within_the_rules_range() {
        let mut eng = ReflexEngine::new(vec![slot_rule()]);
        assert_eq!(eng.descend(&[(3, 0.0)]), Ok(1)); // threshold → 20
        assert_eq!(eng.evaluate(&snap(&[("sensor.temp", 25.0)]), 0).len(), 1);
        assert_eq!(eng.descend(&[(3, 1.0)]), Ok(1)); // threshold → 60
        assert!(eng.evaluate(&snap(&[("sensor.temp", 55.0)]), 1).is_empty());
        assert_eq!(eng.evaluate(&snap(&[("sensor.temp", 61.0)]), 2).len(), 1);
    }

    #[test]
    fn a_bad_pair_refuses_the_whole_message_and_changes_nothing() {
        let mut eng = ReflexEngine::new(vec![slot_rule()]);
        eng.descend(&[(3, 0.25)]).unwrap();
        let err = eng.descend(&[(3, 0.9), (16, 0.5)]).unwrap_err();
        assert!(err.contains("slot 16"), "{err}");
        assert_eq!(
            eng.modulations().level(3),
            Some(0.25),
            "the good pair did not land either"
        );
        let err = eng.descend(&[(2, 1.5)]).unwrap_err();
        assert!(err.contains("not in [0, 1]"), "{err}");
        assert!(eng.descend(&[(2, f64::NAN)]).is_err());
        assert_eq!(eng.modulations().active(), vec![(3, 0.25)]);
    }

    #[test]
    fn a_rule_binding_a_slot_the_node_cannot_hold_is_refused_at_the_door() {
        let mut eng = ReflexEngine::default();
        let mut bad = slot_rule();
        if let Condition::SensorSlot { slot, .. } = &mut bad.when {
            *slot = 16;
        }
        let err = eng.set_rules(vec![bad]).unwrap_err();
        assert!(
            err.contains("rule vent") && err.contains("slot 16"),
            "{err}"
        );
        assert_eq!(eng.rule_count(), 0, "nothing loaded");
        // A nested one is caught too.
        let mut nested = slot_rule();
        nested.when = Condition::And {
            all: vec![nested.when.clone()],
        };
        if let Condition::And { all } = &mut nested.when {
            if let Condition::SensorSlot { default, .. } = &mut all[0] {
                *default = 2.0;
            }
        }
        assert!(eng.set_rules(vec![nested]).is_err());
        assert!(eng.set_rules(vec![slot_rule()]).is_ok());
    }

    #[test]
    fn levels_survive_a_rule_push_and_clear_returns_to_defaults() {
        let mut eng = ReflexEngine::new(vec![slot_rule()]);
        eng.descend(&[(3, 0.0)]).unwrap();
        eng.set_rules(vec![slot_rule()]).unwrap();
        assert_eq!(
            eng.modulations().level(3),
            Some(0.0),
            "levels belong to slots, not rules"
        );
        assert_eq!(eng.evaluate(&snap(&[("sensor.temp", 25.0)]), 0).len(), 1);
        eng.clear_modulations();
        assert!(eng.evaluate(&snap(&[("sensor.temp", 25.0)]), 1).is_empty());
    }

    #[test]
    fn slot_rule_round_trips_the_host_json() {
        let json = r#"{"type":"sensor_slot","entity":"sensor.temp","op":"gt","slot":3,"min":20.0,"max":60.0,"default":0.5}"#;
        let c: Condition = serde_json::from_str(json).unwrap();
        assert_eq!(c, slot_rule().when);
        assert_eq!(serde_json::to_string(&c).unwrap(), json);
    }

    #[test]
    fn and_or_compose() {
        let c = Condition::And {
            all: vec![
                Condition::Sensor {
                    entity: "a".into(),
                    op: Cmp::Ge,
                    value: 1.0,
                },
                Condition::Or {
                    any: vec![
                        Condition::Sensor {
                            entity: "b".into(),
                            op: Cmp::Lt,
                            value: 0.0,
                        },
                        Condition::GpioEq {
                            entity: "c".into(),
                            value: 1,
                        },
                    ],
                },
            ],
        };
        let m = Modulations::default();
        assert!(c.eval(&snap(&[("a", 2.0), ("c", 1.0)]), &m));
        assert!(!c.eval(&snap(&[("a", 2.0), ("c", 0.0)]), &m));
    }

    #[test]
    fn debounce_blocks_rapid_refire() {
        let mut eng = ReflexEngine::new(vec![rule(
            "r1",
            Condition::Sensor {
                entity: "sensor.temp".into(),
                op: Cmp::Gt,
                value: 28.0,
            },
            Action::Escalate {
                reason: "hot".into(),
            },
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
        let a: Action =
            serde_json::from_str(r#"{"type":"gpio_write","node_id":"node-1","pin":2,"value":1}"#)
                .unwrap();
        assert_eq!(
            a,
            Action::GpioWrite {
                node_id: "node-1".into(),
                pin: 2,
                value: 1
            }
        );
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
        assert!(matches!(
            fired[0].action,
            Action::GpioWrite {
                pin: 7,
                value: 1,
                ..
            }
        ));
    }
}

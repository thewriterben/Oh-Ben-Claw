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
    /// True when `value op (baseline + offset)`, `baseline` being the engine's
    /// time-aware exponential moving average of the entity with time constant
    /// `tau_s` — a distance from where the signal has been, not a place on
    /// the scale. Mirror of the host's; the first sample is the baseline, and
    /// an entity never seen makes the leaf false.
    SensorBaseline {
        entity: String,
        op: Cmp,
        offset: f64,
        tau_s: f64,
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

/// A time-aware EMA of one entity (mirror of the host `Baseline`):
/// `b += (1 − e^(−dt/τ))·(v − b)` with `dt` the real time since the last
/// update, so τ is seconds of signal, not ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    pub value: f64,
    pub at_ms: u64,
}

impl Baseline {
    fn update(&mut self, v: f64, now_ms: u64, tau_s: f64) {
        let dt_s = now_ms.saturating_sub(self.at_ms) as f64 / 1000.0;
        let alpha = 1.0 - (-dt_s / tau_s).exp();
        self.value += alpha * (v - self.value);
        self.at_ms = now_ms;
    }
}

/// Baselines by (entity, `tau_s` bits) — the same key the host uses.
pub type Baselines = HashMap<(String, u64), Baseline>;

fn baseline_key(entity: &str, tau_s: f64) -> (String, u64) {
    (entity.to_string(), tau_s.to_bits())
}

impl Condition {
    /// Evaluate against a snapshot of entity → numeric value, with the node's
    /// current modulation levels resolving any slot-bound thresholds and its
    /// baselines resolving any history-bound ones.
    pub fn eval(
        &self,
        snapshot: &HashMap<String, f64>,
        mods: &Modulations,
        baselines: &Baselines,
    ) -> bool {
        match self {
            Condition::Sensor { entity, op, value } => {
                snapshot.get(entity).is_some_and(|v| op.test(*v, *value))
            }
            Condition::SensorBaseline {
                entity,
                op,
                offset,
                tau_s,
            } => match (
                snapshot.get(entity),
                baselines.get(&baseline_key(entity, *tau_s)),
            ) {
                (Some(v), Some(b)) => op.test(*v, b.value + offset),
                _ => false,
            },
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
            Condition::And { all } => all.iter().all(|c| c.eval(snapshot, mods, baselines)),
            Condition::Or { any } => any.iter().any(|c| c.eval(snapshot, mods, baselines)),
        }
    }

    /// Every (entity, `tau_s`) baseline this condition asks for.
    fn collect_baselines(&self, set: &mut Vec<(String, u64)>) {
        match self {
            Condition::SensorBaseline { entity, tau_s, .. } => {
                let k = baseline_key(entity, *tau_s);
                if !set.contains(&k) {
                    set.push(k);
                }
            }
            Condition::And { all } => all.iter().for_each(|c| c.collect_baselines(set)),
            Condition::Or { any } => any.iter().for_each(|c| c.collect_baselines(set)),
            Condition::Sensor { .. } | Condition::SensorSlot { .. } | Condition::GpioEq { .. } => {}
        }
    }

    /// Reject a slot the node cannot hold or a range that cannot be sat in.
    /// Checked when rules are pushed, so a bad rule is refused at the door
    /// rather than silently never modulating.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Condition::SensorBaseline {
                entity,
                offset,
                tau_s,
                ..
            } => {
                if !(tau_s.is_finite() && *tau_s > 0.0) {
                    return Err(format!(
                        "baseline on {entity}: tau_s {tau_s} must be finite and > 0"
                    ));
                }
                if !offset.is_finite() {
                    return Err(format!("baseline on {entity}: offset must be finite"));
                }
                Ok(())
            }
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
    /// Fire on the condition's false→true transition only; while it holds,
    /// nothing, however many ticks pass. It fires again once the condition
    /// has dropped and returned. Debounce still applies on top.
    ///
    /// The same field, and the same intent, as the host's `fire_on_change`
    /// — "did anything happen?" rather than "has enough time passed?" — with
    /// the only evidence this node has: the truth of its own condition. The
    /// host judges by fact ids because its snapshots carry them; a value-only
    /// snapshot never does, so the host's rule would not suppress here at
    /// all. Measured 2026-09-13: two holding slot rules with `debounce_ms`
    /// 10 000 put a 200-byte report on the mesh every 10 s each, and under
    /// that chatter the base's commands reached the bridge 4 times in 6.
    /// (One of the holding rules was `safe-link-offline`, and it was holding
    /// because the silence clock ignored the mesh — fixed the same day in
    /// the main loop; the edge behaviour here is still right for the rest.)
    #[serde(default)]
    pub fire_on_change: bool,
    /// The condition must have held, without a break, for at least this long
    /// before the rule may fire; zero (default) means one true tick is enough.
    /// Persistence, the third question beside rate (`debounce_ms`) and edge
    /// (`fire_on_change`), and the cheapest vetting stage there is: a reading
    /// that crosses a threshold for one tick and drops back is a transient,
    /// and at 1 °C quantisation the die temperature does exactly that at its
    /// slot threshold. Same field and semantics as the host's; judged at
    /// tick resolution (1 s here). With `fire_on_change`, the one fire per
    /// run comes once the hold has elapsed; a run shorter than the hold
    /// fires nothing.
    #[serde(default)]
    pub hold_ms: u64,
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
    /// Per `fire_on_change` rule: whether it fired for the condition's
    /// current run of truth. Cleared when the condition drops.
    fired_this_run: HashMap<String, bool>,
    /// Per `hold_ms` rule: the tick its condition last became true. Present
    /// while it holds, removed the tick it drops.
    true_since: HashMap<String, u64>,
    /// Descending modulation levels; RAM only, reboot returns rules to defaults.
    mods: Modulations,
    /// The baselines the rules ask for, advanced each autonomous tick from
    /// the snapshot before any rule is judged. Evidence history, not rule
    /// state: kept across a rule push, like the modulation levels.
    baselines: Baselines,
}

impl ReflexEngine {
    #[allow(dead_code)] // standard constructor; used by tests (main uses Default + set_rules)
    pub fn new(rules: Vec<ReflexRule>) -> Self {
        Self {
            rules,
            last_fire: HashMap::new(),
            fired_this_run: HashMap::new(),
            true_since: HashMap::new(),
            mods: Modulations::default(),
            baselines: Baselines::new(),
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
        self.fired_this_run.clear();
        self.true_since.clear();
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
        // Baselines are read as they stand and not advanced: a bench tick is a
        // question about this snapshot, not a sample of the signal.
        self.rules
            .iter()
            .filter(|rule| rule.when.eval(snapshot, &self.mods, &self.baselines))
            .map(|rule| FiredReflex {
                rule_id: rule.id.clone(),
                action: rule.then.clone(),
            })
            .collect()
    }

    /// The baseline held for `entity` at `tau_s`, if a rule asks for one and
    /// the entity has been seen.
    pub fn baseline(&self, entity: &str, tau_s: f64) -> Option<Baseline> {
        self.baselines.get(&baseline_key(entity, tau_s)).copied()
    }

    /// Evaluate all rules against `snapshot` at `now_ms`, honouring debounce/
    /// rate, recording fire times. Returns the reflexes that fired.
    pub fn evaluate(&mut self, snapshot: &HashMap<String, f64>, now_ms: u64) -> Vec<FiredReflex> {
        // Advance every baseline the rules ask for from this tick's readings
        // before any rule is judged: the first sample of an entity is its
        // baseline, so a rule cannot fire on the tick that started its history.
        let mut keys = Vec::new();
        for rule in &self.rules {
            rule.when.collect_baselines(&mut keys);
        }
        for key in keys {
            if let Some(&v) = snapshot.get(&key.0) {
                let tau_s = f64::from_bits(key.1);
                self.baselines
                    .entry(key)
                    .and_modify(|b| b.update(v, now_ms, tau_s))
                    .or_insert(Baseline {
                        value: v,
                        at_ms: now_ms,
                    });
            }
        }
        let mut fired = Vec::new();
        for rule in &self.rules {
            if !rule.when.eval(snapshot, &self.mods, &self.baselines) {
                // The run of truth is over; the next true is a new event, and
                // a new hold.
                if rule.fire_on_change {
                    self.fired_this_run.insert(rule.id.clone(), false);
                }
                if rule.hold_ms > 0 {
                    self.true_since.remove(&rule.id);
                }
                continue;
            }
            if rule.hold_ms > 0 {
                // Persistence before edge and rate: not held long enough is
                // not a candidate, whatever the clock says.
                let began = *self.true_since.entry(rule.id.clone()).or_insert(now_ms);
                if now_ms.saturating_sub(began) < rule.hold_ms {
                    continue;
                }
            }
            if rule.fire_on_change && self.fired_this_run.get(&rule.id).copied().unwrap_or(false) {
                continue; // still holding, already reported
            }
            let min_interval = rule.min_interval_ms();
            if min_interval > 0 {
                if let Some(&last) = self.last_fire.get(&rule.id) {
                    if now_ms.saturating_sub(last) < min_interval {
                        // Debounced: not fired for this run yet, so a
                        // `fire_on_change` rule will fire once it elapses.
                        continue;
                    }
                }
            }
            self.last_fire.insert(rule.id.clone(), now_ms);
            if rule.fire_on_change {
                self.fired_this_run.insert(rule.id.clone(), true);
            }
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
            fire_on_change: false,
            hold_ms: 0,
        }
    }

    fn hot_rule(fire_on_change: bool, debounce_ms: u64) -> ReflexRule {
        ReflexRule {
            id: "hot".to_string(),
            when: Condition::Sensor {
                entity: "sensor.t".into(),
                op: Cmp::Gt,
                value: 50.0,
            },
            then: Action::Escalate {
                reason: "hot".into(),
            },
            debounce_ms,
            max_rate_hz: None,
            fire_on_change,
            hold_ms: 0,
        }
    }

    #[test]
    fn a_transient_does_not_satisfy_a_hold() {
        // The die temperature is quantised to ~1 °C and the reflex tick is 1 s:
        // a reading that sits at its slot threshold flickers across it for a
        // tick at a time. With a hold, a flicker is not a fire.
        let mut hold = hot_rule(false, 0);
        hold.hold_ms = 3_000;
        let mut eng = ReflexEngine::new(vec![hold]);
        let hot = snap(&[("sensor.t", 60.0)]);
        let cool = snap(&[("sensor.t", 40.0)]);
        assert!(eng.evaluate(&hot, 0).is_empty(), "the hold starts");
        assert!(eng.evaluate(&cool, 1_000).is_empty(), "a drop resets it");
        assert!(eng.evaluate(&hot, 2_000).is_empty(), "a new run from 2 s");
        assert!(eng.evaluate(&hot, 4_000).is_empty(), "2 s in");
        assert_eq!(eng.evaluate(&hot, 5_000).len(), 1, "3 s held: fires");
        assert_eq!(
            eng.evaluate(&hot, 6_000).len(),
            1,
            "no edge, no debounce: keeps firing while held, as before"
        );
    }

    #[test]
    fn hold_composes_with_the_edge() {
        // One fire per run, and only for a run that outlasts the hold.
        let mut r = hot_rule(true, 0);
        r.hold_ms = 2_000;
        let mut eng = ReflexEngine::new(vec![r]);
        let hot = snap(&[("sensor.t", 60.0)]);
        let cool = snap(&[("sensor.t", 40.0)]);
        assert!(eng.evaluate(&hot, 0).is_empty());
        assert!(
            eng.evaluate(&cool, 1_000).is_empty(),
            "a 1 s run fires nothing at all"
        );
        assert!(eng.evaluate(&hot, 2_000).is_empty());
        assert!(eng.evaluate(&hot, 3_000).is_empty());
        assert_eq!(eng.evaluate(&hot, 4_000).len(), 1, "2 s held: the one fire");
        for t in 5..60u64 {
            assert!(eng.evaluate(&hot, t * 1_000).is_empty(), "holding at {t}s");
        }
        assert!(eng.evaluate(&cool, 60_000).is_empty());
        assert!(
            eng.evaluate(&hot, 61_000).is_empty(),
            "a new run, a new hold"
        );
        assert_eq!(eng.evaluate(&hot, 63_000).len(), 1);
    }

    #[test]
    fn set_rules_resets_the_hold() {
        let mut r = hot_rule(false, 0);
        r.hold_ms = 2_000;
        let mut eng = ReflexEngine::new(vec![r.clone()]);
        let hot = snap(&[("sensor.t", 60.0)]);
        assert!(eng.evaluate(&hot, 0).is_empty());
        eng.set_rules(vec![r]).unwrap();
        assert!(
            eng.evaluate(&hot, 2_000).is_empty(),
            "a fresh rule set starts its holds from its own first tick"
        );
        assert_eq!(eng.evaluate(&hot, 4_000).len(), 1);
    }

    // ── SensorBaseline (mirror of the host's known answers) ─────────────

    fn baseline_rule(offset: f64, tau_s: f64) -> ReflexRule {
        rule(
            "rising",
            Condition::SensorBaseline {
                entity: "t".into(),
                op: Cmp::Gt,
                offset,
                tau_s,
            },
            Action::Escalate {
                reason: "rising".into(),
            },
            0,
        )
    }

    #[test]
    fn the_first_sample_is_the_baseline_and_a_step_above_it_fires() {
        let mut eng = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        for t in 0..10u64 {
            assert!(eng.evaluate(&snap(&[("t", 20.0)]), t * 1_000).is_empty());
        }
        assert!((eng.baseline("t", 10.0).unwrap().value - 20.0).abs() < 1e-12);
        assert_eq!(eng.evaluate(&snap(&[("t", 25.0)]), 10_000).len(), 1);
        let expected = 20.0 + (1.0 - (-0.1f64).exp()) * 5.0;
        assert!((eng.baseline("t", 10.0).unwrap().value - expected).abs() < 1e-12);
    }

    #[test]
    fn a_slow_drift_never_fires_because_the_baseline_follows_it() {
        // A ramp of 0.01 °C/s over 1000 s, five offsets' worth of climb, and
        // not one fire: the EMA lags a ramp by s·(1−α)/α ≈ s·τ. A fixed
        // threshold would have fired at 200 s and stayed fired.
        let mut eng = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        for t in 0..=1000u64 {
            let v = 20.0 + 0.01 * t as f64;
            assert!(
                eng.evaluate(&snap(&[("t", v)]), t * 1_000).is_empty(),
                "t={t}"
            );
        }
        let alpha = 1.0 - (-0.1f64).exp();
        let lag = 30.0 - eng.baseline("t", 10.0).unwrap().value;
        assert!((lag - 0.01 * (1.0 - alpha) / alpha).abs() < 1e-9, "{lag}");
        assert_eq!(eng.evaluate(&snap(&[("t", 35.0)]), 1_001_000).len(), 1);
    }

    #[test]
    fn the_time_constant_is_seconds_not_ticks() {
        // The node ticks at 1 Hz and the host at whatever its interval is;
        // the same signal gives the same baseline at the same instant.
        let mut fast = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        let mut slow = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        // Both sample t = 10 (the last 20), so both date the step to the same
        // instant — a sample stands for the interval that ends at it.
        let v = |t: u64| if t <= 10 { 20.0 } else { 25.0 };
        for t in 0..=20u64 {
            fast.evaluate(&snap(&[("t", v(t))]), t * 1_000);
            if t % 2 == 0 {
                slow.evaluate(&snap(&[("t", v(t))]), t * 1_000);
            }
        }
        let expected = 25.0 - 5.0 * (-1.0f64).exp();
        assert!((fast.baseline("t", 10.0).unwrap().value - expected).abs() < 1e-12);
        assert!((slow.baseline("t", 10.0).unwrap().value - expected).abs() < 1e-12);
    }

    #[test]
    fn a_scratch_tick_reads_the_baseline_and_does_not_move_it() {
        let mut eng = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        eng.evaluate(&snap(&[("t", 20.0)]), 0);
        assert_eq!(
            eng.evaluate_scratch(&snap(&[("t", 25.0)])).len(),
            1,
            "judged against the live baseline"
        );
        assert_eq!(
            eng.baseline("t", 10.0).unwrap().value,
            20.0,
            "a bench question is not a sample"
        );
        assert!(
            ReflexEngine::new(vec![baseline_rule(2.0, 10.0)])
                .evaluate_scratch(&snap(&[("t", 25.0)]))
                .is_empty(),
            "no history at all: false, like any missing evidence"
        );
    }

    #[test]
    fn baselines_survive_a_rule_push_like_modulation_levels() {
        let mut eng = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        eng.evaluate(&snap(&[("t", 20.0)]), 0);
        eng.set_rules(vec![baseline_rule(2.0, 10.0)]).unwrap();
        assert_eq!(
            eng.evaluate(&snap(&[("t", 25.0)]), 1_000).len(),
            1,
            "the history belongs to the entity, not the rule"
        );
    }

    #[test]
    fn a_baseline_rule_the_node_cannot_run_is_refused_at_the_door() {
        let mut eng = ReflexEngine::default();
        let err = eng.set_rules(vec![baseline_rule(2.0, 0.0)]).unwrap_err();
        assert!(
            err.contains("rule rising") && err.contains("tau_s"),
            "{err}"
        );
        assert!(eng.set_rules(vec![baseline_rule(f64::NAN, 5.0)]).is_err());
        assert!(eng.set_rules(vec![baseline_rule(-2.0, 0.5)]).is_ok());
    }

    #[test]
    fn baseline_condition_round_trips_the_host_json() {
        let json = r#"{"type":"sensor_baseline","entity":"t","op":"gt","offset":3.0,"tau_s":60.0}"#;
        let c: Condition = serde_json::from_str(json).unwrap();
        assert_eq!(c, baseline_rule(3.0, 60.0).when);
        assert_eq!(serde_json::to_string(&c).unwrap(), json);
    }

    #[test]
    fn hold_ms_round_trips_the_host_json_and_defaults_to_zero() {
        let js = r#"{"id":"r","when":{"type":"sensor","entity":"x","op":"gt","value":0.0},"then":{"type":"escalate","reason":"r"}}"#;
        let r: ReflexRule = serde_json::from_str(js).unwrap();
        assert_eq!(r.hold_ms, 0);
        let js = r#"{"id":"r","when":{"type":"sensor","entity":"x","op":"gt","value":0.0},"then":{"type":"escalate","reason":"r"},"hold_ms":1500}"#;
        assert_eq!(
            serde_json::from_str::<ReflexRule>(js).unwrap().hold_ms,
            1_500
        );
    }

    #[test]
    fn a_holding_condition_re_fires_at_the_debounce_by_default() {
        // The behaviour every rule had before 2026-09-13, kept as the default:
        // debounce is a rate limit, not an edge.
        let mut eng = ReflexEngine::new(vec![hot_rule(false, 10_000)]);
        let hot = snap(&[("sensor.t", 60.0)]);
        let fired: Vec<u64> = (0..6)
            .map(|i| i * 5_000)
            .filter(|&t| !eng.evaluate(&hot, t).is_empty())
            .collect();
        assert_eq!(
            fired,
            vec![0, 10_000, 20_000],
            "every debounce interval while holding"
        );
    }

    #[test]
    fn fire_on_change_fires_once_per_run_of_truth() {
        let mut eng = ReflexEngine::new(vec![hot_rule(true, 0)]);
        let hot = snap(&[("sensor.t", 60.0)]);
        let cool = snap(&[("sensor.t", 40.0)]);
        assert_eq!(eng.evaluate(&hot, 0).len(), 1, "the transition");
        for t in 1..200u64 {
            assert!(
                eng.evaluate(&hot, t * 1_000).is_empty(),
                "holding at t={t}s must not re-fire"
            );
        }
        assert!(
            eng.evaluate(&cool, 300_000).is_empty(),
            "false fires nothing"
        );
        assert_eq!(
            eng.evaluate(&hot, 301_000).len(),
            1,
            "true again is a new event"
        );
        assert!(eng.evaluate(&hot, 302_000).is_empty());
    }

    #[test]
    fn fire_on_change_still_honours_the_debounce_on_the_transition() {
        // Flapping around the threshold faster than the debounce fires once
        // per debounce interval at most, and never while holding.
        let mut eng = ReflexEngine::new(vec![hot_rule(true, 10_000)]);
        let hot = snap(&[("sensor.t", 60.0)]);
        let cool = snap(&[("sensor.t", 40.0)]);
        assert_eq!(eng.evaluate(&hot, 0).len(), 1);
        assert!(eng.evaluate(&cool, 1_000).is_empty());
        assert!(
            eng.evaluate(&hot, 2_000).is_empty(),
            "a new run, but inside the debounce"
        );
        assert!(
            eng.evaluate(&hot, 5_000).is_empty(),
            "still holding, still inside"
        );
        assert_eq!(
            eng.evaluate(&hot, 10_000).len(),
            1,
            "the run that started at 2 s fires once the debounce elapses"
        );
        assert!(
            eng.evaluate(&hot, 30_000).is_empty(),
            "and then holds silently"
        );
    }

    #[test]
    fn a_modulation_move_that_flips_the_condition_is_a_transition() {
        // The descending path changes the threshold, not the reading; for an
        // edge rule that is exactly the event worth one report.
        let mut eng = ReflexEngine::new(vec![ReflexRule {
            id: "slot-hot".into(),
            when: Condition::SensorSlot {
                entity: "sensor.t".into(),
                op: Cmp::Gt,
                slot: 0,
                min: 30.0,
                max: 70.0,
                default: 0.5, // 50 °C
            },
            then: Action::Escalate {
                reason: "hot".into(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: true,
            hold_ms: 0,
        }]);
        let t38 = snap(&[("sensor.t", 38.0)]);
        assert!(eng.evaluate(&t38, 0).is_empty(), "38 < 50");
        eng.descend(&[(0, 0.15)]).unwrap(); // 36 °C
        assert_eq!(
            eng.evaluate(&t38, 1_000).len(),
            1,
            "the threshold moved under it"
        );
        assert!(eng.evaluate(&t38, 2_000).is_empty());
        eng.clear_modulations(); // back to 50
        assert!(eng.evaluate(&t38, 3_000).is_empty());
        eng.descend(&[(0, 0.15)]).unwrap();
        assert_eq!(
            eng.evaluate(&t38, 4_000).len(),
            1,
            "and again on the next move"
        );
    }

    #[test]
    fn set_rules_resets_the_edge_state() {
        let mut eng = ReflexEngine::new(vec![hot_rule(true, 0)]);
        let hot = snap(&[("sensor.t", 60.0)]);
        assert_eq!(eng.evaluate(&hot, 0).len(), 1);
        assert!(eng.evaluate(&hot, 1).is_empty());
        eng.set_rules(vec![hot_rule(true, 0)]).unwrap();
        assert_eq!(
            eng.evaluate(&hot, 2).len(),
            1,
            "a fresh rule set reports its standing conditions once"
        );
    }

    #[test]
    fn sensor_threshold_fires_and_missing_entity_is_false() {
        let cond = Condition::Sensor {
            entity: "sensor.temp".into(),
            op: Cmp::Gt,
            value: 28.0,
        };
        let m = Modulations::default();
        assert!(cond.eval(&snap(&[("sensor.temp", 30.0)]), &m, &Baselines::new()));
        assert!(!cond.eval(&snap(&[("sensor.temp", 20.0)]), &m, &Baselines::new()));
        assert!(!cond.eval(&snap(&[]), &m, &Baselines::new())); // missing entity → false
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
        assert!(c.eval(&snap(&[("a", 2.0), ("c", 1.0)]), &m, &Baselines::new()));
        assert!(!c.eval(&snap(&[("a", 2.0), ("c", 0.0)]), &m, &Baselines::new()));
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

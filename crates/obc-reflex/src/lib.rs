//! Dual-system reflex engine — System 1 (Phase 18).
//!
//! The agent's LLM is **System 2**: slow, general, cloud/host-side reasoning.
//! This module is **System 1**: fast, local, near-deterministic reflexes that
//! react to world state without waking the LLM. A reflex is a rule — *when this
//! condition holds, do this action* — subject to debounce and rate limits.
//!
//! Rules are authored on the host and serialize to a compact form so they can be
//! pushed to peripheral nodes (over `obc/nodes/{id}/reflex`) and evaluated there
//! with no host in the loop. This type *is* the wire format; the same evaluator
//! runs host-side (e.g. against world memory) and, mirrored, on the node.
//!
//! Safety: an [`Action::GpioWrite`] is still bounded by the node's deterministic
//! `SafetyGate` (Track 0) — a reflex can request an actuator change but cannot
//! exceed the on-MCU limits. [`Action::Escalate`] hands control up to System 2.

use async_trait::async_trait;
use obc_movement::{MovementCommand, MovementController};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// The standard safing rule set — the library of rules this engine was built to
/// run, and the escalation text `docs/playbooks/safing-escalations.md`
/// documents.
///
/// It spent its life in `obc-agent` because that is where the reflex engine
/// used to live, and it kept spelling this crate's types through the agent's
/// `pub use obc_reflex as reflex` alias long after the engine had left. Every
/// name it imports — [`Action`], [`ActionSink`], [`Cmp`], [`Condition`],
/// [`ReflexRule`] — is declared here; nothing in it named the agent. Moved
/// 2026-08-20.
pub mod safing;

/// Extract a numeric value from a world-memory fact value: a number, a bool
/// (1.0/0.0), a numeric string, or a sensor-fusion object `{"value": …}`.
fn fact_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse().ok(),
        Value::Object(o) => o.get("value").and_then(fact_to_f64),
        _ => None,
    }
}

/// Comparison operator for a sensor condition.
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
    /// Apply the comparison: `a {op} b`.
    pub fn test(self, a: f64, b: f64) -> bool {
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

/// A snapshot of current world state used to evaluate conditions: numeric values
/// (for `Sensor`/`GpioEq`) and the raw fact values (for `State`). A missing
/// entity makes the leaf condition false.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Entity → numeric value (number, bool, numeric string, or `{value}` object).
    pub nums: HashMap<String, f64>,
    /// Entity → raw fact value (for categorical `State` matching).
    pub vals: HashMap<String, Value>,
    /// Entity → row id of the fact this snapshot read.
    ///
    /// The identity of the evidence, as distinct from its value. Two snapshots can agree
    /// on every value and still be different observations, or agree on the id and be the
    /// same observation read twice — and only the second is a reason not to act again.
    /// Empty for snapshots built by value alone; a rule that needs it will simply not
    /// find its entities and will not suppress.
    pub ids: HashMap<String, i64>,
}

impl Snapshot {
    /// An empty snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from numeric pairs only (convenience; no categorical values).
    pub fn from_nums(nums: HashMap<String, f64>) -> Self {
        Self {
            nums,
            vals: HashMap::new(),
            ids: HashMap::new(),
        }
    }
}

/// A condition evaluated against a [`Snapshot`] of current world state. A missing
/// entity makes the leaf condition false.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Condition {
    /// Compare a sensor/entity numeric value (e.g. `living_room.temp > 28`).
    Sensor { entity: String, op: Cmp, value: f64 },
    /// Like `Sensor`, but the threshold is bound to a node **modulation slot**:
    /// `min + level·(max − min)`, where `level ∈ [0, 1]` is whatever the node
    /// last received on its `descend` command, or `default` if nothing. This
    /// is the spinal tier's handle — the brain moves a threshold inside a
    /// range the rule owns; it never names an actuator. The host evaluates
    /// this at `default`: modulation is a node-side concept, and a rule that
    /// runs on the host has no descending path to be modulated by. See
    /// `firmware/obc-esp32-s3/src/reflex.rs` for the node half.
    SensorSlot {
        entity: String,
        op: Cmp,
        slot: u8,
        min: f64,
        max: f64,
        default: f64,
    },
    /// Compare a reading against **its own recent history**: true when
    /// `value op (baseline + offset)`, where `baseline` is an exponential moving
    /// average of the entity with time constant `tau_s`, kept by the engine and
    /// advanced every tick the entity is present (time-aware, so a host ticking at
    /// one cadence and a node at another agree). The threshold is a *distance from
    /// where the signal has been*, not a place on the scale — invariant to slow
    /// drift and to where a sensor happens to sit.
    ///
    /// Offset, not ratio: WILD (Zhao et al. 2026) thresholds a power envelope at
    /// k × its baseline mean, which is right for a positive quantity; °C and most
    /// sensor scales are not ratio-scaled, and `baseline + 3` reads the same at
    /// 20 °C as at 40 °C where `1.15 × baseline` does not.
    ///
    /// The first sample *is* the baseline, so the leaf is false until the signal
    /// has moved; with no baseline yet (the entity has never been seen) it is
    /// false, like any leaf with missing evidence. `SensorBaseline` rules are
    /// evaluated only through an engine, which owns the state — a bare
    /// [`Condition::eval`] has no history and answers false.
    SensorBaseline {
        entity: String,
        op: Cmp,
        offset: f64,
        tau_s: f64,
    },
    /// A GPIO/entity equals an integer value.
    GpioEq { entity: String, value: i64 },
    /// A fact's (optionally nested) string value equals `equals`. With `field`,
    /// the fact value must be an object and `field` its string member (e.g.
    /// entity `power.mode`, field `mode`, equals `critical`); without `field`,
    /// the fact value must itself be a JSON string. This is how reflexes match
    /// the categorical mode hooks the suites emit (`power.mode`, `net.mode`,
    /// `audio.{stream}` labels, sensor `quality`).
    State {
        entity: String,
        #[serde(default)]
        field: Option<String>,
        equals: String,
    },
    /// All sub-conditions hold.
    And { all: Vec<Condition> },
    /// Any sub-condition holds.
    Or { any: Vec<Condition> },
}

/// A time-aware exponential moving average of one entity: the engine-owned
/// state behind [`Condition::SensorBaseline`].
///
/// `b += (1 − e^(−dt/τ)) · (v − b)`, with `dt` the real time since the last
/// update, so the time constant is a property of the signal and not of the tick:
/// a missed tick, a slower cadence, a host ticking at one rate and a node at
/// another all leave τ meaning the same seconds. A sample stands for the
/// interval that ends at it, so for a signal that holds between samples this is
/// the exact continuous EMA at the sample instants — and one sample after a
/// long gap stands for the whole gap, which is the right reading of a gap: a
/// hole in the evidence is not a step in the signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    /// The current average.
    pub value: f64,
    /// When it was last advanced (ms).
    pub at_ms: u64,
}

impl Baseline {
    /// Start a baseline at the first sample.
    pub fn new(value: f64, at_ms: u64) -> Self {
        Self { value, at_ms }
    }

    /// Advance to `now_ms` with sample `v` under time constant `tau_s`.
    pub fn update(&mut self, v: f64, now_ms: u64, tau_s: f64) {
        let dt_s = now_ms.saturating_sub(self.at_ms) as f64 / 1000.0;
        let alpha = 1.0 - (-dt_s / tau_s).exp();
        self.value += alpha * (v - self.value);
        self.at_ms = now_ms;
    }
}

/// The key a baseline lives under: an entity and a time constant. Two rules
/// asking for the same pair share one; different `tau_s` are different
/// histories. Keyed on the bits so `f64` can be a map key.
pub type BaselineKey = (String, u64);

/// Baselines by key — what an engine passes into evaluation.
pub type Baselines = HashMap<BaselineKey, Baseline>;

/// The key for `entity` at `tau_s`.
pub fn baseline_key(entity: &str, tau_s: f64) -> BaselineKey {
    (entity.to_string(), tau_s.to_bits())
}

impl Condition {
    /// Evaluate against a [`Snapshot`] with no baseline history — every
    /// [`Condition::SensorBaseline`] leaf is false. Engines call
    /// [`Condition::eval_with`].
    pub fn eval(&self, snap: &Snapshot) -> bool {
        self.eval_with(snap, &Baselines::new())
    }

    /// Evaluate against a [`Snapshot`] and the engine's [`Baselines`].
    pub fn eval_with(&self, snap: &Snapshot, baselines: &Baselines) -> bool {
        match self {
            Condition::Sensor { entity, op, value } => {
                snap.nums.get(entity).is_some_and(|v| op.test(*v, *value))
            }
            Condition::SensorBaseline {
                entity,
                op,
                offset,
                tau_s,
            } => match (
                snap.nums.get(entity),
                baselines.get(&baseline_key(entity, *tau_s)),
            ) {
                (Some(v), Some(b)) => op.test(*v, b.value + offset),
                _ => false,
            },
            Condition::SensorSlot {
                entity,
                op,
                min,
                max,
                default,
                ..
            } => {
                let threshold = min + default.clamp(0.0, 1.0) * (max - min);
                snap.nums
                    .get(entity)
                    .is_some_and(|v| op.test(*v, threshold))
            }
            Condition::GpioEq { entity, value } => snap
                .nums
                .get(entity)
                .is_some_and(|v| Cmp::Eq.test(*v, *value as f64)),
            Condition::State {
                entity,
                field,
                equals,
            } => snap.vals.get(entity).is_some_and(|v| {
                let s = match field {
                    Some(f) => v.get(f).and_then(|x| x.as_str()),
                    None => v.as_str(),
                };
                s == Some(equals.as_str())
            }),
            Condition::And { all } => all.iter().all(|c| c.eval_with(snap, baselines)),
            Condition::Or { any } => any.iter().any(|c| c.eval_with(snap, baselines)),
        }
    }

    /// Collect all entity names this condition references (for snapshotting).
    pub fn collect_entities(&self, set: &mut HashSet<String>) {
        match self {
            Condition::Sensor { entity, .. }
            | Condition::SensorBaseline { entity, .. }
            | Condition::SensorSlot { entity, .. }
            | Condition::GpioEq { entity, .. }
            | Condition::State { entity, .. } => {
                set.insert(entity.clone());
            }
            Condition::And { all } => all.iter().for_each(|c| c.collect_entities(set)),
            Condition::Or { any } => any.iter().for_each(|c| c.collect_entities(set)),
        }
    }

    /// Collect every baseline this condition asks for: the (entity, `tau_s`)
    /// pairs an engine has to keep advancing.
    pub fn collect_baselines(&self, set: &mut HashSet<BaselineKey>) {
        match self {
            Condition::SensorBaseline { entity, tau_s, .. } => {
                set.insert(baseline_key(entity, *tau_s));
            }
            Condition::And { all } => all.iter().for_each(|c| c.collect_baselines(set)),
            Condition::Or { any } => any.iter().for_each(|c| c.collect_baselines(set)),
            Condition::Sensor { .. }
            | Condition::SensorSlot { .. }
            | Condition::GpioEq { .. }
            | Condition::State { .. } => {}
        }
    }

    /// Reject a slot binding no node can hold. The same check the node runs
    /// when rules are pushed (`set_reflex_rules`), applied here at config load
    /// so a bad rule fails the host at startup rather than the node at push.
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
            Condition::And { all } => all.iter().try_for_each(Condition::validate),
            Condition::Or { any } => any.iter().try_for_each(Condition::validate),
            Condition::Sensor { .. } | Condition::GpioEq { .. } | Condition::State { .. } => Ok(()),
        }
    }
}

/// Modulation slots a node holds — pinned to the firmware's `MAX_SLOTS` by
/// `tests/firmware_node_gates.rs`.
pub const MAX_SLOTS: usize = 16;

/// The action a fired reflex performs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Action {
    /// Drive a node's GPIO pin (still bounded by the node's Track 0 `SafetyGate`).
    GpioWrite {
        node_id: String,
        pin: i64,
        value: i64,
    },
    /// Publish a payload to a spine topic.
    Publish { topic: String, payload: Value },
    /// Hand control up to System 2 (wake the LLM agent) with a reason.
    ///
    /// The reason is the woken agent's *prompt*, so it is long on purpose — the
    /// safing playbooks in [`safing`] run to a thousand characters of triage.
    /// Log [`escalation_label`] of it, never the whole thing; see that function
    /// for what a full-text log cost us.
    Escalate { reason: String },
    /// Apply a typed, safety-bounded movement (Movement subsystem). Still bounded
    /// by the Track 0 gate inside the `MovementController` before it actuates.
    Move { command: MovementCommand },
}

/// A reflex rule: when `when` holds, perform `then`, subject to debounce/rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflexRule {
    /// Unique id.
    pub id: String,
    /// The condition that triggers the rule.
    pub when: Condition,
    /// The action to perform.
    pub then: Action,
    /// Minimum ms between fires of this rule.
    #[serde(default)]
    pub debounce_ms: u64,
    /// Optional max firing rate (Hz); the larger of this interval and
    /// `debounce_ms` is enforced.
    #[serde(default)]
    pub max_rate_hz: Option<f64>,
    /// Only fire again when the *evidence* changed, not merely when the clock allowed it.
    ///
    /// Debounce is a rate limit: it asks "has enough time passed?" and, for a condition
    /// that stays true, the answer is eventually always yes. A rule watching a standing
    /// state therefore re-fires forever at the debounce interval, on the same facts.
    /// Measured here: the vision rules escalated to the 30B reasoner every hour on
    /// detections from 6 July, indefinitely, because "a verified person was detected"
    /// never stopped being true.
    ///
    /// With this set, the rule also requires the *ids* of the facts satisfying it to
    /// differ from the ids at its last fire. Same rows, no fire — nothing new happened,
    /// whatever the clock says. New detection, new row, fires immediately (subject to
    /// debounce).
    ///
    /// **Opt-in, and it must stay that way.** A safing rule *should* keep firing while a
    /// dangerous state holds: "the battery is still critical" is worth repeating, and
    /// suppressing it because the reading has not changed is precisely the wrong
    /// behaviour. This is for rules that report events, not for rules that hold a
    /// condition.
    ///
    /// A snapshot with no ids (built by value alone, e.g. on a node) never suppresses —
    /// missing evidence identity is not evidence of sameness.
    #[serde(default)]
    pub fire_on_change: bool,
    /// The condition must have held, without a break, for at least this long before
    /// the rule may fire. Zero (the default) means a single true tick is enough.
    ///
    /// The third of three questions a rule can ask, distinct from the other two.
    /// `debounce_ms` asks "has enough time passed since I last fired?" — a rate.
    /// `fire_on_change` asks "did anything happen?" — an edge. This asks "is it
    /// still true?" — persistence. A reading that crosses a threshold for one
    /// sample and drops back is a transient, and a transient is exactly what a
    /// one-tick rule fires on: a 1 °C quantised die temperature flickering across
    /// its slot threshold, a link-silence reading at the edge of its timeout.
    ///
    /// This is the duration criterion of an event detector: WILD (Zhao et al.
    /// 2026) accepts a ripple only if the power envelope stays over threshold for
    /// 20–600 ms, and it is the cheapest vetting stage there is — no model, no
    /// window, one timestamp per rule. Continuity is judged at tick resolution:
    /// the condition is "still true" if every tick since it became true said so,
    /// and the engine cannot see between ticks.
    ///
    /// Composes with the other two. With `fire_on_change`, the one fire per run
    /// comes once the hold has elapsed; a run shorter than the hold fires nothing.
    /// Debounce applies on top, as it always did.
    #[serde(default)]
    pub hold_ms: u64,
}

impl ReflexRule {
    /// Reject a rule the node side would refuse (see [`Condition::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        self.when
            .validate()
            .map_err(|e| format!("reflex rule {}: {e}", self.id))
    }

    /// The minimum interval (ms) between fires implied by `debounce_ms` + `max_rate_hz`.
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
#[derive(Debug, Clone, PartialEq)]
pub struct FiredReflex {
    /// The rule that fired.
    pub rule_id: String,
    /// The action to perform.
    pub action: Action,
}

/// Evaluates a set of [`ReflexRule`]s against world snapshots, with per-rule
/// debounce/rate state. Cheap to evaluate; runs on the host (against world
/// memory) and, mirrored, on the node.
#[derive(Debug)]
pub struct ReflexEngine {
    rules: Vec<ReflexRule>,
    last_fire: Mutex<HashMap<String, u64>>,
    /// Per rule, the fact ids that satisfied it when it last fired.
    ///
    /// Only consulted for rules with [`ReflexRule::fire_on_change`]. Kept separate from
    /// `last_fire` so a rule that does not opt in pays nothing and behaves exactly as
    /// before.
    last_evidence: Mutex<HashMap<String, Vec<i64>>>,
    /// Per rule with a `hold_ms`: the tick at which its condition last became true.
    ///
    /// Present while the condition holds, removed the tick it drops, so "held for
    /// `hold_ms`" is `now − true_since`. Only rules with a non-zero hold are entered,
    /// for the same reason as `last_evidence`: a rule that does not ask pays nothing.
    true_since: Mutex<HashMap<String, u64>>,
    /// The baselines every `SensorBaseline` leaf in the rule set asks for, advanced
    /// at each `evaluate` from the snapshot before the rules are judged. Evidence
    /// history, not rule state — it belongs to the entity.
    baselines: Mutex<Baselines>,
    trusted: obc_memory::world::OriginSet,
}

/// A default engine is an engine with no rules and the standard trust policy.
///
/// Written out rather than derived: a derived `Default` would give the trust set its
/// *numeric* default (the empty set), so `ReflexEngine::default()` would silently trust
/// nothing and never fire, quietly disagreeing with `new()`. A safety-relevant policy
/// should not be an accident of which fields a macro can see.
impl Default for ReflexEngine {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl ReflexEngine {
    /// Build an engine from a set of rules, trusting only *evidence*.
    ///
    /// Reflexes are automatic physical responses to sensed conditions, so by default they
    /// act on [`OriginSet::EVIDENCE`] — what the world reported and what the framework
    /// computed from it — and ignore `Asserted` and `Instructed` facts entirely.
    ///
    /// Neither exclusion is arbitrary. An agent's claim that the battery is at 5% is a
    /// claim, and a reflex that stops an actuator on it is letting a language model drive
    /// hardware through the back door. A human's typed reading is authoritative about
    /// *intent* but is still not a sensor: an operator who wants an actuator stopped
    /// should command that, not report a battery level and let safing infer it.
    pub fn new(rules: Vec<ReflexRule>) -> Self {
        Self {
            rules,
            last_fire: Mutex::new(HashMap::new()),
            last_evidence: Mutex::new(HashMap::new()),
            true_since: Mutex::new(HashMap::new()),
            baselines: Mutex::new(Baselines::new()),
            trusted: obc_memory::world::OriginSet::EVIDENCE,
        }
    }

    /// Override which origins this engine will act on.
    ///
    /// Widening this to include `Asserted` re-opens the path where an agent's own writes
    /// drive automatic physical responses — the hazard this gate exists to close. Do it
    /// only for read-only or simulation engines.
    pub fn with_trusted_origins(mut self, trusted: obc_memory::world::OriginSet) -> Self {
        self.trusted = trusted;
        self
    }

    /// Number of rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The baseline this engine holds for `entity` at `tau_s`, if any rule has
    /// asked for one and the entity has been seen.
    pub fn baseline(&self, entity: &str, tau_s: f64) -> Option<Baseline> {
        self.baselines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&baseline_key(entity, tau_s))
            .copied()
    }

    /// Evaluate all rules against `snapshot` at `now_ms`; returns the actions to
    /// perform (respecting debounce/rate), and records fire times.
    pub fn evaluate(&self, snapshot: &Snapshot, now_ms: u64) -> Vec<FiredReflex> {
        // Advance every baseline the rules ask for, from this tick's readings,
        // before any rule is judged: the first sample of an entity *is* its
        // baseline, so a rule cannot fire on the tick that started its history.
        let mut baselines = self.baselines.lock().unwrap_or_else(|p| p.into_inner());
        let mut keys = HashSet::new();
        for rule in &self.rules {
            rule.when.collect_baselines(&mut keys);
        }
        for key in keys {
            if let Some(&v) = snapshot.nums.get(&key.0) {
                let tau_s = f64::from_bits(key.1);
                baselines
                    .entry(key)
                    .and_modify(|b| b.update(v, now_ms, tau_s))
                    .or_insert_with(|| Baseline::new(v, now_ms));
            }
        }
        let mut guard = self.last_fire.lock().unwrap_or_else(|p| p.into_inner());
        let mut fired = Vec::new();
        for rule in &self.rules {
            if !rule.when.eval_with(snapshot, &baselines) {
                if rule.hold_ms > 0 {
                    // The run of truth is over; the next true starts the hold again.
                    self.true_since
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&rule.id);
                }
                continue;
            }
            if rule.hold_ms > 0 {
                // Persistence, before rate and edge: a condition that has not yet held
                // long enough is not a candidate at all, whatever the clock or the
                // evidence say. `entry` keeps the tick the run began; the check reads it.
                let mut since = self.true_since.lock().unwrap_or_else(|p| p.into_inner());
                let began = *since.entry(rule.id.clone()).or_insert(now_ms);
                if now_ms.saturating_sub(began) < rule.hold_ms {
                    continue;
                }
            }
            let min_interval = rule.min_interval_ms();
            if min_interval > 0 {
                if let Some(&last) = guard.get(&rule.id) {
                    if now_ms.saturating_sub(last) < min_interval {
                        continue;
                    }
                }
            }
            // Evidence check, after debounce and before recording the fire. Debounce
            // asks whether enough time has passed; this asks whether anything happened.
            // A standing condition answers yes to the first forever and no to the second.
            if rule.fire_on_change {
                let mut entities = HashSet::new();
                rule.when.collect_entities(&mut entities);
                let mut ids: Vec<i64> = entities
                    .iter()
                    .filter_map(|e| snapshot.ids.get(e).copied())
                    .collect();
                ids.sort_unstable();
                // No ids at all means the snapshot cannot speak to identity — on a node,
                // or in a value-only test. Missing evidence identity is not evidence of
                // sameness, so it must not suppress.
                if !ids.is_empty() {
                    let mut ev = self.last_evidence.lock().unwrap_or_else(|p| p.into_inner());
                    if ev.get(&rule.id) == Some(&ids) {
                        continue;
                    }
                    ev.insert(rule.id.clone(), ids);
                }
            }
            guard.insert(rule.id.clone(), now_ms);
            fired.push(FiredReflex {
                rule_id: rule.id.clone(),
                action: rule.then.clone(),
            });
        }
        fired
    }

    /// All entity names referenced by any rule's condition.
    pub fn referenced_entities(&self) -> HashSet<String> {
        let mut set = HashSet::new();
        for rule in &self.rules {
            rule.when.collect_entities(&mut set);
        }
        set
    }

    /// Build a snapshot from the *current* world-memory facts of the referenced
    /// entities, then evaluate. This is the host-side System 1 loop: perception
    /// (world memory) → reflexes → actions. The caller dispatches the returned
    /// actions (GPIO writes over the spine — bounded by Track 0 — publishes, or
    /// escalation to the LLM agent).
    pub fn tick(
        &self,
        world: &obc_memory::world::WorldMemory,
        now_ms: u64,
    ) -> anyhow::Result<Vec<FiredReflex>> {
        let mut snapshot = Snapshot::new();
        for entity in self.referenced_entities() {
            if let Some(fact) = world.current(&entity)? {
                // The trust gate, applied once here rather than in each `Condition`
                // variant: a fact this engine does not accept simply never enters the
                // snapshot. Every leaf condition uses `is_some_and`, so an absent entity
                // evaluates false and no rule fires on it — withholding is fail-safe.
                if !self.trusted.accepts(fact.origin) {
                    // Say so. A reflex that silently declines to fire is exactly the kind
                    // of invisible behaviour that cost an evening on 2026-07-17, and the
                    // difference between "the sensor is fine" and "I refused to believe
                    // the sensor" is not something to leave to inference.
                    tracing::debug!(
                        entity = %entity,
                        origin = %fact.origin.as_str(),
                        source = %fact.source,
                        "reflex: fact withheld from the snapshot — origin not trusted by this engine"
                    );
                    continue;
                }
                if let Some(v) = fact_to_f64(&fact.value) {
                    snapshot.nums.insert(entity.clone(), v);
                }
                // Carry the row id alongside the value. Two ticks reading the same row
                // are the same observation; two ticks reading different rows with equal
                // values are not. Only `fire_on_change` rules consult it.
                snapshot.ids.insert(entity.clone(), fact.id);
                snapshot.vals.insert(entity, fact.value);
            }
        }
        Ok(self.evaluate(&snapshot, now_ms))
    }
}

// ── Dispatch (System 1 output) ──────────────────────────────────────────────────

/// Performs the actions a reflex fires. Implementations route to the real world:
/// a GPIO write over the spine (bounded by the node's Track 0 `SafetyGate`), a
/// publish to a spine topic, or an escalation that wakes the LLM agent (System 2).
#[async_trait]
pub trait ActionSink: Send + Sync {
    /// Drive a node's GPIO pin.
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()>;
    /// Publish a payload to a spine topic.
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()>;
    /// Hand control to System 2 (the LLM agent) with a reason.
    async fn escalate(&self, reason: &str) -> anyhow::Result<()>;
    /// Apply a typed movement command. Default: no-op with a warning — sinks that
    /// support actuation override this (see [`MovementActionSink`], which routes
    /// the command through the Track 0–bounded [`MovementController`]).
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        tracing::warn!(
            actuator = command.name(),
            "reflex: move action dispatched to a sink without movement support (no-op)"
        );
        Ok(())
    }
}

/// Dispatch fired reflex actions to a sink, in order.
pub async fn dispatch(actions: &[FiredReflex], sink: &dyn ActionSink) -> anyhow::Result<()> {
    for f in actions {
        match &f.action {
            Action::GpioWrite {
                node_id,
                pin,
                value,
            } => sink.gpio_write(node_id, *pin, *value).await?,
            Action::Publish { topic, payload } => sink.publish(topic, payload).await?,
            Action::Escalate { reason } => sink.escalate(reason).await?,
            Action::Move { command } => sink.move_actuator(command).await?,
        }
    }
    Ok(())
}

/// The part of an escalation reason worth putting on a log line: its first
/// sentence, or the whole string when there is no sentence break.
///
/// An escalation reason does double duty. It is the prompt System 2 is woken
/// with (`build_objective` interpolates it verbatim), so the safing playbooks
/// are written as full triage directives — [`safing::MESH_LOST_PLAYBOOK`] is
/// about 1,060 characters. It was also the log message at four sites, which is
/// why on the night of 2026-07-28 a phantom mesh node (see the module comment
/// on `mesh_supervisor::snapshot`) wrote 2,367 lines carrying the same
/// paragraph — 2.5 MB, 41 % of a 46-day log file, in about nine hours at
/// twelve lines a minute. The reasoner still gets every word; the log gets the
/// sentence a human reads.
///
/// The break is `". "` or a trailing `"."` — deliberately not any `'.'`, so
/// `mesh_status` calls and `docs/playbooks/x.md` paths inside a sentence do not
/// split it. A reason with no break is returned whole, which is the old
/// behaviour and is right for the short ones (`"person detected (verified) on
/// a camera"`). Capped at [`LABEL_MAX`] so a reason written as one long
/// sentence still cannot flood a line.
pub fn escalation_label(reason: &str) -> &str {
    let end = reason
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| reason.strip_suffix('.').map(str::len).map(|n| n + 1))
        .unwrap_or(reason.len());
    let label = &reason[..end];
    match label.char_indices().nth(LABEL_MAX) {
        Some((cut, _)) => &label[..cut],
        None => label,
    }
}

/// Character cap on an [`escalation_label`].
pub const LABEL_MAX: usize = 160;

/// A safe default sink that only *logs* intended actions without executing them
/// — useful for dry-run / supervised rollout before wiring the real spine sink.
pub struct LoggingActionSink;

#[async_trait]
impl ActionSink for LoggingActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        tracing::info!(node_id, pin, value, "reflex: gpio_write (dry-run)");
        Ok(())
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        tracing::info!(topic, %payload, "reflex: publish (dry-run)");
        Ok(())
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        tracing::info!(
            escalation = escalation_label(reason),
            "reflex: escalate to System 2 (dry-run)"
        );
        Ok(())
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        tracing::info!(
            actuator = command.name(),
            tool = command.tool(),
            "reflex: move (dry-run)"
        );
        Ok(())
    }
}

/// An [`ActionSink`] that applies reflex `Move` actions through the safety-bounded
/// [`MovementController`] (Track 0 gate + world-memory record), delegating GPIO /
/// publish / escalate to an inner sink. This is how a reflex actuates *typed*
/// movement locally rather than emitting a raw GPIO write.
pub struct MovementActionSink {
    movement: Arc<MovementController>,
    inner: Arc<dyn ActionSink>,
}

impl MovementActionSink {
    /// Wrap an inner sink, routing `Move` actions through `movement`.
    pub fn new(movement: Arc<MovementController>, inner: Arc<dyn ActionSink>) -> Self {
        Self { movement, inner }
    }
}

#[async_trait]
impl ActionSink for MovementActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        self.inner.gpio_write(node_id, pin, value).await
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        self.inner.publish(topic, payload).await
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        self.inner.escalate(reason).await
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // A refused movement (safety violation) is logged, not propagated — one
        // bad reflex must never stall the System 1 loop (mirrors the spine sink).
        if let Err(e) = self.movement.apply(command, now).await {
            tracing::warn!(actuator = command.name(), error = %e, "reflex move refused/failed");
        }
        Ok(())
    }
}

/// Rate-limits escalations from System 1 to System 2 (the LLM), so a noisy
/// reflex can't flood the expensive reasoner. Sliding window.
#[derive(Debug)]
pub struct EscalationBudget {
    max_per_window: u32,
    window_ms: u64,
    times: Mutex<VecDeque<u64>>,
}

impl EscalationBudget {
    /// At most `max_per_window` escalations per `window_ms`. `max_per_window = 0`
    /// means unlimited.
    pub fn new(max_per_window: u32, window_ms: u64) -> Self {
        Self {
            max_per_window,
            window_ms,
            times: Mutex::new(VecDeque::new()),
        }
    }

    /// Convenience: at most `max` escalations per minute.
    pub fn per_minute(max: u32) -> Self {
        Self::new(max, 60_000)
    }

    /// Whether an escalation is allowed at `now_ms`; records it when allowed.
    pub fn allow(&self, now_ms: u64) -> bool {
        if self.max_per_window == 0 {
            return true;
        }
        let mut times = self.times.lock().unwrap_or_else(|p| p.into_inner());
        let cutoff = now_ms.saturating_sub(self.window_ms);
        while times.front().is_some_and(|&t| t < cutoff) {
            times.pop_front();
        }
        if (times.len() as u32) < self.max_per_window {
            times.push_back(now_ms);
            true
        } else {
            false
        }
    }
}

/// Ties the reflex engine to world memory and an action sink: one `tick` reads
/// world state, fires reflexes, and dispatches their actions. This is the
/// host-side System 1 controller; spawn its `tick_and_dispatch` on a cadence.
pub struct ReflexController {
    engine: ReflexEngine,
    world: Arc<obc_memory::world::WorldMemory>,
    sink: Arc<dyn ActionSink>,
    escalation_budget: Option<EscalationBudget>,
    metrics: Option<Arc<obc_observability::MetricsRegistry>>,
}

impl ReflexController {
    /// Build a controller.
    pub fn new(
        engine: ReflexEngine,
        world: Arc<obc_memory::world::WorldMemory>,
        sink: Arc<dyn ActionSink>,
    ) -> Self {
        Self {
            engine,
            world,
            sink,
            escalation_budget: None,
            metrics: None,
        }
    }

    /// Cap how often reflexes may escalate to System 2.
    pub fn with_escalation_budget(mut self, budget: EscalationBudget) -> Self {
        self.escalation_budget = Some(budget);
        self
    }

    /// Record per-rule / per-action fire counts into a metrics registry (surfaced
    /// on the gateway `/metrics` endpoint). Counters: `reflex.fired_total`,
    /// `reflex.rule.{id}`, `reflex.action.{kind}`.
    pub fn with_metrics(mut self, metrics: Arc<obc_observability::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    fn record_fire(&self, f: &FiredReflex) {
        if let Some(m) = &self.metrics {
            m.counter("reflex.fired_total").inc();
            m.counter(format!("reflex.rule.{}", f.rule_id)).inc();
            let kind = match &f.action {
                Action::GpioWrite { .. } => "gpio_write",
                Action::Publish { .. } => "publish",
                Action::Escalate { .. } => "escalate",
                Action::Move { .. } => "move",
            };
            m.counter(format!("reflex.action.{kind}")).inc();
        }
    }

    /// Read world state, evaluate reflexes, dispatch the fired actions; returns
    /// what fired. Escalations beyond the budget are fired-but-not-dispatched.
    pub async fn tick_and_dispatch(&self, now_ms: u64) -> anyhow::Result<Vec<FiredReflex>> {
        let fired = self.engine.tick(&self.world, now_ms)?;
        for f in &fired {
            self.record_fire(f);
            match &f.action {
                Action::GpioWrite {
                    node_id,
                    pin,
                    value,
                } => self.sink.gpio_write(node_id, *pin, *value).await?,
                Action::Publish { topic, payload } => self.sink.publish(topic, payload).await?,
                Action::Escalate { reason } => {
                    let allowed = self
                        .escalation_budget
                        .as_ref()
                        .is_none_or(|b| b.allow(now_ms));
                    if allowed {
                        self.sink.escalate(reason).await?;
                    } else {
                        tracing::debug!(reason, "reflex: escalation suppressed by budget");
                    }
                }
                Action::Move { command } => self.sink.move_actuator(command).await?,
            }
        }
        Ok(fired)
    }
}

#[cfg(test)]
mod tests {
    // Relocated with Severity from the agent's notify module,
    // 2026-08-13. It travels with the type it tests.
    #[test]
    fn severity_classifies_from_the_reason() {
        assert_eq!(
            Severity::classify("a mesh node is presumed lost"),
            Severity::Critical
        );
        assert_eq!(
            Severity::classify("battery critical — safing"),
            Severity::Critical
        );
        assert_eq!(
            Severity::classify("sensor humidity out of range"),
            Severity::Warning
        );
        assert!(Severity::Critical > Severity::Warning && Severity::Warning > Severity::Info);
    }

    use super::*;
    use obc_memory::world::WorldMemory;
    use serde_json::json;

    #[test]
    fn escalation_label_is_the_first_sentence() {
        // The real playbook. Its first sentence is what a reader needs; the
        // remaining ~1,000 characters are triage instructions for the LLM.
        let full = safing::MESH_LOST_PLAYBOOK;
        assert!(full.len() > 900, "playbook shrank: {}", full.len());
        assert_eq!(
            escalation_label(full),
            "A mesh node is presumed lost (LoRa escalation)."
        );
    }

    #[test]
    fn a_reason_with_no_sentence_break_is_kept_whole() {
        // The short reasons were never the problem and must not be truncated.
        let short = "person detected (verified) on a camera";
        assert_eq!(escalation_label(short), short);
        assert_eq!(escalation_label(""), "");
    }

    #[test]
    fn only_a_sentence_break_splits_it() {
        // A bare '.' split would cut at `mesh_status` calls and at
        // `docs/playbooks/x.md`, producing a label that reads as a fragment.
        assert_eq!(
            escalation_label("call `a.b` then stop. And more."),
            "call `a.b` then stop."
        );
        assert_eq!(
            escalation_label("see docs/playbooks/mesh-node-lost.md"),
            "see docs/playbooks/mesh-node-lost.md"
        );
        // A trailing period is a break; a one-sentence reason keeps it.
        assert_eq!(escalation_label("all quiet."), "all quiet.");
    }

    #[test]
    fn one_long_sentence_still_cannot_flood_a_line() {
        let long = "x".repeat(LABEL_MAX * 3);
        assert_eq!(escalation_label(&long).chars().count(), LABEL_MAX);
    }

    #[test]
    fn the_cap_cuts_on_a_character_not_a_byte() {
        // Slicing mid-codepoint would panic, and a reason can carry an em dash.
        let long = "é".repeat(LABEL_MAX * 2);
        assert_eq!(escalation_label(&long).chars().count(), LABEL_MAX);
    }

    #[test]
    fn fact_value_extraction() {
        assert_eq!(fact_to_f64(&json!(3.5)), Some(3.5));
        assert_eq!(fact_to_f64(&json!(true)), Some(1.0));
        assert_eq!(fact_to_f64(&json!(false)), Some(0.0));
        assert_eq!(fact_to_f64(&json!({"value": 7, "n": 2})), Some(7.0)); // fusion shape
        assert_eq!(fact_to_f64(&json!("nope")), None);
    }

    #[test]
    fn referenced_entities_collects_from_nested_conditions() {
        let rule = ReflexRule {
            id: "r".into(),
            when: Condition::And {
                all: vec![
                    Condition::Sensor {
                        entity: "t".into(),
                        op: Cmp::Gt,
                        value: 1.0,
                    },
                    Condition::Or {
                        any: vec![Condition::GpioEq {
                            entity: "armed".into(),
                            value: 1,
                        }],
                    },
                ],
            },
            then: Action::Escalate { reason: "x".into() },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        };
        let e = ReflexEngine::new(vec![rule]);
        let ents = e.referenced_entities();
        assert!(ents.contains("t") && ents.contains("armed"));
    }

    #[test]
    fn an_asserted_fact_cannot_fire_a_reflex() {
        use obc_memory::world::Origin;
        // The whole point of the gate: an agent writing a temperature does not get to
        // drive an automatic physical response. Same entity, same value, same rule — only
        // the origin differs.
        let observed = WorldMemory::open_in_memory().unwrap();
        observed
            .observe_as(
                "sensor.temperature",
                json!({"value": 30.0, "n": 2}),
                1_000,
                1_000,
                "fusion",
                Origin::Observed,
            )
            .unwrap();
        assert_eq!(
            ReflexEngine::new(vec![fan_rule()])
                .tick(&observed, 2_000)
                .unwrap()
                .len(),
            1,
            "a real reading fires the reflex"
        );

        let asserted = WorldMemory::open_in_memory().unwrap();
        asserted
            .observe_as(
                "sensor.temperature",
                json!({"value": 30.0, "n": 2}),
                1_000,
                1_000,
                "agent",
                Origin::Asserted,
            )
            .unwrap();
        assert!(
            ReflexEngine::new(vec![fan_rule()])
                .tick(&asserted, 2_000)
                .unwrap()
                .is_empty(),
            "an agent's claim about the temperature must not actuate anything"
        );
    }

    #[test]
    fn an_instructed_fact_is_also_not_evidence() {
        use obc_memory::world::Origin;
        // A human typing a reading is authoritative about intent, not about the world.
        // Someone who wants an actuator stopped should command that, not report a value
        // and let safing infer it.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "sensor.temperature",
                json!({"value": 30.0, "n": 2}),
                1_000,
                1_000,
                "operator",
                Origin::Instructed,
            )
            .unwrap();
        assert!(ReflexEngine::new(vec![fan_rule()])
            .tick(&world, 2_000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn widening_the_trust_set_re_enables_asserted_facts() {
        use obc_memory::world::{Origin, OriginSet};
        // The escape hatch exists for read-only and simulation engines; it is deliberately
        // explicit, because using it on an actuating engine re-opens the hazard.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "sensor.temperature",
                json!({"value": 30.0, "n": 2}),
                1_000,
                1_000,
                "agent",
                Origin::Asserted,
            )
            .unwrap();
        let permissive = ReflexEngine::new(vec![fan_rule()]).with_trusted_origins(OriginSet::ALL);
        assert_eq!(permissive.tick(&world, 2_000).unwrap().len(), 1);
    }

    #[test]
    fn tick_reads_world_memory_and_fires() {
        let world = WorldMemory::open_in_memory().unwrap();
        // sensor-fusion shape: {value, std_dev, n}
        world
            .observe(
                "sensor.temperature",
                json!({"value": 30.0, "n": 2}),
                1_000,
                1_000,
                "fusion",
            )
            .unwrap();
        let e = ReflexEngine::new(vec![fan_rule()]);
        let fired = e.tick(&world, 2_000).unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "fan-on-hot");

        // when the world cools below threshold, the reflex stops firing
        world
            .observe(
                "sensor.temperature",
                json!({"value": 20.0, "n": 2}),
                3_000,
                3_000,
                "fusion",
            )
            .unwrap();
        assert!(e.tick(&world, 4_000).unwrap().is_empty());
    }

    fn snap(pairs: &[(&str, f64)]) -> Snapshot {
        Snapshot::from_nums(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }

    fn rule_on_change(id: &str, entity: &str) -> ReflexRule {
        ReflexRule {
            id: id.to_string(),
            when: Condition::Sensor {
                entity: entity.to_string(),
                op: Cmp::Gt,
                value: 0.0,
            },
            then: Action::Escalate {
                reason: "seen".to_string(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: true,
            hold_ms: 0,
        }
    }

    #[test]
    fn fire_on_change_does_not_re_fire_on_the_same_fact() {
        // The bench failure: a condition that stays true re-fires at every debounce
        // interval, forever, on the same evidence. Debounce asks whether enough time has
        // passed; this asks whether anything happened.
        use obc_memory::world::{Origin, WorldMemory};
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as(
            "vision.hits",
            json!(1),
            1_000,
            1_000,
            "clawcam",
            Origin::Observed,
        )
        .unwrap();

        let e = ReflexEngine::new(vec![rule_on_change("r", "vision.hits")]);
        assert_eq!(e.tick(&w, 1_000).unwrap().len(), 1, "first sighting fires");
        for t in 1..6 {
            assert!(
                e.tick(&w, 1_000 + t * 3_600_000).unwrap().is_empty(),
                "hour {t}: re-fired on evidence that had not changed"
            );
        }

        // A genuinely new observation is a new row, and fires at once.
        w.observe_as(
            "vision.hits",
            json!(2),
            9_000,
            9_000,
            "clawcam",
            Origin::Observed,
        )
        .unwrap();
        assert_eq!(e.tick(&w, 9_000).unwrap().len(), 1, "new evidence fires");
    }

    #[test]
    fn a_rule_without_the_flag_still_re_fires_on_a_standing_condition() {
        // Safing depends on this. "The battery is still critical" is worth repeating,
        // and suppressing it because the reading has not changed is exactly backwards.
        use obc_memory::world::{Origin, WorldMemory};
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as(
            "power.critical",
            json!(1),
            1_000,
            1_000,
            "power",
            Origin::Observed,
        )
        .unwrap();
        let mut rule = rule_on_change("safe", "power.critical");
        rule.fire_on_change = false;
        let e = ReflexEngine::new(vec![rule]);
        assert_eq!(e.tick(&w, 1_000).unwrap().len(), 1);
        assert_eq!(
            e.tick(&w, 2_000).unwrap().len(),
            1,
            "still critical, still says so"
        );
    }

    fn rule_with_hold(hold_ms: u64, fire_on_change: bool) -> ReflexRule {
        ReflexRule {
            id: "held".to_string(),
            when: Condition::Sensor {
                entity: "x".to_string(),
                op: Cmp::Gt,
                value: 0.0,
            },
            then: Action::Escalate {
                reason: "held".to_string(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change,
            hold_ms,
        }
    }

    #[test]
    fn a_transient_does_not_satisfy_a_hold() {
        // The persistence question. One true tick is a candidate for a rule with
        // no hold; for a rule that asks for 3 s it is a transient until it has
        // been true, without a break, for 3 s of ticks.
        let e = ReflexEngine::new(vec![rule_with_hold(3_000, false)]);
        let on = snap(&[("x", 1.0)]);
        let off = snap(&[("x", 0.0)]);
        assert!(
            e.evaluate(&on, 0).is_empty(),
            "first true tick starts the hold"
        );
        assert!(e.evaluate(&off, 1_000).is_empty(), "a drop resets it");
        assert!(
            e.evaluate(&on, 2_000).is_empty(),
            "true again: a new run from 2 s"
        );
        assert!(e.evaluate(&on, 4_000).is_empty(), "2 s into the new run");
        assert_eq!(e.evaluate(&on, 5_000).len(), 1, "3 s held: fires");
        assert_eq!(
            e.evaluate(&on, 6_000).len(),
            1,
            "no edge flag, no debounce: keeps firing while held, as before"
        );
    }

    #[test]
    fn a_rule_with_no_hold_fires_on_its_first_true_tick() {
        // The default, and every rule written before the field existed.
        let e = ReflexEngine::new(vec![rule_with_hold(0, false)]);
        assert_eq!(e.evaluate(&snap(&[("x", 1.0)]), 0).len(), 1);
    }

    #[test]
    fn hold_composes_with_fire_on_change() {
        // Edge and persistence together: one fire per run, and only for a run
        // that outlasts the hold. A short run fires nothing at all.
        use obc_memory::world::{Origin, WorldMemory};
        let w = WorldMemory::open_in_memory().unwrap();
        let e = ReflexEngine::new(vec![rule_with_hold(2_000, true)]);
        let observe = |v: f64, t: u64| {
            w.observe_as("x", json!(v), t, t, "bench", Origin::Observed)
                .unwrap()
        };
        observe(1.0, 0);
        assert!(e.tick(&w, 0).unwrap().is_empty(), "hold starts");
        observe(0.0, 1_000);
        assert!(e.tick(&w, 1_000).unwrap().is_empty(), "a 1 s run: nothing");
        observe(1.0, 2_000);
        assert!(e.tick(&w, 2_000).unwrap().is_empty());
        observe(1.0, 3_000);
        assert!(e.tick(&w, 3_000).unwrap().is_empty(), "1 s in");
        observe(1.0, 4_000);
        assert_eq!(e.tick(&w, 4_000).unwrap().len(), 1, "2 s held: fires");
        assert!(
            e.tick(&w, 4_500).unwrap().is_empty(),
            "same row, still held: the host's edge is evidence identity, and nothing new was observed"
        );
        observe(1.0, 5_000);
        assert_eq!(
            e.tick(&w, 5_000).unwrap().len(),
            1,
            "a new row is new evidence and the hold is long satisfied — fires again (host semantics; \
             the node, which has no ids, fires once per run of truth)"
        );
    }

    // ── SensorBaseline: a threshold relative to the signal's own history ──

    fn baseline_rule(offset: f64, tau_s: f64) -> ReflexRule {
        ReflexRule {
            id: "rising".to_string(),
            when: Condition::SensorBaseline {
                entity: "t".to_string(),
                op: Cmp::Gt,
                offset,
                tau_s,
            },
            then: Action::Escalate {
                reason: "rising".to_string(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        }
    }

    #[test]
    fn the_first_sample_is_the_baseline_and_a_step_above_it_fires() {
        // τ = 10 s, offset 2. Ten seconds at 20 °C, then 25 °C. On the step tick
        // the baseline has already moved a little toward 25 — α = 1 − e^(−1/10) —
        // and the reading is still well above it.
        let e = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        for t in 0..10u64 {
            assert!(
                e.evaluate(&snap(&[("t", 20.0)]), t * 1_000).is_empty(),
                "steady at 20: nothing at t={t}"
            );
        }
        let b = e.baseline("t", 10.0).unwrap();
        assert!(
            (b.value - 20.0).abs() < 1e-12,
            "a constant signal is its own baseline"
        );
        assert_eq!(
            e.evaluate(&snap(&[("t", 25.0)]), 10_000).len(),
            1,
            "the step fires"
        );
        let b = e.baseline("t", 10.0).unwrap();
        let expected = 20.0 + (1.0 - (-0.1f64).exp()) * 5.0;
        assert!(
            (b.value - expected).abs() < 1e-12,
            "{} vs {expected}",
            b.value
        );
    }

    #[test]
    fn a_slow_drift_never_fires_because_the_baseline_follows_it() {
        // The invariance the rule exists for. A ramp of 0.01 °C/s climbs 10 °C
        // over 1000 s — five times the offset — and never fires, because an EMA
        // tracking a ramp lags it by a constant: s·(1−α)/α per sample step, which
        // is ≈ s·τ (0.095 °C here, against s·τ = 0.1). A fixed threshold at 22
        // would have fired at t = 200 s and stayed fired.
        let e = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        for t in 0..=1000u64 {
            let v = 20.0 + 0.01 * t as f64;
            assert!(
                e.evaluate(&snap(&[("t", v)]), t * 1_000).is_empty(),
                "drift fired at t={t}, v={v}"
            );
        }
        let b = e.baseline("t", 10.0).unwrap();
        let lag = 30.0 - b.value;
        let alpha = 1.0 - (-0.1f64).exp();
        let expected = 0.01 * (1.0 - alpha) / alpha;
        assert!(
            (lag - expected).abs() < 1e-9,
            "steady-state lag {lag} vs {expected}"
        );
        // …and a step on top of the drift still fires at once.
        assert_eq!(e.evaluate(&snap(&[("t", 35.0)]), 1_001_000).len(), 1);
    }

    #[test]
    fn the_time_constant_is_seconds_not_ticks() {
        // Two engines see the same signal — 20 °C through t = 10 s, 25 °C after —
        // one ticked every second, one every two. Both sample t = 10 (the last
        // 20) and t = 20, so both attribute the step to the same instant, and at
        // t = 20 s both hold the exact continuous answer 25 − 5·e^(−1): the tick
        // cadence did not change τ. (A sample stands for the interval that ends
        // at it — a step is dated to the sample before the first one that shows
        // it — so two cadences agree exactly when they share that sample.)
        let fast = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        let slow = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        let v = |t: u64| if t <= 10 { 20.0 } else { 25.0 };
        for t in 0..=20u64 {
            fast.evaluate(&snap(&[("t", v(t))]), t * 1_000);
            if t % 2 == 0 {
                slow.evaluate(&snap(&[("t", v(t))]), t * 1_000);
            }
        }
        let expected = 25.0 - 5.0 * (-1.0f64).exp();
        for (name, e) in [("1 Hz", &fast), ("0.5 Hz", &slow)] {
            let b = e.baseline("t", 10.0).unwrap().value;
            assert!((b - expected).abs() < 1e-12, "{name}: {b} vs {expected}");
        }
    }

    #[test]
    fn a_missing_reading_neither_fires_nor_moves_the_baseline() {
        let e = ReflexEngine::new(vec![baseline_rule(2.0, 10.0)]);
        e.evaluate(&snap(&[("t", 20.0)]), 0);
        assert!(e.evaluate(&snap(&[]), 1_000).is_empty());
        assert_eq!(
            e.baseline("t", 10.0).unwrap().at_ms,
            0,
            "not advanced on absence"
        );
        // Seen again after a 10 s gap at 25: one update with dt = 10 s = τ, not
        // ten with dt = 1. That one sample stands for the whole gap, so the
        // baseline absorbs 63 % of the step — 23.16 — and the reading is 1.84
        // above it, under the offset: no fire. A gap in the evidence is not a
        // step in the signal.
        assert!(e.evaluate(&snap(&[("t", 25.0)]), 10_000).is_empty());
        let expected = 20.0 + (1.0 - (-1.0f64).exp()) * 5.0;
        assert!((e.baseline("t", 10.0).unwrap().value - expected).abs() < 1e-12);
    }

    #[test]
    fn a_bare_eval_has_no_history_and_answers_false() {
        // Only an engine owns baselines. The pure `eval` is what the `And`/`Or`
        // tests use; a baseline leaf there is false, like any missing evidence.
        assert!(!baseline_rule(2.0, 10.0).when.eval(&snap(&[("t", 100.0)])));
    }

    #[test]
    fn a_baseline_rule_the_node_would_refuse_fails_validation() {
        let mut r = baseline_rule(2.0, 0.0);
        assert!(r.validate().unwrap_err().contains("tau_s"));
        r = baseline_rule(2.0, f64::NAN);
        assert!(r.validate().is_err());
        r = baseline_rule(f64::INFINITY, 10.0);
        assert!(r.validate().unwrap_err().contains("offset"));
        assert!(
            baseline_rule(-2.0, 0.5).validate().is_ok(),
            "a falling rule is fine"
        );
    }

    #[test]
    fn baseline_condition_serializes_to_the_node_wire_form() {
        let c = baseline_rule(3.0, 60.0).when;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sensor_baseline","entity":"t","op":"gt","offset":3.0,"tau_s":60.0}"#
        );
        assert_eq!(serde_json::from_str::<Condition>(&json).unwrap(), c);
        let mut ents = HashSet::new();
        c.collect_entities(&mut ents);
        assert!(ents.contains("t"), "the snapshot has to fetch the entity");
    }

    #[test]
    fn hold_ms_is_optional_on_the_wire() {
        // A rule written before the field existed still parses, and holds nothing.
        let js = r#"{"id":"r","when":{"type":"sensor","entity":"x","op":"gt","value":0.0},"then":{"type":"escalate","reason":"r"}}"#;
        let r: ReflexRule = serde_json::from_str(js).unwrap();
        assert_eq!(r.hold_ms, 0);
        let js = serde_json::to_string(&rule_with_hold(1_500, false)).unwrap();
        assert!(js.contains("\"hold_ms\":1500"), "{js}");
        assert_eq!(
            serde_json::from_str::<ReflexRule>(&js).unwrap().hold_ms,
            1_500
        );
    }

    #[test]
    fn a_snapshot_without_ids_never_suppresses() {
        // Value-only snapshots (a node, a simulation) cannot speak to evidence identity.
        // Missing identity is not evidence of sameness, so it must not silence a rule.
        let e = ReflexEngine::new(vec![rule_on_change("r", "x")]);
        let s = snap(&[("x", 1.0)]);
        assert_eq!(e.evaluate(&s, 1_000).len(), 1);
        assert_eq!(e.evaluate(&s, 2_000).len(), 1, "no ids, no suppression");
    }

    fn snap_vals(pairs: &[(&str, Value)]) -> Snapshot {
        let mut s = Snapshot::new();
        for (k, v) in pairs {
            s.vals.insert(k.to_string(), v.clone());
        }
        s
    }

    #[test]
    fn state_condition_matches_categorical_modes() {
        // bare-string fact value
        let bare = Condition::State {
            entity: "net.mode".into(),
            field: None,
            equals: "offline".into(),
        };
        assert!(bare.eval(&snap_vals(&[("net.mode", json!("offline"))])));
        assert!(!bare.eval(&snap_vals(&[("net.mode", json!("online"))])));

        // nested-field fact value (power.mode object)
        let nested = Condition::State {
            entity: "power.mode".into(),
            field: Some("mode".into()),
            equals: "critical".into(),
        };
        assert!(nested.eval(&snap_vals(&[(
            "power.mode",
            json!({"mode": "critical", "soc_pct": 8.0})
        )])));
        assert!(!nested.eval(&snap_vals(&[("power.mode", json!({"mode": "normal"}))])));
        // missing entity ⇒ false
        assert!(!nested.eval(&snap_vals(&[])));
    }

    #[test]
    fn state_condition_roundtrips() {
        let c = Condition::State {
            entity: "power.mode".into(),
            field: Some("mode".into()),
            equals: "critical".into(),
        };
        let js = serde_json::to_string(&c).unwrap();
        assert!(js.contains("\"type\":\"state\""));
        assert_eq!(serde_json::from_str::<Condition>(&js).unwrap(), c);
    }

    #[test]
    fn tick_fires_state_rule_from_world_memory() {
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe(
                "power.mode",
                json!({"mode": "critical", "soc_pct": 5.0}),
                1_000,
                1_000,
                "power",
            )
            .unwrap();
        let rule = ReflexRule {
            id: "safe-power-critical".into(),
            when: Condition::State {
                entity: "power.mode".into(),
                field: Some("mode".into()),
                equals: "critical".into(),
            },
            then: Action::Escalate {
                reason: "battery critical".into(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        };
        let e = ReflexEngine::new(vec![rule]);
        assert_eq!(e.tick(&world, 2_000).unwrap().len(), 1);
    }

    fn fan_rule() -> ReflexRule {
        ReflexRule {
            id: "fan-on-hot".to_string(),
            when: Condition::Sensor {
                entity: "sensor.temperature".to_string(),
                op: Cmp::Gt,
                value: 28.0,
            },
            then: Action::GpioWrite {
                node_id: "node-1".to_string(),
                pin: 18,
                value: 1,
            },
            debounce_ms: 500,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        }
    }

    #[test]
    fn fires_when_condition_holds() {
        let e = ReflexEngine::new(vec![fan_rule()]);
        let fired = e.evaluate(&snap(&[("sensor.temperature", 30.0)]), 1_000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "fan-on-hot");
    }

    fn slot_cond(slot: u8, default: f64) -> Condition {
        Condition::SensorSlot {
            entity: "sensor.temperature".to_string(),
            op: Cmp::Gt,
            slot,
            min: 20.0,
            max: 60.0,
            default,
        }
    }

    #[test]
    fn a_slot_bound_threshold_evaluates_at_its_default_on_the_host() {
        // 20 + 0.5·40 = 40 °C: the host has no descending path, so the rule
        // holds its default — the same answer a freshly booted node gives.
        let c = slot_cond(3, 0.5);
        assert!(c.eval(&snap(&[("sensor.temperature", 45.0)])));
        assert!(!c.eval(&snap(&[("sensor.temperature", 35.0)])));
        assert!(!c.eval(&snap(&[])));
        let mut ents = HashSet::new();
        c.collect_entities(&mut ents);
        assert!(ents.contains("sensor.temperature"));
    }

    #[test]
    fn a_slot_the_node_cannot_hold_fails_validation_and_names_the_rule() {
        let mut r = fan_rule();
        r.when = slot_cond(16, 0.5);
        let err = r.validate().unwrap_err();
        assert!(
            err.contains("fan-on-hot") && err.contains("slot 16"),
            "{err}"
        );
        r.when = Condition::Or {
            any: vec![slot_cond(1, 1.5)],
        };
        assert!(r.validate().unwrap_err().contains("not in [0, 1]"));
        r.when = slot_cond(15, 1.0);
        assert!(r.validate().is_ok());
        assert!(
            fan_rule().validate().is_ok(),
            "a literal threshold has nothing to validate"
        );
    }

    #[test]
    fn slot_condition_serializes_to_the_node_wire_form() {
        let json = serde_json::to_string(&slot_cond(3, 0.5)).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sensor_slot","entity":"sensor.temperature","op":"gt","slot":3,"min":20.0,"max":60.0,"default":0.5}"#
        );
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, slot_cond(3, 0.5));
    }

    #[test]
    fn does_not_fire_when_condition_false_or_entity_missing() {
        let e = ReflexEngine::new(vec![fan_rule()]);
        assert!(e
            .evaluate(&snap(&[("sensor.temperature", 20.0)]), 1_000)
            .is_empty());
        assert!(e.evaluate(&snap(&[("other", 99.0)]), 2_000).is_empty());
    }

    #[test]
    fn debounce_suppresses_rapid_refire() {
        let e = ReflexEngine::new(vec![fan_rule()]);
        let s = snap(&[("sensor.temperature", 30.0)]);
        assert_eq!(e.evaluate(&s, 1_000).len(), 1); // fires
        assert_eq!(e.evaluate(&s, 1_200).len(), 0); // within 500ms debounce
        assert_eq!(e.evaluate(&s, 1_600).len(), 1); // after debounce
    }

    #[test]
    fn max_rate_hz_enforced() {
        let mut r = fan_rule();
        r.debounce_ms = 0;
        r.max_rate_hz = Some(2.0); // ≤2/sec ⇒ min 500ms
        let e = ReflexEngine::new(vec![r]);
        let s = snap(&[("sensor.temperature", 30.0)]);
        assert_eq!(e.evaluate(&s, 0).len(), 1);
        assert_eq!(e.evaluate(&s, 400).len(), 0);
        assert_eq!(e.evaluate(&s, 500).len(), 1);
    }

    #[test]
    fn and_or_conditions() {
        let cond = Condition::And {
            all: vec![
                Condition::Sensor {
                    entity: "t".into(),
                    op: Cmp::Gt,
                    value: 28.0,
                },
                Condition::Or {
                    any: vec![
                        Condition::GpioEq {
                            entity: "armed".into(),
                            value: 1,
                        },
                        Condition::Sensor {
                            entity: "h".into(),
                            op: Cmp::Ge,
                            value: 80.0,
                        },
                    ],
                },
            ],
        };
        assert!(cond.eval(&snap(&[("t", 30.0), ("armed", 1.0)])));
        assert!(cond.eval(&snap(&[("t", 30.0), ("armed", 0.0), ("h", 85.0)])));
        assert!(!cond.eval(&snap(&[("t", 30.0), ("armed", 0.0), ("h", 50.0)])));
        assert!(!cond.eval(&snap(&[("t", 20.0), ("armed", 1.0)]))); // temp gate fails
    }

    #[test]
    fn escalate_action_fires() {
        let rule = ReflexRule {
            id: "novelty".to_string(),
            when: Condition::Sensor {
                entity: "motion".into(),
                op: Cmp::Eq,
                value: 1.0,
            },
            then: Action::Escalate {
                reason: "unexpected motion".to_string(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        };
        let e = ReflexEngine::new(vec![rule]);
        let fired = e.evaluate(&snap(&[("motion", 1.0)]), 1);
        assert_eq!(fired.len(), 1);
        assert!(matches!(fired[0].action, Action::Escalate { .. }));
    }

    #[test]
    fn rule_serde_roundtrip() {
        let rule = fan_rule();
        let js = serde_json::to_string(&rule).unwrap();
        // wire shape is stable + readable (for pushing to nodes)
        assert!(js.contains("\"type\":\"sensor\""));
        assert!(js.contains("\"type\":\"gpio_write\""));
        let back: ReflexRule = serde_json::from_str(&js).unwrap();
        assert_eq!(back, rule);
    }

    #[test]
    fn publish_action_roundtrips() {
        let a = Action::Publish {
            topic: "obc/alerts".to_string(),
            payload: json!({"level": "warn"}),
        };
        let back: Action = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn move_action_roundtrips() {
        let a = Action::Move {
            command: MovementCommand::ServoAngle {
                name: "arm".into(),
                channel: 0,
                degrees: 45.0,
            },
        };
        let js = serde_json::to_string(&a).unwrap();
        assert!(js.contains("\"type\":\"move\""));
        assert!(js.contains("\"type\":\"servo_angle\""));
        assert_eq!(serde_json::from_str::<Action>(&js).unwrap(), a);
    }

    #[tokio::test]
    async fn move_action_applies_through_movement_controller() {
        use obc_movement::LoggingActuatorSink;
        use obc_safety::limits::{SafetyGate, SafetyLimit};

        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let mut limit = SafetyLimit::new("n1", "servo_angle");
        limit.allowed_pins = Some(vec![0]);
        limit.value_min = Some(0);
        limit.value_max = Some(180);
        let movement = Arc::new(
            MovementController::new(
                "n1",
                Arc::new(SafetyGate::new(vec![limit])),
                Arc::new(LoggingActuatorSink),
            )
            .with_world_memory(Arc::clone(&world)),
        );
        let inner: Arc<dyn ActionSink> = Arc::new(LoggingActionSink);
        let sink = MovementActionSink::new(movement, inner);

        let fired = vec![FiredReflex {
            rule_id: "swivel".into(),
            action: Action::Move {
                command: MovementCommand::ServoAngle {
                    name: "arm".into(),
                    channel: 0,
                    degrees: 90.0,
                },
            },
        }];
        dispatch(&fired, &sink).await.unwrap();

        // The reflex Move was applied through the gate and recorded in memory.
        let fact = world.current("actuator.arm").unwrap().unwrap();
        assert_eq!(fact.value["tool"], "servo_angle");
        assert!((fact.value["value"].as_f64().unwrap() - 90.0).abs() < 1e-9);
    }

    #[derive(Default)]
    struct MockSink {
        calls: Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl ActionSink for MockSink {
        async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("gpio:{node_id}:{pin}:{value}"));
            Ok(())
        }
        async fn publish(&self, topic: &str, _payload: &Value) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(format!("pub:{topic}"));
            Ok(())
        }
        async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(format!("esc:{reason}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn controller_ticks_and_dispatches_gpio() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe(
                "sensor.temperature",
                json!({"value": 30.0}),
                1_000,
                1_000,
                "f",
            )
            .unwrap();
        let sink = Arc::new(MockSink::default());
        let sink_dyn: Arc<dyn ActionSink> = sink.clone();
        let ctl = ReflexController::new(
            ReflexEngine::new(vec![fan_rule()]),
            Arc::clone(&world),
            sink_dyn,
        );

        let fired = ctl.tick_and_dispatch(2_000).await.unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(
            sink.calls.lock().unwrap().as_slice(),
            &["gpio:node-1:18:1".to_string()]
        );
    }

    #[tokio::test]
    async fn controller_records_fire_metrics() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe(
                "sensor.temperature",
                json!({"value": 30.0}),
                1_000,
                1_000,
                "f",
            )
            .unwrap();
        let metrics = Arc::new(obc_observability::MetricsRegistry::new());
        let sink: Arc<dyn ActionSink> = Arc::new(LoggingActionSink);
        let ctl = ReflexController::new(
            ReflexEngine::new(vec![fan_rule()]),
            Arc::clone(&world),
            sink,
        )
        .with_metrics(Arc::clone(&metrics));
        ctl.tick_and_dispatch(2_000).await.unwrap();
        assert_eq!(metrics.counter("reflex.fired_total").get(), 1);
        assert_eq!(metrics.counter("reflex.rule.fan-on-hot").get(), 1);
        assert_eq!(metrics.counter("reflex.action.gpio_write").get(), 1);
    }

    #[tokio::test]
    async fn dispatch_routes_each_action_kind() {
        let sink = MockSink::default();
        let fired = vec![
            FiredReflex {
                rule_id: "a".into(),
                action: Action::Publish {
                    topic: "t".into(),
                    payload: json!(1),
                },
            },
            FiredReflex {
                rule_id: "b".into(),
                action: Action::Escalate {
                    reason: "why".into(),
                },
            },
        ];
        dispatch(&fired, &sink).await.unwrap();
        assert_eq!(
            sink.calls.lock().unwrap().as_slice(),
            &["pub:t".to_string(), "esc:why".to_string()]
        );
    }

    #[test]
    fn escalation_budget_sliding_window() {
        let b = EscalationBudget::new(2, 60_000);
        assert!(b.allow(0));
        assert!(b.allow(1_000));
        assert!(!b.allow(2_000)); // 2 already used within the window
        assert!(b.allow(61_001)); // the t=0 escalation has expired
    }

    #[test]
    fn escalation_budget_zero_is_unlimited() {
        let b = EscalationBudget::new(0, 60_000);
        for t in 0..10 {
            assert!(b.allow(t));
        }
    }

    #[tokio::test]
    async fn controller_budget_suppresses_extra_escalations() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world.observe("motion", json!(1.0), 0, 0, "pir").unwrap();
        let rule = ReflexRule {
            id: "e".into(),
            when: Condition::Sensor {
                entity: "motion".into(),
                op: Cmp::Eq,
                value: 1.0,
            },
            then: Action::Escalate {
                reason: "motion".into(),
            },
            debounce_ms: 0,
            max_rate_hz: None,
            fire_on_change: false,
            hold_ms: 0,
        };
        let sink = Arc::new(MockSink::default());
        let sink_dyn: Arc<dyn ActionSink> = sink.clone();
        let ctl =
            ReflexController::new(ReflexEngine::new(vec![rule]), Arc::clone(&world), sink_dyn)
                .with_escalation_budget(EscalationBudget::per_minute(1));

        let f1 = ctl.tick_and_dispatch(1_000).await.unwrap();
        let f2 = ctl.tick_and_dispatch(2_000).await.unwrap();
        assert_eq!(f1.len(), 1);
        assert_eq!(f2.len(), 1); // the reflex still fires both ticks
                                 // but only one escalation was actually dispatched (budget = 1/min)
        assert_eq!(sink.calls.lock().unwrap().len(), 1);
    }
}

// ── The vocabulary of an escalation ─────────────────────────────────────────
// `Action::Escalate` above hands control to System 2. These two say what that
// escalation *is*: how urgent, and whether it is a periodic digest of earlier
// ones rather than a fresh event.
//
// They lived in the agent's `notify` module until 2026-08-13 — the escalation
// vocabulary inside the escalation *delivery*, which is the same arrangement
// `RiskClass` had inside `tools::traits` and `NodeState` had inside `fleet`.
// It is the arrangement that keeps producing back-edges: `spine` needed to
// classify an escalation for its mesh view and had to name `agent` to do it,
// and that one `use` line was in five dependency cycles.
//
// Nothing here has a dependency. Forty-five lines, and moving them turned an
// edge that four modules were routed through.

/// Escalation severity, for routing (a channel can require a minimum). `Info < Warning
/// < Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Severity {
    Info,
    #[default]
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
    /// Parse a severity name for a channel's *minimum*; unknown/none → `Info` (accept all).
    pub fn from_name(s: Option<&str>) -> Severity {
        match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
            Some("critical") => Severity::Critical,
            Some("warning") => Severity::Warning,
            _ => Severity::Info,
        }
    }
    /// Classify an escalation reason by keywords. Escalations default to `Warning`; clear
    /// danger words raise it to `Critical`.
    pub fn classify(reason: &str) -> Severity {
        let r = reason.to_ascii_lowercase();
        const CRIT: [&str; 6] = [
            "critical",
            "presumed lost",
            "alarm",
            "overheat",
            "emergency",
            "over limit",
        ];
        if CRIT.iter().any(|k| r.contains(k)) {
            Severity::Critical
        } else {
            Severity::Warning
        }
    }
}

/// Prefix on every periodic digest message. Also used to exclude prior digests from the
/// raw escalation history when the next digest is built (so digests don't compound).
pub const DIGEST_PREFIX: &str = "OBC escalation digest";

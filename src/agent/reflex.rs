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

use crate::movement::{MovementCommand, MovementController};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

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

impl Condition {
    /// Evaluate against a [`Snapshot`].
    pub fn eval(&self, snap: &Snapshot) -> bool {
        match self {
            Condition::Sensor { entity, op, value } => {
                snap.nums.get(entity).is_some_and(|v| op.test(*v, *value))
            }
            Condition::GpioEq { entity, value } => snap
                .nums
                .get(entity)
                .is_some_and(|v| Cmp::Eq.test(*v, *value as f64)),
            Condition::State { entity, field, equals } => {
                snap.vals.get(entity).is_some_and(|v| {
                    let s = match field {
                        Some(f) => v.get(f).and_then(|x| x.as_str()),
                        None => v.as_str(),
                    };
                    s == Some(equals.as_str())
                })
            }
            Condition::And { all } => all.iter().all(|c| c.eval(snap)),
            Condition::Or { any } => any.iter().any(|c| c.eval(snap)),
        }
    }

    /// Collect all entity names this condition references (for snapshotting).
    pub fn collect_entities(&self, set: &mut HashSet<String>) {
        match self {
            Condition::Sensor { entity, .. }
            | Condition::GpioEq { entity, .. }
            | Condition::State { entity, .. } => {
                set.insert(entity.clone());
            }
            Condition::And { all } => all.iter().for_each(|c| c.collect_entities(set)),
            Condition::Or { any } => any.iter().for_each(|c| c.collect_entities(set)),
        }
    }
}

/// The action a fired reflex performs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Action {
    /// Drive a node's GPIO pin (still bounded by the node's Track 0 `SafetyGate`).
    GpioWrite { node_id: String, pin: i64, value: i64 },
    /// Publish a payload to a spine topic.
    Publish { topic: String, payload: Value },
    /// Hand control up to System 2 (wake the LLM agent) with a reason.
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
}

impl ReflexRule {
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
#[derive(Debug, Default)]
pub struct ReflexEngine {
    rules: Vec<ReflexRule>,
    last_fire: Mutex<HashMap<String, u64>>,
}

impl ReflexEngine {
    /// Build an engine from a set of rules.
    pub fn new(rules: Vec<ReflexRule>) -> Self {
        Self {
            rules,
            last_fire: Mutex::new(HashMap::new()),
        }
    }

    /// Number of rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate all rules against `snapshot` at `now_ms`; returns the actions to
    /// perform (respecting debounce/rate), and records fire times.
    pub fn evaluate(&self, snapshot: &Snapshot, now_ms: u64) -> Vec<FiredReflex> {
        let mut guard = self
            .last_fire
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut fired = Vec::new();
        for rule in &self.rules {
            if !rule.when.eval(snapshot) {
                continue;
            }
            let min_interval = rule.min_interval_ms();
            if min_interval > 0 {
                if let Some(&last) = guard.get(&rule.id) {
                    if now_ms.saturating_sub(last) < min_interval {
                        continue;
                    }
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
        world: &crate::memory::world::WorldMemory,
        now_ms: u64,
    ) -> anyhow::Result<Vec<FiredReflex>> {
        let mut snapshot = Snapshot::new();
        for entity in self.referenced_entities() {
            if let Some(fact) = world.current(&entity)? {
                if let Some(v) = fact_to_f64(&fact.value) {
                    snapshot.nums.insert(entity.clone(), v);
                }
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
            Action::GpioWrite { node_id, pin, value } => {
                sink.gpio_write(node_id, *pin, *value).await?
            }
            Action::Publish { topic, payload } => sink.publish(topic, payload).await?,
            Action::Escalate { reason } => sink.escalate(reason).await?,
            Action::Move { command } => sink.move_actuator(command).await?,
        }
    }
    Ok(())
}

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
        tracing::info!(reason, "reflex: escalate to System 2 (dry-run)");
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

/// Routes reflex actions over the MQTT spine: a GPIO write becomes a `gpio_write`
/// tool call to the node (bounded there by the firmware Track 0 `SafetyGate`),
/// publishes go to the topic, and escalations are published to `obc/escalation`
/// for System 2 (the gateway/agent) to act on. Best-effort: spine errors are
/// logged, not propagated, so one unreachable node never stalls the reflex loop.
pub struct SpineActionSink {
    spine: Arc<crate::spine::SpineClient>,
}

impl SpineActionSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<crate::spine::SpineClient>) -> Self {
        Self { spine }
    }
}

#[async_trait]
impl ActionSink for SpineActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        let args = serde_json::json!({ "pin": pin, "value": value });
        if let Err(e) = self.spine.invoke_tool(node_id, "gpio_write", args).await {
            tracing::warn!(node_id, pin, value, error = %e, "reflex gpio_write over spine failed");
        }
        Ok(())
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        if let Err(e) = self.spine.publish(topic, payload).await {
            tracing::warn!(topic, error = %e, "reflex publish over spine failed");
        }
        Ok(())
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        let topic = format!("{}/escalation", crate::spine::TOPIC_PREFIX);
        let payload = serde_json::json!({ "reason": reason });
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(error = %e, "reflex escalation publish failed");
        }
        tracing::info!(reason, "reflex: escalated to System 2");
        Ok(())
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        // Publish the typed command to the movement topic; the movement node /
        // controller applies it under its own Track 0 bounds.
        let topic = format!("{}/movement", crate::spine::TOPIC_PREFIX);
        let payload = serde_json::to_value(command).unwrap_or(Value::Null);
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(actuator = command.name(), error = %e, "reflex movement publish failed");
        }
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
    world: Arc<crate::memory::world::WorldMemory>,
    sink: Arc<dyn ActionSink>,
    escalation_budget: Option<EscalationBudget>,
    metrics: Option<Arc<crate::observability::MetricsRegistry>>,
}

impl ReflexController {
    /// Build a controller.
    pub fn new(
        engine: ReflexEngine,
        world: Arc<crate::memory::world::WorldMemory>,
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
    pub fn with_metrics(mut self, metrics: Arc<crate::observability::MetricsRegistry>) -> Self {
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
                Action::GpioWrite { node_id, pin, value } => {
                    self.sink.gpio_write(node_id, *pin, *value).await?
                }
                Action::Publish { topic, payload } => self.sink.publish(topic, payload).await?,
                Action::Escalate { reason } => {
                    let allowed = self
                        .escalation_budget
                        .as_ref()
                        .map_or(true, |b| b.allow(now_ms));
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
    use super::*;
    use crate::memory::world::WorldMemory;
    use serde_json::json;

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
                    Condition::Sensor { entity: "t".into(), op: Cmp::Gt, value: 1.0 },
                    Condition::Or {
                        any: vec![Condition::GpioEq { entity: "armed".into(), value: 1 }],
                    },
                ],
            },
            then: Action::Escalate { reason: "x".into() },
            debounce_ms: 0,
            max_rate_hz: None,
        };
        let e = ReflexEngine::new(vec![rule]);
        let ents = e.referenced_entities();
        assert!(ents.contains("t") && ents.contains("armed"));
    }

    #[test]
    fn tick_reads_world_memory_and_fires() {
        let world = WorldMemory::open_in_memory().unwrap();
        // sensor-fusion shape: {value, std_dev, n}
        world
            .observe("sensor.temperature", json!({"value": 30.0, "n": 2}), 1_000, 1_000, "fusion")
            .unwrap();
        let e = ReflexEngine::new(vec![fan_rule()]);
        let fired = e.tick(&world, 2_000).unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "fan-on-hot");

        // when the world cools below threshold, the reflex stops firing
        world
            .observe("sensor.temperature", json!({"value": 20.0, "n": 2}), 3_000, 3_000, "fusion")
            .unwrap();
        assert!(e.tick(&world, 4_000).unwrap().is_empty());
    }

    fn snap(pairs: &[(&str, f64)]) -> Snapshot {
        Snapshot::from_nums(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
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
        assert!(nested.eval(&snap_vals(&[("power.mode", json!({"mode": "critical", "soc_pct": 8.0}))])));
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
            .observe("power.mode", json!({"mode": "critical", "soc_pct": 5.0}), 1_000, 1_000, "power")
            .unwrap();
        let rule = ReflexRule {
            id: "safe-power-critical".into(),
            when: Condition::State {
                entity: "power.mode".into(),
                field: Some("mode".into()),
                equals: "critical".into(),
            },
            then: Action::Escalate { reason: "battery critical".into() },
            debounce_ms: 0,
            max_rate_hz: None,
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
        }
    }

    #[test]
    fn fires_when_condition_holds() {
        let e = ReflexEngine::new(vec![fan_rule()]);
        let fired = e.evaluate(&snap(&[("sensor.temperature", 30.0)]), 1_000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "fan-on-hot");
    }

    #[test]
    fn does_not_fire_when_condition_false_or_entity_missing() {
        let e = ReflexEngine::new(vec![fan_rule()]);
        assert!(e.evaluate(&snap(&[("sensor.temperature", 20.0)]), 1_000).is_empty());
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
                Condition::Sensor { entity: "t".into(), op: Cmp::Gt, value: 28.0 },
                Condition::Or {
                    any: vec![
                        Condition::GpioEq { entity: "armed".into(), value: 1 },
                        Condition::Sensor { entity: "h".into(), op: Cmp::Ge, value: 80.0 },
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
            when: Condition::Sensor { entity: "motion".into(), op: Cmp::Eq, value: 1.0 },
            then: Action::Escalate { reason: "unexpected motion".to_string() },
            debounce_ms: 0,
            max_rate_hz: None,
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
            command: MovementCommand::ServoAngle { name: "arm".into(), channel: 0, degrees: 45.0 },
        };
        let js = serde_json::to_string(&a).unwrap();
        assert!(js.contains("\"type\":\"move\""));
        assert!(js.contains("\"type\":\"servo_angle\""));
        assert_eq!(serde_json::from_str::<Action>(&js).unwrap(), a);
    }

    #[tokio::test]
    async fn move_action_applies_through_movement_controller() {
        use crate::movement::LoggingActuatorSink;
        use crate::security::limits::{SafetyGate, SafetyLimit};

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
            self.calls.lock().unwrap().push(format!("gpio:{node_id}:{pin}:{value}"));
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
            .observe("sensor.temperature", json!({"value": 30.0}), 1_000, 1_000, "f")
            .unwrap();
        let sink = Arc::new(MockSink::default());
        let sink_dyn: Arc<dyn ActionSink> = sink.clone();
        let ctl = ReflexController::new(ReflexEngine::new(vec![fan_rule()]), Arc::clone(&world), sink_dyn);

        let fired = ctl.tick_and_dispatch(2_000).await.unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(sink.calls.lock().unwrap().as_slice(), &["gpio:node-1:18:1".to_string()]);
    }

    #[tokio::test]
    async fn controller_records_fire_metrics() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe("sensor.temperature", json!({"value": 30.0}), 1_000, 1_000, "f")
            .unwrap();
        let metrics = Arc::new(crate::observability::MetricsRegistry::new());
        let sink: Arc<dyn ActionSink> = Arc::new(LoggingActionSink);
        let ctl = ReflexController::new(ReflexEngine::new(vec![fan_rule()]), Arc::clone(&world), sink)
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
                action: Action::Publish { topic: "t".into(), payload: json!(1) },
            },
            FiredReflex {
                rule_id: "b".into(),
                action: Action::Escalate { reason: "why".into() },
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
            when: Condition::Sensor { entity: "motion".into(), op: Cmp::Eq, value: 1.0 },
            then: Action::Escalate { reason: "motion".into() },
            debounce_ms: 0,
            max_rate_hz: None,
        };
        let sink = Arc::new(MockSink::default());
        let sink_dyn: Arc<dyn ActionSink> = sink.clone();
        let ctl = ReflexController::new(ReflexEngine::new(vec![rule]), Arc::clone(&world), sink_dyn)
            .with_escalation_budget(EscalationBudget::per_minute(1));

        let f1 = ctl.tick_and_dispatch(1_000).await.unwrap();
        let f2 = ctl.tick_and_dispatch(2_000).await.unwrap();
        assert_eq!(f1.len(), 1);
        assert_eq!(f2.len(), 1); // the reflex still fires both ticks
        // but only one escalation was actually dispatched (budget = 1/min)
        assert_eq!(sink.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn spine_sink_is_best_effort_when_disconnected() {
        use crate::config::SpineConfig;
        let spine = Arc::new(crate::spine::SpineClient::new(SpineConfig::default(), "test"));
        let sink = SpineActionSink::new(spine);
        // An unconnected spine makes the underlying calls fail, but the sink logs
        // and returns Ok so a reflex tick is never broken by a transient outage.
        assert!(sink.gpio_write("node-1", 18, 1).await.is_ok());
        assert!(sink.publish("obc/x", &json!({"a": 1})).await.is_ok());
        assert!(sink.escalate("why").await.is_ok());
    }
}

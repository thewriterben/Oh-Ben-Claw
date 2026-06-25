//! Safing reflex rules — System 1 behaviors that consume the suite mode hooks.
//!
//! The perception suites (power, comms, sensing, audio, …) record categorical
//! mode facts into world memory: `power.mode`, `net.mode`, `sensor.{q}`'s
//! `quality`, `audio.{stream}`'s `label`. On their own those are just
//! observations. This module turns them into *reactions* — canonical
//! [`ReflexRule`]s that fire deterministic, near-instant safing actions when a
//! mode goes bad, without waking the LLM (System 2).
//!
//! Each rule matches a mode with a [`Condition::State`] and performs a
//! conservative action:
//! - **power critical** → escalate (and, if an actuator is given, `Stop` it —
//!   bounded by the movement controller's Track 0 gate).
//! - **power low** → publish a "shed load" advisory.
//! - **net offline / degraded** → publish a connectivity-safing advisory so the
//!   system drops to local/low-bandwidth behavior.
//!
//! Rules are debounced so a persistent bad mode doesn't spam actions, and
//! escalations are additionally capped by the controller's escalation budget.
//! The set is appended to the operator's configured rules; nothing here fires
//! unless the corresponding suite is producing the mode fact.

use crate::agent::reflex::{Action, ActionSink, Cmp, Condition, ReflexRule};
use crate::movement::MovementCommand;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Topic safing advisories are published to (System 1 → local subscribers).
pub const SAFING_TOPIC: &str = "obc/safing";

/// Options controlling which safing rules are emitted.
#[derive(Debug, Clone, Default)]
pub struct SafingOptions {
    /// If set, `power.mode == critical` also issues a `Stop` to this actuator
    /// (`name`, `channel`) through the movement controller's Track 0 gate.
    pub stop_actuator: Option<(String, i64)>,
    /// Audio streams to watch for an `"alarm"` label → escalate (e.g. `["mic0"]`).
    pub alarm_streams: Vec<String>,
    /// Sensor quantities to watch for an out-of-range `quality` → escalate
    /// (distrust bad data), e.g. `["temperature"]`.
    pub unreliable_sensors: Vec<String>,
    /// Numeric over-limit guards: `(quantity, threshold)` → escalate when
    /// `sensor.{quantity}` value exceeds `threshold` (e.g. overheat).
    pub overheat: Vec<(String, f64)>,
    /// Debounce for every safing rule (ms). Default 5000 when `0`.
    pub debounce_ms: u64,
}

fn debounce(opts: &SafingOptions) -> u64 {
    if opts.debounce_ms == 0 {
        5_000
    } else {
        opts.debounce_ms
    }
}

fn state(entity: &str, field: &str, equals: &str) -> Condition {
    Condition::State {
        entity: entity.to_string(),
        field: Some(field.to_string()),
        equals: equals.to_string(),
    }
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

/// `power.mode == critical` → escalate to System 2 with a reason.
pub fn power_critical_escalate(opts: &SafingOptions) -> ReflexRule {
    rule(
        "safe-power-critical-escalate",
        state("power.mode", "mode", "critical"),
        Action::Escalate {
            reason: "battery critical — entering low-power safing".to_string(),
        },
        debounce(opts),
    )
}

/// `power.mode == critical` → `Stop` the configured actuator (Track 0–bounded).
/// Only produced when [`SafingOptions::stop_actuator`] is set.
pub fn power_critical_stop(opts: &SafingOptions) -> Option<ReflexRule> {
    let (name, channel) = opts.stop_actuator.clone()?;
    Some(rule(
        "safe-power-critical-stop",
        state("power.mode", "mode", "critical"),
        Action::Move {
            command: MovementCommand::Stop { name, channel },
        },
        debounce(opts),
    ))
}

/// `power.mode == low` → publish a "shed non-essential load" advisory.
pub fn power_low_shed(opts: &SafingOptions) -> ReflexRule {
    rule(
        "safe-power-low",
        state("power.mode", "mode", "low"),
        Action::Publish {
            topic: SAFING_TOPIC.to_string(),
            payload: json!({ "subsystem": "power", "mode": "low", "action": "shed_load" }),
        },
        debounce(opts),
    )
}

/// `net.mode == offline` → publish a degraded/offline-safing advisory.
pub fn net_offline_safe(opts: &SafingOptions) -> ReflexRule {
    rule(
        "safe-net-offline",
        state("net.mode", "mode", "offline"),
        Action::Publish {
            topic: SAFING_TOPIC.to_string(),
            payload: json!({ "subsystem": "comms", "mode": "offline", "action": "degraded_mode" }),
        },
        debounce(opts),
    )
}

/// `net.mode == degraded` → publish a degraded-mode advisory.
pub fn net_degraded_safe(opts: &SafingOptions) -> ReflexRule {
    rule(
        "safe-net-degraded",
        state("net.mode", "mode", "degraded"),
        Action::Publish {
            topic: SAFING_TOPIC.to_string(),
            payload: json!({ "subsystem": "comms", "mode": "degraded", "action": "reduce_bandwidth" }),
        },
        debounce(opts),
    )
}

/// `audio.{stream}` label == `"alarm"` → escalate to System 2.
pub fn audio_alarm_escalate(stream: &str, opts: &SafingOptions) -> ReflexRule {
    rule(
        &format!("safe-audio-alarm-{stream}"),
        state(&format!("audio.{stream}"), "label", "alarm"),
        Action::Escalate {
            reason: format!("alarm sound detected on {stream}"),
        },
        debounce(opts),
    )
}

/// `sensor.{quantity}` quality == `"out_of_range"` → escalate (distrust bad data).
pub fn sensor_unreliable_escalate(quantity: &str, opts: &SafingOptions) -> ReflexRule {
    rule(
        &format!("safe-sensor-unreliable-{quantity}"),
        state(&format!("sensor.{quantity}"), "quality", "out_of_range"),
        Action::Escalate {
            reason: format!("sensor {quantity} reading out of range"),
        },
        debounce(opts),
    )
}

/// `sensor.{quantity}` numeric value `>` threshold → escalate (overheat /
/// over-limit). Uses a numeric [`Condition::Sensor`]; `fact_to_f64` reads the
/// reading's `value` field from the sensing fact.
pub fn overheat_escalate(quantity: &str, threshold: f64, opts: &SafingOptions) -> ReflexRule {
    rule(
        &format!("safe-overheat-{quantity}"),
        Condition::Sensor {
            entity: format!("sensor.{quantity}"),
            op: Cmp::Gt,
            value: threshold,
        },
        Action::Escalate {
            reason: format!("{quantity} over limit ({threshold})"),
        },
        debounce(opts),
    )
}

/// The standard safing rule set for the given options. Order is stable.
pub fn standard_safing_rules(opts: &SafingOptions) -> Vec<ReflexRule> {
    let mut rules = vec![power_critical_escalate(opts)];
    if let Some(stop) = power_critical_stop(opts) {
        rules.push(stop);
    }
    rules.push(power_low_shed(opts));
    rules.push(net_offline_safe(opts));
    rules.push(net_degraded_safe(opts));
    for stream in &opts.alarm_streams {
        rules.push(audio_alarm_escalate(stream, opts));
    }
    for quantity in &opts.unreliable_sensors {
        rules.push(sensor_unreliable_escalate(quantity, opts));
    }
    for (quantity, threshold) in &opts.overheat {
        rules.push(overheat_escalate(quantity, *threshold, opts));
    }
    rules
}

// ── Safing executor (consume advisories in-process) ──────────────────────────

/// Host-side safing state, flipped by `obc/safing` advisories. Shareable across
/// the running loops (poll tasks, controllers) that should back off under stress.
/// Atomics so any task can read/observe it cheaply without a lock.
#[derive(Debug, Default)]
pub struct SafingState {
    shed_load: AtomicBool,
    degraded_net: AtomicBool,
    offline: AtomicBool,
}

impl SafingState {
    /// All-clear state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Non-essential load should be shed (e.g. pause polling, dim, sleep).
    pub fn shed_load(&self) -> bool {
        self.shed_load.load(Ordering::Relaxed)
    }

    /// The network is degraded — prefer local/low-bandwidth behavior.
    pub fn degraded_net(&self) -> bool {
        self.degraded_net.load(Ordering::Relaxed)
    }

    /// The network is fully offline.
    pub fn offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    /// Update flags from a safing advisory payload (the JSON published to
    /// [`SAFING_TOPIC`]). Logs on a 0→1 transition so the shed is visible.
    pub fn apply_advisory(&self, payload: &Value) {
        match payload.get("action").and_then(Value::as_str).unwrap_or("") {
            "shed_load" => self.set(&self.shed_load, "shed_load"),
            "degraded_mode" => {
                self.set(&self.offline, "offline");
                self.set(&self.degraded_net, "degraded_net");
            }
            "reduce_bandwidth" => self.set(&self.degraded_net, "degraded_net"),
            _ => {}
        }
    }

    fn set(&self, flag: &AtomicBool, name: &str) {
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!(flag = name, "safing engaged");
        }
    }

    /// Clear all flags (recovery — call when modes return to normal).
    pub fn clear(&self) {
        for (flag, name) in [
            (&self.shed_load, "shed_load"),
            (&self.degraded_net, "degraded_net"),
            (&self.offline, "offline"),
        ] {
            if flag.swap(false, Ordering::Relaxed) {
                tracing::info!(flag = name, "safing cleared");
            }
        }
    }
}

/// An [`ActionSink`] wrapper that consumes safing advisories in-process: a
/// `Publish` to [`SAFING_TOPIC`] updates a shared [`SafingState`] (so the host
/// actually backs off), and *also* forwards every action — including that publish
/// — to an inner sink for distributed consumers. Mirrors `MovementActionSink`.
pub struct SafingSink {
    state: Arc<SafingState>,
    inner: Arc<dyn ActionSink>,
}

impl SafingSink {
    /// Wrap an inner sink, tapping `obc/safing` publishes into `state`.
    pub fn new(state: Arc<SafingState>, inner: Arc<dyn ActionSink>) -> Self {
        Self { state, inner }
    }
}

#[async_trait]
impl ActionSink for SafingSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        self.inner.gpio_write(node_id, pin, value).await
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        if topic == SAFING_TOPIC {
            self.state.apply_advisory(payload);
        }
        self.inner.publish(topic, payload).await
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        self.inner.escalate(reason).await
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        self.inner.move_actuator(command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::reflex::{Action, ReflexEngine};
    use crate::comms::{CommsController, LinkReading, LinkThresholds};
    use crate::memory::world::WorldMemory;
    use crate::power::{BatteryReading, ChargeState, PowerController, PowerThresholds};
    use std::sync::Arc;

    fn opts_with_actuator() -> SafingOptions {
        SafingOptions {
            stop_actuator: Some(("arm".to_string(), 0)),
            debounce_ms: 0,
            ..Default::default()
        }
    }

    #[test]
    fn standard_set_includes_stop_only_with_actuator() {
        assert_eq!(standard_safing_rules(&SafingOptions::default()).len(), 4);
        assert_eq!(standard_safing_rules(&opts_with_actuator()).len(), 5);
    }

    #[test]
    fn rule_ids_are_unique_and_stable() {
        let rules = standard_safing_rules(&opts_with_actuator());
        let mut ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate rule ids");
    }

    // ── End-to-end: suite controller → world memory → reflex fires safing ──────

    #[test]
    fn power_critical_drives_escalate_and_stop_end_to_end() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let power = PowerController::new(PowerThresholds::default())
            .with_world_memory(Arc::clone(&world));
        // A real critical battery reading flows through the suite into world memory.
        let status = power
            .ingest(
                &BatteryReading {
                    soc_pct: 6.0,
                    voltage: None,
                    current_a: None,
                    charging: ChargeState::Discharging,
                    source: None,
                },
                1_000,
            )
            .unwrap();
        assert_eq!(status.mode.as_str(), "critical");

        let engine = ReflexEngine::new(standard_safing_rules(&opts_with_actuator()));
        let fired = engine.tick(&world, 2_000).unwrap();

        // Both the escalate and the actuator-stop reflexes fired off power.mode.
        assert!(fired.iter().any(|f| f.rule_id == "safe-power-critical-escalate"
            && matches!(f.action, Action::Escalate { .. })));
        assert!(fired.iter().any(|f| {
            f.rule_id == "safe-power-critical-stop"
                && matches!(&f.action, Action::Move { command: MovementCommand::Stop { name, channel } } if name == "arm" && *channel == 0)
        }));
    }

    #[test]
    fn healthy_power_fires_nothing() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let power = PowerController::new(PowerThresholds::default())
            .with_world_memory(Arc::clone(&world));
        power
            .ingest(
                &BatteryReading {
                    soc_pct: 85.0,
                    voltage: None,
                    current_a: None,
                    charging: ChargeState::Discharging,
                    source: None,
                },
                1_000,
            )
            .unwrap();
        let engine = ReflexEngine::new(standard_safing_rules(&SafingOptions::default()));
        assert!(engine.tick(&world, 2_000).unwrap().is_empty());
    }

    #[test]
    fn net_offline_publishes_safing_advisory_end_to_end() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let comms = CommsController::new(LinkThresholds::default())
            .with_world_memory(Arc::clone(&world));
        // Every link down ⇒ net.mode offline.
        comms
            .ingest(
                &LinkReading {
                    link: "wifi".to_string(),
                    rssi_dbm: None,
                    latency_ms: None,
                    loss_pct: None,
                    up: Some(false),
                    source: None,
                },
                1_000,
            )
            .unwrap();

        let engine = ReflexEngine::new(standard_safing_rules(&SafingOptions::default()));
        let fired = engine.tick(&world, 2_000).unwrap();
        let advisory = fired
            .iter()
            .find(|f| f.rule_id == "safe-net-offline")
            .expect("offline safing rule should fire");
        match &advisory.action {
            Action::Publish { topic, payload } => {
                assert_eq!(topic, SAFING_TOPIC);
                assert_eq!(payload["action"], "degraded_mode");
            }
            other => panic!("expected publish, got {other:?}"),
        }
    }

    #[test]
    fn audio_alarm_escalates_end_to_end() {
        use crate::audio::suite::{AudioController, HeardEvent, LoggingSpeechSink};
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let audio = AudioController::new(Arc::new(LoggingSpeechSink)).with_world_memory(Arc::clone(&world));
        audio
            .observe(
                &HeardEvent {
                    stream: "mic0".to_string(),
                    text: None,
                    label: Some("alarm".to_string()),
                    confidence: 0.98,
                    source: None,
                },
                1_000,
            )
            .unwrap();

        let opts = SafingOptions {
            alarm_streams: vec!["mic0".to_string()],
            ..Default::default()
        };
        let engine = ReflexEngine::new(standard_safing_rules(&opts));
        let fired = engine.tick(&world, 2_000).unwrap();
        assert!(fired
            .iter()
            .any(|f| f.rule_id == "safe-audio-alarm-mic0" && matches!(f.action, Action::Escalate { .. })));
    }

    #[test]
    fn out_of_range_sensor_escalates_end_to_end() {
        use crate::sensing::{QuantitySpec, Sample, SensingController};
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let sensing = SensingController::new(vec![(
            "temperature".to_string(),
            QuantitySpec { min: Some(-40.0), max: Some(85.0), max_staleness_ms: None, unit: None },
        )])
        .with_world_memory(Arc::clone(&world));
        sensing
            .ingest(&Sample { quantity: "temperature".into(), value: 200.0, unit: None, source: None }, 1_000)
            .unwrap();

        let opts = SafingOptions {
            unreliable_sensors: vec!["temperature".to_string()],
            ..Default::default()
        };
        let engine = ReflexEngine::new(standard_safing_rules(&opts));
        let fired = engine.tick(&world, 2_000).unwrap();
        assert!(fired.iter().any(|f| f.rule_id == "safe-sensor-unreliable-temperature"));
    }

    #[test]
    fn overheat_escalates_on_numeric_threshold_end_to_end() {
        use crate::sensing::{QuantitySpec, Sample, SensingController};
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        // No range limits, so the reading is in-range (quality ok) — the overheat
        // guard fires purely on the numeric value crossing the threshold.
        let sensing = SensingController::new(vec![(
            "cpu_temp".to_string(),
            QuantitySpec::default(),
        )])
        .with_world_memory(Arc::clone(&world));
        sensing
            .ingest(&Sample { quantity: "cpu_temp".into(), value: 92.0, unit: Some("C".into()), source: None }, 1_000)
            .unwrap();

        let opts = SafingOptions {
            overheat: vec![("cpu_temp".to_string(), 80.0)],
            ..Default::default()
        };
        let engine = ReflexEngine::new(standard_safing_rules(&opts));
        let fired = engine.tick(&world, 2_000).unwrap();
        assert!(fired.iter().any(|f| f.rule_id == "safe-overheat-cpu_temp"));

        // Below threshold ⇒ no fire.
        let world2 = Arc::new(WorldMemory::open_in_memory().unwrap());
        let s2 = SensingController::new(vec![]).with_world_memory(Arc::clone(&world2));
        s2.ingest(&Sample { quantity: "cpu_temp".into(), value: 50.0, unit: None, source: None }, 1_000).unwrap();
        let engine2 = ReflexEngine::new(standard_safing_rules(&opts));
        assert!(engine2.tick(&world2, 2_000).unwrap().is_empty());
    }

    #[test]
    fn safing_state_applies_advisories() {
        let s = SafingState::new();
        assert!(!s.shed_load());
        s.apply_advisory(&json!({ "action": "shed_load" }));
        assert!(s.shed_load());
        s.apply_advisory(&json!({ "action": "degraded_mode" }));
        assert!(s.offline() && s.degraded_net());
        s.clear();
        assert!(!s.shed_load() && !s.offline() && !s.degraded_net());
    }

    #[tokio::test]
    async fn safing_sink_taps_advisory_and_forwards() {
        use crate::agent::reflex::LoggingActionSink;
        let state = Arc::new(SafingState::new());
        let sink = SafingSink::new(Arc::clone(&state), Arc::new(LoggingActionSink));
        // a power-low safing publish flips shed_load (and is still forwarded).
        sink.publish(SAFING_TOPIC, &json!({ "action": "shed_load" })).await.unwrap();
        assert!(state.shed_load());
        // an unrelated topic does not touch safing state.
        let s2 = Arc::new(SafingState::new());
        let sink2 = SafingSink::new(Arc::clone(&s2), Arc::new(LoggingActionSink));
        sink2.publish("obc/other", &json!({ "action": "shed_load" })).await.unwrap();
        assert!(!s2.shed_load());
    }

    #[test]
    fn power_low_fires_shed_load() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let power = PowerController::new(PowerThresholds { low_pct: 20.0, critical_pct: 10.0 })
            .with_world_memory(Arc::clone(&world));
        power
            .ingest(
                &BatteryReading {
                    soc_pct: 15.0,
                    voltage: None,
                    current_a: None,
                    charging: ChargeState::Discharging,
                    source: None,
                },
                1_000,
            )
            .unwrap();
        let engine = ReflexEngine::new(standard_safing_rules(&SafingOptions::default()));
        let fired = engine.tick(&world, 2_000).unwrap();
        assert!(fired.iter().any(|f| f.rule_id == "safe-power-low"));
        // critical/offline rules must NOT fire at merely-low charge
        assert!(!fired.iter().any(|f| f.rule_id == "safe-power-critical-escalate"));
    }
}

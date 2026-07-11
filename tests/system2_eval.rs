//! Phase 18 closing eval — dual-system perception-action.
//!
//! Pins the three roadmap criteria:
//! 1. **System 1 reflex latency budget met offline** — pure-computation rule
//!    evaluation over a realistic rule/entity load stays far under the reflex
//!    tick, with no LLM or network anywhere in the path.
//! 2. **System 2 invoked only on novelty** — end-to-end through the real
//!    plumbing (reflex `Action::Escalate` → `System2Sink` → wake channel →
//!    `System2Reasoner`): a storm of repeat escalations produces exactly one
//!    LLM invocation per novel situation, within the wake budget.
//! 3. **World-memory queries return temporally-correct device state** — `at()`
//!    answers with the device state that was true at the asked instant, across
//!    state transitions.

use oh_ben_claw::agent::reflex::{
    dispatch, Action, ActionSink, Cmp, Condition, FiredReflex, ReflexEngine, ReflexRule, Snapshot,
};
use oh_ben_claw::agent::system2::{
    GateDecision, NoveltyGate, Reasoner, System2Reasoner, System2Sink,
};
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::movement::MovementCommand;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

// ── 1. System 1 latency budget (offline) ──────────────────────────────────────

fn overheat_rule(i: usize) -> ReflexRule {
    ReflexRule {
        id: format!("overheat-{i}"),
        when: Condition::And {
            all: vec![
                Condition::Sensor {
                    entity: format!("sensor.temp{i}"),
                    op: Cmp::Gt,
                    value: 85.0,
                },
                Condition::State {
                    entity: "power.mode".to_string(),
                    field: Some("mode".to_string()),
                    equals: "normal".to_string(),
                },
            ],
        },
        then: Action::Escalate {
            reason: format!("overheat on sensor.temp{i}"),
        },
        debounce_ms: 0,
        max_rate_hz: None,
    }
}

#[test]
fn eval_system1_reflex_latency_budget_offline() {
    // A realistic load: 64 rules over 64 entities, evaluated 1000 times.
    let rules: Vec<ReflexRule> = (0..64).map(overheat_rule).collect();
    let engine = ReflexEngine::new(rules);

    let mut nums = HashMap::new();
    for i in 0..64 {
        // Half the sensors are hot so half the rules fire — worst-ish case.
        nums.insert(
            format!("sensor.temp{i}"),
            if i % 2 == 0 { 90.0 } else { 40.0 },
        );
    }
    let mut snap = Snapshot::from_nums(nums);
    snap.vals
        .insert("power.mode".to_string(), json!({"mode": "normal"}));

    let start = Instant::now();
    let mut fired_total = 0usize;
    for tick in 0..1_000u64 {
        // Fresh engine state not needed — debounce 0; evaluate is the hot path.
        fired_total += engine.evaluate(&snap, tick * 1_000).len();
    }
    let elapsed = start.elapsed();
    assert!(fired_total > 0, "the load actually fires rules");

    // Budget: 1000 evaluations of 64 rules in well under one reflex tick
    // (1 s default). 100 ms total ⇒ ≤100 µs per evaluation — generous enough
    // to be flake-free in debug builds, orders of magnitude under the tick.
    assert!(
        elapsed.as_millis() < 100,
        "System 1 evaluation blew the offline latency budget: {elapsed:?} for 1000 ticks"
    );
}

// ── 2. System 2 invoked only on novelty (end-to-end plumbing) ─────────────────

struct CountingReasoner {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Reasoner for CountingReasoner {
    async fn reason(&self, objective: &str) -> anyhow::Result<String> {
        assert!(objective.contains("SYSTEM 1 ESCALATION"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("investigated".to_string())
    }
}

struct NullSink;
#[async_trait::async_trait]
impl ActionSink for NullSink {
    async fn gpio_write(&self, _n: &str, _p: i64, _v: i64) -> anyhow::Result<()> {
        Ok(())
    }
    async fn publish(&self, _t: &str, _p: &Value) -> anyhow::Result<()> {
        Ok(())
    }
    async fn escalate(&self, _r: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn move_actuator(&self, _c: &MovementCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn eval_system2_wakes_only_on_novelty() {
    // Real chain: fired escalations → dispatch → System2Sink → channel.
    let (sink, rx) = System2Sink::new(Arc::new(NullSink), 64);

    // A storm: the same node-lost alarm fires 10 ticks in a row (durations
    // vary), then a genuinely different situation appears.
    let mut fired: Vec<FiredReflex> = Vec::new();
    for tick in 0..10 {
        fired.push(FiredReflex {
            rule_id: "mesh-node-lost".to_string(),
            action: Action::Escalate {
                reason: format!("mesh node lost: node-3 offline {}s", 30 * (tick + 1)),
            },
        });
    }
    fired.push(FiredReflex {
        rule_id: "overheat".to_string(),
        action: Action::Escalate {
            reason: "overheat on sensor.temp4 92.1".to_string(),
        },
    });
    dispatch(&fired, &sink).await.unwrap();
    drop(sink); // close the channel so the reasoner loop terminates

    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let counter = Arc::new(CountingReasoner {
        calls: AtomicUsize::new(0),
    });
    let reasoner = System2Reasoner::new(
        NoveltyGate::new(600_000, 6),
        counter.clone() as Arc<dyn Reasoner>,
    )
    .with_world(Arc::clone(&world));

    // Drain the channel through the reasoner (the production `run` loop).
    reasoner.run(rx).await;

    assert_eq!(
        counter.calls.load(Ordering::SeqCst),
        2,
        "11 escalations, 2 novel situations, exactly 2 LLM wakes"
    );
    // The wake trail is in world memory.
    let history = world.history("system2.last_wake").unwrap();
    assert_eq!(history.len(), 2);
    assert!(history[0].value["reason"]
        .as_str()
        .unwrap()
        .contains("node-3"));
    assert!(history[1].value["reason"]
        .as_str()
        .unwrap()
        .contains("overheat"));
}

#[test]
fn eval_system2_budget_caps_wakes_even_for_novel_situations() {
    let gate = NoveltyGate::new(0, 3);
    let mut wakes = 0;
    for i in 0..10 {
        if gate.admit(&format!("distinct situation {i} kind-{i}"), i as u64 + 1)
            == GateDecision::Wake
        {
            wakes += 1;
        }
    }
    assert_eq!(wakes, 3, "hourly budget bounds LLM spend");
}

// ── 3. Temporally-correct device state ────────────────────────────────────────

#[test]
fn eval_world_memory_returns_temporally_correct_device_state() {
    let w = WorldMemory::open_in_memory().unwrap();

    // A door lock and a pump change state over the day.
    w.observe("front_door.lock", json!("locked"), 8_000, 8_000, "node-1")
        .unwrap();
    w.observe(
        "front_door.lock",
        json!("unlocked"),
        12_000,
        12_000,
        "node-1",
    )
    .unwrap();
    w.observe("front_door.lock", json!("locked"), 20_000, 20_000, "node-1")
        .unwrap();
    w.observe(
        "pump.state",
        json!({"running": true, "rpm": 1200}),
        9_000,
        9_000,
        "node-2",
    )
    .unwrap();
    w.observe(
        "pump.state",
        json!({"running": false, "rpm": 0}),
        15_000,
        15_000,
        "node-2",
    )
    .unwrap();

    // "What was true at time T?" — across every transition.
    assert_eq!(
        w.at("front_door.lock", 9_500).unwrap().unwrap().value,
        json!("locked")
    );
    assert_eq!(
        w.at("front_door.lock", 12_000).unwrap().unwrap().value,
        json!("unlocked"),
        "boundary instant belongs to the new state"
    );
    assert_eq!(
        w.at("front_door.lock", 19_999).unwrap().unwrap().value,
        json!("unlocked")
    );
    assert_eq!(
        w.at("front_door.lock", 25_000).unwrap().unwrap().value,
        json!("locked")
    );
    assert!(
        w.at("front_door.lock", 7_999).unwrap().is_none(),
        "before first observation nothing is believed"
    );

    assert_eq!(
        w.at("pump.state", 14_000).unwrap().unwrap().value["running"],
        json!(true)
    );
    assert_eq!(
        w.current("pump.state").unwrap().unwrap().value["running"],
        json!(false)
    );

    // The full trail is retained (non-destructive).
    assert_eq!(w.history("front_door.lock").unwrap().len(), 3);
}

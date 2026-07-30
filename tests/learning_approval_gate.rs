//! The self-authoring loop, end to end, through its approval gate.
//!
//! `src/learning` is the only pipeline in this tree that ends by **activating a
//! rule which drives behaviour**, and it had four unit tests and no integration
//! coverage — the thinnest coverage of any module, on the one path where a wrong
//! correlation could start acting on its own. The module docs make the safety
//! claim plainly: *"Proposals are never activated automatically"* and *"gated by
//! approval so a wrong correlation can never silently start driving behavior."*
//!
//! The unit tests check that `approve()` pushes a rule into a `Vec`. That is not
//! the claim. The claim is about the **live engine**: that a mined rule cannot
//! influence what the system does until a human says so. So this suite wires the
//! real chain — `WorldMemory` → `RuleMiner` → `ProposalStore` → the shared buffer
//! → a live `ForesightEngine` — and asserts on what the engine fires, which is
//! the thing that actually reaches the rest of the system.
//!
//! Four properties, in the order they matter:
//!
//!   1. Mining alone changes nothing the engine can see.
//!   2. Rejection changes nothing the engine can see.
//!   3. Approval, and only approval, makes the rule live.
//!   4. What goes live can only escalate — never actuate.
//!
//! (1) and (2) assert that the engine fires *nothing*, and an empty result is the
//! easiest thing in the world to get for the wrong reason. The first draft of this
//! file got exactly that: the fixture ended with pressure flat and low, no forecast
//! ever crossed the threshold, and three of these four tests passed on `[] == []`
//! — a gate narrower than its claim, in a file written to complain about gates
//! narrower than their claims. So every negative assertion here is paired with
//! [`proof_the_fixture_is_hot`], which injects the same rule directly into a
//! throwaway buffer and requires it to fire. Emptiness only counts as evidence
//! once the world is known to be capable of setting the rule off.
//!
//! (4) is the invariant that makes the rest safe to get wrong. `to_foresight_rule`
//! hardcodes `Action::Escalate`, so a self-authored rule wakes the reasoner and
//! cannot drive a pin or a motor. Nothing enforced that but the constructor, and
//! a future edit widening it would be a silent, large increase in blast radius.

use std::sync::{Arc, Mutex};

use oh_ben_claw::agent::reflex::{Action, Cmp};
use oh_ben_claw::foresight::{ForesightEngine, ForesightRule};
use oh_ben_claw::learning::{OutcomeSpec, ProposalStatus, ProposalStore, RuleMiner};
use oh_ben_claw::memory::world::WorldMemory;
use serde_json::json;

/// A history where `pressure` climbs hard just before every `fault` event, and
/// `humidity` drifts unrelated. Mining should propose the first and not the
/// second; the engine should see neither until someone approves.
fn world_with_a_real_antecedent() -> Arc<WorldMemory> {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let obs = |entity: &str, t: u64, v: f64| {
        world
            .observe(entity, json!({ "value": v }), t, t, "test-suite")
            .unwrap();
    };

    // Three fault episodes. Each one is preceded within the lookback by a
    // pressure spike; humidity wanders on its own schedule.
    let mut t = 1_000u64;
    for episode in 0..3 {
        obs("fault", t, 0.0);
        obs("pressure", t + 500, 10.0 + episode as f64);
        obs("humidity", t + 600, 40.0 + (episode as f64 * 7.0) % 13.0);

        obs("pressure", t + 1_000, 95.0 + episode as f64); // the antecedent
        obs("fault", t + 1_200, 1.0); // the outcome
        obs("fault", t + 3_000, 0.0); // recovery, so the next edge counts

        t += 20_000;
    }
    // Quiet background: pressure low, humidity in the same range it takes near
    // the events, so humidity looks unspecific and pressure looks specific.
    for i in 0..8 {
        let bt = t + i * 1_000;
        obs("pressure", bt, 12.0);
        obs("humidity", bt, 44.0);
    }

    // Then pressure climbs back into the antecedent band and stays there. This
    // tail is what makes the suite mean anything: with it, an approved rule has
    // something to fire on, so "the engine fired nothing" before approval is a
    // statement about the gate rather than about a quiet world. It costs one
    // background positive — mined confidence is 0.75 rather than 1.0, still well
    // over the miner's 0.6 floor.
    obs("pressure", t + 9_000, 60.0);
    obs("humidity", t + 9_000, 44.0);
    obs("pressure", t + 10_000, 96.0);
    obs("humidity", t + 10_000, 44.0);
    world
}

/// Prove the fixture can set off the rule at all, independent of the gate.
///
/// Injects the rule the miner produces straight into a private buffer — bypassing
/// `ProposalStore` entirely — and requires the engine to fire it. If this fails,
/// every "the engine fired nothing" assertion in this file is meaningless and the
/// fixture needs fixing, not the gate.
fn proof_the_fixture_is_hot(world: &WorldMemory) {
    let direct: Arc<Mutex<Vec<ForesightRule>>> = Arc::new(Mutex::new(vec![ForesightRule {
        id: "control".to_string(),
        entity: "pressure".to_string(),
        op: Cmp::Ge,
        threshold: 95.0,
        horizon_ms: 60_000,
        then: Action::Escalate {
            reason: "control".to_string(),
        },
        debounce_ms: 0,
    }]));
    let fired = live_engine(direct).evaluate(world, NOW_MS);
    assert_eq!(
        fired.len(),
        1,
        "the fixture cannot fire this rule even when it is live, so every negative \
         assertion in this file is vacuous"
    );
}

fn miner() -> RuleMiner {
    RuleMiner {
        lookback_ms: 5_000,
        min_support: 2,
        min_confidence: 0.6,
        candidates: vec!["pressure".to_string(), "humidity".to_string()],
    }
}

fn fault_outcome() -> OutcomeSpec {
    OutcomeSpec {
        entity: "fault".to_string(),
        op: Cmp::Ge,
        threshold: 1.0,
    }
}

/// Build the live half: a foresight engine with **no** static rules, reading the
/// same buffer the approval gate writes into. Anything it fires therefore came
/// through the gate, which is what makes these assertions mean something.
fn live_engine(buffer: Arc<Mutex<Vec<ForesightRule>>>) -> ForesightEngine {
    ForesightEngine::new(vec![]).with_learned_rules(buffer)
}

/// Latest timestamp in the fixture, plus a margin — evaluate "now" so the
/// forecaster has the recent history in view.
const NOW_MS: u64 = 70_000;

#[test]
fn mining_alone_activates_nothing() {
    let world = world_with_a_real_antecedent();
    let buffer: Arc<Mutex<Vec<ForesightRule>>> = Arc::new(Mutex::new(Vec::new()));
    let store = ProposalStore::new(Arc::clone(&buffer));

    let mined = miner().mine(&world, &fault_outcome()).unwrap();
    assert!(
        !mined.is_empty(),
        "fixture is not exercising the miner — nothing was proposed, so the rest \
         of this suite would pass vacuously"
    );
    assert!(
        mined.iter().any(|p| p.entity == "pressure"),
        "expected the real antecedent to be mined, got: {:?}",
        mined.iter().map(|p| &p.entity).collect::<Vec<_>>()
    );

    let added = store.ingest(mined);
    assert!(added >= 1);

    // The gate: proposals exist, and the live engine cannot see any of them.
    assert!(store
        .list()
        .iter()
        .all(|p| p.status == ProposalStatus::Pending));
    assert_eq!(store.active_count(), 0, "mining must not activate");

    let fired = live_engine(Arc::clone(&buffer)).evaluate(&world, NOW_MS);
    assert!(
        fired.is_empty(),
        "a mined-but-unapproved rule reached the live engine: {:?}",
        fired.iter().map(|f| &f.rule_id).collect::<Vec<_>>()
    );
    proof_the_fixture_is_hot(&world);
}

#[test]
fn rejection_never_reaches_the_engine() {
    let world = world_with_a_real_antecedent();
    let buffer: Arc<Mutex<Vec<ForesightRule>>> = Arc::new(Mutex::new(Vec::new()));
    let store = ProposalStore::new(Arc::clone(&buffer));
    store.ingest(miner().mine(&world, &fault_outcome()).unwrap());

    let id = store.list()[0].rule.id.clone();
    assert!(store.reject(&id));

    assert_eq!(
        store
            .list()
            .iter()
            .find(|p| p.rule.id == id)
            .map(|p| p.status),
        Some(ProposalStatus::Rejected)
    );
    assert_eq!(store.active_count(), 0);
    assert!(
        live_engine(Arc::clone(&buffer))
            .evaluate(&world, NOW_MS)
            .is_empty(),
        "a rejected rule reached the live engine"
    );
    proof_the_fixture_is_hot(&world);

    // A rejected proposal must not be approvable afterwards — otherwise the gate
    // is a suggestion. `approve` returns false for anything not Pending.
    assert!(
        !store.approve(&id),
        "a rejected proposal was approved on a second pass"
    );
    assert_eq!(store.active_count(), 0);
}

#[test]
fn approval_is_what_makes_a_learned_rule_live() {
    let world = world_with_a_real_antecedent();
    let buffer: Arc<Mutex<Vec<ForesightRule>>> = Arc::new(Mutex::new(Vec::new()));
    let store = ProposalStore::new(Arc::clone(&buffer));
    store.ingest(miner().mine(&world, &fault_outcome()).unwrap());

    let id = store
        .list()
        .iter()
        .find(|p| p.rule.entity == "pressure")
        .map(|p| p.rule.id.clone())
        .expect("the pressure antecedent should have been proposed");

    // Before.
    assert_eq!(store.active_count(), 0);
    let before = live_engine(Arc::clone(&buffer)).evaluate(&world, NOW_MS);

    assert!(store.approve(&id));

    // After: the engine is now reading a rule it could not see a moment ago.
    assert_eq!(store.active_count(), 1);
    assert_eq!(
        store
            .list()
            .iter()
            .find(|p| p.rule.id == id)
            .map(|p| p.status),
        Some(ProposalStatus::Approved)
    );
    let after: Vec<String> = buffer
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(after, vec![id.clone()], "exactly the approved rule is live");
    assert!(
        before.is_empty(),
        "the engine fired before approval, so this test proves nothing about the gate"
    );

    // The half that makes the other half mean something: the engine now fires the
    // rule it could not see before. Same world, same engine construction, same
    // instant — the only thing that changed is that somebody approved it.
    let fired = live_engine(Arc::clone(&buffer)).evaluate(&world, NOW_MS);
    assert_eq!(
        fired.iter().map(|f| f.rule_id.clone()).collect::<Vec<_>>(),
        vec![id.clone()],
        "approval did not make the rule live; nothing distinguishes this state \
         from the unapproved one"
    );

    // Approving twice must not double-register: the engine would then fire the
    // same learned rule twice per tick.
    assert!(!store.approve(&id), "a second approve should be a no-op");
    assert_eq!(store.active_count(), 1);
}

#[test]
fn an_approved_rule_can_only_escalate() {
    let world = world_with_a_real_antecedent();
    let buffer: Arc<Mutex<Vec<ForesightRule>>> = Arc::new(Mutex::new(Vec::new()));
    let store = ProposalStore::new(Arc::clone(&buffer));
    store.ingest(miner().mine(&world, &fault_outcome()).unwrap());

    for p in store.list() {
        store.approve(&p.rule.id);
    }
    assert!(store.active_count() >= 1);

    // Every rule the gate admits, whatever the miner proposed, wakes the reasoner
    // and nothing else. A self-authored rule must never be able to drive a pin, a
    // motor, or a spine topic.
    for rule in buffer.lock().unwrap().iter() {
        match &rule.then {
            Action::Escalate { reason } => {
                assert!(
                    reason.starts_with("learned:"),
                    "an escalation from the learning layer should say so: {reason}"
                );
            }
            other => panic!(
                "the approval gate admitted a rule that is not escalate-only: {other:?}. \
                 Self-authored rules must not reach an actuator; if this is intended, \
                 it is a Track 0 decision and not a refactor."
            ),
        }
    }

    // And whatever the live engine fires from those rules carries the same
    // restriction, checked at the point the action leaves the engine.
    let fired_now = live_engine(Arc::clone(&buffer)).evaluate(&world, NOW_MS);
    assert!(
        !fired_now.is_empty(),
        "nothing fired, so the loop below checks nothing"
    );
    for fired in fired_now {
        assert!(
            matches!(fired.action, Action::Escalate { .. }),
            "learned rule {} fired a non-escalate action: {:?}",
            fired.rule_id,
            fired.action
        );
    }
}

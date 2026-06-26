//! Behavior trees for missions — full BT grammar (BehaviorTree.CPP-style).
//!
//! The linear [`MissionRunner`](super::MissionRunner) executes a flat step list.
//! This module upgrades that to a real **behavior tree**: composites
//! (`sequence`, `reactive_sequence`, `fallback`, `parallel`), decorators
//! (`invert`, `retry`, `repeat`, `force_success`), and leaves that reuse the
//! existing pieces — `condition` ticks a reflex [`Condition`] against world
//! memory, and `action` runs a [`MissionStep`] over the navigation/audio suites.
//! Each tick returns `Success` / `Failure` / `Running`, exactly the BT contract.
//!
//! Reactivity is explicit: `sequence`/`fallback` are **memory** nodes (they don't
//! re-tick completed children — safe for one-shot actions), while
//! `reactive_sequence`/`reactive_fallback` re-tick from the start each cycle (so a
//! guard condition that turns false aborts the branch). The tree is declarative
//! ([`BtSpec`], serde) and compiled to a stateful [`Bt`] for execution.

use super::MissionStep;
use crate::agent::reflex::{Condition, Snapshot};
use crate::audio::suite::AudioController;
use crate::memory::world::WorldMemory;
use crate::navigation::{NavController, NavGoal};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

/// The result of ticking a behavior-tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Failure,
    Running,
}

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse().ok(),
        Value::Object(o) => o.get("value").and_then(value_to_f64),
        _ => None,
    }
}

/// Declarative behavior tree (serde — author in config/JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BtSpec {
    /// Run children in order; fail on the first failure, succeed when all do.
    /// Remembers progress (one-shot safe).
    Sequence { children: Vec<BtSpec> },
    /// Like `sequence` but re-ticks from the start each cycle (reactive).
    ReactiveSequence { children: Vec<BtSpec> },
    /// Try children in order; succeed on the first success, fail when all fail.
    Fallback { children: Vec<BtSpec> },
    /// Reactive fallback (re-ticks from the start each cycle).
    ReactiveFallback { children: Vec<BtSpec> },
    /// Tick all children; succeed when `success_threshold` succeed (0 ⇒ all).
    Parallel {
        children: Vec<BtSpec>,
        #[serde(default)]
        success_threshold: usize,
    },
    /// Invert the child's Success/Failure.
    Invert { child: Box<BtSpec> },
    /// Retry the child up to `n` times on failure.
    Retry { n: u32, child: Box<BtSpec> },
    /// Repeat the child up to `n` times on success.
    Repeat { n: u32, child: Box<BtSpec> },
    /// Always succeed (mask a child failure).
    ForceSuccess { child: Box<BtSpec> },
    /// Leaf: succeed iff a world-memory condition holds (never Running).
    Condition { check: Condition },
    /// Leaf: run a mission step (Running until complete, then Success).
    Action { step: MissionStep },
}

impl BtSpec {
    /// Compile to an executable, stateful tree.
    pub fn compile(self) -> Bt {
        match self {
            BtSpec::Sequence { children } => Bt::Sequence {
                children: children.into_iter().map(BtSpec::compile).collect(),
                cursor: 0,
                reactive: false,
            },
            BtSpec::ReactiveSequence { children } => Bt::Sequence {
                children: children.into_iter().map(BtSpec::compile).collect(),
                cursor: 0,
                reactive: true,
            },
            BtSpec::Fallback { children } => Bt::Fallback {
                children: children.into_iter().map(BtSpec::compile).collect(),
                cursor: 0,
                reactive: false,
            },
            BtSpec::ReactiveFallback { children } => Bt::Fallback {
                children: children.into_iter().map(BtSpec::compile).collect(),
                cursor: 0,
                reactive: true,
            },
            BtSpec::Parallel { children, success_threshold } => Bt::Parallel {
                children: children.into_iter().map(BtSpec::compile).collect(),
                threshold: success_threshold,
            },
            BtSpec::Invert { child } => Bt::Invert(Box::new(child.compile())),
            BtSpec::Retry { n, child } => Bt::Retry { n, count: 0, child: Box::new(child.compile()) },
            BtSpec::Repeat { n, child } => Bt::Repeat { n, count: 0, child: Box::new(child.compile()) },
            BtSpec::ForceSuccess { child } => Bt::ForceSuccess(Box::new(child.compile())),
            BtSpec::Condition { check } => Bt::Condition(check),
            BtSpec::Action { step } => Bt::Action { step, entered: false, start_ms: 0, done: false },
        }
    }
}

/// Execution context: the suites and memory a tree acts over.
pub struct BtContext<'a> {
    pub world: &'a WorldMemory,
    pub nav: Option<&'a Arc<NavController>>,
    pub audio: Option<&'a Arc<AudioController>>,
    pub default_voice: String,
    pub source: String,
}

/// A compiled, stateful behavior tree.
pub enum Bt {
    Sequence { children: Vec<Bt>, cursor: usize, reactive: bool },
    Fallback { children: Vec<Bt>, cursor: usize, reactive: bool },
    Parallel { children: Vec<Bt>, threshold: usize },
    Invert(Box<Bt>),
    Retry { n: u32, count: u32, child: Box<Bt> },
    Repeat { n: u32, count: u32, child: Box<Bt> },
    ForceSuccess(Box<Bt>),
    Condition(Condition),
    Action { step: MissionStep, entered: bool, start_ms: u64, done: bool },
}

impl Bt {
    /// Reset all runtime state to initial (for re-runs / retries).
    pub fn reset(&mut self) {
        match self {
            Bt::Sequence { children, cursor, .. } | Bt::Fallback { children, cursor, .. } => {
                *cursor = 0;
                children.iter_mut().for_each(Bt::reset);
            }
            Bt::Parallel { children, .. } => children.iter_mut().for_each(Bt::reset),
            Bt::Invert(c) | Bt::ForceSuccess(c) => c.reset(),
            Bt::Retry { count, child, .. } | Bt::Repeat { count, child, .. } => {
                *count = 0;
                child.reset();
            }
            Bt::Condition(_) => {}
            Bt::Action { entered, done, .. } => {
                *entered = false;
                *done = false;
            }
        }
    }

    /// Tick the node once, returning its status.
    pub fn tick(&mut self, ctx: &BtContext, now: u64) -> Status {
        match self {
            Bt::Sequence { children, cursor, reactive } => {
                if *reactive {
                    *cursor = 0;
                }
                while *cursor < children.len() {
                    match children[*cursor].tick(ctx, now) {
                        Status::Success => *cursor += 1,
                        Status::Failure => {
                            *cursor = 0;
                            return Status::Failure;
                        }
                        Status::Running => return Status::Running,
                    }
                }
                *cursor = 0;
                Status::Success
            }
            Bt::Fallback { children, cursor, reactive } => {
                if *reactive {
                    *cursor = 0;
                }
                while *cursor < children.len() {
                    match children[*cursor].tick(ctx, now) {
                        Status::Failure => *cursor += 1,
                        Status::Success => {
                            *cursor = 0;
                            return Status::Success;
                        }
                        Status::Running => return Status::Running,
                    }
                }
                *cursor = 0;
                Status::Failure
            }
            Bt::Parallel { children, threshold } => {
                let need = if *threshold == 0 { children.len() } else { *threshold };
                let mut succ = 0;
                let mut fail = 0;
                for c in children.iter_mut() {
                    match c.tick(ctx, now) {
                        Status::Success => succ += 1,
                        Status::Failure => fail += 1,
                        Status::Running => {}
                    }
                }
                if succ >= need {
                    Status::Success
                } else if fail > children.len().saturating_sub(need) {
                    Status::Failure
                } else {
                    Status::Running
                }
            }
            Bt::Invert(child) => match child.tick(ctx, now) {
                Status::Success => Status::Failure,
                Status::Failure => Status::Success,
                Status::Running => Status::Running,
            },
            Bt::Retry { n, count, child } => match child.tick(ctx, now) {
                Status::Failure => {
                    if *count < *n {
                        *count += 1;
                        child.reset();
                        Status::Running
                    } else {
                        Status::Failure
                    }
                }
                Status::Success => {
                    *count = 0;
                    Status::Success
                }
                Status::Running => Status::Running,
            },
            Bt::Repeat { n, count, child } => match child.tick(ctx, now) {
                Status::Success => {
                    if *count + 1 < *n {
                        *count += 1;
                        child.reset();
                        Status::Running
                    } else {
                        *count = 0;
                        Status::Success
                    }
                }
                other => other,
            },
            Bt::ForceSuccess(child) => match child.tick(ctx, now) {
                Status::Failure => Status::Success,
                other => other,
            },
            Bt::Condition(cond) => {
                if eval_condition(cond, ctx.world) {
                    Status::Success
                } else {
                    Status::Failure
                }
            }
            Bt::Action { step, entered, start_ms, done } => {
                if *done {
                    return Status::Success;
                }
                if !*entered {
                    enter_action(step, ctx, now);
                    *entered = true;
                    *start_ms = now;
                }
                if action_complete(step, *start_ms, now, ctx) {
                    *done = true;
                    Status::Success
                } else {
                    Status::Running
                }
            }
        }
    }
}

/// Evaluate a reflex condition against a world-memory snapshot.
fn eval_condition(cond: &Condition, world: &WorldMemory) -> bool {
    let mut set = HashSet::new();
    cond.collect_entities(&mut set);
    let mut snap = Snapshot::new();
    for e in set {
        if let Ok(Some(fact)) = world.current(&e) {
            if let Some(n) = value_to_f64(&fact.value) {
                snap.nums.insert(e.clone(), n);
            }
            snap.vals.insert(e, fact.value);
        }
    }
    cond.eval(&snap)
}

/// Perform an action's one-shot side effect (sync; `speak` is fire-and-forget).
fn enter_action(step: &MissionStep, ctx: &BtContext, now: u64) {
    match step {
        MissionStep::NavigateTo { x, y, tolerance } => {
            if let Some(nav) = ctx.nav {
                let goal = NavGoal { x: *x, y: *y, tolerance: *tolerance };
                if nav.has_grid() {
                    let _ = nav.plan_to(goal, now);
                } else {
                    nav.set_goal(goal, now);
                }
            }
        }
        MissionStep::Speak { text, voice } => {
            if let Some(audio) = ctx.audio {
                let a = Arc::clone(audio);
                let t = text.clone();
                let v = voice.clone().unwrap_or_else(|| ctx.default_voice.clone());
                // fire-and-forget: emit the utterance without blocking the tick
                tokio::spawn(async move {
                    let _ = a.speak(t, v, now).await;
                });
            }
        }
        MissionStep::Record { entity, value } => {
            let _ = ctx.world.observe(entity, value.clone(), now, now, &ctx.source);
        }
        MissionStep::Wait { .. } | MissionStep::AwaitState { .. } => {}
    }
}

/// Is an action finished?
fn action_complete(step: &MissionStep, start_ms: u64, now: u64, ctx: &BtContext) -> bool {
    match step {
        MissionStep::NavigateTo { .. } => {
            ctx.nav.map(|n| n.current_goal().is_none()).unwrap_or(true)
        }
        MissionStep::Wait { ms } => now.saturating_sub(start_ms) >= *ms,
        MissionStep::Speak { .. } | MissionStep::Record { .. } => true,
        MissionStep::AwaitState { entity, field, equals } => ctx
            .world
            .current(entity)
            .ok()
            .flatten()
            .map(|f| {
                let s = match field {
                    Some(fl) => f.value.get(fl).and_then(|x| x.as_str()),
                    None => f.value.as_str(),
                };
                s == Some(equals.as_str())
            })
            .unwrap_or(false),
    }
}

/// Runs one behavior tree over the suites + world memory. Tick it on a cadence.
pub struct BtRunner {
    bt: std::sync::Mutex<Bt>,
    world: Arc<WorldMemory>,
    nav: Option<Arc<NavController>>,
    audio: Option<Arc<AudioController>>,
    default_voice: String,
    source: String,
}

impl BtRunner {
    pub fn new(spec: BtSpec, world: Arc<WorldMemory>) -> Self {
        Self {
            bt: std::sync::Mutex::new(spec.compile()),
            world,
            nav: None,
            audio: None,
            default_voice: "nova".to_string(),
            source: "mission".to_string(),
        }
    }
    pub fn with_nav(mut self, nav: Arc<NavController>) -> Self {
        self.nav = Some(nav);
        self
    }
    pub fn with_audio(mut self, audio: Arc<AudioController>) -> Self {
        self.audio = Some(audio);
        self
    }

    /// Tick the tree once; returns the root status.
    pub fn tick(&self, now: u64) -> Status {
        let ctx = BtContext {
            world: &self.world,
            nav: self.nav.as_ref(),
            audio: self.audio.as_ref(),
            default_voice: self.default_voice.clone(),
            source: self.source.clone(),
        };
        let mut bt = self.bt.lock().unwrap_or_else(|p| p.into_inner());
        bt.tick(&ctx, now)
    }

    /// Reset the tree to its initial state.
    pub fn reset(&self) {
        self.bt.lock().unwrap_or_else(|p| p.into_inner()).reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::reflex::Cmp;
    use serde_json::json;

    fn ctx_world() -> Arc<WorldMemory> {
        Arc::new(WorldMemory::open_in_memory().unwrap())
    }

    fn runner(spec: BtSpec, world: &Arc<WorldMemory>) -> BtRunner {
        BtRunner::new(spec, Arc::clone(world))
    }

    fn record(entity: &str, v: i64) -> BtSpec {
        BtSpec::Action { step: MissionStep::Record { entity: entity.into(), value: json!(v) } }
    }
    fn cond_ge(entity: &str, value: f64) -> BtSpec {
        BtSpec::Condition { check: Condition::Sensor { entity: entity.into(), op: Cmp::Ge, value } }
    }

    #[test]
    fn sequence_runs_all_then_succeeds() {
        let world = ctx_world();
        let r = runner(
            BtSpec::Sequence { children: vec![record("a", 1), record("b", 2)] },
            &world,
        );
        // record actions complete in one tick each (memory sequence advances)
        assert_eq!(r.tick(0), Status::Success);
        assert_eq!(world.current("a").unwrap().unwrap().value, json!(1));
        assert_eq!(world.current("b").unwrap().unwrap().value, json!(2));
    }

    #[test]
    fn fallback_takes_first_success() {
        let world = ctx_world();
        // first child's condition fails (entity absent), second records
        let r = runner(
            BtSpec::Fallback { children: vec![cond_ge("missing", 1.0), record("done", 1)] },
            &world,
        );
        assert_eq!(r.tick(0), Status::Success);
        assert_eq!(world.current("done").unwrap().unwrap().value, json!(1));
    }

    #[test]
    fn condition_gates_a_sequence() {
        let world = ctx_world();
        let r = runner(
            BtSpec::Sequence { children: vec![cond_ge("flag", 1.0), record("ran", 1)] },
            &world,
        );
        // flag not set → condition Failure → sequence Failure → action not run
        assert_eq!(r.tick(0), Status::Failure);
        assert!(world.current("ran").unwrap().is_none());
        // set the flag → sequence proceeds
        world.observe("flag", json!(1), 0, 0, "t").unwrap();
        assert_eq!(r.tick(1), Status::Success);
        assert_eq!(world.current("ran").unwrap().unwrap().value, json!(1));
    }

    #[test]
    fn invert_flips_result() {
        let world = ctx_world();
        let r = runner(BtSpec::Invert { child: Box::new(cond_ge("x", 1.0)) }, &world);
        assert_eq!(r.tick(0), Status::Success); // condition fails → inverted to success
    }

    #[test]
    fn wait_runs_until_elapsed() {
        let world = ctx_world();
        let r = runner(
            BtSpec::Action { step: MissionStep::Wait { ms: 1_000 } },
            &world,
        );
        assert_eq!(r.tick(0), Status::Running);
        assert_eq!(r.tick(500), Status::Running);
        assert_eq!(r.tick(1_000), Status::Success);
    }

    #[test]
    fn reactive_sequence_aborts_when_guard_turns_false() {
        let world = ctx_world();
        world.observe("ok", json!(1), 0, 0, "t").unwrap();
        let r = runner(
            BtSpec::ReactiveSequence {
                children: vec![cond_ge("ok", 1.0), BtSpec::Action { step: MissionStep::Wait { ms: 10_000 } }],
            },
            &world,
        );
        assert_eq!(r.tick(0), Status::Running); // guard ok, wait running
        // guard turns false → reactive sequence re-checks and fails
        world.observe("ok", json!(0), 1, 1, "t").unwrap();
        assert_eq!(r.tick(1), Status::Failure);
    }

    #[test]
    fn retry_then_succeeds_via_changing_world() {
        let world = ctx_world();
        // a condition that only becomes true after the flag is set; retry keeps it alive
        let r = runner(
            BtSpec::Retry { n: 5, child: Box::new(cond_ge("go", 1.0)) },
            &world,
        );
        assert_eq!(r.tick(0), Status::Running); // failed once, retrying
        world.observe("go", json!(1), 1, 1, "t").unwrap();
        assert_eq!(r.tick(1), Status::Success);
    }

    #[test]
    fn parallel_threshold() {
        let world = ctx_world();
        world.observe("a", json!(1), 0, 0, "t").unwrap();
        // 2 conditions, threshold 1: one passes → success
        let r = runner(
            BtSpec::Parallel {
                children: vec![cond_ge("a", 1.0), cond_ge("b", 1.0)],
                success_threshold: 1,
            },
            &world,
        );
        assert_eq!(r.tick(0), Status::Success);
    }

    #[test]
    fn spec_serde_roundtrips() {
        let spec = BtSpec::Sequence {
            children: vec![
                cond_ge("power.mode_ok", 1.0),
                BtSpec::Fallback { children: vec![record("x", 1)] },
            ],
        };
        let js = serde_json::to_string(&spec).unwrap();
        assert!(js.contains("\"type\":\"sequence\""));
        assert_eq!(serde_json::from_str::<BtSpec>(&js).unwrap(), spec);
    }
}

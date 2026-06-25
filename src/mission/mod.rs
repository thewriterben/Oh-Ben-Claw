//! Mission sequencer — the deliberative layer above the reflexes.
//!
//! Reflexes (System 1) react instant-by-instant; the agent (System 2) reasons
//! slowly. Between them sits *deliberative sequencing*: a **mission** is an
//! ordered list of steps — navigate somewhere, say something, wait, record a
//! fact, await a world-state condition — executed in turn, and continuously
//! **guarded** so a bad condition (battery critical, link offline) **preempts**
//! the whole thing and safes the platform.
//!
//! A mission composes the suites without new machinery: `navigate_to` drives the
//! navigation suite (which itself plans around obstacles and is Track 0–bounded),
//! `speak` drives the audio suite, `await_state` blocks on world memory the other
//! suites write, and guards reuse the reflex [`Condition`] grammar. The runner is
//! reactive — call [`MissionRunner::tick`] on a cadence; it advances at most one
//! step per tick and aborts the moment a guard trips.

use crate::agent::reflex::{Condition, Snapshot};
use crate::audio::suite::AudioController;
use crate::memory::world::WorldMemory;
use crate::navigation::{NavController, NavGoal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

fn default_tolerance() -> f64 {
    0.5
}

/// Extract a scalar from a world-memory fact value (number, bool, numeric string,
/// or `{value}` object) — mirrors the reflex engine's extraction.
fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse().ok(),
        Value::Object(o) => o.get("value").and_then(value_to_f64),
        _ => None,
    }
}

/// One step of a mission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MissionStep {
    /// Drive to a world position (via the navigation suite; obstacle-aware when a
    /// grid is configured). Completes when the goal is reached.
    NavigateTo {
        x: f64,
        y: f64,
        #[serde(default = "default_tolerance")]
        tolerance: f64,
    },
    /// Wait a fixed duration (ms).
    Wait { ms: u64 },
    /// Say something (via the audio suite).
    Speak {
        text: String,
        #[serde(default)]
        voice: Option<String>,
    },
    /// Record a fact into world memory (annotate progress / set a flag).
    Record { entity: String, value: Value },
    /// Block until a world-memory fact's string value (or nested `field`) equals
    /// `equals` — e.g. wait for a sensor mode or another suite's signal.
    AwaitState {
        entity: String,
        #[serde(default)]
        field: Option<String>,
        equals: String,
    },
}

impl MissionStep {
    fn label(&self) -> &'static str {
        match self {
            MissionStep::NavigateTo { .. } => "navigate_to",
            MissionStep::Wait { .. } => "wait",
            MissionStep::Speak { .. } => "speak",
            MissionStep::Record { .. } => "record",
            MissionStep::AwaitState { .. } => "await_state",
        }
    }
}

/// A preemption guard: when `abort_when` holds, the mission aborts with `reason`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guard {
    pub abort_when: Condition,
    #[serde(default)]
    pub reason: String,
}

/// An ordered mission with preemption guards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub steps: Vec<MissionStep>,
    #[serde(default)]
    pub guards: Vec<Guard>,
}

/// The status of the active mission.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MissionStatus {
    /// No mission loaded.
    Idle,
    /// Executing step `step` of `total` (`label` names the step kind).
    Running { step: usize, total: usize, label: String },
    /// All steps completed.
    Completed { id: String },
    /// A guard tripped (or an explicit abort).
    Aborted { id: String, reason: String },
}

struct RunnerState {
    mission: Option<Arc<Mission>>,
    step: usize,
    entered: bool,
    step_start: u64,
    status: MissionStatus,
}

/// Runs one mission at a time over the navigation + audio suites and world memory.
/// Tick it on a cadence; it is reactive and guard-preemptible.
pub struct MissionRunner {
    world: Arc<WorldMemory>,
    nav: Option<Arc<NavController>>,
    audio: Option<Arc<AudioController>>,
    default_voice: String,
    source: String,
    inner: Mutex<RunnerState>,
}

impl MissionRunner {
    /// A runner with no controllers (sequencing only). Attach suites with
    /// [`with_nav`](Self::with_nav) / [`with_audio`](Self::with_audio).
    pub fn new(world: Arc<WorldMemory>) -> Self {
        Self {
            world,
            nav: None,
            audio: None,
            default_voice: "nova".to_string(),
            source: "mission".to_string(),
            inner: Mutex::new(RunnerState {
                mission: None,
                step: 0,
                entered: false,
                step_start: 0,
                status: MissionStatus::Idle,
            }),
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
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunnerState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Load a mission and start it from the first step.
    pub fn start(&self, mission: Mission) {
        let total = mission.steps.len();
        let id = mission.id.clone();
        let mut st = self.lock();
        st.mission = Some(Arc::new(mission));
        st.step = 0;
        st.entered = false;
        st.step_start = 0;
        st.status = if total == 0 {
            MissionStatus::Completed { id }
        } else {
            MissionStatus::Running { step: 0, total, label: "pending".to_string() }
        };
    }

    /// The current status.
    pub fn status(&self) -> MissionStatus {
        self.lock().status.clone()
    }

    /// Whether a mission is active (running).
    pub fn is_running(&self) -> bool {
        matches!(self.lock().status, MissionStatus::Running { .. })
    }

    fn is_terminal(status: &MissionStatus) -> bool {
        matches!(status, MissionStatus::Completed { .. } | MissionStatus::Aborted { .. } | MissionStatus::Idle)
    }

    /// Abort the active mission with a reason (halts navigation if present).
    pub async fn abort(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let id = {
            let mut st = self.lock();
            let id = st.mission.as_ref().map(|m| m.id.clone()).unwrap_or_default();
            st.status = MissionStatus::Aborted { id: id.clone(), reason: reason.clone() };
            id
        };
        if let Some(nav) = &self.nav {
            let _ = nav.halt(now_or_zero()).await;
        }
        self.record_status();
        let _ = id;
    }

    /// Build a world snapshot covering every entity the guards reference.
    fn guard_snapshot(&self, mission: &Mission) -> Snapshot {
        let mut set = HashSet::new();
        for g in &mission.guards {
            g.abort_when.collect_entities(&mut set);
        }
        let mut snap = Snapshot::new();
        for e in set {
            if let Ok(Some(fact)) = self.world.current(&e) {
                if let Some(n) = value_to_f64(&fact.value) {
                    snap.nums.insert(e.clone(), n);
                }
                snap.vals.insert(e, fact.value);
            }
        }
        snap
    }

    /// The first guard that trips, if any.
    fn tripped_guard(&self, mission: &Mission) -> Option<String> {
        if mission.guards.is_empty() {
            return None;
        }
        let snap = self.guard_snapshot(mission);
        for g in &mission.guards {
            if g.abort_when.eval(&snap) {
                return Some(if g.reason.is_empty() {
                    "guard tripped".to_string()
                } else {
                    g.reason.clone()
                });
            }
        }
        None
    }

    /// Is the current step finished?
    fn step_complete(&self, step: &MissionStep, step_start: u64, now: u64) -> bool {
        match step {
            MissionStep::NavigateTo { .. } => {
                // Arrived ⇒ the navigation suite clears the goal. No nav ⇒ instant.
                self.nav.as_ref().map(|n| n.current_goal().is_none()).unwrap_or(true)
            }
            MissionStep::Wait { ms } => now.saturating_sub(step_start) >= *ms,
            MissionStep::Speak { .. } | MissionStep::Record { .. } => true, // one-shot
            MissionStep::AwaitState { entity, field, equals } => self
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

    /// Perform a step's one-shot side effect on entry.
    async fn enter_step(&self, step: &MissionStep, now: u64) -> anyhow::Result<()> {
        match step {
            MissionStep::NavigateTo { x, y, tolerance } => {
                if let Some(nav) = &self.nav {
                    let goal = NavGoal { x: *x, y: *y, tolerance: *tolerance };
                    if nav.has_grid() {
                        let _ = nav.plan_to(goal, now)?; // best-effort planned route
                    } else {
                        nav.set_goal(goal, now);
                    }
                }
            }
            MissionStep::Speak { text, voice } => {
                if let Some(audio) = &self.audio {
                    let v = voice.clone().unwrap_or_else(|| self.default_voice.clone());
                    audio.speak(text.clone(), v, now).await?;
                }
            }
            MissionStep::Record { entity, value } => {
                self.world.observe(entity, value.clone(), now, now, &self.source)?;
            }
            MissionStep::Wait { .. } | MissionStep::AwaitState { .. } => {}
        }
        Ok(())
    }

    fn record_status(&self) {
        let status = self.status();
        let body = serde_json::to_value(&status).unwrap_or(Value::Null);
        let now = now_or_zero();
        let _ = self.world.observe("mission.status", body, now, now, &self.source);
    }

    /// Advance the mission by at most one step. Guards are checked first; a
    /// tripped guard aborts (and halts navigation). Returns the new status.
    pub async fn tick(&self, now: u64) -> anyhow::Result<MissionStatus> {
        enum Decision {
            NoOp(MissionStatus),
            Abort(String),
            Enter(MissionStep),
            Advance,
            Complete(String),
        }

        let decision = {
            let mut st = self.lock();
            let Some(mission) = st.mission.clone() else {
                return Ok(MissionStatus::Idle);
            };
            if Self::is_terminal(&st.status) {
                return Ok(st.status.clone());
            }
            if let Some(reason) = self.tripped_guard(&mission) {
                st.status = MissionStatus::Aborted { id: mission.id.clone(), reason: reason.clone() };
                Decision::Abort(reason)
            } else if st.step >= mission.steps.len() {
                st.status = MissionStatus::Completed { id: mission.id.clone() };
                Decision::Complete(mission.id.clone())
            } else {
                let step = mission.steps[st.step].clone();
                if !st.entered {
                    st.entered = true;
                    st.step_start = now;
                    st.status = MissionStatus::Running {
                        step: st.step,
                        total: mission.steps.len(),
                        label: step.label().to_string(),
                    };
                    Decision::Enter(step)
                } else if self.step_complete(&step, st.step_start, now) {
                    st.step += 1;
                    st.entered = false;
                    Decision::Advance
                } else {
                    Decision::NoOp(st.status.clone())
                }
            }
        };

        match decision {
            Decision::NoOp(s) => Ok(s),
            Decision::Abort(reason) => {
                if let Some(nav) = &self.nav {
                    let _ = nav.halt(now).await;
                }
                self.record_status();
                Ok(self.status())
            }
            Decision::Enter(step) => {
                self.enter_step(&step, now).await?;
                self.record_status();
                Ok(self.status())
            }
            Decision::Advance => {
                self.record_status();
                Ok(self.status())
            }
            Decision::Complete(_) => {
                if let Some(nav) = &self.nav {
                    let _ = nav.halt(now).await;
                }
                self.record_status();
                Ok(self.status())
            }
        }
    }
}

fn now_or_zero() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::reflex::Cmp;

    fn runner() -> (MissionRunner, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        (MissionRunner::new(Arc::clone(&world)), world)
    }

    /// Tick repeatedly (advancing time) until terminal or a budget is hit.
    async fn run_to_end(r: &MissionRunner, step_ms: u64, budget: usize) -> MissionStatus {
        let mut t = 0u64;
        for _ in 0..budget {
            let s = r.tick(t).await.unwrap();
            if matches!(s, MissionStatus::Completed { .. } | MissionStatus::Aborted { .. }) {
                return s;
            }
            t += step_ms;
        }
        r.status()
    }

    #[tokio::test]
    async fn sequence_runs_steps_in_order_and_completes() {
        let (r, world) = runner();
        r.start(Mission {
            id: "demo".into(),
            steps: vec![
                MissionStep::Record { entity: "m.a".into(), value: json!(1) },
                MissionStep::Wait { ms: 1_000 },
                MissionStep::Record { entity: "m.b".into(), value: json!(2) },
            ],
            guards: vec![],
        });
        let end = run_to_end(&r, 500, 50).await;
        assert!(matches!(end, MissionStatus::Completed { .. }), "got {end:?}");
        assert_eq!(world.current("m.a").unwrap().unwrap().value, json!(1));
        assert_eq!(world.current("m.b").unwrap().unwrap().value, json!(2));
    }

    #[tokio::test]
    async fn guard_preempts_and_aborts() {
        let (r, world) = runner();
        // power critical should abort a long wait
        world
            .observe("power.mode", json!({"mode": "critical"}), 0, 0, "power")
            .unwrap();
        r.start(Mission {
            id: "patrol".into(),
            steps: vec![MissionStep::Wait { ms: 1_000_000 }],
            guards: vec![Guard {
                abort_when: Condition::State {
                    entity: "power.mode".into(),
                    field: Some("mode".into()),
                    equals: "critical".into(),
                },
                reason: "battery critical — abort".into(),
            }],
        });
        let s = r.tick(0).await.unwrap();
        match s {
            MissionStatus::Aborted { reason, .. } => assert!(reason.contains("battery")),
            other => panic!("expected abort, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn await_state_blocks_until_fact_matches() {
        let (r, world) = runner();
        r.start(Mission {
            id: "wait-go".into(),
            steps: vec![
                MissionStep::AwaitState { entity: "flag".into(), field: None, equals: "go".into() },
                MissionStep::Record { entity: "done".into(), value: json!(true) },
            ],
            guards: vec![],
        });
        // ticks while the flag is unset → stays running, never records `done`
        for t in 0..5 {
            r.tick(t).await.unwrap();
        }
        assert!(r.is_running());
        assert!(world.current("done").unwrap().is_none());
        // set the flag → mission unblocks and completes
        world.observe("flag", json!("go"), 10, 10, "ext").unwrap();
        let end = run_to_end(&r, 1, 20).await;
        assert!(matches!(end, MissionStatus::Completed { .. }), "got {end:?}");
        assert_eq!(world.current("done").unwrap().unwrap().value, json!(true));
    }

    #[tokio::test]
    async fn no_guard_does_not_abort() {
        let (r, _world) = runner();
        r.start(Mission {
            id: "x".into(),
            steps: vec![MissionStep::Wait { ms: 10 }],
            guards: vec![Guard {
                abort_when: Condition::Sensor { entity: "absent".into(), op: Cmp::Gt, value: 1.0 },
                reason: "never".into(),
            }],
        });
        let end = run_to_end(&r, 20, 10).await;
        assert!(matches!(end, MissionStatus::Completed { .. }));
    }

    #[test]
    fn mission_serde_roundtrips() {
        let m = Mission {
            id: "m".into(),
            steps: vec![
                MissionStep::NavigateTo { x: 1.0, y: 2.0, tolerance: 0.5 },
                MissionStep::Speak { text: "hi".into(), voice: None },
            ],
            guards: vec![],
        };
        let js = serde_json::to_string(&m).unwrap();
        assert!(js.contains("\"type\":\"navigate_to\""));
        assert_eq!(serde_json::from_str::<Mission>(&js).unwrap(), m);
    }
}

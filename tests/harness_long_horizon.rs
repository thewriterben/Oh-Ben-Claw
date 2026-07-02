//! Phase 17 long-horizon eval — the roadmap's headline scenario:
//!
//! > "an unattended fleet completes a defined multi-hour routine across an
//! > induced crash/reboot with correct resume and no duplicated physical
//! > actions"
//!
//! Compressed to test time: a 3-objective mission runs through a **fresh
//! harness instance per phase** (each rebuild = a process reboot; the only
//! carried state is the on-disk progress record), with the crash induced
//! mid-objective — after the actuator fired but before the objective was
//! marked done. Evidence (a scripted sensor) must decide the resume, and the
//! actuator must fire **exactly once** across the whole mission.

use async_trait::async_trait;
use oh_ben_claw::agent::Agent;
use oh_ben_claw::config::{AgentConfig, ProviderConfig};
use oh_ben_claw::harness::{
    Harness, HarnessCheck, Objective, ObjectiveStatus, PassOutcome, ProgressStore,
};
use oh_ben_claw::memory::MemoryStore;
use oh_ben_claw::providers::{ChatCompletion, ChatMessage, Provider, ToolCall};
use oh_ben_claw::tools::traits::{Tool, ToolResult};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ── Mocks ─────────────────────────────────────────────────────────────────────

/// A provider whose every turn is: call the tool named in the prompt's
/// objective id (if it maps to one), then answer "done".
struct WorkerProvider;

#[async_trait]
impl Provider for WorkerProvider {
    fn name(&self) -> &str {
        "worker-mock"
    }
    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        _config: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        let last = messages.last().map(|m| m.content.as_str()).unwrap_or("");
        // Second call in a turn (after a tool result) → final answer.
        if last.starts_with("[Tool result") {
            return Ok(ChatCompletion {
                message: "objective complete".to_string(),
                tool_calls: vec![],
                provider: "worker-mock".to_string(),
                model: "mock".to_string(),
            });
        }
        // Worker prompts name their objective; actuate for the door objective.
        let tool_calls = if last.contains("'open-door'") {
            vec![ToolCall {
                id: "call_door".to_string(),
                name: "door_actuator".to_string(),
                args: json!({"action": "open"}).to_string(),
            }]
        } else {
            vec![]
        };
        Ok(ChatCompletion {
            message: if tool_calls.is_empty() {
                "objective complete".to_string()
            } else {
                String::new()
            },
            tool_calls,
            provider: "worker-mock".to_string(),
            model: "mock".to_string(),
        })
    }
}

/// The physical actuator — counts invocations and flips the shared door state.
struct DoorActuator {
    fired: Arc<AtomicUsize>,
    door_open: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Tool for DoorActuator {
    fn name(&self) -> &str {
        "door_actuator"
    }
    fn description(&self) -> &str {
        "opens the door"
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        self.fired.fetch_add(1, Ordering::SeqCst);
        self.door_open.store(true, Ordering::SeqCst);
        Ok(ToolResult::ok("door actuated"))
    }
}

/// The evidence source — reports the shared door state (read-only).
struct DoorSensor {
    door_open: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Tool for DoorSensor {
    fn name(&self) -> &str {
        "door_sensor"
    }
    fn description(&self) -> &str {
        "reads the door state"
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult::ok(if self.door_open.load(Ordering::SeqCst) {
            "door=open"
        } else {
            "door=closed"
        }))
    }
}

// ── Fixture ───────────────────────────────────────────────────────────────────

struct Fixture {
    fired: Arc<AtomicUsize>,
    door_open: Arc<std::sync::atomic::AtomicBool>,
    store_dir: std::path::PathBuf,
    memory: Arc<MemoryStore>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
        memory.create_session_with_id("harness-routine").unwrap();
        Self {
            fired: Arc::new(AtomicUsize::new(0)),
            door_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            store_dir: std::env::temp_dir().join(format!("obc-harness-eval-{tag}-{nanos}")),
            memory,
        }
    }

    /// A fresh harness instance — each call models a process (re)boot; only
    /// the progress record on disk carries over.
    fn boot(&self) -> Harness {
        let agent = Arc::new(Agent::new(
            AgentConfig {
                name: "harness-eval".to_string(),
                system_prompt: "worker".to_string(),
                max_tool_iterations: 4,
            },
            Arc::new(WorkerProvider),
            Arc::clone(&self.memory),
            vec![
                Box::new(DoorActuator {
                    fired: Arc::clone(&self.fired),
                    door_open: Arc::clone(&self.door_open),
                }),
                Box::new(DoorSensor {
                    door_open: Arc::clone(&self.door_open),
                }),
            ],
        ));
        Harness::new(
            ProgressStore::new(&self.store_dir),
            agent,
            ProviderConfig::default(),
            "harness-routine".to_string(),
        )
    }

    fn objectives() -> Vec<Objective> {
        let obj = |id: &str, desc: &str, verify: Vec<HarnessCheck>| Objective {
            id: id.to_string(),
            description: desc.to_string(),
            verify,
            status: ObjectiveStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            note: String::new(),
        };
        vec![
            obj(
                "open-door",
                "open the front door",
                vec![HarnessCheck::ToolContains {
                    tool: "door_sensor".to_string(),
                    args: json!({}),
                    contains: "door=open".to_string(),
                }],
            ),
            obj(
                "log-status",
                "summarize the routine status",
                vec![HarnessCheck::Command {
                    cmd: "exit 0".to_string(),
                    expect_exit: 0,
                }],
            ),
            obj("note-unverified", "write a wrap-up note", vec![]),
        ]
    }
}

// ── The long-horizon eval ─────────────────────────────────────────────────────

#[tokio::test]
async fn long_horizon_routine_survives_crash_with_no_duplicate_actuation() {
    let fx = Fixture::new("main");

    // ── Boot 1: start the routine; the door objective actuates + verifies.
    let harness = fx.boot();
    let mut record = harness.initialize("routine", Fixture::objectives()).unwrap();
    let out = harness.run_once(&mut record).await.unwrap();
    assert_eq!(out, PassOutcome::Advanced("open-door".to_string()));
    assert_eq!(record.objectives[0].status, ObjectiveStatus::Done);
    assert_eq!(fx.fired.load(Ordering::SeqCst), 1, "door actuated once");

    // ── CRASH: induced mid-objective-2 — after the InFlight checkpoint but
    // before completion. Model it exactly as the worker does: flip the
    // record to InFlight and persist, then drop everything in-memory.
    record.objectives[1].status = ObjectiveStatus::InFlight;
    ProgressStore::new(&fx.store_dir).save(&record).unwrap();
    drop(harness);
    drop(record);

    // ── Boot 2 (reboot): initializer must resume from disk, not restart.
    let harness = fx.boot();
    let mut record = harness.initialize("routine", Fixture::objectives()).unwrap();
    assert_eq!(record.run_count, 2, "resume, not a fresh mission");
    assert_eq!(
        record.objectives[0].status,
        ObjectiveStatus::Done,
        "completed physical objective survives the reboot untouched"
    );
    assert_eq!(
        record.objectives[1].status,
        ObjectiveStatus::NeedsVerification,
        "in-flight objective is quarantined for evidence, not re-run"
    );

    // ── Drive to settlement.
    harness
        .run_mission(&mut record, 20, std::time::Duration::from_millis(1))
        .await
        .unwrap();

    assert!(record.settled(), "the routine settles unattended");
    let (done, failed, outstanding) = record.tally();
    assert_eq!((done, failed, outstanding), (3, 0, 0), "{record:?}");
    assert!(
        record.objectives[2].note.contains("UNVERIFIED"),
        "check-less objective is honest about its evidence"
    );

    // THE invariant: across boot, crash, and resume, the physical actuator
    // fired exactly once.
    assert_eq!(
        fx.fired.load(Ordering::SeqCst),
        1,
        "no duplicated physical actions across crash/resume"
    );

    std::fs::remove_dir_all(&fx.store_dir).ok();
}

/// A crash inside a *check-less* objective must fail closed on resume —
/// never a blind re-run that could double-actuate.
#[tokio::test]
async fn checkless_inflight_objective_fails_closed_on_resume() {
    let fx = Fixture::new("failclosed");

    let harness = fx.boot();
    let mut record = harness
        .initialize(
            "routine",
            vec![Objective {
                id: "mystery".to_string(),
                description: "unverifiable side effect".to_string(),
                verify: vec![],
                status: ObjectiveStatus::InFlight, // crashed mid-flight
                attempts: 0,
                max_attempts: 3,
                note: String::new(),
            }],
        )
        .unwrap();
    // initialize() loaded nothing from disk (fresh), so simulate the crash
    // state persisting and reboot once more.
    record.objectives[0].status = ObjectiveStatus::InFlight;
    ProgressStore::new(&fx.store_dir).save(&record).unwrap();
    drop(harness);

    let harness = fx.boot();
    let record = harness.initialize("routine", vec![]).unwrap();
    assert_eq!(record.objectives[0].status, ObjectiveStatus::Failed);
    assert!(record.objectives[0].note.contains("fail closed"));
    assert_eq!(fx.fired.load(Ordering::SeqCst), 0, "nothing actuated");

    std::fs::remove_dir_all(&fx.store_dir).ok();
}

/// Failed verification on resume reopens the objective (attempts counted),
/// and the worker then completes it for real.
#[tokio::test]
async fn failed_resume_verification_reopens_and_completes() {
    let fx = Fixture::new("reopen");

    // Crash state: door objective in-flight, but the door is still closed
    // (the crash happened BEFORE the actuator fired).
    let harness = fx.boot();
    let mut record = harness.initialize("routine", Fixture::objectives()).unwrap();
    record.objectives[0].status = ObjectiveStatus::InFlight;
    ProgressStore::new(&fx.store_dir).save(&record).unwrap();
    drop(harness);

    let harness = fx.boot();
    let mut record = harness.initialize("routine", vec![]).unwrap();
    assert_eq!(record.objectives[0].status, ObjectiveStatus::NeedsVerification);

    // Pass 1: verification says door=closed → reopened, not failed.
    harness.run_once(&mut record).await.unwrap();
    assert_eq!(record.objectives[0].status, ObjectiveStatus::Pending);
    assert_eq!(record.objectives[0].attempts, 1);
    assert_eq!(fx.fired.load(Ordering::SeqCst), 0, "verification never actuates");

    // Pass 2: the worker re-runs the objective for real this time.
    harness.run_once(&mut record).await.unwrap();
    assert_eq!(record.objectives[0].status, ObjectiveStatus::Done);
    assert_eq!(fx.fired.load(Ordering::SeqCst), 1, "actuated exactly once overall");

    std::fs::remove_dir_all(&fx.store_dir).ok();
}

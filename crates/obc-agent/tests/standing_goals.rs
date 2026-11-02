//! Standing goals through the real loop (personal-agents item 4, 2026-11-02).
//!
//! A goal is set from a chat turn with the `goal` tool, and its task is fired
//! by `run_scheduler_loop` -> `scheduled::dispatch` -> `goals::tick`, exactly
//! as `src/main.rs` wires it. Each "not reported", "not closed" or "refused"
//! case is paired with the same fixture doing the opposite, so a test that
//! passes because nothing ran at all fails its pair.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use obc_agent::goals::{progress_entity, GoalReporter, GoalTool, Goals};
use obc_agent::handle::AgentHandle;
use obc_agent::scheduled::{dispatch, Ran};
use obc_agent::{Agent, AgentConfig};
use obc_approval::{
    ApprovalManager, ApprovalPrompter, ApprovalResponse, AutonomyConfig, ForeverGrants,
    PendingApproval,
};
use obc_memory::world::{Closure, WorldMemory};
use obc_memory::MemoryStore;
use obc_providers::{ChatCompletion, ChatMessage, Provider, ProviderConfig, ToolCall};
use obc_safety::risk::{BlastRadius, RiskClass};
use obc_scheduler::{run_scheduler_loop, Goal, GoalStatus, ScheduledTask, Scheduler, TaskKind, Tz};
use obc_tool_api::{Tool, ToolResult};
use serde_json::{json, Value};

/// One model completion: a tool call, or a reply.
#[derive(Clone)]
enum Step {
    Call(&'static str, Value),
    Say(&'static str),
}

/// A model that plays `steps` in order (the last one repeats), counts its
/// calls and keeps every goal-tick message it was sent.
struct Script {
    steps: Mutex<VecDeque<Step>>,
    calls: Arc<Mutex<usize>>,
    ticks_seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Provider for Script {
    fn name(&self) -> &str {
        "script"
    }
    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        c: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        *self.calls.lock().unwrap() += 1;
        if let Some(m) = messages
            .iter()
            .rev()
            .find(|m| m.content.starts_with("[Standing goal]"))
        {
            let mut seen = self.ticks_seen.lock().unwrap();
            if seen.last() != Some(&m.content) {
                seen.push(m.content.clone());
            }
        }
        let step = {
            let mut steps = self.steps.lock().unwrap();
            if steps.len() > 1 {
                steps.pop_front().unwrap()
            } else {
                steps.front().cloned().unwrap_or(Step::Say("NOTE: nothing"))
            }
        };
        let (message, tool_calls) = match step {
            Step::Call(name, args) => (
                String::new(),
                vec![ToolCall {
                    id: format!("c-{name}"),
                    name: name.into(),
                    args: args.to_string(),
                }],
            ),
            Step::Say(text) => (text.to_string(), vec![]),
        };
        Ok(ChatCompletion {
            message,
            tool_calls,
            provider: "script".into(),
            model: c.model.clone(),
            usage: None,
        })
    }
}

/// Where reports went: (owner, text).
#[derive(Default)]
struct Reports(Mutex<Vec<(String, String)>>);

#[async_trait]
impl GoalReporter for Reports {
    async fn report(&self, owner: &str, text: &str) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push((owner.to_string(), text.to_string()));
        Ok(())
    }
}

/// An irreversible software action: gated, so a tick needs a grant or a tap.
struct PostNote(Arc<Mutex<usize>>);

#[async_trait]
impl Tool for PostNote {
    fn name(&self) -> &str {
        "post_note"
    }
    fn description(&self) -> &str {
        "posts a note somewhere it cannot be taken back"
    }
    fn risk_class(&self) -> RiskClass {
        RiskClass {
            reversible: false,
            blast: BlastRadius::None,
            physical: false,
        }
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        *self.0.lock().unwrap() += 1;
        Ok(ToolResult::ok("posted"))
    }
}

#[derive(Default)]
struct Phone(Mutex<Vec<PendingApproval>>);

impl ApprovalPrompter for Phone {
    fn name(&self) -> &str {
        "phone"
    }
    fn prompt(&self, pending: &PendingApproval) {
        self.0.lock().unwrap().push(pending.clone());
    }
}

struct Bench {
    handle: Arc<AgentHandle>,
    goals: Arc<Goals>,
    scheduler: Arc<Scheduler>,
    world: Arc<WorldMemory>,
    reports: Arc<Reports>,
    approval: Arc<ApprovalManager>,
    phone: Arc<Phone>,
    posted: Arc<Mutex<usize>>,
    calls: Arc<Mutex<usize>>,
    ticks_seen: Arc<Mutex<Vec<String>>>,
}

/// `Full` autonomy, an approval manager with a phone attached (calls wait
/// `wait_secs` for a tap), world memory, a recording reporter, and two tools:
/// `goal` and the gated `post_note`.
fn bench(steps: Vec<Step>, wait_secs: u64) -> Bench {
    let scheduler = Scheduler::new(":memory:").unwrap();
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let reports = Arc::new(Reports::default());
    let goals = Arc::new(
        Goals::new(Arc::clone(&scheduler))
            .with_world(Arc::clone(&world))
            .with_reporter(Arc::clone(&reports) as Arc<dyn GoalReporter>)
            .with_history(5),
    );
    let grants = std::env::temp_dir().join(format!(
        "obc-standing-goals-grants-{}.json",
        uuid::Uuid::new_v4()
    ));
    let approval = Arc::new(ApprovalManager::with_grants(
        &AutonomyConfig {
            approval_wait_secs: wait_secs,
            ..AutonomyConfig::default()
        },
        ForeverGrants::load(grants),
        true,
    ));
    let phone = Arc::new(Phone::default());
    approval.add_prompter(Arc::clone(&phone) as Arc<dyn ApprovalPrompter>);
    let posted = Arc::new(Mutex::new(0));
    let calls = Arc::new(Mutex::new(0));
    let ticks_seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(Script {
        steps: Mutex::new(steps.into()),
        calls: Arc::clone(&calls),
        ticks_seen: Arc::clone(&ticks_seen),
    });
    let agent = Agent::new(
        AgentConfig {
            name: "standing-goals-test".into(),
            system_prompt: "test".into(),
            max_tool_iterations: 3,
            ..AgentConfig::default()
        },
        provider,
        Arc::new(MemoryStore::open_in_memory().unwrap()),
        vec![
            Box::new(GoalTool::new(
                Arc::clone(&goals),
                Tz::Utc,
                "every 30 minutes",
            )),
            Box::new(PostNote(Arc::clone(&posted))),
        ],
    )
    .with_approval(Arc::clone(&approval));
    let handle = Arc::new(AgentHandle::new(Arc::new(agent), ProviderConfig::default()));
    Bench {
        handle,
        goals,
        scheduler,
        world,
        reports,
        approval,
        phone,
        posted,
        calls,
        ticks_seen,
    }
}

/// Run the real scheduler loop with `main.rs`'s dispatch until `n` tasks
/// have run, with standing goals on or off.
async fn run(b: &Bench, n: usize, goals_on: bool) -> Vec<Ran> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Ran>(n.max(1));
    let h = Arc::clone(&b.handle);
    let g = goals_on.then(|| Arc::clone(&b.goals));
    let looper = tokio::spawn(run_scheduler_loop(Arc::clone(&b.scheduler), 1, move |d| {
        let h = Arc::clone(&h);
        let g = g.clone();
        let tx = tx.clone();
        async move {
            let _ = tx.send(dispatch(&h, d, None, g.as_deref()).await).await;
        }
    }));
    let mut out = Vec::new();
    while out.len() < n {
        let ran = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("the scheduler ran a task within 10 s")
            .expect("an outcome");
        out.push(ran);
    }
    looper.abort();
    out
}

fn tick(r: &Ran) -> &obc_agent::goals::TickOutcome {
    match r {
        Ran::Goal(t) => t,
        Ran::Task(o) => panic!("a goal ran as a plain task: {o:?}"),
    }
}

fn add_goal(b: &Bench, text: &str, done_when: &str) -> Goal {
    let g = Goal::new(text, done_when, "telegram:42");
    b.scheduler
        .add_goal(&g, TaskKind::Interval(1), Tz::Utc)
        .unwrap();
    g
}

#[tokio::test]
async fn a_goal_set_in_chat_reports_back_there_only_when_something_changed() {
    let b = bench(
        vec![
            Step::Call(
                "goal",
                json!({"action": "set", "goal": "Keep the greenhouse between 18 and 24 C",
                       "every": "every 5 seconds"}),
            ),
            Step::Say("Watching it."),
            Step::Say("NOTE: 21 C, fine\nCHANGED: yes\nDONE: no"),
            Step::Say("NOTE: 21 C, fine\nCHANGED: no\nDONE: no"),
            Step::Say("**NOTE:** 26 C, too warm\n**CHANGED:** yes\n**DONE:** no"),
        ],
        30,
    );
    let set = b
        .handle
        .process("tg-42", "keep the greenhouse between 18 and 24 C")
        .await
        .unwrap();
    let goal = b
        .scheduler
        .find_goal("keep the greenhouse between 18 and 24 c")
        .unwrap()
        .unwrap_or_else(|| panic!("the goal tool stored no goal: {:?}", set.tool_calls));
    // The owner is the chat it was set from, not anything the model said.
    assert_eq!(goal.owner, "telegram:42");
    // The tool's floor is 5 s; tick every second so the test is quick.
    b.scheduler
        .add_goal(&goal, TaskKind::Interval(1), Tz::Utc)
        .unwrap();

    let ran = run(&b, 3, true).await;
    let reported: Vec<bool> = ran.iter().map(|r| tick(r).reported).collect();
    assert_eq!(
        reported,
        [true, false, true],
        "an unchanged check was reported, or a changed one was not: {ran:?}"
    );
    let reports = b.reports.0.lock().unwrap().clone();
    assert_eq!(
        reports,
        [
            (
                "telegram:42".to_string(),
                "🎯 Keep the greenhouse between 18 and 24 C: 21 C, fine".to_string()
            ),
            (
                "telegram:42".to_string(),
                "🎯 Keep the greenhouse between 18 and 24 C: 26 C, too warm".to_string()
            ),
        ]
    );

    // Each tick saw the goal and the notes before it.
    let seen = b.ticks_seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "{seen:#?}");
    assert!(seen[0].contains("This is the first check."), "{}", seen[0]);
    assert!(
        seen[2].contains("Notes from earlier checks, oldest first:\n- 21 C, fine\n- 21 C, fine"),
        "{}",
        seen[2]
    );

    // The notes are world-memory history; the goal stays open and ticking.
    let history = b.world.history(&progress_entity(&goal.id)).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[2].value["note"], "26 C, too warm");
    let goal = b.scheduler.get_goal(&goal.id).unwrap().unwrap();
    assert_eq!((goal.status, goal.ticks), (GoalStatus::Active, 3));
    assert!(b.scheduler.get_task(&goal.id).unwrap().unwrap().enabled);
}

#[tokio::test]
async fn a_goal_closes_itself_when_met_and_not_before() {
    let b = bench(
        vec![
            Step::Say("NOTE: no deer yet\nCHANGED: no\nDONE: no"),
            Step::Say("NOTE: deer at 05:12, logged\nCHANGED: yes\nDONE: yes"),
        ],
        30,
    );
    let g = add_goal(
        &b,
        "Log every deer on the trail cam",
        "a deer has been logged",
    );
    let ran = run(&b, 2, true).await;
    let (first, second) = (tick(&ran[0]), tick(&ran[1]));
    assert_eq!((first.status, first.done), (GoalStatus::Active, false));
    assert!(!first.reported, "a quiet check was reported: {first:?}");
    assert_eq!((second.status, second.done), (GoalStatus::Done, true));

    assert_eq!(
        b.scheduler.get_goal(&g.id).unwrap().unwrap().status,
        GoalStatus::Done
    );
    assert!(
        !b.scheduler.get_task(&g.id).unwrap().unwrap().enabled,
        "a met goal kept its task running"
    );
    let reports = b.reports.0.lock().unwrap().clone();
    assert_eq!(
        reports,
        [(
            "telegram:42".to_string(),
            "🎯 Done — Log every deer on the trail cam: deer at 05:12, logged".to_string()
        )]
    );
    // Its progress belief is closed because its source stopped, not erased.
    let entity = progress_entity(&g.id);
    assert!(b.world.current(&entity).unwrap().is_none());
    let last = b.world.history(&entity).unwrap().pop().unwrap();
    assert_eq!(
        Closure::of(&last),
        Closure::SourceStopped(format!("goal:{}", g.id))
    );

    // The same "DONE: yes" cannot close a goal that has no end condition.
    let b = bench(vec![Step::Say("NOTE: dies in about 60 h\nDONE: yes")], 30);
    let g = add_goal(&b, "Watch the LoRa node's battery trend", "");
    let ran = run(&b, 1, true).await;
    assert_eq!(tick(&ran[0]).status, GoalStatus::Active, "{ran:?}");
    assert!(b.scheduler.get_task(&g.id).unwrap().unwrap().enabled);
}

#[tokio::test]
async fn a_gated_tool_in_a_tick_waits_for_the_chat_tap_like_a_scheduled_task() {
    let steps = || {
        vec![
            Step::Call("post_note", json!({})),
            Step::Say("NOTE: battery 3.41 V, posted\nCHANGED: yes\nDONE: no"),
        ]
    };

    // Nobody answers the prompt: the tool does not run.
    let b = bench(steps(), 1);
    add_goal(&b, "Watch the LoRa node's battery trend", "");
    let ran = run(&b, 1, true).await;
    assert!(tick(&ran[0]).ok, "{ran:?}");
    assert_eq!(*b.posted.lock().unwrap(), 0, "a gated tool ran unanswered");
    let shown = b.phone.0.lock().unwrap()[0].clone();
    assert_eq!(shown.tool_name, "post_note");
    let text = shown.prompt_text();
    assert!(
        text.contains("standing goal 'Watch the LoRa node's battery trend'"),
        "the prompt should name the goal that asked: {text}"
    );

    // The same tick with the approver tapping Yes: it runs.
    let b = bench(steps(), 30);
    add_goal(&b, "Watch the LoRa node's battery trend", "");
    let (approval, phone) = (Arc::clone(&b.approval), Arc::clone(&b.phone));
    let tap = tokio::spawn(async move {
        for _ in 0..1000 {
            let shown = phone.0.lock().unwrap().first().cloned();
            if let Some(p) = shown {
                return approval.resolve_pending(
                    &p.id,
                    ApprovalResponse::Yes,
                    "telegram",
                    "42",
                    &["42".to_string()],
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the phone was never asked");
    });
    let ran = run(&b, 1, true).await;
    assert!(tap.await.unwrap().is_ok());
    assert_eq!(
        *b.posted.lock().unwrap(),
        1,
        "approved in chat, still refused"
    );
    assert_eq!(tick(&ran[0]).note, "battery 3.41 V, posted");
}

#[tokio::test]
async fn goal_ticks_and_plain_tasks_take_their_own_paths_and_goals_off_skips_ticks() {
    let b = bench(vec![Step::Say("NOTE: 20 C\nDONE: no")], 30);
    let g = add_goal(&b, "Keep the greenhouse between 18 and 24 C", "");
    let reminder = ScheduledTask::one_shot(
        "reminder",
        "reminder",
        "say hi",
        "scheduled-test",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 1,
    );
    b.scheduler.add_task(reminder).unwrap();
    let ran = run(&b, 2, true).await;
    assert!(
        ran.iter()
            .any(|r| matches!(r, Ran::Goal(t) if t.goal_id == g.id && t.ok)),
        "{ran:?}"
    );
    assert!(
        ran.iter()
            .any(|r| matches!(r, Ran::Task(o) if o.task_id == "reminder" && o.ok)),
        "{ran:?}"
    );

    // With standing goals off, a goal's task runs nothing.
    let b = bench(vec![Step::Say("NOTE: 20 C\nDONE: no")], 30);
    let g = add_goal(&b, "Keep the greenhouse between 18 and 24 C", "");
    let ran = run(&b, 1, false).await;
    match &ran[0] {
        Ran::Task(o) => {
            assert!(!o.ok);
            assert!(o.text.contains("standing goals are off"), "{}", o.text);
        }
        other => panic!("a goal ticked with goals off: {other:?}"),
    }
    assert_eq!(*b.calls.lock().unwrap(), 0, "the model was asked");
    assert_eq!(b.scheduler.get_goal(&g.id).unwrap().unwrap().ticks, 0);
}

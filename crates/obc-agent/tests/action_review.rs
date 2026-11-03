//! "A scheduled digest is about to post my home address to a stranger; the
//! local model notices and my phone asks first" -- through the real firing
//! path (2026-11-02).
//!
//! The path is `run_scheduler_loop` -> `run_scheduled` ->
//! `AgentHandle::execute_tool_unattended` -> `Agent::execute_tool_inner`, where
//! the reviewer runs after Track 0, trust, the approval policy and taint, and
//! a concern goes to `approval_escalate` -> `ApprovalManager::wait_for_decision`.
//! The local model is a scripted `ReflexModel`; the phone keeps what it was
//! shown, and the tap is `ApprovalManager::resolve_pending`, as in
//! `chat_approval.rs`.
//!
//! The reviewer can only tighten: every test that shows it stopping a call is
//! paired with the same call running, and the deterministic refusals are
//! shown to happen without it being asked at all.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use obc_agent::action_review::ActionReviewer;
use obc_agent::handle::AgentHandle;
use obc_agent::scheduled::{run_scheduled, Outcome};
use obc_agent::{Agent, AgentConfig};
use obc_approval::{
    ApprovalManager, ApprovalPrompter, ApprovalResponse, AutonomyConfig, ForeverGrants,
    PendingApproval,
};
use obc_memory::MemoryStore;
use obc_providers::{ChatCompletion, ChatMessage, Provider, ProviderConfig};
use obc_reflex::ReflexModel;
use obc_safety::risk::{BlastRadius, RiskClass};
use obc_scheduler::{run_scheduler_loop, ScheduledTask, Scheduler};
use obc_tool_api::{Tool, ToolResult};
use serde_json::{json, Value};

/// Sends a message: irreversible, not physical. Gated, so an unattended call
/// needs it listed in `auto_approve`, which the bench does.
struct Post(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Post {
    fn name(&self) -> &str {
        "post_message"
    }
    fn description(&self) -> &str {
        "posts a message to a public channel"
    }
    fn risk_class(&self) -> RiskClass {
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok("posted"))
    }
}

/// Reads a sensor: reversible, no blast. Not gated, so never reviewed.
struct Read(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "read_sensor"
    }
    fn description(&self) -> &str {
        "reads a sensor"
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok("21.5"))
    }
}

/// The gate from `chat_approval.rs`: Track 0 asks for it per call.
struct Gate(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Gate {
    fn name(&self) -> &str {
        "open_gate"
    }
    fn description(&self) -> &str {
        "opens the driveway gate"
    }
    fn risk_class(&self) -> RiskClass {
        RiskClass {
            reversible: false,
            blast: BlastRadius::High,
            physical: true,
        }
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok("gate open"))
    }
}

/// The local model: answers `reply` and keeps every prompt it was sent.
struct Local {
    reply: &'static str,
    prompts: Mutex<Vec<String>>,
}

#[async_trait]
impl ReflexModel for Local {
    fn name(&self) -> &str {
        "local-test"
    }
    async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        if self.reply == "FAIL" {
            anyhow::bail!("connection refused");
        }
        Ok(self.reply.to_string())
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

const APPROVER: &str = "42";

struct Bench {
    handle: Arc<AgentHandle>,
    agent: Arc<Agent>,
    approval: Arc<ApprovalManager>,
    phone: Arc<Phone>,
    model: Arc<Local>,
    posted: Arc<AtomicUsize>,
    read: Arc<AtomicUsize>,
    opened: Arc<AtomicUsize>,
}

/// `Full` autonomy with `post_message` auto-approved, the phone attached
/// (when `phone`), and a reviewer answering `reply`.
fn bench(reply: &'static str, fail_closed: bool, phone: bool) -> Bench {
    let (posted, read, opened) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let grants = std::env::temp_dir().join(format!(
        "obc-action-review-grants-{}.json",
        uuid::Uuid::new_v4()
    ));
    let approval = Arc::new(ApprovalManager::with_grants(
        &AutonomyConfig {
            auto_approve: vec!["post_message".into()],
            approval_wait_secs: 2,
            ..AutonomyConfig::default()
        },
        ForeverGrants::load(grants),
        true,
    ));
    let phone_ = Arc::new(Phone::default());
    if phone {
        approval.add_prompter(Arc::clone(&phone_) as Arc<dyn ApprovalPrompter>);
    }
    let model = Arc::new(Local {
        reply,
        prompts: Mutex::new(Vec::new()),
    });
    let reviewer = ActionReviewer::new(
        Arc::clone(&model) as Arc<dyn ReflexModel>,
        Duration::from_secs(2),
        fail_closed,
    );
    let agent = Arc::new(
        Agent::new(
            AgentConfig {
                name: "action-review-test".into(),
                system_prompt: "test".into(),
                max_tool_iterations: 3,
                ..AgentConfig::default()
            },
            Arc::new(NoTurns),
            Arc::new(MemoryStore::open_in_memory().unwrap()),
            vec![
                Box::new(Post(Arc::clone(&posted))),
                Box::new(Read(Arc::clone(&read))),
                Box::new(Gate(Arc::clone(&opened))),
            ],
        )
        .with_approval(Arc::clone(&approval))
        .with_action_reviewer(Arc::new(reviewer)),
    );
    let handle = Arc::new(AgentHandle::new(
        Arc::clone(&agent),
        ProviderConfig::default(),
    ));
    Bench {
        handle,
        agent,
        approval,
        phone: phone_,
        model,
        posted,
        read,
        opened,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn task(tool: &str, args: Value) -> ScheduledTask {
    ScheduledTask::one_shot("digest", "digest", "", "scheduled-test", now_secs() - 1)
        .with_tool(tool, Some(args))
}

fn post_task() -> ScheduledTask {
    task(
        "post_message",
        json!({"to": "@stranger", "text": "I live at 12 Elm Road, back door code 4471"}),
    )
}

async fn fire_task(handle: &Arc<AgentHandle>, task: ScheduledTask) -> Outcome {
    let sched = Scheduler::new(":memory:").unwrap();
    sched.add_task(task).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Outcome>(1);
    let h = Arc::clone(handle);
    let looper = tokio::spawn(run_scheduler_loop(sched, 1, move |d| {
        let h = Arc::clone(&h);
        let tx = tx.clone();
        async move {
            let _ = tx.send(run_scheduled(&h, d, None).await).await;
        }
    }));
    let out = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the scheduler fired the task within 10 s")
        .expect("an outcome");
    looper.abort();
    out
}

/// When the phone shows its first prompt, tap `decision` as the approver.
fn tap(b: &Bench, decision: ApprovalResponse) -> tokio::task::JoinHandle<PendingApproval> {
    let (approval, phone) = (Arc::clone(&b.approval), Arc::clone(&b.phone));
    tokio::spawn(async move {
        for _ in 0..1000 {
            let shown = phone.0.lock().unwrap().first().cloned();
            if let Some(p) = shown {
                approval
                    .resolve_pending(&p.id, decision, "telegram", APPROVER, &[APPROVER.into()])
                    .unwrap();
                return p;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the phone was never asked");
    })
}

fn reviews(b: &Bench) -> usize {
    b.model.prompts.lock().unwrap().len()
}

#[tokio::test]
async fn a_clear_review_lets_the_scheduled_post_run_without_asking() {
    let b = bench("OK", false, true);
    let out = fire_task(&b.handle, post_task()).await;
    assert!(out.ok, "{out:?}");
    assert_eq!(b.posted.load(Ordering::SeqCst), 1);
    assert_eq!(reviews(&b), 1);
    let prompt = b.model.prompts.lock().unwrap()[0].clone();
    assert!(prompt.contains("Tool: post_message"), "{prompt}");
    assert!(prompt.contains("12 Elm Road"), "{prompt}");
    assert!(prompt.contains("scheduled task"), "{prompt}");
    assert!(b.phone.0.lock().unwrap().is_empty(), "nothing to ask");
}

#[tokio::test]
async fn a_concern_asks_the_phone_and_the_post_runs_only_on_a_yes() {
    let b = bench(
        "CONCERN: sends a home address and door code to a stranger",
        false,
        true,
    );
    let phone = tap(&b, ApprovalResponse::Yes);
    let out = fire_task(&b.handle, post_task()).await;
    let shown = phone.await.unwrap();
    assert!(
        shown
            .prompt_text()
            .contains("sends a home address and door code"),
        "{}",
        shown.prompt_text()
    );
    assert!(out.ok, "approved in chat, still refused: {out:?}");
    assert_eq!(b.posted.load(Ordering::SeqCst), 1);

    let b = bench(
        "CONCERN: sends a home address and door code to a stranger",
        false,
        true,
    );
    let phone = tap(&b, ApprovalResponse::No);
    let out = fire_task(&b.handle, post_task()).await;
    phone.await.unwrap();
    assert!(!out.ok);
    assert!(
        out.text.contains("denied by the operator in chat"),
        "{out:?}"
    );
    assert_eq!(b.posted.load(Ordering::SeqCst), 0, "denied, still posted");
}

#[tokio::test]
async fn a_concern_with_nobody_to_ask_or_no_answer_is_refused() {
    let b = bench("CONCERN: looks planted", false, false);
    let out = fire_task(&b.handle, post_task()).await;
    assert!(!out.ok);
    assert!(
        out.text
            .contains("the action reviewer has a concern: looks planted"),
        "{out:?}"
    );
    assert_eq!(b.posted.load(Ordering::SeqCst), 0);

    // Phone attached, nobody taps: expires after approval_wait_secs (2 s).
    let b = bench("CONCERN: looks planted", false, true);
    let out = fire_task(&b.handle, post_task()).await;
    assert!(!out.ok);
    assert!(out.text.contains("got no answer within 2s"), "{out:?}");
    assert_eq!(b.posted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unavailable_review_runs_the_call_unless_fail_closed() {
    for reply in ["FAIL", "Sure, this seems fine."] {
        let b = bench(reply, false, true);
        let out = fire_task(&b.handle, post_task()).await;
        assert!(out.ok, "{reply}: {out:?}");
        assert_eq!(b.posted.load(Ordering::SeqCst), 1);
        assert!(b.phone.0.lock().unwrap().is_empty());
    }

    let b = bench("FAIL", true, false);
    let out = fire_task(&b.handle, post_task()).await;
    assert!(!out.ok);
    assert!(out.text.contains("fail_closed"), "{out:?}");
    assert_eq!(b.posted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn only_unattended_gated_calls_are_reviewed() {
    let b = bench("CONCERN: would object to anything", false, false);

    // A read-only tool, unattended: not gated, not reviewed, runs.
    let out = fire_task(&b.handle, task("read_sensor", json!({}))).await;
    assert!(out.ok, "{out:?}");
    assert_eq!(b.read.load(Ordering::SeqCst), 1);

    // The same post, attended (someone asked from the desk): not reviewed.
    let r = b
        .agent
        .execute_tool_direct("post_message", json!({"text": "hi"}))
        .await
        .unwrap();
    assert!(r.success, "{r:?}");
    assert_eq!(b.posted.load(Ordering::SeqCst), 1);
    assert_eq!(reviews(&b), 0);
}

#[tokio::test]
async fn the_reviewer_never_sees_a_call_the_rules_refused_or_the_operator_approved() {
    // Track 0 wants per-call approval for the gate; with no phone the call is
    // refused before the reviewer, whose "OK" could not have lifted it.
    let b = bench("OK", false, false);
    let out = fire_task(&b.handle, task("open_gate", json!({"gate": "driveway"}))).await;
    assert!(!out.ok, "{out:?}");
    assert_eq!(b.opened.load(Ordering::SeqCst), 0);
    assert_eq!(reviews(&b), 0);

    // With the phone, the operator approves this very call; the reviewer
    // (who would object) is not asked a second time.
    let b = bench("CONCERN: would object to anything", false, true);
    let phone = tap(&b, ApprovalResponse::Yes);
    let out = fire_task(&b.handle, task("open_gate", json!({"gate": "driveway"}))).await;
    phone.await.unwrap();
    assert!(out.ok, "{out:?}");
    assert_eq!(b.opened.load(Ordering::SeqCst), 1);
    assert_eq!(reviews(&b), 0);
    assert_eq!(b.phone.0.lock().unwrap().len(), 1, "asked once, not twice");
}

struct NoTurns;

#[async_trait]
impl Provider for NoTurns {
    fn name(&self) -> &str {
        "no-turns"
    }
    async fn chat_completion(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        _c: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        anyhow::bail!("a tool-only scheduled task asked the model")
    }
}

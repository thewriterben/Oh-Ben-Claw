//! "A scheduled task wants to open the gate, I tap Approve on my phone, the
//! gate opens" -- through the real firing path (2026-11-01).
//!
//! The path is `run_scheduler_loop` -> `run_scheduled` ->
//! `AgentHandle::execute_tool_unattended` -> `Agent::execute_tool_inner` ->
//! `approval_authorize_waiting` -> `ApprovalManager::wait_for_decision`, which
//! shows the call on every attached `ApprovalPrompter` and waits. The phone
//! here is a prompter that keeps what it was shown, and the tap is
//! `ApprovalManager::resolve_pending`, which is what the Telegram and Slack
//! channels call when a button is pressed. Their wire formats are tested in
//! `obc-channels`.
//!
//! As in `scheduled_track0.rs`, every "it did not run" is paired with the same
//! fixture running once someone with the right to approve says yes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use obc_agent::handle::AgentHandle;
use obc_agent::scheduled::{run_scheduled, Outcome};
use obc_agent::{Agent, AgentConfig};
use obc_approval::{
    ApprovalManager, ApprovalPrompter, ApprovalResponse, AutonomyConfig, ForeverGrants,
    PendingApproval, ResolveError,
};
use obc_memory::MemoryStore;
use obc_providers::{ChatCompletion, ChatMessage, Provider, ProviderConfig, ToolCall};
use obc_safety::risk::{BlastRadius, RiskClass};
use obc_scheduler::{run_scheduler_loop, ScheduledTask, Scheduler};
use obc_tool_api::{Tool, ToolResult};
use serde_json::{json, Value};

/// The gate: physical, irreversible, high blast. Track 0 asks for it per call
/// even under `Full` autonomy, and a forever grant would not cover it.
struct Gate(Arc<Mutex<usize>>);

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
        *self.0.lock().unwrap() += 1;
        Ok(ToolResult::ok("gate open"))
    }
}

/// Stands in for Telegram or Slack: keeps every prompt it was asked to show.
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

fn approvers() -> Vec<String> {
    vec![APPROVER.to_string()]
}

struct Bench {
    handle: Arc<AgentHandle>,
    approval: Arc<ApprovalManager>,
    phone: Arc<Phone>,
    opened: Arc<Mutex<usize>>,
}

/// `Full` autonomy (the default), an approval manager with the phone attached
/// and calls waiting `wait_secs`, and one tool: the gate.
fn bench(provider: Arc<dyn Provider>, wait_secs: u64) -> Bench {
    let opened = Arc::new(Mutex::new(0));
    let grants = std::env::temp_dir().join(format!(
        "obc-chat-approval-grants-{}.json",
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
    let agent = Agent::new(
        AgentConfig {
            name: "chat-approval-test".into(),
            system_prompt: "test".into(),
            max_tool_iterations: 3,
            ..AgentConfig::default()
        },
        provider,
        Arc::new(MemoryStore::open_in_memory().unwrap()),
        vec![Box::new(Gate(Arc::clone(&opened)))],
    )
    .with_approval(Arc::clone(&approval));
    let handle = Arc::new(AgentHandle::new(Arc::new(agent), ProviderConfig::default()));
    Bench {
        handle,
        approval,
        phone,
        opened,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn gate_task() -> ScheduledTask {
    ScheduledTask::one_shot(
        "open-gate",
        "open-gate",
        "",
        "scheduled-test",
        now_secs() - 1,
    )
    .with_tool("open_gate", Some(json!({"gate": "driveway"})))
}

/// Let the real scheduler loop pick up `task` and hand it to `run_scheduled`.
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

/// The next prompt the phone shows that it has not shown before.
async fn next_prompt(phone: &Phone, seen: usize) -> PendingApproval {
    for _ in 0..1000 {
        if let Some(p) = phone.0.lock().unwrap().get(seen).cloned() {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the phone was never asked");
}

/// When the phone shows its first prompt, tap `decision` as `user`.
fn tap(
    b: &Bench,
    user: &'static str,
    decision: ApprovalResponse,
) -> tokio::task::JoinHandle<(PendingApproval, Result<PendingApproval, ResolveError>)> {
    let (approval, phone) = (Arc::clone(&b.approval), Arc::clone(&b.phone));
    tokio::spawn(async move {
        let shown = next_prompt(&phone, 0).await;
        let answer = approval.resolve_pending(&shown.id, decision, "telegram", user, &approvers());
        (shown, answer)
    })
}

#[tokio::test]
async fn a_scheduled_task_waits_for_a_tap_and_the_gate_opens() {
    let b = bench(Arc::new(NoTurns), 30);
    let phone = tap(&b, APPROVER, ApprovalResponse::Yes);

    let out = fire_task(&b.handle, gate_task()).await;
    let (shown, answer) = phone.await.unwrap();
    assert!(
        answer.is_ok(),
        "the approver's tap was not taken: {answer:?}"
    );
    assert!(out.ok, "approved in chat, still refused: {out:?}");
    assert_eq!(*b.opened.lock().unwrap(), 1, "the gate did not open");

    // The phone was told what, why, and who asked.
    assert_eq!(shown.tool_name, "open_gate");
    assert_eq!(shown.arguments, json!({"gate": "driveway"}));
    let text = shown.prompt_text();
    assert!(text.contains("scheduled task 'open-gate'"), "{text}");
    assert!(text.contains("PHYSICAL"), "{text}");

    let last = b.approval.audit_log().pop().unwrap();
    assert_eq!(last.channel, "telegram:42");
    assert_eq!(last.decision, ApprovalResponse::Yes);
    assert!(b.approval.pending().is_empty());
}

#[tokio::test]
async fn nobody_answering_refuses_the_gate_and_audits_the_expiry() {
    let b = bench(Arc::new(NoTurns), 1);
    let out = fire_task(&b.handle, gate_task()).await;
    assert!(!out.ok, "an unanswered gate opened: {out:?}");
    assert_eq!(*b.opened.lock().unwrap(), 0);
    assert!(
        out.text.contains("no answer within 1s"),
        "the refusal should say it expired: {}",
        out.text
    );
    assert_eq!(
        b.phone.0.lock().unwrap().len(),
        1,
        "the phone was asked once"
    );
    let last = b.approval.audit_log().pop().unwrap();
    assert_eq!(
        (last.channel.as_str(), last.decision),
        ("expired", ApprovalResponse::No)
    );

    // The same fixture, answered this time.
    let phone = {
        let (approval, phone) = (Arc::clone(&b.approval), Arc::clone(&b.phone));
        tokio::spawn(async move {
            let shown = next_prompt(&phone, 1).await;
            approval.resolve_pending(
                &shown.id,
                ApprovalResponse::Yes,
                "telegram",
                APPROVER,
                &approvers(),
            )
        })
    };
    let out = fire_task(&b.handle, gate_task()).await;
    assert!(phone.await.unwrap().is_ok());
    assert!(out.ok, "{out:?}");
    assert_eq!(*b.opened.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_tap_from_someone_else_is_refused_and_the_approver_still_decides() {
    let b = bench(Arc::new(NoTurns), 30);
    let (approval, phone) = (Arc::clone(&b.approval), Arc::clone(&b.phone));
    let taps = tokio::spawn(async move {
        let shown = next_prompt(&phone, 0).await;
        let stranger = approval.resolve_pending(
            &shown.id,
            ApprovalResponse::Yes,
            "telegram",
            "99",
            &approvers(),
        );
        let still_waiting = approval.pending().len();
        let owner = approval.resolve_pending(
            &shown.id,
            ApprovalResponse::Yes,
            "telegram",
            APPROVER,
            &approvers(),
        );
        (stranger, still_waiting, owner)
    });

    let out = fire_task(&b.handle, gate_task()).await;
    let (stranger, still_waiting, owner) = taps.await.unwrap();
    assert_eq!(stranger.unwrap_err(), ResolveError::NotAnApprover);
    assert_eq!(still_waiting, 1, "a stranger's tap answered the call");
    assert!(owner.is_ok());
    assert!(out.ok, "{out:?}");
    assert_eq!(*b.opened.lock().unwrap(), 1);

    let log = b.approval.audit_log();
    let refused = log
        .iter()
        .find(|e| e.channel == "telegram:99")
        .expect("the stranger's tap is audited");
    assert_eq!(refused.refused.as_deref(), Some("not an approver"));
    assert_eq!(refused.tool_name, "open_gate");
}

#[tokio::test]
async fn deny_in_chat_keeps_the_gate_shut() {
    let b = bench(Arc::new(NoTurns), 30);
    let phone = tap(&b, APPROVER, ApprovalResponse::No);
    let out = fire_task(&b.handle, gate_task()).await;
    assert!(phone.await.unwrap().1.is_ok());
    assert!(!out.ok, "denied in chat, still ran: {out:?}");
    assert!(
        out.text
            .contains("denied by the operator in chat (telegram:42)"),
        "{}",
        out.text
    );
    assert_eq!(*b.opened.lock().unwrap(), 0);

    // Control: the same fixture approved.
    let b = bench(Arc::new(NoTurns), 30);
    let phone = tap(&b, APPROVER, ApprovalResponse::Always);
    let out = fire_task(&b.handle, gate_task()).await;
    assert!(phone.await.unwrap().1.is_ok());
    assert!(out.ok, "{out:?}");
    assert_eq!(*b.opened.lock().unwrap(), 1);
    assert_eq!(b.approval.session_grants(), vec!["open_gate".to_string()]);
}

/// An A2A turn waits the same way, and the prompt says another agent asked.
#[tokio::test]
async fn an_a2a_turn_that_asks_for_the_gate_waits_for_the_tap_too() {
    use obc_a2a::{Message, TaskExecutor};
    use obc_agent::a2a_executor::AgentExecutor;

    let b = bench(Arc::new(AsksForGate(Mutex::new(0))), 30);
    let phone = tap(&b, APPROVER, ApprovalResponse::Yes);
    let executor = AgentExecutor::new(b.handle.agent_arc(), ProviderConfig::default());
    let task = tokio::time::timeout(
        Duration::from_secs(10),
        executor.execute(Message::user_text("open the gate")),
    )
    .await
    .expect("the A2A turn finished");
    let (shown, answer) = phone.await.unwrap();
    assert!(answer.is_ok());
    assert!(shown.prompt_text().contains("an A2A call"));
    assert_eq!(*b.opened.lock().unwrap(), 1, "{:?}", task.status);

    // Control: the same turn with nobody to ask is refused at once.
    let b = bench(Arc::new(AsksForGate(Mutex::new(0))), 0);
    let executor = AgentExecutor::new(b.handle.agent_arc(), ProviderConfig::default());
    executor.execute(Message::user_text("open the gate")).await;
    assert_eq!(*b.opened.lock().unwrap(), 0);
    assert!(b.phone.0.lock().unwrap().is_empty());
}

/// For tool-only tasks: the model is never asked.
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

/// Ask for `open_gate` once, then stop.
struct AsksForGate(Mutex<usize>);

#[async_trait]
impl Provider for AsksForGate {
    fn name(&self) -> &str {
        "asks-for-gate"
    }
    async fn chat_completion(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        c: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        let first = {
            let mut s = self.0.lock().unwrap();
            *s += 1;
            *s == 1
        };
        let tool_calls = if first {
            vec![ToolCall {
                id: "g1".into(),
                name: "open_gate".into(),
                args: "{}".into(),
            }]
        } else {
            vec![]
        };
        Ok(ChatCompletion {
            message: if first { String::new() } else { "done".into() },
            tool_calls,
            provider: "asks-for-gate".into(),
            model: c.model.clone(),
            usage: None,
        })
    }
}

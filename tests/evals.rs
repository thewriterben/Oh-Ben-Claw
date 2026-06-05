//! Phase 15 WS4 — evaluation harness (CC/CD).
//!
//! Golden-expectation evals run as integration tests so `cargo test` is the
//! release gate: **no release while evals regress**. Unlike unit tests, these
//! pin end-to-end *behavior* — agent-loop tool routing against a scripted
//! deterministic provider, MCP/A2A wire-shape goldens, and the approval
//! policy matrix.
//!
//! Design notes:
//! - The provider mock is fully deterministic (scripted completions), per the
//!   WS4 rule that gates use deterministic mocks; LLM-as-judge scoring stays
//!   advisory-only until variance is measured.
//! - Goldens assert exact values, not just "is ok" — a changed wire shape or
//!   routing order is a regression even if nothing crashes.

use async_trait::async_trait;
use oh_ben_claw::agent::Agent;
use oh_ben_claw::config::{AgentConfig, AutonomyConfig, AutonomyLevel, ProviderConfig};
use oh_ben_claw::memory::MemoryStore;
use oh_ben_claw::providers::{ChatCompletion, ChatMessage, Provider, ToolCall};
use oh_ben_claw::tools::traits::{Tool, ToolResult};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;

// ── Scripted provider mock ────────────────────────────────────────────────────

/// A deterministic provider that replays a fixed script of completions.
struct ScriptedProvider {
    script: Mutex<Vec<ChatCompletion>>,
    /// Records the number of tools visible to the model on each call.
    tool_counts_seen: Mutex<Vec<usize>>,
}

impl ScriptedProvider {
    fn new(completions: Vec<ChatCompletion>) -> Self {
        let mut script = completions;
        script.reverse(); // pop() from the back
        Self {
            script: Mutex::new(script),
            tool_counts_seen: Mutex::new(Vec::new()),
        }
    }

    fn text(message: &str) -> ChatCompletion {
        ChatCompletion {
            message: message.to_string(),
            tool_calls: vec![],
            provider: "scripted".to_string(),
            model: "eval-mock".to_string(),
        }
    }

    fn tool_call(name: &str, args: Value) -> ChatCompletion {
        ChatCompletion {
            message: String::new(),
            tool_calls: vec![ToolCall {
                id: format!("call_{name}"),
                name: name.to_string(),
                args: args.to_string(),
            }],
            provider: "scripted".to_string(),
            model: "eval-mock".to_string(),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn chat_completion(
        &self,
        _messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        _config: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        self.tool_counts_seen.lock().push(tools.len());
        self.script
            .lock()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted provider exhausted"))
    }
}

// ── Test tools ────────────────────────────────────────────────────────────────

/// A tool that records its invocations and echoes its args.
struct EchoTool {
    tool_name: String,
    calls: Arc<Mutex<Vec<String>>>,
    fail: bool,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        "eval echo tool"
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        self.calls.lock().push(args.to_string());
        if self.fail {
            Ok(ToolResult::err("simulated tool failure"))
        } else {
            Ok(ToolResult::ok(format!("echo:{args}")))
        }
    }
}

/// Build an agent with an in-memory store and a freshly created session.
/// Returns the agent and the session id (append_message has a FK on sessions,
/// so the session must exist before `process()` is called).
fn make_agent(
    provider: Arc<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
) -> (Agent, String) {
    let config = AgentConfig {
        name: "eval-agent".to_string(),
        system_prompt: "You are an eval agent.".to_string(),
        max_tool_iterations: 4,
    };
    let memory = Arc::new(MemoryStore::open_in_memory().expect("in-memory store"));
    let session_id = memory.create_session("eval").expect("create session");
    (Agent::new(config, provider, memory, tools), session_id)
}

fn echo_tool(name: &str, fail: bool) -> (Box<dyn Tool>, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Box::new(EchoTool {
            tool_name: name.to_string(),
            calls: calls.clone(),
            fail,
        }),
        calls,
    )
}

// ── Eval: agent-loop tool routing ─────────────────────────────────────────────

#[tokio::test]
async fn eval_routing_direct_answer_uses_no_tools() {
    let provider = Arc::new(ScriptedProvider::new(vec![ScriptedProvider::text(
        "The answer is 4.",
    )]));
    let (tool, calls) = echo_tool("camera_capture", false);
    let (agent, session) = make_agent(provider, vec![tool]);

    let resp = agent
        .process(&session, "what is 2+2?", &ProviderConfig::default())
        .await
        .unwrap();

    assert_eq!(resp.message, "The answer is 4.");
    assert!(!resp.used_tools());
    assert!(calls.lock().is_empty());
}

#[tokio::test]
async fn eval_routing_single_tool_then_answer() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call("camera_capture", json!({"device_id": "cam-01"})),
        ScriptedProvider::text("Captured an image from cam-01."),
    ]));
    let (tool, calls) = echo_tool("camera_capture", false);
    let (agent, session) = make_agent(provider, vec![tool]);

    let resp = agent
        .process(&session, "take a photo", &ProviderConfig::default())
        .await
        .unwrap();

    // Golden: exactly one tool call, with the exact args, then the final text.
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "camera_capture");
    assert_eq!(calls.lock().len(), 1);
    assert!(calls.lock()[0].contains("cam-01"));
    assert_eq!(resp.message, "Captured an image from cam-01.");
}

#[tokio::test]
async fn eval_routing_multi_step_sequence_order() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call("sensor_read", json!({"sensor": "temp"})),
        ScriptedProvider::tool_call("send_alert", json!({"message": "hot"})),
        ScriptedProvider::text("Temperature high; alert sent."),
    ]));
    let (sensor, sensor_calls) = echo_tool("sensor_read", false);
    let (alert, alert_calls) = echo_tool("send_alert", false);
    let (agent, session) = make_agent(provider, vec![sensor, alert]);

    let resp = agent
        .process(&session, "check temp and alert if hot", &ProviderConfig::default())
        .await
        .unwrap();

    // Golden: both tools executed, in order, one call each.
    let names: Vec<_> = resp.tool_calls.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, vec!["sensor_read", "send_alert"]);
    assert_eq!(sensor_calls.lock().len(), 1);
    assert_eq!(alert_calls.lock().len(), 1);
    assert_eq!(resp.message, "Temperature high; alert sent.");
}

#[tokio::test]
async fn eval_routing_tool_failure_recovers_to_final_answer() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call("flaky_tool", json!({})),
        ScriptedProvider::text("The tool failed; reporting gracefully."),
    ]));
    let (tool, _) = echo_tool("flaky_tool", true);
    let (agent, session) = make_agent(provider, vec![tool]);

    let resp = agent
        .process(&session, "do the flaky thing", &ProviderConfig::default())
        .await
        .unwrap();

    // Golden: failure is fed back, loop continues to a final response.
    assert_eq!(resp.tool_calls.len(), 1);
    assert!(resp.tool_calls[0].result.contains("simulated tool failure"));
    assert_eq!(resp.message, "The tool failed; reporting gracefully.");
}

#[tokio::test]
async fn eval_routing_unknown_tool_degrades_gracefully() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call("nonexistent_tool", json!({})),
        ScriptedProvider::text("That tool does not exist."),
    ]));
    let (tool, calls) = echo_tool("real_tool", false);
    let (agent, session) = make_agent(provider, vec![tool]);

    let resp = agent
        .process(&session, "use a ghost tool", &ProviderConfig::default())
        .await
        .unwrap();

    // Golden: no real tool ran; the error surfaced to the model; final answer produced.
    assert!(calls.lock().is_empty());
    assert_eq!(resp.message, "That tool does not exist.");
}

// ── Eval: MCP wire-shape goldens ──────────────────────────────────────────────

mod mcp_goldens {
    use oh_ben_claw::mcp::{server::McpServer, JsonRpcRequest, ProtocolMode};
    use serde_json::{json, Value};

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    async fn respond(method: &str, params: Value) -> Value {
        let mut server = McpServer::with_mode(vec![], ProtocolMode::Stateless2026);
        let resp = server.handle_request(req(method, params)).await;
        serde_json::to_value(&resp).unwrap()
    }

    #[tokio::test]
    async fn eval_mcp_initialize_golden() {
        let v = respond("initialize", json!({})).await;
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], "oh-ben-claw");
        assert_eq!(v["result"]["capabilities"]["tools"]["listChanged"], false);
    }

    #[tokio::test]
    async fn eval_mcp_discover_golden() {
        let v = respond("server/discover", json!({})).await;
        assert_eq!(v["result"]["protocolVersion"], "2026-07-28");
    }

    #[tokio::test]
    async fn eval_mcp_tools_list_cache_metadata_golden() {
        let v = respond("tools/list", json!({})).await;
        assert_eq!(v["result"]["ttlMs"], 60_000);
        assert_eq!(v["result"]["cacheScope"], "private");
        assert!(v["result"]["tools"].is_array());
    }

    #[tokio::test]
    async fn eval_mcp_unknown_method_error_golden() {
        let v = respond("bogus/method", json!({})).await;
        assert_eq!(v["error"]["code"], -32601);
    }
}

// ── Eval: A2A wire-shape goldens ──────────────────────────────────────────────

mod a2a_goldens {
    use oh_ben_claw::a2a::{error_codes, A2AMessage, A2AServer, Message};
    use serde_json::json;

    fn server() -> A2AServer {
        A2AServer::new(A2AServer::build_card(
            "eval-agent",
            "eval",
            "http://localhost/a2a",
            "0.0.1",
            &["summarize".to_string()],
        ))
    }

    #[test]
    fn eval_a2a_send_message_task_golden() {
        let mut s = server();
        let msg = Message::user_text("hello");
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "SendMessage",
            Some(json!({ "message": msg })),
            Some(json!(1)),
        ));
        let task = &resp["result"]["task"];
        assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED");
        assert!(task["contextId"].is_string());
        assert_eq!(task["history"][0]["role"], "ROLE_USER");
        // v1.0: no kind discriminators anywhere.
        assert!(task.get("kind").is_none());
        assert!(task["history"][0]["parts"][0].get("kind").is_none());
    }

    #[test]
    fn eval_a2a_task_not_found_error_golden() {
        let mut s = server();
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "GetTask",
            Some(json!({"id": "ghost"})),
            Some(json!(2)),
        ));
        assert_eq!(resp["error"]["code"], error_codes::TASK_NOT_FOUND);
        assert_eq!(
            resp["error"]["data"]["@type"],
            "type.googleapis.com/google.rpc.ErrorInfo"
        );
        assert_eq!(resp["error"]["data"]["domain"], "a2a-protocol.org");
    }

    #[test]
    fn eval_a2a_agent_card_golden() {
        let s = server();
        let card = serde_json::to_value(s.handle_discover()).unwrap();
        assert_eq!(card["supportedInterfaces"][0]["protocolVersion"], "1.0");
        assert_eq!(card["supportedInterfaces"][0]["protocolBinding"], "JSONRPC");
        assert_eq!(card["skills"][0]["id"], "summarize");
    }
}

// ── Eval: observability wiring (WS5) ──────────────────────────────────────────

#[tokio::test]
async fn eval_agent_run_records_spans_and_counters() {
    use oh_ben_claw::observability::{ObsContext, SpanStatus};

    let obs = Arc::new(ObsContext::new());
    let provider = Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call("camera_capture", json!({"device_id": "cam-01"})),
        ScriptedProvider::tool_call("flaky_tool", json!({})),
        ScriptedProvider::text("done"),
    ]));
    let (cam, _) = echo_tool("camera_capture", false);
    let (flaky, _) = echo_tool("flaky_tool", true);
    let (agent, session) = make_agent(provider, vec![cam, flaky]);
    let agent = agent.with_obs(obs.clone());

    agent
        .process(&session, "capture then fail", &ProviderConfig::default())
        .await
        .unwrap();

    // Golden: one agent.process span (ok) + two agent.tool spans (1 ok, 1 error).
    let run_spans = obs.spans.by_name("agent.process");
    assert_eq!(run_spans.len(), 1);
    assert_eq!(run_spans[0].status, SpanStatus::Ok);
    assert_eq!(
        run_spans[0].attrs.get("session_id").map(String::as_str),
        Some(session.as_str())
    );
    assert_eq!(run_spans[0].attrs.get("tool_calls").map(String::as_str), Some("2"));

    let tool_spans = obs.spans.by_name("agent.tool");
    assert_eq!(tool_spans.len(), 2);
    assert_eq!(obs.spans.errors().len(), 1);

    // Counters: exactly 2 tool calls, 1 tool error, 1 agent turn.
    let snap = obs.snapshot();
    assert_eq!(snap.agent_turns_total, 1);
    assert_eq!(snap.tool_errors_total, 1);
    assert_eq!(snap.tool_calls_total, 2);
}

#[tokio::test]
async fn eval_agent_without_obs_records_nothing_and_still_works() {
    let provider = Arc::new(ScriptedProvider::new(vec![ScriptedProvider::text("hi")]));
    let (tool, _) = echo_tool("x", false);
    let (agent, session) = make_agent(provider, vec![tool]);
    let resp = agent
        .process(&session, "hello", &ProviderConfig::default())
        .await
        .unwrap();
    assert_eq!(resp.message, "hi");
}

// ── Eval: approval policy matrix golden ───────────────────────────────────────

#[test]
fn eval_approval_policy_matrix_golden() {
    use oh_ben_claw::approval::{ApprovalManager, ForeverGrants};

    let grants_path = std::env::temp_dir().join(format!(
        "obc_eval_grants_{}.json",
        std::process::id()
    ));
    let cfg = AutonomyConfig {
        level: AutonomyLevel::Supervised,
        auto_approve: vec!["read_file".to_string()],
        always_ask: vec!["delete_file".to_string()],
    };
    let mgr = ApprovalManager::with_grants(&cfg, ForeverGrants::load(&grants_path), true);

    // Golden truth table: (tool, needs_approval)
    let table = [
        ("read_file", false),  // auto_approve
        ("delete_file", true), // always_ask
        ("shell", true),       // supervised default
    ];
    for (tool, expected) in table {
        assert_eq!(mgr.needs_approval(tool), expected, "tool {tool}");
    }
    std::fs::remove_file(&grants_path).ok();
}

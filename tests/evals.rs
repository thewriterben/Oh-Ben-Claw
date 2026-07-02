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
    /// Records the message contents visible to the model on each call.
    messages_seen: Mutex<Vec<Vec<String>>>,
}

impl ScriptedProvider {
    fn new(completions: Vec<ChatCompletion>) -> Self {
        let mut script = completions;
        script.reverse(); // pop() from the back
        Self {
            script: Mutex::new(script),
            tool_counts_seen: Mutex::new(Vec::new()),
            messages_seen: Mutex::new(Vec::new()),
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
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        _config: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        self.tool_counts_seen.lock().push(tools.len());
        self.messages_seen
            .lock()
            .push(messages.iter().map(|m| m.content.clone()).collect());
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

// ── Eval: Phase 16 learned-skill loop closure ─────────────────────────────────

mod skill_loop {
    use super::*;
    use oh_ben_claw::skill_forge::{SkillForge, SkillKind, SkillManifest};

    fn tmp_forge(tag: &str) -> SkillForge {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        SkillForge::new(std::env::temp_dir().join(format!("obc-eval-skills-{tag}-{nanos}")))
    }

    fn delegate_manifest(name: &str, target: &str, fixed: Value, enabled: bool) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: format!("eval skill delegating to {target}"),
            kind: SkillKind::Delegate {
                tool: target.to_string(),
                fixed_args: fixed,
            },
            parameters: json!({ "type": "object", "properties": {} }),
            version: Some("0.1.0-eval".to_string()),
            stage: Default::default(),
            tags: vec!["learned".to_string()],
            enabled,
            timeout_secs: 5,
        }
    }

    /// Golden: sync_skills registers enabled skills, skips disabled ones,
    /// refuses to shadow a built-in, and unregisters skills disabled later.
    #[tokio::test]
    async fn eval_sync_skills_hot_add_remove_and_shadow_guard() {
        let forge = tmp_forge("sync");
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (builtin, _) = echo_tool("http_fetch", false);
        let (agent, _session) = make_agent(provider, vec![builtin]);

        forge
            .install_skill(&delegate_manifest("learned_a", "http_fetch", json!({}), true))
            .unwrap();
        forge
            .install_skill(&delegate_manifest("learned_off", "http_fetch", json!({}), false))
            .unwrap();
        forge
            .install_skill(&delegate_manifest("http_fetch", "http_fetch", json!({}), true))
            .unwrap(); // would shadow the built-in

        let (added, removed, shadowed) = agent.sync_skills(&forge);
        assert_eq!((added, removed, shadowed), (1, 0, 1));
        let names = agent.tool_names();
        assert!(names.contains(&"learned_a".to_string()));
        assert!(!names.contains(&"learned_off".to_string()));
        assert_eq!(names.iter().filter(|n| *n == "http_fetch").count(), 1);

        // Re-sync is idempotent.
        assert_eq!(agent.sync_skills(&forge), (0, 0, 1));

        // Disable on disk → unregistered on next sync (hot removal).
        forge
            .install_skill(&delegate_manifest("learned_a", "http_fetch", json!({}), false))
            .unwrap();
        let (added, removed, _) = agent.sync_skills(&forge);
        assert_eq!((added, removed), (0, 1));
        assert!(!agent.tool_names().contains(&"learned_a".to_string()));

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    /// Golden: a learned Delegate skill invoked by the model routes through the
    /// real underlying tool inside the agent chokepoint — the loop is closed.
    #[tokio::test]
    async fn eval_learned_delegate_skill_executes_underlying_tool() {
        let forge = tmp_forge("route");
        let provider = Arc::new(ScriptedProvider::new(vec![
            ScriptedProvider::tool_call("learned_check_weather", json!({"city": "Oslo"})),
            ScriptedProvider::text("done"),
        ]));
        let (builtin, calls) = echo_tool("http_fetch", false);
        let (agent, session) = make_agent(provider, vec![builtin]);

        forge
            .install_skill(&delegate_manifest(
                "learned_check_weather",
                "http_fetch",
                json!({"q": "weather"}),
                true,
            ))
            .unwrap();
        assert_eq!(agent.sync_skills(&forge).0, 1);

        let resp = agent
            .process(&session, "check the weather", &ProviderConfig::default())
            .await
            .unwrap();

        assert_eq!(resp.message, "done");
        // The underlying tool ran once, with fixed args merged under runtime args.
        let recorded = calls.lock();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains("\"q\":\"weather\""), "fixed arg kept: {}", recorded[0]);
        assert!(recorded[0].contains("\"city\":\"Oslo\""), "runtime arg merged: {}", recorded[0]);
        // And the tool-call record shows a real result, not a delegation stub.
        assert!(resp.tool_calls[0].result.starts_with("echo:"), "{}", resp.tool_calls[0].result);

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    /// Golden: a delegate cycle is cut by the hop bound, not an infinite loop.
    #[tokio::test]
    async fn eval_delegate_cycle_is_bounded() {
        let forge = tmp_forge("cycle");
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (builtin, _) = echo_tool("http_fetch", false);
        let (agent, _session) = make_agent(provider, vec![builtin]);

        forge
            .install_skill(&delegate_manifest("learned_x", "learned_y", json!({}), true))
            .unwrap();
        forge
            .install_skill(&delegate_manifest("learned_y", "learned_x", json!({}), true))
            .unwrap();
        assert_eq!(agent.sync_skills(&forge).0, 2);

        let result = agent
            .execute_tool_direct("learned_x", json!({}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("hops"));

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    fn sequence_manifest(
        name: &str,
        steps: Vec<(&str, Value)>,
        enabled: bool,
    ) -> SkillManifest {
        use oh_ben_claw::skill_forge::SkillStep;
        SkillManifest {
            name: name.to_string(),
            description: "eval sequence skill".to_string(),
            kind: SkillKind::Sequence {
                steps: steps
                    .into_iter()
                    .map(|(tool, args)| SkillStep {
                        tool: tool.to_string(),
                        args,
                    })
                    .collect(),
            },
            parameters: json!({ "type": "object", "properties": {} }),
            version: Some("0.1.0-eval".to_string()),
            stage: Default::default(),
            tags: vec!["learned".to_string()],
            enabled,
            timeout_secs: 5,
        }
    }

    /// Golden: a Sequence skill runs each step through the chokepoint in
    /// order, substituting `{param}` placeholders — numbers stay numbers.
    #[tokio::test]
    async fn eval_sequence_skill_runs_steps_in_order_with_typed_params() {
        let forge = tmp_forge("seq");
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (builtin, calls) = echo_tool("http_fetch", false);
        let (agent, _session) = make_agent(provider, vec![builtin]);

        forge
            .install_skill(&sequence_manifest(
                "learned_report",
                vec![
                    ("http_fetch", json!({"q": "weather", "city": "{city}"})),
                    ("http_fetch", json!({"q": "news", "page": "{page}"})),
                ],
                true,
            ))
            .unwrap();
        assert_eq!(agent.sync_skills(&forge).0, 1);

        let result = agent
            .execute_tool_direct("learned_report", json!({"city": "Oslo", "page": 2}))
            .await
            .unwrap();
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("[step 1 http_fetch]"));
        assert!(result.output.contains("[step 2 http_fetch]"));

        let recorded = calls.lock();
        assert_eq!(recorded.len(), 2);
        assert!(recorded[0].contains("\"city\":\"Oslo\""), "{}", recorded[0]);
        assert!(recorded[1].contains("\"page\":2"), "number preserved: {}", recorded[1]);

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    /// Golden: the first failing step aborts the recipe with a precise error.
    #[tokio::test]
    async fn eval_sequence_aborts_on_first_failing_step() {
        let forge = tmp_forge("seq-abort");
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (good, _) = echo_tool("http_fetch", false);
        let (bad, bad_calls) = echo_tool("flaky_tool", true);
        let (agent, _session) = make_agent(provider, vec![good, bad]);

        forge
            .install_skill(&sequence_manifest(
                "learned_flaky",
                vec![
                    ("flaky_tool", json!({})),
                    ("http_fetch", json!({"q": "never reached"})),
                ],
                true,
            ))
            .unwrap();
        agent.sync_skills(&forge);

        let result = agent
            .execute_tool_direct("learned_flaky", json!({}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("failed at step 1 (flaky_tool)"));
        assert_eq!(bad_calls.lock().len(), 1);

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    /// Golden: sequences cannot nest — bounded recipe depth.
    #[tokio::test]
    async fn eval_nested_sequence_is_refused() {
        let forge = tmp_forge("seq-nest");
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (builtin, _) = echo_tool("http_fetch", false);
        let (agent, _session) = make_agent(provider, vec![builtin]);

        forge
            .install_skill(&sequence_manifest("learned_inner", vec![("http_fetch", json!({}))], true))
            .unwrap();
        forge
            .install_skill(&sequence_manifest("learned_outer", vec![("learned_inner", json!({}))], true))
            .unwrap();
        assert_eq!(agent.sync_skills(&forge).0, 2);

        let result = agent
            .execute_tool_direct("learned_outer", json!({}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap_or("").contains("cannot run inside another sequence"),
            "{:?}",
            result.error
        );

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }

    /// Golden: learned-skill invocations are counted (Phase 16 reuse metric).
    #[tokio::test]
    async fn eval_learned_skill_invocation_counter() {
        use oh_ben_claw::observability::ObsContext;

        let forge = tmp_forge("metric");
        let obs = Arc::new(ObsContext::new());
        let provider = Arc::new(ScriptedProvider::new(vec![
            ScriptedProvider::tool_call("learned_ping", json!({})),
            ScriptedProvider::text("ok"),
        ]));
        let (builtin, _) = echo_tool("http_fetch", false);
        let (agent, session) = make_agent(provider, vec![builtin]);
        let agent = agent.with_obs(obs.clone());

        forge
            .install_skill(&delegate_manifest("learned_ping", "http_fetch", json!({}), true))
            .unwrap();
        agent.sync_skills(&forge);

        agent
            .process(&session, "ping", &ProviderConfig::default())
            .await
            .unwrap();

        assert_eq!(obs.metrics.counter("learned_skill_invocations_total").get(), 1);

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }
}

// ── Eval: Phase 16 P3 staged rollout (Track 0 red-team) ──────────────────────

mod staged_rollout {
    use super::*;
    use oh_ben_claw::approval::{ApprovalManager, ForeverGrants};
    use oh_ben_claw::skill_forge::rollout::tracker_in;
    use oh_ben_claw::skill_forge::{SkillForge, SkillKind, SkillManifest};
    use oh_ben_claw::tools::traits::RolloutStage;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("obc-eval-rollout-{tag}-{nanos}"))
    }

    fn staged_manifest(name: &str, target: &str, stage: RolloutStage) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: "physical learned skill".to_string(),
            kind: SkillKind::Delegate {
                tool: target.to_string(),
                fixed_args: json!({"pin": 17, "value": 1}),
            },
            parameters: json!({ "type": "object", "properties": {} }),
            version: Some("0.1.0-learned".to_string()),
            stage,
            tags: vec!["learned".to_string(), "track0:supervised".to_string()],
            enabled: true,
            timeout_secs: 5,
        }
    }

    fn approval(level: AutonomyLevel, auto_approve: Vec<String>) -> Arc<ApprovalManager> {
        let cfg = AutonomyConfig {
            level,
            auto_approve,
            always_ask: vec![],
        };
        let grants = std::env::temp_dir().join(format!(
            "obc-eval-rollout-grants-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Arc::new(ApprovalManager::with_grants(&cfg, ForeverGrants::load(grants), true))
    }

    /// RED TEAM: even if the model calls a simulate-stage actuator skill, the
    /// actuator is never touched — the run is a dry-run, counted toward
    /// promotion, with an auditable description of what would have happened.
    #[tokio::test]
    async fn eval_redteam_simulate_stage_skill_never_actuates() {
        let dir = tmp_dir("simulate");
        let forge = SkillForge::new(&dir);
        forge
            .install_skill(&staged_manifest("learned_unlock", "gpio_write", RolloutStage::Simulate))
            .unwrap();
        let tracker = Arc::new(tracker_in(&dir));

        let provider = Arc::new(ScriptedProvider::new(vec![
            ScriptedProvider::tool_call("learned_unlock", json!({})),
            ScriptedProvider::text("done"),
        ]));
        let (actuator, actuator_calls) = echo_tool("gpio_write", false);
        let (agent, session) = make_agent(provider, vec![actuator]);
        let agent = agent.with_rollout(Arc::clone(&tracker));
        agent.sync_skills(&forge);

        let resp = agent
            .process(&session, "unlock the door", &ProviderConfig::default())
            .await
            .unwrap();

        assert!(actuator_calls.lock().is_empty(), "the actuator must never fire");
        assert!(resp.tool_calls[0].result.contains("SIMULATION"), "{}", resp.tool_calls[0].result);
        assert!(resp.tool_calls[0].result.contains("gpio_write"), "auditable description");
        let rec = tracker.record("learned_unlock").unwrap();
        assert_eq!((rec.stage, rec.clean_runs), (RolloutStage::Simulate, 1));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// RED TEAM: a supervised-stage skill is refused without an explicit
    /// operator grant — Full autonomy is NOT a grant, and no approval manager
    /// at all fails closed.
    #[tokio::test]
    async fn eval_redteam_supervised_skill_refused_without_explicit_grant() {
        let dir = tmp_dir("supervised-refuse");
        let forge = SkillForge::new(&dir);
        forge
            .install_skill(&staged_manifest("learned_unlock", "gpio_write", RolloutStage::Supervised))
            .unwrap();

        // Case 1: no approval manager attached → fail closed.
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (actuator, calls) = echo_tool("gpio_write", false);
        let (agent, _s) = make_agent(provider, vec![actuator]);
        agent.sync_skills(&forge);
        let r = agent.execute_tool_direct("learned_unlock", json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap_or("").contains("explicit operator grant"));
        assert!(calls.lock().is_empty());

        // Case 2: Full autonomy, but no explicit grant → still refused.
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (actuator, calls) = echo_tool("gpio_write", false);
        let (agent, _s) = make_agent(provider, vec![actuator]);
        let agent = agent.with_approval(approval(AutonomyLevel::Full, vec![]));
        agent.sync_skills(&forge);
        let r = agent.execute_tool_direct("learned_unlock", json!({})).await.unwrap();
        assert!(!r.success, "Full autonomy must not count as an operator grant");
        assert!(calls.lock().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Golden: with an explicit grant a supervised skill executes for real,
    /// and the clean run is recorded toward promotion.
    #[tokio::test]
    async fn eval_supervised_skill_runs_with_grant_and_records_clean() {
        let dir = tmp_dir("supervised-run");
        let forge = SkillForge::new(&dir);
        forge
            .install_skill(&staged_manifest("learned_unlock", "gpio_write", RolloutStage::Supervised))
            .unwrap();
        let tracker = Arc::new(tracker_in(&dir));

        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (actuator, calls) = echo_tool("gpio_write", false);
        let (agent, _s) = make_agent(provider, vec![actuator]);
        let agent = agent
            .with_approval(approval(AutonomyLevel::Full, vec!["learned_unlock".to_string()]))
            .with_rollout(Arc::clone(&tracker));
        agent.sync_skills(&forge);

        let r = agent.execute_tool_direct("learned_unlock", json!({})).await.unwrap();
        assert!(r.success, "{:?}", r.error);
        assert_eq!(calls.lock().len(), 1, "the real actuator ran exactly once");
        let rec = tracker.record("learned_unlock").unwrap();
        assert_eq!((rec.stage, rec.clean_runs, rec.failures), (RolloutStage::Supervised, 1, 0));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Golden: a supervised skill that fails a real run is auto-demoted to
    /// simulate — the next invocation is a dry-run (halt on drift).
    #[tokio::test]
    async fn eval_supervised_failure_auto_demotes_to_simulate() {
        let dir = tmp_dir("supervised-demote");
        let forge = SkillForge::new(&dir);
        forge
            .install_skill(&staged_manifest("learned_flaky", "gpio_write", RolloutStage::Supervised))
            .unwrap();
        let tracker = Arc::new(tracker_in(&dir));

        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let (actuator, calls) = echo_tool("gpio_write", true); // fails
        let (agent, _s) = make_agent(provider, vec![actuator]);
        let agent = agent
            .with_approval(approval(AutonomyLevel::Full, vec!["learned_flaky".to_string()]))
            .with_rollout(Arc::clone(&tracker))
            .with_forge_dir(&dir);
        agent.sync_skills(&forge);

        let r = agent.execute_tool_direct("learned_flaky", json!({})).await.unwrap();
        assert!(!r.success);
        assert_eq!(calls.lock().len(), 1);

        // Manifest demoted on disk…
        let m = forge
            .list_manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "learned_flaky")
            .unwrap();
        assert_eq!(m.stage, RolloutStage::Simulate, "auto-demoted after real-run failure");

        // …and the live registry was resynced: the next call only simulates.
        let r2 = agent.execute_tool_direct("learned_flaky", json!({})).await.unwrap();
        assert!(r2.success);
        assert!(r2.output.contains("SIMULATION"));
        assert_eq!(calls.lock().len(), 1, "actuator not touched again");

        std::fs::remove_dir_all(&dir).ok();
    }
}

// ── Eval: Phase 16 P1 experience retrieval ────────────────────────────────────

mod experience {
    use super::*;
    use oh_ben_claw::memory::trajectory::{Episode, EpisodeStep, Outcome, TrajectoryStore};

    fn seeded_store(objective: &str, tool: &str) -> Arc<TrajectoryStore> {
        let store = TrajectoryStore::open_in_memory().unwrap();
        store
            .record(&Episode {
                id: "past-1".to_string(),
                session_id: "old".to_string(),
                objective: objective.to_string(),
                steps: vec![EpisodeStep {
                    tool: tool.to_string(),
                    args: json!({"q": "weather"}),
                    result: "ok".to_string(),
                    ok: true,
                }],
                outcome: Outcome::Success,
                ts_ms: 1,
                duration_ms: None,
                tokens_est: None,
            })
            .unwrap();
        Arc::new(store)
    }

    /// Golden: a similar past success is surfaced as a system block containing
    /// the proven recipe, inserted right after the system prompt.
    #[tokio::test]
    async fn eval_experience_block_surfaces_similar_past_success() {
        let provider = Arc::new(ScriptedProvider::new(vec![ScriptedProvider::text("done")]));
        let (tool, _) = echo_tool("http_fetch", false);
        let (agent, session) = make_agent(provider.clone(), vec![tool]);
        let agent = agent
            .with_trajectory_store(seeded_store("check the weather", "http_fetch"))
            .with_experience_retrieval(3);

        agent
            .process(&session, "check the weather in Oslo", &ProviderConfig::default())
            .await
            .unwrap();

        let seen = provider.messages_seen.lock();
        let first_call = &seen[0];
        // Block is the second message (right after the system prompt).
        assert!(first_call[1].starts_with("[Learned experience"), "{}", first_call[1]);
        assert!(first_call[1].contains("\"check the weather\""));
        assert!(first_call[1].contains("http_fetch"));
    }

    /// Golden: novel tasks get no block — zero prompt noise, no counter tick.
    #[tokio::test]
    async fn eval_no_experience_block_on_novel_task() {
        use oh_ben_claw::observability::ObsContext;

        let obs = Arc::new(ObsContext::new());
        let provider = Arc::new(ScriptedProvider::new(vec![ScriptedProvider::text("hi")]));
        let (tool, _) = echo_tool("http_fetch", false);
        let (agent, session) = make_agent(provider.clone(), vec![tool]);
        let agent = agent
            .with_trajectory_store(seeded_store("water the tomato plants", "http_fetch"))
            .with_experience_retrieval(3)
            .with_obs(obs.clone());

        agent
            .process(&session, "photograph incoming birds", &ProviderConfig::default())
            .await
            .unwrap();

        let seen = provider.messages_seen.lock();
        assert!(
            seen[0].iter().all(|m| !m.contains("[Learned experience")),
            "novel task must not get an experience block"
        );
        assert_eq!(obs.metrics.counter("experience_blocks_injected_total").get(), 0);
    }

    /// Golden: a registered learned skill relevant to the task is recommended
    /// in the block by name (and the injection counter ticks).
    #[tokio::test]
    async fn eval_experience_block_recommends_learned_skill() {
        use oh_ben_claw::observability::ObsContext;
        use oh_ben_claw::skill_forge::{SkillForge, SkillKind, SkillManifest};

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let forge =
            SkillForge::new(std::env::temp_dir().join(format!("obc-eval-exp-{nanos}")));
        forge
            .install_skill(&SkillManifest {
                name: "learned_check_the_weather".to_string(),
                description: "Learned from a successful run: check the weather".to_string(),
                kind: SkillKind::Delegate {
                    tool: "http_fetch".to_string(),
                    fixed_args: json!({"q": "weather"}),
                },
                parameters: json!({ "type": "object", "properties": {} }),
                version: Some("0.1.0-learned".to_string()),
                stage: Default::default(),
                tags: vec!["learned".to_string()],
                enabled: true,
                timeout_secs: 5,
            })
            .unwrap();

        let obs = Arc::new(ObsContext::new());
        let provider = Arc::new(ScriptedProvider::new(vec![ScriptedProvider::text("done")]));
        let (tool, _) = echo_tool("http_fetch", false);
        let (agent, session) = make_agent(provider.clone(), vec![tool]);
        let agent = agent.with_experience_retrieval(3).with_obs(obs.clone());
        agent.sync_skills(&forge);

        agent
            .process(&session, "check the weather", &ProviderConfig::default())
            .await
            .unwrap();

        let seen = provider.messages_seen.lock();
        assert!(
            seen[0][1].contains("learned_check_the_weather"),
            "learned skill recommended by name: {}",
            seen[0][1]
        );
        assert_eq!(obs.metrics.counter("experience_blocks_injected_total").get(), 1);

        std::fs::remove_dir_all(&forge.skill_dir).ok();
    }
}

// ── Eval: LLM-as-judge advisory scoring (Phase 15 WS4) ───────────────────────

/// Advisory only, per the WS4 rule: gates stay deterministic. Without an
/// `OBC_JUDGE_PROVIDER`/`OBC_JUDGE_MODEL` environment, the eval skips
/// cleanly; with one, it runs the judge over a golden transcript, prints the
/// score, and asserts nothing beyond "a score parsed into [0, 1]".
#[tokio::test]
async fn eval_llm_judge_advisory_scoring() {
    use oh_ben_claw::agent::judge::LlmJudge;

    let Some(judge) = LlmJudge::from_env() else {
        eprintln!(
            "advisory: LLM judge not configured (set OBC_JUDGE_PROVIDER / OBC_JUDGE_MODEL); \
             skipping — deterministic gates are unaffected"
        );
        return;
    };

    // Golden transcript from the routing evals: direct-answer arithmetic.
    let score = judge
        .score("what is 2+2?", "The answer is 4.")
        .await
        .expect("judge call failed");
    eprintln!(
        "advisory judge score: {:.2} — {}",
        score.score,
        score.rationale.lines().next().unwrap_or("")
    );
    // Parse-sanity only; the score value itself never gates.
    assert!((0.0..=1.0).contains(&score.score));
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

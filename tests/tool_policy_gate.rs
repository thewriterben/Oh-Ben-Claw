//! The tool-execution policy engine, at the point it actually blocks something.
//!
//! `PolicyEngine` is consulted in `Agent::execute_tool` on **every tool call, at
//! every hop** — so it re-evaluates after skill delegation, not just on the name
//! the model first said. It supports glob tool patterns, an optional substring
//! match on the serialised arguments, and allow / deny / audit. It has unit tests.
//! It is configurable from `[[security.policies]]`.
//!
//! And until 2026-07-30 it was documented in exactly zero places — not in
//! `config.example.toml`, not in the README — and it shipped with no rules, so
//! `policy_count()` was 0 on every deployment and startup said nothing about it.
//! A working security control nobody could discover is the inverse of the problem
//! the rest of this audit found, and it fails just as quietly.
//!
//! Its unit tests check that `evaluate()` returns `Deny`. That is not the property
//! that matters. The property that matters is that a denied tool **does not run**,
//! which is a fact about the agent loop rather than about the engine, so this
//! suite drives real `Agent`s over spy tools that record every execution.
//!
//! Every "it did not run" assertion is paired with the same tool running when
//! policy permits it. An un-executed tool is trivially easy to achieve by
//! accident — misspell the name and the call fails before policy is consulted —
//! and this file was written a day after three tests in a sibling suite passed on
//! `[] == []`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oh_ben_claw::agent::Agent;
use oh_ben_claw::config::{AgentConfig, ProviderConfig};
use oh_ben_claw::memory::MemoryStore;
use oh_ben_claw::providers::{ChatCompletion, ChatMessage, Provider};
use oh_ben_claw::security::policy::{self, PolicyEngine, ToolPolicy, ToolPolicyAction};
use oh_ben_claw::tools::traits::{Tool, ToolResult};
use serde_json::{json, Value};

/// Records every execution. If a policy is doing its job, this stays empty.
struct SpyTool {
    tool_name: String,
    calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Tool for SpyTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        "records that it ran"
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "action": { "type": "string" } } })
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        self.calls.lock().unwrap().push(args.to_string());
        Ok(ToolResult::ok("ran"))
    }
}

fn spy(name: &str) -> (Box<dyn Tool>, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Box::new(SpyTool {
            tool_name: name.to_string(),
            calls: Arc::clone(&calls),
        }),
        calls,
    )
}

/// The agent loop needs a provider to exist; these tests never reach it, because
/// `execute_tool_direct` runs one tool call rather than a conversation.
struct UnusedProvider;

#[async_trait]
impl Provider for UnusedProvider {
    fn name(&self) -> &str {
        "unused"
    }
    async fn chat_completion(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        _config: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        panic!("these tests must not reach the model")
    }
}

fn agent_with(tools: Vec<Box<dyn Tool>>, policies: Vec<ToolPolicy>) -> Agent {
    let config = AgentConfig {
        name: "policy-gate-test".to_string(),
        system_prompt: "test".to_string(),
        max_tool_iterations: 2,
    };
    let memory = Arc::new(MemoryStore::open_in_memory().expect("in-memory store"));
    Agent::new(config, Arc::new(UnusedProvider), memory, tools)
        .with_policy(PolicyEngine::new(policies))
}

fn deny(name: &str, pattern: &str) -> ToolPolicy {
    ToolPolicy {
        name: name.to_string(),
        tool_pattern: pattern.to_string(),
        arg_contains: None,
        action: ToolPolicyAction::Deny,
        reason: Some("blocked by test".to_string()),
    }
}

#[tokio::test]
async fn a_denied_tool_does_not_execute() {
    // Control first: with no policy the spy runs, so "it did not run" below is a
    // statement about the policy rather than about a broken fixture.
    let (tool, ran) = spy("dangerous");
    let out = agent_with(vec![tool], vec![])
        .execute_tool_direct("dangerous", json!({ "action": "go" }))
        .await
        .expect("tool call should complete");
    assert!(out.success, "control run failed: {out:?}");
    assert_eq!(ran.lock().unwrap().len(), 1, "control: the spy should run");

    // Same tool, same call, one deny rule.
    let (tool, ran) = spy("dangerous");
    let out = agent_with(vec![tool], vec![deny("no-dangerous", "dangerous")])
        .execute_tool_direct("dangerous", json!({ "action": "go" }))
        .await
        .expect("a denied call is a failed result, not a transport error");

    assert!(!out.success, "a denied tool call reported success");
    assert!(
        ran.lock().unwrap().is_empty(),
        "the tool executed despite being denied — the policy annotated the result \
         instead of preventing the action, which is the whole difference"
    );
    let msg = format!("{out:?}");
    assert!(
        msg.contains("no-dangerous"),
        "the refusal should name the policy that caused it, got: {msg}"
    );
}

#[tokio::test]
async fn glob_patterns_and_first_match_wins() {
    // `browser_*` denies the family; the earlier, narrower allow wins for one of
    // them, because policies are evaluated in order and the first match decides.
    let (click, click_ran) = spy("browser_click");
    let (nav, nav_ran) = spy("browser_navigate");

    let policies = vec![
        ToolPolicy {
            name: "navigation-is-fine".to_string(),
            tool_pattern: "browser_navigate".to_string(),
            arg_contains: None,
            action: ToolPolicyAction::Allow,
            reason: None,
        },
        deny("no-browser", "browser_*"),
    ];

    let agent = agent_with(vec![click, nav], policies);

    let denied = agent
        .execute_tool_direct("browser_click", json!({}))
        .await
        .unwrap();
    assert!(!denied.success);
    assert!(
        click_ran.lock().unwrap().is_empty(),
        "glob deny did not block"
    );

    let allowed = agent
        .execute_tool_direct("browser_navigate", json!({}))
        .await
        .unwrap();
    assert!(
        allowed.success,
        "the earlier allow rule should win over the later glob deny: {allowed:?}"
    );
    assert_eq!(
        nav_ran.lock().unwrap().len(),
        1,
        "first-match-wins did not hold; the broad deny swallowed the specific allow"
    );
}

#[tokio::test]
async fn arg_contains_narrows_a_deny_to_one_shape_of_call() {
    // Deny `file` only when the arguments mention delete. The same tool must stay
    // usable for reads — a rule that blocks the whole tool would be a different,
    // much blunter policy, and the point of arg_contains is that it need not be.
    let policies = vec![ToolPolicy {
        name: "no-file-delete".to_string(),
        tool_pattern: "file".to_string(),
        arg_contains: Some("delete".to_string()),
        action: ToolPolicyAction::Deny,
        reason: Some("irreversible".to_string()),
    }];

    let (tool, ran) = spy("file");
    let agent = agent_with(vec![tool], policies.clone());

    let blocked = agent
        .execute_tool_direct("file", json!({ "action": "delete", "path": "/tmp/x" }))
        .await
        .unwrap();
    assert!(!blocked.success);
    assert!(ran.lock().unwrap().is_empty(), "the delete was not blocked");

    let allowed = agent
        .execute_tool_direct("file", json!({ "action": "read", "path": "/tmp/x" }))
        .await
        .unwrap();
    assert!(allowed.success, "reads should still work: {allowed:?}");
    assert_eq!(
        ran.lock().unwrap().len(),
        1,
        "the read did not run, so this test cannot distinguish a narrow deny from \
         a total one"
    );
}

#[tokio::test]
async fn the_recommended_baseline_blocks_the_shell_and_nothing_ordinary() {
    let baseline = policy::baseline();
    assert!(
        !baseline.is_empty(),
        "baseline() is empty, so the assertions below check nothing"
    );

    let (shell, shell_ran) = spy("shell");
    let (world, world_ran) = spy("world_memory");
    let agent = agent_with(vec![shell, world], baseline);

    let blocked = agent
        .execute_tool_direct("shell", json!({ "command": "echo hi" }))
        .await
        .unwrap();
    assert!(!blocked.success, "the baseline permitted a shell call");
    assert!(
        shell_ran.lock().unwrap().is_empty(),
        "the baseline let the shell run"
    );

    // The baseline is a starting point, not a lockdown: a tool it says nothing
    // about is still permitted, because the engine defaults to allow on no match.
    let ok = agent
        .execute_tool_direct("world_memory", json!({ "action": "list" }))
        .await
        .unwrap();
    assert!(
        ok.success,
        "the baseline blocked an ordinary tool it names nowhere: {ok:?}"
    );
    assert_eq!(world_ran.lock().unwrap().len(), 1);
}

/// Audit must allow. It is the rule people reach for when they want visibility
/// without breakage, and a mistake here turns an observability choice into an
/// outage.
#[tokio::test]
async fn audit_logs_without_blocking() {
    let policies = vec![ToolPolicy {
        name: "watch-http".to_string(),
        tool_pattern: "http".to_string(),
        arg_contains: None,
        action: ToolPolicyAction::Audit,
        reason: Some("egress".to_string()),
    }];

    let (tool, ran) = spy("http");
    let out = agent_with(vec![tool], policies)
        .execute_tool_direct("http", json!({ "url": "https://example.invalid" }))
        .await
        .unwrap();

    assert!(out.success, "audit blocked a call: {out:?}");
    assert_eq!(
        ran.lock().unwrap().len(),
        1,
        "audit must allow the call through; only deny stops it"
    );
}

/// The commented example in `config.example.toml` must parse, and must be the
/// baseline it claims to be.
///
/// Documented configuration is the most reliably wrong part of any project: it is
/// prose until someone pastes it, and by then they are debugging rather than
/// reading. This repo has already shipped a `[[safety.limit]]` that silently did
/// nothing because the real key was `[[safety.limits]]`, and a generator config
/// with two keys the agent does not have. So the example is uncommented, fed to
/// the real `Config` deserialiser, and compared against `policy::baseline()`.
///
/// If this fails, the example and the code have diverged — fix whichever is wrong,
/// and do not delete the test to make the diff smaller.
#[test]
fn the_documented_example_parses_and_matches_the_baseline() {
    let example = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config.example.toml"
    ))
    .expect("config.example.toml");

    let start = example
        .find("# ── Tool execution policy")
        .expect("the policy section is missing from config.example.toml");
    let section = &example[start..];
    let end = section
        .find("# ── Gateway")
        .expect("could not find the end of the policy section");

    // Uncomment only the TOML lines: `# [[...]]` and `# key = value`.
    let uncommented: String = section[..end]
        .lines()
        .filter_map(|l| l.strip_prefix("# "))
        .filter(|l| l.starts_with("[[") || (l.contains(" = ") && !l.starts_with(' ')))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        uncommented.contains("[[security.policies]]"),
        "extracted nothing from the example; the comment format changed:\n{uncommented}"
    );

    let parsed: oh_ben_claw::config::Config =
        toml::from_str(&uncommented).expect("the documented example does not parse");

    let documented = &parsed.security.policies;
    let baseline = policy::baseline();
    assert_eq!(
        documented.len(),
        baseline.len(),
        "example has {} policies, baseline() has {}",
        documented.len(),
        baseline.len()
    );
    for (doc, base) in documented.iter().zip(baseline.iter()) {
        assert_eq!(doc.name, base.name, "policy order or naming diverged");
        assert_eq!(doc.tool_pattern, base.tool_pattern, "{}: pattern", doc.name);
        assert_eq!(doc.arg_contains, base.arg_contains, "{}: arg_contains", doc.name);
        assert_eq!(doc.action, base.action, "{}: action", doc.name);
    }
}

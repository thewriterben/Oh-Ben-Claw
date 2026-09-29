//! `[agent.tools]` end to end (2026-09-28): a shelved tool is listed in the
//! system prompt, loaded on request with `load_tools`, callable from the next
//! step, remembered for the session, and invisible to other sessions. With no
//! `full` list, nothing changes.

use anyhow::Result;
use obc_agent::tool_prefix::{ToolPrefixConfig, LOAD_TOOLS};
use obc_agent::{Agent, AgentConfig};
use obc_memory::MemoryStore;
use obc_providers::{ChatCompletion, ChatMessage, ProviderConfig, ToolCall};
use obc_tool_api::{Tool, ToolResult};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct Dummy(&'static str);

#[async_trait::async_trait]
impl Tool for Dummy {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "A bench tool. Its second sentence never reaches the shelf."
    }
    async fn execute(&self, _args: Value) -> Result<ToolResult> {
        Ok(ToolResult::ok(format!("{} ran", self.0)))
    }
}

/// One call log entry: the tool names offered and the system message.
#[derive(Clone)]
struct Seen {
    tools: Vec<String>,
    system: String,
}

/// Call 1 asks for a shelved tool (and a typo); call 2 uses it; every other
/// call answers with text.
struct Scripted {
    seen: Mutex<Vec<Seen>>,
}

#[async_trait::async_trait]
impl obc_providers::Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn chat_completion(
        &self,
        m: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        c: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        let n = {
            let mut s = self.seen.lock().unwrap();
            s.push(Seen {
                tools: names.clone(),
                system: m.first().map(|x| x.content.clone()).unwrap_or_default(),
            });
            s.len()
        };
        let done = |message: &str, tool_calls: Vec<ToolCall>| ChatCompletion {
            message: message.to_string(),
            tool_calls,
            provider: "scripted".into(),
            model: c.model.clone(),
            usage: None,
        };
        Ok(match n {
            1 => done(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: LOAD_TOOLS.into(),
                    args: r#"{"names":["gnss_fix","nope"]}"#.into(),
                }],
            ),
            2 => {
                assert!(
                    names.iter().any(|x| x == "gnss_fix"),
                    "the loaded schema is offered on the very next step"
                );
                done(
                    "",
                    vec![ToolCall {
                        id: "c2".into(),
                        name: "gnss_fix".into(),
                        args: "{}".into(),
                    }],
                )
            }
            _ => done("done", vec![]),
        })
    }
}

fn agent_with(cfg: ToolPrefixConfig) -> (Agent, Arc<MemoryStore>, Arc<Scripted>) {
    let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
    let provider = Arc::new(Scripted {
        seen: Mutex::new(Vec::new()),
    });
    let agent = Agent::new(
        AgentConfig {
            tools: cfg,
            ..AgentConfig::default()
        },
        provider.clone(),
        Arc::clone(&memory),
        vec![
            Box::new(Dummy("shell")),
            Box::new(Dummy("gnss_fix")),
            Box::new(Dummy("ota_update")),
        ],
    );
    (agent, memory, provider)
}

#[tokio::test]
async fn a_shelved_tool_is_loaded_on_request_and_stays_for_the_session() {
    let (agent, memory, provider) = agent_with(ToolPrefixConfig {
        full: vec!["shell".into()],
        catalog: true,
    });
    let pc = ProviderConfig::default();
    let session = memory.create_session("console").unwrap();

    let r = agent
        .process(&session, "where is node 3", &pc)
        .await
        .unwrap();
    assert_eq!(r.message, "done");
    let calls: Vec<&str> = r.tool_calls.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(calls, [LOAD_TOOLS, "gnss_fix"]);
    assert!(
        r.tool_calls[0].result.starts_with("Loaded: gnss_fix."),
        "{}",
        r.tool_calls[0].result
    );
    assert!(r.tool_calls[0].result.contains("Not a tool here: nope."));
    assert_eq!(r.tool_calls[1].result, "gnss_fix ran");

    let seen = provider.seen.lock().unwrap().clone();
    assert_eq!(
        seen[0].tools,
        ["shell", LOAD_TOOLS],
        "the first step sees only the full list"
    );
    assert_eq!(
        seen[1].tools,
        ["shell", "gnss_fix", LOAD_TOOLS],
        "the next step sees the loaded one"
    );
    assert_eq!(
        seen[0].system, seen[1].system,
        "loading changes the tools array, not the system message"
    );
    let system = &seen[0].system;
    assert!(system.contains("## Tools on the shelf"), "{system}");
    assert!(system.contains("- `gnss_fix` — A bench tool."));
    assert!(system.contains("- `ota_update` — A bench tool."));
    assert!(
        !system.contains("- `shell`"),
        "a full tool is not on the shelf"
    );
    assert!(!system.contains("second sentence"));

    // The same session keeps it on the next turn …
    agent.process(&session, "again", &pc).await.unwrap();
    let seen = provider.seen.lock().unwrap().clone();
    assert_eq!(seen[3].tools, ["shell", "gnss_fix", LOAD_TOOLS]);

    // … and another session starts from the configured list.
    let other = memory.create_session("other").unwrap();
    agent.process(&other, "hello", &pc).await.unwrap();
    let seen = provider.seen.lock().unwrap().clone();
    assert_eq!(seen[4].tools, ["shell", LOAD_TOOLS]);
}

#[tokio::test]
async fn with_no_full_list_every_schema_is_in_the_prompt_as_before() {
    let (agent, memory, provider) = agent_with(ToolPrefixConfig::default());
    let session = memory.create_session("console").unwrap();
    // Call 1 of the script asks for `load_tools`, which is not offered here; the
    // chokepoint answers it like any unknown tool and the turn still completes.
    let r = agent
        .process(&session, "hello", &ProviderConfig::default())
        .await
        .unwrap();
    assert_eq!(r.message, "done");
    let seen = provider.seen.lock().unwrap().clone();
    assert_eq!(seen[0].tools, ["shell", "gnss_fix", "ota_update"]);
    assert!(!seen[0].system.contains("Tools on the shelf"));
}

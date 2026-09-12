//! Anthropic provider adapter.
//!
//! Streams since 2026-09-11. `chat_completion_streaming` sends `"stream": true`
//! and folds the Messages API's server-sent events: `text_delta`s go to the
//! sink as they arrive, `tool_use` blocks accumulate their `input_json_delta`
//! fragments into one arguments string, and `message_stop` closes the
//! completion. The fold is a pure function over decoded events
//! ([`fold_event`]) so the wire format is tested without a network.
//!
//! Prompt caching since 2026-09-11 (`ProviderConfig::prompt_caching`): three
//! `cache_control` breakpoints — the system prompt, the last tool definition,
//! and the last history message before the agent's ephemeral blocks. The
//! agent marks that boundary by convention: the first `System`-role message
//! after the leading one is ephemeral (world state, experience), so the
//! message before it is the end of the stable prefix. Cache hits bill at 10%
//! of input; the stable prefix here is ~5k tokens of tool schemas.

use crate::ProviderConfig;
use crate::{
    ChatCompletion, ChatMessage, ChatRole, DeltaSink, Provider, ResponseFormat, StreamDelta,
    ToolCall,
};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use obc_tools::Tool;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Anthropic provider.
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    client: Client,
}

impl AnthropicProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn api_key(config: &ProviderConfig) -> Result<String> {
        config
            .api_key
            .as_ref()
            .map(|k| k.expose().to_string())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| anyhow::anyhow!("ANTHROPIC_API_KEY not set"))
    }

    /// The request body both paths send; only `stream` differs.
    pub(crate) fn build_request(
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
        stream: bool,
    ) -> Result<(String, Value)> {
        let url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());

        // Separate system prompt from conversation messages
        let (system_prompt, conversation) = if let Some(first) = messages.first() {
            if first.role == ChatRole::System {
                (Some(first.content.clone()), &messages[1..])
            } else {
                (None, messages)
            }
        } else {
            (None, messages)
        };

        // The end of the stable prefix: the message before the first ephemeral
        // (System-role, non-leading) block. `None` when the prompt has no
        // ephemeral tail, in which case the whole message list is stable and
        // the rolling breakpoint is the last message.
        let boundary = conversation
            .iter()
            .position(|m| m.role == ChatRole::System)
            .and_then(|i| i.checked_sub(1));
        let caching = config.prompt_caching;

        let anth_messages: Vec<AnthropicMessage> = conversation
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let role = match m.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    // Ephemeral blocks ride as user content; Anthropic has one
                    // system slot and it is spent on the stable prompt.
                    ChatRole::System => "user",
                };
                let mark = caching
                    && match boundary {
                        Some(b) => i == b,
                        None => i + 1 == conversation.len(),
                    };
                AnthropicMessage {
                    role: role.into(),
                    content: if mark {
                        serde_json::json!([{
                            "type": "text",
                            "text": m.content,
                            "cache_control": {"type": "ephemeral"}
                        }])
                    } else {
                        Value::String(m.content.clone())
                    },
                }
            })
            .collect();

        // One tool with a bad name used to fail the whole request: the bench's
        // 208-character learned skill drew `tools.30.custom.name: String should
        // have at most 128 characters` on every cloud turn (2026-09-11). Skip
        // such tools, loudly, and send the rest.
        let mut kept: Vec<AnthropicTool> = Vec::with_capacity(tools.len());
        for t in tools {
            if !valid_tool_name(t.name()) {
                tracing::warn!(
                    tool = %t.name().chars().take(80).collect::<String>(),
                    len = t.name().len(),
                    "anthropic: tool left out of the request, its name breaks the API rule \
                     (1-128 chars of A-Z a-z 0-9 _ -); the request would otherwise be refused"
                );
                continue;
            }
            kept.push(AnthropicTool {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.parameters_schema(),
            });
        }
        let anth_tools: Option<Vec<AnthropicTool>> =
            if kept.is_empty() { None } else { Some(kept) };

        // No `temperature`: current models (Sonnet 5, Opus 4.7 and later) refuse
        // it with `temperature is deprecated for this model`, and the default is
        // what we want anyway. `ProviderConfig::temperature` still applies to
        // the other providers.
        let mut body = serde_json::json!({
            "model": config.model,
            "messages": anth_messages,
            "max_tokens": 4096,
        });
        // `think`: `false` turns thinking off (accepted on Sonnet 5 and the 4.6+
        // family; the agent does not replay thinking blocks, so this is the
        // setting to use for a tool-using brain), `true` asks for adaptive
        // thinking, unset leaves the model's default.
        match config.think {
            Some(false) => body["thinking"] = serde_json::json!({"type": "disabled"}),
            Some(true) => body["thinking"] = serde_json::json!({"type": "adaptive"}),
            None => {}
        }
        if stream {
            body["stream"] = Value::Bool(true);
        }

        if let Some(sys) = system_prompt {
            body["system"] = Value::String(sys);
        }
        // Caching breakpoint 1: the system prompt as a block. The JSON-mode
        // suffixes below append to a string, so they run against the string
        // form and the block form is produced last.

        // Anthropic does not have a native `response_format` field. We emulate
        // JSON mode by appending an instruction to the system prompt and, for
        // structured schemas, including the schema definition.
        if let Some(ref fmt) = config.response_format {
            match fmt {
                ResponseFormat::Text => {} // default — nothing to do
                ResponseFormat::JsonObject => {
                    let existing = body
                        .get("system")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let suffix =
                        "\n\nYou must respond with valid JSON only. No markdown, no explanation.";
                    body["system"] = Value::String(format!("{existing}{suffix}"));
                }
                ResponseFormat::JsonSchema {
                    name,
                    schema,
                    strict: _,
                } => {
                    let existing = body
                        .get("system")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let suffix = format!(
                        "\n\nYou must respond with valid JSON that conforms to the \
                         following JSON schema named \"{name}\":\n{schema}\n\
                         Output only the JSON object. No markdown, no explanation."
                    );
                    body["system"] = Value::String(format!("{existing}{suffix}"));
                }
            }
        }

        if let Some(t) = anth_tools {
            let mut tools = serde_json::to_value(t)?;
            if caching {
                // Breakpoint 2: the last tool definition, so every schema before
                // it is in the cached prefix.
                if let Some(last) = tools.as_array_mut().and_then(|a| a.last_mut()) {
                    last["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
            }
            body["tools"] = tools;
            body["tool_choice"] = serde_json::json!({"type": "auto"});
        }
        if caching {
            if let Some(sys) = body
                .get("system")
                .and_then(|v| v.as_str())
                .map(String::from)
            {
                body["system"] = serde_json::json!([{
                    "type": "text",
                    "text": sys,
                    "cache_control": {"type": "ephemeral"}
                }]);
            }
        }
        Ok((url, body))
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let api_key = Self::api_key(config)?;
        let (url, body) = Self::build_request(messages, tools, config, false)?;

        // Surface API errors instead of force-parsing them (see openai.rs).
        let http_response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        let status = http_response.status();
        let body_text = http_response.text().await?;
        if !status.is_success() {
            anyhow::bail!("Anthropic API error ({status}): {body_text}");
        }
        let response: AnthropicResponse = serde_json::from_str(&body_text).map_err(|e| {
            anyhow::anyhow!(
                "unexpected Anthropic response shape ({e}): {}",
                body_text.chars().take(300).collect::<String>()
            )
        })?;

        let mut message = String::new();
        let mut tool_calls = Vec::new();

        for item in response.content {
            match item {
                AnthropicContent::Text { text } => message.push_str(&text),
                AnthropicContent::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        args: input.to_string(),
                    });
                }
                AnthropicContent::Other => {}
            }
        }

        Ok(ChatCompletion {
            message,
            tool_calls,
            provider: self.name().to_string(),
            model: config.model.clone(),
        })
    }

    async fn chat_completion_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
        sink: DeltaSink<'_>,
    ) -> Result<ChatCompletion> {
        let api_key = Self::api_key(config)?;
        let (url, body) = Self::build_request(messages, tools, config, true)?;

        let http_response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await?;
        let status = http_response.status();
        if !status.is_success() {
            let body_text = http_response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error ({status}): {body_text}");
        }

        let mut fold = StreamFold::default();
        let mut buf = String::new();
        let mut stream = http_response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE: events are separated by a blank line. Keep a partial event.
            while let Some(end) = buf.find("\n\n") {
                let raw = buf[..end].to_string();
                buf.drain(..end + 2);
                let Some(data) = sse_data(&raw) else { continue };
                if fold_event(&data, &mut fold, sink)? {
                    return Ok(fold.finish(self.name(), &config.model));
                }
            }
        }
        tracing::warn!("Anthropic stream ended without message_stop; using what arrived");
        Ok(fold.finish(self.name(), &config.model))
    }
}

/// The Messages API's rule for a tool name: 1-128 characters of `[A-Za-z0-9_-]`.
pub fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Join the `data:` lines of one SSE event; `None` for comments/keep-alives.
pub fn sse_data(raw: &str) -> Option<String> {
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Accumulated state of one streamed completion. Tool-use blocks are keyed by
/// their content-block index because their JSON arrives in fragments.
#[derive(Debug, Default)]
pub struct StreamFold {
    pub message: String,
    blocks: Vec<(usize, ToolCall)>,
}

impl StreamFold {
    fn finish(self, provider: &str, model: &str) -> ChatCompletion {
        let tool_calls: Vec<ToolCall> = self
            .blocks
            .into_iter()
            .map(|(_, mut call)| {
                if call.args.is_empty() {
                    call.args = "{}".to_string();
                }
                call
            })
            .collect();
        ChatCompletion {
            message: self.message,
            tool_calls,
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }
}

/// Fold one decoded SSE `data` payload. Returns `Ok(true)` on `message_stop`.
pub fn fold_event(data: &str, fold: &mut StreamFold, sink: DeltaSink<'_>) -> Result<bool> {
    let ev: StreamEvent = serde_json::from_str(data).map_err(|e| {
        anyhow::anyhow!(
            "unexpected Anthropic stream event ({e}): {}",
            data.chars().take(300).collect::<String>()
        )
    })?;
    match ev {
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            if let ContentBlockStart::ToolUse { id, name } = content_block {
                fold.blocks.push((
                    index,
                    ToolCall {
                        id,
                        name,
                        args: String::new(),
                    },
                ));
            }
        }
        StreamEvent::ContentBlockDelta { index, delta } => match delta {
            BlockDelta::TextDelta { text } => {
                if !text.is_empty() {
                    fold.message.push_str(&text);
                    sink(StreamDelta::Text(text));
                }
            }
            BlockDelta::InputJsonDelta { partial_json } => {
                if let Some((_, call)) = fold.blocks.iter_mut().find(|(i, _)| *i == index) {
                    call.args.push_str(&partial_json);
                }
            }
            BlockDelta::Other => {}
        },
        StreamEvent::Error { error } => {
            anyhow::bail!("Anthropic API error (stream): {}", error.message);
        }
        StreamEvent::MessageStop => return Ok(true),
        StreamEvent::Other => {}
    }
    Ok(false)
}

// ── Anthropic API Data Structures ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    /// A plain string, or an array of content blocks when the message carries
    /// a `cache_control` breakpoint.
    content: Value,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// `thinking`, `redacted_thinking`, and whatever comes next: not ours to
    /// read, and not a reason to fail the turn.
    #[serde(other)]
    Other,
}

/// The streaming event types this fold cares about; everything else
/// (`message_start`, `message_delta`, `content_block_stop`, `ping`) is `Other`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: BlockDelta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Error { error: StreamError },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlockStart {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct StreamError {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn sse_data_joins_data_lines_and_ignores_comments() {
        assert_eq!(
            sse_data("event: content_block_delta\ndata: {\"a\":1}").as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(sse_data(": ping").as_deref(), None);
        assert_eq!(
            sse_data("data: one\ndata: two").as_deref(),
            Some("one\ntwo")
        );
    }

    #[test]
    fn folds_text_deltas_and_a_fragmented_tool_use_block() {
        let seen = Mutex::new(Vec::new());
        let sink = |d: StreamDelta| seen.lock().unwrap().push(d);
        let mut fold = StreamFold::default();
        let events = [
            r#"{"type":"message_start","message":{"id":"msg_1","role":"assistant"}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Checking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" the clock."}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"shell","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"and\": \"time /T\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":12}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let mut done = false;
        for e in events {
            done = fold_event(e, &mut fold, &sink).unwrap();
        }
        assert!(done);
        let c = fold.finish("anthropic", "claude-sonnet-5");
        assert_eq!(c.message, "Checking the clock.");
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].id, "toolu_01");
        assert_eq!(c.tool_calls[0].name, "shell");
        assert_eq!(c.tool_calls[0].args, r#"{"command": "time /T"}"#);
        assert_eq!(
            seen.into_inner().unwrap(),
            vec![
                StreamDelta::Text("Checking".into()),
                StreamDelta::Text(" the clock.".into())
            ]
        );
    }

    #[test]
    fn cache_breakpoints_land_on_system_last_tool_and_the_prefix_boundary() {
        struct Noop;
        #[async_trait]
        impl Tool for Noop {
            fn name(&self) -> &str {
                "noop"
            }
            fn description(&self) -> &str {
                "does nothing"
            }
            fn parameters_schema(&self) -> Value {
                serde_json::json!({"type":"object","properties":{}})
            }
            async fn execute(&self, _args: Value) -> anyhow::Result<obc_tool_api::ToolResult> {
                Ok(obc_tool_api::ToolResult::ok(""))
            }
        }
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(Noop), Box::new(Noop)];
        let msgs = vec![
            ChatMessage {
                role: ChatRole::System,
                content: "who I am".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "earlier".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "reply".into(),
            },
            ChatMessage {
                role: ChatRole::System,
                content: "## World state".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "the ask".into(),
            },
        ];
        let cfg = ProviderConfig {
            name: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            ..Default::default()
        };
        let (_, body) = AnthropicProvider::build_request(&msgs, &tools, &cfg, false).unwrap();

        assert_eq!(body["system"][0]["text"], "who I am");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(
            m[0]["content"], "earlier",
            "plain string before the boundary"
        );
        assert_eq!(
            m[1]["content"][0]["text"], "reply",
            "the last stable message is the breakpoint"
        );
        assert_eq!(m[1]["content"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            m[2]["role"], "user",
            "the ephemeral block rides as user content"
        );
        assert_eq!(m[2]["content"], "## World state");
        assert_eq!(m[3]["content"], "the ask");

        let off = ProviderConfig {
            prompt_caching: false,
            ..cfg.clone()
        };
        let (_, body) = AnthropicProvider::build_request(&msgs, &tools, &off, false).unwrap();
        assert_eq!(
            body["system"], "who I am",
            "a plain string when caching is off"
        );
        assert!(body["tools"][1].get("cache_control").is_none());
        assert_eq!(body["messages"][1]["content"], "reply");
    }

    #[test]
    fn a_tool_use_block_with_no_json_deltas_gets_empty_object_args() {
        let sink = |_d: StreamDelta| {};
        let mut fold = StreamFold::default();
        fold_event(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"mesh_status","input":{}}}"#,
            &mut fold,
            &sink,
        )
        .unwrap();
        assert!(fold_event(r#"{"type":"message_stop"}"#, &mut fold, &sink).unwrap());
        let c = fold.finish("anthropic", "m");
        assert_eq!(c.tool_calls[0].args, "{}");
    }

    #[test]
    fn an_error_event_fails_the_stream_with_the_apis_message() {
        let sink = |_d: StreamDelta| {};
        let mut fold = StreamFold::default();
        let err = fold_event(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            &mut fold,
            &sink,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Overloaded"));
    }
}

#[cfg(test)]
mod current_models_tests {
    use super::*;
    use async_trait::async_trait;

    struct Named(&'static str);

    #[async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "t"
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        // Named at runtime, so the Track 0 audit wants the risk said out loud.
        fn risk_class(&self) -> obc_tool_api::RiskClass {
            obc_tool_api::RiskClass::safe()
        }
        async fn execute(&self, _args: Value) -> anyhow::Result<obc_tool_api::ToolResult> {
            Ok(obc_tool_api::ToolResult::ok("ok"))
        }
    }

    fn cfg(think: Option<bool>) -> ProviderConfig {
        ProviderConfig {
            name: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            think,
            ..Default::default()
        }
    }

    fn msgs() -> Vec<ChatMessage> {
        vec![ChatMessage {
            role: ChatRole::User,
            content: "hi".into(),
        }]
    }

    #[test]
    fn no_temperature_and_think_maps_to_the_thinking_field() {
        let (_, body) = AnthropicProvider::build_request(&msgs(), &[], &cfg(None), false).unwrap();
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("thinking").is_none());
        let (_, body) =
            AnthropicProvider::build_request(&msgs(), &[], &cfg(Some(false)), false).unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
        let (_, body) =
            AnthropicProvider::build_request(&msgs(), &[], &cfg(Some(true)), false).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
    }

    #[test]
    fn a_tool_with_an_illegal_name_is_left_out_not_fatal() {
        let long: &'static str = Box::leak("learned_".repeat(30).into_boxed_str());
        assert_eq!(long.len(), 240);
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(Named("schedule")),
            Box::new(Named(long)),
            Box::new(Named("has space")),
        ];
        let (_, body) =
            AnthropicProvider::build_request(&msgs(), &tools, &cfg(None), false).unwrap();
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["schedule"]);
        // all bad -> no tools key at all rather than an empty array
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(Named(long))];
        let (_, body) =
            AnthropicProvider::build_request(&msgs(), &tools, &cfg(None), false).unwrap();
        assert!(body.get("tools").is_none());
        assert!(valid_tool_name("a-b_C9"));
        assert!(!valid_tool_name(""));
        assert!(!valid_tool_name(&"x".repeat(129)));
    }

    #[test]
    fn a_thinking_block_in_the_response_is_ignored_not_an_error() {
        let raw = r#"{"content":[{"type":"thinking","thinking":"","signature":"sig"},
            {"type":"text","text":"hello"},
            {"type":"tool_use","id":"t1","name":"schedule","input":{"action":"list"}}]}"#;
        let r: AnthropicResponse = serde_json::from_str(raw).unwrap();
        let kinds: Vec<&str> = r
            .content
            .iter()
            .map(|c| match c {
                AnthropicContent::Text { .. } => "text",
                AnthropicContent::ToolUse { .. } => "tool_use",
                AnthropicContent::Other => "other",
            })
            .collect();
        assert_eq!(kinds, ["other", "text", "tool_use"]);
    }
}

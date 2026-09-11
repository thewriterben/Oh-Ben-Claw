//! Ollama provider adapter.
//!
//! Streams since 2026-09-11. `chat_completion` still sends `"stream": false`
//! and reads one JSON object; `chat_completion_streaming` sends
//! `"stream": true` and folds Ollama's NDJSON chunks as they arrive — each
//! `message.content` fragment goes to the sink the moment it is read, tool
//! calls are collected from whichever chunks carry them, and the final chunk
//! (`"done": true`) closes the completion. The fold is a pure function over
//! lines ([`fold_chunk`]) so it is tested without a server.
//!
//! Two things learned on the bench on 2026-09-11 after the agent moved its
//! ephemeral blocks (world state, experience) behind the history so the prompt
//! prefix would cache:
//!
//! - **Ollama hoists every `system`-role message into the template's
//!   `.System`**, ahead of the tools and the history, wherever it sat in the
//!   list. The reorder was undone on the wire and the runner's longest cached
//!   prefix stayed at ~240 tokens — the system prompt and nothing else — so
//!   every turn re-evaluated ~7k tokens. Only the *leading* system message is
//!   sent as `system` now; any later one rides as `user` content, in place,
//!   which is where the agent put it. Same rule the Anthropic adapter follows.
//! - **`/no_think` in the system prompt is not read by Qwen3's template.**
//!   The template appends `/no_think` to the last user turn only when the
//!   request sets `think`; without it the model thought for 228 tokens
//!   (5.9 s) before a 23-character answer. `ProviderConfig::think`, when
//!   set, goes on the wire as `"think"`; it is `None` by default because
//!   Ollama rejects the field for models without the thinking capability.

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

/// The Ollama provider.
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    client: Client,
}

impl OllamaProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// The request body both paths send; only `stream` differs.
    fn build_request(
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
        stream: bool,
    ) -> (String, Value) {
        let url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434/api/chat".to_string());

        let ollama_messages: Vec<OllamaMessage> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| OllamaMessage {
                role: match m.role {
                    ChatRole::System if i == 0 => "system".into(),
                    // A later system message is ephemeral context the agent
                    // placed deliberately; `user` keeps it there (see module doc).
                    ChatRole::System => "user".into(),
                    ChatRole::User => "user".into(),
                    ChatRole::Assistant => "assistant".into(),
                },
                content: m.content.clone(),
            })
            .collect();

        let ollama_tools: Option<Vec<OllamaTool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| OllamaTool {
                        r#type: "function".into(),
                        function: OllamaFunction {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            parameters: t.parameters_schema(),
                        },
                    })
                    .collect(),
            )
        };

        let mut request = serde_json::json!({
            "model": config.model,
            "messages": ollama_messages,
            "tools": ollama_tools,
            "stream": stream,
        });
        if let Some(think) = config.think {
            request["think"] = Value::Bool(think);
        }

        // Ollama supports a `format` field: `"json"` for free-form JSON or an
        // inline JSON schema object for structured output.
        if let Some(ref fmt) = config.response_format {
            match fmt {
                ResponseFormat::Text => {}
                ResponseFormat::JsonObject => {
                    request["format"] = Value::String("json".into());
                }
                ResponseFormat::JsonSchema { schema, .. } => {
                    request["format"] = schema.clone();
                }
            }
        }
        (url, request)
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let (url, request) = Self::build_request(messages, tools, config, false);

        // Surface API errors instead of force-parsing them (see openai.rs; an
        // uninstalled model's error JSON showed as "error decoding response").
        let http_response = self.client.post(&url).json(&request).send().await?;
        let status = http_response.status();
        let body = http_response.text().await?;
        if !status.is_success() {
            anyhow::bail!("Ollama API error ({status}): {body}");
        }
        let response: OllamaResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!(
                "unexpected Ollama response shape ({e}): {}",
                body.chars().take(300).collect::<String>()
            )
        })?;

        Ok(ChatCompletion {
            message: response.message.content,
            tool_calls: response
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
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
        let (url, request) = Self::build_request(messages, tools, config, true);

        let http_response = self.client.post(&url).json(&request).send().await?;
        let status = http_response.status();
        if !status.is_success() {
            let body = http_response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama API error ({status}): {body}");
        }

        let mut fold = StreamFold::default();
        let mut buf = String::new();
        let mut body = http_response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // NDJSON: one object per line. Keep any trailing partial line.
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                if line.is_empty() {
                    continue;
                }
                if fold_chunk(&line, &mut fold, sink)? {
                    return Ok(fold.finish(self.name(), &config.model));
                }
            }
        }
        // The server closed without a `done: true` line. What was folded is
        // still a completion; say so rather than fail a turn that produced text.
        if !buf.trim().is_empty() {
            fold_chunk(buf.trim(), &mut fold, sink)?;
        }
        tracing::warn!("Ollama stream ended without a done chunk; using what arrived");
        Ok(fold.finish(self.name(), &config.model))
    }
}

/// Accumulated state of one streamed completion.
#[derive(Debug, Default)]
pub struct StreamFold {
    pub message: String,
    pub tool_calls: Vec<ToolCall>,
}

impl StreamFold {
    fn finish(self, provider: &str, model: &str) -> ChatCompletion {
        ChatCompletion {
            message: self.message,
            tool_calls: self.tool_calls,
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }
}

/// Fold one NDJSON line into the accumulator, forwarding text to the sink.
/// Returns `Ok(true)` when the line was the final (`done: true`) chunk.
pub fn fold_chunk(line: &str, fold: &mut StreamFold, sink: DeltaSink<'_>) -> Result<bool> {
    let chunk: OllamaChunk = serde_json::from_str(line).map_err(|e| {
        anyhow::anyhow!(
            "unexpected Ollama stream chunk ({e}): {}",
            line.chars().take(300).collect::<String>()
        )
    })?;
    if let Some(err) = chunk.error {
        anyhow::bail!("Ollama API error (stream): {err}");
    }
    if let Some(message) = chunk.message {
        if !message.content.is_empty() {
            fold.message.push_str(&message.content);
            sink(StreamDelta::Text(message.content));
        }
        if let Some(calls) = message.tool_calls {
            fold.tool_calls.extend(calls.into_iter().map(Into::into));
        }
    }
    Ok(chunk.done)
}

// ── Ollama API Data Structures ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaFunction,
}

#[derive(Debug, Serialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    message: OllamaResponseMessage,
}

/// One line of a streamed response. `message` is absent on error lines;
/// `content` is empty on the tool-call and final chunks.
#[derive(Debug, Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaResponseMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    #[serde(default)]
    content: String,
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: Value,
}

impl From<OllamaToolCall> for ToolCall {
    fn from(call: OllamaToolCall) -> Self {
        Self {
            id: format!("ollama-{}", uuid::Uuid::new_v4()),
            name: call.function.name,
            args: call.function.arguments.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn only_the_leading_system_message_is_sent_as_system() {
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
            name: "ollama".into(),
            model: "qwen3".into(),
            ..Default::default()
        };
        let (_, body) = OllamaProvider::build_request(&msgs, &[], &cfg, false);
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "user", "assistant", "user", "user"]);
        assert_eq!(body["messages"][3]["content"], "## World state");
        assert!(body.get("think").is_none(), "absent unless configured");

        let cfg = ProviderConfig {
            think: Some(false),
            ..cfg
        };
        let (_, body) = OllamaProvider::build_request(&msgs, &[], &cfg, true);
        assert_eq!(body["think"], false);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn folds_text_chunks_then_tool_calls_then_done() {
        let seen = Mutex::new(Vec::new());
        let sink = |d: StreamDelta| seen.lock().unwrap().push(d);
        let mut fold = StreamFold::default();
        let lines = [
            r#"{"model":"m","message":{"role":"assistant","content":"The "},"done":false}"#,
            r#"{"model":"m","message":{"role":"assistant","content":"time"},"done":false}"#,
            r#"{"model":"m","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"shell","arguments":{"command":"time /T"}}}]},"done":false}"#,
            r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"eval_count":9}"#,
        ];
        let mut done = false;
        for l in lines {
            done = fold_chunk(l, &mut fold, &sink).unwrap();
        }
        assert!(done);
        assert_eq!(fold.message, "The time");
        assert_eq!(fold.tool_calls.len(), 1);
        assert_eq!(fold.tool_calls[0].name, "shell");
        assert_eq!(fold.tool_calls[0].args, r#"{"command":"time /T"}"#);
        assert_eq!(
            seen.into_inner().unwrap(),
            vec![
                StreamDelta::Text("The ".into()),
                StreamDelta::Text("time".into())
            ],
            "empty content chunks (tool call, done) emit nothing"
        );
    }

    #[test]
    fn an_error_line_fails_the_stream_with_the_servers_message() {
        let sink = |_d: StreamDelta| {};
        let mut fold = StreamFold::default();
        let err =
            fold_chunk(r#"{"error":"model 'nope' not found"}"#, &mut fold, &sink).unwrap_err();
        assert!(err.to_string().contains("model 'nope' not found"));
    }

    #[test]
    fn a_non_json_line_is_reported_not_swallowed() {
        let sink = |_d: StreamDelta| {};
        let mut fold = StreamFold::default();
        let err = fold_chunk("<html>proxy said no</html>", &mut fold, &sink).unwrap_err();
        assert!(err.to_string().contains("unexpected Ollama stream chunk"));
    }
}

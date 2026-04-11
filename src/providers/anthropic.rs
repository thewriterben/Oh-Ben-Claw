//! Anthropic provider adapter.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ResponseFormat, ToolCall};
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
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
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| anyhow::anyhow!("ANTHROPIC_API_KEY not set"))?;

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

        let anth_messages: Vec<AnthropicMessage> = conversation
            .iter()
            .filter(|m| m.role != ChatRole::System)
            .map(|m| AnthropicMessage {
                role: match m.role {
                    ChatRole::User => "user".into(),
                    ChatRole::Assistant => "assistant".into(),
                    ChatRole::System => "user".into(),
                },
                content: m.content.clone(),
            })
            .collect();

        let anth_tools: Option<Vec<AnthropicTool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| AnthropicTool {
                        name: t.name().to_string(),
                        description: t.description().to_string(),
                        input_schema: t.parameters_schema(),
                    })
                    .collect(),
            )
        };

        let mut body = serde_json::json!({
            "model": config.model,
            "messages": anth_messages,
            "temperature": config.temperature,
            "max_tokens": 4096,
        });

        if let Some(sys) = system_prompt {
            body["system"] = Value::String(sys);
        }

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
            body["tools"] = serde_json::to_value(t)?;
            body["tool_choice"] = serde_json::json!({"type": "auto"});
        }

        let response: AnthropicResponse = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

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
            }
        }

        Ok(ChatCompletion {
            message,
            tool_calls,
            provider: self.name().to_string(),
            model: config.model.clone(),
        })
    }
}

// ── Anthropic API Data Structures ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
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
}

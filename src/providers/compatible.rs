//! Generic OpenAI-compatible provider adapter.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ResponseFormat, ToolCall};
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A generic OpenAI-compatible provider.
#[derive(Debug, Clone)]
pub struct CompatibleProvider {
    config: ProviderConfig,
    client: Client,
}

impl CompatibleProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for CompatibleProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let api_key = config.api_key.clone();

        let url = config
            .base_url
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!("base_url not set for compatible provider '{}'", config.name)
            })?
            .clone();

        let compat_messages: Vec<CompatMessage> = messages
            .iter()
            .map(|m| CompatMessage {
                role: match m.role {
                    ChatRole::System => "system".into(),
                    ChatRole::User => "user".into(),
                    ChatRole::Assistant => "assistant".into(),
                },
                content: m.content.clone(),
            })
            .collect();

        let compat_tools: Option<Vec<CompatTool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| CompatTool {
                        r#type: "function".into(),
                        function: CompatFunction {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            parameters: t.parameters_schema(),
                        },
                    })
                    .collect(),
            )
        };

        let request = serde_json::json!({
            "model": config.model,
            "messages": compat_messages,
            "tools": compat_tools,
            "tool_choice": if tools.is_empty() { Value::Null } else { Value::String("auto".into()) },
            "temperature": config.temperature,
        });

        let mut request = request;
        if let Some(ref fmt) = config.response_format {
            match fmt {
                ResponseFormat::Text => {}
                ResponseFormat::JsonObject => {
                    request["response_format"] = serde_json::json!({"type": "json_object"});
                }
                ResponseFormat::JsonSchema {
                    name,
                    schema,
                    strict,
                } => {
                    request["response_format"] = serde_json::json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": name,
                            "schema": schema,
                            "strict": strict,
                        }
                    });
                }
            }
        }

        let mut builder = self.client.post(&url);
        if let Some(key) = api_key {
            builder = builder.bearer_auth(key);
        }

        let response: CompatResponse = builder.json(&request).send().await?.json().await?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in compatible provider response"))?;

        Ok(ChatCompletion {
            message: choice.message.content.unwrap_or_default(),
            tool_calls: choice
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
}

// ── Compatible API Data Structures (OpenAI-compatible) ───────────────────────

#[derive(Debug, Serialize)]
struct CompatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct CompatTool {
    r#type: String,
    function: CompatFunction,
}

#[derive(Debug, Serialize)]
struct CompatFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct CompatResponse {
    choices: Vec<CompatChoice>,
}

#[derive(Debug, Deserialize)]
struct CompatChoice {
    message: CompatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct CompatResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<CompatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct CompatToolCall {
    id: String,
    function: CompatToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct CompatToolCallFunction {
    name: String,
    arguments: String,
}

impl From<CompatToolCall> for ToolCall {
    fn from(call: CompatToolCall) -> Self {
        Self {
            id: call.id,
            name: call.function.name,
            args: call.function.arguments,
        }
    }
}

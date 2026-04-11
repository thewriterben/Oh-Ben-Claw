//! OpenRouter provider adapter.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ResponseFormat, ToolCall};
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The OpenRouter provider.
#[derive(Debug, Clone)]
pub struct OpenRouterProvider {
    client: Client,
}

impl OpenRouterProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
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
            .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
            .ok_or_else(|| anyhow::anyhow!("OPENROUTER_API_KEY not set"))?;

        let url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://openrouter.ai/api/v1/chat/completions".to_string());

        let or_messages: Vec<OrMessage> = messages
            .iter()
            .map(|m| OrMessage {
                role: match m.role {
                    ChatRole::System => "system".into(),
                    ChatRole::User => "user".into(),
                    ChatRole::Assistant => "assistant".into(),
                },
                content: m.content.clone(),
            })
            .collect();

        let or_tools: Option<Vec<OrTool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| OrTool {
                        r#type: "function".into(),
                        function: OrFunction {
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
            "messages": or_messages,
            "tools": or_tools,
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
                ResponseFormat::JsonSchema { name, schema, strict } => {
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

        let response: OrResponse = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in OpenRouter response"))?;

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

// ── OpenRouter API Data Structures (OpenAI-compatible) ───────────────────────

#[derive(Debug, Serialize)]
struct OrMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OrTool {
    r#type: String,
    function: OrFunction,
}

#[derive(Debug, Serialize)]
struct OrFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct OrResponse {
    choices: Vec<OrChoice>,
}

#[derive(Debug, Deserialize)]
struct OrChoice {
    message: OrResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OrResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OrToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OrToolCall {
    id: String,
    function: OrToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OrToolCallFunction {
    name: String,
    arguments: String,
}

impl From<OrToolCall> for ToolCall {
    fn from(call: OrToolCall) -> Self {
        Self {
            id: call.id,
            name: call.function.name,
            args: call.function.arguments,
        }
    }
}

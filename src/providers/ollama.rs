//! Ollama provider adapter.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ToolCall};
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Ollama provider.
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    config: ProviderConfig,
    client: Client,
}

impl OllamaProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
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
        let url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434/api/chat".to_string());

        let ollama_messages: Vec<OllamaMessage> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    ChatRole::System => "system".into(),
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

        let request = serde_json::json!({
            "model": config.model,
            "messages": ollama_messages,
            "tools": ollama_tools,
            "stream": false,
        });

        let response: OllamaResponse = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

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

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
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

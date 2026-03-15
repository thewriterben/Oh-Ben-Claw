//! OpenAI provider adapter.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ToolCall};
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The OpenAI provider.
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    config: ProviderConfig,
    client: Client,
}

impl OpenAiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
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
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .ok_or_else(|| anyhow::anyhow!("OPENAI_API_KEY not set"))?;

        let url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string());

        let oai_messages: Vec<OpenAiMessage> = messages.iter().map(|m| m.into()).collect();
        let oai_tools: Option<Vec<OpenAiTool>> = if tools.is_empty() {
            None
        } else {
            Some(tools.iter().map(|t| t.as_ref().into()).collect())
        };

        let request = serde_json::json!({
            "model": config.model,
            "messages": oai_messages,
            "tools": oai_tools,
            "tool_choice": if tools.is_empty() { Value::Null } else { Value::String("auto".into()) },
            "temperature": config.temperature,
        });

        let response: OpenAiResponse = self
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
            .ok_or_else(|| anyhow::anyhow!("No choices in OpenAI response"))?;

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

// ── OpenAI API Data Structures ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

impl From<&ChatMessage> for OpenAiMessage {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            role: match msg.role {
                ChatRole::System => "system".into(),
                ChatRole::User => "user".into(),
                ChatRole::Assistant => "assistant".into(),
            },
            content: msg.content.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: Value,
}

impl From<&dyn Tool> for OpenAiTool {
    fn from(tool: &dyn Tool) -> Self {
        Self {
            r#type: "function".into(),
            function: OpenAiFunction {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: String,
}

impl From<OpenAiToolCall> for ToolCall {
    fn from(call: OpenAiToolCall) -> Self {
        Self {
            id: call.id,
            name: call.function.name,
            args: call.function.arguments,
        }
    }
}

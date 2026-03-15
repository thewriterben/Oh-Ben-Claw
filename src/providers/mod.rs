//! Oh-Ben-Claw LLM provider adapters.
//!
//! Each provider adapter implements the `Provider` trait, which defines a
//! common interface for sending messages to an LLM and receiving responses.

use crate::config::ProviderConfig;
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod anthropic;
pub mod compatible;
pub mod ollama;
pub mod openai;
pub mod openrouter;

// ── Provider Trait ───────────────────────────────────────────────────────────

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// The role of a message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// The response from a provider after a chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletion {
    /// The assistant's primary response message.
    pub message: String,
    /// Any tool calls requested by the model.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// The provider that generated this completion.
    pub provider: String,
    /// The model that generated this completion.
    pub model: String,
}

/// A provider that can generate chat completions.
#[async_trait]
pub trait Provider: Send + Sync {
    /// The name of this provider (e.g., "openai", "anthropic").
    fn name(&self) -> &str;

    /// Generate a chat completion based on the given messages and tools.
    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
    ) -> Result<ChatCompletion>;
}

// ── Provider Factory ─────────────────────────────────────────────────────────

/// Create a provider instance from configuration.
pub fn from_config(config: &ProviderConfig) -> Result<Arc<dyn Provider>> {
    match config.name.as_str() {
        "openai" => Ok(Arc::new(openai::OpenAiProvider::new(config.clone()))),
        "anthropic" => Ok(Arc::new(anthropic::AnthropicProvider::new(
            config.clone(),
        ))),
        "ollama" => Ok(Arc::new(ollama::OllamaProvider::new(config.clone()))),
        "openrouter" => Ok(Arc::new(openrouter::OpenRouterProvider::new(
            config.clone(),
        ))),
        _ => Ok(Arc::new(compatible::CompatibleProvider::new(
            config.clone(),
        ))),
    }
}

//! Oh-Ben-Claw LLM provider adapters.
//!
//! Each provider adapter implements the `Provider` trait, which defines a
//! common interface for sending messages to an LLM and receiving responses.
//!
//! ## Reliability features (inspired by OpenClaw)
//!
//! * **Model failover** — configure `[[provider.fallbacks]]` to chain multiple
//!   providers/models.  If the primary fails, the next fallback is tried.
//! * **Retry policy** — configure `[provider.retry]` to automatically retry
//!   transient errors (rate-limits, network blips) with exponential back-off.

use crate::config::ProviderConfig;
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod anthropic;
pub mod compatible;
pub mod failover;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod retry;

pub use failover::FailoverProvider;
pub use retry::{RetryConfig, RetryProvider};

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

/// Create a raw provider instance from configuration (no failover/retry wrapping).
pub fn from_config(config: &ProviderConfig) -> Result<Arc<dyn Provider>> {
    match config.name.as_str() {
        "openai" => Ok(Arc::new(openai::OpenAiProvider::new(config.clone()))),
        "anthropic" => Ok(Arc::new(anthropic::AnthropicProvider::new(config.clone()))),
        "ollama" => Ok(Arc::new(ollama::OllamaProvider::new(config.clone()))),
        "openrouter" => Ok(Arc::new(openrouter::OpenRouterProvider::new(
            config.clone(),
        ))),
        _ => Ok(Arc::new(compatible::CompatibleProvider::new(
            config.clone(),
        ))),
    }
}

/// Create a fully-configured provider, applying failover and retry wrapping as
/// specified in `config`.
///
/// * If `config.fallbacks` is non-empty a [`FailoverProvider`] is constructed,
///   wrapping the primary provider and each fallback in order.
/// * If `config.retry` is `Some(_)` the result is further wrapped in a
///   [`RetryProvider`].
pub fn from_config_full(config: &ProviderConfig) -> Result<Arc<dyn Provider>> {
    // Build failover chain (includes primary + fallbacks).
    let base: Arc<dyn Provider> = if config.fallbacks.is_empty() {
        from_config(config)?
    } else {
        Arc::new(FailoverProvider::from_config(config.clone())?)
    };

    // Optionally wrap with retry policy.
    if let Some(retry_cfg) = &config.retry {
        Ok(Arc::new(RetryProvider::new(base, retry_cfg.clone())))
    } else {
        Ok(base)
    }
}

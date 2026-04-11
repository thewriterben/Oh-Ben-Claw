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

// ── Response Format ──────────────────────────────────────────────────────────

/// Requested response format for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ResponseFormat {
    /// Plain text (default). Omits `response_format` from the API request.
    #[default]
    Text,
    /// Ask the model to return valid JSON (`{"type":"json_object"}`).
    JsonObject,
    /// Ask the model to return JSON conforming to a specific schema.
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        strict: bool,
    },
}

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_format_default_is_text() {
        let fmt = ResponseFormat::default();
        assert!(matches!(fmt, ResponseFormat::Text));
    }

    #[test]
    fn response_format_serialize_text() {
        let fmt = ResponseFormat::Text;
        let json = serde_json::to_value(&fmt).unwrap();
        assert_eq!(json["type"], "text");
    }

    #[test]
    fn response_format_serialize_json_object() {
        let fmt = ResponseFormat::JsonObject;
        let json = serde_json::to_value(&fmt).unwrap();
        assert_eq!(json["type"], "json_object");
    }

    #[test]
    fn response_format_serialize_json_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        });
        let fmt = ResponseFormat::JsonSchema {
            name: "my_schema".to_string(),
            schema: schema.clone(),
            strict: true,
        };
        let json = serde_json::to_value(&fmt).unwrap();
        assert_eq!(json["type"], "json_schema");
        assert_eq!(json["name"], "my_schema");
        assert_eq!(json["schema"], schema);
        assert_eq!(json["strict"], true);
    }

    #[test]
    fn response_format_roundtrip_text() {
        let fmt = ResponseFormat::Text;
        let json_str = serde_json::to_string(&fmt).unwrap();
        let decoded: ResponseFormat = serde_json::from_str(&json_str).unwrap();
        assert!(matches!(decoded, ResponseFormat::Text));
    }

    #[test]
    fn response_format_roundtrip_json_object() {
        let fmt = ResponseFormat::JsonObject;
        let json_str = serde_json::to_string(&fmt).unwrap();
        let decoded: ResponseFormat = serde_json::from_str(&json_str).unwrap();
        assert!(matches!(decoded, ResponseFormat::JsonObject));
    }

    #[test]
    fn response_format_roundtrip_json_schema() {
        let schema = serde_json::json!({ "type": "object" });
        let fmt = ResponseFormat::JsonSchema {
            name: "test".to_string(),
            schema: schema.clone(),
            strict: false,
        };
        let json_str = serde_json::to_string(&fmt).unwrap();
        let decoded: ResponseFormat = serde_json::from_str(&json_str).unwrap();
        match decoded {
            ResponseFormat::JsonSchema {
                name,
                schema: s,
                strict,
            } => {
                assert_eq!(name, "test");
                assert_eq!(s, schema);
                assert!(!strict);
            }
            _ => panic!("Expected JsonSchema variant"),
        }
    }

    #[test]
    fn response_format_deserialize_from_json_literal() {
        let input = r#"{"type":"json_object"}"#;
        let decoded: ResponseFormat = serde_json::from_str(input).unwrap();
        assert!(matches!(decoded, ResponseFormat::JsonObject));
    }
}

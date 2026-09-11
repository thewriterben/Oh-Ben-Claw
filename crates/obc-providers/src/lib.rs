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

use anyhow::Result;
use async_trait::async_trait;
use obc_tools::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod anthropic;
pub mod compatible;
pub mod failover;
pub mod model_registry;
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

// `ChatMessage` and `ChatRole` moved into the `obc-memory` crate on 2026-07-30.
// They are the shape a conversation is stored in, and the substrate that stores
// them should own them; re-exported here because `crate::ChatMessage`
// is what every existing caller says.
pub use obc_memory::{ChatMessage, ChatRole};

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

/// A piece of a streamed completion, handed to a [`DeltaSink`] as it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    /// More assistant text. Concatenating every `Text` in order reproduces
    /// the final [`ChatCompletion::message`].
    Text(String),
    /// Discard everything received so far for this completion: a wrapper
    /// (retry, failover) is starting the request over after a partial stream
    /// failed. A client that has already rendered text clears it.
    Restart,
}

/// Where streamed deltas go. A plain `&dyn Fn` rather than a channel or a
/// trait object with state, so a caller can capture whatever it likes
/// (an event bus, a `String`, a test's `Vec`) and nothing here has to know.
pub type DeltaSink<'a> = &'a (dyn Fn(StreamDelta) + Send + Sync);

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

    /// Generate a chat completion, delivering text to `sink` as it is produced.
    ///
    /// The returned [`ChatCompletion`] is complete and identical in shape to
    /// what [`Provider::chat_completion`] returns, so the agent loop's tool
    /// handling does not change; only the operator sees words earlier.
    ///
    /// The default does not stream: it runs `chat_completion` and hands the
    /// whole message to the sink once. Every provider therefore "streams" in
    /// the sense the agent needs (one delta, then done), and only the ones
    /// that override this — Ollama and Anthropic since 2026-09-11 — stream in
    /// the sense the operator wants.
    async fn chat_completion_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
        sink: DeltaSink<'_>,
    ) -> Result<ChatCompletion> {
        let completion = self.chat_completion(messages, tools, config).await?;
        if !completion.message.is_empty() {
            sink(StreamDelta::Text(completion.message.clone()));
        }
        Ok(completion)
    }
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

// ── The provider's own configuration block ──────────────────────────────────
// Moved here from the root config module on 2026-08-13. That module
// re-exports it, so every call site outside this directory is unchanged.
//
// Why it had to move: two of its fields are typed from this module
// (`RetryConfig`, `ResponseFormat`) while all ten files in here imported the
// struct from `config`. One struct, split across two modules, pointing both
// ways — a mutual pair in the dependency graph and most of the core's
// remaining cycles.
//
// Every extracted crate here already works this way: obc-planner owns
// `DeploymentConfig`, obc-conscience owns `ConscienceConfig`, obc-cost owns
// `CostConfig`. This was the exception, and it was the expensive one.
//
// `SecretString` comes from obc-safety, not from config: it is a
// redact-in-Debug wrapper, which is secret hygiene rather than configuration,
// and it moved to the crate that already owns the vault on the same day. That
// is what let this struct leave without dragging its edge along with it.

use obc_safety::SecretString;

/// Configuration for the LLM provider.
///
/// ## Reliability (inspired by OpenClaw)
///
/// Add `[[provider.fallbacks]]` tables to define an ordered list of backup
/// providers.  If the primary provider fails the next fallback is tried
/// automatically via the `FailoverProvider` wrapper.
///
/// Add a `[provider.retry]` table to enable transparent exponential-back-off
/// retries on transient errors (rate-limits, network blips).
///
/// ```toml
/// [provider]
/// name    = "openai"
/// model   = "gpt-4o"
/// api_key = "sk-..."
///
/// [provider.retry]
/// max_retries      = 3
/// initial_backoff_ms = 500
///
/// [[provider.fallbacks]]
/// name    = "anthropic"
/// model   = "claude-3-5-sonnet-20241022"
/// api_key = "sk-ant-..."
///
/// [[provider.fallbacks]]
/// name  = "ollama"
/// model = "llama3.2"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// The provider name (e.g., "openai", "anthropic", "gemini", "ollama").
    #[serde(default = "default_provider_name")]
    pub name: String,
    /// The model to use (e.g., "gpt-4o", "claude-3-5-sonnet-20241022").
    #[serde(default = "default_model")]
    pub model: String,
    /// The API key for the provider — **prefer the environment variable**.
    ///
    /// Leave this unset and export `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
    /// `OPENROUTER_API_KEY` (etc.) instead. That is the supported path, and it is the
    /// one that keeps a credential out of the file people paste into issues, commit to
    /// a repo, or hand over when asking for help. An inline key here works and warns at
    /// startup.
    ///
    /// [`SecretString`] redacts itself in `Debug` and `Display`, so a config that ends
    /// up in a log line or a panic message does not carry the key with it.
    #[serde(default)]
    pub api_key: Option<SecretString>,
    /// The base URL for the provider API (for OpenAI-compatible endpoints).
    #[serde(default)]
    pub base_url: Option<String>,
    /// The default temperature for LLM calls.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Ordered list of fallback provider configurations to try when the
    /// primary provider fails (model failover, inspired by OpenClaw).
    #[serde(default)]
    pub fallbacks: Vec<ProviderConfig>,
    /// Optional retry policy for transient errors (rate-limits, network
    /// issues).  If unset, no automatic retries are performed.
    #[serde(default)]
    pub retry: Option<retry::RetryConfig>,
    /// Optional response format (structured output / JSON mode).
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,
    /// Ask the provider to cache the stable prefix (system prompt, tool
    /// schemas, history up to the ephemeral blocks). Anthropic honours it via
    /// `cache_control` breakpoints; Ollama caches the prefix on its own. On by
    /// default since 2026-09-11.
    #[serde(default = "default_prompt_caching")]
    pub prompt_caching: bool,
    /// Ollama only: the request's `think` field. `Some(false)` switches a
    /// thinking model (Qwen3) to answering directly — the `/no_think` soft
    /// switch in a system prompt is not honoured by the template. Leave unset
    /// for models without the thinking capability; Ollama rejects the field.
    #[serde(default)]
    pub think: Option<bool>,
}

fn default_prompt_caching() -> bool {
    true
}

fn default_provider_name() -> String {
    "openai".to_string()
}

fn default_model() -> String {
    "gpt-4o".to_string()
}

fn default_temperature() -> f64 {
    0.7
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: default_provider_name(),
            model: default_model(),
            api_key: None,
            base_url: None,
            temperature: default_temperature(),
            fallbacks: vec![],
            retry: None,
            response_format: None,
            prompt_caching: default_prompt_caching(),
            think: None,
        }
    }
}

#[cfg(test)]
mod streaming_default_tests {
    use super::*;
    use std::sync::Mutex;

    struct OneShot;

    #[async_trait]
    impl Provider for OneShot {
        fn name(&self) -> &str {
            "oneshot"
        }
        async fn chat_completion(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Box<dyn Tool>],
            config: &ProviderConfig,
        ) -> Result<ChatCompletion> {
            Ok(ChatCompletion {
                message: "whole answer".into(),
                tool_calls: vec![],
                provider: "oneshot".into(),
                model: config.model.clone(),
            })
        }
    }

    #[tokio::test]
    async fn a_provider_that_does_not_stream_still_delivers_one_delta() {
        let seen = Mutex::new(Vec::new());
        let sink = |d: StreamDelta| seen.lock().unwrap().push(d);
        let cfg = ProviderConfig::default();
        let c = OneShot
            .chat_completion_streaming(&[], &[], &cfg, &sink)
            .await
            .unwrap();
        assert_eq!(c.message, "whole answer");
        assert_eq!(
            seen.into_inner().unwrap(),
            vec![StreamDelta::Text("whole answer".into())]
        );
    }
}

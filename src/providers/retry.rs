//! Retry policy for LLM provider calls.
//!
//! Wraps any `Provider` implementation and automatically retries failed
//! requests using exponential back-off with optional jitter.  Transient
//! network errors and HTTP 5xx / 429 (rate-limit) responses are retried;
//! hard application errors (bad API key, invalid request, etc.) surface
//! immediately without retrying.

use crate::config::ProviderConfig;
use crate::providers::{ChatCompletion, ChatMessage, Provider};
use crate::tools::traits::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Retry configuration embedded in [`ProviderConfig`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial try).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial back-off delay in milliseconds.
    #[serde(default = "default_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    /// Maximum back-off delay in milliseconds.
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
    /// Back-off multiplier applied after each failure.
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,
}

fn default_max_retries() -> u32 {
    3
}
fn default_initial_backoff_ms() -> u64 {
    500
}
fn default_max_backoff_ms() -> u64 {
    10_000
}
fn default_backoff_multiplier() -> f64 {
    2.0
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_backoff_ms: default_initial_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
            backoff_multiplier: default_backoff_multiplier(),
        }
    }
}

// ── RetryProvider ─────────────────────────────────────────────────────────────

/// A provider decorator that retries transient failures using exponential
/// back-off.
pub struct RetryProvider {
    inner: Arc<dyn Provider>,
    config: RetryConfig,
}

impl RetryProvider {
    /// Wrap `inner` with the given retry configuration.
    pub fn new(inner: Arc<dyn Provider>, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    /// Return `true` if the error string looks like a transient failure worth
    /// retrying (rate-limits, network issues, temporary server errors).
    fn is_transient(err: &anyhow::Error) -> bool {
        let msg = err.to_string().to_lowercase();
        msg.contains("429")
            || msg.contains("rate limit")
            || msg.contains("too many requests")
            || msg.contains("503")
            || msg.contains("502")
            || msg.contains("500")
            || msg.contains("connection")
            || msg.contains("timeout")
            || msg.contains("network")
            || msg.contains("temporarily")
    }
}

#[async_trait]
impl Provider for RetryProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let mut backoff_ms = self.config.initial_backoff_ms;
        let mut last_err: anyhow::Error = anyhow::anyhow!("No attempts made");

        for attempt in 0..=self.config.max_retries {
            match self.inner.chat_completion(messages, tools, config).await {
                Ok(completion) => return Ok(completion),
                Err(e) => {
                    if attempt == self.config.max_retries || !Self::is_transient(&e) {
                        return Err(e);
                    }
                    tracing::warn!(
                        provider = self.inner.name(),
                        attempt,
                        backoff_ms,
                        error = %e,
                        "Transient provider error — retrying after back-off"
                    );
                    last_err = e;
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms as f64 * self.config.backoff_multiplier) as u64;
                    backoff_ms = backoff_ms.min(self.config.max_backoff_ms);
                }
            }
        }
        Err(last_err)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_transient_rate_limit() {
        let err = anyhow::anyhow!("HTTP 429: rate limit exceeded");
        assert!(RetryProvider::is_transient(&err));
    }

    #[test]
    fn test_is_transient_connection() {
        let err = anyhow::anyhow!("connection refused");
        assert!(RetryProvider::is_transient(&err));
    }

    #[test]
    fn test_not_transient_auth() {
        let err = anyhow::anyhow!("Invalid API key");
        assert!(!RetryProvider::is_transient(&err));
    }

    #[test]
    fn test_retry_config_defaults() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.initial_backoff_ms, 500);
        assert!(cfg.backoff_multiplier > 1.0);
    }
}

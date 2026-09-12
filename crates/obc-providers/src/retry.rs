//! Retry policy for LLM provider calls.
//!
//! Wraps any `Provider` implementation and automatically retries failed
//! requests using exponential back-off with optional jitter.  Transient
//! network errors and HTTP 5xx / 429 (rate-limit) responses are retried;
//! hard application errors (bad API key, invalid request, etc.) surface
//! immediately without retrying.

use crate::ProviderConfig;
use crate::{ChatCompletion, ChatMessage, Provider};
use anyhow::Result;
use async_trait::async_trait;
use obc_tool_api::Tool;
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

    async fn chat_completion_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        config: &ProviderConfig,
        sink: crate::DeltaSink<'_>,
    ) -> Result<ChatCompletion> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut backoff_ms = self.config.initial_backoff_ms;
        let mut last_err: anyhow::Error = anyhow::anyhow!("No attempts made");
        let emitted = AtomicBool::new(false);

        for attempt in 0..=self.config.max_retries {
            if emitted.swap(false, Ordering::SeqCst) {
                sink(crate::StreamDelta::Restart);
            }
            let tracking = |d: crate::StreamDelta| {
                emitted.store(true, Ordering::SeqCst);
                sink(d)
            };
            match self
                .inner
                .chat_completion_streaming(messages, tools, config, &tracking)
                .await
            {
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
                        "Transient provider error mid-stream — retrying after back-off"
                    );
                    last_err = e;
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms as f64 * self.config.backoff_multiplier) as u64;
                    if backoff_ms > self.config.max_backoff_ms {
                        backoff_ms = self.config.max_backoff_ms;
                    }
                }
            }
        }

        Err(last_err)
    }
}

#[cfg(test)]
mod streaming_restart_tests {
    use super::*;
    use crate::{ChatCompletion, DeltaSink, ProviderConfig, StreamDelta};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// Streams "par" then fails with a transient error on the first attempt,
    /// streams "whole" and succeeds on the second.
    struct FlakyStreamer {
        attempts: AtomicU32,
    }

    #[async_trait::async_trait]
    impl crate::Provider for FlakyStreamer {
        fn name(&self) -> &str {
            "flaky"
        }
        async fn chat_completion(
            &self,
            _m: &[ChatMessage],
            _t: &[Box<dyn Tool>],
            _c: &ProviderConfig,
        ) -> Result<ChatCompletion> {
            unreachable!("streaming path only")
        }
        async fn chat_completion_streaming(
            &self,
            _m: &[ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &ProviderConfig,
            sink: DeltaSink<'_>,
        ) -> Result<ChatCompletion> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                sink(StreamDelta::Text("par".into()));
                anyhow::bail!("connection reset mid-stream");
            }
            sink(StreamDelta::Text("whole".into()));
            Ok(ChatCompletion {
                message: "whole".into(),
                tool_calls: vec![],
                provider: "flaky".into(),
                model: c.model.clone(),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn a_partial_stream_before_a_retry_is_restarted_not_appended() {
        let inner = Arc::new(FlakyStreamer {
            attempts: AtomicU32::new(0),
        });
        let retry = RetryProvider::new(
            inner,
            RetryConfig {
                max_retries: 2,
                initial_backoff_ms: 1,
                max_backoff_ms: 2,
                backoff_multiplier: 1.0,
            },
        );
        let seen = Mutex::new(Vec::new());
        let sink = |d: StreamDelta| seen.lock().unwrap().push(d);
        let cfg = ProviderConfig::default();
        let c = retry
            .chat_completion_streaming(&[], &[], &cfg, &sink)
            .await
            .unwrap();
        assert_eq!(c.message, "whole");
        assert_eq!(
            seen.into_inner().unwrap(),
            vec![
                StreamDelta::Text("par".into()),
                StreamDelta::Restart,
                StreamDelta::Text("whole".into()),
            ]
        );
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

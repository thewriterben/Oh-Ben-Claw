//! Model failover provider.
//!
//! Wraps an ordered list of `Provider` instances and attempts each in turn
//! until one succeeds.  If every provider fails the last error is surfaced.
//!
//! # Configuration
//!
//! ```toml
//! [provider]
//! name    = "openai"
//! model   = "gpt-4o"
//! api_key = "sk-..."
//!
//! [[provider.fallbacks]]
//! name    = "anthropic"
//! model   = "claude-3-5-sonnet-20241022"
//! api_key = "sk-ant-..."
//!
//! [[provider.fallbacks]]
//! name  = "ollama"
//! model = "llama3.2"
//! ```

use crate::config::ProviderConfig;
use crate::providers::{from_config, ChatCompletion, ChatMessage, Provider};
use crate::tools::traits::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// A provider that tries each of an ordered list of providers and returns the
/// first successful completion.
pub struct FailoverProvider {
    /// Ordered list of providers to try.  The first provider is tried first;
    /// remaining providers are used only if the previous ones fail.
    providers: Vec<(Arc<dyn Provider>, ProviderConfig)>,
}

impl FailoverProvider {
    /// Build a `FailoverProvider` from a primary config plus its list of
    /// `fallbacks`.
    pub fn from_config(primary: ProviderConfig) -> Result<Self> {
        let mut providers: Vec<(Arc<dyn Provider>, ProviderConfig)> = Vec::new();

        // Primary provider
        let primary_provider = from_config(&primary)?;
        providers.push((primary_provider, primary.clone()));

        // Fallback providers
        for fallback_cfg in &primary.fallbacks {
            let p = from_config(fallback_cfg)?;
            providers.push((p, fallback_cfg.clone()));
        }

        Ok(Self { providers })
    }
}

#[async_trait]
impl Provider for FailoverProvider {
    fn name(&self) -> &str {
        // Report the primary provider name.
        self.providers
            .first()
            .map(|(p, _)| p.name())
            .unwrap_or("failover")
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        _config: &ProviderConfig,
    ) -> Result<ChatCompletion> {
        let mut last_err: anyhow::Error = anyhow::anyhow!("No providers configured");

        for (provider, cfg) in &self.providers {
            match provider.chat_completion(messages, tools, cfg).await {
                Ok(completion) => {
                    return Ok(completion);
                }
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        model = cfg.model,
                        error = %e,
                        "Provider failed — trying next fallback"
                    );
                    last_err = e;
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
    use crate::config::ProviderConfig;

    #[test]
    fn failover_build_primary_only() {
        let cfg = ProviderConfig {
            name: "ollama".to_string(),
            model: "llama3.2".to_string(),
            fallbacks: vec![],
            ..Default::default()
        };
        let fp = FailoverProvider::from_config(cfg).unwrap();
        assert_eq!(fp.providers.len(), 1);
        assert_eq!(fp.name(), "ollama");
    }

    #[test]
    fn failover_build_with_fallbacks() {
        let cfg = ProviderConfig {
            name: "openai".to_string(),
            model: "gpt-4o".to_string(),
            fallbacks: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    model: "claude-3-5-sonnet-20241022".to_string(),
                    ..Default::default()
                },
                ProviderConfig {
                    name: "ollama".to_string(),
                    model: "llama3.2".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let fp = FailoverProvider::from_config(cfg).unwrap();
        assert_eq!(fp.providers.len(), 3);
        assert_eq!(fp.name(), "openai");
    }
}

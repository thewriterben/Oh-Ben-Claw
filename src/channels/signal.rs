//! Signal channel adapter via signal-cli JSON-RPC HTTP daemon.
//!
//! This adapter connects to a locally-running
//! [signal-cli](https://github.com/AsamK/signal-cli) instance that exposes a
//! JSON-RPC 2.0 HTTP endpoint.  signal-cli must be started in daemon mode:
//!
//! ```shell
//! signal-cli -a +1234567890 daemon --http localhost:8080
//! ```
//!
//! The adapter polls `receive` to retrieve new messages and uses `sendMessage`
//! to reply.
//!
//! # Setup
//!
//! ```toml
//! [channels.signal]
//! cli_url        = "http://localhost:8080"
//! phone_number   = "+1234567890"
//! allowed_numbers = ["+10987654321"]
//! ```

use crate::agent::Agent;
use crate::config::{ProviderConfig, SignalConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

// ── JSON-RPC types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a, P: Serialize> {
    jsonrpc: &'a str,
    method: &'a str,
    id: u64,
    params: P,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<R> {
    result: Option<R>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    message: String,
}

// ── Receive response ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReceiveResult {
    #[serde(default)]
    envelope: Option<SignalEnvelope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalEnvelope {
    source: Option<String>,
    data_message: Option<SignalDataMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalDataMessage {
    message: Option<String>,
}

// ── Send params ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SendParams<'a> {
    recipient: &'a str,
    message: &'a str,
    account: &'a str,
}

// ── SignalChannel ──────────────────────────────────────────────────────────────

/// Signal channel adapter.
pub struct SignalChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    config: SignalConfig,
    http: reqwest::Client,
    allowed: HashSet<String>,
    rpc_id: std::sync::atomic::AtomicU64,
}

impl SignalChannel {
    /// Create a new `SignalChannel`.
    ///
    /// Returns `None` if `cli_url` or `phone_number` are not set.
    pub fn new(
        config: &SignalConfig,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
    ) -> Option<Self> {
        if config.cli_url.is_none() || config.phone_number.is_none() {
            return None;
        }
        let allowed: HashSet<String> = config.allowed_numbers.iter().cloned().collect();
        Some(Self {
            agent,
            provider_config,
            config: config.clone(),
            http: reqwest::Client::new(),
            allowed,
            rpc_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Start the polling loop.
    pub async fn run(&self) -> Result<()> {
        let url = self.config.cli_url.as_deref().unwrap_or("http://localhost:8080");
        let account = self.config.phone_number.as_deref().unwrap_or("");
        let poll_secs = self.config.poll_interval_secs;

        tracing::info!(url, account, "Starting Signal channel adapter");

        let mut interval = tokio::time::interval(Duration::from_secs(poll_secs));

        loop {
            interval.tick().await;

            match self.receive_messages(url, account).await {
                Ok(envelopes) => {
                    for env in envelopes {
                        self.handle_envelope(url, account, env).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Signal: receive failed");
                }
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.rpc_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn receive_messages(
        &self,
        url: &str,
        account: &str,
    ) -> Result<Vec<SignalEnvelope>> {
        #[derive(Serialize)]
        struct ReceiveParams<'a> {
            account: &'a str,
            timeout: u64,
        }

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "receive",
            id: self.next_id(),
            params: ReceiveParams {
                account,
                timeout: 1,
            },
        };

        let resp: JsonRpcResponse<Vec<ReceiveResult>> = self
            .http
            .post(url)
            .json(&req)
            .send()
            .await
            .context("Signal: HTTP request failed")?
            .json()
            .await
            .context("Signal: JSON parse failed")?;

        if let Some(err) = resp.error {
            anyhow::bail!("Signal RPC error: {}", err.message);
        }

        let results = resp.result.unwrap_or_default();
        Ok(results
            .into_iter()
            .filter_map(|r| r.envelope)
            .collect())
    }

    async fn handle_envelope(&self, url: &str, account: &str, env: SignalEnvelope) {
        let sender = match env.source.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return,
        };

        // Allowlist check
        if !self.allowed.is_empty() && !self.allowed.contains(&sender) {
            tracing::debug!(sender, "Signal: sender not in allowlist — ignoring");
            return;
        }

        let text = match env
            .data_message
            .and_then(|d| d.message)
            .filter(|m| !m.is_empty())
        {
            Some(t) => t,
            None => return,
        };

        tracing::debug!(sender, text = %text, "Signal: received message");

        let session_id = format!("signal:{}", sender);
        match self.agent.process(&session_id, &text, &self.provider_config).await {
            Ok(response) => {
                if let Err(e) = self.send_message(url, account, &sender, &response.message).await {
                    tracing::error!(error = %e, "Signal: failed to send reply");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Signal: agent processing error");
                let _ = self
                    .send_message(
                        url,
                        account,
                        &sender,
                        "Sorry, I ran into an error. Please try again.",
                    )
                    .await;
            }
        }
    }

    async fn send_message(
        &self,
        url: &str,
        account: &str,
        recipient: &str,
        message: &str,
    ) -> Result<()> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "sendMessage",
            id: self.next_id(),
            params: SendParams {
                recipient,
                message,
                account,
            },
        };

        let resp = self
            .http
            .post(url)
            .json(&req)
            .send()
            .await
            .context("Signal: sendMessage HTTP request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("Signal: sendMessage returned status {}", resp.status());
        }

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SignalConfig;

    #[test]
    fn new_returns_none_without_config() {
        // No cli_url set.
        let cfg = SignalConfig::default();
        assert!(cfg.cli_url.is_none());
    }

    #[test]
    fn allowlist_empty_means_accept_all() {
        let cfg = SignalConfig {
            cli_url: Some("http://localhost:8080".into()),
            phone_number: Some("+1234567890".into()),
            allowed_numbers: vec![],
            poll_interval_secs: 2,
        };
        let allowed: HashSet<String> = cfg.allowed_numbers.iter().cloned().collect();
        // Empty allowlist → accept all
        assert!(allowed.is_empty());
    }

    #[test]
    fn allowlist_blocks_unknown_sender() {
        let allowed: HashSet<String> =
            ["+10987654321".to_string()].into_iter().collect();
        assert!(!allowed.contains("+19999999999"));
        assert!(allowed.contains("+10987654321"));
    }

    #[test]
    fn deserialize_envelope() {
        let json = r#"{"source":"+1234567890","dataMessage":{"message":"hello"}}"#;
        let env: SignalEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.source.as_deref(), Some("+1234567890"));
        assert_eq!(
            env.data_message.unwrap().message.as_deref(),
            Some("hello")
        );
    }
}

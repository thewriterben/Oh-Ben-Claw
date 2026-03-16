//! Oh-Ben-Claw Network Tunnel
//!
//! This module provides secure remote access to the Oh-Ben-Claw brain from
//! anywhere on the internet. Two tunnel backends are supported:
//!
//! - **Cloudflare Tunnel** (`cloudflared`): Zero-config HTTPS tunnel via
//!   Cloudflare's edge network. No port forwarding or firewall rules needed.
//!   Supports both quick tunnels (temporary URLs) and named tunnels (persistent
//!   custom domains).
//!
//! - **Tailscale**: Peer-to-peer WireGuard mesh VPN. Devices on the same
//!   Tailnet can reach the brain directly by hostname. Supports Tailscale
//!   Funnel for public HTTPS exposure.
//!
//! # Architecture
//!
//! ```text
//!   Mobile / Browser
//!        |
//!   [Cloudflare Edge]  or  [Tailscale Funnel]
//!        |
//!   [TunnelManager]  <-- manages child process lifecycle
//!        |
//!   [Axum Gateway]  <-- REST + WebSocket API on localhost
//!        |
//!   [Agent]  <-- core reasoning loop
//! ```
//!
//! # Quick Start
//!
//! ```toml
//! [tunnel]
//! enabled = true
//! backend = "cloudflare"
//! local_port = 8080
//! ```

pub mod cloudflare;
pub mod tailscale;

use crate::config::TunnelConfig;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// The public URL exposed by the tunnel (e.g., `https://abc123.trycloudflare.com`).
pub type TunnelUrl = String;

/// Status of the tunnel connection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    /// Tunnel is not running.
    #[default]
    Stopped,
    /// Tunnel is starting up.
    Starting,
    /// Tunnel is running and the public URL is available.
    Running { url: TunnelUrl },
    /// Tunnel encountered an error.
    Error { message: String },
}

impl std::fmt::Display for TunnelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped"),
            Self::Starting => write!(f, "starting"),
            Self::Running { url } => write!(f, "running at {url}"),
            Self::Error { message } => write!(f, "error: {message}"),
        }
    }
}

/// Which tunnel backend to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelBackend {
    Cloudflare,
    Tailscale,
}

impl std::fmt::Display for TunnelBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cloudflare => write!(f, "cloudflare"),
            Self::Tailscale => write!(f, "tailscale"),
        }
    }
}

/// Manages the lifecycle of the active tunnel process.
#[derive(Debug)]
pub struct TunnelManager {
    config: TunnelConfig,
    status: Arc<Mutex<TunnelStatus>>,
    handle: Arc<Mutex<Option<TunnelHandle>>>,
}

/// A handle to a running tunnel process.
#[derive(Debug)]
pub struct TunnelHandle {
    pub backend: TunnelBackend,
    pub url: TunnelUrl,
    pub child: tokio::process::Child,
}

impl TunnelManager {
    /// Create a new `TunnelManager` from config.
    pub fn new(config: TunnelConfig) -> Self {
        Self {
            config,
            status: Arc::new(Mutex::new(TunnelStatus::Stopped)),
            handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the tunnel. Returns the public URL once the tunnel is ready.
    pub async fn start(&self) -> Result<TunnelUrl> {
        let mut status = self.status.lock().await;
        if let TunnelStatus::Running { url } = &*status {
            return Ok(url.clone());
        }
        *status = TunnelStatus::Starting;
        drop(status);

        let backend = self.parse_backend()?;
        let local_port = self.config.local_port;

        let result = match backend {
            TunnelBackend::Cloudflare => {
                cloudflare::start_cloudflare_tunnel(local_port, &self.config).await
            }
            TunnelBackend::Tailscale => {
                tailscale::start_tailscale_funnel(local_port, &self.config).await
            }
        };

        match result {
            Ok((url, child)) => {
                let mut status = self.status.lock().await;
                *status = TunnelStatus::Running { url: url.clone() };
                let mut handle = self.handle.lock().await;
                *handle = Some(TunnelHandle {
                    backend,
                    url: url.clone(),
                    child,
                });
                tracing::info!(url = %url, "Tunnel started");
                Ok(url)
            }
            Err(e) => {
                let msg = e.to_string();
                let mut status = self.status.lock().await;
                *status = TunnelStatus::Error {
                    message: msg.clone(),
                };
                bail!("Failed to start tunnel: {msg}")
            }
        }
    }

    /// Stop the tunnel gracefully.
    pub async fn stop(&self) -> Result<()> {
        let mut handle = self.handle.lock().await;
        if let Some(mut h) = handle.take() {
            h.child
                .kill()
                .await
                .context("Failed to kill tunnel process")?;
            tracing::info!("Tunnel stopped");
        }
        let mut status = self.status.lock().await;
        *status = TunnelStatus::Stopped;
        Ok(())
    }

    /// Get the current tunnel status.
    pub async fn status(&self) -> TunnelStatus {
        self.status.lock().await.clone()
    }

    /// Get the current public URL if the tunnel is running.
    pub async fn url(&self) -> Option<TunnelUrl> {
        match &*self.status.lock().await {
            TunnelStatus::Running { url } => Some(url.clone()),
            _ => None,
        }
    }

    fn parse_backend(&self) -> Result<TunnelBackend> {
        match self.config.backend.to_lowercase().as_str() {
            "cloudflare" | "cloudflared" => Ok(TunnelBackend::Cloudflare),
            "tailscale" => Ok(TunnelBackend::Tailscale),
            other => bail!("Unknown tunnel backend: '{other}'. Use 'cloudflare' or 'tailscale'."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> TunnelConfig {
        TunnelConfig {
            enabled: true,
            backend: "cloudflare".to_string(),
            local_port: 8080,
            named_tunnel: None,
            token: None,
            tailscale_funnel: false,
        }
    }

    #[test]
    fn tunnel_status_display() {
        assert_eq!(TunnelStatus::Stopped.to_string(), "stopped");
        assert_eq!(TunnelStatus::Starting.to_string(), "starting");
        assert_eq!(
            TunnelStatus::Running {
                url: "https://abc.trycloudflare.com".to_string()
            }
            .to_string(),
            "running at https://abc.trycloudflare.com"
        );
        assert_eq!(
            TunnelStatus::Error {
                message: "no binary".to_string()
            }
            .to_string(),
            "error: no binary"
        );
    }

    #[test]
    fn tunnel_backend_display() {
        assert_eq!(TunnelBackend::Cloudflare.to_string(), "cloudflare");
        assert_eq!(TunnelBackend::Tailscale.to_string(), "tailscale");
    }

    #[test]
    fn parse_backend_cloudflare() {
        let config = default_config();
        let mgr = TunnelManager::new(config);
        assert_eq!(mgr.parse_backend().unwrap(), TunnelBackend::Cloudflare);
    }

    #[test]
    fn parse_backend_cloudflared_alias() {
        let mut config = default_config();
        config.backend = "cloudflared".to_string();
        let mgr = TunnelManager::new(config);
        assert_eq!(mgr.parse_backend().unwrap(), TunnelBackend::Cloudflare);
    }

    #[test]
    fn parse_backend_tailscale() {
        let mut config = default_config();
        config.backend = "tailscale".to_string();
        let mgr = TunnelManager::new(config);
        assert_eq!(mgr.parse_backend().unwrap(), TunnelBackend::Tailscale);
    }

    #[test]
    fn parse_backend_unknown_returns_error() {
        let mut config = default_config();
        config.backend = "ngrok".to_string();
        let mgr = TunnelManager::new(config);
        assert!(mgr.parse_backend().is_err());
    }

    #[tokio::test]
    async fn initial_status_is_stopped() {
        let mgr = TunnelManager::new(default_config());
        assert_eq!(mgr.status().await, TunnelStatus::Stopped);
    }

    #[tokio::test]
    async fn url_returns_none_when_stopped() {
        let mgr = TunnelManager::new(default_config());
        assert!(mgr.url().await.is_none());
    }
}

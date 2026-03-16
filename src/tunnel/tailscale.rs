//! Tailscale tunnel adapter.
//!
//! Supports two modes:
//!
//! - **Tailnet access**: The brain is reachable by its Tailscale hostname from
//!   any device on the same Tailnet. No extra configuration needed.
//!
//! - **Tailscale Funnel**: Exposes the gateway publicly over HTTPS via
//!   Tailscale's infrastructure. Requires Tailscale Funnel to be enabled on
//!   your account.
//!
//! # Prerequisites
//!
//! ```bash
//! # Install Tailscale
//! curl -fsSL https://tailscale.com/install.sh | sh
//!
//! # Authenticate
//! sudo tailscale up
//!
//! # Enable Funnel (optional, for public access)
//! sudo tailscale funnel 8080
//! ```

use crate::config::TunnelConfig;
use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Start a Tailscale Funnel or return the Tailnet hostname for private access.
///
/// Returns `(public_url, child_process)` when the tunnel is ready.
pub async fn start_tailscale_funnel(
    local_port: u16,
    config: &TunnelConfig,
) -> Result<(String, Child)> {
    // Verify tailscale is installed
    let check = tokio::process::Command::new("tailscale")
        .arg("version")
        .output()
        .await;

    if check.is_err() {
        bail!(
            "tailscale is not installed or not in PATH.\n\
             Install it from: https://tailscale.com/download"
        );
    }

    // Get the current Tailscale hostname for the Tailnet URL
    let tailnet_url = get_tailscale_url(local_port).await?;

    if config.tailscale_funnel {
        // Start Tailscale Funnel for public HTTPS access
        let mut cmd = Command::new("tailscale");
        cmd.arg("funnel").arg(local_port.to_string());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn tailscale funnel")?;

        // Parse the public URL from tailscale funnel output
        let stderr = child
            .stderr
            .take()
            .context("Failed to capture tailscale stderr")?;

        let url = tokio::time::timeout(
            Duration::from_secs(20),
            parse_tailscale_url(stderr, &tailnet_url),
        )
        .await
        .context("Timed out waiting for tailscale funnel to start (20s)")?
        .unwrap_or_else(|_| tailnet_url.clone());

        tracing::info!(url = %url, "Tailscale Funnel started");
        Ok((url, child))
    } else {
        // Private Tailnet access only — spawn a no-op process as the "handle"
        // since there's no daemon to manage; Tailscale is already running.
        let child = Command::new("tailscale")
            .arg("status")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn tailscale status")?;

        tracing::info!(url = %tailnet_url, "Tailscale Tailnet access ready");
        Ok((tailnet_url, child))
    }
}

/// Get the Tailscale URL for the local machine on the Tailnet.
async fn get_tailscale_url(port: u16) -> Result<String> {
    // `tailscale ip -4` returns the Tailscale IPv4 address
    let output = tokio::process::Command::new("tailscale")
        .arg("ip")
        .arg("-4")
        .output()
        .await
        .context("Failed to run 'tailscale ip -4'")?;

    if output.status.success() {
        let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !ip.is_empty() {
            return Ok(format!("http://{ip}:{port}"));
        }
    }

    // Fallback: try to get the Tailscale hostname
    let hostname_output = tokio::process::Command::new("tailscale")
        .arg("status")
        .arg("--json")
        .output()
        .await;

    if let Ok(out) = hostname_output {
        if out.status.success() {
            let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            if let Some(hostname) = json
                .get("Self")
                .and_then(|s| s.get("DNSName"))
                .and_then(|d| d.as_str())
            {
                let hostname = hostname.trim_end_matches('.');
                return Ok(format!("https://{hostname}:{port}"));
            }
        }
    }

    bail!("Could not determine Tailscale IP or hostname. Is Tailscale running? Run: sudo tailscale up")
}

/// Read tailscale funnel output to find the public URL.
async fn parse_tailscale_url(
    stderr: tokio::process::ChildStderr,
    fallback: &str,
) -> Result<String> {
    let reader = BufReader::new(stderr);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        tracing::debug!(tailscale = %line);
        if line.contains("https://") {
            if let Some(start) = line.find("https://") {
                let rest = &line[start..];
                let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
                let url = rest[..end].trim_end_matches([',', '.']);
                if url.len() > 8 {
                    return Ok(url.to_string());
                }
            }
        }
    }

    Ok(fallback.to_string())
}

/// Check whether `tailscale` is available in PATH.
pub async fn is_tailscale_available() -> bool {
    tokio::process::Command::new("tailscale")
        .arg("version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Return the installed Tailscale version string.
pub async fn tailscale_version() -> Option<String> {
    let output = tokio::process::Command::new("tailscale")
        .arg("version")
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_version_returns_option() {
        // This test just verifies the function signature compiles correctly.
        // Actual Tailscale availability depends on the host environment.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _version: Option<String> = rt.block_on(tailscale_version());
        // No assertion — tailscale may or may not be installed in CI
    }

    #[test]
    fn is_tailscale_available_returns_bool() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _available: bool = rt.block_on(is_tailscale_available());
        // No assertion — tailscale may or may not be installed in CI
    }
}

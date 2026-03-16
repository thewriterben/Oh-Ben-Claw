//! Cloudflare Tunnel adapter.
//!
//! Spawns `cloudflared tunnel --url http://localhost:{port}` as a child process
//! and parses the public URL from its stderr output.
//!
//! # Prerequisites
//!
//! Install `cloudflared`:
//! ```bash
//! # macOS
//! brew install cloudflare/cloudflare/cloudflared
//!
//! # Linux (Debian/Ubuntu)
//! curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb -o cloudflared.deb
//! sudo dpkg -i cloudflared.deb
//!
//! # Windows
//! winget install --id Cloudflare.cloudflared
//! ```
//!
//! # Named Tunnels
//!
//! For persistent custom domains, create a named tunnel first:
//! ```bash
//! cloudflared tunnel login
//! cloudflared tunnel create oh-ben-claw
//! cloudflared tunnel route dns oh-ben-claw obc.yourdomain.com
//! ```
//! Then set `named_tunnel = "oh-ben-claw"` and `token = "..."` in config.

use crate::config::TunnelConfig;
use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Start a Cloudflare quick tunnel or named tunnel.
///
/// Returns `(public_url, child_process)` when the tunnel is ready.
pub async fn start_cloudflare_tunnel(
    local_port: u16,
    config: &TunnelConfig,
) -> Result<(String, Child)> {
    // Verify cloudflared is installed
    let check = tokio::process::Command::new("cloudflared")
        .arg("--version")
        .output()
        .await;

    if check.is_err() {
        bail!(
            "cloudflared is not installed or not in PATH.\n\
             Install it from: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
        );
    }

    let mut cmd = Command::new("cloudflared");
    cmd.arg("tunnel");
    cmd.arg("--no-autoupdate");

    if let Some(token) = &config.token {
        // Named tunnel with token (persistent, custom domain)
        cmd.arg("run").arg("--token").arg(token);
        if let Some(name) = &config.named_tunnel {
            cmd.arg(name);
        }
    } else {
        // Quick tunnel (temporary URL, no auth required)
        cmd.arg("--url")
            .arg(format!("http://localhost:{local_port}"));
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn cloudflared process")?;

    // Parse the public URL from cloudflared's stderr output.
    // cloudflared prints: "Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  https://xxx.trycloudflare.com"
    let stderr = child
        .stderr
        .take()
        .context("Failed to capture cloudflared stderr")?;

    let url = tokio::time::timeout(Duration::from_secs(30), parse_cloudflare_url(stderr))
        .await
        .context("Timed out waiting for cloudflared to start (30s)")?
        .context("Failed to parse cloudflared URL from output")?;

    Ok((url, child))
}

/// Read cloudflared stderr line by line until we find the public URL.
async fn parse_cloudflare_url(stderr: tokio::process::ChildStderr) -> Result<String> {
    let reader = BufReader::new(stderr);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        tracing::debug!(cloudflared = %line);

        // Quick tunnel URL pattern
        if line.contains("trycloudflare.com") || line.contains("https://") {
            if let Some(url) = extract_https_url(&line) {
                return Ok(url);
            }
        }

        // Named tunnel ready pattern
        if line.contains("Registered tunnel connection") || line.contains("Connection") {
            if let Some(url) = extract_https_url(&line) {
                return Ok(url);
            }
        }

        // Error patterns
        if line.contains("failed to") || line.contains("error") || line.contains("ERR") {
            tracing::warn!(cloudflared_error = %line);
        }
    }

    bail!("cloudflared process exited without providing a URL")
}

/// Extract the first `https://` URL from a string.
fn extract_https_url(s: &str) -> Option<String> {
    let start = s.find("https://")?;
    let rest = &s[start..];
    // URL ends at first whitespace or end of string
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches([',', '.', ')']);
    if url.len() > 8 {
        Some(url.to_string())
    } else {
        None
    }
}

/// Check whether `cloudflared` is available in PATH.
pub async fn is_cloudflared_available() -> bool {
    tokio::process::Command::new("cloudflared")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Return the installed `cloudflared` version string.
pub async fn cloudflared_version() -> Option<String> {
    let output = tokio::process::Command::new("cloudflared")
        .arg("--version")
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    combined.lines().next().map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_from_quick_tunnel_line() {
        let line = "Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  https://abc-def-123.trycloudflare.com";
        let url = extract_https_url(line).unwrap();
        assert_eq!(url, "https://abc-def-123.trycloudflare.com");
    }

    #[test]
    fn extract_url_with_trailing_punctuation() {
        let line = "Tunnel ready at https://example.trycloudflare.com.";
        let url = extract_https_url(line).unwrap();
        assert_eq!(url, "https://example.trycloudflare.com");
    }

    #[test]
    fn extract_url_returns_none_for_plain_text() {
        let line = "Starting cloudflared tunnel...";
        assert!(extract_https_url(line).is_none());
    }

    #[test]
    fn extract_url_from_named_tunnel_line() {
        let line = "Registered tunnel connection connIndex=0 ip=198.41.200.13 location=LAX url=https://obc.example.com";
        // Named tunnels may not have https in the log line — this tests the fallback
        let url = extract_https_url(line);
        assert!(url.is_some());
        assert!(url.unwrap().starts_with("https://"));
    }
}

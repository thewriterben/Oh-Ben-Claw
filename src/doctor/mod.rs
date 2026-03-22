//! System diagnostics for Oh-Ben-Claw.
//!
//! The `doctor` command performs a series of checks on the configuration and
//! environment, then prints a human-readable health report.

use crate::config::Config;
use serde::Serialize;
use std::net::ToSocketAddrs;

/// Diagnostic severity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warn,
    Error,
}

/// A single diagnostic result item.
#[derive(Debug, Clone, Serialize)]
pub struct DiagResult {
    pub severity: Severity,
    pub category: String,
    pub message: String,
}

impl DiagResult {
    fn ok(category: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Ok,
            category: category.to_string(),
            message: message.into(),
        }
    }
    fn warn(category: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warn,
            category: category.to_string(),
            message: message.into(),
        }
    }
    fn error(category: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            category: category.to_string(),
            message: message.into(),
        }
    }
}

/// Run all diagnostic checks and return the results.
pub fn diagnose(config: &Config) -> Vec<DiagResult> {
    let mut results = Vec::new();

    // ── Config semantics ─────────────────────────────────────────────────────
    let api_key_set = config
        .provider
        .api_key
        .as_deref()
        .map(|k| !k.is_empty())
        .unwrap_or(false)
        || match config.provider.name.as_str() {
            "openai" => std::env::var("OPENAI_API_KEY")
                .map(|v| !v.is_empty())
                .unwrap_or(false),
            "anthropic" => std::env::var("ANTHROPIC_API_KEY")
                .map(|v| !v.is_empty())
                .unwrap_or(false),
            "openrouter" => std::env::var("OPENROUTER_API_KEY")
                .map(|v| !v.is_empty())
                .unwrap_or(false),
            "ollama" => true, // no key needed
            _ => false,
        };

    if api_key_set {
        results.push(DiagResult::ok("config", "Provider API key is set"));
    } else if config.provider.name == "ollama" {
        results.push(DiagResult::ok(
            "config",
            "Ollama provider — no API key needed",
        ));
    } else {
        results.push(DiagResult::warn(
            "config",
            format!(
                "No API key found for provider '{}' — set it in config or environment",
                config.provider.name
            ),
        ));
    }

    if config.agent.system_prompt.trim().is_empty() {
        results.push(DiagResult::error("config", "Agent system prompt is empty"));
    } else {
        results.push(DiagResult::ok("config", "Agent system prompt is set"));
    }

    if config.agent.name.trim().is_empty() {
        results.push(DiagResult::error("config", "Agent name is empty"));
    } else {
        results.push(DiagResult::ok(
            "config",
            format!("Agent name: '{}'", config.agent.name),
        ));
    }

    // ── Environment ──────────────────────────────────────────────────────────
    if std::env::var("RUST_LOG").is_ok() {
        results.push(DiagResult::ok("environment", "RUST_LOG is set"));
    } else {
        results.push(DiagResult::warn(
            "environment",
            "RUST_LOG not set — consider setting it for debugging",
        ));
    }

    let home_set = std::env::var("HOME").is_ok() || std::env::var("USERPROFILE").is_ok();
    if home_set {
        results.push(DiagResult::ok(
            "environment",
            "HOME/USERPROFILE is available",
        ));
    } else {
        results.push(DiagResult::warn("environment", "HOME/USERPROFILE not set"));
    }

    // ── Workspace ────────────────────────────────────────────────────────────
    match crate::config::Config::default_config_path() {
        Ok(path) => {
            let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            if dir.exists() {
                results.push(DiagResult::ok(
                    "workspace",
                    format!("Config directory exists: {}", dir.display()),
                ));
            } else {
                results.push(DiagResult::warn(
                    "workspace",
                    format!("Config directory not found: {}", dir.display()),
                ));
            }
        }
        Err(e) => {
            results.push(DiagResult::error(
                "workspace",
                format!("Cannot determine config path: {e}"),
            ));
        }
    }

    // ── Channels ─────────────────────────────────────────────────────────────
    if let Some(ref token) = config.channels.telegram.token {
        if token.is_empty() {
            results.push(DiagResult::warn(
                "channels",
                "Telegram token is set but empty",
            ));
        } else {
            results.push(DiagResult::ok("channels", "Telegram token is configured"));
        }
    }

    if let Some(ref token) = config.channels.discord.token {
        if token.is_empty() {
            results.push(DiagResult::warn(
                "channels",
                "Discord token is set but empty",
            ));
        } else {
            results.push(DiagResult::ok("channels", "Discord token is configured"));
        }
    }

    if let Some(ref token) = config.channels.slack.app_token {
        if token.is_empty() {
            results.push(DiagResult::warn(
                "channels",
                "Slack app token is set but empty",
            ));
        } else {
            results.push(DiagResult::ok("channels", "Slack app token is configured"));
        }
    }

    // ── Spine ────────────────────────────────────────────────────────────────
    if config.spine.kind == "mqtt" || config.spine.kind == "p2p" {
        let host = &config.spine.host;
        let addr = format!("{}:80", host);
        match addr.to_socket_addrs() {
            Ok(_) => {
                results.push(DiagResult::ok(
                    "spine",
                    format!("Spine host '{}' resolved OK", host),
                ));
            }
            Err(_) => {
                results.push(DiagResult::warn(
                    "spine",
                    format!("Spine host '{}' could not be resolved (DNS check)", host),
                ));
            }
        }
    }

    results
}

/// Run the doctor check and print a human-readable report to stdout.
///
/// Always returns `Ok(())` — errors in individual checks are shown in the report,
/// not propagated as `Err`.
pub fn run(config: &Config) -> anyhow::Result<()> {
    let results = diagnose(config);

    println!("\n🩺 Oh-Ben-Claw Doctor\n");

    // Collect unique ordered categories
    let mut seen = std::collections::HashSet::new();
    let categories: Vec<String> = results
        .iter()
        .map(|r| r.category.clone())
        .filter(|c| seen.insert(c.clone()))
        .collect();

    for cat in &categories {
        println!("  📂 {}:", cat);
        for r in results.iter().filter(|r| &r.category == cat) {
            let icon = match r.severity {
                Severity::Ok => "✅",
                Severity::Warn => "⚠️ ",
                Severity::Error => "❌",
            };
            println!("     {} {}", icon, r.message);
        }
        println!();
    }

    let errors = results
        .iter()
        .filter(|r| r.severity == Severity::Error)
        .count();
    let warnings = results
        .iter()
        .filter(|r| r.severity == Severity::Warn)
        .count();
    println!(
        "  Summary: {} error(s), {} warning(s), {} ok\n",
        errors,
        warnings,
        results
            .iter()
            .filter(|r| r.severity == Severity::Ok)
            .count()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn diagnose_returns_results_for_default_config() {
        let config = Config::default();
        let results = diagnose(&config);
        assert!(!results.is_empty());
    }

    #[test]
    fn diagnose_checks_agent_name() {
        let mut config = Config::default();
        config.agent.name = String::new();
        let results = diagnose(&config);
        let has_name_error = results
            .iter()
            .any(|r| r.severity == Severity::Error && r.message.contains("name"));
        assert!(has_name_error);
    }

    #[test]
    fn diagnose_checks_system_prompt() {
        let mut config = Config::default();
        config.agent.system_prompt = String::new();
        let results = diagnose(&config);
        let has_prompt_error = results
            .iter()
            .any(|r| r.severity == Severity::Error && r.message.contains("system prompt"));
        assert!(has_prompt_error);
    }

    #[test]
    fn run_returns_ok() {
        let config = Config::default();
        assert!(run(&config).is_ok());
    }
}

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

    // ── Subsystem suites & safing coherence ──────────────────────────────────
    check_subsystems(config, &mut results);
    check_hardware_onboarding(config, &mut results);

    results
}

/// Track 0 onboarding hygiene: every configured peripheral board should be a known,
/// trusted vendor. A board the registry doesn't recognize has no capability data and
/// an unverified vendor — flagged so an unknown/typo'd board isn't silently trusted.
fn check_hardware_onboarding(config: &Config, results: &mut Vec<DiagResult>) {
    use crate::peripherals::onboarding::{OnboardDecision, VendorAllowlist};
    use crate::peripherals::registry::known_boards;

    if !config.peripherals.enabled || config.peripherals.boards.is_empty() {
        return;
    }
    let allow = VendorAllowlist::from_known_boards();
    for b in &config.peripherals.boards {
        match known_boards().iter().find(|kb| kb.name == b.board.as_str()) {
            Some(kb) => match allow.decide(kb.vid) {
                OnboardDecision::AutoTrust => results.push(DiagResult::ok(
                    "hardware",
                    format!("Board '{}' vendor '{}' is trusted", b.board, kb.vendor),
                )),
                OnboardDecision::Quarantine => results.push(DiagResult::warn(
                    "hardware",
                    format!(
                        "Board '{}' vendor {:#06x} is not allowlisted — quarantine",
                        b.board, kb.vid
                    ),
                )),
            },
            None => results.push(DiagResult::warn(
                "hardware",
                format!(
                    "Board '{}' is not in the hardware registry — vendor unverified, no capability data",
                    b.board
                ),
            )),
        }
    }
}

/// Validate that the capability suites and the safing layer are configured
/// coherently, and report the active capability surface. These catch the silent
/// "enabled but skipped" cases `main` only logs at startup.
fn check_subsystems(config: &Config, results: &mut Vec<DiagResult>) {
    // Movement is physical — it requires the Track 0 safety gate, or `main`
    // refuses to expose the tool.
    if config.movement.enabled && !config.safety.enabled {
        results.push(DiagResult::error(
            "subsystems",
            "[movement] enabled but [safety] is off — movement is physical and requires \
             deterministic Track 0 limits; the move_actuator tool will be skipped",
        ));
    }

    // The suites and the reflex loop record/read world memory; without it they
    // are skipped at startup.
    let world = config.perception.world_memory;
    for (enabled, name) in [
        (config.sensing.enabled, "sensing"),
        (config.audio_suite.enabled, "audio_suite"),
        (config.power.enabled, "power"),
        (config.comms.enabled, "comms"),
    ] {
        if enabled && !world {
            results.push(DiagResult::error(
                "subsystems",
                format!(
                    "[{name}] enabled but [perception].world_memory is off — the suite needs \
                     world memory to record/query and will be skipped"
                ),
            ));
        }
    }
    if config.reflex.enabled && !world {
        results.push(DiagResult::error(
            "subsystems",
            "[reflex] enabled but [perception].world_memory is off — reflexes read world memory \
             and will not run",
        ));
    }

    // Safing depends on the reflex loop running.
    if config.reflex.safing && !config.reflex.enabled {
        results.push(DiagResult::warn(
            "subsystems",
            "[reflex] safing = true but the reflex loop is disabled — no safing rules will run",
        ));
    }
    // A power-critical stop actuator needs the movement subsystem to actuate it.
    if config.reflex.safing
        && config.reflex.safing_stop_actuator.is_some()
        && !config.movement.enabled
    {
        results.push(DiagResult::warn(
            "subsystems",
            "[reflex.safing_stop_actuator] set but [movement] is disabled — the power-critical \
             Stop will be a no-op",
        ));
    }

    // Local TTS rendering needs an API key, else speech is silently skipped.
    if config.audio_suite.enabled && config.audio_suite.render_tts {
        let key = std::env::var("OPENAI_API_KEY").map(|v| !v.is_empty()).unwrap_or(false);
        if !key {
            results.push(DiagResult::warn(
                "subsystems",
                "[audio_suite] render_tts = true but OPENAI_API_KEY is not set — speech renders \
                 will be skipped (best-effort)",
            ));
        }
    }

    // Report the active capability surface.
    let mut active: Vec<&str> = Vec::new();
    if config.sensing.enabled {
        active.push("sensing");
    }
    if config.audio_suite.enabled {
        active.push("audio");
    }
    if config.power.enabled {
        active.push("power");
    }
    if config.comms.enabled {
        active.push("comms");
    }
    if config.movement.enabled {
        active.push("movement");
    }
    if active.is_empty() {
        results.push(DiagResult::ok(
            "subsystems",
            "No capability suites enabled",
        ));
    } else {
        results.push(DiagResult::ok(
            "subsystems",
            format!("Active suites: {}", active.join(", ")),
        ));
    }
    if config.reflex.enabled {
        let safing = if config.reflex.safing { " + safing" } else { "" };
        results.push(DiagResult::ok(
            "subsystems",
            format!("Reflex loop enabled ({} rules{safing})", config.reflex.rules.len()),
        ));
    }
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

    #[test]
    fn hardware_onboarding_flags_unknown_boards() {
        use crate::config::PeripheralBoardConfig;
        let board = |name: &str| PeripheralBoardConfig {
            board: name.to_string(),
            transport: "serial".to_string(),
            path: None,
            baud: 115_200,
            node_id: None,
        };
        let mut config = Config::default();
        config.peripherals.enabled = true;
        config.peripherals.boards = vec![board("esp32-c3"), board("frobnicator-9000")];
        let results = diagnose(&config);
        // a registry board is a trusted vendor
        assert!(results.iter().any(|r| r.severity == Severity::Ok
            && r.message.contains("esp32-c3")
            && r.message.contains("trusted")));
        // an unrecognized board is flagged
        assert!(results.iter().any(|r| r.severity == Severity::Warn
            && r.message.contains("frobnicator-9000")
            && r.message.contains("not in the hardware registry")));
    }

    #[test]
    fn movement_without_safety_is_error() {
        let mut config = Config::default();
        config.movement.enabled = true;
        config.safety.enabled = false;
        let results = diagnose(&config);
        assert!(results.iter().any(|r| r.severity == Severity::Error
            && r.category == "subsystems"
            && r.message.contains("[movement]")));
    }

    #[test]
    fn suite_without_world_memory_is_error() {
        let mut config = Config::default();
        config.power.enabled = true;
        config.perception.world_memory = false;
        let results = diagnose(&config);
        assert!(results.iter().any(|r| r.severity == Severity::Error
            && r.category == "subsystems"
            && r.message.contains("[power]")));
    }

    #[test]
    fn safing_without_reflex_is_warning() {
        let mut config = Config::default();
        config.reflex.safing = true;
        config.reflex.enabled = false;
        let results = diagnose(&config);
        assert!(results.iter().any(|r| r.severity == Severity::Warn
            && r.category == "subsystems"
            && r.message.contains("safing")));
    }

    #[test]
    fn active_suites_are_reported() {
        let mut config = Config::default();
        config.perception.world_memory = true;
        config.sensing.enabled = true;
        config.comms.enabled = true;
        let results = diagnose(&config);
        assert!(results.iter().any(|r| r.severity == Severity::Ok
            && r.category == "subsystems"
            && r.message.contains("Active suites")
            && r.message.contains("sensing")
            && r.message.contains("comms")));
    }
}

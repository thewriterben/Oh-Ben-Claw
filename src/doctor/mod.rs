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
    // `api_key_set` means exactly what it says: a key is present. Ollama is
    // deliberately *not* folded in here — see the branch below.
    let api_key_set = config
        .provider
        .api_key
        .as_ref()
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
            _ => false,
        };

    // Ollama first, and that ordering is the whole fix.
    //
    // Until 2026-08-02 the match above had an `"ollama" => true, // no key
    // needed` arm, which set `api_key_set` and sent Ollama down the first
    // branch — so `doctor` printed **"Provider API key is set"** on a machine
    // with no key anywhere, and the accurate message in the branch below was
    // unreachable. Measured on `bodies/benchtop` with no provider variables in
    // the environment: the config loader warned "no API key is set" and two
    // lines later doctor called it ok.
    //
    // It is a small lie in the one command whose entire job is telling an
    // operator the truth about their setup, on the path a newcomer takes —
    // Ollama is the documented fallback for someone who has no key at all.
    if config.provider.name == "ollama" {
        results.push(ollama_reachability(&config.provider));
    } else if api_key_set {
        results.push(DiagResult::ok("config", "Provider API key is set"));
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
    // "Where does this instance keep its data" is the first question anyone asks
    // when two agents share a machine, and until now doctor could not answer it —
    // it reported only whether the *platform* config directory happened to exist,
    // which it warned about even when a config had been loaded from elsewhere.
    let root = crate::config::paths::data_dir();
    let source =
        if std::env::var(crate::config::paths::DATA_DIR_VAR).is_ok_and(|v| !v.trim().is_empty()) {
            "OBC_DATA_DIR"
        } else if config.paths.data_dir.is_some() {
            "[paths].data_dir"
        } else {
            "platform default"
        };
    results.push(DiagResult::ok(
        "workspace",
        format!("Data root: {} ({source})", root.display()),
    ));
    if !root.exists() {
        results.push(DiagResult::warn(
            "workspace",
            format!(
                "Data root does not exist yet: {} — it is created on first write",
                root.display()
            ),
        ));
    }

    match crate::config::Config::default_config_path() {
        Ok(path) => {
            let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            if dir.exists() {
                results.push(DiagResult::ok(
                    "workspace",
                    format!("Platform config directory exists: {}", dir.display()),
                ));
            } else {
                // Not a warning. There are four places a config may legitimately
                // live, and this one being absent says nothing about whether a
                // config was found — which is what the reader actually wants to
                // know, and what the startup log already tells them.
                results.push(DiagResult::ok(
                    "workspace",
                    format!(
                        "No config in the platform directory ({}); the search also covers --config/OBC_CONFIG, the data root, and ~/.oh-ben-claw",
                        dir.display()
                    ),
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

/// Is the Ollama a fallback would reach actually listening?
///
/// Added 2026-08-02. The branch above used to push a bare
/// `ok("Ollama provider — no API key needed")` derived from nothing but the
/// provider *name*. True as far as it went, and reassuring about the wrong
/// thing: on a machine with no Ollama installed, `doctor` reported a green for
/// a provider that was not there, and the first model call failed.
///
/// That matters most on exactly the path the comment above names. Ollama is the
/// documented fallback for someone who has no API key at all — which is also
/// the person most likely not to have it running yet. Two fixes deep, this is
/// the same defect each time: the command whose job is telling an operator the
/// truth about their setup, reporting something it had not checked.
///
/// A TCP connect rather than an HTTP request: it is enough to distinguish
/// "nothing is listening" from "something is", needs no async in a sync
/// function and no client, and cannot hang past the timeout. It deliberately
/// does *not* claim the model named in the config is pulled — that is a
/// stronger claim needing a real request, and an unchecked version of it is
/// how this function came to exist.
fn ollama_reachability(provider: &crate::config::ProviderConfig) -> DiagResult {
    const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(400);

    let raw = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:11434/api/chat".to_string());

    // Enough URL parsing to get host:port, without adding a URL crate to a
    // diagnostic. Anything unparseable is reported as unparseable rather than
    // guessed at.
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .unwrap_or(&raw);
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) => (h.to_string(), p),
            Err(_) => (authority.to_string(), 11434),
        },
        None => (authority.to_string(), 11434),
    };
    if host.is_empty() {
        return DiagResult::warn(
            "config",
            format!("Provider is Ollama but base_url '{raw}' has no host to check"),
        );
    }

    let addrs = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => {
            return DiagResult::warn(
                "config",
                format!("Provider is Ollama but '{host}' does not resolve"),
            )
        }
    };

    let up = addrs
        .iter()
        .any(|a| std::net::TcpStream::connect_timeout(a, TIMEOUT).is_ok());

    if up {
        DiagResult::ok(
            "config",
            format!("Ollama is listening at {host}:{port} — no API key needed"),
        )
    } else {
        DiagResult::warn(
            "config",
            format!(
                "Provider is Ollama (no API key needed) but nothing is listening at \
                 {host}:{port} — start it with `ollama serve`, or set \
                 ANTHROPIC_API_KEY / OPENAI_API_KEY / OPENROUTER_API_KEY to use a \
                 cloud provider instead"
            ),
        )
    }
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
        let key = std::env::var("OPENAI_API_KEY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
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
        results.push(DiagResult::ok("subsystems", "No capability suites enabled"));
    } else {
        results.push(DiagResult::ok(
            "subsystems",
            format!("Active suites: {}", active.join(", ")),
        ));
    }
    if config.reflex.enabled {
        let safing = if config.reflex.safing {
            " + safing"
        } else {
            ""
        };
        results.push(DiagResult::ok(
            "subsystems",
            format!(
                "Reflex loop enabled ({} rules{safing})",
                config.reflex.rules.len()
            ),
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

    /// `doctor` exists to tell an operator the truth about their setup, and
    /// until 2026-08-02 it told the newcomer path a small lie: with Ollama —
    /// the documented fallback for someone who has no key at all — it printed
    /// "Provider API key is set" on a machine with no key anywhere.
    ///
    /// Caught by running it against `bodies/benchtop`, whose README cites this
    /// command's output as its evidence, in a shell with no provider variables.
    #[test]
    fn ollama_is_not_reported_as_having_an_api_key() {
        let mut config = Config::default();
        config.provider.name = "ollama".to_string();
        config.provider.api_key = None;

        let results = diagnose(&config);
        let config_msgs: Vec<&str> = results
            .iter()
            .filter(|r| r.category == "config")
            .map(|r| r.message.as_str())
            .collect();

        assert!(
            !config_msgs.iter().any(|m| m.contains("API key is set")),
            "doctor claimed a key is set for a provider that needs none: {config_msgs:?}"
        );
        assert!(
            config_msgs.iter().any(|m| m.contains("no API key needed")),
            "the accurate Ollama message is unreachable again: {config_msgs:?}"
        );
        // Which *branch* produces that phrase depends on whether this machine
        // happens to have Ollama running — ok when it does, warn when it does
        // not. Deliberate: both messages carry "no API key needed", because
        // that fact is true either way and is what this test is about. The two
        // tests below pin the severities, on ports they control.
    }

    /// A green for Ollama must mean something is listening, not just that the
    /// config says "ollama".
    ///
    /// Bound to a port and then dropped, so the address is real, routable and
    /// almost certainly free — which is a sharper probe than picking a number
    /// and hoping. A test that passed because 59999 happened to be closed would
    /// be testing the machine, not the code.
    #[test]
    fn ollama_that_is_not_running_is_a_warning_not_an_ok() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let mut config = Config::default();
        config.provider.name = "ollama".to_string();
        config.provider.base_url = Some(format!("http://{addr}/api/chat"));

        let results = diagnose(&config);
        let hit = results
            .iter()
            .find(|r| r.category == "config" && r.message.contains("Ollama"))
            .expect("doctor said nothing at all about the Ollama provider");

        assert_eq!(
            hit.severity,
            Severity::Warn,
            "doctor reported a green for an Ollama that is not there: {}",
            hit.message
        );
        assert!(
            hit.message.contains("nothing is listening"),
            "the warning does not say what is wrong: {}",
            hit.message
        );
    }

    /// And the other side, so the fix cannot be "always warn about Ollama".
    #[test]
    fn ollama_that_is_running_is_reported_ok() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let mut config = Config::default();
        config.provider.name = "ollama".to_string();
        config.provider.base_url = Some(format!("http://{addr}/api/chat"));

        let results = diagnose(&config);
        let hit = results
            .iter()
            .find(|r| r.category == "config" && r.message.contains("Ollama"))
            .expect("doctor said nothing at all about the Ollama provider");

        assert_eq!(hit.severity, Severity::Ok, "{}", hit.message);
        assert!(hit.message.contains("is listening"), "{}", hit.message);
        assert!(
            hit.message.contains("no API key needed"),
            "the reason a key is not needed got lost: {}",
            hit.message
        );
        drop(listener);
    }

    /// An unparseable base_url is reported as unparseable, not silently
    /// defaulted to localhost and then reported on.
    #[test]
    fn a_base_url_with_no_host_is_not_guessed_at() {
        let mut config = Config::default();
        config.provider.name = "ollama".to_string();
        config.provider.base_url = Some("http:///api/chat".to_string());

        let results = diagnose(&config);
        let hit = results
            .iter()
            .find(|r| r.category == "config" && r.message.contains("Ollama"))
            .expect("doctor said nothing at all about the Ollama provider");

        assert_eq!(hit.severity, Severity::Warn, "{}", hit.message);
        assert!(hit.message.contains("no host"), "{}", hit.message);
    }

    /// The complement, so the fix cannot be "never say a key is set".
    #[test]
    fn a_configured_key_is_still_reported() {
        let mut config = Config::default();
        config.provider.name = "anthropic".to_string();
        config.provider.api_key = Some("sk-ant-not-a-real-key".into());

        let results = diagnose(&config);
        assert!(
            results
                .iter()
                .any(|r| r.severity == Severity::Ok && r.message.contains("API key is set")),
            "a provider with a key in config should report it"
        );
    }

    /// And a provider that needs a key and has none must still warn, or the
    /// ordering change would have turned a real problem into silence.
    #[test]
    fn a_missing_key_still_warns() {
        let mut config = Config::default();
        config.provider.name = "anthropic".to_string();
        config.provider.api_key = None;

        // Only meaningful when the ambient environment has no key either; if a
        // developer has one exported, skip rather than assert something false.
        if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|v| !v.is_empty()) {
            return;
        }

        let results = diagnose(&config);
        assert!(
            results
                .iter()
                .any(|r| r.severity == Severity::Warn && r.message.contains("No API key found")),
            "a provider that needs a key and has none must warn"
        );
    }

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

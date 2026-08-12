//! Oh-Ben-Claw — Advanced multi-device AI assistant.
//!
//! # Usage
//!
//! ```bash
//! # Start the interactive CLI
//! oh-ben-claw start
//!
//! # Start with a specific provider
//! oh-ben-claw start --provider openai --model gpt-4o
//!
//! # Start with Ollama (local)
//! oh-ben-claw start --provider ollama --model llama3.2
//!
//! # Check system status
//! oh-ben-claw status
//!
//! # Manage peripheral nodes
//! oh-ben-claw peripheral list
//! oh-ben-claw peripheral add esp32-s3 /dev/ttyUSB0
//!
//! # Manage conversation history
//! oh-ben-claw history list
//! oh-ben-claw history clear
//! ```

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

use oh_ben_claw::agent::{Agent, AgentHandle, OrchestratorAgent};
use oh_ben_claw::channels::{
    CliChannel, DiscordChannel, FeishuChannel, IMessageChannel, IrcChannel, MatrixChannel,
    MattermostChannel, SignalChannel, SlackChannel, TelegramChannel, WhatsAppChannel,
};
use oh_ben_claw::config::Config;
use oh_ben_claw::memory::MemoryStore;
use oh_ben_claw::tools::default_tools;
use oh_ben_claw::{
    config, gateway, observability, peripherals, providers, scheduler, security, spine, tunnel,
};

/// Oh-Ben-Claw — Advanced multi-device AI assistant.
#[derive(Parser, Debug)]
#[command(name = "oh-ben-claw")]
#[command(author = "thewriterben")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Advanced multi-device AI assistant with distributed peripheral nodes.", long_about = None)]
struct Cli {
    /// Path to a config file (overrides the default location and the
    /// OBC_CONFIG env var). E.g. `oh-ben-claw start --config bench-config.toml`.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the Oh-Ben-Claw agent (interactive CLI, gateway, and tunnel).
    Start {
        /// LLM provider name (openai, anthropic, ollama, openrouter, or any compatible).
        #[arg(long)]
        provider: Option<String>,
        /// Model name (e.g., gpt-4o, claude-3-5-sonnet-20241022, llama3.2).
        #[arg(long)]
        model: Option<String>,
        /// Session ID for conversation history (default: "default").
        #[arg(long, default_value = "default")]
        session: String,
        /// Skip connecting to the MQTT spine.
        #[arg(long)]
        no_spine: bool,
        /// Start the REST/WebSocket gateway (overrides config).
        #[arg(long)]
        gateway: bool,
        /// Start the network tunnel (overrides config).
        #[arg(long)]
        tunnel: bool,
    },

    /// Check the status of the agent and all connected peripheral nodes.
    Status,

    /// Manage peripheral hardware nodes.
    #[command(subcommand)]
    Peripheral(PeripheralCommands),

    /// Manage conversation history.
    #[command(subcommand)]
    History(HistoryCommands),

    /// Manage learned skills and their Track 0 staged rollout.
    #[command(subcommand)]
    Skill(SkillCommands),

    /// Run the MCP server standalone (stdio for host integration, http for
    /// gateways and the official conformance suite).
    McpServe {
        /// Transport: "stdio" or "http".
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for the http transport.
        #[arg(long, default_value = "3411")]
        port: u16,
        /// Protocol mode: "legacy-2024" (headers optional) or
        /// "stateless-2026" (2026-07-28 routing headers required).
        #[arg(long, default_value = "legacy-2024")]
        mode: String,
    },

    /// Serve this agent over the A2A protocol: the well-known agent card and
    /// the JSON-RPC endpoint other agents call.
    A2aServe {
        /// Port to bind.
        #[arg(long, default_value = "4311")]
        port: u16,
        /// The base URL to advertise in the agent card's interface entry.
        /// Other agents call this, so it must be reachable from where they run
        /// — not necessarily the bind address.
        #[arg(long)]
        advertise: Option<String>,
        /// Serve the protocol without a model: `SendMessage` completes and
        /// echoes the message back. For conformance testing the wire shape.
        #[arg(long)]
        echo: bool,
    },

    /// Measure LLM-judge calibration against a gold label set (Cohen's κ).
    /// The judge is configured via `OBC_JUDGE_PROVIDER`/`OBC_JUDGE_MODEL`
    /// (+ optional `OBC_JUDGE_API_KEY`/`OBC_JUDGE_BASE_URL`).
    JudgeCalibrate {
        /// Path to a JSON gold set (array of {task, response, human}). When
        /// omitted, falls back to `OBC_JUDGE_GOLD`, then the built-in seed set.
        #[arg(long)]
        gold: Option<String>,
        /// Accept/reject binarization threshold for the judge's 0–1 score.
        #[arg(long, default_value = "0.5")]
        threshold: f32,
    },

    /// Measure the perception detector's false-negative (miss) rate against a
    /// human-labeled evaluation set — the residual risk the conscience gate
    /// cannot cover, because it can only gate labels the detector emits.
    ///
    /// The frames file is a JSON array of {frame_id?, truth: [labels],
    /// detected: [labels]}: what a human says was present, and what the detector
    /// reported. Both are classified onto consent classes (using
    /// `[conscience.classifier]` from config), so the safety-critical number is
    /// the `human` miss rate, reported with a 95% upper confidence bound.
    EvalDetector {
        /// Path to a JSON array of evaluation frames.
        #[arg(long)]
        frames: String,
    },

    /// Replay a conscience decision log against the current policy — after-the-
    /// fact review. Each recorded perception/reach decision is re-evaluated; a
    /// verdict that no longer reproduces is flagged as policy *drift* (the config
    /// changed) or, if the record claims the same policy, as a determinism *bug*.
    /// Exits non-zero only on the latter, so it is safe in CI.
    ///
    /// The log is a JSON array of records: {ts, input, verdict, config_fingerprint?}
    /// where input is {kind:"perception", label, confidence} or
    /// {kind:"reach", tool, host}.
    ReplayDecisions {
        /// Path to a JSON array of decision records.
        #[arg(long)]
        log: String,
    },

    /// Decide a multi-party-consent frame: given the people detected in a frame
    /// and a consent ledger, would it be captured? Consent is opt-in and keyed to
    /// a token the subject presents (never a recognized identity); a person with
    /// no valid token is not consented and, under the default policy, refuses the
    /// whole frame. A policy-authoring / testing tool.
    ///
    /// `--frame` is a JSON array of {class, consent_token?}; `--ledger` is a JSON
    /// array of {subject, purpose, expires_at_ms, revoked?}.
    ConsentCheck {
        /// Path to a JSON array of subject presences in the frame.
        #[arg(long)]
        frame: String,
        /// Path to a JSON array of consent grants (the ledger).
        #[arg(long)]
        ledger: String,
        /// Purpose the frame is being captured for (must match the grant).
        #[arg(long, default_value = "record")]
        purpose: String,
        /// Policy: "require-all" (refuse the frame if anyone lacks consent) or
        /// "redact" (list subjects to blank).
        #[arg(long, default_value = "require-all")]
        policy: String,
    },

    /// Run system diagnostics to check configuration and connectivity.
    Doctor,
}

#[derive(Subcommand, Debug)]
enum PeripheralCommands {
    /// List all configured peripheral nodes.
    List,
    /// Add a new peripheral node.
    Add {
        /// The board type (e.g., "esp32-s3", "nanopi-neo3").
        board: String,
        /// The device path or transport (e.g., "/dev/ttyUSB0", "native", "mqtt").
        path: String,
    },
    /// Remove a peripheral node.
    Remove {
        /// The board type to remove.
        board: String,
    },
    /// Show the capabilities of a known board type.
    Capabilities {
        /// The board type to inspect.
        board: String,
    },
}

#[derive(Subcommand, Debug)]
enum HistoryCommands {
    /// List all conversation sessions.
    List,
    /// Clear the history for a session.
    Clear {
        /// Session ID to clear (default: "default").
        #[arg(default_value = "default")]
        session: String,
    },
    /// Show messages in a session.
    Show {
        /// Session ID to show (default: "default").
        #[arg(default_value = "default")]
        session: String,
        /// Maximum number of messages to show.
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
enum SkillCommands {
    /// List installed skills with stage and clean-run record.
    List,
    /// Show one skill's manifest and rollout record.
    Show { name: String },
    /// Promote a skill one stage up (simulate → supervised → autonomous).
    /// Refused without the required clean-run record (Track 0).
    Promote { name: String },
    /// Demote a skill one stage down (always allowed).
    Demote { name: String },
    /// Reset a skill's clean-run/failure record at its current stage.
    ResetRecord { name: String },
    /// Revert a skill's description to the value before its last evolution.
    RevertDescription { name: String },
    /// Remove a skill from the forge.
    Remove { name: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Honor RUST_LOG (e.g. `RUST_LOG=debug` for the LoRa gateway's raw-line
    // view); default to INFO when unset. The old hardcoded INFO level silently
    // ignored RUST_LOG — bench-caught 2026-07-17.
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
    // `--config` wins over OBC_CONFIG, which wins over the default path —
    // Config::load() reads OBC_CONFIG, so surface the flag through it.
    if let Some(path) = &cli.config {
        std::env::set_var("OBC_CONFIG", path);
    }
    let mut config = Config::load()?;

    match cli.command {
        Commands::Start {
            provider,
            model,
            session,
            no_spine,
            gateway,
            tunnel,
        } => {
            // Apply CLI overrides to config
            if let Some(p) = provider {
                config.provider.name = p;
            }
            if let Some(m) = model {
                config.provider.model = m;
            }
            if gateway {
                config.gateway.enabled = true;
            }
            if tunnel {
                config.tunnel.enabled = true;
            }
            run_start(config, &session, no_spine).await?;
        }
        Commands::Status => {
            run_status(&config).await?;
        }
        Commands::Peripheral(cmd) => {
            run_peripheral(config, cmd).await?;
        }
        Commands::History(cmd) => {
            run_history(cmd).await?;
        }
        Commands::Skill(cmd) => {
            run_skill(&config, cmd)?;
        }
        Commands::McpServe {
            transport,
            port,
            mode,
        } => {
            run_mcp_serve(&config, &transport, port, &mode).await?;
        }
        Commands::A2aServe {
            port,
            advertise,
            echo,
        } => {
            run_a2a_serve(&config, port, advertise.as_deref(), echo).await?;
        }
        Commands::JudgeCalibrate { gold, threshold } => {
            run_judge_calibrate(gold.as_deref(), threshold).await?;
        }
        Commands::EvalDetector { frames } => {
            run_eval_detector(&config, &frames)?;
        }
        Commands::ReplayDecisions { log } => {
            run_replay_decisions(&config, &log)?;
        }
        Commands::ConsentCheck {
            frame,
            ledger,
            purpose,
            policy,
        } => {
            run_consent_check(&config, &frame, &ledger, &purpose, &policy)?;
        }
        Commands::Doctor => {
            oh_ben_claw::doctor::run(&config)?;
        }
    }

    Ok(())
}

async fn run_start(config: Config, session_id: &str, no_spine: bool) -> Result<()> {
    info!("Starting Oh-Ben-Claw v{}", env!("CARGO_PKG_VERSION"));

    // Open memory store
    let memory = Arc::new(MemoryStore::open()?);
    let session = memory.default_session()?;
    let session_id = if session_id == "default" {
        session
    } else {
        let sessions = memory.list_sessions()?;
        if sessions.iter().any(|s| s.id == session_id) {
            session_id.to_string()
        } else {
            memory.create_session(session_id)?
        }
    };

    // Build LLM provider
    let provider = providers::from_config(&config.provider)?;
    info!(
        provider = %config.provider.name,
        model = %config.provider.model,
        "LLM provider ready"
    );

    // Track 0 safety gate + action auditor — built BEFORE the tool registry so
    // the conscience reach gate can share the auditor (a reach refusal becomes a
    // tamper-evident record, exactly as a perception refusal is). The audit-key
    // helpers used here are nested `fn` items, so calling them before their
    // textual definition is fine.
    let mut safety_gate: Option<Arc<security::SafetyGate>> = None;
    let mut action_auditor: Option<Arc<std::sync::Mutex<security::ActionAuditor>>> = None;
    if config.safety.enabled {
        safety_gate = Some(Arc::new(security::SafetyGate::new(
            config.safety.limits.clone(),
        )));
        info!(
            limits = config.safety.limits.len(),
            "Track 0 safety gate active"
        );

        let key = resolve_audit_key(
            config.safety.audit_key.as_deref(),
            config.security.vault_enabled,
            config.security.vault_path.as_deref(),
            config.security.pairing_secret.as_deref(),
        );
        let audit_path = config
            .safety
            .audit_log_path
            .clone()
            .unwrap_or_else(|| track0_data_path("action_audit.jsonl"));
        if let Some(parent) = std::path::Path::new(&audit_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match security::ActionAuditor::open(key, audit_path.clone()) {
            Ok(auditor) => {
                action_auditor = Some(Arc::new(std::sync::Mutex::new(auditor)));
                info!(path = %audit_path, "Track 0 action audit log active");
            }
            Err(e) => tracing::warn!("Track 0: failed to open action audit log: {e}"),
        }
    }

    // Build tool registry. When [conscience] is enabled, attach the egress reach
    // gate AND the auditor, so outbound tool calls hit the allowlist and every
    // refusal is written to the tamper-evident log (Track 0 for reach).
    let conscience = obc_conscience::Conscience::new(&config.conscience);
    // Conscience credential resolver (item (b)): resolve a credential the reach
    // gate names on an allow, by name, at the egress boundary — never seen by
    // the model. Prefer the unlocked vault (encrypted store + env fallback);
    // otherwise environment-only, so env-backed credentials still resolve.
    let credential_resolver: Option<
        std::sync::Arc<dyn oh_ben_claw::tools::credentials::CredentialResolver>,
    > = if conscience.enabled {
        let vault_backed = if config.security.vault_enabled {
            std::env::var("OBC_VAULT_PASSWORD").ok().and_then(|pw| {
                let vpath = config
                    .security
                    .vault_path
                    .clone()
                    .unwrap_or_else(|| track0_data_path("vault.db"));
                let vault = security::SecretsVault::open(&vpath).ok()?;
                vault.unlock(&pw).ok()?;
                tracing::info!(
                    "conscience: credential resolver using the unlocked vault (env fallback)"
                );
                Some(std::sync::Arc::new(vault)
                    as std::sync::Arc<
                        dyn oh_ben_claw::tools::credentials::CredentialResolver,
                    >)
            })
        } else {
            None
        };
        Some(vault_backed.unwrap_or_else(|| {
            tracing::info!(
                "conscience: credential resolver is environment-only (vault locked or disabled)"
            );
            std::sync::Arc::new(oh_ben_claw::tools::credentials::EnvResolver)
        }))
    } else {
        None
    };
    let mut all_tools = if conscience.enabled {
        oh_ben_claw::tools::default_tools_with_reach(
            Some(conscience.reach.clone()),
            action_auditor.clone(),
            credential_resolver.clone(),
        )
    } else {
        default_tools()
    };
    // Skill forge management tool (list/install/remove skills at runtime).
    all_tools.push(Box::new(
        oh_ben_claw::skill_forge::SkillForgeTool::default_dir(),
    ));
    let mut node_count = 0usize;

    // Connect to MQTT spine and discover peripheral tools
    let spine_client = if !no_spine && config.spine.kind == "mqtt" {
        match spine::SpineClient::new(config.spine.clone(), "obc-brain")
            // `[security] require_pairing` finally reaches the announcement
            // path. Before 2026-08-01 it was validated at boot and gated
            // nothing: any node that could publish on the topic got its tools
            // registered, paired or not.
            .with_pairing(
                obc_safety::NodePairingManager::new(config.security.pairing_secret.clone()),
                config.security.require_pairing,
            )
            .with_frame_auth(obc_safety::frame_auth::FrameAuth::new(
                config.security.pairing_secret.clone(),
                config.security.require_frame_auth,
            ))
            .connect()
            .await
        {
            Ok(client) => {
                info!(
                    host = %config.spine.host,
                    port = config.spine.port,
                    "Connected to MQTT spine"
                );
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let mqtt_tools = client.build_mqtt_tools().await;
                if !mqtt_tools.is_empty() {
                    node_count += mqtt_tools.len();
                    info!(count = mqtt_tools.len(), "Discovered MQTT peripheral tools");
                    all_tools.extend(mqtt_tools);
                }
                Some(client)
            }
            Err(e) => {
                tracing::warn!(
                    "Could not connect to MQTT spine: {}. Continuing without spine.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    // Keep a spine handle for the Phase 18 reflex sink (peripherals consumes
    // `spine_client` below).
    let reflex_spine = spine_client.clone();

    // Connect to directly-wired peripheral boards
    if config.peripherals.enabled {
        let peripheral_tools =
            peripherals::create_peripheral_tools(&config.peripherals, spine_client).await?;
        if !peripheral_tools.is_empty() {
            node_count += config.peripherals.boards.len();
            info!(
                count = peripheral_tools.len(),
                "Peripheral tools registered"
            );
            all_tools.extend(peripheral_tools);
        }
    }

    // Build security context
    let security_ctx = security::SecurityContext::new(&config.security).unwrap_or_else(|e| {
        tracing::warn!("Failed to init security context: {}; using defaults", e);
        security::SecurityContext::new(&Default::default()).unwrap()
    });

    if security_ctx.policy.policy_count() > 0 {
        info!(
            policies = security_ctx.policy.policy_count(),
            "Security policy engine active"
        );
    } else {
        // Previously silent. The engine is wired and consulted on every tool call
        // at every hop; with no rules it permits everything, which is a reasonable
        // default and an unreasonable thing not to say out loud. `[[security.policies]]`
        // was implemented, tested and documented nowhere, so nobody could have
        // known it existed to set it.
        tracing::warn!(
            "no tool execution policies configured — every tool is permitted, \
             including `shell`. See [[security.policies]] in config.example.toml \
             for a recommended baseline (security::policy::baseline())."
        );
    }

    // ── Deployment: re-derive the scheme this config was planned from ──────────
    //
    // `[deployment]` is written by DeploymentPlanner (and, byte-identically, by
    // the TypeScript emitter in OBC-deployment-generator), and the planner sets
    // `auto_plan = true` in everything it writes. Until now nothing read it back:
    // `config.deployment` was consulted by exactly one thing in the whole tree, a
    // test asserting the emitted TOML deserialises. An operator could set every
    // key here and get silence — the same shape as the four channels that were
    // exported and never started.
    //
    // What this does is cheap and worth the milliseconds: rebuild the inventory,
    // re-run the planner (it is pure), and say what it now concludes. The config
    // was planned once, by whichever planner was current then; the agent reading
    // it may be several versions later. Unsatisfied desires are the payload —
    // "you asked for vision and no item here provides camera_capture" is worth
    // knowing at startup rather than at the first failed tool call.
    if config.deployment.auto_plan {
        let inventory =
            oh_ben_claw::deployment::HardwareInventory::from_deployment_config(&config.deployment);

        if inventory.items.is_empty() {
            tracing::warn!(
                scenario = %config.deployment.scenario,
                "deployment.auto_plan is set but [[deployment.hardware]] is empty — \
                 nothing to plan. The planner writes both together, so a config with \
                 one and not the other was probably hand-edited."
            );
        } else {
            let scheme = oh_ben_claw::deployment::DeploymentPlanner::plan(&inventory);
            info!(
                scenario = %scheme.scenario_name,
                host = %scheme.host_board,
                sub_agents = scheme.sub_agent_count(),
                "deployment scheme re-derived from [deployment]"
            );
            for w in &scheme.warnings {
                tracing::warn!(deployment = true, "{w}");
            }
            // The reason to do this at startup at all. `is_complete()` is false
            // when a feature_desire has no hardware to satisfy it, which is
            // exactly the drift a config outliving its hardware produces.
            if !scheme.is_complete() {
                for s in &scheme.suggested_hardware {
                    tracing::warn!(
                        "deployment: feature desire unsatisfied by the recorded \
                         hardware — {s:?}"
                    );
                }
            }
        }
    } else if config.deployment.enabled && !config.deployment.hardware.is_empty() {
        // Not a warning: a recorded inventory with auto_plan off is a legitimate
        // choice, and saying so at info level makes the switch discoverable to
        // someone who did not know the block did anything. Until this release it
        // genuinely did not.
        info!(
            items = config.deployment.hardware.len(),
            "deployment inventory recorded; set deployment.auto_plan = true to \
             re-check it against the current planner at startup"
        );
    }

    // ── Track 0: physical-action safety (shared by plain agent + orchestrator) ──
    // Resolve the audit MAC key with a secure precedence:
    //   explicit config key > vault (if unlockable via OBC_VAULT_PASSWORD)
    //   > node pairing secret > a persisted, auto-generated random key.
    // The old hardcoded dev key is gone — a published constant would let anyone
    // forge audit entries.
    fn track0_data_path(name: &str) -> String {
        oh_ben_claw::config::paths::in_data_dir(name)
            .to_string_lossy()
            .into_owned()
    }
    fn track0_random_key() -> String {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        hex::encode(buf)
    }
    fn resolve_audit_key(
        explicit: Option<&str>,
        vault_enabled: bool,
        vault_path: Option<&str>,
        pairing_secret: Option<&str>,
    ) -> Vec<u8> {
        if let Some(k) = explicit {
            return k.as_bytes().to_vec();
        }
        // Vault: only usable when the operator supplies the master password.
        if vault_enabled {
            if let Ok(pw) = std::env::var("OBC_VAULT_PASSWORD") {
                let vpath = vault_path
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| track0_data_path("vault.db"));
                if let Ok(vault) = security::SecretsVault::open(&vpath) {
                    if vault.unlock(&pw).is_ok() {
                        match vault.get("OBC_TRACK0_AUDIT_KEY") {
                            Ok(Some(k)) => {
                                tracing::info!("Track 0: audit key loaded from vault");
                                return k.into_bytes();
                            }
                            Ok(None) => {
                                let k = track0_random_key();
                                if vault.set("OBC_TRACK0_AUDIT_KEY", &k).is_ok() {
                                    tracing::info!("Track 0: generated audit key stored in vault");
                                    return k.into_bytes();
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
        }
        if let Some(k) = pairing_secret {
            tracing::info!("Track 0: deriving audit key from the node pairing secret");
            return k.as_bytes().to_vec();
        }
        // Persisted random key (secret + stable across restarts).
        let key_path = track0_data_path("action_audit.key");
        if let Ok(existing) = std::fs::read_to_string(&key_path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return trimmed.as_bytes().to_vec();
            }
        }
        let k = track0_random_key();
        if let Some(parent) = std::path::Path::new(&key_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&key_path, &k) {
            Ok(_) => tracing::warn!(
                path = %key_path,
                "Track 0: generated a persisted random audit key — set [safety].audit_key or enable the vault for production"
            ),
            Err(e) => {
                tracing::warn!("Track 0: could not persist audit key ({e}); using an ephemeral key")
            }
        }
        k.into_bytes()
    }

    // (Track 0 safety gate + action auditor are built earlier, before the tool
    // registry, so the conscience reach gate shares the auditor — see above.)

    // Phase 16: trajectory store for experiential self-improvement (shared by
    // the plain agent and the orchestrator's inner agent).
    let mut trajectory_store: Option<Arc<oh_ben_claw::memory::trajectory::TrajectoryStore>> = None;
    if config.self_improvement.enabled {
        let traj_path = config
            .self_improvement
            .db_path
            .clone()
            .unwrap_or_else(|| track0_data_path("trajectories.db"));
        if let Some(parent) = std::path::Path::new(&traj_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match oh_ben_claw::memory::trajectory::TrajectoryStore::open(&traj_path) {
            Ok(store) => {
                // Dense retrieval leg (local embeddings) — build- and
                // config-gated; the store works identically without it.
                #[allow(unused_mut)]
                let mut store = store;
                if config.self_improvement.semantic {
                    #[cfg(feature = "semantic")]
                    match oh_ben_claw::memory::embed::FastEmbedder::try_default() {
                        Ok(embedder) => {
                            store = store.with_embedder(Box::new(embedder));
                            info!("Phase 16 semantic retrieval leg active (local embeddings)");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "semantic retrieval requested but embedder failed to init");
                        }
                    }
                    #[cfg(not(feature = "semantic"))]
                    tracing::warn!(
                        "[self_improvement].semantic is set but this build lacks the \
                         `semantic` cargo feature — rebuild with `--features semantic`"
                    );
                }
                trajectory_store = Some(Arc::new(store));
                info!(path = %traj_path, "Phase 16 trajectory capture active");
            }
            Err(e) => tracing::warn!("Phase 16: failed to open trajectory store: {e}"),
        }
    }

    // Phase 18: open world memory once, shared by the world_memory tool and the
    // reflex controller.
    let world_mem: Option<Arc<oh_ben_claw::memory::world::WorldMemory>> = if config
        .perception
        .world_memory
    {
        let world_path = config
            .perception
            .world_db_path
            .clone()
            .unwrap_or_else(|| track0_data_path("world.db"));
        if let Some(parent) = std::path::Path::new(&world_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match oh_ben_claw::memory::world::WorldMemory::open(&world_path) {
            Ok(wm) => {
                let wm = Arc::new(wm);
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::world::WorldMemoryTool::new(Arc::clone(&wm)),
                ));
                info!(path = %world_path, "Phase 18 world memory tool active");
                // The typed way for the agent to write down a finding. Playbooks name
                // this tool for their "record what you found" step, so the entity name,
                // schema and provenance are owned by the framework rather than
                // improvised per wake (see tools::builtin::incident).
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::incident::RecordIncidentTool::new(Arc::clone(&wm)),
                ));
                info!("Incident recording tool active");
                // Conservation Grid G0: the shared geospatial frame lives in world
                // memory, so node poses ↔ (lat, lon) through one anchored site.
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::site_anchor::SiteAnchorTool::new(Arc::clone(&wm)),
                ));
                info!("Site anchor tool active (Conservation Grid G0)");
                // Swap the stateless gnss_fix for the frame-aware one: fixes then
                // land in the anchored site frame when a site is pinned.
                all_tools.retain(|t| t.name() != "gnss_fix");
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::gnss::GnssFixTool::with_world(Arc::clone(&wm)),
                ));
                // System 2 mesh awareness: a read-only mesh_status tool so the agent
                // can see fleet health when a mesh escalation wakes it (paired with
                // mesh_command for action). Registered whenever mesh is in play.
                if config.mesh_supervisor.enabled || config.lora_gateway.is_some() {
                    all_tools.push(Box::new(
                        oh_ben_claw::tools::builtin::mesh::MeshStatusTool::new(Arc::clone(&wm)),
                    ));
                    info!("Mesh status tool active (System 2 mesh awareness)");
                }
                Some(wm)
            }
            Err(e) => {
                tracing::warn!("Phase 18: failed to open world memory: {e}");
                None
            }
        }
    } else {
        None
    };

    // Handle to the mesh command sink (set when the LoRa gateway opens), shared with
    // the mesh supervisor below so it can auto-issue recovery commands over the mesh.
    #[allow(unused_mut)]
    let mut mesh_sink: Option<Arc<dyn oh_ben_claw::spine::lora_gateway::CommandSink>> = None;

    // Phase B: LoRa mesh gateway bridge — pipe a base-station Heltec's USB console
    // into world memory. Node spine messages heard over the air (link state, power
    // mode, reflex/safing reports) are parsed from the gateway's `SPINE ◄ … : {json}`
    // lines and observed into the brain's world model, exactly as if the node were
    // on the wired MQTT spine. Read-only; needs the `hardware` feature (serial I/O).
    if let Some(gw) = &config.lora_gateway {
        info!(port = %gw.port, baud = gw.baud, "Phase B: LoRa gateway bridge (mesh <-> host)");
        #[cfg(feature = "hardware")]
        {
            match oh_ben_claw::spine::lora_gateway::open_split(&gw.port, gw.baud) {
                Ok((rd, wr)) => {
                    // Inbound: mesh node messages -> world memory (needs a store).
                    match &world_mem {
                        Some(world) => {
                            let world_rx = Arc::clone(world);
                            tokio::spawn(async move {
                                let now_ms = || {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0)
                                };
                                oh_ben_claw::spine::lora_gateway::run_gateway_rx(
                                    rd, world_rx, now_ms,
                                )
                                .await;
                                tracing::warn!("LoRa gateway RX loop ended (serial link closed)");
                            });
                            info!("LoRa gateway: inbound mesh -> world memory active");
                        }
                        None => {
                            drop(rd);
                            tracing::warn!(
                                "[lora_gateway] inbound disabled ([perception].world_memory is \
                                 off); outbound mesh_command still active"
                            );
                        }
                    }
                    // Outbound: expose `mesh_command` so the agent can command a node
                    // over LoRa. The node gates execution on its own Track 0.
                    let sink: Arc<dyn oh_ben_claw::spine::lora_gateway::CommandSink> =
                        Arc::new(oh_ben_claw::spine::lora_gateway::SerialCommandSink::new(wr));
                    mesh_sink = Some(Arc::clone(&sink));
                    all_tools.push(Box::new(
                        oh_ben_claw::tools::builtin::mesh::MeshCommandTool::new(sink),
                    ));
                    info!("LoRa gateway: outbound mesh_command tool active");
                }
                Err(e) => tracing::warn!("[lora_gateway] failed to open {}: {e}", gw.port),
            }
        }
        #[cfg(not(feature = "hardware"))]
        {
            tracing::warn!(
                "[lora_gateway] configured but this build lacks the `hardware` feature \
                 (serial I/O); the bridge will not start"
            );
        }
    }

    // Phase B: mesh supervisor — fold the mesh into the brain. Each tick it derives a
    // per-node health view from the mesh facts in world memory and, when a node goes
    // offline, can autonomously issue a rate-limited recovery command over the mesh.
    if config.mesh_supervisor.enabled {
        match &world_mem {
            Some(world) => {
                let world_sup = Arc::clone(world);
                let sink_sup = mesh_sink.clone();
                let cfg = config.mesh_supervisor.clone();
                info!(
                    stale_ms = cfg.stale_ms,
                    recover = ?cfg.recover,
                    "Mesh supervisor active (mesh -> brain)"
                );
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
                        cfg.tick_ms.max(500),
                    ));
                    loop {
                        ticker.tick().await;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let _ = oh_ben_claw::spine::mesh_supervisor::tick(
                            &world_sup,
                            sink_sup.as_ref(),
                            &cfg,
                            now,
                        )
                        .await;
                    }
                });
            }
            None => tracing::warn!(
                "[mesh_supervisor] enabled but [perception].world_memory is off; nothing to supervise"
            ),
        }
    }

    // Source liveness: a subsystem that is configured off is not going to refresh,
    // correct or retract anything it wrote, so its beliefs are unmaintained from the
    // moment this process starts without it. That is not the same as those beliefs
    // being false, and the difference is a bug we shipped: `mesh.escalated_count = 2`
    // sat open in the bench store with the supervisor disabled, and the
    // `safe-mesh-node-lost` reflex kept escalating on a number whose author no longer
    // existed.
    //
    // So the disappearance gets written down (Prometheus staleness markers, 2017:
    // absence of data is itself a datum) and what rested on it stops being believed.
    // Only the deterministic half runs here — "config says this is off". The
    // inferential half, `sweep_silent`, guesses from silence and is deliberately not
    // wired to boot, where a source that simply has not spoken yet looks identical to
    // one that died.
    if let Some(world) = &world_mem {
        use oh_ben_claw::memory::liveness::{is_marked_stopped, stopped, Stopped};
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let configured_off: [(bool, &str, &str); 2] = [
            (
                !config.mesh_supervisor.enabled,
                oh_ben_claw::spine::mesh_supervisor::SUPERVISOR_SOURCE,
                "[mesh_supervisor] enabled = false",
            ),
            (
                config.lora_gateway.is_none(),
                oh_ben_claw::spine::lora_gateway::SOURCE,
                "[lora_gateway] not configured",
            ),
        ];
        for (is_off, source, reason) in configured_off {
            if !is_off {
                continue;
            }
            match is_marked_stopped(world, source) {
                Ok(true) => continue, // already recorded; do not churn the marker
                Err(e) => {
                    tracing::warn!(source, "liveness: could not read marker: {e}");
                    continue;
                }
                Ok(false) => {}
            }
            match stopped(world, source, Stopped::Retired, now, reason) {
                Ok(s) if !s.is_empty() => info!(
                    source,
                    retracted = s.closed.len(),
                    unsupported = s.unsupported.len(),
                    "source retired: beliefs it left standing are no longer believed"
                ),
                Ok(_) => info!(source, "source retired (nothing was standing)"),
                Err(e) => tracing::warn!(source, "liveness: retirement failed: {e}"),
            }
        }

        // Repair the subject counters, once, at boot. They are accumulators, so the
        // damage from a non-idempotent ingest compounds instead of washing out:
        // `prev + 1` per poll per event turned 50 detections into a deer count of 5,376.
        // Ingest is idempotent now, which stops the bleeding but cannot undo it — only a
        // recount from distinct event ids can.
        //
        // Cheap and idempotent: after the first correcting boot it finds every counter
        // already right and writes nothing, so it stays as a self-healing check rather
        // than becoming a migration someone has to remember to run.
        match oh_ben_claw::vision::clawcam_ingest::recount_subjects(world, now, "clawcam") {
            Ok(fixed) if !fixed.is_empty() => {
                for (subject, old, new) in &fixed {
                    info!(
                        subject,
                        old, new, "subject count corrected from distinct events"
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("subject recount failed: {e}"),
        }

        // Retention. The fourth and last way a belief stops being held, and the only one
        // that comes from a rule rather than from the world: an agent's own note is never
        // superseded (nothing rewrites that entity), never orphaned (the agent does not
        // stop existing), and never undercut (an assertion rests on nothing recorded).
        // Without a policy it stays believed forever — the bench store carried six such
        // notes from an investigation that had concluded ten days earlier.
        //
        // Runs after liveness so a note already withdrawn for a better reason keeps that
        // reason. Empty config does nothing at all, which is the intended default.
        if !config.perception.expiry.is_empty() {
            let policies = &config.perception.expiry;
            match oh_ben_claw::memory::expiry::expire(world, policies, now) {
                Ok(out) => {
                    for why in &out.rejected {
                        // Loud, because a retention rule that quietly does nothing is how
                        // you find out a year later that nothing ever expired.
                        tracing::error!("[[perception.expiry]] policy rejected: {why}");
                    }
                    if !out.is_empty() {
                        info!(
                            expired = out.expired.len(),
                            unsupported = out.unsupported.len(),
                            policies = policies.len(),
                            "retention: beliefs aged out of currency"
                        );
                    }
                }
                Err(e) => tracing::warn!("retention sweep failed: {e}"),
            }
        }
    }

    // Movement subsystem: expose the safety-bounded `move_actuator` tool to the
    // agent (System 2), and keep the controller so System 1 reflexes can route
    // `Action::Move` through the *same* gate (below). Physical actuation MUST be
    // deterministically bounded (Suite §7), so it requires the Track 0 gate;
    // commanded state is recorded into world memory when available.
    let movement_controller: Option<Arc<oh_ben_claw::movement::MovementController>> = if config
        .movement
        .enabled
    {
        match &safety_gate {
            Some(gate) => {
                // Drive real hardware over the spine when connected; fall back
                // to the dry-run logging sink otherwise.
                let sink: Arc<dyn oh_ben_claw::movement::ActuatorSink> = match &reflex_spine {
                    Some(spine) => {
                        info!("Movement actuation dispatches over the spine");
                        Arc::new(oh_ben_claw::spine::SpineActuatorSink::new(Arc::clone(
                            spine,
                        )))
                    }
                    None => Arc::new(oh_ben_claw::movement::LoggingActuatorSink),
                };
                let mut controller = oh_ben_claw::movement::MovementController::new(
                    config.movement.node_id.clone(),
                    Arc::clone(gate),
                    sink,
                );
                if let Some(world) = &world_mem {
                    controller = controller.with_world_memory(Arc::clone(world));
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::movement::MovementTool::new(Arc::clone(
                        &controller,
                    )),
                ));
                info!(node_id = %config.movement.node_id, "Movement subsystem: move_actuator tool active");
                Some(controller)
            }
            None => {
                tracing::warn!(
                    "[movement] enabled but [safety] is off — movement is physical and \
                         requires deterministic limits; skipping move_actuator tool"
                );
                None
            }
        }
    } else {
        None
    };

    // Sensing subsystem: expose the quality-aware `sense` tool. Non-actuating
    // (reads + reversible memory appends), so it needs no Track 0 gate; it
    // records quality-classified readings into world memory as `sensor.{quantity}`
    // facts, which reflexes already consume. Requires world memory to be on.
    if config.sensing.enabled {
        match &world_mem {
            Some(world) => {
                let specs: Vec<(String, oh_ben_claw::sensing::QuantitySpec)> = config
                    .sensing
                    .quantities
                    .iter()
                    .map(|q| {
                        (
                            q.name.clone(),
                            oh_ben_claw::sensing::QuantitySpec {
                                min: q.min,
                                max: q.max,
                                max_staleness_ms: q.max_staleness_ms,
                                unit: q.unit.clone(),
                            },
                        )
                    })
                    .collect();
                let mut controller = oh_ben_claw::sensing::SensingController::new(specs)
                    .with_world_memory(Arc::clone(world));
                if let Some(source) = &config.sensing.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::sensing::SenseTool::new(
                        Arc::clone(&controller),
                        Arc::clone(world),
                    ),
                ));
                info!(
                    quantities = config.sensing.quantities.len(),
                    "Sensing subsystem: sense tool active"
                );
            }
            None => {
                tracing::warn!(
                    "[sensing] enabled but [perception].world_memory is off — the sense \
                     tool needs world memory to record and query readings; skipping"
                );
            }
        }
    }

    // Audio suite: expose `hear` (perceive) + `speak` (act). Heard events and
    // spoken utterances are recorded into world memory (`audio.{stream}`,
    // `speech.last`); speech is emitted through the dry-run logging sink until a
    // real engine/speaker is wired. Requires world memory to be on.
    // Hoisted so the mission runner can reuse the audio controller.
    let mut audio_controller: Option<Arc<oh_ben_claw::audio::suite::AudioController>> = None;
    if config.audio_suite.enabled {
        match &world_mem {
            Some(world) => {
                // Render speech locally via TTS, else over the spine when
                // connected, else dry-run.
                let speech_sink: Arc<dyn oh_ben_claw::audio::suite::SpeechSink> =
                    if config.audio_suite.render_tts {
                        let dir = config
                            .audio_suite
                            .tts_out_dir
                            .clone()
                            .unwrap_or_else(|| "/tmp".to_string());
                        info!(dir = %dir, "Audio: speech rendered locally via TTS");
                        Arc::new(oh_ben_claw::audio::suite::TtsSpeechSink::new(dir))
                    } else {
                        match &reflex_spine {
                            Some(spine) => {
                                info!("Audio: speech emitted over the spine");
                                Arc::new(oh_ben_claw::audio::suite::SpineSpeechSink::new(
                                    Arc::clone(spine),
                                ))
                            }
                            None => Arc::new(oh_ben_claw::audio::suite::LoggingSpeechSink),
                        }
                    };
                let mut controller = oh_ben_claw::audio::suite::AudioController::new(speech_sink)
                    .with_world_memory(Arc::clone(world));
                if let Some(min) = config.audio_suite.min_confidence {
                    controller = controller.with_min_confidence(min);
                }
                if let Some(source) = &config.audio_suite.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                audio_controller = Some(Arc::clone(&controller));
                let voice = config
                    .audio_suite
                    .voice
                    .clone()
                    .unwrap_or_else(|| "nova".to_string());
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::audio_suite::HearTool::new(
                        Arc::clone(&controller),
                        Arc::clone(world),
                    ),
                ));
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::audio_suite::SpeakTool::new(
                        Arc::clone(&controller),
                        voice,
                    ),
                ));
                info!("Audio suite: hear + speak tools active");
            }
            None => {
                tracing::warn!(
                    "[audio_suite] enabled but [perception].world_memory is off — the hear/speak \
                     tools need world memory to record events; skipping"
                );
            }
        }
    }

    // Power suite: expose the `power` tool and record battery telemetry + a
    // derived power mode into world memory (`power.battery`, `power.mode`).
    // Reflexes can watch `power.mode == critical` for low-power safing. Requires
    // world memory to be on.
    if config.power.enabled {
        match &world_mem {
            Some(world) => {
                let defaults = oh_ben_claw::power::PowerThresholds::default();
                let thresholds = oh_ben_claw::power::PowerThresholds {
                    low_pct: config.power.low_pct.unwrap_or(defaults.low_pct),
                    critical_pct: config.power.critical_pct.unwrap_or(defaults.critical_pct),
                };
                let mut controller = oh_ben_claw::power::PowerController::new(thresholds)
                    .with_world_memory(Arc::clone(world));
                if let Some(source) = &config.power.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::power::PowerTool::new(
                        Arc::clone(&controller),
                        Arc::clone(world),
                    ),
                ));
                info!(
                    low_pct = thresholds.low_pct,
                    critical_pct = thresholds.critical_pct,
                    "Power suite: power tool active"
                );
            }
            None => {
                tracing::warn!(
                    "[power] enabled but [perception].world_memory is off — the power tool needs \
                     world memory to record telemetry; skipping"
                );
            }
        }
    }

    // Comms suite: expose the `comms` tool and record per-link state
    // (`link.{name}`) + an aggregate `net.mode` into world memory. Reflexes can
    // watch `net.mode` (offline/degraded) for connectivity safing. Requires
    // world memory to be on.
    if config.comms.enabled {
        match &world_mem {
            Some(world) => {
                let defaults = oh_ben_claw::comms::LinkThresholds::default();
                let thresholds = oh_ben_claw::comms::LinkThresholds {
                    min_rssi_dbm: config.comms.min_rssi_dbm.unwrap_or(defaults.min_rssi_dbm),
                    max_latency_ms: config
                        .comms
                        .max_latency_ms
                        .unwrap_or(defaults.max_latency_ms),
                    max_loss_pct: config.comms.max_loss_pct.unwrap_or(defaults.max_loss_pct),
                };
                let mut controller = oh_ben_claw::comms::CommsController::new(thresholds)
                    .with_world_memory(Arc::clone(world));
                if let Some(source) = &config.comms.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::comms::CommsTool::new(
                        Arc::clone(&controller),
                        Arc::clone(world),
                    ),
                ));
                info!(
                    max_latency_ms = thresholds.max_latency_ms,
                    "Comms suite: comms tool active"
                );
            }
            None => {
                tracing::warn!(
                    "[comms] enabled but [perception].world_memory is off — the comms tool needs \
                     world memory to record telemetry; skipping"
                );
            }
        }
    }

    // Navigation suite: the fusing subsystem — localize from sensor pose facts
    // and drive toward a goal through the (gated) movement controller. Reuses the
    // movement controller, so it needs both world memory and movement enabled.
    // Hoisted so the mission runner can issue navigation steps.
    let mut nav_controller: Option<Arc<oh_ben_claw::navigation::NavController>> = None;
    if config.navigation.enabled {
        match (&world_mem, &movement_controller) {
            (Some(world), Some(mc)) => {
                let steer = config
                    .navigation
                    .steer
                    .clone()
                    .map(|a| (a.name, a.channel))
                    .unwrap_or_else(|| ("steer".to_string(), 0));
                let drive = config
                    .navigation
                    .drive
                    .clone()
                    .map(|a| (a.name, a.channel))
                    .unwrap_or_else(|| ("drive".to_string(), 1));
                let mut gains = oh_ben_claw::navigation::NavGains::default();
                if let Some(v) = config.navigation.forward_speed {
                    gains.forward_speed = v;
                }
                if let Some(v) = config.navigation.max_steer_deg {
                    gains.max_steer_deg = v;
                }
                if let Some(v) = config.navigation.heading_kp {
                    gains.heading_kp = v;
                }
                if let Some(v) = config.navigation.align_threshold_deg {
                    gains.align_threshold_deg = v;
                }
                let mut nav =
                    oh_ben_claw::navigation::NavController::new(Arc::clone(mc), steer, drive)
                        .with_world_memory(Arc::clone(world))
                        .with_gains(gains);
                if let Some(s) = &config.navigation.source {
                    nav = nav.with_source(s.clone());
                }
                if let Some(r) = config.navigation.sensor_max_range {
                    nav = nav.with_sensor_range(r);
                }
                // Clearance-aware planning: inflate obstacles by the robot footprint.
                if let (Some(insc), Some(infl)) = (
                    config.navigation.inscribed_radius,
                    config.navigation.inflation_radius,
                ) {
                    let decay = config.navigation.inflation_decay.unwrap_or(2.0);
                    nav = nav.with_inflation(insc, infl, decay);
                    info!(
                        inscribed = insc,
                        inflation = infl,
                        "Navigation: clearance-aware planning (inflation)"
                    );
                }
                // Obstacle-aware planning: attach an occupancy grid if configured.
                let has_grid = config.navigation.grid.is_some();
                if let Some(gc) = &config.navigation.grid {
                    let grid = oh_ben_claw::navigation::planning::OccupancyGrid::new(
                        gc.origin_x,
                        gc.origin_y,
                        gc.resolution,
                        gc.width,
                        gc.height,
                    );
                    nav = nav.with_grid(Arc::new(std::sync::Mutex::new(grid)));
                    info!(
                        width = gc.width,
                        height = gc.height,
                        "Navigation: obstacle-aware planning enabled"
                    );
                }
                let nav = Arc::new(nav);
                nav_controller = Some(Arc::clone(&nav));
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::navigation::NavigateTool::new(Arc::clone(&nav)),
                ));
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::navigation::NavStatusTool::new(
                        Arc::clone(&nav),
                        Arc::clone(world),
                    ),
                ));
                if has_grid {
                    all_tools.push(Box::new(
                        oh_ben_claw::tools::builtin::navigation::NavMapTool::new(Arc::clone(&nav)),
                    ));
                }
                let interval = std::time::Duration::from_millis(
                    config.navigation.interval_ms.unwrap_or(500).max(100),
                );

                // Pose fusion (SLAM-lite): when sources are configured, fuse them
                // into the canonical pose entities the localizer reads, on the same
                // cadence — so navigation transparently consumes the fused pose.
                if !config.navigation.pose_sources.is_empty() {
                    let sources: Vec<oh_ben_claw::navigation::pose_fusion::PoseSource> = config
                        .navigation
                        .pose_sources
                        .iter()
                        .map(|s| {
                            oh_ben_claw::navigation::pose_fusion::PoseSource::with_prefix(
                                &s.prefix, s.weight,
                            )
                        })
                        .collect();
                    let fuser = oh_ben_claw::navigation::pose_fusion::PoseFuser::new(
                        sources,
                        Arc::clone(world),
                    );
                    info!(
                        sources = fuser.source_count(),
                        "Navigation: pose fusion loop active"
                    );
                    tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(interval);
                        loop {
                            ticker.tick().await;
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            if let Err(e) = fuser.fuse(now) {
                                tracing::warn!("pose fusion failed: {e}");
                            }
                        }
                    });
                }

                let nav_loop = Arc::clone(&nav);
                let explore = config.navigation.explore && has_grid;
                if explore {
                    info!("Navigation: autonomous frontier exploration enabled");
                }
                info!("Navigation suite: navigate + nav_status tools active");
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(interval);
                    loop {
                        ticker.tick().await;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        // Autonomous exploration: when idle, head to the next
                        // frontier; the drive step then carries the robot there.
                        if explore && nav_loop.current_goal().is_none() {
                            if let Err(e) = nav_loop.explore_step(now) {
                                tracing::warn!("exploration step failed: {e}");
                            }
                        }
                        if let Err(e) = nav_loop.step_toward_goal(now).await {
                            tracing::warn!("nav step failed: {e}");
                        }
                    }
                });
            }
            (None, _) => tracing::warn!(
                "[navigation] enabled but [perception].world_memory is off — navigation needs \
                 world memory; skipping"
            ),
            (_, None) => tracing::warn!(
                "[navigation] enabled but the movement controller is unavailable (needs \
                 [movement] enabled + [safety]); skipping"
            ),
        }
    }

    // Mission sequencer: deliberative, guarded missions composed over the suites
    // (navigate, speak, wait, record, await). The `mission` tool starts one; a
    // runner ticks the active mission and aborts on guard trips. Requires world
    // memory; uses navigation + audio when present.
    if config.mission.enabled {
        if let Some(world) = &world_mem {
            use std::collections::HashMap;
            let mut runner = oh_ben_claw::mission::MissionRunner::new(Arc::clone(world));
            if let Some(nav) = &nav_controller {
                runner = runner.with_nav(Arc::clone(nav));
            }
            if let Some(audio) = &audio_controller {
                runner = runner.with_audio(Arc::clone(audio));
            }
            let runner = Arc::new(runner);
            let mut library: HashMap<String, oh_ben_claw::mission::Mission> = HashMap::new();
            for m in &config.mission.missions {
                library.insert(m.id.clone(), m.clone());
            }
            let library = Arc::new(library);
            all_tools.push(Box::new(
                oh_ben_claw::tools::builtin::mission::MissionStartTool::new(
                    Arc::clone(&runner),
                    Arc::clone(&library),
                ),
            ));
            all_tools.push(Box::new(
                oh_ben_claw::tools::builtin::mission::MissionStatusTool::new(
                    Arc::clone(&runner),
                    Arc::clone(&library),
                ),
            ));
            let interval = std::time::Duration::from_millis(
                config.mission.interval_ms.unwrap_or(500).max(100),
            );
            let runner_loop = Arc::clone(&runner);
            info!(missions = library.len(), "Mission sequencer active");
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if let Err(e) = runner_loop.tick(now).await {
                        tracing::warn!("mission tick failed: {e}");
                    }
                }
            });
        } else {
            tracing::warn!(
                "[mission] enabled but [perception].world_memory is off — missions need world \
                 memory; skipping"
            );
        }
    }

    // Shared observability context — one metrics registry across the reflex loop
    // (per-rule/action fire counts) and the gateway `/metrics` endpoint.
    let obs = Arc::new(observability::ObsContext::new());

    // No-op-fallback metric export: when OBC_METRICS_EXPORT_SECS is set, push the
    // metrics registry to a collector on that cadence. The exporter buffers offline
    // and reconciles on reconnect; the default sink logs (swap in a real collector).
    if let Some(secs) = std::env::var("OBC_METRICS_EXPORT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
    {
        let exporter = Arc::new(observability::ReconcilingExporter::new(
            Arc::clone(&obs.metrics),
            Arc::new(observability::LoggingMetricSink),
        ));
        let interval = std::time::Duration::from_secs(secs);
        info!(interval_secs = secs, "metrics export loop spawned");
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let _ = exporter.export_now().await;
            }
        });
    }

    // Shared host-side safing state: flipped in-process when a safing advisory
    // fires (via the SafingSink tap below), read by load-shedding consumers
    // (e.g. the ClawCam poll backs off when shed_load is set).
    let safing_state = Arc::new(oh_ben_claw::agent::safing::SafingState::new());

    // Phase 18: spawn the dual-system reflex controller (System 1) when enabled.
    // Operator rules are merged with the standard safing rules (when [reflex]
    // safing = true) so the suite mode hooks (power.mode, net.mode, …) drive
    // deterministic safing actions. Uses the dry-run logging sink until spine.
    // Connect the ClawCam MCP bridge once (if configured): shared by the reflex
    // actuation sink (OBC → ClawCam capture/arm/alert) and the detection/health/
    // audio poll, so the brain reads *and* commands the cameras over one bridge.
    let clawcam_client: Option<Arc<tokio::sync::Mutex<oh_ben_claw::mcp::client::McpClient>>> =
        match config.perception.clawcam_poll.clone() {
            Some(cfg) if cfg.enabled => {
                match oh_ben_claw::mcp::client::McpClient::connect(&cfg.server).await {
                    Ok(c) => {
                        info!("ClawCam MCP bridge connected (actuation + poll)");
                        Some(Arc::new(tokio::sync::Mutex::new(c)))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Could not connect to ClawCam MCP server: {e}; ClawCam features disabled"
                        );
                        None
                    }
                }
            }
            _ => None,
        };

    let reflex_rules: Vec<oh_ben_claw::agent::reflex::ReflexRule> = {
        let mut rules = config.reflex.rules.clone();
        if config.reflex.safing {
            let opts = oh_ben_claw::agent::safing::SafingOptions {
                stop_actuator: config
                    .reflex
                    .safing_stop_actuator
                    .as_ref()
                    .map(|a| (a.name.clone(), a.channel)),
                alarm_streams: config.reflex.safing_alarm_streams.clone(),
                unreliable_sensors: config.reflex.safing_unreliable_sensors.clone(),
                overheat: config
                    .reflex
                    .safing_overheat
                    .iter()
                    .map(|o| (o.quantity.clone(), o.threshold))
                    .collect(),
                debounce_ms: 0,
            };
            let safing = oh_ben_claw::agent::safing::standard_safing_rules(&opts);
            info!(count = safing.len(), "Phase 18 safing rules appended");
            rules.extend(safing);
        }
        if config.perception.vision_rules.enabled {
            let vc = &config.perception.vision_rules;
            let opts = oh_ben_claw::vision::clawcam_rules::VisionRuleOptions {
                alert_subjects: vc.alert_subjects.clone(),
                require_state: vc.require_state.clone(),
                debounce_ms: vc.debounce_ms,
                capture_node: vc.capture_node.clone(),
                rate_threshold: vc.rate_threshold,
                horizon_ms: vc.horizon_ms,
            };
            let vrules = oh_ben_claw::vision::clawcam_rules::vision_security_rules(&opts);
            info!(count = vrules.len(), "vision-driven reflex rules appended");
            rules.extend(vrules);
        }
        // ClawCam analytics reflexes ("today is weird"): only meaningful when
        // the analytics poll is feeding `clawcam.analytics.*` facts.
        if let Some(cp) = &config.perception.clawcam_poll {
            if cp.enabled && cp.poll_analytics {
                let aopts = oh_ben_claw::vision::clawcam_rules::AnalyticsRuleOptions {
                    z_alert: cp.anomaly_z_alert,
                    debounce_ms: cp.analytics_debounce_ms,
                };
                let arules = oh_ben_claw::vision::clawcam_rules::vision_analytics_rules(&aopts);
                info!(
                    count = arules.len(),
                    "ClawCam analytics reflex rules appended"
                );
                rules.extend(arules);
            }
        }
        rules
    };
    // Phase 18 System 2: when armed, the reflex sink chain hands escalation
    // wakes to the slow reasoner through this channel (spawned once the agent
    // handle exists, further below).
    let mut system2_rx: Option<
        tokio::sync::mpsc::Receiver<oh_ben_claw::agent::system2::WakeEvent>,
    > = None;
    if config.reflex.enabled && !reflex_rules.is_empty() {
        if let Some(world) = &world_mem {
            use oh_ben_claw::agent::reflex::{
                ActionSink, EscalationBudget, LoggingActionSink, MovementActionSink,
                ReflexController, ReflexEngine,
            };
            use oh_ben_claw::spine::SpineActionSink;
            let engine = ReflexEngine::new(reflex_rules.clone());
            let base_sink: Arc<dyn ActionSink> = match &reflex_spine {
                Some(spine) => {
                    info!("Phase 18 reflexes dispatch over the spine");
                    Arc::new(SpineActionSink::new(Arc::clone(spine)))
                }
                None => {
                    info!("Phase 18 reflexes use the dry-run logging sink (spine not connected)");
                    Arc::new(LoggingActionSink)
                }
            };
            // OBC → ClawCam actuation: intercept `clawcam/cmd/*` reflex publishes
            // and call ClawCam write tools (capture/arm/alert) over the shared MCP
            // bridge — still passing ClawCam's own approval model. Other publishes
            // pass through to the sink above.
            let base_sink: Arc<dyn ActionSink> = match &clawcam_client {
                Some(client) => {
                    info!("Phase 18 reflexes can command ClawCam (capture/arm/alert)");
                    Arc::new(
                        oh_ben_claw::vision::clawcam_actuate::ClawCamActionSink::new(
                            Arc::clone(client),
                            base_sink,
                        ),
                    )
                }
                None => base_sink,
            };
            // Route reflex `Move` actions through the gated movement controller
            // (other actions delegate to the spine/logging sink). System 1 now
            // actuates typed, safety-bounded movement, not just raw GPIO.
            let move_sink: Arc<dyn ActionSink> = match &movement_controller {
                Some(mc) => {
                    info!("Phase 18 reflex Move actions routed through the movement controller");
                    Arc::new(MovementActionSink::new(Arc::clone(mc), base_sink))
                }
                None => base_sink,
            };
            // Tap safing advisories into the shared SafingState (in-process
            // load-shedding), still forwarding every action to the sink above.
            let sink: Arc<dyn ActionSink> = Arc::new(oh_ben_claw::agent::safing::SafingSink::new(
                Arc::clone(&safing_state),
                move_sink,
            ));
            // Wire escalations to notification channels (durable log-of-record in world
            // memory + optional webhook), best-effort — the wake-System-2 path is
            // unchanged; a down webhook never stalls System 1.
            let sink: Arc<dyn ActionSink> = if config.notifications.enabled {
                let mut notifier = oh_ben_claw::agent::notify::Notifier::new()
                    .with_dedup_window(config.notifications.dedup_window_ms);
                use oh_ben_claw::agent::notify::Severity;
                if config.notifications.log_to_world_memory {
                    notifier = notifier.with_channel_min(
                        Arc::new(oh_ben_claw::agent::notify::WorldMemoryChannel::new(
                            Arc::clone(world),
                        )),
                        Severity::from_name(config.notifications.log_min_severity.as_deref()),
                    );
                }
                if let Some(url) = &config.notifications.webhook_url {
                    notifier = notifier.with_channel_min(
                        Arc::new(oh_ben_claw::agent::notify::WebhookChannel::new(url.clone())),
                        Severity::from_name(config.notifications.webhook_min_severity.as_deref()),
                    );
                }
                if config.notifications.speak_escalations {
                    // Speak escalations aloud through a speech sink (same TTS / spine /
                    // dry-run selection as the audio suite). Best-effort, headline only.
                    let speech: Arc<dyn oh_ben_claw::audio::suite::SpeechSink> =
                        if config.audio_suite.render_tts {
                            let dir = config
                                .audio_suite
                                .tts_out_dir
                                .clone()
                                .unwrap_or_else(|| "/tmp".to_string());
                            Arc::new(oh_ben_claw::audio::suite::TtsSpeechSink::new(dir))
                        } else if let Some(spine) = &reflex_spine {
                            Arc::new(oh_ben_claw::audio::suite::SpineSpeechSink::new(Arc::clone(
                                spine,
                            )))
                        } else {
                            Arc::new(oh_ben_claw::audio::suite::LoggingSpeechSink)
                        };
                    let voice = config
                        .audio_suite
                        .voice
                        .clone()
                        .unwrap_or_else(|| "nova".to_string());
                    notifier = notifier.with_channel_min(
                        Arc::new(
                            oh_ben_claw::agent::notify::SpeechChannel::new(speech)
                                .with_voice(voice),
                        ),
                        Severity::from_name(config.notifications.speak_min_severity.as_deref()),
                    );
                }
                info!(
                    channels = notifier.channel_count(),
                    "Escalation notifications wired"
                );
                let notifier = Arc::new(notifier);

                // Periodic digest: roll the escalation log up by reason on a schedule and
                // deliver a one-line summary through the same channels.
                if config.notifications.digest_interval_ms > 0 {
                    let interval = config.notifications.digest_interval_ms;
                    let label = if interval.is_multiple_of(86_400_000) {
                        format!("{}d", interval / 86_400_000)
                    } else if interval.is_multiple_of(3_600_000) {
                        format!("{}h", interval / 3_600_000)
                    } else {
                        format!("{}m", (interval / 60_000).max(1))
                    };
                    let notifier_d = Arc::clone(&notifier);
                    let world_d = Arc::clone(world);
                    info!(interval_ms = interval, "Escalation digest scheduled");
                    tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
                            interval.max(1_000),
                        ));
                        ticker.tick().await; // consume the immediate first tick
                        loop {
                            ticker.tick().await;
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            let records: Vec<_> =
                                world_d
                                    .history("notifications.escalation")
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter_map(|f| {
                                        f.value.get("reason").and_then(|r| r.as_str()).map(
                                            |reason| oh_ben_claw::agent::notify::EscalationRecord {
                                                reason: reason.to_string(),
                                                ts_ms: f.valid_from,
                                            },
                                        )
                                    })
                                    // Don't fold prior digests back into the next digest.
                                    .filter(|r| {
                                        !r.reason
                                            .starts_with(oh_ben_claw::agent::notify::DIGEST_PREFIX)
                                    })
                                    .collect();
                            let lines =
                                oh_ben_claw::agent::notify::build_digest(&records, interval, now);
                            if let Some(text) =
                                oh_ben_claw::agent::notify::format_digest(&lines, &label)
                            {
                                notifier_d.deliver_summary(text, now).await;
                            }
                        }
                    });
                }

                Arc::new(oh_ben_claw::agent::notify::NotifyingActionSink::new(
                    sink, notifier,
                ))
            } else {
                sink
            };
            // Phase 18 System 2: top of the chain — escalations additionally
            // wake the slow reasoner (novelty-gated, budget-capped, never
            // blocking System 1).
            let sink: Arc<dyn ActionSink> = if config.system2.enabled {
                let cap = config.system2.queue_capacity.unwrap_or(8);
                let (s2_sink, rx) = oh_ben_claw::agent::system2::System2Sink::new(sink, cap);
                system2_rx = Some(rx);
                info!("Phase 18 System 2 wake channel armed (escalation → slow reasoner)");
                Arc::new(s2_sink)
            } else {
                sink
            };
            let mut controller = ReflexController::new(engine, Arc::clone(world), sink)
                .with_metrics(Arc::clone(&obs.metrics));
            if let Some(max) = config.reflex.max_escalations_per_min {
                controller = controller.with_escalation_budget(EscalationBudget::per_minute(max));
            }
            let interval =
                std::time::Duration::from_millis(config.reflex.interval_ms.unwrap_or(1000));
            info!(
                rules = reflex_rules.len(),
                "Phase 18 reflex controller spawned"
            );
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if let Err(e) = controller.tick_and_dispatch(now).await {
                        tracing::warn!("reflex tick failed: {e}");
                    }
                }
            });
        } else {
            tracing::warn!(
                "[reflex] enabled but [perception].world_memory is off; reflexes need world memory"
            );
        }
    }

    // Shared buffer of approved *learned* rules (self-authoring); the foresight
    // engine evaluates these alongside its static rules, and the learning layer
    // pushes into it on approval.
    let learned_rules: Arc<std::sync::Mutex<Vec<oh_ben_claw::foresight::ForesightRule>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    // Foresight (Track 1): the predictive control layer. A `foresight` query tool
    // and — when rules are configured (or learning is on) — a loop that forecasts
    // world-memory trends and fires *before* a threshold crossing.
    if config.foresight.enabled {
        if let Some(world) = &world_mem {
            all_tools.push(Box::new(
                oh_ben_claw::tools::builtin::foresight::ForesightTool::new(Arc::clone(world)),
            ));
            if !config.foresight.rules.is_empty()
                || config.learning.enabled
                || config.perception.vision_rules.enabled
            {
                use oh_ben_claw::agent::reflex::{ActionSink, EscalationBudget, LoggingActionSink};
                use oh_ben_claw::spine::SpineActionSink;
                let mut foresight_rules = config.foresight.rules.clone();
                if config.perception.vision_rules.enabled {
                    let vc = &config.perception.vision_rules;
                    let opts = oh_ben_claw::vision::clawcam_rules::VisionRuleOptions {
                        alert_subjects: vc.alert_subjects.clone(),
                        require_state: vc.require_state.clone(),
                        debounce_ms: vc.debounce_ms,
                        capture_node: vc.capture_node.clone(),
                        rate_threshold: vc.rate_threshold,
                        horizon_ms: vc.horizon_ms,
                    };
                    foresight_rules.extend(
                        oh_ben_claw::vision::clawcam_rules::vision_foresight_rules(&opts),
                    );
                }
                let engine = oh_ben_claw::foresight::ForesightEngine::new(foresight_rules)
                    .with_learned_rules(Arc::clone(&learned_rules));
                let sink: Arc<dyn ActionSink> = match &reflex_spine {
                    Some(spine) => Arc::new(SpineActionSink::new(Arc::clone(spine))),
                    None => Arc::new(LoggingActionSink),
                };
                let mut controller = oh_ben_claw::foresight::ForesightController::new(
                    engine,
                    Arc::clone(world),
                    sink,
                );
                if let Some(max) = config.foresight.max_escalations_per_min {
                    controller =
                        controller.with_escalation_budget(EscalationBudget::per_minute(max));
                }
                let interval = std::time::Duration::from_millis(
                    config.foresight.interval_ms.unwrap_or(1000).max(100),
                );
                info!(
                    rules = config.foresight.rules.len(),
                    "Foresight (Track 1) predictive controller active"
                );
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(interval);
                    loop {
                        ticker.tick().await;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        if let Err(e) = controller.tick_and_dispatch(now).await {
                            tracing::warn!("foresight tick failed: {e}");
                        }
                    }
                });
            } else {
                info!("Foresight: query tool active (no predictive rules configured)");
            }
        } else {
            tracing::warn!(
                "[foresight] enabled but [perception].world_memory is off — foresight needs world \
                 memory; skipping"
            );
        }
    }

    // Self-authored reflexes: mine antecedents of a configured bad outcome from
    // world-memory history and surface them via the `learn` tool. Approval
    // activates an (escalate-only) rule into the shared learned buffer the
    // foresight engine evaluates. Requires world memory + a configured outcome.
    if config.learning.enabled {
        match (&world_mem, &config.learning.outcome) {
            (Some(world), Some(oc)) => {
                let miner = oh_ben_claw::learning::RuleMiner {
                    lookback_ms: config.learning.lookback_ms.unwrap_or(5_000),
                    min_support: config.learning.min_support.unwrap_or(2),
                    min_confidence: config.learning.min_confidence.unwrap_or(0.6),
                    candidates: config.learning.candidates.clone(),
                };
                let outcome = oh_ben_claw::learning::OutcomeSpec {
                    entity: oc.entity.clone(),
                    op: oc.op,
                    threshold: oc.threshold,
                };
                let store = Arc::new(
                    oh_ben_claw::learning::ProposalStore::new(Arc::clone(&learned_rules))
                        .with_params(
                            config.learning.horizon_ms.unwrap_or(60_000),
                            config.learning.debounce_ms.unwrap_or(30_000),
                        ),
                );
                all_tools.push(Box::new(
                    oh_ben_claw::tools::builtin::learn::LearnTool::new(
                        Arc::clone(world),
                        Arc::clone(&store),
                        miner.clone(),
                        outcome.clone(),
                    ),
                ));
                if let Some(ms) = config.learning.auto_mine_interval_ms {
                    let world_l = Arc::clone(world);
                    let store_l = Arc::clone(&store);
                    let miner_l = miner.clone();
                    let outcome_l = outcome.clone();
                    let interval = std::time::Duration::from_millis(ms.max(1_000));
                    info!("Self-authored reflexes: learn tool + auto-mine loop active");
                    tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(interval);
                        loop {
                            ticker.tick().await;
                            if let Ok(props) = miner_l.mine(&world_l, &outcome_l) {
                                let n = store_l.ingest(props);
                                if n > 0 {
                                    info!(
                                        proposals = n,
                                        "learning: new rule proposals (awaiting approval)"
                                    );
                                }
                            }
                        }
                    });
                } else {
                    info!("Self-authored reflexes: learn tool active (mine on demand)");
                }
            }
            (None, _) => tracing::warn!(
                "[learning] enabled but [perception].world_memory is off — learning needs world \
                 memory; skipping"
            ),
            (_, None) => tracing::warn!(
                "[learning] enabled but no [learning.outcome] is configured; skipping"
            ),
        }
    }

    // Fleet coordination (Phase 20): one brain, many bodies. A coordinator
    // ingests node heartbeats, queues tasks, and allocates each to the nearest
    // online idle node with enough battery — assignments are advisory (recorded
    // to world memory), the node still actuates under its own Track 0 gate.
    if config.fleet.enabled {
        let mut coord = oh_ben_claw::fleet::Coordinator::new();
        if let Some(world) = &world_mem {
            coord = coord.with_world_memory(Arc::clone(world));
        }
        if let Some(s) = config.fleet.stale_ms {
            coord = coord.with_stale_ms(s);
        }
        // Assignment egress: collect assignment intents into the coordinator's outbox
        // so any connected transport (MQTT spine and/or LoRa mesh) can deliver them.
        let want_outbox = reflex_spine.is_some()
            || (cfg!(feature = "hardware") && config.fleet.lora_serial.is_some());
        if want_outbox {
            coord = coord.with_assignment_outbox();
        }
        let coord = Arc::new(coord);
        all_tools.push(Box::new(
            oh_ben_claw::tools::builtin::fleet::FleetTool::new(Arc::clone(&coord)),
        ));
        all_tools.push(Box::new(
            oh_ben_claw::tools::builtin::fleet::FleetStatusTool::new(Arc::clone(&coord)),
        ));
        // Distributed fleet: ingest node heartbeats from the spine when connected.
        if let Some(spine) = &reflex_spine {
            match spine
                .subscribe_handler(
                    oh_ben_claw::fleet::HEARTBEAT_FILTER,
                    oh_ben_claw::fleet::spine_heartbeat_handler(Arc::clone(&coord)),
                )
                .await
            {
                Ok(()) => info!("Fleet: ingesting node heartbeats over the spine"),
                Err(e) => tracing::warn!("fleet heartbeat subscription failed: {e}"),
            }
        }
        // Off-grid: attach a serial LoRa-mesh node (firmware/lora-node) so the fleet
        // coordinates over the air — no WiFi, no broker. The RX loop bridges received
        // heartbeats into `coord` and rebroadcasts multi-hop frames; the radio handle
        // is kept for the assignment egress below. Returns `(radio, relay_hops)`.
        #[cfg(feature = "hardware")]
        let lora_egress: Option<(Arc<oh_ben_claw::spine::lora_mesh::SerialMeshRadio>, u8)> =
            if let Some(lora) = &config.fleet.lora_serial {
                match oh_ben_claw::spine::lora_mesh::SerialMeshRadio::open(&lora.port, lora.baud) {
                    Ok((radio, rd)) => {
                        let radio = Arc::new(radio);
                        info!(port = %lora.port, baud = lora.baud, hops = lora.relay_hops, "Fleet: LoRa-mesh serial bridge attached");
                        let coord_rx = Arc::clone(&coord);
                        let radio_rx = Arc::clone(&radio);
                        let relay =
                            Arc::new(oh_ben_claw::spine::lora_mesh::relay::MeshRelay::new());
                        tokio::spawn(async move {
                            oh_ben_claw::spine::lora_mesh::run_serial_rx_relay(
                                rd,
                                coord_rx,
                                radio_rx,
                                relay,
                                || {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0)
                                },
                            )
                            .await;
                            tracing::warn!("Fleet: LoRa-mesh serial link closed");
                        });
                        Some((radio, lora.relay_hops))
                    }
                    Err(e) => {
                        tracing::warn!("LoRa-mesh serial bridge failed to open: {e}");
                        None
                    }
                }
            } else {
                None
            };
        // Unified assignment egress: drain the coordinator's outbox and fan each
        // intent out to every connected transport — publish over the MQTT spine
        // (`obc/fleet/assign/{node}`) and/or broadcast over the LoRa mesh (multi-hop
        // MeshFrame::Assign). One drain, N transports; the coordinator stays
        // transport-blind.
        if want_outbox {
            let coord_eg = Arc::clone(&coord);
            let spine_eg = reflex_spine.clone();
            #[cfg(feature = "hardware")]
            let lora_eg = lora_egress;
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(500));
                #[cfg(feature = "hardware")]
                let mut msg_id: u64 = 0;
                loop {
                    ticker.tick().await;
                    for (node, x, y) in coord_eg.drain_outbox() {
                        if let Some(spine) = &spine_eg {
                            let goal = oh_ben_claw::navigation::NavGoal {
                                x,
                                y,
                                tolerance: 0.5,
                            };
                            let _ =
                                oh_ben_claw::fleet::publish_assignment(spine, &node, &goal).await;
                        }
                        #[cfg(feature = "hardware")]
                        if let Some((radio, hops)) = &lora_eg {
                            msg_id = msg_id.wrapping_add(1);
                            let _ = oh_ben_claw::spine::lora_mesh::send_assignment_frame(
                                radio.as_ref(),
                                &node,
                                x,
                                y,
                                msg_id,
                                *hops,
                            )
                            .await;
                        }
                    }
                }
            });
        }
        let interval =
            std::time::Duration::from_millis(config.fleet.interval_ms.unwrap_or(2_000).max(500));
        let coord_loop = Arc::clone(&coord);
        info!("Fleet coordinator active");
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                coord_loop.tick(now);
            }
        });
    }

    // Phase 18 / S1b: fold a ClawCam (vision subsystem) MCP server's detections —
    // and optionally node health + audio classifications — into the embodied stack
    // on a cadence, reusing the shared ClawCam MCP bridge.
    if let (Some(world), Some(client)) = (&world_mem, &clawcam_client) {
        if let Some(cfg) = config.perception.clawcam_poll.clone() {
            if cfg.enabled {
                let world = Arc::clone(world);
                let client = Arc::clone(client);
                let poll_safing = Arc::clone(&safing_state);
                let audio_ctrl = audio_controller.clone();
                // Conscience perception gate for this poll. When [conscience] is
                // disabled the gate admits everything, so this is always safe.
                let poll_conscience = obc_conscience::Conscience::new(&config.conscience);
                let poll_auditor = action_auditor.clone();
                // #2 decision log (opt-in via OBC_DECISION_LOG): persist every
                // perception decision so `replay-decisions` runs on real history.
                let poll_decision_log: Option<Arc<oh_ben_claw::decision_log::DecisionLog>> =
                    std::env::var("OBC_DECISION_LOG").ok().and_then(|path| {
                        match oh_ben_claw::decision_log::DecisionLog::open(
                            &path,
                            &config.conscience,
                        ) {
                            Ok(l) => {
                                info!(path = %path, "conscience: perception decision log active");
                                Some(Arc::new(l))
                            }
                            Err(e) => {
                                tracing::warn!("could not open OBC_DECISION_LOG: {e}");
                                None
                            }
                        }
                    });
                // #3 multi-party consent (opt-in via OBC_CONSENT_LEDGER): drop or
                // redact frames whose subjects didn't present a valid consent token.
                let poll_consent: Option<(
                    Arc<obc_conscience::ConsentLedger>,
                    obc_conscience::ConsentPolicy,
                    String,
                )> = std::env::var("OBC_CONSENT_LEDGER").ok().and_then(|path| {
                    let text = std::fs::read_to_string(&path)
                        .map_err(|e| tracing::warn!("could not read OBC_CONSENT_LEDGER: {e}"))
                        .ok()?;
                    let grants: Vec<obc_conscience::ConsentGrant> = serde_json::from_str(&text)
                        .map_err(|e| tracing::warn!("could not parse OBC_CONSENT_LEDGER: {e}"))
                        .ok()?;
                    let policy = match std::env::var("OBC_CONSENT_POLICY").as_deref() {
                        Ok("redact") => obc_conscience::ConsentPolicy::Redact,
                        _ => obc_conscience::ConsentPolicy::RequireAll,
                    };
                    let purpose =
                        std::env::var("OBC_CONSENT_PURPOSE").unwrap_or_else(|_| "record".into());
                    info!(path = %path, grants = grants.len(),
                        "conscience: multi-party consent active");
                    Some((
                        Arc::new(obc_conscience::ConsentLedger::from_grants(grants)),
                        policy,
                        purpose,
                    ))
                });
                // Spatial fusion: a fixed camera's detection becomes a hazard disc
                // in the occupancy grid, so the planner routes the mobile robot
                // clear of what a *static* node saw. Both halves were already
                // live -- the detection poll below and NavController above -- and
                // vision/clawcam_spatial.rs sat unwired between them until
                // 2026-07-30. Off unless hazard_radius_m > 0 and cameras are listed.
                let spatial_nav = nav_controller.clone();
                let camera_map = {
                    let mut m = oh_ben_claw::vision::clawcam_spatial::CameraMap::new();
                    for c in &cfg.cameras {
                        m.set(c.node.clone(), c.x, c.y);
                    }
                    m
                };
                let hazard_radius = cfg.hazard_radius_m;
                let hazard_step = cfg.hazard_step_m;
                if hazard_radius > 0.0 && !camera_map.is_empty() {
                    if spatial_nav.is_some() {
                        info!(
                            cameras = camera_map.len(),
                            radius_m = hazard_radius,
                            "ClawCam spatial fusion active: detections become nav hazards"
                        );
                    } else {
                        tracing::warn!(
                            "[perception.clawcam_poll] lists cameras and a hazard radius, \
                             but no navigation grid is configured -- detections cannot \
                             become obstacles. Configure [navigation] or drop the cameras."
                        );
                    }
                }
                let interval = std::time::Duration::from_millis(cfg.interval_ms.max(250));
                info!(
                    tool = %cfg.tool,
                    interval_ms = cfg.interval_ms,
                    poll_health = cfg.poll_health,
                    poll_audio = cfg.poll_audio,
                    "Phase 18 ClawCam → world memory poll spawned"
                );
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(interval);
                    loop {
                        ticker.tick().await;
                        // Load-shedding: when safing has engaged shed_load (e.g.
                        // battery critical/low), skip the poll until charge recovers.
                        if poll_safing.shed_load() {
                            continue;
                        }
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let guard = {
                            let mut g = oh_ben_claw::vision::clawcam_ingest::PerceptionGuard::new(
                                &poll_conscience,
                                poll_auditor.as_deref(),
                            );
                            g.decision_log = poll_decision_log.as_deref();
                            if let Some((ledger, policy, purpose)) = poll_consent.as_ref() {
                                g.consent =
                                    Some(oh_ben_claw::vision::clawcam_ingest::ConsentContext {
                                        ledger: ledger.as_ref(),
                                        policy: *policy,
                                        purpose: purpose.as_str(),
                                    });
                            }
                            g
                        };
                        match oh_ben_claw::vision::clawcam_ingest::poll_clawcam_guarded(
                            Arc::clone(&client),
                            &world,
                            &cfg.tool,
                            cfg.args.clone(),
                            now,
                            &cfg.source,
                            &guard,
                        )
                        .await
                        {
                            Ok(entities) if !entities.is_empty() => {
                                // Maintain vision.count.{subject} so foresight can
                                // trend the detection rate over time.
                                let _ = oh_ben_claw::vision::clawcam_ingest::record_subject_counts(
                                    &world,
                                    &entities,
                                    now,
                                    &cfg.source,
                                );

                                // Stamp a hazard disc around every camera that just
                                // reported. device_id rides in the fact value the
                                // ingest wrote, so this reads the shared world model
                                // rather than needing the poll to hand it over --
                                // which is how clawcam_spatial.rs describes closing
                                // the loop.
                                if let Some(nav) = spatial_nav.as_ref() {
                                    if hazard_radius > 0.0 {
                                        let mut seen: Vec<String> = Vec::new();
                                        for ent in &entities {
                                            if let Some(node) =
                                                oh_ben_claw::vision::clawcam_ingest::detection_node_of(
                                                    &world, ent,
                                                )
                                            {
                                                if !seen.contains(&node) {
                                                    seen.push(node);
                                                }
                                            }
                                        }
                                        for node in seen {
                                            let cells =
                                                oh_ben_claw::vision::clawcam_spatial::mark_detection_hazard(
                                                    nav,
                                                    &camera_map,
                                                    &node,
                                                    hazard_radius,
                                                    hazard_step,
                                                );
                                            if cells > 0 {
                                                info!(
                                                    camera = %node,
                                                    cells,
                                                    "detection marked as a navigation hazard"
                                                );
                                            }
                                        }
                                    }
                                }
                                info!(
                                    count = entities.len(),
                                    "ClawCam detections folded into world memory"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!("ClawCam poll failed: {e}"),
                        }
                        // Camera node health → namespaced `clawcam.node.{id}` facts
                        // (kept distinct from the robot's own power/comms suites).
                        if cfg.poll_health {
                            let raw = {
                                let mut g = client.lock().await;
                                g.call_tool("get_node_health", serde_json::json!({})).await
                            };
                            if let Ok(raw) = raw {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                                    let rows =
                                        oh_ben_claw::vision::clawcam_ingest::extract_node_health(
                                            &v,
                                        );
                                    let _ = oh_ben_claw::vision::clawcam_ingest::ingest_node_health(
                                        &world,
                                        &rows,
                                        now,
                                        &cfg.source,
                                    );
                                }
                            }
                        }
                        // Camera audio classifications → the audio suite (distinct
                        // streams), so a glassbreak is classifiable by safing.
                        if cfg.poll_audio {
                            if let Some(ac) = &audio_ctrl {
                                let raw = {
                                    let mut g = client.lock().await;
                                    g.call_tool("list_audio_classifications", serde_json::json!({}))
                                        .await
                                };
                                if let Ok(raw) = raw {
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                                        for a in
                                            oh_ben_claw::vision::clawcam_ingest::extract_audio_classes(&v)
                                        {
                                            let ev =
                                                oh_ben_claw::vision::clawcam_ingest::audio_class_to_event(&a);
                                            // A real camera classified this off its own microphone.
                                            let _ = ac.observe(
                                                &ev,
                                                now,
                                                oh_ben_claw::memory::world::Origin::Observed,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    // ClawCam analytics reports → `clawcam.analytics.*` facts on their own slow
    // cadence (the reports are daily aggregates), so the analytics reflexes can
    // act on "today is weird": an unusually quiet day reads as a possible
    // knocked-over/obstructed camera, a spike as a surge worth System 2's
    // attention, and calibration drift as a threshold-retune prompt.
    if let (Some(world), Some(client)) = (&world_mem, &clawcam_client) {
        if let Some(cfg) = config.perception.clawcam_poll.clone() {
            if cfg.enabled && cfg.poll_analytics {
                let world = Arc::clone(world);
                let client = Arc::clone(client);
                let poll_safing = Arc::clone(&safing_state);
                let interval =
                    std::time::Duration::from_millis(cfg.analytics_interval_ms.max(60_000));
                info!(
                    interval_ms = cfg.analytics_interval_ms,
                    z_alert = cfg.anomaly_z_alert,
                    "ClawCam analytics → world memory poll spawned"
                );
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(interval);
                    loop {
                        ticker.tick().await;
                        // Same load-shedding contract as the detection poll.
                        if poll_safing.shed_load() {
                            continue;
                        }
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        match oh_ben_claw::vision::clawcam_analytics::poll_clawcam_analytics(
                            Arc::clone(&client),
                            &world,
                            now,
                            &cfg.source,
                        )
                        .await
                        {
                            Ok(entities) if !entities.is_empty() => info!(
                                count = entities.len(),
                                "ClawCam analytics folded into world memory"
                            ),
                            Ok(_) => {}
                            Err(e) => tracing::warn!("ClawCam analytics poll failed: {e}"),
                        }
                    }
                });
            }
        }
    }

    // Shared safety subsystems — the SAME instances go to the plain agent and,
    // in orchestrator mode, to the orchestrator's inner agent, so both modes
    // enforce an identical safety posture (audit 2026-07-02).
    //
    // Approval policy: the autonomy level + auto-approve list + session/forever
    // grants gate every tool call (composing with Track 0 + trust). Full
    // autonomy (the default) passes everything; under supervised/manual an
    // un-granted tool is refused in this autonomous loop.
    let approval = Arc::new(oh_ben_claw::approval::ApprovalManager::from_config(
        &config.autonomy,
    ));
    // Track 0 dynamic trust: refuse physical actions from a node whose behavior
    // (latency/failures) has demoted it; every tool round-trip feeds the score.
    let trust_scorer = if config.safety.enabled && config.safety.dynamic_trust {
        info!("Track 0 dynamic trust scoring enabled");
        Some(Arc::new(
            oh_ben_claw::security::trust::TrustScorer::default(),
        ))
    } else {
        None
    };
    // Track 0 taint tracking: opt-in (unset ⇒ off). Shared by both the plain
    // and orchestrator inner agents.
    let taint_mode =
        oh_ben_claw::security::taint::TaintMode::from_config(config.safety.taint_mode.as_deref());
    if taint_mode != oh_ben_claw::security::taint::TaintMode::Off {
        info!(mode = ?taint_mode, "Track 0 taint tracking enabled");
    }
    // Phase 16 P3: Track 0 staged rollout — clean-run record + auto-demotion.
    let rollout_tracker = Arc::new(oh_ben_claw::skill_forge::rollout::RolloutTracker::load(
        oh_ben_claw::skill_forge::rollout::RolloutTracker::default_path(),
    ));
    // Phase 15/9: token cost tracking (estimated usage; USD when the operator
    // configures [cost] prices). Persisted so daily/monthly budgets survive
    // restarts; falls back to session-only on DB failure.
    let cost_tracker: Option<Arc<oh_ben_claw::cost::CostTracker>> = if config.cost.enabled {
        let db_path = track0_data_path("costs.db");
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tracker = match oh_ben_claw::cost::CostTracker::with_db(config.cost.clone(), &db_path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "cost DB unavailable; tracking session-only");
                oh_ben_claw::cost::CostTracker::new(config.cost.clone())
            }
        };
        info!(
            daily_limit = config.cost.daily_limit_usd,
            monthly_limit = config.cost.monthly_limit_usd,
            "Cost tracking enabled"
        );
        Some(Arc::new(tracker))
    } else {
        None
    };
    // Phase 16 P1: experience retrieval top-k (None = disabled).
    let experience_k =
        if trajectory_store.is_some() && config.self_improvement.retrieval.unwrap_or(true) {
            Some(config.self_improvement.retrieval_k.unwrap_or(3))
        } else {
            None
        };

    // Build the plain reasoning agent, attaching Track 0 + Phase 16 when configured.
    let mut agent = Agent::new(
        config.agent.clone(),
        provider,
        Arc::clone(&memory),
        all_tools,
    )
    .with_policy(security_ctx.policy.clone());
    if let Some(gate) = &safety_gate {
        agent = agent.with_safety_gate(Arc::clone(gate));
    }
    if let Some(auditor) = &action_auditor {
        agent = agent.with_action_auditor(Arc::clone(auditor));
    }
    // Credentials in the config file: works, warns. See `inline_secret_providers`.
    for label in oh_ben_claw::config::inline_secret_providers(&config) {
        tracing::warn!(
            at = %label,
            "API key stored inline in config.toml — prefer the provider's environment \
             variable (ANTHROPIC_API_KEY / OPENAI_API_KEY / OPENROUTER_API_KEY). The \
             config file is the one people paste into issues and commit to repos."
        );
    }

    // Close the loop between the world model and the thing reasoning on top of it.
    // Without this the agent's entire context is its system prompt and the last 50
    // messages: it can query `world_memory`, but nothing tells it there is anything to
    // ask about, and an agent that cannot see what it believes cannot notice that a
    // belief has been withdrawn.
    if let Some(world) = &world_mem {
        if config.perception.context.enabled {
            agent = agent.with_world_context(Arc::clone(world), config.perception.context.clone());
            info!(
                max_facts = config.perception.context.max_facts,
                max_withdrawals = config.perception.context.max_withdrawals,
                "World state joins the agent's context each turn"
            );
        }
    }
    if let Some(t) = &trajectory_store {
        agent = agent.with_trajectory_store(Arc::clone(t));
    }
    if let Some(k) = experience_k {
        agent = agent.with_experience_retrieval(k);
    }
    if let Some(trust) = &trust_scorer {
        agent = agent.with_trust(Arc::clone(trust));
    }
    agent = agent.with_approval(Arc::clone(&approval));
    agent = agent.with_obs(Arc::clone(&obs));
    if let Some(cost) = &cost_tracker {
        agent = agent.with_cost(
            Arc::clone(cost),
            config.cost.input_price_per_million,
            config.cost.output_price_per_million,
        );
    }
    agent = agent
        .with_rollout(Arc::clone(&rollout_tracker))
        .with_forge_dir(oh_ben_claw::skill_forge::SkillForge::default_dir())
        .with_taint_mode(taint_mode);
    // Phase 16: load enabled skills (authored + learned) from the forge into the
    // live tool registry. Disabled skills stay invisible; simulate-stage skills
    // load but can only dry-run until promoted.
    {
        let forge = oh_ben_claw::skill_forge::SkillForge::new(
            oh_ben_claw::skill_forge::SkillForge::default_dir(),
        );
        let (added, _removed, shadowed) = agent.sync_skills(&forge);
        if added > 0 || shadowed > 0 {
            info!(added, shadowed, "Skill forge tools registered");
        }
    }
    let agent = Arc::new(agent);

    // Build orchestrator or plain agent handle
    let (handle, maybe_pool) = if config.orchestrator.enabled {
        info!("Multi-agent orchestration enabled — building OrchestratorAgent");
        // Same safety posture as the plain agent: policy, approval, trust,
        // obs, rollout, and forge dir all attach to the inner agent too
        // (audit 2026-07-02 — these were previously missing in orchestrator
        // mode, which silently skipped policy + autonomy enforcement).
        let orch = OrchestratorAgent::new_with_deps(
            config.agent.clone(),
            config.provider.clone(),
            Arc::clone(&memory),
            config.orchestrator.clone(),
            session_id.clone(),
            oh_ben_claw::agent::orchestrator::InnerAgentDeps {
                safety: safety_gate.clone(),
                auditor: action_auditor.clone(),
                // Conscience reach gate flows to the orchestrator's inner agent
                // AND its sub-agent pool, so orchestration/delegation can't
                // bypass the egress allowlist the plain agent honors.
                reach: if conscience.enabled {
                    Some(conscience.reach.clone())
                } else {
                    None
                },
                // Credential resolver (item (b)) flows to the inner agent and the
                // sub-agent pool too, so injection covers delegated egress paths.
                resolver: credential_resolver.clone(),
                trajectory: trajectory_store.clone(),
                policy: Some(security_ctx.policy.clone()),
                approval: Some(Arc::clone(&approval)),
                obs: Some(Arc::clone(&obs)),
                trust: trust_scorer.clone(),
                cost: cost_tracker.as_ref().map(|c| {
                    (
                        Arc::clone(c),
                        config.cost.input_price_per_million,
                        config.cost.output_price_per_million,
                    )
                }),
                rollout: Some(Arc::clone(&rollout_tracker)),
                forge_dir: Some(oh_ben_claw::skill_forge::SkillForge::default_dir()),
                experience_k,
                taint_mode,
            },
        )?;
        let pool = orch.pool.clone();
        let h = orch.handle.clone();
        h.set_node_count(node_count).await;
        info!(
            sub_agents = pool.active_count(),
            tool_count = h.tool_count(),
            node_count = node_count,
            session_id = %session_id,
            "Orchestrator ready"
        );
        (h, Some(pool))
    } else {
        let handle = AgentHandle::new(Arc::clone(&agent), config.provider.clone());
        handle.set_node_count(node_count).await;
        info!(
            tool_count = agent.tool_count(),
            node_count = node_count,
            session_id = %session_id,
            "Agent ready"
        );
        (handle, None)
    };

    // Phase 18 System 2: spawn the slow reasoner over the live agent. Wakes
    // arrive from the reflex chain (System 1), pass the novelty gate + hourly
    // budget, and run one agent turn on a dedicated session; the outcome lands
    // in world memory (`system2.last_wake`).
    if let Some(rx) = system2_rx.take() {
        use oh_ben_claw::agent::system2::{NoveltyGate, Reasoner, System2Reasoner};

        struct AgentReasoner {
            handle: oh_ben_claw::agent::AgentHandle,
        }
        #[async_trait::async_trait]
        impl Reasoner for AgentReasoner {
            async fn reason(&self, objective: &str) -> anyhow::Result<String> {
                let resp = self.handle.process("system2", objective).await?;
                Ok(resp.message)
            }
        }
        // The reasoner's dedicated "system2" session must exist before the first
        // wake — message appends FK-reference sessions.id, and the first real
        // hardware wake died on exactly that (bench, 2026-07-17).
        if let Err(e) = memory.create_session_with_id("system2") {
            tracing::warn!("could not pre-create the system2 session: {e}");
        }

        let gate = NoveltyGate::new(
            config.system2.novelty_window_ms.unwrap_or(600_000),
            config.system2.max_wakes_per_hour.unwrap_or(6),
        );
        let mut reasoner = System2Reasoner::new(
            gate,
            Arc::new(AgentReasoner {
                handle: handle.clone(),
            }),
        )
        .with_obs(Arc::clone(&obs));
        if let Some(world) = &world_mem {
            reasoner = reasoner.with_world(Arc::clone(world));
        }
        info!(
            novelty_window_ms = config.system2.novelty_window_ms.unwrap_or(600_000),
            max_wakes_per_hour = config.system2.max_wakes_per_hour.unwrap_or(6),
            "Phase 18 System 2 slow reasoner spawned"
        );
        tokio::spawn(reasoner.run(rx));
    }

    // Phase 16: spawn the autonomous self-improvement loop when enabled. It
    // periodically synthesizes + verifies skills from successful trajectories;
    // physical skills are quarantined for operator promotion (Track 0).
    // The executor is the **active** agent (the handle's) so replay runs
    // through the live chokepoint and hot-reloads reach the agent actually
    // serving traffic — in orchestrator mode that's the inner agent, not the
    // plain one (audit 2026-07-02).
    if config.self_improvement.enabled {
        if let Some(traj) = &trajectory_store {
            let improver = oh_ben_claw::skill_forge::improve::SkillImprover::new(
                Arc::clone(traj),
                oh_ben_claw::skill_forge::SkillForge::new(
                    oh_ben_claw::skill_forge::SkillForge::default_dir(),
                ),
                vec![
                    "gpio_write".to_string(),
                    "relay".to_string(),
                    "motor".to_string(),
                    "servo".to_string(),
                ],
                config.self_improvement.max_learned.unwrap_or(500),
            )
            .with_obs(Arc::clone(&obs))
            .with_verification_rules(
                config
                    .self_improvement
                    .verification
                    .iter()
                    .filter_map(|r| {
                        use oh_ben_claw::skill_forge::synthesis::VerificationCheck;
                        let check = match r.kind.as_str() {
                            "test_command" => Some(VerificationCheck::TestCommand {
                                cmd: r.cmd.clone()?,
                                expect_exit: r.expect_exit.unwrap_or(0),
                            }),
                            "sensor_assertion" => Some(VerificationCheck::SensorAssertion {
                                tool: r.tool.clone()?,
                                contains: r.contains.clone()?,
                            }),
                            other => {
                                tracing::warn!(
                                    kind = %other,
                                    "unknown [[self_improvement.verification]] kind; ignored"
                                );
                                None
                            }
                        }?;
                        Some(oh_ben_claw::skill_forge::improve::VerificationRule {
                            skill_pattern: r.skill.clone(),
                            check,
                        })
                    })
                    .collect(),
            );
            let executor: Arc<dyn oh_ben_claw::skill_forge::improve::ReplayExecutor> =
                handle.agent_arc();
            let interval = std::time::Duration::from_secs(
                config.self_improvement.interval_secs.unwrap_or(3600),
            );
            tokio::spawn(async move {
                improver.run_periodically(executor, interval).await;
            });
            info!("Phase 16 self-improvement loop spawned");

            // Phase 16 P4: offline description evolution (config-gated, daily
            // by default). Uses its own provider instance; only rewrites
            // learned-skill descriptions — never stage/enabled/kind.
            if config.self_improvement.evolve {
                match providers::from_config(&config.provider) {
                    Ok(evolve_provider) => {
                        let evolver = oh_ben_claw::skill_forge::evolve::DescriptionEvolver::new(
                            oh_ben_claw::skill_forge::SkillForge::new(
                                oh_ben_claw::skill_forge::SkillForge::default_dir(),
                            ),
                            Arc::clone(traj),
                            evolve_provider,
                            config.provider.clone(),
                            oh_ben_claw::skill_forge::evolve::default_log_path(),
                            config.self_improvement.evolve_max_per_pass.unwrap_or(5),
                        );
                        let evolve_interval = std::time::Duration::from_secs(
                            config
                                .self_improvement
                                .evolve_interval_secs
                                .unwrap_or(86_400),
                        );
                        tokio::spawn(async move {
                            evolver.run_periodically(evolve_interval).await;
                        });
                        info!("Phase 16 offline evolution job spawned");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Phase 16 evolution: provider unavailable");
                    }
                }
            }
        }
    }

    // Phase 17: spawn autostart harness missions — durable, resumable,
    // self-verifying long-horizon objectives (design: docs/PHASE17-PLAN.md).
    if config.harness.enabled {
        let pass_delay =
            std::time::Duration::from_millis(config.harness.pass_delay_ms.unwrap_or(2_000));
        let max_passes = config.harness.max_passes.unwrap_or(1_000);
        for mission_cfg in config.harness.mission.iter().filter(|m| m.autostart) {
            use oh_ben_claw::harness::{Harness, HarnessCheck, Objective, ProgressStore};

            // Map config objectives + checks into the harness types.
            let objectives: Vec<Objective> = mission_cfg
                .objective
                .iter()
                .map(|o| Objective {
                    id: o.id.clone(),
                    description: o.description.clone(),
                    verify: o
                        .verify
                        .iter()
                        .filter_map(|c| match c.kind.as_str() {
                            "tool_contains" => Some(HarnessCheck::ToolContains {
                                tool: c.tool.clone()?,
                                args: c.args.clone().unwrap_or(serde_json::json!({})),
                                contains: c.contains.clone()?,
                            }),
                            "command" => Some(HarnessCheck::Command {
                                cmd: c.cmd.clone()?,
                                expect_exit: c.expect_exit.unwrap_or(0),
                            }),
                            "world_fact" => Some(HarnessCheck::WorldFact {
                                entity: c.entity.clone()?,
                                contains: c.contains.clone()?,
                            }),
                            other => {
                                tracing::warn!(kind = %other, "unknown harness check kind; ignored");
                                None
                            }
                        })
                        .collect(),
                    status: oh_ben_claw::harness::ObjectiveStatus::Pending,
                    attempts: 0,
                    max_attempts: o.max_attempts.unwrap_or(3),
                    note: String::new(),
                })
                .collect();

            // Dedicated conversation session per mission (idempotent).
            let harness_session = format!("harness-{}", mission_cfg.name);
            let _ = memory.create_session_with_id(&harness_session);

            let mut harness = Harness::new(
                ProgressStore::new(ProgressStore::default_dir()),
                handle.agent_arc(),
                config.provider.clone(),
                harness_session,
            );
            if let Some(world) = &world_mem {
                harness = harness.with_world(Arc::clone(world));
            }
            let mission_name = mission_cfg.name.clone();
            tokio::spawn(async move {
                match harness.initialize(&mission_name, objectives) {
                    Ok(mut record) => {
                        if let Err(e) = harness
                            .run_mission(&mut record, max_passes, pass_delay)
                            .await
                        {
                            tracing::warn!(mission = %mission_name, error = %e, "harness mission errored");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(mission = %mission_name, error = %e, "harness init failed")
                    }
                }
            });
            info!(mission = %mission_cfg.name, "Phase 17 harness mission spawned");
        }
    }

    // Start the gateway (with live agent attached) if enabled
    let _gateway_state = if config.gateway.enabled {
        // Build the full gateway state with all subsystems (reusing the shared
        // observability context so reflex/safing fire counts appear in /metrics).
        let obs = Arc::clone(&obs);
        // Scheduler::new takes a *path*, not a name. Passing the agent name
        // created a relative file in the process's working directory -- and
        // then `.unwrap()`ed the fallback, so the whole agent panicked on
        // startup whenever that directory wasn't writable. Every service
        // manager runs with a working directory the user doesn't control
        // (Task Scheduler uses System32), which made this fatal for any
        // unattended start. Resolve a real path in the data directory.
        let sched_path = oh_ben_claw::config::paths::in_data_dir("scheduler.db");
        let sched = match scheduler::Scheduler::new(&sched_path.to_string_lossy()) {
            Ok(s) => s,
            Err(e) => {
                // In-memory keeps the gateway serving; scheduled tasks simply
                // don't survive a restart. Better than refusing to start.
                tracing::warn!(
                    path = %sched_path.display(),
                    error = %e,
                    "scheduler database unavailable - falling back to in-memory (tasks will not persist)"
                );
                scheduler::Scheduler::new(":memory:").expect("in-memory scheduler cannot fail")
            }
        };
        let mut gs = gateway::GatewayState::new(config.gateway.clone())
            .with_agent(handle.clone())
            .with_memory(Arc::clone(&memory))
            .with_obs(obs)
            .with_scheduler(sched)
            .with_skills(gateway::SkillOps {
                skill_dir: oh_ben_claw::skill_forge::SkillForge::default_dir(),
                tracker: Arc::clone(&rollout_tracker),
                required_clean: config.self_improvement.promotion_clean_runs.unwrap_or(3),
            });
        if let Some(cost) = &cost_tracker {
            gs = gs.with_cost(Arc::clone(cost));
        }
        if let Some(pool) = maybe_pool.clone() {
            gs = gs.with_agent_pool(pool);
        }
        if let Some(world) = &world_mem {
            gs = gs.with_world(Arc::clone(world));
        }
        // I4 Operate mode: remote approval management + Track 0 signed audit
        // for remote mutating requests.
        gs = gs.with_approval(Arc::clone(&approval));
        if let Some(auditor) = &action_auditor {
            gs = gs.with_action_audit(Arc::clone(auditor));
        }
        if config.gateway.operate_token.is_some() {
            info!("Gateway Operate tier active (mutating requests require X-OBC-Operate)");
        }
        let state = Arc::new(gs);
        let router = gateway::build_router(state.clone());
        let bind_addr = format!("{}:{}", config.gateway.host, config.gateway.port);
        match tokio::net::TcpListener::bind(&bind_addr).await {
            Ok(listener) => {
                let url = format!("http://{}", listener.local_addr()?);
                info!(url = %url, "Gateway listening");
                let state_clone = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, router).await {
                        tracing::error!("Gateway error: {e}");
                    }
                });
                // Start tunnel if enabled
                if config.tunnel.enabled {
                    let mgr = tunnel::TunnelManager::new(config.tunnel.clone());
                    let handle_clone = handle.clone();
                    let state_clone2 = state.clone();
                    tokio::spawn(async move {
                        match mgr.start().await {
                            Ok(public_url) => {
                                info!(url = %public_url, "Tunnel active — public access at {}", public_url);
                                handle_clone.set_tunnel_url(Some(public_url.clone())).await;
                                state_clone2.broadcast(gateway::GatewayEvent::Status {
                                    agent_running: true,
                                    node_count,
                                    tunnel_url: Some(public_url),
                                });
                            }
                            Err(e) => tracing::warn!("Tunnel failed to start: {}", e),
                        }
                    });
                }

                // Broadcast initial status to any early SSE subscribers
                state_clone.broadcast(gateway::GatewayEvent::Status {
                    agent_running: true,
                    node_count,
                    tunnel_url: None,
                });

                Some(state)
            }
            Err(e) => {
                // This used to be a bare warning, and the agent carried on
                // running with no API at all: the process list looked healthy,
                // the logs looked nearly normal, and nothing answered on the
                // port. The usual cause is a restart racing the outgoing
                // process for the socket, so name that explicitly rather than
                // leaving the operator to infer it from an OS error code.
                tracing::error!(
                    host = %config.gateway.host,
                    port = config.gateway.port,
                    error = %e,
                    "GATEWAY FAILED TO BIND - the agent is running with no API. \
                     Usually the port is still held by a previous instance \
                     (wait for it to exit) or taken by another service."
                );
                None
            }
        }
    } else {
        None
    };

    // Spawn background communication channels.
    spawn_channels(Arc::clone(&agent), &config);

    // Run the interactive CLI channel
    let cli_channel = CliChannel::new(agent, config.provider.clone(), session_id);
    cli_channel.run().await?;

    Ok(())
}

/// Spawn all configured communication channels as independent background tasks.
///
/// Each channel runs in its own `tokio::spawn` task with an infinite retry
/// loop so that transient errors (network blips, rate-limits, WebSocket
/// reconnects) don't crash the entire process.
fn spawn_channels(agent: Arc<Agent>, config: &Config) {
    let provider = config.provider.clone();

    // ── Telegram ──────────────────────────────────────────────────────────────
    if let Some(ch) = TelegramChannel::new(
        &config.channels.telegram,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting Telegram channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Telegram channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Discord ───────────────────────────────────────────────────────────────
    if let Some(ch) = DiscordChannel::new(
        &config.channels.discord,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting Discord channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Discord channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Slack ─────────────────────────────────────────────────────────────────
    if let Some(ch) =
        SlackChannel::new(&config.channels.slack, Arc::clone(&agent), provider.clone())
    {
        info!("Starting Slack channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Slack channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── WhatsApp ──────────────────────────────────────────────────────────────
    if let Some(ch) = WhatsAppChannel::new(
        &config.channels.whatsapp,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting WhatsApp channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "WhatsApp channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── iMessage (macOS only) ─────────────────────────────────────────────────
    if let Some(ch) = IMessageChannel::new(
        &config.channels.imessage,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting iMessage channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "iMessage channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Matrix ──────────────────────────────────────────────────────────────────
    if let Some(ch) = MatrixChannel::new(
        &config.channels.matrix,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting Matrix channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Matrix channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // The four below were written, unit-tested, and never added to this function.
    // `channels/mod.rs` re-exported them, which is why nothing looked wrong: the
    // types were reachable, so no survey flagged them, and the README counted
    // eleven channels while seven could start. Each `new()` returns None when the
    // channel is unconfigured, so wiring them changes nothing for anyone who has
    // not set the corresponding config block.
    //
    // Wired 2026-07-30, and not verified against a live server — no IRC network,
    // no signal-cli daemon, no Mattermost or Feishu tenant was available. Unit
    // tests only. tests/channel_wiring.rs keeps this list and the exports in step.

    // ── IRC ─────────────────────────────────────────────────────────────────────
    if let Some(ch) = IrcChannel::new(&config.channels.irc, Arc::clone(&agent), provider.clone()) {
        info!("Starting IRC channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "IRC channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Signal ──────────────────────────────────────────────────────────────────
    if let Some(ch) = SignalChannel::new(
        &config.channels.signal,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting Signal channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Signal channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Mattermost ──────────────────────────────────────────────────────────────
    if let Some(ch) = MattermostChannel::new(
        &config.channels.mattermost,
        Arc::clone(&agent),
        provider.clone(),
    ) {
        info!("Starting Mattermost channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Mattermost channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }

    // ── Feishu ──────────────────────────────────────────────────────────────────
    if let Some(ch) = FeishuChannel::new(&config.channels.feishu, Arc::clone(&agent), provider) {
        info!("Starting Feishu channel");
        tokio::spawn(async move {
            loop {
                if let Err(e) = ch.run().await {
                    tracing::warn!(error = %e, "Feishu channel error; restarting in 10 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                }
            }
        });
    }
}

async fn run_status(config: &Config) -> Result<()> {
    println!("\n🦀🧠 Oh-Ben-Claw Status\n");
    println!("Version:  {}", env!("CARGO_PKG_VERSION"));
    println!("Agent:    {}", config.agent.name);
    println!(
        "Provider: {} / {}",
        config.provider.name, config.provider.model
    );
    println!(
        "Spine:    {} @ {}:{}",
        config.spine.kind, config.spine.host, config.spine.port
    );

    // Memory stats
    match MemoryStore::open() {
        Ok(memory) => {
            let sessions = memory.list_sessions().unwrap_or_default();
            println!("\nMemory:   {} session(s)", sessions.len());
            for s in sessions.iter().take(5) {
                let count = memory.message_count(&s.id).unwrap_or(0);
                println!("  [{}] {} — {} messages", s.id, s.title, count);
            }
        }
        Err(e) => println!("Memory:   unavailable ({})", e),
    }

    println!(
        "\nPeripherals ({} configured):",
        config.peripherals.boards.len()
    );
    if config.peripherals.boards.is_empty() {
        println!("  (none)");
    }
    for board in &config.peripherals.boards {
        println!(
            "  {} | transport: {} | path: {}",
            board.board,
            board.transport,
            board.path.as_deref().unwrap_or("native")
        );
    }
    // Mesh nodes (Phase B): per-node health derived by the mesh supervisor, read from
    // world memory. Shows online/degraded/offline, link RSSI, last message, and age;
    // flags any node the supervisor has escalated (presumed lost).
    if config.perception.world_memory {
        let world_path = config.perception.world_db_path.clone().unwrap_or_else(|| {
            oh_ben_claw::config::paths::in_data_dir("world.db")
                .to_string_lossy()
                .into_owned()
        });
        if let Ok(world) = oh_ben_claw::memory::world::WorldMemory::open(&world_path) {
            let views = oh_ben_claw::spine::mesh_supervisor::snapshot(&world);
            if !views.is_empty() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                println!("\nMesh nodes ({}):", views.len());
                for v in &views {
                    let health = v.prev_health.map(|h| h.as_str()).unwrap_or("unknown");
                    let rollup = world.current(&format!("mesh.{}", v.node)).ok().flatten();
                    let rssi = rollup
                        .as_ref()
                        .and_then(|f| f.value.get("rssi_dbm").and_then(|r| r.as_i64()));
                    let last_type = rollup
                        .as_ref()
                        .and_then(|f| f.value.get("last_type").and_then(|t| t.as_str()))
                        .unwrap_or("-")
                        .to_string();
                    let escalated = world
                        .current(&format!("mesh.{}.escalation", v.node))
                        .ok()
                        .flatten()
                        .and_then(|f| {
                            f.value
                                .get("status")
                                .and_then(|s| s.as_str())
                                .map(|s| s == "escalated")
                        })
                        .unwrap_or(false);
                    let tag = if escalated { " (presumed lost)" } else { "" };
                    let rssi_s = rssi
                        .map(|r| format!("{r} dBm"))
                        .unwrap_or_else(|| "-".to_string());
                    let age_s = now.saturating_sub(v.last_seen_ms) / 1000;
                    println!(
                        "  {} | {}{} | rssi: {} | last: {} ({}s ago)",
                        v.node, health, tag, rssi_s, last_type, age_s
                    );
                }
            }

            // Recent escalations (notifications log-of-record), newest first.
            let escalations: Vec<(u64, String)> = world
                .history("notifications.escalation")
                .unwrap_or_default()
                .into_iter()
                .filter_map(|f| {
                    f.value
                        .get("reason")
                        .and_then(|r| r.as_str())
                        .map(|r| (f.valid_from, r.to_string()))
                })
                .filter(|(_, r)| !r.starts_with(oh_ben_claw::agent::notify::DIGEST_PREFIX))
                .collect();
            if !escalations.is_empty() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                println!("\nRecent escalations ({}):", escalations.len());
                for (ts, reason) in escalations.iter().rev().take(5) {
                    let sev = oh_ben_claw::agent::notify::Severity::classify(reason);
                    let head = reason.split_once(". ").map(|(h, _)| h).unwrap_or(reason);
                    let age_s = now.saturating_sub(*ts) / 1000;
                    println!("  [{}] {} ({}s ago)", sev.as_str(), head, age_s);
                }
            }
        }
    }

    println!();
    Ok(())
}

async fn run_peripheral(mut config: Config, cmd: PeripheralCommands) -> Result<()> {
    match cmd {
        PeripheralCommands::List => {
            println!("\n🔌 Configured Peripheral Nodes\n");
            if config.peripherals.boards.is_empty() {
                println!("No peripheral nodes configured.");
                println!("Use `oh-ben-claw peripheral add <board> <path>` to add one.");
            } else {
                for board in &config.peripherals.boards {
                    println!(
                        "  {} | transport: {} | path: {}",
                        board.board,
                        board.transport,
                        board.path.as_deref().unwrap_or("native")
                    );
                }
            }
        }
        PeripheralCommands::Add { board, path } => {
            let transport = if path == "native" {
                "native"
            } else if path.starts_with("mqtt") {
                "mqtt"
            } else {
                "serial"
            };
            let new_board = config::PeripheralBoardConfig {
                board: board.clone(),
                transport: transport.to_string(),
                path: if path == "native" { None } else { Some(path) },
                baud: 115_200,
                node_id: None,
            };
            config.peripherals.boards.push(new_board);
            config.peripherals.enabled = true;
            config.save()?;
            println!("✅ Added peripheral: {} ({})", board, transport);
        }
        PeripheralCommands::Remove { board } => {
            config.peripherals.boards.retain(|b| b.board != board);
            config.save()?;
            println!("✅ Removed peripheral: {}", board);
        }
        PeripheralCommands::Capabilities { board } => {
            if let Some(info) = peripherals::registry::known_boards()
                .iter()
                .find(|b| b.name == board)
            {
                println!("\n📋 Capabilities for {}\n", board);
                println!("Architecture: {}", info.architecture.unwrap_or("unknown"));
                println!("Transport:    {}", info.transport);
                println!("Capabilities: {}", info.capabilities.join(", "));
            } else {
                bail!(
                    "Unknown board: '{}'. Use `oh-ben-claw peripheral list` to see configured boards.",
                    board
                );
            }
        }
    }
    Ok(())
}

/// Measure the perception detector's false-negative rate against a labeled eval
/// set and print the report. This is the residual risk the conscience gate can't
/// cover — it can only gate labels the detector emits, so a person the detector
/// misses entirely is never seen by the gate. See `obc_conscience::detector_eval`.
fn run_eval_detector(config: &Config, frames_path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(frames_path)
        .with_context(|| format!("reading eval frames from {frames_path}"))?;
    let frames: Vec<obc_conscience::EvalFrame> = serde_json::from_str(&raw)
        .context("parsing eval frames JSON (expected an array of {truth, detected})")?;
    if frames.is_empty() {
        bail!("no evaluation frames in {frames_path} — nothing to measure");
    }
    // Classify with the deployment's own vocabulary so the measurement matches
    // what the live gate would do with these labels.
    let classifier = obc_conscience::SubjectClassifier::from_config(&config.conscience.classifier);
    let report = obc_conscience::measure_false_negatives(&frames, &classifier);
    print!("{report}");
    Ok(())
}

/// Replay a conscience decision log against the current policy and print the
/// report. Exits non-zero only on an *unexplained* mismatch (same policy
/// fingerprint, different verdict — a determinism bug); policy drift is
/// informational and does not fail. See `obc_conscience::replay`.
fn run_replay_decisions(config: &Config, log_path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(log_path)
        .with_context(|| format!("reading decision log from {log_path}"))?;
    // Accepts both a JSON array and JSON Lines (the runtime decision log is JSONL).
    let records = oh_ben_claw::decision_log::parse_log(&raw)?;
    if records.is_empty() {
        bail!("no decision records in {log_path} — nothing to replay");
    }
    let conscience = obc_conscience::Conscience::new(&config.conscience);
    let report = obc_conscience::replay_decisions(&records, &conscience, &config.conscience);
    print!("{report}");
    // A same-config verdict flip is a real defect; fail so CI notices. Policy
    // drift is expected review output, not an error.
    if report.undrifted_mismatches() > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Decide a multi-party-consent frame from a frame file + a consent ledger, and
/// print the verdict. Consent is opt-in and token-based (never identity
/// recognition); the restricted class comes from `[conscience.classifier]`. See
/// `obc_conscience::multiparty`.
fn run_consent_check(
    config: &Config,
    frame_path: &str,
    ledger_path: &str,
    purpose: &str,
    policy: &str,
) -> Result<()> {
    let subjects: Vec<obc_conscience::SubjectPresence> = serde_json::from_str(
        &std::fs::read_to_string(frame_path)
            .with_context(|| format!("reading frame from {frame_path}"))?,
    )
    .context("parsing frame JSON (expected an array of {class, consent_token?})")?;
    let grants: Vec<obc_conscience::ConsentGrant> = serde_json::from_str(
        &std::fs::read_to_string(ledger_path)
            .with_context(|| format!("reading ledger from {ledger_path}"))?,
    )
    .context("parsing ledger JSON (expected an array of consent grants)")?;
    let policy = match policy {
        "require-all" => obc_conscience::ConsentPolicy::RequireAll,
        "redact" => obc_conscience::ConsentPolicy::Redact,
        other => bail!("unknown policy '{other}' (require-all | redact)"),
    };
    let ledger = obc_conscience::ConsentLedger::from_grants(grants);
    let restricted = &config.conscience.classifier.restricted_class;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let decision =
        obc_conscience::decide_frame(&subjects, &ledger, restricted, purpose, now_ms, policy);
    match &decision {
        obc_conscience::FrameConsent::Allow => println!(
            "ALLOW — every '{restricted}' in the frame presented valid consent for '{purpose}' (or none present)."
        ),
        obc_conscience::FrameConsent::Refuse { unconsented } => println!(
            "REFUSE — {unconsented} subject(s) lacked valid consent; the whole frame is dropped (require-all)."
        ),
        obc_conscience::FrameConsent::Redact { indices } => println!(
            "REDACT — blank subject indices {indices:?} before storing; a body that cannot redact per-subject must refuse the frame."
        ),
    }
    Ok(())
}

/// `oh-ben-claw mcp-serve` — run the MCP server standalone with the default
/// tool set. The http transport is what the official conformance suite tests
/// (`npx @modelcontextprotocol/conformance server --url http://…/mcp`).
async fn run_mcp_serve(config: &Config, transport: &str, port: u16, mode: &str) -> Result<()> {
    use oh_ben_claw::mcp::{server::McpServer, ProtocolMode};
    use oh_ben_claw::tools::{default_tools, default_tools_with_reach};

    let mode = match mode {
        "stateless-2026" => ProtocolMode::Stateless2026,
        "legacy-2024" => ProtocolMode::Legacy2024,
        other => anyhow::bail!("unknown protocol mode '{other}' (legacy-2024 | stateless-2026)"),
    };
    // Tools exposed over MCP are an egress surface too — an external client could
    // drive the HTTP/browser tools through this server. Gate them with the
    // conscience reach allowlist when enabled. (No auditor in this standalone
    // path; the gate still refuses, it just isn't logged here.)
    let conscience = obc_conscience::Conscience::new(&config.conscience);
    let tools = if conscience.enabled {
        default_tools_with_reach(Some(conscience.reach.clone()), None, None)
    } else {
        default_tools()
    };
    let server = McpServer::with_mode(tools, mode);

    match transport {
        "stdio" => {
            // stdout is the JSON-RPC stream — status goes to stderr (MCP rule).
            eprintln!("MCP server on stdio ({} tools)", server.tool_count());
            server.run_stdio().await
        }
        "http" => {
            let router = server.http_router();
            let addr = format!("127.0.0.1:{port}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!(url = %format!("http://{addr}/mcp"), mode = ?mode, "MCP server on http");
            axum::serve(listener, router).await?;
            Ok(())
        }
        other => anyhow::bail!("unknown transport '{other}' (stdio | http)"),
    }
}

/// `oh-ben-claw a2a-serve` — expose this agent to other agents.
///
/// Serves the two things the A2A spec requires to be reachable: the well-known
/// agent card (discovery) and the JSON-RPC endpoint. The card advertises the
/// agent's tools as skills, so a caller can see what it is being offered.
///
/// This is the entry point the module spent five months without. `--echo` keeps
/// the old stub behaviour available on purpose: it serves the protocol with no
/// model behind it, which is what a conformance run wants and is the honest way
/// to describe what `SendMessage` did for everyone before today.
async fn run_a2a_serve(
    config: &Config,
    port: u16,
    advertise: Option<&str>,
    echo: bool,
) -> Result<()> {
    use oh_ben_claw::a2a::{A2AServer, EchoExecutor, TaskExecutor};
    use oh_ben_claw::a2a_agent::AgentExecutor;
    use oh_ben_claw::agent::Agent;
    use oh_ben_claw::memory::MemoryStore;
    use oh_ben_claw::tools::{default_tools, default_tools_with_reach};

    let addr = format!("127.0.0.1:{port}");
    // `[a2a]` in the config file was parsed and never read by anything —
    // `config.a2a` had zero reads in `src/` and `tests/`, so `enabled = true`
    // changed nothing. It is read here. `enabled` gates the command rather than
    // being ignored, and the card fields are the config's, with `--advertise`
    // as the override for the common case where the reachable URL differs from
    // the bind address.
    if !config.a2a.enabled {
        anyhow::bail!(
            "A2A is disabled — set `enabled = true` under [a2a] in your config to serve it"
        );
    }
    let url = advertise
        .map(str::to_string)
        .unwrap_or_else(|| config.a2a.agent_url.clone());

    // Same reasoning as mcp-serve: an A2A caller is an external party driving
    // this agent's tools, so the conscience reach allowlist gates them when it
    // is enabled.
    let conscience = obc_conscience::Conscience::new(&config.conscience);
    let tools = if conscience.enabled {
        default_tools_with_reach(Some(conscience.reach.clone()), None, None)
    } else {
        default_tools()
    };
    // Skills advertised on the card: the config's list when it names any,
    // otherwise the agent's actual tool names. A card that advertises a skill
    // the agent does not have is a lie told at discovery time, so the fallback
    // is what is really loaded rather than a hopeful default.
    let skill_names: Vec<String> = if config.a2a.skills.is_empty() {
        tools.iter().map(|t| t.name().to_string()).collect()
    } else {
        config.a2a.skills.clone()
    };

    let (executor, how): (Arc<dyn TaskExecutor>, &str) = if echo {
        (Arc::new(EchoExecutor), "echo (no model)")
    } else {
        let provider = providers::from_config(&config.provider)?;
        let memory = Arc::new(MemoryStore::open()?);
        let agent = Arc::new(Agent::new(config.agent.clone(), provider, memory, tools));
        (
            Arc::new(AgentExecutor::new(agent, config.provider.clone())),
            "agent dispatch",
        )
    };

    let card = A2AServer::build_card(
        &config.a2a.agent_name,
        &config.a2a.agent_description,
        &url,
        env!("CARGO_PKG_VERSION"),
        &skill_names,
    );
    let router = oh_ben_claw::a2a::http_router(A2AServer::new(card), executor);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(
        card = %format!("http://{addr}/.well-known/agent-card.json"),
        rpc = %format!("http://{addr}/a2a"),
        advertised = %url,
        execution = how,
        skills = skill_names.len(),
        "A2A server listening"
    );
    axum::serve(listener, router).await?;
    Ok(())
}

/// `oh-ben-claw judge-calibrate` — measure whether the configured LLM judge
/// agrees with a human gold set (Cohen's κ) before it is trusted for anything
/// beyond advice. Prints the calibration report and exits non-zero when the
/// judge is not configured (so it's scriptable in CI).
async fn run_judge_calibrate(gold: Option<&str>, threshold: f32) -> Result<()> {
    use oh_ben_claw::agent::judge::{CalibrationCase, LlmJudge};

    let Some(judge) = LlmJudge::from_env() else {
        anyhow::bail!(
            "no judge configured — set OBC_JUDGE_PROVIDER and OBC_JUDGE_MODEL \
             (optionally OBC_JUDGE_API_KEY / OBC_JUDGE_BASE_URL)"
        );
    };

    // Gold set: explicit --gold, else OBC_JUDGE_GOLD, else the built-in seed set.
    let gold_path = gold
        .map(String::from)
        .or_else(|| std::env::var("OBC_JUDGE_GOLD").ok());
    let (cases, source) = match &gold_path {
        Some(path) => (CalibrationCase::load(path)?, path.as_str()),
        None => (CalibrationCase::seed_set(), "built-in seed set"),
    };

    info!(
        model = judge.model(),
        cases = cases.len(),
        gold = source,
        "running judge calibration"
    );
    let report = judge.calibrate(&cases, threshold).await;

    println!("{}", serde_json::to_string_pretty(&report)?);
    println!(
        "\nJudge '{}' (rubric v{}): κ = {:.3} over {} case(s) ({} error(s)) → {}",
        report.judge_model,
        report.rubric_version,
        report.kappa,
        report.n,
        report.errors,
        if report.calibrated {
            "CALIBRATED (κ ≥ 0.6 — trustworthy as an advisory signal)"
        } else {
            "NOT CALIBRATED (κ < 0.6 — treat scores with caution; never gate on them)"
        }
    );
    // Exit non-zero when not calibrated so this is usable as a deployment gate
    // (`judge-calibrate && …`). This is a standalone operator command — it gates
    // nothing inside the running agent; Track 0 owns actuation safety.
    if !report.calibrated {
        std::process::exit(1);
    }
    Ok(())
}

/// `oh-ben-claw skill …` — learned-skill management + Track 0 staged rollout.
/// Works directly on the forge directory and rollout record; a running agent
/// picks changes up via the gateway endpoints or its next improvement pass.
fn run_skill(config: &Config, cmd: SkillCommands) -> Result<()> {
    use oh_ben_claw::skill_forge::{rollout, SkillForge};

    let forge = SkillForge::new(SkillForge::default_dir());
    let tracker = rollout::RolloutTracker::load(rollout::RolloutTracker::default_path());
    let required = config.self_improvement.promotion_clean_runs.unwrap_or(3);

    match cmd {
        SkillCommands::List => {
            let manifests = forge.list_manifests()?;
            if manifests.is_empty() {
                println!("No skills installed in {}", forge.skill_dir.display());
                return Ok(());
            }
            println!(
                "{:<40} {:<11} {:<8} {:<7} {:<9} TAGS",
                "NAME", "STAGE", "ENABLED", "CLEAN", "FAILURES"
            );
            for m in manifests {
                let rec = tracker.record(&m.name).unwrap_or_default();
                let (clean, failures) = if rec.stage == m.stage {
                    (rec.clean_runs, rec.failures)
                } else {
                    (0, 0)
                };
                println!(
                    "{:<40} {:<11} {:<8} {:<7} {:<9} {}",
                    m.name,
                    m.stage.as_str(),
                    m.enabled,
                    clean,
                    failures,
                    m.tags.join(",")
                );
            }
        }
        SkillCommands::Show { name } => {
            let manifest = forge
                .list_manifests()?
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| anyhow::anyhow!("no skill named '{name}'"))?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            match tracker.record(&name) {
                Some(rec) => println!(
                    "\nRollout record: stage={} clean_runs={} failures={} (promotion needs {} clean)",
                    rec.stage.as_str(),
                    rec.clean_runs,
                    rec.failures,
                    required
                ),
                None => println!("\nRollout record: none (promotion needs {required} clean runs)"),
            }
        }
        SkillCommands::Promote { name } => {
            let stage = rollout::promote(&forge, &tracker, &name, required)?;
            println!("'{name}' promoted to stage '{}'", stage.as_str());
            println!("(a running agent applies this on its next self-improvement pass, or promote via the gateway for an immediate hot reload)");
        }
        SkillCommands::Demote { name } => {
            let stage = rollout::demote(&forge, &tracker, &name)?;
            println!("'{name}' demoted to stage '{}'", stage.as_str());
        }
        SkillCommands::ResetRecord { name } => {
            let manifest = forge
                .list_manifests()?
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| anyhow::anyhow!("no skill named '{name}'"))?;
            tracker.reset(&name, manifest.stage);
            println!(
                "record for '{name}' reset at stage '{}'",
                manifest.stage.as_str()
            );
        }
        SkillCommands::RevertDescription { name } => {
            let restored = oh_ben_claw::skill_forge::evolve::revert_description(
                &forge,
                &oh_ben_claw::skill_forge::evolve::default_log_path(),
                &name,
            )?;
            println!("'{name}' description reverted to: {restored}");
        }
        SkillCommands::Remove { name } => {
            forge.remove_skill(&name)?;
            println!("'{name}' removed from the forge");
        }
    }
    Ok(())
}

async fn run_history(cmd: HistoryCommands) -> Result<()> {
    let memory = MemoryStore::open()?;
    match cmd {
        HistoryCommands::List => {
            let sessions = memory.list_sessions()?;
            if sessions.is_empty() {
                println!("No conversation sessions found.");
            } else {
                println!("\n📚 Conversation Sessions\n");
                for s in &sessions {
                    let count = memory.message_count(&s.id).unwrap_or(0);
                    println!(
                        "  [{}] {} — {} messages (updated {})",
                        s.id,
                        s.title,
                        count,
                        s.updated_at.format("%Y-%m-%d %H:%M")
                    );
                }
            }
        }
        HistoryCommands::Clear { session } => {
            memory.clear_session(&session)?;
            println!("✅ Cleared history for session '{}'", session);
        }
        HistoryCommands::Show { session, limit } => {
            let messages = memory.load_recent_messages(&session, limit)?;
            if messages.is_empty() {
                println!("No messages in session '{}'.", session);
            } else {
                println!("\n📖 Session '{}' (last {} messages)\n", session, limit);
                for msg in &messages {
                    let role_label = match msg.role {
                        providers::ChatRole::System => "system",
                        providers::ChatRole::User => "you",
                        providers::ChatRole::Assistant => "obc",
                    };
                    println!("[{}] {}", role_label, msg.content);
                    println!();
                }
            }
        }
    }
    Ok(())
}

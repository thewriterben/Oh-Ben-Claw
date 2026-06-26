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

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::sync::Arc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use oh_ben_claw::agent::{Agent, AgentHandle, OrchestratorAgent};
use oh_ben_claw::channels::{
    CliChannel, DiscordChannel, IMessageChannel, MatrixChannel, SlackChannel, TelegramChannel,
    WhatsAppChannel,
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

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
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

    // Build tool registry
    let mut all_tools = default_tools();
    let mut node_count = 0usize;

    // Connect to MQTT spine and discover peripheral tools
    let spine_client = if !no_spine && config.spine.kind == "mqtt" {
        match spine::SpineClient::new(config.spine.clone(), "obc-brain")
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
    }

    // ── Track 0: physical-action safety (shared by plain agent + orchestrator) ──
    // Resolve the audit MAC key with a secure precedence:
    //   explicit config key > vault (if unlockable via OBC_VAULT_PASSWORD)
    //   > node pairing secret > a persisted, auto-generated random key.
    // The old hardcoded dev key is gone — a published constant would let anyone
    // forge audit entries.
    fn track0_data_path(name: &str) -> String {
        directories::ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .map(|d| d.data_dir().join(name).to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string())
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
                trajectory_store = Some(Arc::new(store));
                info!(path = %traj_path, "Phase 16 trajectory capture active");
            }
            Err(e) => tracing::warn!("Phase 16: failed to open trajectory store: {e}"),
        }
    }

    // Phase 18: open world memory once, shared by the world_memory tool and the
    // reflex controller.
    let world_mem: Option<Arc<oh_ben_claw::memory::world::WorldMemory>> =
        if config.perception.world_memory {
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

    // Movement subsystem: expose the safety-bounded `move_actuator` tool to the
    // agent (System 2), and keep the controller so System 1 reflexes can route
    // `Action::Move` through the *same* gate (below). Physical actuation MUST be
    // deterministically bounded (Suite §7), so it requires the Track 0 gate;
    // commanded state is recorded into world memory when available.
    let movement_controller: Option<Arc<oh_ben_claw::movement::MovementController>> =
        if config.movement.enabled {
            match &safety_gate {
                Some(gate) => {
                    // Drive real hardware over the spine when connected; fall back
                    // to the dry-run logging sink otherwise.
                    let sink: Arc<dyn oh_ben_claw::movement::ActuatorSink> = match &reflex_spine {
                        Some(spine) => {
                            info!("Movement actuation dispatches over the spine");
                            Arc::new(oh_ben_claw::movement::SpineActuatorSink::new(Arc::clone(spine)))
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
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::sensing::SenseTool::new(
                    Arc::clone(&controller),
                    Arc::clone(world),
                )));
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
                                Arc::new(oh_ben_claw::audio::suite::SpineSpeechSink::new(Arc::clone(spine)))
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
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::audio_suite::HearTool::new(
                    Arc::clone(&controller),
                    Arc::clone(world),
                )));
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::audio_suite::SpeakTool::new(
                    Arc::clone(&controller),
                    voice,
                )));
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
                let mut controller =
                    oh_ben_claw::power::PowerController::new(thresholds).with_world_memory(Arc::clone(world));
                if let Some(source) = &config.power.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::power::PowerTool::new(
                    Arc::clone(&controller),
                    Arc::clone(world),
                )));
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
                    max_latency_ms: config.comms.max_latency_ms.unwrap_or(defaults.max_latency_ms),
                    max_loss_pct: config.comms.max_loss_pct.unwrap_or(defaults.max_loss_pct),
                };
                let mut controller =
                    oh_ben_claw::comms::CommsController::new(thresholds).with_world_memory(Arc::clone(world));
                if let Some(source) = &config.comms.source {
                    controller = controller.with_source(source.clone());
                }
                let controller = Arc::new(controller);
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::comms::CommsTool::new(
                    Arc::clone(&controller),
                    Arc::clone(world),
                )));
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
                let mut nav = oh_ben_claw::navigation::NavController::new(Arc::clone(mc), steer, drive)
                    .with_world_memory(Arc::clone(world))
                    .with_gains(gains);
                if let Some(s) = &config.navigation.source {
                    nav = nav.with_source(s.clone());
                }
                if let Some(r) = config.navigation.sensor_max_range {
                    nav = nav.with_sensor_range(r);
                }
                // Clearance-aware planning: inflate obstacles by the robot footprint.
                if let (Some(insc), Some(infl)) =
                    (config.navigation.inscribed_radius, config.navigation.inflation_radius)
                {
                    let decay = config.navigation.inflation_decay.unwrap_or(2.0);
                    nav = nav.with_inflation(insc, infl, decay);
                    info!(inscribed = insc, inflation = infl, "Navigation: clearance-aware planning (inflation)");
                }
                // Obstacle-aware planning: attach an occupancy grid if configured.
                let has_grid = config.navigation.grid.is_some();
                if let Some(gc) = &config.navigation.grid {
                    let grid = oh_ben_claw::navigation::planning::OccupancyGrid::new(
                        gc.origin_x, gc.origin_y, gc.resolution, gc.width, gc.height,
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
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::navigation::NavigateTool::new(
                    Arc::clone(&nav),
                )));
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::navigation::NavStatusTool::new(
                    Arc::clone(&nav),
                    Arc::clone(world),
                )));
                if has_grid {
                    all_tools.push(Box::new(oh_ben_claw::tools::builtin::navigation::NavMapTool::new(
                        Arc::clone(&nav),
                    )));
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
            all_tools.push(Box::new(oh_ben_claw::tools::builtin::mission::MissionStartTool::new(
                Arc::clone(&runner),
                Arc::clone(&library),
            )));
            all_tools.push(Box::new(oh_ben_claw::tools::builtin::mission::MissionStatusTool::new(
                Arc::clone(&runner),
                Arc::clone(&library),
            )));
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

    // Shared host-side safing state: flipped in-process when a safing advisory
    // fires (via the SafingSink tap below), read by load-shedding consumers
    // (e.g. the ClawCam poll backs off when shed_load is set).
    let safing_state = Arc::new(oh_ben_claw::agent::safing::SafingState::new());

    // Phase 18: spawn the dual-system reflex controller (System 1) when enabled.
    // Operator rules are merged with the standard safing rules (when [reflex]
    // safing = true) so the suite mode hooks (power.mode, net.mode, …) drive
    // deterministic safing actions. Uses the dry-run logging sink until spine.
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
        rules
    };
    if config.reflex.enabled && !reflex_rules.is_empty() {
        if let Some(world) = &world_mem {
            use oh_ben_claw::agent::reflex::{
                ActionSink, EscalationBudget, LoggingActionSink, MovementActionSink,
                ReflexController, ReflexEngine, SpineActionSink,
            };
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
            all_tools.push(Box::new(oh_ben_claw::tools::builtin::foresight::ForesightTool::new(
                Arc::clone(world),
            )));
            if !config.foresight.rules.is_empty()
                || config.learning.enabled
                || config.perception.vision_rules.enabled
            {
                use oh_ben_claw::agent::reflex::{
                    ActionSink, EscalationBudget, LoggingActionSink, SpineActionSink,
                };
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
                    foresight_rules
                        .extend(oh_ben_claw::vision::clawcam_rules::vision_foresight_rules(&opts));
                }
                let engine = oh_ben_claw::foresight::ForesightEngine::new(foresight_rules)
                    .with_learned_rules(Arc::clone(&learned_rules));
                let sink: Arc<dyn ActionSink> = match &reflex_spine {
                    Some(spine) => Arc::new(SpineActionSink::new(Arc::clone(spine))),
                    None => Arc::new(LoggingActionSink),
                };
                let mut controller =
                    oh_ben_claw::foresight::ForesightController::new(engine, Arc::clone(world), sink);
                if let Some(max) = config.foresight.max_escalations_per_min {
                    controller = controller.with_escalation_budget(EscalationBudget::per_minute(max));
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
                all_tools.push(Box::new(oh_ben_claw::tools::builtin::learn::LearnTool::new(
                    Arc::clone(world),
                    Arc::clone(&store),
                    miner.clone(),
                    outcome.clone(),
                )));
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
                                    info!(proposals = n, "learning: new rule proposals (awaiting approval)");
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
        let coord = Arc::new(coord);
        all_tools.push(Box::new(oh_ben_claw::tools::builtin::fleet::FleetTool::new(Arc::clone(
            &coord,
        ))));
        all_tools.push(Box::new(oh_ben_claw::tools::builtin::fleet::FleetStatusTool::new(
            Arc::clone(&coord),
        )));
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

    // Phase 18 / S1b: fold a ClawCam (vision subsystem) MCP server's detections
    // into world memory on a cadence — the vision subsystem feeding the brain's
    // bitemporal memory, which the reflex engine then reacts to.
    if let Some(world) = &world_mem {
        if let Some(cfg) = config.perception.clawcam_poll.clone() {
            if cfg.enabled {
                let world = Arc::clone(world);
                let poll_safing = Arc::clone(&safing_state);
                match oh_ben_claw::mcp::client::McpClient::connect(&cfg.server).await {
                    Ok(client) => {
                        let client = Arc::new(tokio::sync::Mutex::new(client));
                        let interval =
                            std::time::Duration::from_millis(cfg.interval_ms.max(250));
                        info!(
                            tool = %cfg.tool,
                            interval_ms = cfg.interval_ms,
                            "Phase 18 ClawCam → world memory poll spawned"
                        );
                        tokio::spawn(async move {
                            let mut ticker = tokio::time::interval(interval);
                            loop {
                                ticker.tick().await;
                                // Load-shedding: when safing has engaged shed_load
                                // (e.g. battery critical/low), skip the poll to drop
                                // its network + CPU cost until charge recovers.
                                if poll_safing.shed_load() {
                                    continue;
                                }
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0);
                                match oh_ben_claw::vision::clawcam_ingest::poll_clawcam_into_world(
                                    Arc::clone(&client),
                                    &world,
                                    &cfg.tool,
                                    cfg.args.clone(),
                                    now,
                                    &cfg.source,
                                )
                                .await
                                {
                                    Ok(entities) if !entities.is_empty() => {
                                        // Maintain vision.count.{subject} so foresight can
                                        // trend the detection rate over time.
                                        let _ = oh_ben_claw::vision::clawcam_ingest::record_subject_counts(
                                            &world, &entities, now, &cfg.source,
                                        );
                                        info!(
                                            count = entities.len(),
                                            "ClawCam detections folded into world memory"
                                        );
                                    }
                                    Ok(_) => {}
                                    Err(e) => tracing::warn!("ClawCam poll failed: {e}"),
                                }
                            }
                        });
                    }
                    Err(e) => tracing::warn!(
                        "Could not connect to ClawCam MCP server: {e}; skipping detection poll"
                    ),
                }
            }
        }
    }

    // Build the plain reasoning agent, attaching Track 0 + Phase 16 when configured.
    let mut agent = Agent::new(
        config.agent.clone(),
        provider,
        Arc::clone(&memory),
        all_tools,
    )
    .with_policy(security_ctx.policy);
    if let Some(gate) = &safety_gate {
        agent = agent.with_safety_gate(Arc::clone(gate));
    }
    if let Some(auditor) = &action_auditor {
        agent = agent.with_action_auditor(Arc::clone(auditor));
    }
    if let Some(t) = &trajectory_store {
        agent = agent.with_trajectory_store(Arc::clone(t));
    }
    let agent = Arc::new(agent);

    // Phase 16: spawn the autonomous self-improvement loop when enabled. It
    // periodically synthesizes + verifies skills from successful trajectories;
    // physical skills are quarantined for operator promotion (Track 0).
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
            );
            let executor: Arc<dyn oh_ben_claw::skill_forge::improve::ReplayExecutor> =
                agent.clone();
            let interval = std::time::Duration::from_secs(
                config.self_improvement.interval_secs.unwrap_or(3600),
            );
            tokio::spawn(async move {
                improver.run_periodically(executor, interval).await;
            });
            info!("Phase 16 self-improvement loop spawned");
        }
    }

    // Build orchestrator or plain agent handle
    let (handle, maybe_pool) = if config.orchestrator.enabled {
        info!("Multi-agent orchestration enabled — building OrchestratorAgent");
        let orch = OrchestratorAgent::new_with_track0(
            config.agent.clone(),
            config.provider.clone(),
            Arc::clone(&memory),
            config.orchestrator.clone(),
            session_id.clone(),
            safety_gate.clone(),
            action_auditor.clone(),
            trajectory_store.clone(),
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

    // Start the gateway (with live agent attached) if enabled
    let _gateway_state = if config.gateway.enabled {
        // Build the full gateway state with all subsystems (reusing the shared
        // observability context so reflex/safing fire counts appear in /metrics).
        let obs = Arc::clone(&obs);
        let sched = scheduler::Scheduler::new(&config.agent.name)
            .unwrap_or_else(|_| scheduler::Scheduler::new("obc").unwrap());
        let mut gs = gateway::GatewayState::new(config.gateway.clone())
            .with_agent(handle.clone())
            .with_memory(Arc::clone(&memory))
            .with_obs(obs)
            .with_scheduler(sched);
        if let Some(pool) = maybe_pool.clone() {
            gs = gs.with_agent_pool(pool);
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
                tracing::warn!("Gateway failed to bind: {}", e);
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

    // ── Matrix ────────────────────────────────────────────────────────────────
    if let Some(ch) = MatrixChannel::new(&config.channels.matrix, Arc::clone(&agent), provider) {
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

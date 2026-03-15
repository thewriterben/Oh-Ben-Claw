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

mod agent;
mod spine;
mod channels;
mod config;
mod gateway;
mod memory;
mod observability;
mod peripherals;
mod providers;
mod security;
mod tools;
mod tunnel;

use agent::Agent;
use channels::CliChannel;
use config::Config;
use memory::MemoryStore;
use tools::default_tools;

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
        Commands::Start { provider, model, session, no_spine, gateway, tunnel } => {
            // Apply CLI overrides to config
            if let Some(p) = provider {
                config.provider.name = p;
            }
            if let Some(m) = model {
                config.provider.model = m;
            }
            if gateway { config.gateway.enabled = true; }
            if tunnel { config.tunnel.enabled = true; }
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
    }

    Ok(())
}

async fn run_start(config: Config, session_id: &str, no_spine: bool) -> Result<()> {
    // Start the REST/WebSocket gateway if enabled
    let _gateway_state = if config.gateway.enabled {
        match gateway::start(config.gateway.clone()).await {
            Ok((state, url)) => {
                info!(url = %url, "Gateway started — PWA available at {}", url);
                if config.tunnel.enabled {
                    let mgr = tunnel::TunnelManager::new(config.tunnel.clone());
                    match mgr.start().await {
                        Ok(public_url) => {
                            info!(url = %public_url, "Tunnel started — public access at {}", public_url);
                            state.broadcast(gateway::GatewayEvent::Status {
                                agent_running: false,
                                node_count: 0,
                                tunnel_url: Some(public_url),
                            });
                        }
                        Err(e) => tracing::warn!("Tunnel failed to start: {}", e),
                    }
                }
                Some(state)
            }
            Err(e) => {
                tracing::warn!("Gateway failed to start: {}", e);
                None
            }
        }
    } else {
        None
    };

    info!("Starting Oh-Ben-Claw v{}", env!("CARGO_PKG_VERSION"));

    // Open memory store
    let memory = Arc::new(MemoryStore::open()?);
    let session = memory.default_session()?;
    let session_id = if session_id == "default" {
        session
    } else {
        // Create or reuse the named session
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

    // Connect to MQTT spine and discover peripheral tools
    let spine_client = if !no_spine && config.spine.kind == "mqtt" {
        match spine::SpineClient::new(config.spine.clone(), "obc-brain").connect().await {
            Ok(client) => {
                info!(
                    host = %config.spine.host,
                    port = config.spine.port,
                    "Connected to MQTT spine"
                );
                // Give nodes a moment to announce themselves
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let mqtt_tools = client.build_mqtt_tools().await;
                if !mqtt_tools.is_empty() {
                    info!(count = mqtt_tools.len(), "Discovered MQTT peripheral tools");
                    all_tools.extend(mqtt_tools);
                }
                Some(client)
            }
            Err(e) => {
                tracing::warn!("Could not connect to MQTT spine: {}. Continuing without spine.", e);
                None
            }
        }
    } else {
        None
    };

    // Connect to directly-wired peripheral boards
    if config.peripherals.enabled {
        let peripheral_tools = peripherals::create_peripheral_tools(
            &config.peripherals,
            spine_client,
        )
        .await?;
        if !peripheral_tools.is_empty() {
            info!(count = peripheral_tools.len(), "Peripheral tools registered");
            all_tools.extend(peripheral_tools);
        }
    }

    info!(
        tool_count = all_tools.len(),
        session_id = %session_id,
        "Agent ready"
    );

    // Build security context and attach policy engine to agent
    let security_ctx = security::SecurityContext::new(&config.security)
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to init security context: {}; using defaults", e);
            security::SecurityContext::new(&Default::default()).unwrap()
        });

    if security_ctx.policy.policy_count() > 0 {
        info!(
            policies = security_ctx.policy.policy_count(),
            "Security policy engine active"
        );
    }

    // Build and start the agent
    let agent = Arc::new(Agent::new(
        config.agent.clone(),
        provider,
        Arc::clone(&memory),
        all_tools,
    ).with_policy(security_ctx.policy));

    // Run the CLI channel
    let cli_channel = CliChannel::new(agent, config.provider.clone(), session_id);
    cli_channel.run().await?;

    Ok(())
}

async fn run_status(config: &Config) -> Result<()> {
    println!("\n🦀🧠 Oh-Ben-Claw Status\n");
    println!("Version:  {}", env!("CARGO_PKG_VERSION"));
    println!("Agent:    {}", config.agent.name);
    println!("Provider: {} / {}", config.provider.name, config.provider.model);
    println!("Spine:    {} @ {}:{}", config.spine.kind, config.spine.host, config.spine.port);

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

    println!("\nPeripherals ({} configured):", config.peripherals.boards.len());
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

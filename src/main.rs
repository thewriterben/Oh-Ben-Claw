//! Oh-Ben-Claw — Advanced multi-device AI assistant.
//!
//! # Usage
//!
//! ```bash
//! # Run the setup wizard
//! oh-ben-claw setup
//!
//! # Start the agent
//! oh-ben-claw start
//!
//! # Check system status
//! oh-ben-claw status
//!
//! # Manage peripheral nodes
//! oh-ben-claw peripheral list
//! oh-ben-claw peripheral add esp32-s3 /dev/ttyUSB0
//!
//! # Manage the background service
//! oh-ben-claw service install
//! oh-ben-claw service start
//! oh-ben-claw service stop
//! ```

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod agent;
mod bus;
mod channels;
mod config;
mod memory;
mod observability;
mod peripherals;
mod providers;
mod security;
mod tools;
mod tunnel;

use config::Config;

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
    /// Run the interactive setup wizard.
    Setup,

    /// Start the Oh-Ben-Claw agent.
    Start {
        /// The host to bind the HTTP gateway to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// The port to bind the HTTP gateway to.
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Check the status of the agent and all connected peripheral nodes.
    Status,

    /// Manage peripheral hardware nodes.
    #[command(subcommand)]
    Peripheral(PeripheralCommands),

    /// Manage the background service.
    #[command(subcommand)]
    Service(ServiceCommands),
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
    /// Show the capabilities of a connected peripheral node.
    Capabilities {
        /// The board type to inspect.
        board: String,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceCommands {
    /// Install the daemon service for auto-start.
    Install,
    /// Start the daemon service.
    Start,
    /// Stop the daemon service.
    Stop,
    /// Check the daemon service status.
    Status,
    /// Uninstall the daemon service.
    Uninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
    let config = Config::load()?;

    match cli.command {
        Commands::Setup => {
            run_setup().await?;
        }
        Commands::Start { host, port } => {
            run_start(config, &host, port).await?;
        }
        Commands::Status => {
            run_status(&config).await?;
        }
        Commands::Peripheral(cmd) => {
            run_peripheral(config, cmd).await?;
        }
        Commands::Service(cmd) => {
            run_service(cmd).await?;
        }
    }

    Ok(())
}

async fn run_setup() -> Result<()> {
    info!("Starting Oh-Ben-Claw setup wizard...");
    println!("\n🦀🧠 Welcome to Oh-Ben-Claw Setup!\n");
    println!("This wizard will guide you through configuring your multi-device AI assistant.");
    println!("\nConfiguration will be saved to: {:?}", Config::default_config_path()?);
    println!("\nSetup wizard is not yet fully implemented. Please edit the config file manually.");
    println!("See README.md for configuration examples.");
    Ok(())
}

async fn run_start(config: Config, host: &str, port: u16) -> Result<()> {
    info!("Starting Oh-Ben-Claw agent on {}:{}", host, port);
    println!("\n🦀🧠 Oh-Ben-Claw v{} starting...\n", env!("CARGO_PKG_VERSION"));

    // Connect to the MQTT bus
    if config.bus.kind == "mqtt" {
        info!(
            host = %config.bus.host,
            port = config.bus.port,
            "Connecting to MQTT bus"
        );
        println!("📡 Connecting to MQTT bus at {}:{}", config.bus.host, config.bus.port);
    }

    // Connect to peripheral nodes
    if config.peripherals.enabled {
        info!("Connecting to {} peripheral boards", config.peripherals.boards.len());
        let tools = peripherals::create_peripheral_tools(&config.peripherals, None).await?;
        println!("🔌 Connected {} peripheral tools", tools.len());
    }

    println!("\n✅ Oh-Ben-Claw is running. Press Ctrl+C to stop.\n");
    println!("Agent: {}", config.agent.name);
    println!("Provider: {} / {}", config.provider.name, config.provider.model);
    println!("Bus: {} @ {}:{}", config.bus.kind, config.bus.host, config.bus.port);

    // TODO: Start the full agent loop with channels, tools, and memory.
    // For now, wait for Ctrl+C.
    tokio::signal::ctrl_c().await?;
    info!("Shutting down Oh-Ben-Claw");
    Ok(())
}

async fn run_status(config: &Config) -> Result<()> {
    println!("\n🦀🧠 Oh-Ben-Claw Status\n");
    println!("Version:  {}", env!("CARGO_PKG_VERSION"));
    println!("Agent:    {}", config.agent.name);
    println!("Provider: {} / {}", config.provider.name, config.provider.model);
    println!("Bus:      {} @ {}:{}", config.bus.kind, config.bus.host, config.bus.port);
    println!("\nPeripherals ({} configured):", config.peripherals.boards.len());
    for board in &config.peripherals.boards {
        println!(
            "  - {} ({}) via {}",
            board.board,
            board.node_id.as_deref().unwrap_or("unnamed"),
            board.transport
        );
    }
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
            let transport = if path == "native" { "native" } else if path.starts_with("mqtt") { "mqtt" } else { "serial" };
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
                bail!("Unknown board: {}. Use `oh-ben-claw peripheral list` to see configured boards.", board);
            }
        }
    }
    Ok(())
}

async fn run_service(cmd: ServiceCommands) -> Result<()> {
    match cmd {
        ServiceCommands::Install => println!("Service installation is not yet implemented."),
        ServiceCommands::Start => println!("Service start is not yet implemented."),
        ServiceCommands::Stop => println!("Service stop is not yet implemented."),
        ServiceCommands::Status => println!("Service status is not yet implemented."),
        ServiceCommands::Uninstall => println!("Service uninstall is not yet implemented."),
    }
    Ok(())
}

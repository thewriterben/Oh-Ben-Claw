//! Interactive CLI channel — a readline-style terminal interface.

use crate::agent::Agent;
use crate::config::ProviderConfig;
use anyhow::Result;
use console::style;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

/// The CLI channel — runs an interactive REPL in the terminal.
pub struct CliChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    session_id: String,
}

impl CliChannel {
    /// Create a new CLI channel.
    pub fn new(agent: Arc<Agent>, provider_config: ProviderConfig, session_id: String) -> Self {
        Self {
            agent,
            provider_config,
            session_id,
        }
    }

    /// Run the interactive REPL loop.
    ///
    /// Reads lines from stdin, sends each to the agent, and prints the response.
    /// Exits on Ctrl-C, Ctrl-D, or the `/quit` command.
    pub async fn run(&self) -> Result<()> {
        self.print_banner();

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        loop {
            // Print prompt
            print!("{} ", style("you >").cyan().bold());
            stdout.flush()?;

            // Read a line
            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(0) => {
                    // EOF (Ctrl-D)
                    println!();
                    println!("{}", style("Goodbye!").dim());
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Read error: {}", e);
                    break;
                }
            }

            let input = line.trim().to_string();
            if input.is_empty() {
                continue;
            }

            // Handle built-in commands
            match input.as_str() {
                "/quit" | "/exit" | "/q" => {
                    println!("{}", style("Goodbye!").dim());
                    break;
                }
                "/clear" => {
                    print!("\x1B[2J\x1B[H");
                    stdout.flush()?;
                    self.print_banner();
                    continue;
                }
                "/tools" => {
                    println!(
                        "{} {} tools registered:",
                        style("tools >").yellow().bold(),
                        self.agent.tool_count()
                    );
                    for name in self.agent.tool_names() {
                        println!("  {}", style(name).green());
                    }
                    continue;
                }
                "/help" => {
                    self.print_help();
                    continue;
                }
                _ => {}
            }

            // Send to agent
            print!("{} ", style("obc  >").magenta().bold());
            stdout.flush()?;

            match self
                .agent
                .process(&self.session_id, &input, &self.provider_config)
                .await
            {
                Ok(response) => {
                    println!("{}", response.message);
                    if response.used_tools() {
                        let tool_names: Vec<&str> = response
                            .tool_calls
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect();
                        println!(
                            "{}",
                            style(format!("  (used tools: {})", tool_names.join(", "))).dim()
                        );
                    }
                }
                Err(e) => {
                    println!("{}", style(format!("Error: {}", e)).red());
                }
            }

            println!();
        }

        Ok(())
    }

    fn print_banner(&self) {
        println!(
            "{}",
            style("╔══════════════════════════════════════╗").cyan()
        );
        println!(
            "{}",
            style("║   Oh-Ben-Claw  —  Multi-Device AI   ║").cyan()
        );
        println!(
            "{}",
            style("╚══════════════════════════════════════╝").cyan()
        );
        println!(
            "  {} tools registered | session: {}",
            style(self.agent.tool_count()).green().bold(),
            style(&self.session_id).dim()
        );
        println!(
            "  Type {} for help or {} to quit.",
            style("/help").yellow(),
            style("/quit").yellow()
        );
        println!();
    }

    fn print_help(&self) {
        println!("{}", style("Built-in commands:").bold());
        println!("  {}  — Show this help message", style("/help").yellow());
        println!(
            "  {}  — List all registered tools",
            style("/tools").yellow()
        );
        println!("  {}  — Clear the terminal", style("/clear").yellow());
        println!("  {}  — Exit the REPL", style("/quit").yellow());
        println!();
        println!("Type any message to send it to the agent.");
        println!();
    }
}

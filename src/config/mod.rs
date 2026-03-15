//! Oh-Ben-Claw configuration schema and loading.
//!
//! Configuration is stored in TOML format at `~/.oh-ben-claw/config.toml`.
//! The `Config` struct is the root of the configuration tree.

use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Provider Configuration ───────────────────────────────────────────────────

/// Configuration for the LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// The provider name (e.g., "openai", "anthropic", "gemini", "ollama").
    #[serde(default = "default_provider_name")]
    pub name: String,
    /// The model to use (e.g., "gpt-4o", "claude-3-5-sonnet-20241022").
    #[serde(default = "default_model")]
    pub model: String,
    /// The API key for the provider. If not set, the environment variable
    /// `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc. will be used.
    #[serde(default)]
    pub api_key: Option<String>,
    /// The base URL for the provider API (for OpenAI-compatible endpoints).
    #[serde(default)]
    pub base_url: Option<String>,
    /// The default temperature for LLM calls.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
}

fn default_provider_name() -> String {
    "openai".to_string()
}

fn default_model() -> String {
    "gpt-4o".to_string()
}

fn default_temperature() -> f64 {
    0.7
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: default_provider_name(),
            model: default_model(),
            api_key: None,
            base_url: None,
            temperature: default_temperature(),
        }
    }
}

// ── Agent Configuration ──────────────────────────────────────────────────────

/// Configuration for the core agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The name of the agent (used in system prompts and UI).
    #[serde(default = "default_agent_name")]
    pub name: String,
    /// The system prompt for the agent.
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// Maximum number of tool-use iterations per user message.
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,
}

fn default_agent_name() -> String {
    "Oh-Ben-Claw".to_string()
}

fn default_system_prompt() -> String {
    "You are Oh-Ben-Claw, an advanced multi-device AI assistant. \
     You can see, hear, sense, and act in the physical world through \
     a fleet of connected hardware devices. Be helpful, precise, and proactive."
        .to_string()
}

fn default_max_tool_iterations() -> usize {
    10
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: default_agent_name(),
            system_prompt: default_system_prompt(),
            max_tool_iterations: default_max_tool_iterations(),
        }
    }
}

// ── Peripheral Configuration ─────────────────────────────────────────────────

/// Configuration for a single connected peripheral board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralBoardConfig {
    /// The board type (e.g., "waveshare-esp32-s3-touch-lcd-2.1", "nanopi-neo3").
    pub board: String,
    /// The transport type ("serial", "native", "mqtt").
    pub transport: String,
    /// The device path for serial transport (e.g., "/dev/ttyUSB0").
    #[serde(default)]
    pub path: Option<String>,
    /// The baud rate for serial transport.
    #[serde(default = "default_baud")]
    pub baud: u32,
    /// The MQTT node ID for MQTT transport.
    #[serde(default)]
    pub node_id: Option<String>,
}

fn default_baud() -> u32 {
    115_200
}

/// Configuration for the peripheral subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeripheralsConfig {
    /// Whether the peripheral subsystem is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The directory containing hardware datasheets for RAG.
    #[serde(default)]
    pub datasheet_dir: Option<String>,
    /// The list of connected peripheral boards.
    #[serde(default)]
    pub boards: Vec<PeripheralBoardConfig>,
}

// ── Spine Configuration ───────────────────────────────────────────────────────

/// Configuration for the MQTT communication spine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpineConfig {
    /// The spine kind (currently only "mqtt" is supported).
    #[serde(default = "default_bus_kind")]
    pub kind: String,
    /// The MQTT broker hostname.
    #[serde(default = "default_bus_host")]
    pub host: String,
    /// The MQTT broker port.
    #[serde(default = "default_bus_port")]
    pub port: u16,
    /// Whether to use TLS for the MQTT connection.
    #[serde(default)]
    pub tls: bool,
    /// Optional MQTT username.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional MQTT password.
    #[serde(default)]
    pub password: Option<String>,
    /// Timeout in seconds for tool call responses from peripheral nodes.
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

fn default_bus_kind() -> String {
    "mqtt".to_string()
}

fn default_bus_host() -> String {
    "localhost".to_string()
}

fn default_bus_port() -> u16 {
    1883
}

fn default_tool_timeout_secs() -> u64 {
    30
}

impl Default for SpineConfig {
    fn default() -> Self {
        Self {
            kind: default_bus_kind(),
            host: default_bus_host(),
            port: default_bus_port(),
            tls: false,
            username: None,
            password: None,
            tool_timeout_secs: default_tool_timeout_secs(),
        }
    }
}

// ── Channel Configuration ────────────────────────────────────────────────────

/// Configuration for the Telegram channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    pub token: Option<String>,
}

/// Configuration for the Discord channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    pub token: Option<String>,
}

/// Configuration for all channels.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
}

// ── Root Configuration ───────────────────────────────────────────────────────

/// The root configuration for Oh-Ben-Claw.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub spine: SpineConfig,
    #[serde(default)]
    pub peripherals: PeripheralsConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub security: crate::security::SecurityConfig,
}

impl Config {
    /// Load the configuration from the default location.
    ///
    /// The default location is `~/.oh-ben-claw/config.toml`.
    /// If the file does not exist, a default configuration is returned.
    pub fn load() -> Result<Self> {
        let config_path = Self::default_config_path()?;
        if !config_path.exists() {
            tracing::info!("No config file found at {:?}, using defaults", config_path);
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&config_path)?;
        let config: Self = toml::from_str(&content)?;
        tracing::info!("Loaded config from {:?}", config_path);
        Ok(config)
    }

    /// Save the configuration to the default location.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::default_config_path()?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        tracing::info!("Saved config to {:?}", config_path);
        Ok(())
    }

    /// Get the default configuration file path.
    pub fn default_config_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        Ok(dirs.config_dir().join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.agent.name, "Oh-Ben-Claw");
        assert_eq!(config.provider.name, "openai");
        assert_eq!(config.provider.model, "gpt-4o");
        assert_eq!(config.spine.host, "localhost");
        assert_eq!(config.spine.port, 1883);
    }

    #[test]
    fn config_serializes_and_deserializes() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.agent.name, config.agent.name);
        assert_eq!(parsed.provider.model, config.provider.model);
    }
}

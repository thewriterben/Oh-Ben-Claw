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
    /// The spine kind: `"mqtt"` (default) or `"p2p"` (broker-free local mesh).
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
    /// Path to a custom CA certificate file for TLS verification.
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    /// Path to a client certificate file for mTLS authentication.
    #[serde(default)]
    pub client_cert_path: Option<String>,
    /// Path to a client private key file for mTLS authentication.
    #[serde(default)]
    pub client_key_path: Option<String>,
    /// Optional MQTT username.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional MQTT password.
    #[serde(default)]
    pub password: Option<String>,
    /// Timeout in seconds for tool call responses from peripheral nodes.
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
    // ── P2P-specific fields (used when `kind = "p2p"`) ─────────────────────
    /// Unique identifier for this node in the P2P mesh.
    /// Defaults to a random UUID prefix if not set.
    #[serde(default)]
    pub p2p_node_id: Option<String>,
    /// Local address to bind the P2P TCP server to (default: `"0.0.0.0"`).
    #[serde(default)]
    pub p2p_bind_host: Option<String>,
    /// TCP port on which this node accepts P2P tool-call connections (default: 44445).
    #[serde(default)]
    pub p2p_tcp_port: Option<u16>,
    /// UDP port used for P2P peer discovery broadcasts (default: 44444).
    #[serde(default)]
    pub p2p_discovery_port: Option<u16>,
    /// Seconds after which a silent peer is removed from the P2P registry (default: 60).
    #[serde(default)]
    pub p2p_peer_timeout_secs: Option<u64>,
    /// How often (in seconds) to broadcast a P2P presence announcement (default: 10).
    #[serde(default)]
    pub p2p_announce_interval_secs: Option<u64>,
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
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            username: None,
            password: None,
            tool_timeout_secs: default_tool_timeout_secs(),
            p2p_node_id: None,
            p2p_bind_host: None,
            p2p_tcp_port: None,
            p2p_discovery_port: None,
            p2p_peer_timeout_secs: None,
            p2p_announce_interval_secs: None,
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

/// Configuration for the Slack channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    /// App-Level Token (`xapp-…`) required for Socket Mode.
    pub app_token: Option<String>,
    /// Bot User OAuth Token (`xoxb-…`) used to post messages.
    pub bot_token: Option<String>,
}

/// Configuration for the WhatsApp Business Cloud API channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    /// Meta Graph API access token.
    pub access_token: Option<String>,
    /// WhatsApp Business phone number ID.
    pub phone_number_id: Option<String>,
    /// Webhook verify token (must match the value set in the Meta dashboard).
    pub verify_token: Option<String>,
    /// Local port for the webhook HTTP server (default: 8444).
    pub webhook_port: Option<u16>,
}

/// Configuration for the iMessage channel (macOS only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IMessageConfig {
    /// Whether the iMessage channel is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Restrict responses to these senders (phone numbers or Apple IDs).
    /// An empty list means all senders are accepted.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// How often to poll the Messages.app database in seconds (default: 2).
    pub poll_interval_secs: Option<u64>,
}

impl Default for IMessageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_senders: vec![],
            poll_interval_secs: None,
        }
    }
}

/// Configuration for the Matrix channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g. `https://matrix.org`).
    pub homeserver: Option<String>,
    /// Access token for the bot Matrix account.
    pub access_token: Option<String>,
}

/// Configuration for all channels.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,
    #[serde(default)]
    pub imessage: IMessageConfig,
    #[serde(default)]
    pub matrix: MatrixConfig,
}

// ── Tunnel Configuration ────────────────────────────────────────────────────

/// Configuration for the network tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// Whether the tunnel is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The tunnel backend: "cloudflare" or "tailscale".
    #[serde(default = "default_tunnel_backend")]
    pub backend: String,
    /// The local port the gateway listens on.
    #[serde(default = "default_tunnel_port")]
    pub local_port: u16,
    /// Named Cloudflare tunnel name (for persistent custom domains).
    #[serde(default)]
    pub named_tunnel: Option<String>,
    /// Cloudflare tunnel token (for named tunnels).
    #[serde(default)]
    pub token: Option<String>,
    /// Whether to enable Tailscale Funnel for public access.
    #[serde(default)]
    pub tailscale_funnel: bool,
}

fn default_tunnel_backend() -> String {
    "cloudflare".to_string()
}

fn default_tunnel_port() -> u16 {
    8080
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_tunnel_backend(),
            local_port: default_tunnel_port(),
            named_tunnel: None,
            token: None,
            tailscale_funnel: false,
        }
    }
}

// ── Gateway Configuration ─────────────────────────────────────────────────────

/// Configuration for the REST/WebSocket API gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Whether the gateway is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The host to bind to (default: 127.0.0.1 for local-only).
    #[serde(default = "default_gateway_host")]
    pub host: String,
    /// The port to listen on.
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    /// Optional Bearer token for API authentication.
    #[serde(default)]
    pub api_token: Option<String>,
    /// Whether to serve the built-in PWA web client.
    #[serde(default = "default_true")]
    pub serve_pwa: bool,
    /// CORS allowed origins (default: same-origin only).
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

fn default_gateway_host() -> String {
    "127.0.0.1".to_string()
}

fn default_gateway_port() -> u16 {
    8080
}

fn default_true() -> bool {
    true
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_gateway_host(),
            port: default_gateway_port(),
            api_token: None,
            serve_pwa: true,
            cors_origins: vec![],
        }
    }
}

// ── Edge-Native Configuration ─────────────────────────────────────────────────

/// Configuration for the edge-native agent mode (NanoPi Neo3 and similar
/// Linux single-board computers running Oh-Ben-Claw without a central host).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    /// Whether edge-native mode is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of messages retained in the rolling conversation history.
    /// Kept small to reduce RAM pressure on resource-constrained devices.
    #[serde(default = "default_edge_max_history")]
    pub max_history_messages: usize,
    /// Maximum tool-use iterations per user message.
    #[serde(default = "default_edge_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// Whether to start the P2P spine and join the local mesh.
    #[serde(default)]
    pub p2p_enabled: bool,
}

fn default_edge_max_history() -> usize {
    20
}

fn default_edge_max_tool_iterations() -> usize {
    5
}

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_history_messages: default_edge_max_history(),
            max_tool_iterations: default_edge_max_tool_iterations(),
            p2p_enabled: false,
        }
    }
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
    #[serde(default)]
    pub tunnel: TunnelConfig,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub orchestrator: crate::agent::OrchestratorConfig,
    #[serde(default)]
    pub edge: EdgeConfig,
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

    /// Validate the configuration for common misconfigurations.
    ///
    /// Returns a list of human-readable warnings. An empty list means the
    /// configuration is valid. Critical issues are returned as `Err`.
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Validate agent
        if self.agent.max_tool_iterations == 0 {
            anyhow::bail!("agent.max_tool_iterations must be > 0");
        }
        if self.agent.max_tool_iterations > 100 {
            warnings.push(format!(
                "agent.max_tool_iterations is very high ({}); consider a lower limit",
                self.agent.max_tool_iterations
            ));
        }

        // Validate provider
        if self.provider.temperature < 0.0 || self.provider.temperature > 2.0 {
            warnings.push(format!(
                "provider.temperature ({}) is outside the typical range [0.0, 2.0]",
                self.provider.temperature
            ));
        }

        // Validate spine
        if self.spine.port == 0 {
            anyhow::bail!("spine.port must be > 0");
        }
        if self.spine.tls && self.spine.port == 1883 {
            warnings.push(
                "spine.tls is enabled but port is 1883 (unencrypted MQTT default); \
                 consider using port 8883 for MQTT over TLS"
                    .to_string(),
            );
        }
        if self.spine.ca_cert_path.is_some() && !self.spine.tls {
            warnings.push(
                "spine.ca_cert_path is set but spine.tls is false; \
                 the CA certificate will not be used"
                    .to_string(),
            );
        }

        // Validate gateway
        if self.gateway.enabled && self.gateway.api_token.is_none() {
            warnings.push(
                "gateway is enabled without an api_token — the API is unprotected".to_string(),
            );
        }
        if self.gateway.port == 0 {
            anyhow::bail!("gateway.port must be > 0");
        }

        // Validate security
        if self.security.require_pairing && self.security.pairing_secret.is_none() {
            anyhow::bail!(
                "security.require_pairing is true but no pairing_secret is set"
            );
        }
        if let Some(ref secret) = self.security.pairing_secret {
            if let Err(e) = crate::security::pairing::NodePairingManager::validate_secret(secret) {
                warnings.push(format!("security.pairing_secret: {}", e));
            }
        }

        // Validate peripherals
        for (i, board) in self.peripherals.boards.iter().enumerate() {
            if board.transport == "serial" && board.path.is_none() {
                warnings.push(format!(
                    "peripherals.boards[{}] ({}) uses serial transport but no path is set",
                    i, board.board
                ));
            }
            if board.transport == "mqtt" && board.node_id.is_none() {
                warnings.push(format!(
                    "peripherals.boards[{}] ({}) uses mqtt transport but no node_id is set",
                    i, board.board
                ));
            }
        }

        // Validate edge mode
        if self.edge.enabled && self.edge.max_tool_iterations == 0 {
            anyhow::bail!("edge.max_tool_iterations must be > 0");
        }
        if self.edge.enabled && self.spine.kind == "mqtt" && !self.edge.p2p_enabled {
            warnings.push(
                "edge mode is enabled with MQTT spine and p2p_enabled=false; \
                 ensure a reachable MQTT broker is configured or enable p2p_enabled"
                    .to_string(),
            );
        }

        Ok(warnings)
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

    #[test]
    fn default_config_validates_clean() {
        let config = Config::default();
        let warnings = config.validate().unwrap();
        assert!(warnings.is_empty(), "Unexpected warnings: {:?}", warnings);
    }

    #[test]
    fn validate_rejects_zero_tool_iterations() {
        let mut config = Config::default();
        config.agent.max_tool_iterations = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_warns_high_tool_iterations() {
        let mut config = Config::default();
        config.agent.max_tool_iterations = 200;
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("very high")));
    }

    #[test]
    fn validate_warns_tls_on_default_port() {
        let mut config = Config::default();
        config.spine.tls = true;
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("8883")));
    }

    #[test]
    fn validate_warns_gateway_without_token() {
        let mut config = Config::default();
        config.gateway.enabled = true;
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("unprotected")));
    }

    #[test]
    fn validate_rejects_pairing_without_secret() {
        let mut config = Config::default();
        config.security.require_pairing = true;
        config.security.pairing_secret = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_warns_serial_without_path() {
        let mut config = Config::default();
        config.peripherals.boards.push(PeripheralBoardConfig {
            board: "arduino-uno".to_string(),
            transport: "serial".to_string(),
            path: None,
            baud: 115_200,
            node_id: None,
        });
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("no path is set")));
    }

    #[test]
    fn spine_config_tls_cert_fields_serialize() {
        let mut config = Config::default();
        config.spine.tls = true;
        config.spine.ca_cert_path = Some("/etc/mqtt/ca.crt".to_string());
        config.spine.client_cert_path = Some("/etc/mqtt/client.crt".to_string());
        config.spine.client_key_path = Some("/etc/mqtt/client.key".to_string());
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            parsed.spine.ca_cert_path,
            Some("/etc/mqtt/ca.crt".to_string())
        );
        assert_eq!(
            parsed.spine.client_cert_path,
            Some("/etc/mqtt/client.crt".to_string())
        );
        assert_eq!(
            parsed.spine.client_key_path,
            Some("/etc/mqtt/client.key".to_string())
        );
    }

    #[test]
    fn validate_warns_ca_cert_without_tls() {
        let mut config = Config::default();
        config.spine.ca_cert_path = Some("/etc/mqtt/ca.crt".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("ca_cert_path") && w.contains("tls is false")));
    }
}

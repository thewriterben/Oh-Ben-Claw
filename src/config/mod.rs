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
///
/// ## Reliability (inspired by OpenClaw)
///
/// Add `[[provider.fallbacks]]` tables to define an ordered list of backup
/// providers.  If the primary provider fails the next fallback is tried
/// automatically via the `FailoverProvider` wrapper.
///
/// Add a `[provider.retry]` table to enable transparent exponential-back-off
/// retries on transient errors (rate-limits, network blips).
///
/// ```toml
/// [provider]
/// name    = "openai"
/// model   = "gpt-4o"
/// api_key = "sk-..."
///
/// [provider.retry]
/// max_retries      = 3
/// initial_backoff_ms = 500
///
/// [[provider.fallbacks]]
/// name    = "anthropic"
/// model   = "claude-3-5-sonnet-20241022"
/// api_key = "sk-ant-..."
///
/// [[provider.fallbacks]]
/// name  = "ollama"
/// model = "llama3.2"
/// ```
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
    /// Ordered list of fallback provider configurations to try when the
    /// primary provider fails (model failover, inspired by OpenClaw).
    #[serde(default)]
    pub fallbacks: Vec<ProviderConfig>,
    /// Optional retry policy for transient errors (rate-limits, network
    /// issues).  If unset, no automatic retries are performed.
    #[serde(default)]
    pub retry: Option<crate::providers::retry::RetryConfig>,
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
            fallbacks: vec![],
            retry: None,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// Configuration for the Matrix channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g. `https://matrix.org`).
    pub homeserver: Option<String>,
    /// Access token for the bot Matrix account.
    pub access_token: Option<String>,
}

// ── IRC Configuration (new in Phase 10) ──────────────────────────────────────

/// Configuration for the IRC channel adapter.
///
/// The adapter connects to an IRC server, joins the configured channels, and
/// forwards PRIVMSG messages to the Oh-Ben-Claw agent.
///
/// ```toml
/// [channels.irc]
/// host     = "irc.libera.chat"
/// port     = 6697
/// use_tls  = true
/// nickname = "oh-ben-claw"
/// channels = ["#ai-bots", "#myserver"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IrcConfig {
    /// IRC server hostname.
    #[serde(default)]
    pub host: Option<String>,
    /// IRC server port (default: 6697 for TLS, 6667 for plain).
    #[serde(default)]
    pub port: Option<u16>,
    /// Whether to use TLS (default: true).
    #[serde(default = "default_true")]
    pub use_tls: bool,
    /// Bot nickname.
    #[serde(default = "default_irc_nick")]
    pub nickname: String,
    /// Optional NickServ password for automatic identification.
    #[serde(default)]
    pub password: Option<String>,
    /// IRC channels to join (e.g. `["#general", "#bots"]`).
    #[serde(default)]
    pub channels: Vec<String>,
    /// SASL PLAIN username (usually the account name, same as nickname).
    #[serde(default)]
    pub sasl_username: Option<String>,
    /// SASL PLAIN password.
    #[serde(default)]
    pub sasl_password: Option<String>,
}

fn default_irc_nick() -> String {
    "oh-ben-claw".to_string()
}

// ── Signal Configuration (new in Phase 10) ────────────────────────────────────

/// Configuration for the Signal channel adapter.
///
/// Uses the [signal-cli](https://github.com/AsamK/signal-cli) JSON-RPC HTTP
/// daemon.  Start signal-cli in daemon mode:
/// ```shell
/// signal-cli -a +1234567890 daemon --http localhost:8080
/// ```
///
/// ```toml
/// [channels.signal]
/// cli_url        = "http://localhost:8080"
/// phone_number   = "+1234567890"
/// allowed_numbers = ["+10987654321"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalConfig {
    /// Base URL of the signal-cli JSON-RPC HTTP daemon.
    #[serde(default)]
    pub cli_url: Option<String>,
    /// The registered phone number of the bot account (E.164 format).
    #[serde(default)]
    pub phone_number: Option<String>,
    /// Optional allowlist of phone numbers that may talk to the bot.
    /// When empty, all senders are accepted.
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
    /// Polling interval in seconds (default: 2).
    #[serde(default = "default_signal_poll_secs")]
    pub poll_interval_secs: u64,
}

fn default_signal_poll_secs() -> u64 {
    2
}

// ── Mattermost Configuration (new in Phase 10) ────────────────────────────────

/// Configuration for the Mattermost channel adapter.
///
/// The adapter uses the Mattermost WebSocket event API to receive messages and
/// the REST API to post replies.
///
/// ```toml
/// [channels.mattermost]
/// server_url = "https://mattermost.example.com"
/// token      = "your-personal-access-token"
/// team_name  = "my-team"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MattermostConfig {
    /// Mattermost server URL (e.g. `https://mattermost.example.com`).
    #[serde(default)]
    pub server_url: Option<String>,
    /// Personal access token or bot token.
    #[serde(default)]
    pub token: Option<String>,
    /// The bot's Mattermost user ID (auto-detected if not set).
    #[serde(default)]
    pub bot_user_id: Option<String>,
    /// Team name the bot operates in (used for display only).
    #[serde(default)]
    pub team_name: Option<String>,
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
    /// IRC channel adapter (new in Phase 10).
    #[serde(default)]
    pub irc: IrcConfig,
    /// Signal channel adapter via signal-cli (new in Phase 10).
    #[serde(default)]
    pub signal: SignalConfig,
    /// Mattermost channel adapter (new in Phase 10).
    #[serde(default)]
    pub mattermost: MattermostConfig,
    /// Feishu/Lark channel adapter (new in Phase 11).
    #[serde(default)]
    pub feishu: FeishuConfig,
    /// Send "typing…" indicators while the agent processes a message.
    /// Supported by Telegram, Discord, and Slack (default: true).
    #[serde(default = "default_true")]
    pub typing_indicators: bool,
}

// ── Feishu Configuration (new in Phase 11) ───────────────────────────────────

/// Configuration for the Feishu/Lark channel adapter.
///
/// Feishu (Lark outside China) is a popular enterprise messaging platform.
/// The adapter receives messages via webhook event subscription and sends
/// replies through the Feishu REST API.
///
/// Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s Feishu
/// integration.
///
/// ```toml
/// [channels.feishu]
/// app_id             = "cli_xxxxxxxxxxxxxx"
/// app_secret         = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
/// verification_token = "your-verification-token"
/// webhook_port       = 18790
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    /// Feishu App ID (e.g. `cli_xxxxxxxxxxxxxx`).
    #[serde(default)]
    pub app_id: Option<String>,
    /// Feishu App Secret.
    #[serde(default)]
    pub app_secret: Option<String>,
    /// Verification token shown in the Event Subscription settings of the app.
    /// When set, every incoming webhook payload's `token` field must match.
    #[serde(default)]
    pub verification_token: Option<String>,
    /// Local port for the webhook HTTP server (default: 18790).
    #[serde(default)]
    pub webhook_port: Option<u16>,
}

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

// ── Autonomy Configuration ────────────────────────────────────────────────────

/// Autonomy level for tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Full autonomy: all tools execute without approval.
    #[default]
    Full,
    /// Supervised: most tools require approval unless auto_approve'd.
    Supervised,
    /// Manual: all tools require explicit approval.
    Manual,
}

/// Configuration for the human-in-the-loop approval system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyConfig {
    /// The autonomy level.
    #[serde(default)]
    pub level: AutonomyLevel,
    /// Tools that never need approval regardless of level.
    #[serde(default)]
    pub auto_approve: Vec<String>,
    /// Tools that always need approval regardless of level or session allowlist.
    #[serde(default)]
    pub always_ask: Vec<String>,
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec![],
        }
    }
}

// ── Cost Configuration ────────────────────────────────────────────────────────

/// Configuration for token cost tracking and budget enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    /// Whether cost tracking is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Daily spending limit in USD (0 = no limit).
    #[serde(default = "default_daily_limit")]
    pub daily_limit_usd: f64,
    /// Monthly spending limit in USD (0 = no limit).
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit_usd: f64,
    /// Warning threshold as a fraction of the limit (e.g. 0.8 = warn at 80%).
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold: f64,
}

fn default_daily_limit() -> f64 {
    10.0
}
fn default_monthly_limit() -> f64 {
    100.0
}
fn default_warn_threshold() -> f64 {
    0.8
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit_usd: default_daily_limit(),
            monthly_limit_usd: default_monthly_limit(),
            warn_threshold: default_warn_threshold(),
        }
    }
}

// ── Docker Configuration ──────────────────────────────────────────────────────

/// Configuration for the Docker runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Docker image to use for sandboxed execution.
    #[serde(default = "default_docker_image")]
    pub image: String,
    /// Docker network to use (default: "none" for isolation).
    #[serde(default = "default_docker_network")]
    pub network: String,
    /// Memory limit in MB for Docker containers.
    #[serde(default = "default_docker_memory_mb")]
    pub memory_mb: u64,
}

fn default_docker_image() -> String {
    "alpine:latest".to_string()
}
fn default_docker_network() -> String {
    "none".to_string()
}
fn default_docker_memory_mb() -> u64 {
    128
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: default_docker_image(),
            network: default_docker_network(),
            memory_mb: default_docker_memory_mb(),
        }
    }
}

// ── Runtime Configuration ──────────────────────────────────────────────────────

/// Configuration for the tool execution runtime (sandbox).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Runtime kind: "native" (default), "docker", or "wasm".
    #[serde(default = "default_runtime_kind")]
    pub kind: String,
    /// Docker runtime configuration (used when kind = "docker").
    #[serde(default)]
    pub docker: DockerConfig,
    /// WASM runtime configuration (used when kind = "wasm").
    #[serde(default)]
    pub wasm: WasmConfig,
}

fn default_runtime_kind() -> String {
    "native".to_string()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            kind: default_runtime_kind(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        }
    }
}

// ── WASM Configuration ───────────────────────────────────────────────────────

/// Configuration for the WebAssembly sandbox runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    /// Whether the WASM runtime is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of WASM linear-memory pages (1 page = 64 KiB).
    /// Default: 256 (= 16 MiB).
    #[serde(default = "default_wasm_max_memory_pages")]
    pub max_memory_pages: u32,
    /// Execution fuel limit — controls how many instructions the guest may run.
    /// Default: 1_000_000.
    #[serde(default = "default_wasm_max_fuel")]
    pub max_fuel: u64,
    /// Host directories the WASI layer may expose to the guest module.
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
}

fn default_wasm_max_memory_pages() -> u32 {
    256
}
fn default_wasm_max_fuel() -> u64 {
    1_000_000
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_memory_pages: default_wasm_max_memory_pages(),
            max_fuel: default_wasm_max_fuel(),
            allowed_dirs: Vec::new(),
        }
    }
}

// ── Multimodal Configuration ───────────────────────────────────────────────────

/// Configuration for multimodal (image) handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalConfig {
    /// Whether multimodal image handling is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum number of images per message.
    #[serde(default = "default_max_images")]
    pub max_images: usize,
    /// Maximum image size in bytes.
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: usize,
    /// Whether to allow fetching remote (URL) images.
    #[serde(default)]
    pub allow_remote: bool,
}

fn default_max_images() -> usize {
    5
}
fn default_max_image_bytes() -> usize {
    5 * 1024 * 1024
}

impl Default for MultimodalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_images: default_max_images(),
            max_image_bytes: default_max_image_bytes(),
            allow_remote: false,
        }
    }
}

// ── Proxy Configuration (new in Phase 11) ────────────────────────────────────

/// Configuration for outbound HTTP proxy support.
///
/// Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s proxy system,
/// which adds HTTP CONNECT tunnel support for networks behind corporate
/// firewalls or restricted internet environments.
///
/// When configured, the proxy settings are applied to all outbound HTTP
/// requests made by Oh-Ben-Claw (LLM API calls, channel webhooks, etc.)
/// via the `HTTPS_PROXY` / `HTTP_PROXY` environment variables.
///
/// ```toml
/// [proxy]
/// host     = "10.0.0.1"
/// port     = 7897
/// kind     = "http"      # "http" (default) or "socks5"
/// username = "user"      # optional
/// password = "pass"      # optional
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    /// Whether the proxy is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Proxy server hostname or IP address.
    #[serde(default)]
    pub host: Option<String>,
    /// Proxy server port.
    #[serde(default)]
    pub port: Option<u16>,
    /// Proxy protocol: `"http"` (default) or `"socks5"`.
    #[serde(default = "default_proxy_kind")]
    pub kind: String,
    /// Optional proxy username for authenticated proxies.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional proxy password for authenticated proxies.
    #[serde(default)]
    pub password: Option<String>,
}

fn default_proxy_kind() -> String {
    "http".to_string()
}

impl ProxyConfig {
    /// Build the proxy URL string (e.g. `http://user:pass@10.0.0.1:7897`).
    ///
    /// Returns `None` if `enabled` is false or host/port are not set.
    pub fn url(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let host = self.host.as_deref()?;
        let port = self.port?;
        let creds = match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("{u}:{p}@"),
            (Some(u), None) => format!("{u}@"),
            _ => String::new(),
        };
        Some(format!("{}://{}{}:{}", self.kind, creds, host, port))
    }

    /// Apply this proxy configuration to the current process environment.
    ///
    /// Sets `HTTP_PROXY` and `HTTPS_PROXY` environment variables so that all
    /// HTTP clients that respect them (including `reqwest`) pick them up.
    pub fn apply_to_env(&self) {
        if let Some(url) = self.url() {
            std::env::set_var("HTTP_PROXY", &url);
            std::env::set_var("HTTPS_PROXY", &url);
            tracing::info!(proxy = %url, "Outbound HTTP proxy configured");
        }
    }
}

// ── Personality Configuration (new in Phase 11) ───────────────────────────────

/// Configuration for the personality file system.
///
/// Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s approach of
/// storing the agent's personality in editable Markdown files (`SOUL.md` for
/// the agent's personality and `USER.md` for the user profile).
///
/// By default both files live in `~/.oh-ben-claw/` (next to `memory.db`).
/// Set custom paths here if you want to keep them elsewhere (e.g. in a shared
/// config management repository).
///
/// ```toml
/// [personality]
/// soul_path = "/home/alice/.oh-ben-claw/SOUL.md"   # optional override
/// user_path = "/home/alice/.oh-ben-claw/USER.md"   # optional override
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonalityConfig {
    /// Custom path to the SOUL.md agent personality file.
    ///
    /// When unset the default data-dir path (`~/.oh-ben-claw/SOUL.md`) is used.
    #[serde(default)]
    pub soul_path: Option<String>,
    /// Custom path to the USER.md user profile file.
    ///
    /// When unset the default data-dir path (`~/.oh-ben-claw/USER.md`) is used.
    #[serde(default)]
    pub user_path: Option<String>,
}

// ── Phase 12 config ───────────────────────────────────────────────────────────

/// Configuration for the browser automation subsystem (Phase 12).
///
/// ```toml
/// [browser]
/// enabled = true
/// cdp_url = "http://localhost:9222"
/// profile = "headless"
/// timeout_secs = 30
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Enable browser automation tools (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Chrome DevTools Protocol base URL.
    ///
    /// Launch Chrome/Chromium with `--remote-debugging-port=9222` then point
    /// this at the resulting endpoint.  When unset the default local port is
    /// used (`http://localhost:9222`).
    #[serde(default)]
    pub cdp_url: Option<String>,
    /// Browser profile — `"headless"` (default) or `"user"` (attach to the
    /// signed-in desktop browser for auth-aware tasks).
    #[serde(default = "default_headless_profile")]
    pub profile: String,
    /// Seconds before a navigation or selector operation times out (default: 30).
    #[serde(default = "default_browser_timeout")]
    pub timeout_secs: u64,
}

fn default_headless_profile() -> String {
    "headless".to_string()
}

fn default_browser_timeout() -> u64 {
    30
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cdp_url: None,
            profile: default_headless_profile(),
            timeout_secs: default_browser_timeout(),
        }
    }
}

/// Configuration for the ClawHub community skill registry (Phase 12).
///
/// ```toml
/// [clawhub]
/// enabled = true
/// registry_url = "https://hub.openclaw.ai"
/// auto_update = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubConfig {
    /// Enable the ClawHub skill registry (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Base URL of the ClawHub registry API.
    #[serde(default = "default_clawhub_url")]
    pub registry_url: String,
    /// Automatically check for skill updates on startup (default: false).
    #[serde(default)]
    pub auto_update: bool,
    /// Local directory where installed skills are stored.
    ///
    /// When unset, defaults to `~/.oh-ben-claw/skills/`.
    #[serde(default)]
    pub skills_dir: Option<String>,
}

fn default_clawhub_url() -> String {
    "https://hub.openclaw.ai".to_string()
}

impl Default for ClawHubConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            registry_url: default_clawhub_url(),
            auto_update: false,
            skills_dir: None,
        }
    }
}

// ── Deployment Configuration (new in Phase 13) ────────────────────────────────

/// Configuration for a single hardware item in a deployment scenario.
///
/// Used inside `DeploymentConfig.hardware` to describe every board or
/// accessory that is part of the deployment.
///
/// ```toml
/// [[deployment.hardware]]
/// name       = "nanopi-neo3"
/// board_name = "nanopi-neo3"
/// transport  = "native"
/// role       = "host"
/// accessories = ["dht22"]
///
/// [[deployment.hardware]]
/// name       = "xiao-esp32s3-sense"
/// board_name = "xiao-esp32s3-sense"
/// transport  = "serial"
/// role       = "vision"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentHardwareConfig {
    /// Human-readable label for this item.
    pub name: String,
    /// Board registry name (e.g. `"nanopi-neo3"`, `"xiao-esp32s3-sense"`).
    pub board_name: String,
    /// Transport type: `"native"`, `"serial"`, `"mqtt"`.
    pub transport: String,
    /// Serial device path (for serial transport).
    #[serde(default)]
    pub path: Option<String>,
    /// MQTT node ID (for mqtt transport).
    #[serde(default)]
    pub node_id: Option<String>,
    /// Operator-assigned role: `"host"`, `"display"`, `"vision"`, `"listening"`,
    /// `"sensing"`, `"peripheral"`.  Leave empty for auto-assignment.
    #[serde(default)]
    pub role: String,
    /// Accessory names attached to this board (e.g. `["dht22"]`).
    #[serde(default)]
    pub accessories: Vec<String>,
}

/// Configuration for the deployment scheme generator (Phase 13).
///
/// Describes the hardware inventory and feature desires for a deployment.
/// When `auto_plan` is true, Oh-Ben-Claw generates a deployment scheme at
/// startup and optionally pre-spawns the required sub-agents.
///
/// ```toml
/// [deployment]
/// enabled      = true
/// scenario     = "NanoPi Home Assistant"
/// auto_plan    = true
/// auto_spawn   = true
///
/// feature_desires = [
///     "vision", "listening", "speech", "environmental_sensing",
///     "display_output", "touch_input", "wireless_mesh",
/// ]
///
/// [[deployment.hardware]]
/// name = "nanopi-neo3"; board_name = "nanopi-neo3"; transport = "native"
/// role = "host"; accessories = ["dht22"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentConfig {
    /// Whether the deployment subsystem is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Human-readable name for this deployment scenario.
    #[serde(default = "default_scenario_name")]
    pub scenario: String,
    /// When true, generate and print the deployment scheme at startup.
    #[serde(default)]
    pub auto_plan: bool,
    /// When true (and `auto_plan` is true), pre-spawn the sub-agents in the
    /// orchestrator pool after planning.
    #[serde(default)]
    pub auto_spawn: bool,
    /// The hardware items in the deployment.
    #[serde(default)]
    pub hardware: Vec<DeploymentHardwareConfig>,
    /// High-level features the operator wants (see `FeatureDesire` variants).
    ///
    /// Recognised values: `"vision"`, `"listening"`, `"speech"`,
    /// `"environmental_sensing"`, `"display_output"`, `"touch_input"`,
    /// `"edge_inference"`, `"wireless_mesh"`, `"persistent_memory"`.
    #[serde(default)]
    pub feature_desires: Vec<String>,
    /// Whether to enable LLM-powered swarm refinement of the deployment scheme.
    /// When false (default), only the rule-based planner is used.
    #[serde(default)]
    pub llm_swarm: bool,
}

fn default_scenario_name() -> String {
    "Oh-Ben-Claw Deployment".to_string()
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scenario: default_scenario_name(),
            auto_plan: false,
            auto_spawn: false,
            hardware: Vec::new(),
            feature_desires: Vec::new(),
            llm_swarm: false,
        }
    }
}

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
    #[serde(default)]
    pub autonomy: AutonomyConfig,
    #[serde(default)]
    pub cost: CostConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub multimodal: MultimodalConfig,
    /// HTTP proxy for outbound requests (new in Phase 11).
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Personality file configuration — SOUL.md and USER.md (new in Phase 11).
    #[serde(default)]
    pub personality: PersonalityConfig,
    /// Browser automation configuration (new in Phase 12).
    #[serde(default)]
    pub browser: BrowserConfig,
    /// ClawHub community skill registry configuration (new in Phase 12).
    #[serde(default)]
    pub clawhub: ClawHubConfig,
    /// Deployment scheme generator configuration (new in Phase 13).
    #[serde(default)]
    pub deployment: DeploymentConfig,
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
            anyhow::bail!("security.require_pairing is true but no pairing_secret is set");
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

        // Validate proxy
        if self.proxy.enabled {
            if self.proxy.host.is_none() {
                anyhow::bail!("proxy is enabled but proxy.host is not set");
            }
            if self.proxy.port.is_none() {
                anyhow::bail!("proxy is enabled but proxy.port is not set");
            }
            if !["http", "socks5"].contains(&self.proxy.kind.as_str()) {
                warnings.push(format!(
                    "proxy.kind '{}' is not recognised; supported values are 'http' and 'socks5'",
                    self.proxy.kind
                ));
            }
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

    // ── Phase 11 tests ─────────────────────────────────────────────────────────

    #[test]
    fn proxy_config_url_disabled_returns_none() {
        let proxy = ProxyConfig::default();
        assert!(proxy.url().is_none());
    }

    #[test]
    fn proxy_config_url_without_creds() {
        let proxy = ProxyConfig {
            enabled: true,
            host: Some("10.0.0.1".to_string()),
            port: Some(7897),
            kind: "http".to_string(),
            username: None,
            password: None,
        };
        assert_eq!(proxy.url(), Some("http://10.0.0.1:7897".to_string()));
    }

    #[test]
    fn proxy_config_url_with_creds() {
        let proxy = ProxyConfig {
            enabled: true,
            host: Some("proxy.corp.com".to_string()),
            port: Some(8080),
            kind: "socks5".to_string(),
            username: Some("alice".to_string()),
            password: Some("s3cr3t".to_string()),
        };
        assert_eq!(
            proxy.url(),
            Some("socks5://alice:s3cr3t@proxy.corp.com:8080".to_string())
        );
    }

    #[test]
    fn validate_rejects_proxy_enabled_without_host() {
        let mut config = Config::default();
        config.proxy.enabled = true;
        config.proxy.port = Some(8080);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_proxy_enabled_without_port() {
        let mut config = Config::default();
        config.proxy.enabled = true;
        config.proxy.host = Some("10.0.0.1".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_proxy_warns_unknown_kind() {
        let mut config = Config::default();
        config.proxy.enabled = true;
        config.proxy.host = Some("10.0.0.1".to_string());
        config.proxy.port = Some(8080);
        config.proxy.kind = "ftp".to_string();
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("'ftp'")));
    }

    #[test]
    fn feishu_config_default_is_empty() {
        let config = FeishuConfig::default();
        assert!(config.app_id.is_none());
        assert!(config.app_secret.is_none());
        assert!(config.verification_token.is_none());
        assert!(config.webhook_port.is_none());
    }

    #[test]
    fn personality_config_default_is_empty() {
        let config = PersonalityConfig::default();
        assert!(config.soul_path.is_none());
        assert!(config.user_path.is_none());
    }

    #[test]
    fn root_config_has_proxy_and_personality_fields() {
        let config = Config::default();
        assert!(!config.proxy.enabled);
        assert!(config.personality.soul_path.is_none());
    }

    // ── Phase 12 tests ─────────────────────────────────────────────────────────

    #[test]
    fn browser_config_default_values() {
        let cfg = BrowserConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.cdp_url.is_none());
        assert_eq!(cfg.profile, "headless");
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn clawhub_config_default_values() {
        let cfg = ClawHubConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.registry_url, "https://hub.openclaw.ai");
        assert!(!cfg.auto_update);
        assert!(cfg.skills_dir.is_none());
    }

    #[test]
    fn root_config_has_browser_and_clawhub_fields() {
        let config = Config::default();
        assert!(config.browser.enabled);
        assert!(config.clawhub.enabled);
        assert_eq!(config.clawhub.registry_url, "https://hub.openclaw.ai");
    }

    #[test]
    fn browser_config_deserializes_from_toml() {
        let toml = r#"
            [browser]
            enabled = false
            cdp_url = "http://192.168.1.5:9222"
            profile = "user"
            timeout_secs = 60
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(!config.browser.enabled);
        assert_eq!(
            config.browser.cdp_url.as_deref(),
            Some("http://192.168.1.5:9222")
        );
        assert_eq!(config.browser.profile, "user");
        assert_eq!(config.browser.timeout_secs, 60);
    }

    #[test]
    fn clawhub_config_deserializes_from_toml() {
        let toml = r#"
            [clawhub]
            enabled = true
            registry_url = "https://my-hub.example.com"
            auto_update = true
            skills_dir = "/opt/skills"
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.clawhub.enabled);
        assert_eq!(config.clawhub.registry_url, "https://my-hub.example.com");
        assert!(config.clawhub.auto_update);
        assert_eq!(config.clawhub.skills_dir.as_deref(), Some("/opt/skills"));
    }
}

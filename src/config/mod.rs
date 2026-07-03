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
    /// Optional response format (structured output / JSON mode).
    #[serde(default)]
    pub response_format: Option<crate::providers::ResponseFormat>,
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
            response_format: None,
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
    /// Input price in USD per million tokens for the configured model.
    /// Default 0.0 — token counts are tracked either way; dollar figures
    /// appear once the operator supplies their model's prices.
    #[serde(default)]
    pub input_price_per_million: f64,
    /// Output price in USD per million tokens. Default 0.0.
    #[serde(default)]
    pub output_price_per_million: f64,
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
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
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
    /// Install-security policy (Phase 15, WS1): operator approval,
    /// checksum verification, version pinning, allowlist, audit log.
    ///
    /// ```toml
    /// [clawhub.install_policy]
    /// require_approval = true
    /// require_checksum = false
    /// allowlist = []
    ///
    /// [clawhub.install_policy.pinned_versions]
    /// weather = "1.2.0"
    /// ```
    #[serde(default)]
    pub install_policy: crate::skill_forge::install_policy::InstallPolicyConfig,
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
            install_policy: Default::default(),
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

/// Agent-to-Agent (A2A) protocol configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AConfig {
    /// Whether A2A protocol support is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The name this agent advertises in its A2A agent card.
    #[serde(default = "default_a2a_agent_name")]
    pub agent_name: String,
    /// A human-readable description of this agent.
    #[serde(default = "default_a2a_agent_description")]
    pub agent_description: String,
    /// The URL where this agent's A2A endpoint is reachable.
    #[serde(default = "default_a2a_agent_url")]
    pub agent_url: String,
    /// List of skill names this agent exposes via A2A.
    #[serde(default)]
    pub skills: Vec<String>,
}

fn default_a2a_agent_name() -> String {
    "oh-ben-claw".to_string()
}

fn default_a2a_agent_description() -> String {
    "Oh-Ben-Claw AI assistant".to_string()
}

fn default_a2a_agent_url() -> String {
    "http://localhost:8080".to_string()
}

impl Default for A2AConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            agent_name: default_a2a_agent_name(),
            agent_description: default_a2a_agent_description(),
            agent_url: default_a2a_agent_url(),
            skills: Vec::new(),
        }
    }
}

/// Phase 18 perception configuration (`[perception]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerceptionConfig {
    /// Enable the world-memory tool (a temporal model of real-world state).
    #[serde(default)]
    pub world_memory: bool,
    /// Path to the world-memory database. Defaults to the data dir's `world.db`.
    #[serde(default)]
    pub world_db_path: Option<String>,
    /// Optional poll of a ClawCam (vision subsystem) MCP server whose detections
    /// are folded into world memory on a cadence (Phase 18 / S1b).
    #[serde(default)]
    pub clawcam_poll: Option<ClawCamPollConfig>,
    /// Vision-driven reflex + foresight rules keyed on ClawCam detections.
    #[serde(default)]
    pub vision_rules: VisionRulesConfig,
}

/// Vision-driven reflex + foresight rules (`[perception.vision_rules]`). Detections
/// folded into world memory become triggers: a confirmed sighting of an alert
/// subject escalates (reflex), and a rising sighting *rate* escalates ahead of time
/// (foresight). Merged into the live reflex/foresight engines, bounded by Track 0.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VisionRulesConfig {
    /// Enable vision-driven rules.
    #[serde(default)]
    pub enabled: bool,
    /// Subjects that warrant an alert (entity `vision.subject.{subject}`).
    #[serde(default)]
    pub alert_subjects: Vec<String>,
    /// Review state a sighting must carry to count as confirmed. Default `verified`.
    #[serde(default = "default_vision_require_state")]
    pub require_state: String,
    /// Minimum ms between re-fires of a given rule.
    #[serde(default = "default_vision_debounce_ms")]
    pub debounce_ms: u64,
    /// Optional camera node to `capture_now` from on alert (needs the ClawCam
    /// actuation sink wired; otherwise the capture publish is a no-op).
    #[serde(default)]
    pub capture_node: Option<String>,
    /// Foresight: escalate when a subject's sighting count is predicted within
    /// `horizon_ms` to reach this many more sightings.
    #[serde(default = "default_vision_rate_threshold")]
    pub rate_threshold: f64,
    /// Foresight look-ahead window (ms).
    #[serde(default = "default_vision_horizon_ms")]
    pub horizon_ms: u64,
}

fn default_vision_require_state() -> String {
    "verified".to_string()
}
fn default_vision_debounce_ms() -> u64 {
    10_000
}
fn default_vision_rate_threshold() -> f64 {
    5.0
}
fn default_vision_horizon_ms() -> u64 {
    60_000
}

/// Poll a ClawCam detection MCP tool into world memory (`[perception.clawcam_poll]`).
/// Requires `[perception] world_memory = true`. Detections become
/// `vision.subject.{species}` facts carrying review state and valid-time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawCamPollConfig {
    /// Enable the poll loop.
    #[serde(default)]
    pub enabled: bool,
    /// How to reach the ClawCam MCP bridge (stdio command or http url).
    pub server: crate::mcp::McpServerConfig,
    /// Detection tool to poll.
    #[serde(default = "default_clawcam_tool")]
    pub tool: String,
    /// Arguments passed to the tool (e.g. `{ "min_confidence": 0.5 }`).
    #[serde(default = "default_clawcam_args")]
    pub args: serde_json::Value,
    /// Poll cadence in milliseconds.
    #[serde(default = "default_clawcam_interval_ms")]
    pub interval_ms: u64,
    /// World-memory `source` label for the ingested facts.
    #[serde(default = "default_clawcam_source")]
    pub source: String,
    /// Also poll `get_node_health` each tick → `clawcam.node.{id}` facts (a
    /// camera's reachability/battery, kept separate from the robot's own suites).
    #[serde(default)]
    pub poll_health: bool,
    /// Also poll `list_audio_classifications` each tick → audio-suite events
    /// (`audio.clawcam:{node}`), so a glassbreak is classifiable by safing.
    #[serde(default)]
    pub poll_audio: bool,
}

fn default_clawcam_tool() -> String {
    "list_species_detections".to_string()
}
fn default_clawcam_args() -> serde_json::Value {
    serde_json::json!({})
}
fn default_clawcam_interval_ms() -> u64 {
    5000
}
fn default_clawcam_source() -> String {
    "clawcam".to_string()
}

/// Movement subsystem configuration (`[movement]`). Exposes the safety-bounded
/// `move_actuator` tool to the agent. Requires `[safety] enabled = true` —
/// movement is physical and MUST be deterministically bounded (Suite §7).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MovementConfig {
    /// Enable the `move_actuator` tool.
    #[serde(default)]
    pub enabled: bool,
    /// Node id the movement safety limits apply to (matches `[[safety.limit]]`
    /// `node_id`); used for the `servo_angle`/`motor_speed`/`stop` limits.
    #[serde(default = "default_movement_node_id")]
    pub node_id: String,
}

fn default_movement_node_id() -> String {
    "movement".to_string()
}

/// Sensing subsystem configuration (`[sensing]`). Exposes the quality-aware
/// `sense` tool and (optionally) records ingested readings into world memory as
/// `sensor.{quantity}` facts. Sensing is non-actuating, so unlike movement it
/// does not require `[safety]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SensingConfig {
    /// Enable the `sense` tool and sensing controller.
    #[serde(default)]
    pub enabled: bool,
    /// World-memory `source` label for ingested readings. Default `"sensing"`.
    #[serde(default)]
    pub source: Option<String>,
    /// Per-quantity expectations driving quality classification
    /// (`[[sensing.quantity]]` array-of-tables).
    #[serde(default, rename = "quantity")]
    pub quantities: Vec<SensingQuantityConfig>,
}

/// Expected bounds + freshness for one sensor stream (`[[sensing.quantity]]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SensingQuantityConfig {
    /// Stream name (e.g. `"temperature"`). Becomes `sensor.{name}`.
    pub name: String,
    /// Inclusive minimum acceptable value; readings below are `out_of_range`.
    #[serde(default)]
    pub min: Option<f64>,
    /// Inclusive maximum acceptable value; readings above are `out_of_range`.
    #[serde(default)]
    pub max: Option<f64>,
    /// Max ms between readings before the stream is considered `stale`.
    #[serde(default)]
    pub max_staleness_ms: Option<u64>,
    /// Canonical unit; used when a reading omits its own.
    #[serde(default)]
    pub unit: Option<String>,
}

/// Audio suite configuration (`[audio_suite]`). Exposes the `hear` (perceive)
/// and `speak` (act) tools. Heard events and spoken utterances are recorded into
/// world memory; speech is emitted through the configured sink (dry-run logging
/// until a real engine is wired). Requires `[perception].world_memory = true`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioSuiteConfig {
    /// Enable the `hear` + `speak` tools and the audio controller.
    #[serde(default)]
    pub enabled: bool,
    /// Confidence floor below which heard events are flagged unreliable. Default 0.5.
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// Default voice for `speak` when the call omits one. Default `"nova"`.
    #[serde(default)]
    pub voice: Option<String>,
    /// World-memory `source` label for audio facts. Default `"audio"`.
    #[serde(default)]
    pub source: Option<String>,
    /// Render speech locally via the OpenAI TTS tool instead of publishing over
    /// the spine. Best-effort (no key ⇒ logged + skipped).
    #[serde(default)]
    pub render_tts: bool,
    /// Output directory for locally rendered TTS audio. Default `/tmp`.
    #[serde(default)]
    pub tts_out_dir: Option<String>,
}

/// Power suite configuration (`[power]`). Exposes the `power` tool and records
/// battery telemetry + a derived power mode into world memory (`power.battery`,
/// `power.mode`). Reflexes can watch `power.mode` for low-power safing. Requires
/// `[perception].world_memory = true`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PowerConfig {
    /// Enable the `power` tool and controller.
    #[serde(default)]
    pub enabled: bool,
    /// SoC percent at/below which (and not charging) the mode is `low`. Default 20.
    #[serde(default)]
    pub low_pct: Option<f64>,
    /// SoC percent at/below which (and not charging) the mode is `critical`. Default 10.
    #[serde(default)]
    pub critical_pct: Option<f64>,
    /// World-memory `source` label for power facts. Default `"power"`.
    #[serde(default)]
    pub source: Option<String>,
}

/// Comms suite configuration (`[comms]`). Exposes the `comms` tool and records
/// per-link state (`link.{name}`) + an aggregate `net.mode` into world memory.
/// Reflexes can watch `net.mode` for offline / degraded-mode safing. Requires
/// `[perception].world_memory = true`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommsConfig {
    /// Enable the `comms` tool and controller.
    #[serde(default)]
    pub enabled: bool,
    /// Below this RSSI (dBm) a link is `degraded`. Default -80.
    #[serde(default)]
    pub min_rssi_dbm: Option<f64>,
    /// Above this latency (ms) a link is `degraded`. Default 500.
    #[serde(default)]
    pub max_latency_ms: Option<f64>,
    /// Above this loss (%) a link is `degraded`. Default 5.
    #[serde(default)]
    pub max_loss_pct: Option<f64>,
    /// World-memory `source` label for comms facts. Default `"comms"`.
    #[serde(default)]
    pub source: Option<String>,
}

/// Navigation suite configuration (`[navigation]`). The fusing suite: localizes
/// from sensor pose facts and drives toward a goal through the movement
/// controller. Requires `[perception].world_memory = true` AND `[movement]`
/// (+ `[safety]`) — it reuses the movement controller and its Track 0 gate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NavigationConfig {
    /// Enable the `navigate` + `nav_status` tools and the stepping loop.
    #[serde(default)]
    pub enabled: bool,
    /// Steering servo actuator (default `steer` on channel 0).
    #[serde(default)]
    pub steer: Option<NavActuatorConfig>,
    /// Drive motor actuator (default `drive` on channel 1).
    #[serde(default)]
    pub drive: Option<NavActuatorConfig>,
    /// Cruise drive speed in -1..1 (default 0.5).
    #[serde(default)]
    pub forward_speed: Option<f64>,
    /// Max steering angle magnitude in degrees (default 45).
    #[serde(default)]
    pub max_steer_deg: Option<f64>,
    /// Heading-error → steering proportional gain (default 1.0).
    #[serde(default)]
    pub heading_kp: Option<f64>,
    /// Heading error (deg) within which to drive at full speed (default 15).
    #[serde(default)]
    pub align_threshold_deg: Option<f64>,
    /// Stepping cadence in ms (default 500).
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// World-memory `source` label for nav facts. Default `"navigation"`.
    #[serde(default)]
    pub source: Option<String>,
    /// Pose-fusion sources (`[[navigation.pose_source]]`). When non-empty, a
    /// fuser loop fuses them into the canonical pose entities the localizer reads.
    #[serde(default, rename = "pose_source")]
    pub pose_sources: Vec<PoseSourceConfig>,
    /// Occupancy grid for obstacle-aware planning (`[navigation.grid]`). When set,
    /// `navigate` plans paths around obstacles and `nav_map` builds the map.
    #[serde(default)]
    pub grid: Option<NavGridConfig>,
    /// Range-sensor max distance for `nav_map scan` mapping (default 10).
    #[serde(default)]
    pub sensor_max_range: Option<f64>,
    /// Autonomous exploration: when idle, drive to the nearest frontier and map
    /// it, until the reachable space is explored. Requires a grid.
    #[serde(default)]
    pub explore: bool,
    /// Robot inscribed radius (world units) — cells this close to an obstacle are
    /// lethal. Setting this (with `inflation_radius`) enables clearance-aware
    /// planning that keeps a safety margin instead of hugging obstacles.
    #[serde(default)]
    pub inscribed_radius: Option<f64>,
    /// Inflation radius (world units) out to which obstacle proximity is penalized.
    #[serde(default)]
    pub inflation_radius: Option<f64>,
    /// Inflation cost decay rate (default 2.0).
    #[serde(default)]
    pub inflation_decay: Option<f64>,
}

/// Occupancy-grid bounds for obstacle-aware planning (`[navigation.grid]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NavGridConfig {
    /// World coordinate of the grid's lower-left corner (x).
    #[serde(default)]
    pub origin_x: f64,
    /// World coordinate of the grid's lower-left corner (y).
    #[serde(default)]
    pub origin_y: f64,
    /// Cell size in world units.
    pub resolution: f64,
    /// Grid width in cells.
    pub width: usize,
    /// Grid height in cells.
    pub height: usize,
}

/// One pose-fusion source (`[[navigation.pose_source]]`): reads
/// `sensor.{prefix}_x/_y/_heading` with the given fusion weight.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PoseSourceConfig {
    /// Entity prefix (e.g. `"odom"`, `"gps"`).
    pub prefix: String,
    /// Fusion weight (higher = more trusted). Default 1.0.
    #[serde(default = "default_pose_weight")]
    pub weight: f64,
}

fn default_pose_weight() -> f64 {
    1.0
}

/// A navigation actuator binding (`[navigation.steer]` / `[navigation.drive]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NavActuatorConfig {
    /// Actuator id (becomes `actuator.{name}`).
    pub name: String,
    /// Hardware channel.
    pub channel: i64,
}

/// Phase 18 dual-system reflex configuration (`[reflex]`). System 1: fast local
/// rules evaluated against world memory on a cadence. Requires `[perception]
/// world_memory = true`. Actions run via the safe dry-run logging sink until the
/// real spine sink is wired.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReflexConfig {
    /// Enable the reflex controller loop.
    #[serde(default)]
    pub enabled: bool,
    /// How often (ms) to evaluate the rules. Default 1000.
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// Max escalations to System 2 (the LLM) per minute. `None`/0 = unlimited.
    #[serde(default)]
    pub max_escalations_per_min: Option<u32>,
    /// The reflex rules to evaluate.
    #[serde(default)]
    pub rules: Vec<crate::agent::reflex::ReflexRule>,
    /// Append the standard safing rules (power/comms mode → safing actions).
    #[serde(default)]
    pub safing: bool,
    /// On `power.mode == critical`, also `Stop` this actuator via the movement
    /// controller (Track 0–bounded). Only used when `safing = true`.
    #[serde(default)]
    pub safing_stop_actuator: Option<SafingActuatorConfig>,
    /// Audio streams to escalate on an `"alarm"` label (safing). E.g. `["mic0"]`.
    #[serde(default)]
    pub safing_alarm_streams: Vec<String>,
    /// Sensor quantities to escalate when out-of-range (safing). E.g. `["temperature"]`.
    #[serde(default)]
    pub safing_unreliable_sensors: Vec<String>,
    /// Overheat / over-limit guards (`[[reflex.safing_overheat]]`): escalate when
    /// `sensor.{quantity}` exceeds `threshold`.
    #[serde(default)]
    pub safing_overheat: Vec<OverheatConfig>,
}

/// Actuator to stop on power-critical safing (`[reflex.safing_stop_actuator]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SafingActuatorConfig {
    /// Actuator id (matches a movement `actuator.{name}`).
    pub name: String,
    /// Hardware channel.
    pub channel: i64,
}

/// A numeric over-limit safing guard (`[[reflex.safing_overheat]]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverheatConfig {
    /// Sensor quantity to watch (becomes `sensor.{quantity}`).
    pub quantity: String,
    /// Escalate when the reading exceeds this value.
    pub threshold: f64,
}

/// Phase 16 experiential self-improvement configuration (`[self_improvement]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SelfImprovementConfig {
    /// Capture each agent run as a trajectory episode for later skill synthesis.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the trajectory database. Defaults to the data dir's
    /// `trajectories.db` when unset.
    #[serde(default)]
    pub db_path: Option<String>,
    /// How often (seconds) the background self-improvement loop runs. Default 3600.
    #[serde(default)]
    pub interval_secs: Option<u64>,
    /// Cap on auto-installed learned skills. Default 500.
    #[serde(default)]
    pub max_learned: Option<usize>,
    /// Inject relevant learned skills + similar past successes into the prompt
    /// each run (experience retrieval, Phase 16 P1). Default true.
    #[serde(default)]
    pub retrieval: Option<bool>,
    /// How many learned skills / past episodes to retrieve per run. Default 3.
    #[serde(default)]
    pub retrieval_k: Option<usize>,
    /// Extra verification requirements for synthesized skills
    /// (`[[self_improvement.verification]]`, Phase 16 P2).
    #[serde(default)]
    pub verification: Vec<VerificationRuleConfig>,
    /// Clean runs required at the current stage before `skill promote` is
    /// allowed (Track 0 staged rollout, Phase 16 P3). Default 3.
    #[serde(default)]
    pub promotion_clean_runs: Option<u32>,
    /// Enable the offline description-evolution job (Phase 16 P4): an LLM
    /// periodically rewrites learned-skill descriptions from usage traces
    /// (diff-logged, revertible; never touches stage/enabled). Default false.
    #[serde(default)]
    pub evolve: bool,
    /// How often (seconds) the evolution job runs. Default 86400 (daily).
    #[serde(default)]
    pub evolve_interval_secs: Option<u64>,
    /// Max descriptions rewritten per evolution pass. Default 5.
    #[serde(default)]
    pub evolve_max_per_pass: Option<usize>,
    /// Enable the dense (local-embedding) retrieval leg for episode memory.
    /// Requires a build with the `semantic` cargo feature; the model downloads
    /// once then inference is fully offline. Default false.
    #[serde(default)]
    pub semantic: bool,
}

/// One `[[self_improvement.verification]]` entry: a check that synthesized
/// skills matching `skill` (exact name, or prefix ending in `*`) must pass
/// before being trusted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationRuleConfig {
    /// Skill-name pattern (e.g. `"learned_*"` or an exact name).
    pub skill: String,
    /// Check kind: `"test_command"` or `"sensor_assertion"`.
    pub kind: String,
    /// Shell command to run (test_command).
    #[serde(default)]
    pub cmd: Option<String>,
    /// Expected exit code (test_command). Default 0.
    #[serde(default)]
    pub expect_exit: Option<i32>,
    /// Read-only tool to invoke (sensor_assertion), e.g. `"sensor_read"`.
    #[serde(default)]
    pub tool: Option<String>,
    /// Substring the tool output must contain (sensor_assertion).
    #[serde(default)]
    pub contains: Option<String>,
}

/// Phase 17 long-horizon harness configuration (`[harness]`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessConfig {
    /// Enable the harness (autostart missions spawn on agent start).
    #[serde(default)]
    pub enabled: bool,
    /// Delay between worker passes, ms. Default 2000.
    #[serde(default)]
    pub pass_delay_ms: Option<u64>,
    /// Hard budget of worker passes per mission run. Default 1000.
    #[serde(default)]
    pub max_passes: Option<usize>,
    /// Missions (`[[harness.mission]]`).
    #[serde(default)]
    pub mission: Vec<HarnessMissionConfig>,
}

/// One `[[harness.mission]]` entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessMissionConfig {
    /// Mission name (also the progress-record filename).
    pub name: String,
    /// Start automatically when the agent starts. Default false.
    #[serde(default)]
    pub autostart: bool,
    /// Objectives (`[[harness.mission.objective]]`).
    #[serde(default)]
    pub objective: Vec<HarnessObjectiveConfig>,
}

/// One `[[harness.mission.objective]]` entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessObjectiveConfig {
    pub id: String,
    pub description: String,
    /// Attempts before the objective is marked failed. Default 3.
    #[serde(default)]
    pub max_attempts: Option<u32>,
    /// Verification checks (`[[harness.mission.objective.verify]]`):
    /// `kind = "tool_contains" | "command" | "world_fact"` with the matching
    /// fields (`tool`/`args`/`contains`, `cmd`/`expect_exit`, `entity`).
    #[serde(default)]
    pub verify: Vec<HarnessCheckConfig>,
}

/// One verification-check entry for a harness objective.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessCheckConfig {
    pub kind: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub cmd: Option<String>,
    #[serde(default)]
    pub expect_exit: Option<i32>,
    #[serde(default)]
    pub entity: Option<String>,
}

/// Mission sequencer configuration (`[mission]`). Named missions (each
/// `[[mission.definition]]`) the `mission` tool can start; a runner ticks the
/// active one over the navigation + audio suites.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionConfig {
    /// Enable the `mission` + `mission_status` tools and the runner loop.
    #[serde(default)]
    pub enabled: bool,
    /// Tick cadence in ms (default 500).
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// The named mission library (`[[mission.definition]]`).
    #[serde(default, rename = "definition")]
    pub missions: Vec<crate::mission::Mission>,
}

/// Foresight (Track 1) configuration (`[foresight]`). Predictive rules that fire
/// *before* a forecast threshold crossing (each `[[foresight.rule]]`), plus the
/// read-only `foresight` query tool. Requires `[perception].world_memory = true`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForesightConfig {
    /// Enable the `foresight` tool and (if rules are set) the predictive loop.
    #[serde(default)]
    pub enabled: bool,
    /// Evaluation cadence in ms (default 1000).
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// Max predictive escalations to System 2 per minute. `None`/0 = unlimited.
    #[serde(default)]
    pub max_escalations_per_min: Option<u32>,
    /// The predictive rules (`[[foresight.rule]]`).
    #[serde(default, rename = "rule")]
    pub rules: Vec<crate::foresight::ForesightRule>,
}

/// Self-authored reflexes configuration (`[learning]`). Mines world-memory
/// history for antecedents of a configured bad `[learning.outcome]` and proposes
/// predictive rules; approval (via the `learn` tool) activates them into the
/// foresight engine. Requires `[perception].world_memory` (and `[foresight]` to
/// run the approved rules).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearningConfig {
    /// Enable the `learn` tool and the mining loop.
    #[serde(default)]
    pub enabled: bool,
    /// If set, auto-mine on this cadence (ms); else mine only on demand.
    #[serde(default)]
    pub auto_mine_interval_ms: Option<u64>,
    /// Lookback before an outcome event for the antecedent value (default 5000).
    #[serde(default)]
    pub lookback_ms: Option<u64>,
    /// Minimum supporting events to propose a rule (default 2).
    #[serde(default)]
    pub min_support: Option<usize>,
    /// Minimum specificity to propose (default 0.6).
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// Horizon applied to approved learned rules (default 60000).
    #[serde(default)]
    pub horizon_ms: Option<u64>,
    /// Debounce applied to approved learned rules (default 30000).
    #[serde(default)]
    pub debounce_ms: Option<u64>,
    /// Candidate antecedent entities to test.
    #[serde(default)]
    pub candidates: Vec<String>,
    /// The bad outcome to learn antecedents of (`[learning.outcome]`).
    #[serde(default)]
    pub outcome: Option<LearningOutcomeConfig>,
}

/// The bad-outcome spec for learning (`[learning.outcome]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningOutcomeConfig {
    /// Numeric entity whose threshold crossing is the "bad event".
    pub entity: String,
    /// Comparison operator.
    pub op: crate::agent::reflex::Cmp,
    /// Threshold value.
    pub threshold: f64,
}

/// Fleet coordination configuration (`[fleet]`). Runs a coordinator that ingests
/// node heartbeats, queues tasks, and allocates them to the best node. Records
/// the fleet view to world memory when available.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FleetConfig {
    /// Enable the `fleet` + `fleet_status` tools and the coordination loop.
    #[serde(default)]
    pub enabled: bool,
    /// Coordination tick cadence in ms (default 2000).
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// Heartbeat staleness (ms) past which a node is considered offline (default 30000).
    #[serde(default)]
    pub stale_ms: Option<u64>,
    /// Off-grid LoRa-mesh bridge: attach a serial LoRa node (see
    /// `firmware/lora-node`) so heartbeats heard over the air feed the coordinator.
    /// Only active when built with the `hardware` feature; ignored otherwise.
    #[serde(default)]
    pub lora_serial: Option<LoraSerialConfig>,
}

/// Serial-attached LoRa-mesh node (transparent serial⇄LoRa bridge). The node runs
/// `firmware/lora-node`; this host opens its port, spawns the RX loop that bridges
/// received `MeshFrame` heartbeats into the fleet `Coordinator`, and exposes the
/// radio as a `MeshRadio` transmit path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoraSerialConfig {
    /// Serial device path, e.g. `/dev/ttyUSB0` or `COM5`.
    pub port: String,
    /// Baud rate; must match the node firmware (`SERIAL_BAUD`, default 115200).
    #[serde(default = "default_lora_baud")]
    pub baud: u32,
    /// Multi-hop flooding: max hops an originated assignment may travel. `0` sends
    /// bare single-hop frames; `3` (default) lets messages relay across the mesh.
    #[serde(default = "default_relay_hops")]
    pub relay_hops: u8,
}

fn default_lora_baud() -> u32 {
    115_200
}

/// Host-side LoRa **mesh gateway bridge** (Phase B). Opens a base-station Heltec's
/// USB console (running `firmware/heltec-lora-linktest`) and ingests the node spine
/// messages it hears over the air into world memory. Read-only; only active when
/// built with the `hardware` feature and `[perception].world_memory` is on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoraGatewayConfig {
    /// Serial device path of the base-station Heltec console, e.g. `COM6`.
    pub port: String,
    /// Baud rate of the Heltec console (ESP-IDF default 115200).
    #[serde(default = "default_lora_baud")]
    pub baud: u32,
}

/// Mesh supervisor (Phase B "fold mesh into brain"): a host control loop that turns the
/// mesh facts in world memory into a derived per-node health view and optional
/// autonomous recovery commands. Requires `[perception].world_memory`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshSupervisorConfig {
    /// Enable the supervisor loop.
    #[serde(default)]
    pub enabled: bool,
    /// A node with no mesh message newer than this (ms) is considered offline.
    #[serde(default = "default_mesh_stale_ms")]
    pub stale_ms: u64,
    /// Supervisor tick cadence (ms).
    #[serde(default = "default_mesh_tick_ms")]
    pub tick_ms: u64,
    /// Command to auto-issue to a node that has gone offline (e.g. `"capabilities"` to
    /// ping it). `None` = observe-only (record health, never send).
    #[serde(default)]
    pub recover: Option<String>,
    /// Minimum interval (ms) between recovery commands to the same node.
    #[serde(default = "default_mesh_recovery_interval_ms")]
    pub min_recovery_interval_ms: u64,
}

impl Default for MeshSupervisorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stale_ms: default_mesh_stale_ms(),
            tick_ms: default_mesh_tick_ms(),
            recover: None,
            min_recovery_interval_ms: default_mesh_recovery_interval_ms(),
        }
    }
}

fn default_mesh_stale_ms() -> u64 {
    60_000
}

fn default_mesh_tick_ms() -> u64 {
    5_000
}

fn default_mesh_recovery_interval_ms() -> u64 {
    30_000
}

fn default_relay_hops() -> u8 {
    3
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
    /// Phase B: host-side LoRa mesh gateway bridge — reads a base-station Heltec's
    /// console and ingests received node spine messages into world memory.
    #[serde(default)]
    pub lora_gateway: Option<LoraGatewayConfig>,
    /// Phase B: mesh supervisor — derive per-node health from mesh facts and optionally
    /// auto-recover offline nodes over the mesh.
    #[serde(default)]
    pub mesh_supervisor: MeshSupervisorConfig,
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
    /// Phase 17 long-horizon harness (`[harness]`).
    #[serde(default)]
    pub harness: HarnessConfig,
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
    /// Agent-to-Agent (A2A) protocol configuration.
    #[serde(default)]
    pub a2a: A2AConfig,
    /// Track 0 physical-action safety: deterministic limits + tamper-evident audit.
    #[serde(default)]
    pub safety: crate::security::SafetyConfig,
    /// Phase 16 experiential self-improvement (trajectory capture).
    #[serde(default)]
    pub self_improvement: SelfImprovementConfig,
    /// Phase 18 perception (world memory).
    #[serde(default)]
    pub perception: PerceptionConfig,
    /// Phase 18 dual-system reflexes (System 1).
    #[serde(default)]
    pub reflex: ReflexConfig,
    /// Movement subsystem — typed, safety-bounded actuation tool.
    #[serde(default)]
    pub movement: MovementConfig,
    /// Sensing subsystem — quality-aware sensor ingestion + `sense` tool.
    #[serde(default)]
    pub sensing: SensingConfig,
    /// Audio suite — `hear` (perceive) + `speak` (act) tools.
    #[serde(default)]
    pub audio_suite: AudioSuiteConfig,
    /// Power suite — battery telemetry + derived power mode for safing.
    #[serde(default)]
    pub power: PowerConfig,
    /// Comms suite — link telemetry + aggregate net mode for offline safing.
    #[serde(default)]
    pub comms: CommsConfig,
    /// Navigation suite — localization + movement fusion (goal-driven driving).
    #[serde(default)]
    pub navigation: NavigationConfig,
    /// Mission sequencer — deliberative, guarded multi-step missions.
    #[serde(default)]
    pub mission: MissionConfig,
    /// Foresight (Track 1) — predictive rules that act before forecast events.
    #[serde(default)]
    pub foresight: ForesightConfig,
    /// Self-authored reflexes — mine antecedents and propose predictive rules.
    #[serde(default)]
    pub learning: LearningConfig,
    /// Fleet coordination — allocate tasks across multiple robot nodes.
    #[serde(default)]
    pub fleet: FleetConfig,
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
            if let Some(ref h) = self.proxy.host {
                if h.trim().is_empty() {
                    anyhow::bail!("proxy is enabled but proxy.host is empty");
                }
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

        // ── Port range validation ──────────────────────────────────────────────
        // u16 already caps at 65535, so we only need to reject 0 for ports that
        // are not yet checked above.
        if self.tunnel.local_port == 0 {
            warnings.push("tunnel.local_port is 0; this is unlikely to be valid".to_string());
        }
        if let Some(p) = self.proxy.port {
            if p == 0 {
                warnings.push("proxy.port is 0; this is unlikely to be valid".to_string());
            }
        }
        if let Some(p) = self.channels.whatsapp.webhook_port {
            if p == 0 {
                warnings.push(
                    "channels.whatsapp.webhook_port is 0; this is unlikely to be valid".to_string(),
                );
            }
        }
        if let Some(p) = self.channels.feishu.webhook_port {
            if p == 0 {
                warnings.push(
                    "channels.feishu.webhook_port is 0; this is unlikely to be valid".to_string(),
                );
            }
        }
        if let Some(p) = self.channels.irc.port {
            if p == 0 {
                warnings.push("channels.irc.port is 0; this is unlikely to be valid".to_string());
            }
        }
        if let Some(p) = self.spine.p2p_tcp_port {
            if p == 0 {
                warnings.push("spine.p2p_tcp_port is 0; this is unlikely to be valid".to_string());
            }
        }
        if let Some(p) = self.spine.p2p_discovery_port {
            if p == 0 {
                warnings.push(
                    "spine.p2p_discovery_port is 0; this is unlikely to be valid".to_string(),
                );
            }
        }

        // ── P2P node_id format ─────────────────────────────────────────────────
        if self.spine.kind == "p2p" {
            if let Some(ref id) = self.spine.p2p_node_id {
                if id.trim().is_empty() {
                    anyhow::bail!("spine.p2p_node_id is set but empty");
                }
                if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    warnings.push(format!(
                        "spine.p2p_node_id '{}' contains characters other than \
                         alphanumerics and hyphens",
                        id
                    ));
                }
            }
        }

        // ── Channel token format validation ────────────────────────────────────
        if let Some(ref token) = self.channels.telegram.token {
            // Telegram bot tokens look like "123456789:ABCdefGHIjklMNOpqrsTUVwxyz"
            let parts: Vec<&str> = token.splitn(2, ':').collect();
            if parts.len() != 2
                || !parts[0].chars().all(|c| c.is_ascii_digit())
                || parts[0].is_empty()
                || parts[1].is_empty()
            {
                warnings.push(
                    "channels.telegram.token does not match expected format \
                     (digits:alphanumeric)"
                        .to_string(),
                );
            }
        }
        if let Some(ref token) = self.channels.discord.token {
            if token.len() < 50 {
                warnings.push(format!(
                    "channels.discord.token is only {} chars; \
                     Discord bot tokens are typically 70+ characters",
                    token.len()
                ));
            }
        }
        if let Some(ref token) = self.channels.slack.bot_token {
            if !token.starts_with("xoxb-") {
                warnings.push(
                    "channels.slack.bot_token does not start with 'xoxb-'; \
                     Slack bot tokens should begin with this prefix"
                        .to_string(),
                );
            }
        }

        // ── MQTT credential validation ─────────────────────────────────────────
        match (&self.spine.username, &self.spine.password) {
            (Some(_), None) => {
                warnings.push(
                    "spine.username is set but spine.password is not; \
                     MQTT brokers usually require both"
                        .to_string(),
                );
            }
            (None, Some(_)) => {
                warnings.push(
                    "spine.password is set but spine.username is not; \
                     MQTT brokers usually require both"
                        .to_string(),
                );
            }
            _ => {}
        }

        // ── Provider validation ────────────────────────────────────────────────
        {
            let primary_has_model = !self.provider.model.trim().is_empty();
            let any_fallback_has_model = self
                .provider
                .fallbacks
                .iter()
                .any(|f| !f.model.trim().is_empty());
            if !primary_has_model && !any_fallback_has_model {
                anyhow::bail!(
                    "no provider has a model set; \
                     at least provider.model or a fallback model must be configured"
                );
            }
        }

        // ── File path validation (non-fatal) ───────────────────────────────────
        for (label, path_opt) in [
            ("spine.ca_cert_path", &self.spine.ca_cert_path),
            ("spine.client_cert_path", &self.spine.client_cert_path),
            ("spine.client_key_path", &self.spine.client_key_path),
        ] {
            if let Some(ref p) = path_opt {
                if !std::path::Path::new(p).exists() {
                    tracing::warn!("{} points to '{}' which does not exist", label, p);
                    warnings.push(format!("{} points to '{}' which does not exist", label, p));
                }
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

    // ── Enhanced validation tests ──────────────────────────────────────────────

    #[test]
    fn validate_warns_zero_tunnel_port() {
        let mut config = Config::default();
        config.tunnel.local_port = 0;
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("tunnel.local_port")));
    }

    #[test]
    fn validate_warns_zero_whatsapp_webhook_port() {
        let mut config = Config::default();
        config.channels.whatsapp.webhook_port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("whatsapp.webhook_port")));
    }

    #[test]
    fn validate_warns_zero_feishu_webhook_port() {
        let mut config = Config::default();
        config.channels.feishu.webhook_port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("feishu.webhook_port")));
    }

    #[test]
    fn validate_warns_zero_irc_port() {
        let mut config = Config::default();
        config.channels.irc.port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("irc.port")));
    }

    #[test]
    fn validate_warns_zero_p2p_tcp_port() {
        let mut config = Config::default();
        config.spine.p2p_tcp_port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("p2p_tcp_port")));
    }

    #[test]
    fn validate_warns_zero_p2p_discovery_port() {
        let mut config = Config::default();
        config.spine.p2p_discovery_port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("p2p_discovery_port")));
    }

    #[test]
    fn validate_warns_zero_proxy_port() {
        let mut config = Config::default();
        config.proxy.port = Some(0);
        let warnings = config.validate().unwrap();
        assert!(warnings.iter().any(|w| w.contains("proxy.port")));
    }

    #[test]
    fn validate_rejects_empty_p2p_node_id() {
        let mut config = Config::default();
        config.spine.kind = "p2p".to_string();
        config.spine.p2p_node_id = Some("  ".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_warns_p2p_node_id_bad_chars() {
        let mut config = Config::default();
        config.spine.kind = "p2p".to_string();
        config.spine.p2p_node_id = Some("node_one!".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("p2p_node_id") && w.contains("characters")));
    }

    #[test]
    fn validate_accepts_good_p2p_node_id() {
        let mut config = Config::default();
        config.spine.kind = "p2p".to_string();
        config.spine.p2p_node_id = Some("node-42-abc".to_string());
        let warnings = config.validate().unwrap();
        assert!(!warnings.iter().any(|w| w.contains("p2p_node_id")));
    }

    #[test]
    fn validate_rejects_proxy_empty_host() {
        let mut config = Config::default();
        config.proxy.enabled = true;
        config.proxy.host = Some("  ".to_string());
        config.proxy.port = Some(8080);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_warns_bad_telegram_token() {
        let mut config = Config::default();
        config.channels.telegram.token = Some("not-a-valid-token".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("telegram.token") && w.contains("format")));
    }

    #[test]
    fn validate_accepts_good_telegram_token() {
        let mut config = Config::default();
        config.channels.telegram.token = Some("123456789:ABCdefGHIjklMNOpqrs".to_string());
        let warnings = config.validate().unwrap();
        assert!(!warnings.iter().any(|w| w.contains("telegram.token")));
    }

    #[test]
    fn validate_warns_short_discord_token() {
        let mut config = Config::default();
        config.channels.discord.token = Some("short".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("discord.token") && w.contains("chars")));
    }

    #[test]
    fn validate_accepts_long_discord_token() {
        let mut config = Config::default();
        config.channels.discord.token = Some("A".repeat(72));
        let warnings = config.validate().unwrap();
        assert!(!warnings.iter().any(|w| w.contains("discord.token")));
    }

    #[test]
    fn validate_warns_slack_token_wrong_prefix() {
        let mut config = Config::default();
        config.channels.slack.bot_token = Some("xoxp-bad-prefix".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("slack.bot_token") && w.contains("xoxb-")));
    }

    #[test]
    fn validate_accepts_good_slack_token() {
        let mut config = Config::default();
        config.channels.slack.bot_token = Some("xoxb-good-token".to_string());
        let warnings = config.validate().unwrap();
        assert!(!warnings.iter().any(|w| w.contains("slack.bot_token")));
    }

    #[test]
    fn validate_warns_mqtt_username_without_password() {
        let mut config = Config::default();
        config.spine.username = Some("admin".to_string());
        config.spine.password = None;
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("username") && w.contains("password")));
    }

    #[test]
    fn validate_warns_mqtt_password_without_username() {
        let mut config = Config::default();
        config.spine.username = None;
        config.spine.password = Some("secret".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("password") && w.contains("username")));
    }

    #[test]
    fn validate_accepts_mqtt_both_creds_set() {
        let mut config = Config::default();
        config.spine.username = Some("admin".to_string());
        config.spine.password = Some("secret".to_string());
        let warnings = config.validate().unwrap();
        assert!(!warnings
            .iter()
            .any(|w| w.contains("username") && w.contains("password")));
    }

    #[test]
    fn validate_rejects_empty_provider_model() {
        let mut config = Config::default();
        config.provider.model = "  ".to_string();
        config.provider.fallbacks = vec![];
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_fallback_with_model() {
        let mut config = Config::default();
        config.provider.model = "  ".to_string();
        config.provider.fallbacks = vec![ProviderConfig {
            model: "claude-3-5-sonnet".to_string(),
            ..ProviderConfig::default()
        }];
        let warnings = config.validate().unwrap();
        assert!(!warnings.iter().any(|w| w.contains("provider")));
    }

    #[test]
    fn validate_warns_nonexistent_cert_path() {
        let mut config = Config::default();
        config.spine.tls = true;
        config.spine.port = 8883;
        config.spine.ca_cert_path = Some("/nonexistent/path/to/ca.crt".to_string());
        let warnings = config.validate().unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("ca_cert_path") && w.contains("does not exist")));
    }

    #[test]
    fn default_config_still_validates_clean() {
        // Ensure all new validations don't break the default config.
        let config = Config::default();
        let warnings = config.validate().unwrap();
        assert!(warnings.is_empty(), "Unexpected warnings: {:?}", warnings);
    }
}

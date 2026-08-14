//! Oh-Ben-Claw configuration schema and loading.
//!
//! Configuration is stored in TOML format at `~/.oh-ben-claw/config.toml`.
//! The `Config` struct is the root of the configuration tree.

use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod first_run;
// `paths` moved to the `obc-paths` crate on 2026-07-30 so the memory substrate can
// depend on it without depending on the agent's config module. Re-exported here
// because `crate::config::paths::…` is the path nine modules already use.
pub use obc_paths as paths;
pub use obc_safety::secret::SecretString;

/// Report any credential stored inline in the config file.
///
/// Not an error — an inline key works, and breaking a running deployment over a style
/// preference would be its own kind of rude. But it is worth saying out loud once per
/// boot, because the failure mode is social rather than technical: the config file is
/// the artifact people paste into issues, commit to repos, and hand over when asking
/// for help, and a key in it leaves with the file.
///
/// Returns the provider labels carrying inline keys, primary first, so the caller can
/// decide how loudly to say it.
pub fn inline_secret_providers(config: &Config) -> Vec<String> {
    fn walk(p: &ProviderConfig, path: String, out: &mut Vec<String>) {
        if p.api_key.as_ref().is_some_and(|k| !k.is_empty()) {
            out.push(format!("{path} ({})", p.name));
        }
        for (i, fb) in p.fallbacks.iter().enumerate() {
            // Fallbacks are the easy ones to forget: they are edited once and never
            // looked at again, which is exactly how a key outlives the person who set it.
            walk(fb, format!("{path}.fallbacks[{i}]"), out);
        }
    }
    let mut out = Vec::new();
    walk(&config.provider, "provider".to_string(), &mut out);
    out
}

// ── Provider Configuration ───────────────────────────────────────────────────

/// The LLM provider's own configuration block, which lives with the providers.
///
/// Moved to [`obc_providers`] on 2026-08-13 and re-exported here, because the
/// root `Config` composes it and the call sites outside `providers/` name it
/// through this path.
///
/// It is the arrangement every extracted crate here already uses — obc-planner
/// owns `DeploymentConfig`, obc-conscience owns `ConscienceConfig`, obc-cost
/// owns `CostConfig` — and this one was the exception. Two of its own fields
/// were typed from the providers module while all ten provider files imported
/// it from here, so a single struct split across two modules was a mutual pair
/// in the dependency graph, and most of the core's remaining cycles ran through
/// it.
pub use obc_providers::ProviderConfig;

/// The spine's own configuration blocks, which live with the spine.
///
/// Moved to [`obc_spine`] on 2026-08-13 and re-exported here, because the
/// root `Config` composes them. Same move as `ProviderConfig` the day before,
/// and the same rule every extracted crate here already follows: the module
/// owns its config block.
///
/// These were the *only* thing a 4935-line module named outside itself, and
/// two of the core's remaining cycles ran through them.
pub use obc_spine::{MeshSupervisorConfig, SpineConfig};

/// The agent's own configuration blocks, which live with the agent.
///
/// Moved to [`crate::agent`] on 2026-08-13 and re-exported here. The root
/// `Config` composes them, and `crate::approval` reads `AutonomyConfig` and
/// `AutonomyLevel` through this path deliberately: routing that module at the
/// agent instead would trade one mutual pair in the dependency graph for
/// another, since `agent` already names `approval`.
///
/// Third and last application of the rule this month -- the module owns its
/// config block, the root `Config` composes it. `ProviderConfig` went to
/// `providers`, `SpineConfig` and `MeshSupervisorConfig` went to `spine`, and
/// these four close the final cycle in the core.
pub use crate::agent::{AgentConfig, EdgeConfig};
pub use obc_approval::{AutonomyConfig, AutonomyLevel};

// ── Agent Configuration ──────────────────────────────────────────────────────

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

// Tunnel configuration moved to `obc-tunnel` on 2026-08-06, with the providers
// that read it. Re-exported, so `crate::config::TunnelConfig` is unchanged.
pub use obc_tunnel::config::TunnelConfig;

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
    /// Optional Operate-mode token (Ecosystem Integration I4). When set,
    /// **mutating** API requests (anything but GET/HEAD) additionally require
    /// the `X-OBC-Operate` header carrying this token — read-only by default,
    /// explicit elevation for remote actions. When unset, mutating requests
    /// follow the ordinary `api_token` rules (local-console compatibility).
    #[serde(default)]
    pub operate_token: Option<String>,
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
            operate_token: None,
            serve_pwa: true,
            cors_origins: vec![],
        }
    }
}

// ── Edge-Native Configuration ─────────────────────────────────────────────────

// ── Autonomy Configuration ────────────────────────────────────────────────────

// Cost configuration moved to `obc-cost` on 2026-08-06, with the tracker that
// reads it. Same arrangement as `DeploymentConfig` (obc-planner) and
// `ConscienceConfig` (obc-conscience): the crate owns its own config block and
// this file composes them. Re-exported, so `crate::config::CostConfig` is
// unchanged at every call site.
pub use obc_cost::config::CostConfig;

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

// ── Paths ────────────────────────────────────────────────────────────────────

/// Where this instance keeps its data.
///
/// ```toml
/// [paths]
/// data_dir = "/srv/obc/kitchen"
/// ```
///
/// Unset means the platform convention (`~/.local/share/oh-ben-claw` on Linux,
/// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS). The
/// `OBC_DATA_DIR` environment variable overrides both, because starting a second
/// instance should not require editing a file that may be shared or checked in.
///
/// See [`paths`] for the resolution order and why there is only one root.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathsConfig {
    /// Root directory for this instance's databases, logs and notes.
    #[serde(default)]
    pub data_dir: Option<String>,
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

// ── Deployment Configuration (new in Phase 13) ────────────────────────────────

/// The `[deployment]` block — re-exported from the [`obc_planner`] crate.
///
/// It moved there on 2026-07-30 so it sits with `to_deployment_toml`, the code
/// that writes it, and `from_deployment_config`, the code that reads it back.
/// The two directions are a fixed point, and a schema whose writer and reader
/// live in different crates is one that can drift silently.
///
/// `oh_ben_claw::config::DeploymentConfig` still resolves, so no call site or
/// config file changed.
pub use obc_planner::config::{DeploymentConfig, DeploymentHardwareConfig};

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
    /// Retention policies for beliefs nothing else will retract — `[[perception.expiry]]`.
    ///
    /// Empty by default, and deliberately so. Supersession needs a newer value for the
    /// same entity, source liveness needs the author to stop existing, and dependency
    /// withdrawal needs a recorded in-list; an agent's own note at `incident.<subject>`
    /// has none of the three and stays believed forever. A policy is the only way to say
    /// that a *kind* of belief goes stale, and since it is the one withdrawal that comes
    /// from a rule rather than from the world, it only ever does what it is told:
    ///
    /// ```toml
    /// [[perception.expiry]]
    /// prefix = "incident."     # required, never empty — an empty prefix is the store
    /// max_age_ms = 604800000   # 7 days, measured from ingest, not caller-set valid_from
    /// origins = ["asserted"]   # optional; omit for any origin under the prefix
    /// ```
    #[serde(default)]
    pub expiry: Vec<crate::memory::expiry::ExpiryPolicy>,
    /// What the agent is shown about its own world model — `[perception.context]`.
    ///
    /// Until this existed, `build_context` was the system prompt plus the last 50
    /// messages and nothing else: the world model and the thing reasoning on top of it
    /// were not connected. Defaults are deliberately modest (24 facts, 5 withdrawals,
    /// a 2400-character hard cap) because this renders on every turn.
    #[serde(default)]
    pub context: crate::agent::world_context::WorldContextConfig,
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

/// Where a fixed camera physically is, in the same world frame the navigation
/// grid uses.
///
/// Cameras are static sensors; the robot is not. Knowing where a camera stands
/// is what lets a detection it makes become an obstacle the planner avoids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraPositionConfig {
    /// The camera's node id, matching `device_id` in ClawCam detections.
    pub node: String,
    /// World X in metres.
    pub x: f64,
    /// World Y in metres.
    pub y: f64,
}

fn default_hazard_step_m() -> f64 {
    0.25
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
    /// World position of each fixed camera, so a detection can be stamped into
    /// the occupancy grid as a hazard the mobile robot routes around.
    ///
    /// Empty (the default) leaves `vision/clawcam_spatial.rs` unwired, which is
    /// what it was until 2026-07-30. Set `hazard_radius_m` and list the cameras
    /// to turn it on.
    #[serde(default)]
    pub cameras: Vec<CameraPositionConfig>,
    /// Radius in metres of the hazard disc stamped around a camera that just
    /// saw something. Zero (the default) disables the whole path, so adding
    /// camera positions alone changes nothing until you also say how far the
    /// hazard reaches.
    #[serde(default)]
    pub hazard_radius_m: f64,
    /// Grid sampling step for the disc, in metres.
    #[serde(default = "default_hazard_step_m")]
    pub hazard_step_m: f64,
    /// Also poll the gateway's analytics reports (`get_anomaly_report`,
    /// `get_encounter_report`, `get_calibration_report`) on their own slower
    /// cadence → `clawcam.analytics.*` facts, and (with `[reflex]` enabled)
    /// append the analytics reflex rules — so an unusually quiet day (possible
    /// knocked-over/obstructed camera), an activity surge, or a miscalibrated
    /// model *escalates* instead of sitting unread in a report.
    #[serde(default)]
    pub poll_analytics: bool,
    /// Analytics poll cadence in ms (the reports are daily aggregates — hourly
    /// is plenty). Default 3 600 000 (1 h); clamped to ≥ 60 000 at spawn.
    #[serde(default = "default_clawcam_analytics_interval_ms")]
    pub analytics_interval_ms: u64,
    /// |z| at/above which the latest day's detection count escalates
    /// (drop ⇒ possible camera fault, spike ⇒ surge). Default 2.0.
    #[serde(default = "default_clawcam_anomaly_z_alert")]
    pub anomaly_z_alert: f64,
    /// Debounce for the analytics reflex rules in ms. Default 21 600 000 (6 h) —
    /// these facts change on a daily scale.
    #[serde(default = "default_clawcam_analytics_debounce_ms")]
    pub analytics_debounce_ms: u64,
}

fn default_clawcam_analytics_interval_ms() -> u64 {
    3_600_000
}
fn default_clawcam_anomaly_z_alert() -> f64 {
    2.0
}
fn default_clawcam_analytics_debounce_ms() -> u64 {
    21_600_000
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
    /// Node id the movement safety limits apply to (matches `[[safety.limits]]`
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
    pub rules: Vec<obc_reflex::ReflexRule>,
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

/// System 2 slow-reasoner configuration (`[system2]`, Phase 18).
///
/// When enabled (and the reflex controller is running), System 1 escalations
/// wake the LLM agent — novelty-gated and budget-capped, never per event.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct System2Config {
    /// Enable escalation-driven LLM wakes. Off by default: escalations still
    /// notify the operator; turning this on additionally spends LLM calls.
    #[serde(default)]
    pub enabled: bool,
    /// Suppress repeat wakes for the same situation fingerprint within this
    /// window (ms). Default 600 000 (10 minutes).
    #[serde(default)]
    pub novelty_window_ms: Option<u64>,
    /// Hard cap on LLM wakes per sliding hour. Default 6.
    #[serde(default)]
    pub max_wakes_per_hour: Option<u32>,
    /// Bounded wake-queue capacity between System 1 and System 2. Default 8.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
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
    pub missions: Vec<obc_mission::Mission>,
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
    pub rules: Vec<obc_foresight::ForesightRule>,
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
    pub op: obc_reflex::Cmp,
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

/// Escalation notifications (`[notifications]`): wire reflex escalations (mesh node
/// lost, battery critical, alarm heard, …) to operator-facing channels — a durable
/// log-of-record in world memory and/or a webhook (Slack/Discord/generic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Enable escalation notifications (wraps the reflex action sink).
    #[serde(default)]
    pub enabled: bool,
    /// Append each escalation to world memory as `notifications.escalation`.
    #[serde(default = "default_notify_log")]
    pub log_to_world_memory: bool,
    /// Optional webhook URL to POST `{ "text": … }` on each escalation.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// Speak escalations aloud (headline only) through the audio speech sink (TTS,
    /// speaker over the spine, or dry-run — same selection as `[audio_suite]`).
    #[serde(default)]
    pub speak_escalations: bool,
    /// De-duplicate identical escalations within this window (ms) across all channels;
    /// the next alert after the window notes how many repeats were collapsed. `0`
    /// disables de-dup (every escalation notifies).
    #[serde(default)]
    pub dedup_window_ms: u64,
    /// Emit a periodic digest (the escalation log rolled up by reason) every this many
    /// ms, over the same trailing window. `0` disables it; e.g. `86400000` = daily.
    #[serde(default)]
    pub digest_interval_ms: u64,
    /// Minimum severity (`"info"`/`"warning"`/`"critical"`) each channel receives; below
    /// it, that channel is skipped. Default (`None`) = the channel receives everything.
    /// E.g. set `webhook_min_severity = "critical"` and `speak_min_severity = "critical"`
    /// to only push/speak the loud stuff while the log records all.
    #[serde(default)]
    pub log_min_severity: Option<String>,
    #[serde(default)]
    pub webhook_min_severity: Option<String>,
    #[serde(default)]
    pub speak_min_severity: Option<String>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            log_to_world_memory: true,
            webhook_url: None,
            speak_escalations: false,
            dedup_window_ms: 0,
            digest_interval_ms: 0,
            log_min_severity: None,
            webhook_min_severity: None,
            speak_min_severity: None,
        }
    }
}

fn default_notify_log() -> bool {
    true
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
    /// Escalation notifications — fan reflex escalations out to a log-of-record and/or
    /// a webhook.
    #[serde(default)]
    pub notifications: NotificationsConfig,
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
    /// HTTP proxy for outbound requests (new in Phase 11).
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Where this instance keeps its data (`[paths]`).
    #[serde(default)]
    pub paths: PathsConfig,
    /// Browser automation configuration (new in Phase 12).
    #[serde(default)]
    pub browser: BrowserConfig,
    /// ClawHub community skill registry configuration (new in Phase 12).
    /// Deployment scheme generator configuration (new in Phase 13).
    #[serde(default)]
    pub deployment: DeploymentConfig,
    /// Agent-to-Agent (A2A) protocol configuration.
    #[serde(default)]
    pub a2a: A2AConfig,
    /// Track 0 physical-action safety: deterministic limits + tamper-evident audit.
    #[serde(default)]
    pub safety: crate::security::SafetyConfig,
    /// Conscience: deterministic gates on what the agent may PERCEIVE (consent
    /// registry, default-deny humans) and REACH (egress allowlist). Track 0
    /// extended to the front of the pipeline. Off unless `[conscience]` is set.
    #[serde(default)]
    pub conscience: obc_conscience::ConscienceConfig,
    /// Phase 16 experiential self-improvement (trajectory capture).
    #[serde(default)]
    pub self_improvement: SelfImprovementConfig,
    /// Phase 18 perception (world memory).
    #[serde(default)]
    pub perception: PerceptionConfig,
    /// Phase 18 dual-system reflexes (System 1).
    #[serde(default)]
    pub reflex: ReflexConfig,
    /// Phase 18 slow reasoner (System 2): escalation-driven, novelty-gated
    /// LLM wakes.
    #[serde(default)]
    pub system2: System2Config,
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
    /// Load the configuration.
    ///
    /// Precedence, first hit wins:
    ///   1. the `OBC_CONFIG` env var (set by the CLI's `--config` flag, or directly)
    ///   2. `config.toml` in the data root — so that `OBC_DATA_DIR=/srv/obc/b` gives
    ///      a fully self-contained second instance, config included, without editing
    ///      anything the first instance reads
    ///   3. [`Self::default_config_path`] (ProjectDirs — e.g.
    ///      `%APPDATA%\thewriterben\oh-ben-claw\config\config.toml` on Windows,
    ///      `~/.config/oh-ben-claw/config.toml` on Linux)
    ///   4. `~/.oh-ben-claw/config.toml` — kept for anyone who followed the old
    ///      documentation, which named it for years
    ///
    /// Whichever wins, `[paths].data_dir` is published to [`paths`] before returning,
    /// so every subsystem resolves the same root. Note the one asymmetry: a config
    /// found at (2) cannot move the data root it was found in — only `OBC_DATA_DIR`
    /// can, and that is the point of the ordering rather than a limitation of it.
    ///
    /// If none exist, built-in defaults are used and a warning names the
    /// provider they select, because those defaults are a cloud provider and
    /// silently reaching for one is worse than failing loudly.
    /// An explicitly named file that is missing or malformed is a hard error —
    /// never silently swapped for defaults. If only the default path is in play
    /// and absent, a default configuration is returned.
    pub fn load() -> Result<Self> {
        if let Ok(explicit) = std::env::var("OBC_CONFIG") {
            let path = PathBuf::from(&explicit);
            if !path.exists() {
                anyhow::bail!("config file not found: {explicit} (from --config / OBC_CONFIG)");
            }
            let content = std::fs::read_to_string(&path)?;
            let config: Self = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("config parse error in {explicit}: {e}"))?;
            tracing::info!("Loaded config from {:?} (explicit)", path);
            return Ok(config
                .with_provider_from_env_if_absent(&content)
                .published());
        }
        // A config beside the data. This is what makes a relocated instance
        // self-contained: `OBC_DATA_DIR=/srv/obc/b obc start` picks up b's config, b's
        // database and b's audit chain without touching anything a's config says.
        let rooted = paths::data_dir().join("config.toml");
        if rooted.exists() {
            let content = std::fs::read_to_string(&rooted)?;
            let config: Self = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("config parse error in {}: {e}", rooted.display()))?;
            tracing::info!("Loaded config from {:?} (data root)", rooted);
            return Ok(config
                .with_provider_from_env_if_absent(&content)
                .published());
        }

        let config_path = Self::default_config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Self = toml::from_str(&content)?;
            tracing::info!("Loaded config from {:?}", config_path);
            return Ok(config
                .with_provider_from_env_if_absent(&content)
                .published());
        }

        // `~/.oh-ben-claw/config.toml` is where the documentation, the setup
        // scripts and this module's own doc comments have always said the
        // config lives -- and it sits next to the data the agent already keeps
        // there. Before this it was not on the search path at all, so a config
        // in the documented location was silently ignored and the agent fell
        // back to built-in defaults, which name a *cloud* provider. Quietly
        // trying to spend money because a config was in the obvious place is a
        // bad failure mode, so look there too.
        if let Some(home) = Self::home_config_path() {
            if home.exists() {
                let content = std::fs::read_to_string(&home)?;
                let config: Self = toml::from_str(&content).map_err(|e| {
                    anyhow::anyhow!("config parse error in {}: {e}", home.display())
                })?;
                tracing::info!("Loaded config from {:?} (home)", home);
                return Ok(config
                    .with_provider_from_env_if_absent(&content)
                    .published());
            }
        }

        // No config: choose the provider whose key is actually present rather than
        // insisting on the compiled-in default. See `first_run` for why this only
        // happens here — an explicit config is a stated intention and is never
        // second-guessed.
        let (resolution, provider) = first_run::resolve_from_env();
        match &resolution {
            first_run::Resolution::FromEnv {
                provider: p,
                var,
                model,
            } => tracing::info!(
                provider = %p, model = %model, from = %var,
                "No config file — using the provider whose API key is set. \
                 Write a config.toml to pin this — see config.example.toml."
            ),
            first_run::Resolution::LocalFallback { provider: p, model } => tracing::warn!(
                provider = %p, model = %model,
                "{}", first_run::guidance()
            ),
        }
        tracing::debug!(
            "No config file at {:?}, in the data root, or at ~/.oh-ben-claw/config.toml; \
             set OBC_CONFIG or pass --config to load a specific file.",
            config_path
        );
        Ok(Config {
            provider,
            ..Self::default()
        }
        .published())
    }

    /// Fill in the provider from the environment when the file did not name one.
    ///
    /// An explicit config is a stated intention and is never second-guessed — but a
    /// file with no `[provider]` table has not stated one. Before this, `serde`'s
    /// `default` quietly supplied `openai`/`gpt-4o`, so a config that deliberately
    /// left the brain unspecified produced "No API key found for provider 'openai'"
    /// at someone who had exported `ANTHROPIC_API_KEY`. Both reference bodies and
    /// every config the deployment generator emits omit `[provider]` on purpose, so
    /// this was the common case, not the exotic one.
    ///
    /// The test is `provider.name`, not the presence of a `[provider]` table. Writing
    /// `[provider.retry]` to tune backoff creates that table without choosing a
    /// vendor, and the first draft of this — checking for the table — meant a body
    /// that tuned retry silently opted out of environment resolution and got
    /// `openai` again. What matters is whether a provider was *named*.
    fn with_provider_from_env_if_absent(mut self, raw: &str) -> Self {
        let names_provider = toml::from_str::<toml::Value>(raw)
            .ok()
            .and_then(|v| v.get("provider").and_then(|p| p.get("name")).cloned())
            .is_some();
        if names_provider {
            return self;
        }
        let (resolution, provider) = first_run::resolve_from_env();
        match &resolution {
            first_run::Resolution::FromEnv {
                provider: p,
                var,
                model,
            } => tracing::info!(
                provider = %p, model = %model, from = %var,
                "Config names no [provider]; using the one whose API key is set"
            ),
            first_run::Resolution::LocalFallback { provider: p, model } => tracing::warn!(
                provider = %p, model = %model,
                "Config names no [provider], and no API key is set. {}",
                first_run::guidance()
            ),
        }
        self.provider = provider;
        self
    }

    /// Publish `[paths].data_dir` so every subsystem resolves the same root, and
    /// return self.
    ///
    /// Called on every path out of [`Self::load`] rather than by the binary, because
    /// a library consumer that loads a config and then opens a `MemoryStore` should
    /// get the database the config asked for — not silently the platform default
    /// because it forgot a wiring call it had no way to know about.
    fn published(self) -> Self {
        if let Some(dir) = self.paths.data_dir.as_deref() {
            paths::set_configured(dir);
        }
        self
    }

    /// `~/.oh-ben-claw/config.toml` — kept on the search path for anyone who
    /// followed the old documentation, which named this location for years.
    pub fn home_config_path() -> Option<PathBuf> {
        directories::UserDirs::new().map(|d| d.home_dir().join(".oh-ben-claw").join("config.toml"))
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
        // `spine.tls` is a hard error, not a warning, and that needs saying out loud:
        // **MQTT-over-TLS has never been implemented.** `src/spine/mod.rs` builds
        // `MqttOptions` and never calls `set_transport`, so the key parsed, produced
        // advice about which port to use, and left the broker link in cleartext.
        //
        // A config key that silently fails to encrypt is the worst kind of dead key.
        // Every other one found in this codebase merely did nothing; this one told the
        // operator their traffic was protected. So it refuses to start rather than
        // warning — someone who set it made a security decision and is entitled to
        // find out that it did not take effect.
        //
        // The rustls stack was also dropped from the rumqttc dependency (see
        // Cargo.toml) because it carried four RUSTSEC advisories for a code path
        // nothing could reach. Implementing TLS means reinstating that feature on a
        // version whose certificate handling is not vulnerable, and wiring
        // `set_transport` — not just deleting this check.
        if self.spine.tls {
            anyhow::bail!(
                "spine.tls = true, but MQTT-over-TLS is not implemented — the \
                 connection would be cleartext. Set spine.tls = false and use a \
                 broker on a trusted network, or tunnel the link. This is a hard \
                 error rather than a warning because the alternative is believing \
                 you are encrypted when you are not."
            );
        }
        for (key, set) in [
            ("spine.ca_cert_path", self.spine.ca_cert_path.is_some()),
            (
                "spine.client_cert_path",
                self.spine.client_cert_path.is_some(),
            ),
            (
                "spine.client_key_path",
                self.spine.client_key_path.is_some(),
            ),
        ] {
            if set {
                warnings.push(format!(
                    "{key} is set but MQTT-over-TLS is not implemented; it has no effect"
                ));
            }
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
    // ── An absent [provider] is not a stated one ──────────────────────────────

    /// These three tests write `ANTHROPIC_API_KEY` and then delete it, and the
    /// environment is per-*process*, not per-test. Run concurrently, one can
    /// remove the variable between another's `set_var` and its assertion.
    ///
    /// That race was here from the day they were written and never fired: with
    /// 908 tests the scheduler happened not to overlap these three. On
    /// 2026-08-02 three new `doctor` tests that bind sockets and wait on
    /// connect timeouts changed the timing enough to overlap them, and the
    /// suite started failing about one run in three — in a test that had
    /// nothing to do with the change and passes in isolation on both branches.
    ///
    /// A lock rather than a rewrite: the function under test reads the process
    /// environment by design, which is the behaviour being tested, so the fix
    /// belongs at the point of contention. `PoisonError` is unwrapped away
    /// because a panic in one of these should fail that test, not cascade.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_anthropic_key<T>(body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
        let out = body();
        std::env::remove_var("ANTHROPIC_API_KEY");
        out
    }

    #[test]
    fn a_config_without_a_provider_table_takes_one_from_the_environment() {
        // Both reference bodies and every config the deployment generator emits omit
        // [provider] deliberately, so serde's `default` was handing them openai/gpt-4o
        // and then complaining about a missing OPENAI_API_KEY.
        let raw = "[agent]\nname = \"benchtop\"\n";
        let cfg: super::Config = toml::from_str(raw).unwrap();
        let cfg = with_anthropic_key(|| cfg.with_provider_from_env_if_absent(raw));
        assert_eq!(cfg.provider.name, "anthropic");
    }

    #[test]
    fn tuning_retry_does_not_count_as_choosing_a_provider() {
        // Found by running a reference body: `[provider.retry]` creates a `provider`
        // table, so a first version of this that looked for the table silently opted
        // the body out of resolution and handed it `openai` again. The question is
        // whether a provider was *named*.
        let raw = "[provider.retry]\nmax_retries = 2\n";
        let cfg: super::Config = toml::from_str(raw).unwrap();
        let cfg = with_anthropic_key(|| cfg.with_provider_from_env_if_absent(raw));
        assert_eq!(cfg.provider.name, "anthropic");
    }

    #[test]
    fn a_named_provider_is_never_overridden() {
        let raw = "[provider]\nname = \"ollama\"\nmodel = \"llama3.2\"\n";
        let cfg: super::Config = toml::from_str(raw).unwrap();
        let cfg = with_anthropic_key(|| cfg.with_provider_from_env_if_absent(raw));
        assert_eq!(cfg.provider.name, "ollama");
        assert_eq!(cfg.provider.model, "llama3.2");
    }

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
    fn validate_refuses_spine_tls_because_it_is_not_implemented() {
        // This test used to assert a *warning* recommending port 8883, which is how
        // the missing feature stayed hidden: the advice implied there was a TLS path
        // to configure. `src/spine/mod.rs` never calls `set_transport`, so the link
        // was cleartext either way. Refusing to start is the only honest answer to a
        // security key that cannot be honoured.
        let mut config = Config::default();
        config.spine.tls = true;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("not implemented"), "{err}");
        assert!(err.contains("cleartext"), "{err}");
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
    fn validate_warns_that_the_spine_cert_paths_have_no_effect() {
        // Previously the warning said the CA cert "will not be used *because tls is
        // false*" — true, and misleading, since it would not be used with tls true
        // either.
        let mut config = Config::default();
        config.spine.ca_cert_path = Some("/etc/mqtt/ca.crt".to_string());
        config.spine.client_key_path = Some("/etc/mqtt/client.key".to_string());
        let warnings = config.validate().unwrap();
        for key in ["ca_cert_path", "client_key_path"] {
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains(key) && w.contains("not implemented")),
                "no warning for {key}: {warnings:?}"
            );
        }
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
    fn root_config_has_proxy_field() {
        let config = Config::default();
        assert!(!config.proxy.enabled);
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
    fn root_config_has_browser_field() {
        let config = Config::default();
        assert!(config.browser.enabled);
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
        // No longer sets spine.tls: that is now a hard error, and this test is about
        // the file-existence check, which still runs and is still worth having for
        // whenever TLS is implemented.
        let mut config = Config::default();
        config.spine.port = 8883;
        config.spine.ca_cert_path = Some("/nonexistent/path/to/ca.crt".to_string());
        let warnings = config.validate().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("ca_cert_path") && w.contains("does not exist")),
            "{warnings:?}"
        );
    }

    #[test]
    fn default_config_still_validates_clean() {
        // Ensure all new validations don't break the default config.
        let config = Config::default();
        let warnings = config.validate().unwrap();
        assert!(warnings.is_empty(), "Unexpected warnings: {:?}", warnings);
    }
}

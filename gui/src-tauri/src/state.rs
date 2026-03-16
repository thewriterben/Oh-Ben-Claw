use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use oh_ben_claw::{
    agent::Agent,
    memory::MemoryStore,
    security::SecurityContext,
    config::{Config, ProviderConfig, SecurityConfig},
};

/// Serializable agent status sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusDto {
    pub running: bool,
    pub provider: String,
    pub model: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "toolCount")]
    pub tool_count: usize,
    #[serde(rename = "nodeCount")]
    pub node_count: usize,
    pub uptime: Option<u64>,
}

/// Serializable tool call log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntryDto {
    pub id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub args: String,
    pub result: Option<String>,
    pub status: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
    pub timestamp: u64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// Serializable peripheral node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralNodeDto {
    pub id: String,
    pub board: String,
    pub transport: String,
    pub status: String,
    pub tools: Vec<PeripheralToolDto>,
    #[serde(rename = "lastSeen")]
    pub last_seen: Option<u64>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralToolDto {
    pub name: String,
    pub description: String,
}

/// Serializable app settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettingsDto {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaUrl")]
    pub ollama_url: Option<String>,
    pub autostart: bool,
    #[serde(rename = "minimizeToTray")]
    pub minimize_to_tray: bool,
    #[serde(rename = "spineHost")]
    pub spine_host: String,
    #[serde(rename = "spinePort")]
    pub spine_port: u16,
    #[serde(rename = "requirePairing")]
    pub require_pairing: bool,
    #[serde(rename = "vaultEnabled")]
    pub vault_enabled: bool,
}

impl Default for AppSettingsDto {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            api_key: None,
            ollama_url: None,
            autostart: false,
            minimize_to_tray: true,
            spine_host: "localhost".into(),
            spine_port: 1883,
            require_pairing: false,
            vault_enabled: false,
        }
    }
}

/// Serializable chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageDto {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(rename = "toolName")]
    pub tool_name: Option<String>,
    #[serde(rename = "toolArgs")]
    pub tool_args: Option<String>,
    pub timestamp: u64,
}

/// Serializable session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDto {
    pub id: String,
    pub title: String,
    #[serde(rename = "messageCount")]
    pub message_count: usize,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

// ── App State ─────────────────────────────────────────────────────────────────

pub struct AgentHandle {
    pub agent: Arc<Agent>,
    pub provider: String,
    pub model: String,
    pub provider_config: ProviderConfig,
    pub session_id: String,
    pub started_at: std::time::Instant,
}

pub struct AppState {
    pub agent: Mutex<Option<AgentHandle>>,
    pub memory: Mutex<Option<Arc<MemoryStore>>>,
    pub security: Mutex<Option<SecurityContext>>,
    pub tool_log: Mutex<Vec<ToolCallEntryDto>>,
    pub nodes: Mutex<Vec<PeripheralNodeDto>>,
    pub settings: Mutex<AppSettingsDto>,
    pub vault_unlocked: Mutex<bool>,
    pub vault_secrets: Mutex<std::collections::HashMap<String, String>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            agent: Mutex::new(None),
            memory: Mutex::new(None),
            security: Mutex::new(None),
            tool_log: Mutex::new(Vec::new()),
            nodes: Mutex::new(Vec::new()),
            settings: Mutex::new(AppSettingsDto::default()),
            vault_unlocked: Mutex::new(false),
            vault_secrets: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

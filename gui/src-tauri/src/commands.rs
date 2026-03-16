use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use anyhow::Context;

use oh_ben_claw::{
    agent::Agent,
    config::{AgentConfig, Config, ProviderConfig, SecurityConfig},
    memory::MemoryStore,
    providers,
    security::SecurityContext,
    tools::builtin::{
        file::FileTool,
        http::HttpTool,
        memory::MemoryTool,
        shell::ShellTool,
    },
};

use crate::state::{
    AgentHandle, AgentStatusDto, AppSettingsDto, AppState,
    ChatMessageDto, PeripheralNodeDto, PeripheralToolDto, SessionDto, ToolCallEntryDto,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn uuid() -> String {
    // Simple UUID v4 without external crate
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    now_ms().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    format!("{:016x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        h.finish(),
        (h.finish() >> 16) & 0xffff,
        (h.finish() >> 8) & 0xfff,
        ((h.finish() >> 4) & 0x3fff) | 0x8000,
        h.finish() & 0xffffffffffff,
    )
}

// ── Agent Commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_agent(
    provider: String,
    model: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();

    // Build config from settings
    let config = Config {
        provider: ProviderConfig {
            name: provider.clone(),
            model: model.clone(),
            api_key: settings.api_key.clone(),
            base_url: settings.ollama_url.clone(),
            ..Default::default()
        },
        agent: AgentConfig {
            name: "Oh-Ben-Claw".into(),
            max_tool_iterations: 15,
            ..Default::default()
        },
        security: SecurityConfig {
            require_pairing: settings.require_pairing,
            vault_enabled: settings.vault_enabled,
            ..Default::default()
        },
        ..Default::default()
    };

    let provider_config = config.provider.clone();

    // Build provider
    let llm_provider = providers::build_provider(&config.provider)
        .map_err(|e| format!("Failed to build provider: {e}"))?;

    // Build memory store
    let memory = Arc::new(
        MemoryStore::open(None)
            .map_err(|e| format!("Failed to open memory store: {e}"))?,
    );

    // Build security context
    let security = SecurityContext::new(&config.security)
        .unwrap_or_else(|_| SecurityContext::new(&Default::default()).unwrap());

    // Build built-in tools
    let tools: Vec<Box<dyn oh_ben_claw::tools::traits::Tool>> = vec![
        Box::new(ShellTool::new()),
        Box::new(FileTool::new()),
        Box::new(HttpTool::new()),
        Box::new(MemoryTool::new()),
    ];

    // Build agent
    let session_id = uuid();
    let agent = Arc::new(
        Agent::new(config.agent.clone(), llm_provider, Arc::clone(&memory), tools)
            .with_policy(security.policy.clone()),
    );

    let mut agent_guard = state.agent.lock().await;
    *agent_guard = Some(AgentHandle {
        agent,
        provider: provider.clone(),
        model: model.clone(),
        provider_config,
        session_id,
        started_at: std::time::Instant::now(),
    });

    let mut mem_guard = state.memory.lock().await;
    *mem_guard = Some(memory);

    let mut sec_guard = state.security.lock().await;
    *sec_guard = Some(security);

    Ok(())
}

#[tauri::command]
pub async fn stop_agent(state: State<'_, AppState>) -> Result<(), String> {
    let mut agent_guard = state.agent.lock().await;
    *agent_guard = None;
    Ok(())
}

#[tauri::command]
pub async fn send_message(
    session_id: String,
    message: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<String, String> {
    // Extract the agent and provider config without holding the lock during processing.
    let (agent, provider_config) = {
        let guard = state.agent.lock().await;
        let handle = guard
            .as_ref()
            .ok_or("Agent is not running. Start it in Settings first.")?;
        (Arc::clone(&handle.agent), handle.provider_config.clone())
    };

    // Run the full agent loop.
    let response = agent
        .process(&session_id, &message, &provider_config)
        .await
        .map_err(|e| e.to_string())?;

    // ── Emit tool-call events ─────────────────────────────────────────────────
    for (i, record) in response.tool_calls.iter().enumerate() {
        let call_id = format!("{}-{}", session_id, i);
        let args_json: serde_json::Value =
            serde_json::from_str(&record.args).unwrap_or_else(|_| serde_json::json!({}));

        let is_error = record.result.starts_with("Tool error:")
            || record.result.starts_with("Tool execution failed:");

        let entry = ToolCallEntryDto {
            id: call_id.clone(),
            tool_name: record.name.clone(),
            args: args_json.to_string(),
            result: Some(record.result.clone()),
            status: if is_error { "error" } else { "success" }.into(),
            duration_ms: Some(record.duration_ms),
            timestamp: now_ms(),
            session_id: session_id.clone(),
        };

        // Persist to in-memory tool log (newest first).
        {
            let mut log = state.tool_log.lock().await;
            log.insert(0, entry.clone());
            if log.len() > 500 {
                log.truncate(500);
            }
        }

        // Emit to the frontend.
        let _ = app_handle.emit("tool-call-event", &entry);
    }

    // ── Stream assistant tokens word-by-word ──────────────────────────────────
    let content = response.message.clone();
    let app = app_handle.clone();
    tokio::spawn(async move {
        let words: Vec<&str> = content.split_inclusive(' ').collect();
        for word in words {
            let _ = app.emit("assistant-token", word.to_string());
            tokio::time::sleep(std::time::Duration::from_millis(18)).await;
        }
        // Sentinel: empty string signals stream completion.
        let _ = app.emit("assistant-token", String::new());
    });

    Ok(response.message)
}

#[tauri::command]
pub async fn get_agent_status(state: State<'_, AppState>) -> Result<Option<AgentStatusDto>, String> {
    let agent_guard = state.agent.lock().await;
    if let Some(handle) = agent_guard.as_ref() {
        Ok(Some(AgentStatusDto {
            running: true,
            provider: handle.provider.clone(),
            model: handle.model.clone(),
            session_id: handle.session_id.clone(),
            tool_count: 4, // built-in tools
            node_count: state.nodes.lock().await.len(),
            uptime: Some(handle.started_at.elapsed().as_secs()),
        }))
    } else {
        Ok(None)
    }
}

// ── Session Commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionDto>, String> {
    let mem_guard = state.memory.lock().await;
    if let Some(memory) = mem_guard.as_ref() {
        let sessions = memory.list_sessions().map_err(|e| e.to_string())?;
        Ok(sessions
            .into_iter()
            .map(|s| SessionDto {
                id: s.id,
                title: s.title,
                message_count: s.message_count,
                created_at: s.created_at,
            })
            .collect())
    } else {
        Ok(vec![SessionDto {
            id: "default".into(),
            title: "Default Session".into(),
            message_count: 0,
            created_at: now_ms(),
        }])
    }
}

#[tauri::command]
pub async fn create_session(
    title: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let mem_guard = state.memory.lock().await;
    if let Some(memory) = mem_guard.as_ref() {
        let id = uuid();
        memory
            .create_session(&id, title.as_deref().unwrap_or("New Session"))
            .map_err(|e| e.to_string())?;
        Ok(id)
    } else {
        Ok(uuid())
    }
}

#[tauri::command]
pub async fn load_session_history(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ChatMessageDto>, String> {
    let mem_guard = state.memory.lock().await;
    if let Some(memory) = mem_guard.as_ref() {
        let messages = memory
            .load_history(&session_id, 100)
            .map_err(|e| e.to_string())?;
        Ok(messages
            .into_iter()
            .map(|m| ChatMessageDto {
                id: uuid(),
                role: m.role,
                content: m.content,
                tool_name: None,
                tool_args: None,
                timestamp: m.timestamp,
            })
            .collect())
    } else {
        Ok(vec![])
    }
}

#[tauri::command]
pub async fn clear_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mem_guard = state.memory.lock().await;
    if let Some(memory) = mem_guard.as_ref() {
        memory.clear_session(&session_id).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mem_guard = state.memory.lock().await;
    if let Some(memory) = mem_guard.as_ref() {
        memory.delete_session(&session_id).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Peripheral Node Commands ──────────────────────────────────────────────────

#[tauri::command]
pub async fn list_nodes(state: State<'_, AppState>) -> Result<Vec<PeripheralNodeDto>, String> {
    Ok(state.nodes.lock().await.clone())
}

#[tauri::command]
pub async fn add_node(
    board: String,
    transport: String,
    path: Option<String>,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let node = PeripheralNodeDto {
        id: uuid(),
        board: board.clone(),
        transport: transport.clone(),
        status: "offline".into(),
        tools: vec![],
        last_seen: None,
        address: path,
    };
    state.nodes.lock().await.push(node.clone());
    let _ = app_handle.emit("node-status-change", &node);
    Ok(())
}

#[tauri::command]
pub async fn remove_node(
    node_id: String,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let mut nodes = state.nodes.lock().await;
    // Emit a removal event (status = "removed") for any matching node.
    if let Some(node) = nodes.iter().find(|n| n.id == node_id).cloned() {
        let removed = PeripheralNodeDto { status: "removed".into(), ..node };
        let _ = app_handle.emit("node-status-change", &removed);
    }
    nodes.retain(|n| n.id != node_id);
    Ok(())
}

#[tauri::command]
pub async fn scan_usb_devices(
    state: State<'_, AppState>,
) -> Result<Vec<PeripheralNodeDto>, String> {
    use oh_ben_claw::peripherals::registry::BoardRegistry;
    let registry = BoardRegistry::default();
    let mut found = Vec::new();

    // Scan /dev/ttyUSB* and /dev/ttyACM* on Linux
    for prefix in &["/dev/ttyUSB", "/dev/ttyACM"] {
        for i in 0..8 {
            let path = format!("{prefix}{i}");
            if std::path::Path::new(&path).exists() {
                let board_name = "unknown-usb-serial";
                let tools = registry
                    .capabilities_for(board_name)
                    .iter()
                    .map(|c| PeripheralToolDto {
                        name: c.to_string(),
                        description: format!("{c} capability"),
                    })
                    .collect();

                found.push(PeripheralNodeDto {
                    id: uuid(),
                    board: board_name.into(),
                    transport: "serial".into(),
                    status: "offline".into(),
                    tools,
                    last_seen: None,
                    address: Some(path),
                });
            }
        }
    }

    let mut nodes = state.nodes.lock().await;
    for node in &found {
        if !nodes.iter().any(|n| n.address == node.address) {
            nodes.push(node.clone());
        }
    }

    Ok(found)
}

// ── Tool Log Commands ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_tool_log(
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<ToolCallEntryDto>, String> {
    let log = state.tool_log.lock().await;
    Ok(log.iter().take(limit).cloned().collect())
}

#[tauri::command]
pub async fn clear_tool_log(state: State<'_, AppState>) -> Result<(), String> {
    state.tool_log.lock().await.clear();
    Ok(())
}

// ── Vault Commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_vault_status(state: State<'_, AppState>) -> Result<String, String> {
    let settings = state.settings.lock().await;
    if !settings.vault_enabled {
        return Ok("disabled".into());
    }
    let unlocked = state.vault_unlocked.lock().await;
    Ok(if *unlocked { "unlocked" } else { "locked" }.into())
}

#[tauri::command]
pub async fn unlock_vault(
    password: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // In a full implementation this would use the Vault from security::vault
    // For now we accept any non-empty password and mark as unlocked
    if password.is_empty() {
        return Err("Password cannot be empty".into());
    }
    *state.vault_unlocked.lock().await = true;
    Ok(())
}

#[tauri::command]
pub async fn lock_vault(state: State<'_, AppState>) -> Result<(), String> {
    *state.vault_unlocked.lock().await = false;
    state.vault_secrets.lock().await.clear();
    Ok(())
}

#[tauri::command]
pub async fn list_vault_secrets(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let unlocked = *state.vault_unlocked.lock().await;
    if !unlocked {
        return Err("Vault is locked".into());
    }
    let secrets = state.vault_secrets.lock().await;
    Ok(secrets.keys().cloned().collect())
}

#[tauri::command]
pub async fn set_vault_secret(
    name: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let unlocked = *state.vault_unlocked.lock().await;
    if !unlocked {
        return Err("Vault is locked".into());
    }
    state.vault_secrets.lock().await.insert(name, value);
    Ok(())
}

#[tauri::command]
pub async fn delete_vault_secret(
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let unlocked = *state.vault_unlocked.lock().await;
    if !unlocked {
        return Err("Vault is locked".into());
    }
    state.vault_secrets.lock().await.remove(&name);
    Ok(())
}

// ── Settings Commands ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettingsDto, String> {
    Ok(state.settings.lock().await.clone())
}

#[tauri::command]
pub async fn save_settings(
    settings: AppSettingsDto,
    state: State<'_, AppState>,
) -> Result<(), String> {
    *state.settings.lock().await = settings;
    Ok(())
}

# Oh-Ben-Claw Roadmap

This document tracks the development phases of Oh-Ben-Claw. Status indicators:
- **✅ Complete** — fully implemented and tested
- **🔄 In Progress** — framework in place, implementation ongoing
- **📋 Planned** — not yet started

---

## Phase 1: Foundation ✅ Complete

The initial release establishes the core architecture and demonstrates the key concepts of the system. It includes the MQTT Spine design, the peripheral tool registry, the hardware board registry, and the ESP32-S3 and NanoPi Neo3 peripheral drivers.

- [x] Repository structure and Cargo workspace
- [x] MQTT Spine protocol design (`src/spine/mod.rs`)
- [x] Hardware board registry with USB VID/PID mappings (`src/peripherals/registry.rs`)
- [x] NanoPi Neo3 GPIO peripheral driver (`src/peripherals/nanopi.rs`)
- [x] ESP32-S3 sensor tools (camera, audio, sensor read) (`src/peripherals/sensors.rs`)
- [x] ESP32-S3 firmware with serial + MQTT support (`firmware/obc-esp32-s3`)
- [x] Configuration schema with MQTT Spine and multi-board support (`src/config/mod.rs`)
- [x] CLI with `start`, `status`, `peripheral`, and `service` subcommands (`src/main.rs`)
- [x] Architecture documentation (`docs/architecture/ARCHITECTURE.md`)
- [x] Hardware datasheets (`docs/datasheets/`)
- [x] CI/CD pipeline (`.github/workflows/ci.yml`)

---

## Phase 2: Core Agent Loop ✅ Complete

Full agent loop with LLM integration, memory, and tool execution. **35 unit tests.**

- [x] LLM provider adapters: OpenAI, Anthropic, Ollama, OpenRouter, OpenAI-compatible (`src/providers/`)
- [x] Agent loop with tool-use iterations, max-iteration guard, and history compaction (`src/agent/mod.rs`)
- [x] SQLite WAL-mode memory backend with session management (`src/memory/mod.rs`)
- [x] Built-in tools: `shell`, `file`, `http`, `memory` (`src/tools/builtin/`)
- [x] Interactive CLI channel with `/help`, `/tools`, `/clear`, `/quit` commands (`src/channels/`)
- [x] `rumqttc`-based MQTT Spine client with dynamic tool discovery (`src/spine/mod.rs`)

---

## Phase 3: Security Subsystem ✅ Complete

Tool execution policies, peripheral node pairing, and encrypted secrets vault. **60 unit tests (+25).**

- [x] Rename: Bus layer → **Spine** layer (all identifiers, configs, docs)
- [x] Tool policy engine — glob pattern matching, arg_contains, allow/deny/audit actions (`src/security/policy.rs`)
- [x] Node pairing — HMAC-SHA256 tokens, 5-minute replay window, quarantine status (`src/security/pairing.rs`)
- [x] Encrypted secrets vault — AES-256-GCM, Argon2id KDF, SQLite backend (`src/security/vault.rs`)
- [x] `SecurityContext` wired into agent loop and startup (`src/main.rs`)
- [x] `[security]` config section with policy examples (`examples/config-multi-device.toml`)

---

## Phase 4: Native Desktop GUI ✅ Complete

Tauri 2 native desktop application — system tray, chat, device panel, tool log, vault UI.

- [x] Tauri 2 + React 18 + TypeScript + TailwindCSS scaffold (`gui/`)
- [x] Custom dark theme with Oh-Ben-Claw brand palette (`gui/src/styles/globals.css`)
- [x] **Chat panel** — multi-session, streaming-ready, tool-call bubbles (`gui/src/components/ChatPanel.tsx`)
- [x] **Devices panel** — node cards with status, tool list, USB scan, add/remove (`gui/src/components/NodesPanel.tsx`)
- [x] **Tool Log panel** — filterable call history with args/result expansion (`gui/src/components/ToolLogPanel.tsx`)
- [x] **Vault panel** — unlock/lock, add/delete secrets, AES-256-GCM display (`gui/src/components/VaultPanel.tsx`)
- [x] **Settings panel** — provider/model, Spine config, security toggles, agent start/stop (`gui/src/components/SettingsPanel.tsx`)
- [x] Tauri backend commands — agent bridge, session management, node registry, vault ops (`gui/src-tauri/src/commands.rs`)
- [x] System tray — left-click to show, right-click menu (Show / Quit) (`gui/src-tauri/src/lib.rs`)
- [x] Minimize-to-tray on window close (`gui/src-tauri/src/lib.rs`)
- [x] Launch-at-login via `tauri-plugin-autostart` (`gui/src-tauri/src/lib.rs`)
- [x] Tauri event streaming — `assistant-token`, `tool-call-event`, `node-status-change` events
- [x] GUI CI job in `.github/workflows/ci.yml`

---

## Phase 5: Expanded Hardware Ecosystem ✅ Complete

All hardware peripheral drivers and Linux bus tools are implemented.

- [x] Raspberry Pi GPIO peripheral driver (`src/peripherals/rpi.rs`)
- [x] Raspberry Pi camera support (via `libcamera-still`) (`src/peripherals/rpi.rs`)
- [x] Arduino serial peripheral driver (`src/peripherals/arduino.rs`)
- [x] STM32 Nucleo peripheral driver (via probe-rs) (`src/peripherals/stm32.rs`)
- [x] I2C bus scan/read/write tools for Linux SBCs (`src/peripherals/bus_tools.rs`)
- [x] SPI bus transfer tool for Linux SBCs (`src/peripherals/bus_tools.rs`)
- [x] PWM control tool for Linux SBCs (`src/peripherals/bus_tools.rs`)

### Phase 5.5: Audit Enhancements ✅ Complete

Board registry expansion, security hardening, and config validation from project audit.

- [x] Board registry: ESP32-C3, nRF52840 DK, Arduino Nano 33 BLE, Teensy 4.1, BeagleBone Black, NVIDIA Jetson Nano, STM32H7 Discovery
- [x] New capability tokens: `ble`, `wifi`, `can`, `dac`, `cuda`
- [x] I2C/SPI accessory registry with 15 known sensors/modules (BME280, BMP388, SHT31, AHT20, MPU6050, LSM6DS3, ADS1115, MCP4725, PCF8574, MCP23017, MAX31855, DS18B20, INA260, SSD1306)
- [x] Accessory lookup functions: by name, I2C address, and capability
- [x] Security: fix Mutex `unwrap()` panics with poisoned-lock recovery (`src/security/pairing.rs`)
- [x] Security: pairing secret strength validation (`NodePairingManager::validate_secret`)
- [x] Security: glob matching ReDoS mitigation with recursion depth limit (`src/security/policy.rs`)
- [x] Config: TLS certificate fields for MQTT spine (CA cert, client cert, client key)
- [x] Config: `Config::validate()` method with comprehensive checks and warnings
- [x] Sensor tools: expanded to 11 supported sensors (added BMP388, LSM6DS3, AHT20, INA260, ADS1115, MAX31855, DS18B20)

---

## Phase 6: Multi-Channel Support ✅ Complete

Add support for all major communication channels.

- [x] Telegram channel — long-polling Bot API adapter (`src/channels/telegram.rs`)
- [x] Discord channel — Gateway WebSocket adapter (`src/channels/discord.rs`)
- [x] Slack channel — Socket Mode WebSocket adapter (`src/channels/slack.rs`)
- [x] WhatsApp channel — Meta Business Cloud API webhook adapter (`src/channels/whatsapp.rs`)
- [x] iMessage channel (macOS only) — AppleScript + Messages.app SQLite polling adapter (`src/channels/imessage.rs`)
- [x] Matrix channel — Client-Server API long-poll adapter (`src/channels/matrix.rs`)

---

## Phase 7: Edge-Native Mode ✅ Complete

Enable peripheral nodes to run the full Oh-Ben-Claw agent locally, without a host.

- [x] Lightweight agent loop for ESP32-S3 (WiFi + cloud LLM) (`firmware/obc-esp32-s3/src/main.rs`)
- [x] Lightweight agent loop for NanoPi Neo3 (local Ollama) (`src/agent/edge.rs`)
- [x] Peer-to-peer node coordination (without a central broker) (`src/spine/p2p.rs`)

---

## Phase 8: Advanced Capabilities ✅ Complete

- [x] Vision pipeline (camera capture → LLM vision → action) (`src/vision/mod.rs`)
- [x] Audio pipeline (microphone → speech-to-text → agent → text-to-speech) (`src/audio/mod.rs`)
- [x] Sensor fusion (combine readings from multiple sensors) (`src/peripherals/fusion.rs`)
- [x] Scheduled tasks and cron jobs (`src/scheduler/mod.rs`)
- [x] Terminal telemetry dashboard — real-time TUI with agent status, nodes, tool log, system metrics (`src/dashboard/mod.rs`)
- [x] Skill forge (automatic discovery and integration of new skills) (`src/skill_forge/mod.rs`)

## Phase 9: ZeroClaw Parity ✅ Complete

Implements key features from the upstream ZeroClaw project to ensure Oh-Ben-Claw is as robust and advanced.

- [x] Human-in-the-loop approval workflow for supervised mode (`src/approval/mod.rs`)
- [x] Token cost tracking and budget enforcement (`src/cost/`)
- [x] System diagnostics CLI command (`oh-ben-claw doctor`) (`src/doctor/mod.rs`)
- [x] Event lifecycle hooks for extensibility (`src/hooks/`)
- [x] Enhanced multimodal message handling with image markers (`src/multimodal.rs`)
- [x] RAG pipeline for hardware datasheet retrieval (`src/rag/mod.rs`)
- [x] Sandboxed tool execution runtime (native + Docker) (`src/runtime/`)
- [x] New config sections: `[autonomy]`, `[cost]`, `[runtime]`, `[multimodal]`

---

## Phase 10: OpenClaw Parity ✅ Complete

Analyses the [OpenClaw](https://github.com/openclaw/openclaw) project and brings
the most valuable features into Oh-Ben-Claw.

### Model Reliability (inspired by OpenClaw's model-failover system)

- [x] **Model failover** — chain ordered fallback providers/models in config (`[[provider.fallbacks]]`); if the primary call fails the next entry is tried automatically (`src/providers/failover.rs`)
- [x] **Retry policy** — transparent exponential-backoff retries for transient errors (rate-limits, network blips) via `[provider.retry]` config section (`src/providers/retry.rs`)
- [x] `from_config_full()` factory wires failover + retry together, applied at startup (`src/providers/mod.rs`)

### New Communication Channels (inspired by OpenClaw's multi-channel inbox)

- [x] **IRC channel** — raw-TCP IRC adapter; supports SASL PLAIN auth, channel joins, CTCP, and nick-collision recovery (`src/channels/irc.rs`)
- [x] **Signal channel** — Signal Messenger via the [signal-cli](https://github.com/AsamK/signal-cli) JSON-RPC HTTP daemon; with sender allowlist (`src/channels/signal.rs`)
- [x] **Mattermost channel** — Mattermost WebSocket event API; uses personal access token (`src/channels/mattermost.rs`)

### UX Improvements (inspired by OpenClaw's typing indicators)

- [x] **Typing indicators** — `TypingTask` helper spawns a background task that refreshes the platform's "typing…" status while the agent processes; auto-cancelled on response (`src/channels/typing.rs`)
- [x] Telegram typing via `sendChatAction` (refresh every 4 s) (`src/channels/telegram.rs`)
- [x] Discord typing via `POST /channels/{id}/typing` (refresh every 8 s) (`src/channels/discord.rs`)
- [x] Slack typing via `conversations.typing` (refresh every 4 s) (`src/channels/slack.rs`)

### Configuration

- [x] `ProviderConfig` gains `fallbacks: Vec<ProviderConfig>` and `retry: Option<RetryConfig>` fields
- [x] `ChannelsConfig` gains `irc`, `signal`, `mattermost`, and `typing_indicators` fields
- [x] `IrcConfig`, `SignalConfig`, `MattermostConfig` structs added to `src/config/mod.rs`
- [x] Example configuration updated with all new sections (`examples/config-multi-device.toml`)

---

## Phase 11: Pycoclaw + MimiClaw Parity ✅ Complete

Analyses [PycoClaw](https://github.com/jetpax/pycoclaw) (MicroPython AI agents on
ESP32-S3/P4) and [MimiClaw](https://github.com/memovai/mimiclaw) (pure-C AI agent
on ESP32-S3) and brings the most valuable features into Oh-Ben-Claw.

### Personality System (inspired by MimiClaw's SOUL.md / USER.md)

MimiClaw stores the agent's personality and user profile as editable plain-text
Markdown files rather than hardcoding them in a config file.  Oh-Ben-Claw adopts
the same pattern.

- [x] **`PersonalityStore`** — reads `SOUL.md` (agent personality) and `USER.md` (user profile) from `~/.oh-ben-claw/`; either file overrides the `system_prompt` in `config.toml` (`src/memory/personality.rs`)
- [x] `build_system_prompt()` helper merges SOUL.md + USER.md into a single system prompt, with a fallback to the config value when the file is absent

### Proactive Task System (inspired by MimiClaw's HEARTBEAT.md)

MimiClaw periodically checks a Markdown task file and triggers the agent when
uncompleted items are found.  Oh-Ben-Claw adopts the same pattern on top of the
existing `Scheduler`.

- [x] **`HeartbeatStore`** — reads `~/.oh-ben-claw/HEARTBEAT.md`, detects uncompleted tasks (skips headers, empty lines, and `- [x]` completed checkboxes), and generates a prompt for the agent (`src/memory/heartbeat.rs`)
- [x] `append_task()` convenience method appends a new `- [ ] …` line to the file
- [x] `build_prompt()` returns the heartbeat prompt when actionable tasks exist

### Daily Journal (inspired by MimiClaw's memory_append_today)

MimiClaw writes per-day Markdown notes as `YYYY-MM-DD.md` files to complement
its SQLite conversation history.  Oh-Ben-Claw adopts the same pattern.

- [x] **`DailyJournal`** — appends timestamped notes to `~/.oh-ben-claw/journal/YYYY-MM-DD.md`; creates file with date header on first write (`src/memory/journal.rs`)
- [x] `read_recent(days)` reads the last N days of notes, sections separated by `---`
- [x] `list_dates()` returns all journal dates in descending order

### HTTP Proxy Support (inspired by MimiClaw's proxy system)

MimiClaw supports HTTP CONNECT tunnels for devices behind corporate firewalls.
Oh-Ben-Claw now exposes the same feature through its TOML configuration.

- [x] **`ProxyConfig`** — `[proxy]` TOML section with `host`, `port`, `kind` (`http`/`socks5`), and optional credentials (`src/config/mod.rs`)
- [x] `ProxyConfig::url()` builds the proxy URL string
- [x] `ProxyConfig::apply_to_env()` sets `HTTP_PROXY` / `HTTPS_PROXY` for all downstream HTTP clients
- [x] `Config::validate()` extended to reject proxy configs missing `host` or `port`

### Feishu/Lark Channel (inspired by MimiClaw's Feishu integration)

MimiClaw is the first OpenClaw-compatible project to support Feishu (Lark outside
China), a popular enterprise messaging platform. Oh-Ben-Claw now includes a
webhook-based Feishu channel adapter.

- [x] **`FeishuChannel`** — Axum webhook server; receives `im.message.receive_v1` events, forwards text to the agent, sends reply via Feishu REST API (`src/channels/feishu.rs`)
- [x] Tenant access token refresh with in-memory cache (auto-refreshes 60 s before expiry)
- [x] Optional `verification_token` signature check on every inbound webhook
- [x] URL verification handshake (Feishu challenge/response)
- [x] Message chunking — long replies are split into ≤ 4 000-character segments
- [x] **`FeishuConfig`** struct added to `src/config/mod.rs`; wired into `ChannelsConfig`

### Configuration

- [x] `ChannelsConfig` gains `feishu: FeishuConfig` field
- [x] `Config` gains `proxy: ProxyConfig` and `personality: PersonalityConfig` fields
- [x] `FeishuConfig`, `ProxyConfig`, `PersonalityConfig` structs in `src/config/mod.rs`

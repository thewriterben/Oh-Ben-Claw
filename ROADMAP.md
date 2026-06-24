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

---

## Phase 12: OpenClaw 3.13 Parity ✅ Complete

Analyses the [OpenClaw 3.13](https://github.com/openclaw/openclaw/releases/tag/v2026.3.13)
release (March 2026) and brings the most impactful new features into Oh-Ben-Claw.

### Browser Automation (inspired by OpenClaw 3.13's stable CDP browser layer)

OpenClaw 3.13 delivered a major browser automation overhaul: Chrome DevTools
Protocol (CDP) attach mode, batched DOM actions, flexible CSS/XPath selector
targeting, and human-like delayed-click support.  Oh-Ben-Claw adopts the same
architecture.

- [x] **`BrowserSession`** — manages a CDP connection to a headless or user Chrome/Chromium instance; supports `"headless"` (default) and `"user"` profiles; falls back to HTTP fetch when no CDP endpoint is reachable (`src/tools/builtin/browser.rs`)
- [x] **`BrowserNavigateTool`** (`browser_navigate`) — navigate to a URL with optional post-navigation delay; validates URL scheme; returns page title
- [x] **`BrowserSnapshotTool`** (`browser_snapshot`) — capture stripped-HTML text snapshot of the active page; configurable `max_chars`; suitable for feeding page content to the LLM
- [x] **`BrowserClickTool`** (`browser_click`) — click an element matched by CSS selector; optional human-like `delay_ms` before the click
- [x] **`BrowserTypeTool`** (`browser_type`) — type text into a focused element or a selector-identified input; optional `submit` (Enter) and keystroke `delay_ms`
- [x] **`BrowserScrollTool`** (`browser_scroll`) — scroll the page up / down / to top / to bottom or directly to a CSS selector; configurable `amount_px`
- [x] **`BrowserNewTabTool`** (`browser_new_tab`) — open a new browser tab, optionally navigating to a URL immediately; tab ID tracked in session state
- [x] **`BrowserCloseTabTool`** (`browser_close_tab`) — close the active tab; session state updated to reflect the next available tab
- [x] `all_browser_tools()` convenience constructor shares a single `BrowserSession` across all seven tools; CDP URL configurable via `OBC_BROWSER_CDP_URL` env var
- [x] HTML helpers: `extract_title()` (no-dependency `<title>` extractor) and `strip_html()` (script/style-aware tag stripper with 8 000-char limit)

### ClawHub Skill Registry (inspired by OpenClaw's community skill marketplace)

OpenClaw popularised the concept of a public skill registry ("ClawHub") where
the community shares pre-built automation scripts.  Oh-Ben-Claw now has a
first-class client for this registry.

- [x] **`ClawHubEntry`** — typed representation of a registry entry: name, version, description, author, download count, star rating, tags, verified status, and manifest URL (`src/skill_forge/registry.rs`)
- [x] **`SkillRegistryIndex`** — locally cached index with `search(query)` (name + description + tags), `find(name)`, `len()`, and `is_empty()` helpers
- [x] **`ClawHubClient`** — async HTTP client for a ClawHub REST API (`GET /api/v1/skills`, `GET /api/v1/skills/{name}`, `GET /api/v1/skills/{name}/{version}/manifest`); local index cache avoids redundant network round-trips; `install()` downloads and writes a `.skill.json` to the configured skills directory
- [x] `pub mod registry` added to `src/skill_forge/mod.rs`

### Image Memory (inspired by OpenClaw 3.13's multimodal image memory)

OpenClaw 3.13 introduced persistent image memory so agents can store and
retrieve visual context across sessions.  Oh-Ben-Claw now provides the same
capability via a SQLite-backed store.

- [x] **`ImageEntry`** — stored image with UUID, MIME type, base64 data, description, tags, session ID, timestamp, and file name; `decode_bytes()`, `estimated_bytes()`, and `has_any_tag()` helpers (`src/memory/image.rs`)
- [x] **`ImageMemoryStore`** — SQLite WAL-mode store with `store()`, `get()`, `delete()`, `search()` (case-insensitive LIKE on description + tags), `list_by_session()`, and `count()` operations; `open_in_memory()` for tests
- [x] `pub mod image` + `pub use image::ImageMemoryStore` added to `src/memory/mod.rs`
- [x] Pre-existing `src/memory/vector.rs` `Tool::execute` return-type bug fixed (`ToolResult` → `anyhow::Result<ToolResult>`) so the vector module can be compiled and exported (`pub mod vector` added to `src/memory/mod.rs`)

### Browser Automation in `default_tools()`

- [x] `default_tools()` now calls `all_browser_tools()` and extends the default tool set with all seven browser tools; CDP URL read from `OBC_BROWSER_CDP_URL` env var at startup (`src/tools/mod.rs`)
- [x] `src/tools/builtin/mod.rs` gains `pub mod browser` declaration

### Configuration

- [x] **`BrowserConfig`** — `[browser]` TOML section: `enabled`, `cdp_url`, `profile` (`"headless"` / `"user"`), `timeout_secs` (`src/config/mod.rs`)
- [x] **`ClawHubConfig`** — `[clawhub]` TOML section: `enabled`, `registry_url`, `auto_update`, `skills_dir` (`src/config/mod.rs`)
- [x] `Config` gains `browser: BrowserConfig` and `clawhub: ClawHubConfig` fields

---

## Phase 13: Hardware-Driven Deployment Scheme Generator ✅ Complete

Implements a comprehensive multi-agent swarm system to create custom deployment
schemes based on available hardware and desired features.  The system analyses
a `HardwareInventory`, maps capabilities to roles, generates a full agent
topology, identifies hardware gaps, and renders a ready-to-use TOML
configuration.

### New Hardware (Board Registry)

Three new boards and two new accessories are added to the registry:

- [x] **Waveshare ESP32-S3-Touch-LCD-2.1** — 2.1" round capacitive touch display with integrated I2S speaker; capability tokens: `display`, `touch`, `audio_sample`, `wifi`, `ble` (`src/peripherals/registry.rs`)
- [x] **Seeed XIAO ESP32S3-Sense** — compact ESP32-S3 module with OV2640 camera and PDM microphone; capability tokens: `camera_capture`, `audio_sample`, `wifi`, `ble`, `sensor_read` (`src/peripherals/registry.rs`)
- [x] **Sipeed 6+1 Mic Array** — USB far-field 6+1 MEMS microphone array (STM32F103 MCU, UAC1 audio class); capability tokens: `audio_sample` (`src/peripherals/registry.rs`)
- [x] **DHT22** accessory — single-wire GPIO temperature & humidity sensor; added to `KNOWN_ACCESSORIES` with `bus = "gpio"` and `compatible_boards` listing (`src/peripherals/registry.rs`)
- [x] **DHT11** accessory — basic single-wire temperature & humidity sensor (`src/peripherals/registry.rs`)
- [x] New capability tokens documented: `display` (integrated display output), `touch` (capacitive/resistive touch input)

### Deployment Subsystem (`src/deployment/`)

- [x] **`HardwareInventory`** — describes boards and accessories available for a deployment, their operator-assigned roles, and the feature desires the operator wants to fulfil (`src/deployment/inventory.rs`)
- [x] **`HardwareItem`** — single board/accessory entry with capability resolution from the registry; resolves board + accessory capabilities at query time (`src/deployment/inventory.rs`)
- [x] **`ItemRole`** — enum: `Host`, `Display`, `Vision`, `Listening`, `Sensing`, `Peripheral`, `Unassigned` (`src/deployment/inventory.rs`)
- [x] **`FeatureDesire`** — enum of high-level features: `Vision`, `Listening`, `Speech`, `EnvironmentalSensing`, `DisplayOutput`, `TouchInput`, `EdgeInference`, `WirelessMesh`, `PersistentMemory`, `Custom` (`src/deployment/inventory.rs`)
- [x] `HardwareInventory::nanopi_scenario()` — pre-built reference scenario for the NanoPi-Neo3 + ESP32 deployment (`src/deployment/inventory.rs`)
- [x] **`HardwareAdvisor`** — gap analyser that checks which feature desires are satisfied, identifies missing capabilities, and suggests specific boards from the registry (`src/deployment/advisor.rs`)
- [x] `HardwareAdvisor::analyse()`, `suggest_missing()`, `compatibility_report()`, `validate()` (`src/deployment/advisor.rs`)
- [x] **`DeploymentScheme`** — output type: agent assignments, hardware suggestions, warnings, TOML config snippet, and human-readable report (`src/deployment/scheme.rs`)
- [x] **`AgentAssignment`** — describes a single sub-agent: name, `NodeRole`, hardware item, tools, TOML snippet (`src/deployment/scheme.rs`)
- [x] **`NodeRole`** — enum: `Orchestrator`, `VisionAgent`, `AudioAgent`, `SpeechDisplayAgent`, `SensingAgent`, `PeripheralAgent` (`src/deployment/scheme.rs`)
- [x] **`DeploymentPlanner`** — deterministic rule-based planner that maps hardware to agent topology and renders full TOML config (no LLM required) (`src/deployment/planner.rs`)
- [x] **`DeploymentSwarm`** — LLM-powered multi-agent swarm with three specialised sub-agents: `hardware-advisor`, `architect`, `requirements-checker`; wraps `DeploymentPlanner` output with LLM refinement (`src/deployment/swarm.rs`)
- [x] `DeploymentSwarm::plan_static()` for offline/test use; `DeploymentSwarm::plan()` for full LLM-enhanced planning (`src/deployment/swarm.rs`)
- [x] `pub mod deployment` registered in `src/lib.rs`

### Configuration

- [x] **`DeploymentConfig`** — `[deployment]` TOML section: `enabled`, `scenario`, `auto_plan`, `auto_spawn`, `feature_desires`, `hardware`, `llm_swarm` (`src/config/mod.rs`)
- [x] **`DeploymentHardwareConfig`** — `[[deployment.hardware]]` entries: `name`, `board_name`, `transport`, `path`, `node_id`, `role`, `accessories` (`src/config/mod.rs`)
- [x] `Config` gains `deployment: DeploymentConfig` field

### Example

- [x] **`examples/config-nanopi-deployment.toml`** — complete reference configuration for the NanoPi-Neo3 scenario with all five hardware items, four pre-spawned sub-agents, full orchestrator config, and deployment scheme section

---

## Phase 14: Cutting-Edge Capabilities ✅ Complete

Analyses cutting-edge developments in the AI agent ecosystem and pushes
Oh-Ben-Claw beyond parity with all related projects. Implements
production-grade completions of stubbed features, new protocol support,
and enhanced reliability.

### Peripheral Spine Integration (resolves Phase 1 TODOs)

- [x] **Sensor tool spine communication** — `CameraCaptureTool`, `AudioSampleTool`, and `SensorReadTool` now accept an optional `Arc<SpineClient>` and route commands through the MQTT spine when available; falls back to stub mode for standalone testing (`src/peripherals/sensors.rs`)
- [x] `with_spine()` builder method on all three sensor tools

### Persistent Cost Tracking (completes Phase 9 cost subsystem)

- [x] **SQLite-backed cost persistence** — `CostTracker::with_db(config, path)` opens a WAL-mode SQLite database (`~/.oh-ben-claw/costs.db`) and records all usage events persistently (`src/cost/tracker.rs`)
- [x] Daily and monthly budget enforcement now works correctly across sessions
- [x] `session_summary()` returns accurate daily and monthly costs from the database

### Enhanced Multimodal Support (completes Phase 9 multimodal)

- [x] **Image source resolution** — `resolve_image_source()` distinguishes local file paths from remote URLs (`src/multimodal.rs`)
- [x] **MIME type validation** — `validate_mime_type()` checks against the `ALLOWED_IMAGE_MIME_TYPES` whitelist
- [x] **Image size validation** — `validate_image_size()` enforces configurable byte-size limits
- [x] **Local image fetching** — `fetch_local_image()` reads, validates, and base64-encodes local image files
- [x] **Batch image preparation** — `prepare_images()` resolves, fetches, and validates multiple image references with count limits

### Mattermost Thread Support (completes Phase 10 channel)

- [x] **Thread replies** — Mattermost adapter now tracks `root_id` and replies in-thread; new messages start a thread, follow-ups continue it (`src/channels/mattermost.rs`)
- [x] Updated `MmPost` and `MmCreatePost` structs with `root_id` field

### WASM Sandbox Runtime (new runtime adapter)

- [x] **`WasmRuntime`** — new runtime adapter for WebAssembly sandboxed execution with configurable memory pages, execution fuel, and WASI directory access (`src/runtime/wasm.rs`)
- [x] **`WasmConfig`** added to `RuntimeConfig` with `enabled`, `max_memory_pages`, `max_fuel`, `allowed_dirs` fields
- [x] Framework-ready for wasmtime integration when the dependency is added

### Structured Output / JSON Mode (new provider capability)

- [x] **`ResponseFormat` enum** — `Text`, `JsonObject`, and `JsonSchema { name, schema, strict }` variants with full serde support (`src/providers/mod.rs`)
- [x] `response_format` field added to `ProviderConfig` for per-provider defaults
- [x] OpenAI, OpenRouter, and Compatible providers emit native `response_format` in API bodies
- [x] Anthropic provider emulates JSON mode via system prompt annotation
- [x] Ollama provider uses native `format` field

### Streaming Tool Calls (new agent capability)

- [x] **`StreamingToolCallAccumulator`** — collects partial tool-call deltas from streaming LLM responses and assembles them into complete `ToolCall` objects (`src/agent/streaming.rs`)
- [x] **`StreamingResponseBuilder`** — incrementally builds a `StreamingResponse` from interleaved text and tool-call chunks
- [x] Integrates with existing `ToolCall` type from `crate::providers`

### A2A Protocol Support (new interoperability layer)

- [x] **Agent-to-Agent (A2A) protocol** — implementation of Google's open protocol for inter-agent communication (`src/a2a/mod.rs`)
- [x] Core types: `AgentCard`, `A2ASkill`, `TaskRequest`, `TaskResponse`, `TaskStatus`, `Artifact`
- [x] **`A2AClient`** — async HTTP client with `discover()`, `send_task()`, `get_task_status()` methods
- [x] **`A2AServer`** — handles discovery and task requests for exposing Oh-Ben-Claw as an A2A endpoint
- [x] **`A2AConfig`** added to root `Config` with `enabled`, `agent_name`, `agent_description`, `agent_url`, `skills` fields

### Enhanced Configuration Validation

- [x] Port range validation (0 detection) for tunnel, proxy, webhook, IRC, and P2P ports
- [x] P2P `node_id` format validation (alphanumeric + hyphens)
- [x] Channel token format validation (Telegram, Discord, Slack)
- [x] MQTT username↔password pairing check
- [x] Provider model requirement check
- [x] TLS certificate file existence warnings

### Test Results

- [x] **630 unit tests** passing (76 new tests added)
- [x] **14 doc-tests** passing
- [x] All Clippy warnings resolved
- [x] All code formatted with `rustfmt`

## Phase 15: Production Hardening 🔄 Planned

Executed in lockstep with **ClawCam Phase 13** (see `NEXT_PHASE_PLAN.md` in the
workspace root). No new product surface area: this phase makes the existing 14
phases trustworthy — supply-chain security, protocol conformance against the
specs as they actually shipped, and the evaluation/observability layer the 2026
agentic-AI ecosystem treats as table stakes. Target window: June 8 – July 31, 2026.

### Skill-Install Security (ClawHub client) — time-sensitive

Driven by the 2026 ClawHub supply-chain compromise (~1 in 12 registry skills
malicious; `SKILL.md` external-URL payloads defeat static scanning).

- [x] Operator approval required for every skill install/update (no silent installs) — `InstallConsent` gate in `ClawHubClient::install()`
- [x] Checksum verification of skill packages (SHA-256 vs. catalogue `sha256`; `require_checksum` mode) — full signature verification deferred until the registry publishes signing keys
- [x] Version pinning in config (`[clawhub.install_policy.pinned_versions]`)
- [x] Static flagging of external-URL fetch instructions, shell execution, and download language, surfaced in the approval prompt (`InstallInspection`)
- [x] Optional local vetted mirror (`allowlist`)
- [x] Install audit log (JSONL, manifest hash + decision + flags) — verified: 17 unit tests, 55/55 module tests passing on Windows

### MCP 2026-07-28 Readiness — deadline July 28, 2026

The MCP release candidate is breaking: stateless protocol core (no init
handshake / session header), extensions framework, Tasks primitive.

- [ ] Audit MCP client + server against the 2026-07-28 RC
- [ ] Dual-mode operation (current spec + RC) behind a config flag
- [ ] Cross-repo integration test with ClawCam (brain ↔ adapter ↔ stdio bridge ↔ gateway) in both modes
- [ ] Flip default mode when the final spec ships (July 28)

### A2A v1.0 Conformance

Phase 14's A2A implementation predates the stable v1.0 spec (Linux Foundation).

- [x] Diff `src/a2a/` against published v1.0 — finding: the Phase 14 sketch matched neither v0.3.0 nor v1.0 (custom REST `/tasks`, snake_case states, pre-0.3 `agent.json`)
- [x] Rewrite to v1.0 (JSON-RPC binding subset): `supportedInterfaces`, `agent-card.json`, PascalCase operations, `TASK_STATE_*`/`ROLE_*` enums, kind-less `Part` oneof, A2A error codes with `ErrorInfo`, `A2A-Version` validation — supported subset documented in module docs
- [x] A2A conformance test suite — 18 unit tests covering card shape, enum wire formats, Part oneof, task lifecycle, error codes, version gate (cargo run pending on Windows)

### Evaluation Harness (CC/CD)

- [x] Golden task set for the agent loop (`tests/evals.rs`): direct answer, single-tool route with exact args, multi-step ordering, tool-failure recovery, unknown-tool degradation — driven by a deterministic `ScriptedProvider` mock
- [x] Wire-shape goldens: MCP (initialize/discover/tools-list/error) and A2A (task lifecycle, ErrorInfo, agent card) + approval policy matrix golden
- [x] CI gate: evals run as integration tests under the existing `cargo test --workspace` job — no release while evals regress (cargo run pending on Windows)
- [ ] LLM-as-judge advisory scoring (deferred until a judge provider is wired; gates stay deterministic)

### Observability / AgentOps

- [x] Audit finding: `src/observability` (spans, ring-buffer sink, counters, `/api/v1/metrics`) already existed and was wired into the gateway — but the agent loop was blind
- [x] Structured trace per agent run: `Agent::with_obs()` records an `agent.process` span (session_id, tool_calls) and per-call `agent.tool` spans with error status; turn/tool/error counters at source (no double-count with gateway counters) — 2 new evals in `tests/evals.rs`
- [x] Counters for approval asks (`ApprovalManager::with_obs()` → `approval_asks_total`); `record_retry`/`record_failover` helpers added — retry/failover already emit structured `tracing::warn!` logs; counter threading into the provider wrappers deferred until the gateway owns an ObsContext for them
- [ ] Cost summary in the gateway metrics view (CostTracker handle not yet in `GatewayState`; carry to Phase 16)

### Approval-Model Upgrade

- [x] Approval scopes: call / session / forever; forever grants persisted (`~/.oh-ben-claw/approval_grants.json`) with audit trail (16 unit tests; cargo run pending on Windows)
- [x] Plan-mode approval: `ApprovedPlan` + `ArgumentBound`s; plan revoked on first violation (halt on drift)
- [x] Shared scope vocabulary with ClawCam: adapter `ApprovalGrants` + `call_tool(scope=…)` — verified, 22/22 tests
- [x] Approval funnel analytics per tool (asked / approved-by-scope / denied / plan violations) in both projects

---

# v2.0 — "Embodied Frontier"

The next major version. Strategy and rationale: `docs/V2-STRATEGY.md`. Thesis:
take the frontier agent capabilities of 2026 — experiential self-improvement,
long-horizon autonomy, dual-system perception-action, real-time multimodal
interaction, and physical-action safety — and realize each one **natively for an
embodied multi-device fleet**, the position no pure-software agent can occupy.

Sequencing: lead with **Phase 16 + Track 0** (highest demand × highest leverage,
and the safety floor that makes shipping autonomy responsible). Phases 17–18 build
the autonomy and architectural substrate; 19–20 are the user-facing payoff. Track 0
runs underneath all of it.

## Track 0: Physical-Action Safety & Trust 📋 Planned *(cross-cutting)*

Physical actions are irreversible and have a real-world blast radius, so the safety
bar sits far above software-only agents. This track lands incrementally alongside
every phase below. Aligns Oh-Ben-Claw with the OWASP Top 10 for Agentic Applications
(Dec 2025) and the NIST AI Agent Standards direction (Feb 2026).

- [ ] **Physical-risk classification** for every tool — `reversible`/`irreversible` × `low`/`high` blast radius; drives approval defaults (e.g., actuator/lock/relay tools default to per-call approval)
- [ ] **Deterministic, model-independent safety limits** at the actuator boundary — rate limits, value ranges, and interlocks enforced in code the LLM cannot override (`src/security/` + `src/peripherals/`)
- [ ] **Pre-action authorization** at the tool-call boundary with cryptographically signed audit records for every physical action (extends `src/approval/` + `src/observability/`)
- [ ] **Staged rollout** for new/synthesized physical skills: `simulate` → `supervised` → `autonomous`, promotion gated on a clean record
- [ ] **Physical-aware approval prompts** — surface risk class, device, and concrete effect ("open GPIO 17 → unlock front door") in the approval UI
- [ ] Embodied red-team evals: injected-malicious-skill and injected-prompt tests must not be able to drive an out-of-limit actuator command (extends Phase 15 eval harness)

## Phase 16: Experiential Self-Improvement 📋 Planned *(flagship / near-term)*

The agent learns reusable, verified skills from its own successful task
trajectories — not just authored or ClawHub-installed skills. Builds directly on
`skill_forge`, `memory`, and the agent loop. The single highest-demand frontier
capability (the engine behind Hermes Agent's adoption), grounded so reflection is
anchored to real verification rather than the model's own say-so.

- [ ] **Trajectory capture** — record agent runs (objective, tool calls, args, results, outcome) as structured episodes (`src/memory/`)
- [ ] **Reflection + skill synthesis** — distil a successful trajectory into a named, parameterized, reusable skill (`src/skill_forge/`)
- [ ] **Self-verification gate** — a synthesized skill must pass concrete verification (test execution and/or sensor/camera confirmation) before it is trusted; never trust intrinsic self-report alone
- [ ] **Learned-skill library + retrieval** — store synthesized skills in the existing local skill library (ClawHub-compatible format); retrieve relevant skills before reasoning from scratch
- [ ] **Offline trace evolution** — GEPA/DSPy-style reflective optimization of prompts and skill descriptions from accumulated execution traces (batch/scheduled job)
- [ ] **Safety interlock** — any synthesized skill that invokes a physical/actuator tool is registered through Track 0 (risk class + staged rollout) before it may run unattended
- [ ] Metrics: learned-skill reuse rate; token/latency reduction on repeated routine tasks; zero unsafe auto-runs of synthesized actuator skills

## Phase 17: Long-Horizon Embodied Autonomy Harness 📋 Planned

Durable, resumable, self-verifying operation across hours/days and across crashes,
reboots, and context limits — the Anthropic initializer+worker harness pattern,
adapted so the externalized "progress file" is the **physical world state**. Builds
on `scheduler`, `heartbeat`, `agent`, `memory`, and `runtime`. Depends on Track 0.

- [ ] **Durable execution** — checkpoint agent/task state to persistent storage; resume cleanly after crash/reboot without re-running completed physical actions (model "non-persistable regions" around side-effecting tool calls)
- [ ] **Initializer + worker split** — initializer establishes environment and an externalized world-state/objective record; worker advances one objective at a time
- [ ] **Externalized world-state progress record** — structured (JSON) objective list with per-objective status, resilient to context compaction
- [ ] **Mandatory self-verification before "done"** — confirm each objective via sensors/cameras/tests, not assertion; re-open objectives that fail verification on resume
- [ ] **Resume smoke test** — on restart, re-establish context cheaply (current device states, outstanding objectives) before acting
- [ ] Long-horizon eval: an unattended fleet completes a defined multi-hour routine across an induced crash/reboot with correct resume and no duplicated physical actions

## Phase 18: Dual-System Perception-Action + World Memory 📋 Planned

Adopt the architecture every 2026 robotics stack converged on — slow reasoner +
fast reflex — backed by a persistent, temporally-aware model of the physical
environment. The most architecturally novel, most embodied-native phase. Builds on
`agent/edge`, `vision`, `peripherals/fusion`, and `memory`.

- [x] **System 1 (fast reflex loop)** — host-side `ReflexEngine` (`src/agent/reflex.rs`): conditions (sensor/gpio/and/or), actions (gpio_write/publish/escalate), debounce + rate limit, serde wire format for pushing to nodes; 8 tests. (On-MCU mirror of the evaluator: firmware follow-up.)
- [ ] **System 2 (slow reasoner)** — cloud/host LLM invoked for planning and novelty, not every event; System 1 escalates to System 2 on uncertainty
- [x] **Bitemporal world memory** — persistent, queryable model of rooms/devices/states over time with validity intervals (valid time + transaction time), so stale facts are invalidated rather than lost; `src/memory/world.rs` (`observe`/`current`/`at`/`history`/`entities`), non-destructive, 5 tests. (Full as-of-transaction-time queries: follow-up.)
- [x] **Perception→memory→action wiring** — sensor-fusion outputs update world memory (`FusionRegistry::observe_into` → `sensor.{quantity}` facts); planning queries via the `world_memory` tool. (Vision→world-memory lands with the ClawCam vision suite.)
- [ ] **Escalation policy + budget** — when System 1 hands off to System 2, with cost/latency guards
- [ ] Eval: System 1 reflex latency budget met offline; System 2 invoked only on novelty; world-memory queries return temporally-correct device state

## Phase 19: Real-Time Multimodal Interaction 📋 Planned

A streaming bidirectional voice + vision session that turns existing nodes — the
ESP32-S3 mic/speaker, the Waveshare ESP32-S3 Touch-LCD already in the board
registry — into ambient conversational agents. Builds on `channels`, `audio`,
`multimodal`. Pairs with Phase 20 for offline graceful degradation.

- [ ] **Realtime session channel** — bidirectional streaming voice via OpenAI Realtime API (`gpt-realtime`) and/or Gemini Live API (`src/channels/`)
- [ ] **Continuous vision input** — stream camera frames into the live session for "see and hear the room" interaction
- [ ] **Barge-in / interruption handling** — user can interrupt; agent yields and re-plans
- [ ] **Mid-conversation tool calls** — invoke fleet tools during a live spoken exchange (gated by Track 0 for physical actions)
- [ ] **Reference node profile** — documented wiring + config for an ESP32-S3 + mic/speaker (and the touch-LCD) ambient device
- [ ] Eval: end-to-end spoken-interaction latency budget met on the reference node

## Phase 20: Edge-Native Intelligence 📋 Planned

Make on-device inference first-class so the fleet is private, low-latency, and
resilient offline — deepening the embodied moat. Builds on `agent/edge`,
`providers`. Enables Phase 18's System 1 and Phase 19's graceful degradation.

- [ ] **Small-model reflex tier** — first-class support for small local models powering System 1 (llama.cpp/Ollama on Pi/Jetson; TinyML on MCUs where viable)
- [ ] **On-device wake-word + STT/TTS** — local audio front-end so devices respond without round-tripping to the cloud
- [ ] **Policy-driven cloud fallback** — deterministic rules for escalating local → cloud (capability, confidence, connectivity), honoring cost and privacy config
- [ ] **Edge model management** — provision/update local models per node role from the deployment planner
- [ ] Eval: defined reflex tasks complete fully offline; correct, audited fallback to cloud when required

## Hardware Ecosystem Expansion 📋 Planned *(standing track)*

Maximize supported hardware breadth — ESP32 boards & ESP32-based electronics,
sensors, microcontrollers/SBCs, **AI accelerators**, displays, radios, actuators,
accessories, and connector ecosystems — and keep it current via an automated
weekly scout. Design + vendor target list + intake rubric: `docs/V2-HARDWARE-ECOSYSTEM.md`.
Most additions are host-side registry metadata (`src/peripherals/registry.rs`); only
new transports/drivers/accelerator-inference touch firmware.

### Registry model upgrades

- [ ] **Connector ecosystem field** — `Connector` enum (Grove, Qwiic, STEMMA QT, STEMMA, M-Bus, FeatherWing, Pmod, Pi HAT, Bare) on `BoardInfo` + `AccessoryInfo`; advisor matches accessories to boards by connector (Qwiic ≡ STEMMA QT equivalence)
- [ ] **`vendor` + `ecosystem` fields** on `BoardInfo` (e.g. Seeed/XIAO, Adafruit/Feather, M5Stack/M5)
- [ ] **AI-accelerator capability tokens** — `npu`, `edge_tpu`, `hailo`, `vpu`, `kpu`, `tensor_rt`, `nn_accel` (the highest-value additions)
- [ ] **Radio/connectivity tokens** — `lora`/`lorawan`, `zigbee`/`thread`/`matter`, `subghz`, `nfc`, `gps`, `ethernet`, `cellular`
- [ ] **I/O/form tokens** — `epaper`, `rgb_led`/`neopixel`, `microsd`, `rtc`, `battery`/`pmic`, `motor_driver`, `relay`, `imu`, `microphone`, `speaker`; actuator-class tokens (`relay`, `motor_driver`) auto-tag a physical `RiskClass` (Track 0)
- [ ] Connector-aware `HardwareAdvisor`/`DeploymentPlanner`; new `FeatureDesire`s: `LongRangeRadio`, `Localization`, `Actuation`

### Vendor coverage (≥1 entry each, then expand the long tail)

- [ ] **Espressif** — ESP32 / -S2 / -S3 / -C3 / -C6 / -H2 / -P4 devkits, ESP-EYE, ESP32-S3-BOX
- [ ] **Seeed Studio** — XIAO (C3/C6/S3/RP2350), Grove sensor family, Grove Vision AI v2, reComputer Jetson + Hailo-8L
- [ ] **M5Stack** — Core2/CoreS3, StickC PLUS2, AtomS3, M5 Units, LLM Module (AX630C)
- [ ] **LILYGO** — T-Display-S3, T-Watch-S3, T-Beam (LoRa+GPS), T-Deck, T-Echo
- [ ] **Adafruit** — Feather ESP32-S3, QT Py, RP2040/RP2350, STEMMA QT sensor catalog
- [ ] **SparkFun** — Thing Plus, Qwiic sensor catalog, MicroMod
- [ ] **Raspberry Pi** — Pico 2 / 2 W, RPi 5 + AI HAT+ (Hailo), AI Camera (IMX500), Sense HAT
- [ ] **Pimoroni / DFRobot** — Pico bases, Enviro, Inky e-paper; FireBeetle ESP32, Gravity line
- [ ] **Radxa/Rockchip** — ROCK 5 (RK3588, `npu`)
- [ ] **NVIDIA Jetson** — Orin Nano/NX, AGX Thor (`cuda` + `tensor_rt`)
- [ ] **Hailo / Google Coral** — Hailo-8/8L/10 (M.2/USB); Coral USB/M.2/Dev Board (`edge_tpu`)
- [ ] **Sipeed / Kendryte** — Maix (K210), MaixCAM (K230, `kpu`)
- [ ] **Arduino / Waveshare / Tindie long-tail** — Nano ESP32, Nicla Vision/Voice, Portenta; Waveshare ESP32-S3 displays & LoRa HATs; niche Tindie boards adding new capability

### Continuous intake (recurring)

- [x] **Weekly hardware scout** — scheduled task `obc-hardware-scout` (Mondays 09:00 local) scans the vendor list, proposes ranked additions with ready-to-paste registry entries + a "needs verification" list, writes a dated report to `Knowledge Base/hardware-scout-YYYY-MM-DD.md`; does not edit `registry.rs` directly
- [ ] Triage + merge rubric applied (new capability / new ecosystem / popularity > clone/out-of-scope); merge requires verified VID/PID (or explicit bridge note) + a passing registry test
- [ ] Per addition: registry entry with connector/vendor/ecosystem, valid/new capability tokens, a resolve test, and a linked firmware item if a new transport/driver/accelerator is implied

### Firmware (only where an addition needs on-device execution)

- [ ] New transport adapters as needed (CAN/RS-485 nodes, Ethernet SBCs, radio gateways)
- [ ] New on-device sensor/peripheral drivers (reuse existing `sensor_read` dispatch where the bus exists)
- [ ] AI-accelerator nodes (Coral/Hailo/Jetson/K230) run as edge-inference nodes via `EdgeAgent`, advertising their accelerator token and exposing local inference as a spine tool

## Ecosystem Integration 📋 Planned *(standing track)*

Unify the three sibling projects — **Oh-Ben-Claw** (Rust runtime/planner/firmware, canonical), the **OBC-deployment-generator** (Expo iOS/Android/web UX front door), and **Accelerapp** (Python multi-platform codegen) — into one pipeline with a single source of truth, ending the three-way drift of registry/planner/firmware. Design + phased plan: `docs/ECOSYSTEM-INTEGRATION.md`.

### I1 — Registry as single source of truth *(highest-value, do first)*

- [ ] Derive `Serialize`/`Deserialize` on `BoardInfo`/`AccessoryInfo`/`Connector`; add a `registry.json` exporter (cargo test / xtask / `--emit-registry`)
- [ ] Generator imports `registry.json` and drops the hand-written `KNOWN_BOARDS` in `lib/obc-data.ts` (gains VID/PID, connectors, vendor, all 44 boards)
- [ ] Accelerapp consumes the same `registry.json`; weekly scout → regenerate → all consumers update
- [ ] CI check fails if `obc-data.ts` contains a hand-written board list; add `schema_version`

### I2 — Deployment planner parity

- [ ] Shared `inventory.json → expected.toml` golden fixtures run in both Rust (`tests/`) and TS (Vitest) suites
- [ ] Align generator `transport` union to Rust (`serial|native|probe|bridge`; drop `mqtt`) and emit the real `[deployment]`/`[[deployment.hardware]]` schema (paste-ready into the runtime)

### I3/I4 — Live-ops bridge (generator ↔ OBC gateway)

- [ ] Generator backend (`server/`) proxies OBC gateway (`/api/v1/nodes|metrics|status`) — phone/web fleet console, read-only first (no-op fallback when offline)
- [ ] Operate mode (auth-gated): push generated scheme/config to a running OBC, approve Track 0 physical actions remotely, run tools, manage scheduler
- [ ] App board catalog refreshes `registry.json` from the live gateway
- [ ] Security: read-only by default + explicit elevation; remote actions flow through Track 0 signed-action audit

### I5/I6 — Retire remaining duplication

- [ ] Compile `src/deployment` to a `wasm-bindgen` npm package; generator drops its TS planner (guaranteed parity, still offline)
- [ ] Shared firmware template set consumed by generator + Accelerapp; align `FirmwareConfig` with real `firmware/obc-esp32-s3` + v2.0 firmware (SafetyGate/reflex); route multi-platform targets to Accelerapp codegen

## Embodied Subsystem Suites 📋 Planned *(standing track)*

Generalize ClawCam's "capability suite plugged into the brain" pattern into a reusable **Subsystem Suite** contract, and instantiate it for **Vision (ClawCam)**, **Sensing**, and **Movement** — each a complete system that perceives/acts, **remembers, learns, improves, and accelerates**, sharing one world memory and one safety layer. Design: `docs/SUBSYSTEM-SUITES.md`. Architectural decision: suites stay separate repos bound by the contract (MCP tools + spine + world memory + Track 0), not merged.

### The Subsystem Suite contract
- [ ] Formalize the 8-point contract (perceive/act · connect · remember · learn · improve · accelerate · stay-safe · observe) as a shared standard in OBC + suite repos

### Vision Suite (ClawCam) — the reference implementation
- [ ] **S0 Consolidate:** fix ClawCam tool-catalog drift (stale 5-tool JSON, 16-vs-32 HTTP listing, lagging docs); commit cross-repo `NEXT_PHASE_PLAN.md`; finish Phase 13↔15 lockstep (plan-mode approval w/ arg bounds; wire `tests/evals` into CI)
- [ ] **S1 Remember (→ Phase 18):** implement ClawCam's documented review-state model; add re-identification/embeddings; feed subjects as entities into OBC bitemporal world memory (valid-time intervals, non-destructive corrections)
- [ ] **S2 Learn (→ Phase 16):** active-learning loop — low-confidence/novel detections → review queue → human/brain correction (ground-truth signal) → improve thresholds/heads + synthesize skills
- [ ] **S3 Accelerate (→ Phase 20/18):** register real models (MegaDetector/BirdNET weights, SpeciesNet) + accelerator detectors (Hailo/Coral/Jetson; ESP-DL/LiteRT-Micro on ESP32-S3-EYE); dual-system fast-trigger/slow-inference split
- [ ] **S4 Safe:** assign `RiskClass` to every world-changing vision tool; route through Track 0 signed audit + staged rollout; use ClawCam's cron engine as a Phase 17 autonomy-loop instance

### Sensing Suite
- [ ] Instantiate the contract: read sensors/fusion → world memory as time-valid facts; on-node reflex rules (Phase 18 System 1); learned thresholds; edge-accelerated fusion

### Movement Suite *(highest-risk — Track 0 must be mature first)*
- [ ] Instantiate the contract for actuators (GPIO/relay/motor/servo/pan-tilt): deterministic on-MCU safety limits, physical risk class, staged rollout, signed audit
- [ ] Closed-loop composition: Vision detects → world memory → brain/reflex commands Movement to track (vision-driven actuation); learned movement skills gated by Track 0

---

> **Beyond v2.0 (watch list, not scope):** integrating an on-device VLA (GR00T /
> Gemini Robotics On-Device / π-class) as a *peripheral capability* once a node has
> a real manipulator; world-model-generated synthetic scenarios (Cosmos/Genie/Marble
> style) for offline skill rehearsal. Explicit non-goals: building our own
> motor-control VLA, and broadening into a general software-agent runtime — both
> forfeit the embodied moat.

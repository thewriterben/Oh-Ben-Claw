# Changelog

All notable changes to Oh-Ben-Claw are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## Unreleased — Phase 14 Cutting-Edge Capabilities (2026-04-11)

### Added

- **A2A Protocol** — Google's Agent-to-Agent interoperability protocol; `AgentCard`, `A2ASkill`, `TaskRequest`, `TaskResponse`, `TaskStatus` types; async `A2AClient` (discover, send_task, get_task_status) and `A2AServer` (handle_discover, handle_task) (`src/a2a/mod.rs`)
- **Structured Output** — `ResponseFormat` enum (`Text`, `JsonObject`, `JsonSchema`) with native support in OpenAI, OpenRouter, Compatible, Ollama providers; Anthropic emulation via system prompt (`src/providers/mod.rs`)
- **Streaming Tool Calls** — `StreamingToolCallAccumulator` and `StreamingResponseBuilder` for incremental tool call assembly from streaming LLM responses (`src/agent/streaming.rs`)
- **WASM Sandbox Runtime** — `WasmRuntime` adapter with configurable memory pages, execution fuel, and WASI directory access; `WasmConfig` in `RuntimeConfig` (`src/runtime/wasm.rs`)
- **Persistent Cost Tracking** — `CostTracker::with_db()` opens SQLite WAL-mode database for cross-session daily/monthly budget enforcement (`src/cost/tracker.rs`)
- **Multimodal Image Pipeline** — `ImageSource`, `ImageData` types; `resolve_image_source()`, `validate_mime_type()`, `validate_image_size()`, `fetch_local_image()`, `prepare_images()` functions (`src/multimodal.rs`)
- **Mattermost Thread Replies** — `root_id` tracking in `MmPost`/`MmCreatePost`; automatic thread continuation (`src/channels/mattermost.rs`)
- **Sensor Spine Communication** — `CameraCaptureTool`, `AudioSampleTool`, `SensorReadTool` now route commands through MQTT spine via optional `SpineClient`; `with_spine()` builders (`src/peripherals/sensors.rs`)

### Improved

- **Configuration Validation** — 16 new validation checks: port range, P2P node_id format, channel token format (Telegram, Discord, Slack), MQTT credential pairing, provider model requirement, TLS certificate file existence (`src/config/mod.rs`)
- **`A2AConfig`** added to root `Config` with `enabled`, `agent_name`, `agent_description`, `agent_url`, `skills` fields
- **`WasmConfig`** added to `RuntimeConfig` with `enabled`, `max_memory_pages`, `max_fuel`, `allowed_dirs` fields
- **`response_format`** field added to `ProviderConfig` for per-provider structured output defaults

### Test Results

- **630 unit tests** passing (+76 new), **14 doc-tests** passing
- All Clippy warnings resolved
- All code formatted with `rustfmt`

---

## [Unreleased] — 2026-03-22

### Fixed — Audit: CI Build & Clippy

This release resolves all 25 clippy errors that were blocking the CI pipeline,
applies `rustfmt` formatting to all source files, and addresses security audit
advisories in transitive dependencies.

#### Clippy Fixes

- **`src/lib.rs`** — removed duplicate `#![allow(dead_code)]` attribute
  (`clippy::duplicated_attributes`)
- **`src/agent/reflexion.rs`** — removed unnecessary `mut` on `config` binding
  (`unused_mut`); replaced `splitn(2, ':').nth(1)` with `split_once(':')`
  (`clippy::manual_split_once`)
- **`src/audio/mod.rs`** — changed three `&PathBuf` parameters to `&Path` in
  `record_alsa`, `record_sox`, and `record_ffmpeg`; added `use std::path::Path`
  (`clippy::ptr_arg`)
- **`src/tools/builtin/audio.rs`** — removed unnecessary `mut` on `cmd_args`
  (`unused_mut`); changed ten `&format!(...)` arguments to `format!(...)`
  (`clippy::needless_borrows_for_generic_args`); changed four `&PathBuf`
  parameters (`transcribe_openai`, `transcribe_local`) to `&Path`
  (`clippy::ptr_arg`); added `use std::path::Path`
- **`src/tools/builtin/ota.rs`** — changed two `&format!(...)` arguments to
  `format!(...)` (`clippy::needless_borrows_for_generic_args`)
- **`src/config/mod.rs`** — replaced manual `Default` impl for `IMessageConfig`
  with `#[derive(Default)]` (`clippy::derivable_impls`)
- **`src/dashboard/mod.rs`** — removed unnecessary `as u64` cast on
  `stat.f_frsize` which is already `u64` (`clippy::unnecessary_cast`)
- **`src/peripherals/fusion.rs`** — replaced `sorted.len() % 2 == 0` with
  `sorted.len().is_multiple_of(2)` (`clippy::manual_is_multiple_of`)
- **`src/hooks/runner.rs`** — replaced `sort_by(|a, b| b.priority().cmp(&a.priority()))`
  with `sort_by_key(|h| Reverse(h.priority()))` (`clippy::unnecessary_sort_by`);
  added `use std::cmp::Reverse`
- **`src/rag/mod.rs`** — replaced `board.map_or(true, |b| ...)` with
  `board.is_none_or(|b| ...)` (`clippy::unnecessary_map_or`)

#### Formatting

- Applied `cargo fmt --all` to all Rust source files including
  `firmware/obc-esp32-s3/src/main.rs` and multiple `src/` modules

#### Dependency Updates

- **`ratatui`** upgraded from `0.29` → `0.30` — resolves
  `RUSTSEC-2024-0436` (`paste` unmaintained, now removed) and
  `RUSTSEC-2026-0002` (`lru 0.12.5` unsound iterator, now `lru 0.16.3`)
- Added **`.cargo/audit.toml`** to acknowledge `RUSTSEC-2025-0134`
  (`rustls-pemfile 2.2.0` unmaintained via `rumqttc 0.24`) with tracking note;
  no exploitable vulnerability — purely a maintenance classification

#### Documentation

- **`README.md`** — full rewrite: added table of contents, Phases 12 & 13
  features (browser automation, ClawHub, image memory, deployment scheme
  generator), new hardware (Seeed XIAO ESP32S3-Sense, Sipeed 6+1 mic array,
  DHT22/DHT11), quick-start section, full CLI reference, updated project
  structure tree, comprehensive feature-comparison table vs ZeroClaw
- **`docs/architecture/ARCHITECTURE.md`** — full rewrite: added deployment
  subsystem section, security model details (vault, pairing, policy engine),
  P2P mesh section, updated component diagram and relationship table (removed
  stale "planned" entries for GUI and pairing that are now implemented)
- **`CHANGELOG.md`** — added this Phase 13 + audit entry (previously missing)
- **`CONTRIBUTING.md`** — improved development setup, added `pnpm` note for
  GUI, deployment and firmware cross-compile sections
- **`SECURITY.md`** — expanded with Docker runtime sandbox, tool policy engine,
  and security audit advisory details

### Test Results

```
test result: ok. 554 passed; 0 failed; 0 ignored; 0 measured
```

554 unit tests pass. Doc-tests: 12 passed, 0 failed, 2 ignored.

---

## [Unreleased] — 2026-03-20

### Added — Phase 13: Hardware-Driven Deployment Scheme Generator

Three new boards and two new accessories are added to the peripheral registry
(`src/peripherals/registry.rs`): **Waveshare ESP32-S3-Touch-LCD-2.1**
(display, touch, audio), **Seeed XIAO ESP32S3-Sense** (camera, audio, WiFi,
BLE), **Sipeed 6+1 Mic Array** (far-field USB audio), **DHT22**, and **DHT11**.
New capability tokens: `display`, `touch`.

A new `src/deployment/` module implements:

- **`HardwareInventory`** / **`HardwareItem`** / **`ItemRole`** / **`FeatureDesire`** —
  structured description of available hardware and desired features
- **`HardwareAdvisor`** — gap analysis: checks which features are satisfied,
  identifies missing capabilities, suggests boards from the registry
- **`DeploymentScheme`** / **`AgentAssignment`** / **`NodeRole`** — output types
  describing the generated agent topology and TOML config snippet
- **`DeploymentPlanner`** — deterministic rule-based planner (no LLM required)
  that maps hardware to agent roles and renders a complete TOML configuration
- **`DeploymentSwarm`** — optional LLM-powered multi-agent swarm (three
  sub-agents: hardware-advisor, architect, requirements-checker)
- `pub mod deployment` registered in `src/lib.rs`

Configuration: **`DeploymentConfig`** (`[deployment]`) and
**`DeploymentHardwareConfig`** (`[[deployment.hardware]]`) added to
`src/config/mod.rs`; `Config` gains `deployment: DeploymentConfig`.

Example: **`examples/config-nanopi-deployment.toml`** — complete reference
configuration for the NanoPi Neo3 + 4-device scenario.

### Added — Phase 12: OpenClaw 3.13 Parity

Research date: 2026-03-20.  This phase analyses OpenClaw v2026.3.13 (the
"browser automation & image memory" release) and the wider OpenClaw ecosystem
to bring Oh-Ben-Claw to parity with the upstream project.

#### Browser Automation (`src/tools/builtin/browser.rs`)

- **`BrowserSession`** — manages a Chrome DevTools Protocol (CDP) connection;
  supports `"headless"` (default) and `"user"` profiles; falls back to plain
  HTTP fetch when no CDP endpoint is reachable.  Thread-safe via
  `Arc<Mutex<SessionState>>`.
- **`BrowserNavigateTool`** (`browser_navigate`) — navigate to a URL with
  optional `wait_ms` post-load delay; validates the URL scheme; returns the
  page title.
- **`BrowserSnapshotTool`** (`browser_snapshot`) — capture a stripped-HTML
  text snapshot of the current page (scripts and styles removed); configurable
  `max_chars` up to 8 000.
- **`BrowserClickTool`** (`browser_click`) — click a CSS-selector-identified
  element; optional `delay_ms` for human-like timing.
- **`BrowserTypeTool`** (`browser_type`) — type text into the focused element
  or a selector-targeted input; optional `submit` flag (presses Enter) and
  per-keystroke `delay_ms`.
- **`BrowserScrollTool`** (`browser_scroll`) — scroll up / down / to top /
  to bottom by `amount_px`, or directly to an element by CSS selector.
- **`BrowserNewTabTool`** (`browser_new_tab`) — open a new browser tab,
  optionally navigating to a URL immediately.
- **`BrowserCloseTabTool`** (`browser_close_tab`) — close the active tab;
  session switches to the previous open tab.
- `all_browser_tools(cdp_url)` — builds all seven browser tools sharing a
  single `BrowserSession`.
- HTML helpers: `extract_title` (no-dependency `<title>` extractor) and
  `strip_html` (script/style-aware tag stripper).

#### ClawHub Skill Registry (`src/skill_forge/registry.rs`)

- **`ClawHubEntry`** — typed representation of a community skill: name,
  version, description, author, download count, star rating, tags, verified
  status, and manifest URL.
- **`SkillRegistryIndex`** — in-process cache with `search(query)` (matches
  name, description, and tags), `find(name)`, `len()`, and `is_empty()`.
- **`ClawHubClient`** — async HTTP client for a ClawHub registry API;
  populates the local index on first search; `install()` downloads and writes
  a `.skill.json` manifest to the configured skills directory.

#### Image Memory (`src/memory/image.rs`)

- **`ImageEntry`** — stored image with UUID, MIME type, base64-encoded data,
  description, tags, session ID, Unix timestamp, and original file name.
  Helpers: `decode_bytes()`, `estimated_bytes()`, `has_any_tag()`.
- **`ImageMemoryStore`** — SQLite WAL-mode store (`image_memory` table) with
  `store()`, `get()`, `delete()`, `search()` (case-insensitive on description
  + tags), `list_by_session()`, and `count()` operations.

#### Configuration (`src/config/mod.rs`)

- **`BrowserConfig`** — `[browser]` TOML section with `enabled`,
  `cdp_url`, `profile`, and `timeout_secs`.
- **`ClawHubConfig`** — `[clawhub]` TOML section with `enabled`,
  `registry_url`, `auto_update`, and `skills_dir`.
- `Config` gains `browser: BrowserConfig` and `clawhub: ClawHubConfig` fields.

### Changed

- **`src/tools/builtin/mod.rs`** — added `pub mod browser`.
- **`src/tools/mod.rs`** — `default_tools()` now registers all seven browser
  tools (CDP URL from `OBC_BROWSER_CDP_URL` env var); re-exports all browser
  tool types.
- **`src/memory/mod.rs`** — added `pub mod image`, `pub mod vector`, and
  corresponding `pub use` re-exports.
- **`src/skill_forge/mod.rs`** — added `pub mod registry`.

### Fixed

- **`src/memory/vector.rs`** — `VectorSearchTool::execute` and
  `DocumentIngestTool::execute` now return `anyhow::Result<ToolResult>` as
  required by the `Tool` trait (pre-existing type mismatch now resolved by
  the addition of `pub mod vector` to `memory/mod.rs`).

### Test Results

```
test result: ok. 503 passed; 0 failed; 0 ignored; 0 measured
```

503 unit tests pass (+65 new tests from Phase 12).
Doc-tests: 11 passed, 0 failed, 2 ignored.

---

## [Unreleased] — 2026-03-15

### Added — Upgrade Set A: Multimodal LLM Capabilities

- **`src/providers/streaming.rs`** — Streaming LLM response support via
  `StreamingProvider` trait and `StreamChunk` type; enables token-by-token
  output for real-time UI feedback.
- **`src/tools/builtin/vision.rs`** — Three new vision/multimodal tools:
  - `VisionTool` — encodes local files and remote URLs (JPEG, PNG, WebP, GIF,
    BMP) to base64 and queries GPT-5.4 / Claude Opus 4.6 vision APIs.
  - `AudioTranscriptionTool` — transcribes audio via the OpenAI Whisper API
    with optional word-level timestamps.
  - `StructuredOutputTool` — forces JSON-schema-constrained output using the
    OpenAI `response_format: json_schema` feature.
- **`src/tools/builtin/audio.rs`** — Two production-ready audio tools:
  - `AudioTranscribeTool` — supports both the OpenAI Whisper API and a local
    `whisper.cpp` binary; auto-detects language; handles MP3, WAV, FLAC, OGG,
    WebM, M4A.
  - `TextToSpeechTool` — converts text to MP3 audio via the OpenAI TTS API
    with configurable voice (`alloy`, `echo`, `fable`, `onyx`, `nova`,
    `shimmer`) and model (`tts-1`, `tts-1-hd`).

### Added — Upgrade Set B: Vector Memory and RAG

- **`src/memory/vector.rs`** — Local vector memory store backed by an in-process
  cosine-similarity index; supports `store`, `search`, `list`, and `delete`
  operations; designed for drop-in replacement with a fastembed or HNSW backend.

### Added — Upgrade Set C: MCP Integration and Agent Patterns

- **`src/mcp/`** — Full Model Context Protocol (MCP) implementation:
  - `src/mcp/mod.rs` — JSON-RPC 2.0 types (`JsonRpcRequest`, `JsonRpcResponse`,
    `McpToolDef`, `McpContent`) and an `McpClientTool` adapter that wraps any
    remote MCP tool as a local `Tool`.
  - `src/mcp/server.rs` — `McpServer` that exposes all registered Oh-Ben-Claw
    tools over stdio (for Claude Desktop / Cursor / VS Code) and HTTP+SSE
    transports via Axum.
  - `src/mcp/client.rs` — `McpClient` that connects to external MCP servers
    and imports their tools into the local registry.
- **`src/agent/reflexion.rs`** — Two advanced orchestration patterns:
  - **Reflexion loop** (Shinn et al., 2023) — iterative generate → critique →
    revise cycle with configurable `max_rounds` and `quality_threshold`.
  - **Plan-and-Execute** — decomposes complex tasks into numbered steps, tracks
    `StepStatus` (Pending / Running / Completed / Failed / Skipped), and
    synthesizes a final answer from all step results.

### Added — Upgrade Set D: Telemetry Dashboard and ESP32 OTA

- **`src/dashboard/`** — Optional Ratatui TUI dashboard (enabled with
  `--features dashboard`):
  - `src/dashboard/mod.rs` — `DashboardApp` with tabbed layout (Overview,
    Tools, Devices, Logs); live metric panels for CPU, memory, active agents,
    tool calls per minute, and tunnel status.
  - `src/dashboard/widgets.rs` — Reusable `MetricGauge`, `SparklineWidget`,
    and `LogPanel` widgets.
- **`src/tools/builtin/ota.rs`** — Two ESP32/embedded OTA tools:
  - `OtaUpdateTool` — flashes firmware to ESP32, STM32, Arduino, and
    Raspberry Pi boards; supports `esptool.py`, `openocd`, `avrdude`, and
    `rpi-imager`; includes dry-run mode.
  - `DeviceHealthTool` — queries MQTT Spine for live device telemetry
    (firmware version, uptime, free heap, signal strength, last-seen
    timestamp).

### Changed

- **`src/tools/mod.rs`** — `default_tools()` now reads `OPENAI_API_KEY` from
  the environment at startup and conditionally registers `VisionTool`; audio
  and OTA tools are always registered.
- **`src/tools/builtin/mod.rs`** — Exports `vision`, `audio`, and `ota`
  sub-modules.
- **`src/agent/mod.rs`** — Imports `reflexion` module.
- **`src/lib.rs`** — Exports `mcp`, `memory::vector`, `providers::streaming`,
  and `agent::reflexion` at the crate root.
- **`Cargo.toml`** — Added optional dependencies: `ratatui`, `crossterm`
  (behind `dashboard` feature); `axum`, `tokio` (HTTP server); `base64`,
  `reqwest/multipart` (vision/audio).
- **`.github/workflows/ci.yml`** — CI matrix now tests both default features
  and `--features dashboard`.

### Fixed

- All new `Tool` implementations correctly return `anyhow::Result<ToolResult>`
  as required by the `Tool` trait.
- `McpServer::handle_tools_call` properly unwraps `Result<ToolResult>` and
  maps execution errors to JSON-RPC `-32603` responses.
- `reflexion_loop` and `create_plan` use `ChatMessage` / `ChatRole` /
  `provider.chat_completion()` matching the existing `Provider` trait API.
- Test assertions in `audio.rs` and `ota.rs` correctly inspect
  `result.error.as_deref()` for error-path messages.

### Test Results

```
test result: ok. 221 passed; 0 failed; 0 ignored; 0 measured
```

All 221 unit tests pass. Doc-tests: 2 passed, 1 ignored (vault integration
test requires a running keyring daemon).

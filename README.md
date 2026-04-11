<h1 align="center">Oh-Ben-Claw 🦀🧠</h1>
<p align="center">
  <strong>Advanced. Distributed. Multi-Device. 100% Rust.</strong><br>
  ⚡️ <strong>One brain, many arms — orchestrate a fleet of AI-powered devices from a single agent.</strong>
</p>
<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  <a href="https://github.com/thewriterben/Oh-Ben-Claw/actions"><img src="https://img.shields.io/github/actions/workflow/status/thewriterben/Oh-Ben-Claw/ci.yml?branch=main" alt="CI" /></a>
  <a href="https://github.com/thewriterben/Oh-Ben-Claw/releases"><img src="https://img.shields.io/github/v/release/thewriterben/Oh-Ben-Claw?include_prereleases" alt="Release" /></a>
  <img src="https://img.shields.io/badge/tests-554%20passing-brightgreen" alt="554 tests passing" />
  <img src="https://img.shields.io/badge/rust-stable-orange" alt="Rust stable" />
</p>

**Oh-Ben-Claw** is an advanced, distributed AI assistant built on the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It extends the core framework with a multi-device coordination layer, enabling a single intelligent agent to orchestrate a fleet of specialized hardware peripherals — cameras, microphones, sensors, actuators, and more — over a unified MQTT communication spine.

> **Mental model:** Oh-Ben-Claw is the brain. Your ESP32s, NanoPis, and Raspberry Pis are the arms, eyes, and ears.

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Key Features](#key-features)
- [Supported Hardware](#supported-hardware)
- [Getting Started](#getting-started)
- [Configuration](#configuration)
- [Personality Files](#personality-files)
- [Browser Automation](#browser-automation)
- [Deployment Scheme Generator](#deployment-scheme-generator)
- [CLI Reference](#cli-reference)
- [Firmware](#firmware)
- [Native GUI](#native-gui)
- [Project Structure](#project-structure)
- [Relationship to ZeroClaw](#relationship-to-zeroclaw)
- [License](#license)

---

## Architecture Overview

Oh-Ben-Claw is organized around three layers:

| Layer | Component | Description |
|---|---|---|
| **Brain** | Core Agent | The central LLM-powered reasoning engine, running on a host machine. Orchestrates all peripheral nodes. |
| **Spine** | MQTT Broker | The unified communication backbone. All devices publish their capabilities and receive commands over MQTT topics. |
| **Appendages** | Peripheral Nodes | Specialized firmware running on ESP32-S3, NanoPi Neo3, Raspberry Pi, and other hardware. Each node exposes its capabilities as tools. |

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Oh-Ben-Claw Core Agent (Host: macOS / Linux / Windows)                      │
│                                                                              │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────────────┐  │
│  │  Channels   │──►│  Agent Loop  │──►│  Unified Tool Registry           │  │
│  │  Telegram   │   │  (LLM calls) │   │  (local + peripheral + browser)  │  │
│  │  Discord    │   └──────┬───────┘   └──────────────────────────────────┘  │
│  │  Feishu     │          │                                                  │
│  │  IRC/Signal │          ▼                                                  │
│  │  CLI / GUI  │   ┌───────────────┐   ┌──────────────┐                    │
│  └─────────────┘   │  MQTT Spine   │   │  Deployment  │                    │
│                    └──────┬────────┘   │  Planner     │                    │
└───────────────────────────┼────────────└──────────────┘────────────────────┘
                            │
          ┌─────────────────┼─────────────────┐
          │                 │                 │
          ▼                 ▼                 ▼
┌─────────────────┐ ┌─────────────┐ ┌─────────────────┐
│ ESP32-S3 Node   │ │ NanoPi Neo3 │ │ Raspberry Pi    │
│ - camera_capture│ │ - gpio_read │ │ - gpio_read     │
│ - audio_sample  │ │ - gpio_write│ │ - gpio_write    │
│ - sensor_read   │ │ - i2c_scan  │ │ - camera_capture│
│ - gpio_read/wrt │ └─────────────┘ │ - audio_sample  │
└─────────────────┘                 └─────────────────┘
```

---

## Key Features

**Multi-Device Orchestration** allows a single agent to command a fleet of hardware nodes simultaneously. Each node registers its capabilities dynamically over the MQTT spine, and the central agent merges all available tools into a single, unified registry. The agent can then use natural language to invoke any tool on any device.

**MQTT Communication Spine** replaces the direct serial-only connections of the base ZeroClaw with a scalable, network-based publish-subscribe model. This enables devices to be located anywhere on the local network (or even the internet via a tunnel), and allows for easy addition and removal of nodes without restarting the core agent.

**P2P Broker-Free Mesh** enables peripheral nodes to discover and communicate with each other directly, without requiring a central MQTT broker. The P2P spine uses mDNS for discovery and direct TCP connections for tool calls.

**Multi-Modal I/O** provides a unified interface for interacting with the physical world. The agent can see (via cameras on ESP32-S3 or Raspberry Pi), hear (via I2S microphones or the Sipeed 6+1 mic array), sense (via I2C/SPI sensors like BME280, MPU6050, DHT22), and act (via GPIO on NanoPi Neo3 or Raspberry Pi).

**Browser Automation** integrates a Chrome DevTools Protocol (CDP) browser layer with seven tools: `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_scroll`, `browser_new_tab`, and `browser_close_tab`. Headless and user-profile modes are supported; falls back to plain HTTP fetch when no CDP endpoint is reachable.

**Image Memory** provides a persistent, SQLite-backed image store so the agent can store and retrieve visual context across sessions. Images are stored with UUID, MIME type, base64 data, description, tags, and session metadata.

**ClawHub Skill Registry** connects Oh-Ben-Claw to the community skill marketplace. The `ClawHubClient` searches, downloads, and installs community-authored skill manifests, expanding the agent's capabilities without code changes.

**Deployment Scheme Generator** analyses your available hardware inventory, maps capabilities to agent roles, identifies hardware gaps, and renders a ready-to-use TOML configuration. An optional LLM-powered multi-agent swarm (hardware-advisor, architect, requirements-checker) can refine the plan further.

**Rich Communication Channels** supports Telegram, Discord, Slack, WhatsApp, iMessage, IRC, Matrix, Signal, Mattermost, Feishu/Lark, and a built-in CLI. Typing indicators keep users informed while the agent processes. A native GUI application (Tauri 2 + React) is included.

**Pluggable LLM Providers** supports all major LLM providers, including OpenAI, Anthropic, Google Gemini, Ollama (local), OpenRouter, and any OpenAI-compatible endpoint. **Model failover** chains fallback providers automatically; **retry policies** handle transient rate-limits with exponential backoff.

**Vision Pipeline** connects camera peripherals to vision-capable LLMs. The pipeline captures images, encodes them, and passes them to the model in a single turn — no manual base64 encoding required.

**Audio Pipeline** connects microphone peripherals to speech-to-text backends (OpenAI Whisper or local `whisper.cpp`). The pipeline captures audio, transcribes it, forwards the text to the agent, and can synthesize spoken replies via text-to-speech.

**Sensor Fusion** combines readings from multiple sensors (e.g., BME280 + MPU6050 + DHT22) into a unified data structure, enabling the agent to reason about the physical environment holistically.

**Skill Forge** provides automatic discovery and integration of new agent skills. Community skills can be browsed and installed from the ClawHub registry, or developed locally.

**Edge-Native Mode** enables peripheral nodes (NanoPi Neo3, ESP32-S3) to run the full Oh-Ben-Claw agent locally, without a host machine. The edge agent loop is lightweight and supports local Ollama inference.

**Human-in-the-Loop Approval** provides a supervised execution mode where tool calls require explicit user approval. Three autonomy levels (`full`, `supervised`, `manual`) control when prompts appear, backed by a session-scoped allowlist and a full audit log.

**Token Cost Tracking** monitors API usage in real time. Budget limits (daily and monthly) can be set in the config to prevent runaway spending, with configurable warning thresholds before hard limits are hit.

**System Diagnostics** (`oh-ben-claw doctor`) runs a comprehensive health check on your configuration, environment, communication channels, and MQTT spine — reporting ✅ / ⚠️ / ❌ for each item.

**Event Lifecycle Hooks** allow external code to observe and intercept all major agent events — session start/stop, incoming messages, tool calls, tool results, and agent responses — with priority-ordered handlers and short-circuit cancellation.

**Enhanced Multimodal Handling** lets you embed `[IMAGE:/path/to/file.png]` markers directly in text messages. The multimodal pipeline strips the markers, reads the images, and injects them as structured content blocks for vision-capable models.

**Hardware Datasheet RAG** indexes your `docs/datasheets/` directory and exposes a `datasheet_search` tool that the agent can use to look up pin layouts, register maps, sensor specs, and I2C addresses — all retrieved via keyword search directly from your markdown and text datasheets.

**Sandboxed Tool Execution** runs shell-based tools through a configurable runtime adapter. The default `native` runtime executes directly on the host; the `docker` runtime wraps every command in a fresh, memory-limited, network-isolated container for maximum safety.

**Terminal TUI Dashboard** (`--features dashboard`) renders a real-time terminal interface with tabbed panels for Overview, Tools, Devices, and Logs. Live metric gauges show CPU, memory, active agents, tool calls per minute, and tunnel status.

**MCP Integration** exposes all registered tools as a Model Context Protocol (MCP) server over stdio and HTTP+SSE, compatible with Claude Desktop, Cursor, and VS Code. An MCP client adapter imports tools from external MCP servers into the local registry.

**Reflexion Loop & Plan-and-Execute** provide advanced orchestration patterns. The Reflexion loop (Shinn et al., 2023) iteratively generates, critiques, and revises responses. Plan-and-Execute decomposes complex tasks into numbered steps with tracked status.

**Personality System** (inspired by [MimiClaw](https://github.com/memovai/mimiclaw)) stores the agent's personality in editable Markdown files. Drop a `SOUL.md` in `~/.oh-ben-claw/` to customise the agent's behaviour without editing `config.toml`. Add a `USER.md` to tell the agent about you — your name, language, preferences.

**Proactive Task Dispatch** monitors `~/.oh-ben-claw/HEARTBEAT.md` for uncompleted tasks and injects them into the agent loop on a schedule. Write tasks in plain Markdown, check them off with `- [x]`, and the agent acts on the rest autonomously.

**Daily Journal** writes human-readable `YYYY-MM-DD.md` notes alongside the SQLite conversation history. The journal files are easy to browse, back up, and share.

**HTTP Proxy Support** routes all outbound HTTP requests (LLM API calls, channel webhooks, …) through a configurable HTTP or SOCKS5 proxy — useful for corporate firewalls or restricted networks.

**A2A Protocol** implements Google's Agent-to-Agent interoperability protocol, enabling cross-platform agent communication. The A2A client can discover and invoke remote agents; the A2A server exposes Oh-Ben-Claw's capabilities via a standard Agent Card so that external A2A-compatible agents can call in.

**Structured Output** adds JSON mode and JSON Schema response formatting to LLM calls. When a schema is provided, the agent constrains model output to valid, parseable JSON — making tool results and downstream integrations more reliable.

**Streaming Tool Calls** accumulates partial tool-call fragments from streaming LLM responses into complete, validated calls. The accumulator and builder pattern ensures no data is lost, even when a single response contains multiple interleaved tool invocations.

**WASM Sandbox** provides a WebAssembly runtime adapter for secure, sandboxed tool execution. In addition to the existing `native` and `docker` runtimes, the `wasm` runtime compiles tool code to WASM and executes it in a memory-safe, capability-restricted environment.

**Persistent Cost Tracking** extends the existing token-cost subsystem with a SQLite-backed store that survives process restarts. Cross-session budget enforcement ensures spending limits are respected even when the agent is restarted mid-billing-period.

---

## Supported Hardware

### Boards

| Device | Transport | Capabilities |
|---|---|---|
| Waveshare ESP32-S3 Touch LCD 2.1 | Serial / MQTT | GPIO, Camera (OV2640), Microphone (I2S), Touch Display, Speaker |
| Seeed XIAO ESP32S3-Sense | Serial / MQTT | Camera (OV2640), Microphone (PDM), GPIO, Wi-Fi, BLE |
| Sipeed 6+1 Mic Array | USB (UAC1) / Serial | Far-field 6+1 MEMS microphone array |
| ESP32-S3 (generic) | Serial / MQTT | GPIO, Camera, Microphone, Sensors |
| ESP32-C3 | Serial / MQTT | GPIO, I2C, SPI, Wi-Fi, BLE |
| NanoPi Neo3 | Native (sysfs) / MQTT | GPIO (sysfs), I2C, SPI |
| Raspberry Pi (all models) | Native (rppal) / MQTT | GPIO, Camera (libcamera), Microphone |
| STM32 Nucleo-F401RE | Serial (probe-rs) | GPIO, ADC, Flash, Memory Map |
| STM32H7 Discovery | Probe (probe-rs) | GPIO, ADC, DAC, I2C, SPI, Flash |
| Arduino Uno / Mega | Serial | GPIO, Analog Read |
| Arduino Nano 33 BLE | Serial | GPIO, Analog Read, I2C, SPI, BLE, Sensors |
| Teensy 4.1 | Serial | GPIO, ADC, DAC, I2C, SPI, PWM, CAN |
| nRF52840 DK | Serial | GPIO, I2C, SPI, BLE, PWM |
| BeagleBone Black | Native | GPIO, ADC, I2C, SPI, PWM, CAN |
| NVIDIA Jetson Nano | Native | GPIO, I2C, SPI, PWM, Camera, CUDA |

### Supported I2C/SPI/GPIO Accessories

| Accessory | Bus | Default Address | Capabilities |
|---|---|---|---|
| BME280 | I2C | 0x76 | Temperature, Humidity, Pressure |
| BMP388 | I2C | 0x77 | Pressure, Altitude, Temperature |
| AHT20 | I2C | 0x38 | Temperature, Humidity |
| MPU6050 | I2C | 0x68 | Accelerometer, Gyroscope |
| LSM6DS3 | I2C | 0x6A | Accelerometer, Gyroscope |
| SHT31 | I2C | 0x44 | Temperature, Humidity |
| ADS1115 | I2C | 0x48 | 16-bit ADC (4-channel) |
| MCP4725 | I2C | 0x60 | 12-bit DAC |
| INA260 | I2C | 0x40 | Voltage, Current, Power |
| PCF8574 | I2C | 0x20 | 8-bit GPIO Expander |
| MCP23017 | I2C | 0x20 | 16-bit GPIO Expander |
| MAX31855 | SPI | — | Thermocouple Temperature |
| DS18B20 | 1-Wire | — | Digital Temperature |
| SSD1306 | I2C | 0x3C | 128×64 OLED Display |
| DHT22 | GPIO | — | Temperature, Humidity |
| DHT11 | GPIO | — | Temperature, Humidity |

---

## Getting Started

### Prerequisites

- **Rust** (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **MQTT broker** (e.g., Mosquitto): `brew install mosquitto` or `apt install mosquitto`
- **LLM API key** — OpenAI, Anthropic, Google Gemini, or a local [Ollama](https://ollama.ai) instance

### Installation

```bash
# Clone the repository
git clone https://github.com/thewriterben/Oh-Ben-Claw.git
cd Oh-Ben-Claw

# Build the core agent (hardware + MQTT features enabled by default)
cargo build --release

# Optional: build with terminal dashboard
cargo build --release --features dashboard

# Run the setup wizard
./target/release/oh-ben-claw setup
```

### Quick Start

```bash
# 1. Start your MQTT broker
mosquitto -d

# 2. Set your LLM API key
export OPENAI_API_KEY="sk-..."

# 3. Start the agent (uses ~/.oh-ben-claw/config.toml)
./target/release/oh-ben-claw start

# 4. In another terminal, run diagnostics
./target/release/oh-ben-claw doctor
```

---

## Configuration

Oh-Ben-Claw uses a TOML configuration file at `~/.oh-ben-claw/config.toml`. The setup wizard will guide you through the initial configuration. See [`examples/config-multi-device.toml`](examples/config-multi-device.toml) for a full annotated example.

```toml
[agent]
name = "Oh-Ben-Claw"
system_prompt = "You are Oh-Ben-Claw, an advanced multi-device AI assistant."
max_tool_iterations = 15

[provider]
name = "openai"
model = "gpt-4o"
# api_key = "sk-..."  # Or set OPENAI_API_KEY

# Fallback chain — tried in order when primary provider fails
[[provider.fallbacks]]
name  = "anthropic"
model = "claude-3-5-sonnet-20241022"

[[provider.fallbacks]]
name  = "ollama"
model = "llama3.2"

# Exponential-backoff retry for transient errors
[provider.retry]
max_retries        = 3
initial_backoff_ms = 500

[spine]
kind = "mqtt"
host = "localhost"
port = 1883

[peripherals]
enabled = true
datasheet_dir = "docs/datasheets"

# ESP32-S3 board via serial
[[peripherals.boards]]
board     = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "serial"
path      = "/dev/ttyUSB0"
baud      = 115200

# NanoPi Neo3 running natively on the host
[[peripherals.boards]]
board     = "nanopi-neo3"
transport = "native"

[channels.telegram]
token = "your-telegram-bot-token"

[channels.discord]
token = "your-discord-bot-token"

# Feishu/Lark enterprise channel
[channels.feishu]
app_id             = "cli_xxxxxxxxxxxxxx"
app_secret         = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
verification_token = "your-verification-token"
webhook_port       = 18790

# Supervised mode — require approval before running tools
[autonomy]
level        = "supervised"     # "full" (default), "supervised", or "manual"
auto_approve = ["sensor_read"]  # tools that never need approval
always_ask   = ["shell"]        # tools that always need approval

# Token cost tracking and budget enforcement
[cost]
enabled           = true
daily_limit_usd   = 5.0
monthly_limit_usd = 50.0
warn_threshold    = 0.8

# Sandboxed tool execution
[runtime]
kind = "native"   # "native" (default) or "docker"

# Multimodal image handling
[multimodal]
enabled         = true
max_images      = 5
max_image_bytes = 5242880   # 5 MB
allow_remote    = false

# Browser automation (CDP)
[browser]
enabled     = true
cdp_url     = "http://localhost:9222"
profile     = "headless"    # "headless" or "user"
timeout_secs = 30

# ClawHub skill marketplace
[clawhub]
enabled      = true
registry_url = "https://clawhub.dev/api/v1"
auto_update  = false
skills_dir   = "~/.oh-ben-claw/skills"

# HTTP proxy for restricted networks
[proxy]
enabled = false
host    = "10.0.0.1"
port    = 7897
kind    = "http"   # "http" (default) or "socks5"
```

---

## Personality Files

Instead of editing `system_prompt` in `config.toml`, you can drop plain Markdown files in `~/.oh-ben-claw/`:

| File | Purpose |
|------|---------|
| `SOUL.md` | Agent personality — overrides `[agent].system_prompt` when present |
| `USER.md` | User profile — appended to the system prompt as a "User Profile" section |
| `HEARTBEAT.md` | Proactive task list — uncompleted items trigger the agent on a schedule |

**`~/.oh-ben-claw/SOUL.md`** example:
```markdown
You are Oh-Ben-Claw, a precise and proactive AI assistant running on distributed
hardware. You prefer short, actionable replies. You never make up tool results.
```

**`~/.oh-ben-claw/USER.md`** example:
```markdown
My name is Ben. I work in home automation and embedded systems. I prefer metric
units. My primary language is English.
```

**`~/.oh-ben-claw/HEARTBEAT.md`** example (the agent checks this on a schedule):
```markdown
# My Tasks

- [ ] Send the weekly status report to the team
- [x] Order replacement fan for the server room   ← completed, agent skips this
- [ ] Book dentist appointment
```

---

## Browser Automation

Oh-Ben-Claw includes a full browser automation layer driven by the Chrome DevTools Protocol (CDP). Start Chrome/Chromium in remote debugging mode and set the `[browser]` config section:

```bash
# Start Chrome with CDP enabled
google-chrome --remote-debugging-port=9222 --headless
```

The following tools are registered automatically:

| Tool | Description |
|------|-------------|
| `browser_navigate` | Navigate to a URL; returns page title |
| `browser_snapshot` | Capture stripped-HTML text snapshot of the active page |
| `browser_click` | Click an element by CSS selector |
| `browser_type` | Type text into a focused element or selector-targeted input |
| `browser_scroll` | Scroll up / down / to top / to bottom / to a CSS selector |
| `browser_new_tab` | Open a new browser tab |
| `browser_close_tab` | Close the active browser tab |

Set `OBC_BROWSER_CDP_URL` environment variable to override the CDP endpoint at runtime.

---

## Deployment Scheme Generator

Phase 13 introduces a hardware-driven deployment planner. Given a list of available hardware and desired features, it generates a complete multi-agent topology and TOML configuration.

```toml
[deployment]
enabled         = true
scenario        = "My Home Hub"
auto_plan       = true    # print scheme on startup
llm_swarm       = false   # use LLM sub-agents to refine the plan

feature_desires = ["vision", "listening", "environmental_sensing", "display_output"]

[[deployment.hardware]]
name       = "nanopi-neo3"
board_name = "nanopi-neo3"
transport  = "native"
role       = "host"
accessories = ["dht22"]

[[deployment.hardware]]
name       = "xiao-esp32s3-sense"
board_name = "xiao-esp32s3-sense"
transport  = "serial"
path       = "/dev/ttyUSB0"
role       = "vision"
```

See [`examples/config-nanopi-deployment.toml`](examples/config-nanopi-deployment.toml) for the full NanoPi Neo3 reference deployment covering all five hardware roles (host, vision, listening, display, sensing).

---

## CLI Reference

```
oh-ben-claw <COMMAND>

Commands:
  start      Start the agent and connect to all configured peripherals and channels
  setup      Interactive setup wizard — creates ~/.oh-ben-claw/config.toml
  doctor     Run system diagnostics (configuration, connectivity, channels, MQTT spine)
  status     Show running agent status, connected nodes, and active sessions
  peripheral List and manage connected peripheral nodes
  service    Manage the oh-ben-claw system service (install / start / stop / uninstall)
  help       Print help for a command

Options:
  -c, --config <PATH>   Path to config file [default: ~/.oh-ben-claw/config.toml]
  -v, --verbose         Enable verbose logging
  --dashboard           Launch the terminal TUI dashboard (requires --features dashboard)
  -h, --help            Print help
  -V, --version         Print version
```

---

## Firmware

Firmware for peripheral nodes is located in the `firmware/` directory.

### ESP32-S3 (`firmware/obc-esp32-s3`)

The ESP32-S3 firmware exposes GPIO, camera, audio, and sensor capabilities over both a serial JSON protocol and MQTT. It also supports an edge-native agent loop with Wi-Fi + cloud LLM.

```bash
# Install the ESP-IDF / Rust toolchain
cargo install espup && espup install && source ~/export-esp.sh

# Build and flash (from the firmware/obc-esp32-s3 directory)
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

Works with:
- Waveshare ESP32-S3 Touch LCD 2.1 (`0x303a:0x8135`)
- Seeed XIAO ESP32S3-Sense (`0x2886:0x0058`)
- Any generic ESP32-S3 board

### NanoPi Neo3

The NanoPi Neo3 can run the full Oh-Ben-Claw agent natively, providing GPIO access via Linux sysfs and native I2C/SPI access.

```bash
# Cross-compile from host (Linux/macOS)
cargo build \
  --target aarch64-unknown-linux-gnu \
  --features hardware,peripheral-nanopi \
  --release

# Copy to the device
scp target/aarch64-unknown-linux-gnu/release/oh-ben-claw root@nanopi:/usr/local/bin/
```

### Raspberry Pi

The Raspberry Pi uses the `rppal` crate for GPIO and `libcamera-still` for camera capture.

```bash
cargo build \
  --target aarch64-unknown-linux-gnu \
  --features hardware,peripheral-rpi \
  --release
```

---

## Native GUI

A native desktop application is included in `gui/`, built with Tauri 2 + React 18 + TypeScript + TailwindCSS.

```bash
# Prerequisites: Node.js 18+ and pnpm
npm install -g pnpm

# Development
cd gui && pnpm install && pnpm tauri dev

# Production build
cd gui && pnpm tauri build
```

See [`gui/README.md`](gui/README.md) for full prerequisites, build instructions, panel descriptions, and the Tauri command reference.

---

## Project Structure

```
Oh-Ben-Claw/
├── src/
│   ├── agent/          # Core agent loop, dispatcher, Reflexion, Plan-and-Execute
│   ├── approval/       # Human-in-the-loop approval workflow
│   ├── audio/          # Audio pipeline (microphone → STT → agent → TTS)
│   ├── channels/       # Communication channels (Telegram, Discord, Feishu, IRC, Signal, …)
│   ├── config/         # Configuration schema and loading (Config::validate)
│   ├── cost/           # Token cost tracking and budget enforcement
│   ├── dashboard/      # Real-time terminal TUI dashboard (--features dashboard)
│   ├── deployment/     # Hardware-driven deployment scheme generator (Phase 13)
│   ├── doctor/         # System diagnostics (oh-ben-claw doctor)
│   ├── gateway/        # REST/WebSocket API gateway (Axum)
│   ├── hooks/          # Event lifecycle hooks
│   ├── mcp/            # Model Context Protocol client/server
│   ├── memory/         # SQLite memory, personality, journal, heartbeat, image, vector
│   ├── multimodal.rs   # Enhanced multimodal message handling ([IMAGE:…] markers)
│   ├── observability/  # Logging, metrics, OpenTelemetry
│   ├── peripherals/    # Hardware drivers (ESP32-S3, NanoPi, RPi, STM32, Arduino, …)
│   ├── providers/      # LLM provider adapters + failover + retry
│   ├── rag/            # RAG pipeline for hardware datasheet retrieval
│   ├── runtime/        # Sandboxed tool execution (native + Docker + WASM)
│   │   └── wasm.rs     # WASM runtime adapter
│   ├── a2a/            # A2A protocol client and server
│   ├── agent/streaming.rs # Streaming tool call accumulator + builder
│   ├── scheduler/      # Scheduled tasks and cron jobs
│   ├── security/       # Policy engine, node pairing, encrypted secrets vault
│   ├── skill_forge/    # Skill discovery, integration, and ClawHub registry client
│   ├── spine/          # MQTT communication spine + P2P broker-free mesh
│   ├── tools/          # Tool registry (shell, file, browser, hardware, OTA, vision, …)
│   ├── tunnel/         # Network tunnels (Cloudflare, ngrok, Tailscale)
│   └── vision/         # Vision pipeline (camera → LLM vision → action)
├── firmware/
│   ├── obc-esp32-s3/   # ESP32-S3 firmware (GPIO + camera + mic + sensors + edge agent)
│   ├── obc-nanopi/     # NanoPi Neo3 native agent
│   └── obc-rpi/        # Raspberry Pi native agent
├── gui/                # Tauri 2 + React 18 native desktop application
├── docs/
│   ├── architecture/   # Architecture design documents
│   └── datasheets/     # Hardware datasheets and pin maps
├── examples/
│   ├── config-multi-device.toml       # Annotated multi-device configuration
│   └── config-nanopi-deployment.toml  # NanoPi Neo3 reference deployment (Phase 13)
└── .github/
    ├── workflows/ci.yml               # CI: build + test + clippy + GUI
    └── ISSUE_TEMPLATE/                # Bug report and feature request templates
```

---

## Relationship to ZeroClaw

Oh-Ben-Claw is built on top of the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It inherits the core agent loop, provider system, channel system, tool registry, and peripheral framework — and extends it significantly:

| Feature | ZeroClaw | Oh-Ben-Claw |
|---|---|---|
| Communication | Direct serial / native GPIO | MQTT spine + P2P mesh + serial / native |
| Tool discovery | Static configuration | Dynamic via node announcements |
| Multi-device | Multiple boards, direct connections | Fleet of nodes over network |
| Browser automation | ✗ | ✅ CDP (7 tools) |
| Image memory | ✗ | ✅ SQLite-backed |
| Deployment planner | ✗ | ✅ LLM + rule-based swarm |
| GUI | ✗ | ✅ Tauri 2 + React 18 |
| Node pairing | ✗ | ✅ HMAC-SHA256 |
| MCP integration | ✗ | ✅ Client + server |
| Vision pipeline | ✗ | ✅ |
| Audio pipeline | ✗ | ✅ |
| Sensor fusion | ✗ | ✅ |
| TUI dashboard | ✗ | ✅ (Ratatui) |
| Personality files | ✗ | ✅ SOUL.md / USER.md |
| Proactive tasks | ✗ | ✅ HEARTBEAT.md |
| Daily journal | ✗ | ✅ YYYY-MM-DD.md |
| ClawHub registry | ✗ | ✅ |
| Feishu/Lark channel | ✗ | ✅ |
| IRC / Signal / Mattermost | ✗ | ✅ |
| Typing indicators | ✗ | ✅ (Telegram, Discord, Slack) |
| Model failover | ✗ | ✅ |
| Retry policy | ✗ | ✅ Exponential backoff |
| Human approval | ✗ | ✅ 3 autonomy levels |
| Cost tracking | ✗ | ✅ Daily/monthly budgets |
| Docker sandbox | ✗ | ✅ |
| Reflexion / Plan-and-Execute | ✗ | ✅ |
| Edge-native mode | ✗ | ✅ (ESP32-S3, NanoPi) |
| Hardware datasheet RAG | ✗ | ✅ |
| A2A protocol | ✗ | ✅ Client + server |
| Structured output | ✗ | ✅ JSON mode / JSON Schema |
| Streaming tool calls | ✗ | ✅ Accumulator + builder |
| WASM sandbox | ✗ | ✅ Runtime adapter |
| Persistent cost tracking | ✗ | ✅ SQLite-backed |

---

## License

MIT — see [LICENSE](LICENSE) for details.

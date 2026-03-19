<h1 align="center">Oh-Ben-Claw 🦀🧠</h1>
<p align="center">
  <strong>Advanced. Distributed. Multi-Device. 100% Rust.</strong><br>
  ⚡️ <strong>One brain, many arms — orchestrate a fleet of AI-powered devices from a single agent.</strong>
</p>
<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  <a href="https://github.com/thewriterben/Oh-Ben-Claw/actions"><img src="https://img.shields.io/github/actions/workflow/status/thewriterben/Oh-Ben-Claw/ci.yml?branch=main" alt="CI" /></a>
</p>

**Oh-Ben-Claw** is an advanced, distributed AI assistant built on the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It extends the core framework with a multi-device coordination layer, enabling a single intelligent agent to orchestrate a fleet of specialized hardware peripherals — cameras, microphones, sensors, actuators, and more — over a unified MQTT communication spine.

> **Mental model:** Oh-Ben-Claw is the brain. Your ESP32s, NanoPis, and Raspberry Pis are the arms, eyes, and ears.

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
│  │  Telegram   │   │  (LLM calls) │   │  (local + all peripheral tools)  │  │
│  │  Discord    │   └──────┬───────┘   └──────────────────────────────────┘  │
│  │  CLI / GUI  │          │                                                  │
│  └─────────────┘          ▼                                                  │
│                   ┌───────────────┐                                          │
│                   │  MQTT Spine   │  ◄── Unified communication backbone      │
│                   └──────┬────────┘                                          │
└──────────────────────────┼───────────────────────────────────────────────────┘
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

**Multi-Modal I/O** provides a unified interface for interacting with the physical world. The agent can see (via cameras on ESP32-S3 or Raspberry Pi), hear (via I2S microphones), sense (via I2C/SPI sensors like BME280, MPU6050), and act (via GPIO on NanoPi Neo3 or Raspberry Pi).

**Rich Communication Channels** supports Telegram, Discord, Slack, WhatsApp, iMessage, IRC, Matrix, Signal, Mattermost, **Feishu/Lark**, and a built-in CLI. A native GUI application is also planned.

**Pluggable LLM Providers** supports all major LLM providers, including OpenAI, Anthropic, Google Gemini, Ollama (local), and any OpenAI-compatible endpoint.

**Human-in-the-Loop Approval** provides a supervised execution mode where tool calls require explicit user approval. Three autonomy levels (`full`, `supervised`, `manual`) control when prompts appear, backed by a session-scoped allowlist and a full audit log.

**Token Cost Tracking** monitors API usage in real time. Budget limits (daily and monthly) can be set in the config to prevent runaway spending, with configurable warning thresholds before hard limits are hit.

**System Diagnostics** (`oh-ben-claw doctor`) runs a comprehensive health check on your configuration, environment, communication channels, and MQTT spine — reporting ✅ / ⚠️ / ❌ for each item.

**Event Lifecycle Hooks** allow external code to observe and intercept all major agent events — session start/stop, incoming messages, tool calls, tool results, and agent responses — with priority-ordered handlers and short-circuit cancellation.

**Enhanced Multimodal Handling** lets you embed `[IMAGE:/path/to/file.png]` markers directly in text messages. The multimodal pipeline strips the markers, reads the images, and injects them as structured content blocks for vision-capable models.

**Hardware Datasheet RAG** indexes your `docs/datasheets/` directory and exposes a `datasheet_search` tool that the agent can use to look up pin layouts, register maps, sensor specs, and I2C addresses — all retrieved via keyword search directly from your markdown and text datasheets.

**Sandboxed Tool Execution** runs shell-based tools through a configurable runtime adapter. The default `native` runtime executes directly on the host; the `docker` runtime wraps every command in a fresh, memory-limited, network-isolated container for maximum safety.

**Personality System** (inspired by [MimiClaw](https://github.com/memovai/mimiclaw)) stores the agent's personality in editable Markdown files. Drop a `SOUL.md` in `~/.oh-ben-claw/` to customise the agent's behaviour without editing `config.toml`. Add a `USER.md` to tell the agent about you — your name, language, preferences.

**Proactive Task Dispatch** (inspired by MimiClaw's heartbeat service) monitors `~/.oh-ben-claw/HEARTBEAT.md` for uncompleted tasks and injects them into the agent loop on a schedule. Write tasks in plain Markdown, check them off with `- [x]`, and the agent acts on the rest autonomously.

**Daily Journal** (inspired by MimiClaw's per-day notes) writes human-readable `YYYY-MM-DD.md` notes alongside the SQLite conversation history. The journal files are easy to browse, back up, and share.

**HTTP Proxy Support** (inspired by MimiClaw's proxy system) routes all outbound HTTP requests (LLM API calls, channel webhooks, …) through a configurable HTTP or SOCKS5 proxy — useful for corporate firewalls or restricted networks.

---

## Supported Hardware

| Device | Transport | Capabilities |
|---|---|---|
| Waveshare ESP32-S3 Touch LCD 2.1 | Serial / MQTT | GPIO, Camera (OV2640), Microphone (I2S), Sensors (I2C) |
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

### Supported I2C/SPI Accessories

| Accessory | Bus | Default Address | Capabilities |
|---|---|---|---|
| BME280 | I2C | 0x76 | Temperature, Humidity, Pressure |
| BMP388 | I2C | 0x77 | Pressure, Altitude, Temperature |
| MPU6050 | I2C | 0x68 | Accelerometer, Gyroscope |
| LSM6DS3 | I2C | 0x6A | Accelerometer, Gyroscope |
| SHT31 | I2C | 0x44 | Temperature, Humidity |
| ADS1115 | I2C | 0x48 | 16-bit ADC (4-channel) |
| INA260 | I2C | 0x40 | Voltage, Current, Power |
| PCF8574 | I2C | 0x20 | 8-bit GPIO Expander |
| MCP23017 | I2C | 0x20 | 16-bit GPIO Expander |
| MAX31855 | SPI | — | Thermocouple Temperature |
| DS18B20 | 1-Wire | — | Digital Temperature |
| SSD1306 | I2C | 0x3C | 128×64 OLED Display |

---

## Getting Started

### Prerequisites

- Rust toolchain (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- An MQTT broker (e.g., Mosquitto): `brew install mosquitto` or `apt install mosquitto`
- An LLM API key (e.g., OpenAI, Anthropic, or a local Ollama instance)

### Installation

```bash
# Clone the repository
git clone https://github.com/thewriterben/Oh-Ben-Claw.git
cd Oh-Ben-Claw

# Build the core agent
cargo build --release --features hardware,mqtt-spine

# Run the setup wizard
./target/release/oh-ben-claw setup
```

### Configuration

Oh-Ben-Claw uses a TOML configuration file at `~/.oh-ben-claw/config.toml`. The setup wizard will guide you through the initial configuration.

```toml
[agent]
name = "Oh-Ben-Claw"
system_prompt = "You are Oh-Ben-Claw, an advanced multi-device AI assistant."

[provider]
name = "openai"
model = "gpt-4o"
api_key = "sk-..."

[spine]
kind = "mqtt"
host = "localhost"
port = 1883

[peripherals]
enabled = true
datasheet_dir = "docs/datasheets"

# Example: Waveshare ESP32-S3 connected via serial
[[peripherals.boards]]
board = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "serial"
path = "/dev/ttyUSB0"
baud = 115200

# Example: NanoPi Neo3 running natively
[[peripherals.boards]]
board = "nanopi-neo3"
transport = "native"

[channels.telegram]
token = "your-telegram-bot-token"

# Optional: supervised mode — require approval before running tools
[autonomy]
level = "supervised"         # "full" (default), "supervised", or "manual"
auto_approve = ["read_file"] # tools that never need approval
always_ask = ["delete_file"] # tools that always need approval

# Optional: token cost tracking and budget enforcement
[cost]
enabled = true
daily_limit_usd = 5.0
monthly_limit_usd = 50.0
warn_threshold = 0.8

# Optional: sandboxed tool execution
[runtime]
kind = "native"              # "native" (default) or "docker"

# Optional: multimodal image handling
[multimodal]
enabled = true
max_images = 5
max_image_bytes = 5242880    # 5 MB
allow_remote = false

# Optional: HTTP proxy for restricted networks (Phase 11 — MimiClaw parity)
[proxy]
enabled = false
host    = "10.0.0.1"
port    = 7897
kind    = "http"             # "http" (default) or "socks5"

# Optional: Feishu/Lark channel (Phase 11 — MimiClaw parity)
[channels.feishu]
app_id             = "cli_xxxxxxxxxxxxxx"
app_secret         = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
verification_token = "your-verification-token"
webhook_port       = 18790
```

### Personality Files (Phase 11)

Instead of editing `system_prompt` in `config.toml`, you can drop plain Markdown
files in `~/.oh-ben-claw/`:

| File | Purpose |
|------|---------|
| `SOUL.md` | Agent personality — overrides `[agent].system_prompt` when present |
| `USER.md` | User profile — appended to the system prompt as a "User Profile" section |
| `HEARTBEAT.md` | Proactive task list — uncompleted items trigger the agent automatically |

Example `~/.oh-ben-claw/SOUL.md`:

```markdown
You are Oh-Ben-Claw, a precise and proactive AI assistant running on distributed
hardware. You prefer short, actionable replies. You never make up tool results.
```

Example `~/.oh-ben-claw/HEARTBEAT.md` (the agent checks this on a schedule):

```markdown
# My Tasks

- [ ] Send the weekly status report to the team
- [x] Order replacement fan for the server room   ← completed, agent skips this
- [ ] Book dentist appointment
```

### Running

```bash
# Start the core agent (connects to MQTT spine and all configured peripherals)
./target/release/oh-ben-claw start

# Run system diagnostics to check configuration and connectivity
./target/release/oh-ben-claw doctor

# Or run as a background service
./target/release/oh-ben-claw service install
./target/release/oh-ben-claw service start
```

---

## Firmware

Firmware for peripheral nodes is located in the `firmware/` directory.

### ESP32-S3 (`firmware/obc-esp32-s3`)

The ESP32-S3 firmware exposes GPIO, camera, audio, and sensor capabilities over both a serial JSON protocol and MQTT.

```bash
# Install ESP toolchain
cargo install espup && espup install && source ~/export-esp.sh

# Build and flash
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

### NanoPi Neo3 (`firmware/obc-nanopi`)

The NanoPi Neo3 runs the core Oh-Ben-Claw agent natively, providing GPIO access via Linux sysfs.

```bash
# Cross-compile from host
cargo build --target aarch64-unknown-linux-gnu --features hardware,peripheral-nanopi --release
```

---

## Project Structure

```
Oh-Ben-Claw/
├── src/
│   ├── agent/          # Core agent loop, dispatcher, memory loader
│   ├── approval/       # Human-in-the-loop approval workflow
│   ├── audio/          # Audio pipeline (microphone → STT → agent → TTS)
│   ├── channels/       # Communication channels (Telegram, Discord, Feishu, CLI, etc.)
│   ├── config/         # Configuration schema and loading
│   ├── cost/           # Token cost tracking and budget enforcement
│   ├── dashboard/      # Real-time terminal TUI dashboard
│   ├── doctor/         # System diagnostics (oh-ben-claw doctor command)
│   ├── gateway/        # REST/WebSocket API gateway
│   ├── hooks/          # Event lifecycle hooks
│   ├── mcp/            # Model Context Protocol client/server
│   ├── memory/         # Memory backends (SQLite, Markdown personality, journal, heartbeat)
│   ├── multimodal.rs   # Enhanced multimodal message handling
│   ├── observability/  # Logging, metrics, OpenTelemetry
│   ├── peripherals/    # Hardware peripheral drivers (ESP32-S3, NanoPi, RPi)
│   ├── providers/      # LLM provider adapters (OpenAI, Anthropic, Gemini, Ollama)
│   ├── rag/            # RAG pipeline for hardware datasheet retrieval
│   ├── runtime/        # Sandboxed tool execution (native + Docker)
│   ├── scheduler/      # Scheduled tasks and cron jobs
│   ├── security/       # Sandboxing, pairing, secrets management
│   ├── skill_forge/    # Automatic skill discovery and integration
│   ├── spine/          # MQTT communication spine (publish, subscribe, discovery)
│   ├── tools/          # Tool registry (shell, file, browser, hardware, etc.)
│   ├── tunnel/         # Network tunnels (Cloudflare, ngrok, Tailscale)
│   └── vision/         # Vision pipeline (camera → LLM vision → action)
├── firmware/
│   ├── obc-esp32-s3/   # ESP32-S3 firmware (GPIO + camera + mic + sensors)
│   ├── obc-nanopi/     # NanoPi Neo3 native agent
│   └── obc-rpi/        # Raspberry Pi native agent
├── docs/
│   ├── architecture/   # Architecture design documents
│   └── datasheets/     # Hardware datasheets and pin maps
├── scripts/            # Utility scripts
└── examples/           # Example configurations and use cases
```

---

## Relationship to ZeroClaw

Oh-Ben-Claw is built on top of the `zeroclaw-labs/zeroclaw` architecture. It inherits the core architecture — the agent loop, provider system, channel system, tool registry, and peripheral framework — and extends it with:

- A dedicated MQTT-based communication spine for distributed, network-connected peripheral nodes.
- An expanded hardware ecosystem with more detailed datasheets and firmware for 12+ boards and 13+ I2C/SPI accessories.
- A native GUI application (Tauri 2 + React) for easier management and monitoring.
- A more opinionated, production-ready configuration and deployment story.
- All key features from upstream ZeroClaw: human-in-the-loop approval, token cost tracking, system diagnostics, event lifecycle hooks, multimodal image handling, hardware datasheet RAG, and sandboxed tool execution.
- Oh-Ben-Claw unique features: MQTT spine, P2P broker-free mesh, vision pipeline, audio pipeline, sensor fusion, TUI dashboard, MCP client/server, skill forge, edge-native mode.
- **Phase 11 — Pycoclaw/MimiClaw parity**: personality system (SOUL.md/USER.md), proactive task dispatch (HEARTBEAT.md), daily journal (YYYY-MM-DD.md), HTTP proxy support, and Feishu/Lark channel.

---

## License

MIT — see [LICENSE](LICENSE) for details.

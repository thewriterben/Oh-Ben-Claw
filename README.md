<h1 align="center">Oh-Ben-Claw 🦀🧠</h1>
<p align="center">
  <strong>An embodied AI agent in 100% Rust — perceive, remember, react, act.</strong><br>
  ⚡️ <strong>One brain, many bodies — a safety-bounded control stack that reasons, reflexes, plans, and coordinates a fleet.</strong>
</p>
<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  <a href="https://github.com/thewriterben/Oh-Ben-Claw/actions"><img src="https://img.shields.io/github/actions/workflow/status/thewriterben/Oh-Ben-Claw/ci.yml?branch=main" alt="CI" /></a>
  <a href="https://github.com/thewriterben/Oh-Ben-Claw/releases"><img src="https://img.shields.io/github/v/release/thewriterben/Oh-Ben-Claw?include_prereleases" alt="Release" /></a>
  <img src="https://img.shields.io/badge/tests-1000%2B%20passing-brightgreen" alt="1000+ tests passing" />
  <img src="https://img.shields.io/badge/rust-stable-orange" alt="Rust stable" />
</p>

**Oh-Ben-Claw** is an advanced, distributed, **embodied** AI agent built on the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It began as a multi-device orchestrator — one LLM brain commanding a fleet of hardware peripherals over an MQTT spine — and has grown a full **embodied control stack**: a bitemporal world memory, millisecond reflexes, a predictive foresight layer, deliberative missions and behavior trees, and multi-robot fleet coordination — all bounded by a single uniform safety gate that runs on the host **and** on the microcontroller.

> **Mental model:** Oh-Ben-Claw is the brain. Your ESP32s, NanoPis, and Raspberry Pis are the arms, eyes, and ears. The brain doesn't just *call* the hardware — it perceives the world into memory, reacts reflexively, anticipates what's coming, plans multi-step missions, and keeps every physical action inside hard safety limits.

---

## Table of Contents

- [What Makes It Embodied](#what-makes-it-embodied)
- [Embodied Control Stack](#embodied-control-stack)
  - [World Memory (the substrate)](#world-memory-the-substrate)
  - [Track 0 — the safety gate](#track-0--the-safety-gate)
  - [The four control modes](#the-four-control-modes)
  - [Subsystem suites](#subsystem-suites)
  - [Navigation, SLAM & autonomy](#navigation-slam--autonomy)
- [Architecture Overview](#architecture-overview)
- [Platform Features](#platform-features)
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

## What Makes It Embodied

Most "AI agents" are a chat loop wrapped around tool calls. Oh-Ben-Claw is built like a robot: perception flows into a persistent world model, fast reflexes guard the body without waiting for the LLM, and every command that touches the physical world passes a hard safety check first.

The whole stack is one loop — **perceive → remember → react → act** — over a single shared substrate:

```
   perceive            remember              react / anticipate / plan          act
 ┌───────────┐      ┌──────────────┐      ┌───────────────────────────┐   ┌──────────────┐
 │ sensors   │      │              │      │  reflexes   (System 1)    │   │  Track 0     │
 │ cameras   │ ───► │   World      │ ───► │  foresight  (Track 1)     │──►│  safety gate │──► actuators
 │ mics      │      │   Memory     │      │  missions   (deliberative)│   │ (host + MCU) │
 │ links     │      │ (bitemporal) │      │  fleet      (coordinated) │   └──────────────┘
 └───────────┘      └──────────────┘      └───────────────────────────┘
```

It is benchmarked component-by-component against the robotics state of the art (ROS 2 Nav2, slam_toolbox / Cartographer, AMCL, BehaviorTree.CPP, Open-RMF) in [`docs/SOTA-COMPARISON.md`](docs/SOTA-COMPARISON.md), and the architecture is documented in [`docs/EMBODIED-ARCHITECTURE.md`](docs/EMBODIED-ARCHITECTURE.md).

---

## Embodied Control Stack

### World Memory (the substrate)

`src/memory/world` is a **bitemporal**, append-only store of facts about the world. Every observation carries both a *valid time* (when it was true) and a *transaction time* (when the brain learned it), so the agent can answer not just "what is the battery now?" but "what did we believe the battery was at 12:04, and when did we find out?". Every subsystem writes here (`sensor.*`, `power.mode`, `nav.pose`, `vision.subject.*`, …) and every control layer reads from here — it is the one source of truth the entire stack composes on. `observe` / `current` / `history` / `at` are exposed to the LLM through the `world_memory` tool.

### Track 0 — the safety gate

Every physical action — moving a servo, driving a motor, toggling a GPIO — passes through `SafetyGate` (`src/security/limits`) **before** it reaches hardware. A `SafetyLimit` constrains the allowed pins/channels, the value range, and the command rate; a `RiskClass` marks each tool `safe` or `physical { reversible, blast_radius }`, and high-blast physical actions require per-call human approval. The same gate logic is mirrored in the ESP32-S3 firmware, so a node protects itself even if the host link drops. Nothing actuates that the gate hasn't cleared.

### The four control modes

All four run on the world-memory substrate and dispatch through the same Track 0 gate — they differ only in *what triggers them*:

| Mode | Layer | Reacts to | Lives in |
|---|---|---|---|
| **Reactive** | Reflexes (System 1) | the present — a fact crosses a condition *now* | `src/agent/reflex`, `src/agent/safing` |
| **Anticipatory** | Foresight (Track 1) | the *predicted* future — a trend will cross a threshold | `src/foresight` |
| **Deliberative** | Missions & behavior trees | a multi-step plan with guards | `src/mission` |
| **Coordinated** | Fleet | many robots sharing work | `src/fleet` |

**Reflexes** (`src/agent/reflex`) evaluate conditions (`Sensor`, `GpioEq`, categorical `State`, `And`/`Or`) against world memory every tick and fire actions (`GpioWrite`, `Publish`, `Escalate`, `Move`) with debounce and an escalation budget — System 1, no LLM in the loop. The **safing** library (`src/agent/safing`) adds canonical self-protection rules (battery critical → escalate + Track 0 stop; battery low → shed load; link offline; audio alarm; out-of-range sensor; overheat) that *recover automatically* when conditions normalize.

**Foresight** (`src/foresight`) fits a trend over an entity's recent history and fires *before* the event — `battery ≤ 10% within 60s → return to base` triggers while the pack is still at 20% but draining fast. The forecaster supports exponentially-weighted (online) regression, so it tracks regime changes instead of lagging behind them.

**Missions** (`src/mission`) execute a guarded sequence of steps (`navigate_to` / `wait` / `speak` / `record` / `await_state`); a **behavior-tree engine** (`src/mission/bt`) adds a full declarative grammar (sequence / reactive-sequence / fallback / parallel / decorators) reusing the same actions and conditions. Guards **preempt and halt** the body when a bad mode appears.

**Self-authored reflexes** (`src/learning`) mine history for antecedents that repeatedly preceded a bad outcome and *propose* new rules with support/confidence — but a proposal only goes live through an explicit **approval gate**, after which it joins the foresight engine's shared rule buffer on the next tick.

### Subsystem suites

Five capability suites share one contract (perceive → remember → act; see [`docs/SUBSYSTEM-SUITE-CONTRACT.md`](docs/SUBSYSTEM-SUITE-CONTRACT.md)), each recording to world memory and exposing gated MCP tools:

| Suite | Module | Perceives / Acts | Mode hook |
|---|---|---|---|
| **Sensing** | `src/sensing` | classifies samples vs range/freshness specs → `sensor.{quantity}` with quality | `quality` |
| **Audio** | `src/audio/suite` | hears (reliability-classified events) and speaks (pluggable TTS / spine sink) | `audio.*` |
| **Power** | `src/power` | battery SoC + charge state → `power.mode` (`normal`/`low`/`critical`/`charging`) | `power.mode` |
| **Comms** | `src/comms` | per-link health → aggregated `net.mode` (`online`/`degraded`/`offline`) | `net.mode` |
| **Movement** | `src/movement` | Track 0–bounded actuation + closed-loop P-controller servo | — |

### Navigation, SLAM & autonomy

`src/navigation` is a full localization → mapping → planning → driving column, SOTA-aligned and bounded by Track 0:

- **Localization** — multi-source pose fusion (circular-mean heading) and a **particle filter** with **KLD-adaptive** sample count (≈ AMCL) carrying an honest position spread.
- **Sensor model** — a **likelihood-field** range-finder model (Thrun §6.4) over a chamfer distance transform; scan updates reweight the belief by how well a pose explains the beams.
- **SLAM** — pose-graph back end (SE2) with loop closure, solved by anchored Gauss-Seidel relaxation **and** a Gauss-Newton least-squares optimizer (≈ slam_toolbox / Cartographer back ends); writes the corrected pose to memory.
- **Mapping** — online occupancy grid built from Bresenham ray-cast scans (sticky obstacles).
- **Planning** — A* over the grid plus a **costmap inflation** layer (inscribed/inflation radii, clearance-aware cost ≈ Nav2) so paths keep a safety margin and refuse gaps narrower than the robot.
- **Autonomy** — frontier detection + nearest-reachable selection lets a robot explore an unknown space on its own.

**Fleet coordination** (`src/fleet`) sits above a swarm of these: nodes heartbeat their state over MQTT, and a `Coordinator` allocates tasks — by nearest-idle node or a **market-based sequential auction** (globally cheaper, queue-order-independent) — with spatial conflict avoidance and coordinated multi-robot exploration.

---

## Architecture Overview

Oh-Ben-Claw is organized around three physical layers; the embodied control stack runs inside the Brain and is mirrored, in miniature, on the microcontrollers.

| Layer | Component | Description |
|---|---|---|
| **Brain** | Core Agent | The LLM reasoning engine **plus** the embodied control stack (world memory, reflexes, foresight, missions, navigation, fleet), running on a host machine. |
| **Spine** | MQTT / P2P | The unified communication backbone. Devices publish capabilities, state heartbeats, and receive commands and safing advisories over topics. |
| **Appendages** | Peripheral Nodes | Firmware on ESP32-S3, NanoPi Neo3, Raspberry Pi, and more. Each exposes its capabilities as tools — and an ESP32-S3 node runs its own on-MCU reflex + safing mirror. |

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Oh-Ben-Claw Core Agent (Host: macOS / Linux / Windows)                      │
│                                                                              │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────────────┐  │
│  │  Channels   │──►│  Agent Loop  │──►│  Unified Tool Registry           │  │
│  │  Telegram   │   │  (LLM calls) │   │  (local + peripheral + browser)  │  │
│  │  Discord    │   └──────┬───────┘   └──────────────────────────────────┘  │
│  │  CLI / GUI  │          │                                                  │
│  └─────────────┘          ▼                                                  │
│                  ┌────────────────────────────────────────────────────────┐ │
│                  │  Embodied Control Stack                                 │ │
│                  │  World Memory ◄─ suites ─► reflexes · foresight ·       │ │
│                  │  missions/BT · navigation/SLAM · fleet  ─► Track 0 gate │ │
│                  └───────────────────────┬────────────────────────────────┘ │
│                    ┌───────────────┐     │     ┌──────────────┐              │
│                    │  MQTT Spine   │◄────┘     │  Deployment  │              │
│                    └──────┬────────┘           │  Planner     │              │
└───────────────────────────┼────────────────────└──────────────┘────────────┘
                            │
          ┌─────────────────┼─────────────────┐
          ▼                 ▼                 ▼
┌─────────────────┐ ┌─────────────┐ ┌─────────────────┐
│ ESP32-S3 Node   │ │ NanoPi Neo3 │ │ Raspberry Pi    │
│ - camera/audio  │ │ - gpio/i2c  │ │ - gpio/camera   │
│ - sensors/gpio  │ │ - spi       │ │ - audio         │
│ - reflex+safing │ └─────────────┘ └─────────────────┘
│   (on-MCU)      │
└─────────────────┘
```

---

## Platform Features

The capabilities that the embodied stack rides on — orchestration, I/O, providers, and operations.

**Multi-Device Orchestration** lets a single agent command a fleet of hardware nodes simultaneously. Each node registers its capabilities dynamically over the MQTT spine, and the central agent merges all available tools into a single unified registry.

**MQTT Communication Spine** replaces direct serial-only connections with a scalable publish-subscribe model, so devices can live anywhere on the network (or the internet via a tunnel) and be added or removed without restarting the core agent. A **P2P broker-free mesh** lets nodes discover each other directly via mDNS + TCP.

**Multi-Modal I/O** provides a unified interface to the physical world: see (cameras on ESP32-S3 / Raspberry Pi), hear (I2S mics or the Sipeed 6+1 array), sense (I2C/SPI sensors like BME280, MPU6050, DHT22), and act (GPIO / actuators).

**Vision Pipeline** connects camera peripherals to vision-capable LLMs (capture → encode → model in one turn). The **ClawCam** vision subsystem folds AI detections into world memory (`vision.subject.*`) so the brain remembers what each camera saw and when.

**Audio Pipeline** connects microphones to speech-to-text (OpenAI Whisper or local `whisper.cpp`) and synthesizes spoken replies via TTS.

**Sensor Fusion** combines readings from multiple sensors into a unified structure (averaging, median, min/max, weighted, and a simple Kalman filter).

**Browser Automation** drives Chrome via the DevTools Protocol with seven tools (`browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_scroll`, `browser_new_tab`, `browser_close_tab`); falls back to plain HTTP fetch when no CDP endpoint is reachable.

**Pluggable LLM Providers** support OpenAI, Anthropic, Google Gemini, Ollama (local), OpenRouter, and any OpenAI-compatible endpoint, with **model failover** chains and exponential-backoff **retry** for transient errors.

**Rich Communication Channels** — Telegram, Discord, Slack, WhatsApp, iMessage, IRC, Matrix, Signal, Mattermost, Feishu/Lark, and a built-in CLI, with typing indicators and a native GUI (Tauri 2 + React).

**Human-in-the-Loop Approval** provides supervised execution with three autonomy levels (`full` / `supervised` / `manual`), a session-scoped allowlist, and a full audit log — and it is what gates high-blast physical actions from the embodied stack.

**Deployment Scheme Generator** analyses your hardware inventory, maps capabilities to agent roles, identifies gaps, and renders a ready-to-use TOML configuration (optionally refined by an LLM-powered planning swarm).

**Skill Forge & ClawHub** discover, vet, and install community skills, with a security install policy (consent, allowlist, version pinning, SHA-256 checksums, static manifest inspection, JSONL audit log). Synthesized physical skills are quarantined behind Track 0 until approved.

**MCP Integration** exposes all tools as a Model Context Protocol server (stdio + HTTP/SSE, dual-mode for the 2026 spec) and imports tools from external MCP servers. **A2A Protocol** implements Google's Agent-to-Agent v1.0 for cross-platform agent interop.

**Operations** — `oh-ben-claw doctor` health checks (now including subsystem/safing coherence), real-time **TUI dashboard** (`--features dashboard`), token **cost tracking** with persistent budgets, **observability** (metrics + spans), event lifecycle **hooks**, scheduled tasks, encrypted secrets **vault**, node **pairing** (HMAC-SHA256), tamper-evident **audit chain**, and sandboxed tool execution (`native` / `docker` / `wasm` runtimes).

**Personality & Memory** — editable `SOUL.md` / `USER.md` personality files, proactive `HEARTBEAT.md` task dispatch, a human-readable daily journal, a vector store for RAG, and a hardware-datasheet RAG (`datasheet_search`).

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

### Accessories

Sensors and I/O over I2C / SPI / 1-Wire / GPIO — including BME280, BMP388, AHT20, MPU6050, LSM6DS3, SHT31, ADS1115, MCP4725, INA260, PCF8574, MCP23017, MAX31855, DS18B20, SSD1306, DHT22/DHT11 — plus **embodied actuation & power** accessories added for the control stack: **SG90** servo, **TB6612FNG** / **PCA9685** motor & PWM drivers, **INMP441** mic, **MAX98357A** amp, **MAX17048** fuel gauge, and **SIM7600** cellular. The full machine-readable list is the registry single-source-of-truth (`src/peripherals/registry.rs` → `registry.json`).

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

Oh-Ben-Claw uses a TOML configuration file at `~/.oh-ben-claw/config.toml`. The setup wizard guides you through the basics; see [`examples/config-multi-device.toml`](examples/config-multi-device.toml) for a full annotated example.

### Core

```toml
[agent]
name = "Oh-Ben-Claw"
system_prompt = "You are Oh-Ben-Claw, an advanced embodied AI assistant."
max_tool_iterations = 15

[provider]
name = "openai"
model = "gpt-4o"
# api_key = "sk-..."  # Or set OPENAI_API_KEY

[[provider.fallbacks]]            # tried in order when primary fails
name  = "anthropic"
model = "claude-3-5-sonnet-20241022"

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

[[peripherals.boards]]
board     = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "serial"
path      = "/dev/ttyUSB0"
baud      = 115200

# Supervised mode — high-blast physical actions require approval
[autonomy]
level        = "supervised"     # "full" (default), "supervised", or "manual"
auto_approve = ["sensor_read"]
always_ask   = ["movement"]
```

### Embodied control stack

Each layer is opt-in. Turn on what your body has; the safety gate is always enforced for physical tools.

```toml
# Reflexes (System 1) + self-healing safing rules
[reflex]
enabled = true
safing  = true                     # battery / link / sensor / overheat self-protection

# Track 0 stop channel asserted on battery-critical
[reflex.safing_stop_actuator]
name    = "drive"
channel = 1

# Safety-bounded movement (Track 0)
[movement]
enabled = true

# Capability suites
[sensing]
enabled = true
[[sensing.quantity]]
name = "temperature"
min  = -10.0
max  = 85.0
max_staleness_ms = 30000

[power]
enabled = true        # battery SoC + charge state → power.mode

[comms]
enabled = true        # per-link health → net.mode

[audio_suite]
enabled = true        # hear (reliability-classified) + speak (TTS / spine)

# Navigation: localization → mapping → planning → driving
[navigation]
enabled    = true
explore    = false    # true → autonomously map unknown space via frontiers
inscribed_radius  = 0.25
inflation_radius  = 0.6

# Deliberative missions (named library) + behavior trees
[mission]
enabled = true

# Foresight (Track 1) — act before the event
[foresight]
enabled = true

# Self-authored reflexes (proposals are approval-gated)
[learning]
enabled = true
# auto_mine_interval_ms = 60000   # set to auto-propose rules on a cadence

# Fleet coordination (one brain, many bodies)
[fleet]
enabled = true
```

Channels, browser, ClawHub, cost, runtime, multimodal, and proxy sections are unchanged — see [`examples/config-multi-device.toml`](examples/config-multi-device.toml).

---

## Personality Files

Instead of editing `system_prompt` in `config.toml`, drop plain Markdown files in `~/.oh-ben-claw/`:

| File | Purpose |
|------|---------|
| `SOUL.md` | Agent personality — overrides `[agent].system_prompt` when present |
| `USER.md` | User profile — appended to the system prompt as a "User Profile" section |
| `HEARTBEAT.md` | Proactive task list — uncompleted items trigger the agent on a schedule |

**`~/.oh-ben-claw/HEARTBEAT.md`** example (the agent checks this on a schedule):
```markdown
# My Tasks

- [ ] Send the weekly status report to the team
- [x] Order replacement fan for the server room   ← completed, agent skips this
- [ ] Book dentist appointment
```

---

## Browser Automation

Oh-Ben-Claw includes a full browser automation layer driven by the Chrome DevTools Protocol (CDP). Start Chrome in remote-debugging mode and set the `[browser]` config section:

```bash
google-chrome --remote-debugging-port=9222 --headless
```

The tools `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_scroll`, `browser_new_tab`, and `browser_close_tab` are registered automatically. Set `OBC_BROWSER_CDP_URL` to override the CDP endpoint at runtime.

---

## Deployment Scheme Generator

Given a list of available hardware and desired features, the deployment planner generates a complete multi-agent topology and TOML configuration.

```toml
[deployment]
enabled         = true
scenario        = "My Home Hub"
auto_plan       = true
llm_swarm       = false
feature_desires = ["vision", "listening", "environmental_sensing", "display_output"]

[[deployment.hardware]]
name       = "nanopi-neo3"
board_name = "nanopi-neo3"
transport  = "native"
role       = "host"
accessories = ["dht22"]
```

See [`examples/config-nanopi-deployment.toml`](examples/config-nanopi-deployment.toml) for the full NanoPi Neo3 reference deployment covering all five hardware roles.

---

## CLI Reference

```
oh-ben-claw <COMMAND>

Commands:
  start      Start the agent and connect to all configured peripherals and channels
  setup      Interactive setup wizard — creates ~/.oh-ben-claw/config.toml
  doctor     Run system diagnostics (config, connectivity, channels, spine, subsystems/safing)
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

Firmware for peripheral nodes lives in `firmware/`.

### ESP32-S3 (`firmware/obc-esp32-s3`)

Exposes GPIO, camera, audio, and sensor capabilities over a serial JSON protocol and MQTT, plus an edge-native agent loop with Wi-Fi + cloud LLM. It also runs an **on-MCU reflex + safing mirror**: the node derives its own power mode from `sensor.battery_soc`, watches a link-silence watchdog, and self-safes (enters a protective mode / reports `power_mode` + `link_state`) even if the host is unreachable — the Track 0 philosophy enforced down at the body.

```bash
# Install the ESP-IDF / Rust toolchain
cargo install espup && espup install && source ~/export-esp.sh

# Build and flash (from the firmware/obc-esp32-s3 directory)
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

Works with the Waveshare ESP32-S3 Touch LCD 2.1, Seeed XIAO ESP32S3-Sense, and generic ESP32-S3 boards.

### NanoPi Neo3 / Raspberry Pi

Both can run the full Oh-Ben-Claw agent natively (GPIO via sysfs / `rppal`, native I2C/SPI, `libcamera` capture):

```bash
cargo build --target aarch64-unknown-linux-gnu --features hardware,peripheral-nanopi --release
# or --features hardware,peripheral-rpi
```

---

## Native GUI

A native desktop application is included in `gui/`, built with Tauri 2 + React 18 + TypeScript + TailwindCSS.

```bash
npm install -g pnpm
cd gui && pnpm install && pnpm tauri dev      # development
cd gui && pnpm tauri build                    # production build
```

See [`gui/README.md`](gui/README.md) for the full build instructions and command reference.

---

## Project Structure

```
Oh-Ben-Claw/
├── src/
│   ├── agent/          # Agent loop, dispatcher, Reflexion, Plan-and-Execute,
│   │   ├── reflex.rs   #   dual-system reflexes (System 1)
│   │   └── safing.rs   #   self-healing safing rule library
│   ├── memory/
│   │   └── world.rs    # Bitemporal world memory (the embodied substrate)
│   ├── security/
│   │   └── limits.rs   # Track 0 SafetyGate (host) — mirrored in firmware
│   ├── sensing/        # Sensing suite (sample → quality-classified facts)
│   ├── audio/          # Audio pipeline + audio suite (hear / speak)
│   ├── power/          # Power suite (battery + charge → power.mode)
│   ├── comms/          # Comms suite (link health → net.mode)
│   ├── movement/       # Track 0–bounded actuation + closed-loop feedback
│   ├── navigation/     # Localization, SLAM, mapping, A*+costmap, particle filter,
│   │                   #   sensor model, frontier exploration
│   ├── mission/        # Mission sequencer + behavior-tree engine
│   ├── foresight/      # Predictive control (Track 1) + online forecaster
│   ├── learning/       # Self-authored reflexes (mine → approve → activate)
│   ├── fleet/          # Multi-robot coordination (registry, auction, conflicts)
│   ├── approval/       # Human-in-the-loop approval workflow
│   ├── channels/       # Telegram, Discord, Feishu, IRC, Signal, Matrix, …
│   ├── config/         # Configuration schema and loading (Config::validate)
│   ├── cost/           # Token cost tracking and budget enforcement
│   ├── dashboard/      # Real-time terminal TUI (--features dashboard)
│   ├── deployment/     # Hardware-driven deployment scheme generator
│   ├── doctor/         # System diagnostics (oh-ben-claw doctor)
│   ├── gateway/        # REST/WebSocket API gateway (Axum)
│   ├── hooks/          # Event lifecycle hooks
│   ├── mcp/            # Model Context Protocol client/server (dual-mode)
│   ├── observability/  # Metrics, spans, OpenTelemetry
│   ├── peripherals/    # Hardware drivers + registry SSOT
│   ├── providers/      # LLM provider adapters + failover + retry
│   ├── rag/            # Datasheet retrieval
│   ├── runtime/        # Sandboxed tool execution (native + docker + wasm)
│   ├── a2a/            # A2A protocol client and server
│   ├── scheduler/      # Scheduled tasks and cron jobs
│   ├── security/       # Policy, pairing, vault, audit chain, Track 0 limits
│   ├── skill_forge/    # Skill discovery, synthesis, ClawHub registry
│   ├── spine/          # MQTT spine + P2P broker-free mesh
│   ├── tools/          # Tool registry (shell, file, browser, hardware, nav, …)
│   ├── tunnel/         # Network tunnels (Cloudflare, ngrok, Tailscale)
│   └── vision/         # Vision pipeline + ClawCam detection ingest
├── firmware/
│   ├── obc-esp32-s3/   # ESP32-S3 firmware + on-MCU reflex/safing mirror
│   ├── obc-nanopi/     # NanoPi Neo3 native agent
│   └── obc-rpi/        # Raspberry Pi native agent
├── gui/                # Tauri 2 + React 18 native desktop application
├── docs/
│   ├── EMBODIED-ARCHITECTURE.md     # The embodied control stack, end to end
│   ├── SOTA-COMPARISON.md           # Component-by-component vs robotics SOTA
│   ├── SUBSYSTEM-SUITE-CONTRACT.md  # The perceive→remember→act suite contract
│   ├── architecture/   # Architecture design documents
│   └── datasheets/     # Hardware datasheets and pin maps
├── examples/           # Annotated reference configurations
└── tests/              # Integration tests (embodied_full_stack, embodied_hil_loop, …)
```

---

## Relationship to ZeroClaw

Oh-Ben-Claw is built on the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It inherits the core agent loop, provider system, channel system, tool registry, and peripheral framework — and extends it into a distributed, embodied platform:

| Capability | ZeroClaw | Oh-Ben-Claw |
|---|---|---|
| Communication | Direct serial / native GPIO | MQTT spine + P2P mesh + serial / native |
| Tool discovery | Static configuration | Dynamic via node announcements |
| Multi-device | Multiple boards, direct connections | Fleet of nodes over network |
| **World memory** | ✗ | ✅ Bitemporal (valid + transaction time) |
| **Safety gate (Track 0)** | ✗ | ✅ Host + firmware, per-call approval |
| **Reflexes / safing** | ✗ | ✅ System 1 + self-healing recovery |
| **Foresight (predictive)** | ✗ | ✅ Trend + online forecaster |
| **Missions / behavior trees** | ✗ | ✅ Guarded sequencer + BT engine |
| **Navigation / SLAM** | ✗ | ✅ Particle filter, pose-graph SLAM, A*+costmap |
| **Self-authored reflexes** | ✗ | ✅ Mine → approve → activate |
| **Fleet coordination** | ✗ | ✅ Auction allocation + conflict avoidance |
| Browser automation | ✗ | ✅ CDP (7 tools) |
| Vision / audio / fusion | ✗ | ✅ |
| Deployment planner | ✗ | ✅ LLM + rule-based swarm |
| GUI | ✗ | ✅ Tauri 2 + React 18 |
| MCP / A2A | ✗ | ✅ Client + server (both) |
| Human approval | ✗ | ✅ 3 autonomy levels |
| Sandboxes | ✗ | ✅ native / docker / wasm |
| Edge-native mode | ✗ | ✅ (ESP32-S3, NanoPi) |

---

## License

MIT — see [LICENSE](LICENSE) for details.

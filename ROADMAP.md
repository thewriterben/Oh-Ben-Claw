# Oh-Ben-Claw Roadmap

This document outlines the planned development phases for Oh-Ben-Claw.

## Phase 1: Foundation (Current)

The initial release establishes the core architecture and demonstrates the key concepts of the system. It includes the MQTT spine design, the peripheral tool registry, the hardware board registry, and the ESP32-S3 and NanoPi Neo3 peripheral drivers.

- [x] Repository structure and Cargo workspace
- [x] MQTT spine protocol design (`src/spine/mod.rs`)
- [x] Hardware board registry with USB VID/PID mappings (`src/peripherals/registry.rs`)
- [x] NanoPi Neo3 GPIO peripheral driver (`src/peripherals/nanopi.rs`)
- [x] ESP32-S3 sensor tools (camera, audio, sensor read) (`src/peripherals/sensors.rs`)
- [x] ESP32-S3 firmware with serial + MQTT support (`firmware/obc-esp32-s3`)
- [x] Configuration schema with MQTT spine and multi-board support (`src/config/mod.rs`)
- [x] CLI with `start`, `status`, `peripheral`, and `service` subcommands (`src/main.rs`)
- [x] Architecture documentation (`docs/architecture/ARCHITECTURE.md`)
- [x] Hardware datasheets (`docs/datasheets/`)
- [x] CI/CD pipeline (`.github/workflows/ci.yml`)

## Phase 2: Core Agent Loop

Implement the full agent loop with LLM integration, memory, and tool execution.

- [ ] LLM provider adapters (OpenAI, Anthropic, Gemini, Ollama)
- [ ] Agent loop with tool-use iterations and history compaction
- [ ] SQLite memory backend
- [ ] Shell, file read/write, and HTTP request tools
- [ ] CLI channel (interactive terminal)
- [ ] Telegram channel

## Phase 3: MQTT Spine Implementation

Implement the full MQTT spine with dynamic tool discovery.

- [ ] `rumqttc`-based MQTT client in `src/spine/mod.rs`
- [ ] Node announcement subscription and dynamic tool registration
- [ ] Tool call request/response over MQTT
- [ ] Node heartbeat monitoring and health tracking
- [ ] Node pairing protocol (HMAC-based authentication)

## Phase 4: Expanded Hardware Ecosystem

Add support for more hardware devices and capabilities.

- [ ] Raspberry Pi GPIO peripheral driver (`src/peripherals/rpi.rs`)
- [ ] Raspberry Pi camera support (via `libcamera`)
- [ ] Arduino serial peripheral driver
- [ ] STM32 Nucleo peripheral driver (via probe-rs)
- [ ] I2C bus scan tool for NanoPi and Raspberry Pi
- [ ] SPI bus tool for NanoPi and Raspberry Pi
- [ ] PWM control tool

## Phase 5: Multi-Channel Support

Add support for all major communication channels.

- [ ] Discord channel
- [ ] Slack channel
- [ ] WhatsApp channel
- [ ] iMessage channel (macOS only)
- [ ] Matrix channel
- [ ] IRC channel

## Phase 6: Native GUI

Build a native desktop application for managing and monitoring the system.

- [ ] System tray application (autorun on startup)
- [ ] Dashboard showing connected peripheral nodes and their status
- [ ] Live tool invocation interface
- [ ] Configuration editor
- [ ] Log viewer

## Phase 7: Edge-Native Mode

Enable peripheral nodes to run the full Oh-Ben-Claw agent locally, without a host.

- [ ] Lightweight agent loop for ESP32-S3 (WiFi + cloud LLM)
- [ ] Lightweight agent loop for NanoPi Neo3 (local Ollama)
- [ ] Peer-to-peer node coordination (without a central broker)

## Phase 8: Advanced Capabilities

- [ ] Vision pipeline (camera capture → LLM vision → action)
- [ ] Audio pipeline (microphone → speech-to-text → agent → text-to-speech)
- [ ] Sensor fusion (combine readings from multiple sensors)
- [ ] Scheduled tasks and cron jobs
- [ ] Skill forge (automatic discovery and integration of new skills)

# Oh-Ben-Claw Architecture

## Overview

Oh-Ben-Claw is a distributed, multi-device AI assistant. It extends the ZeroClaw architecture with a network-based communication spine, enabling a single intelligent agent to orchestrate a fleet of specialized hardware peripherals located anywhere on the local network or internet.

## Design Principles

The system is guided by five core principles.

**Distributed by default** — every component is designed to operate independently and communicate over a network, rather than requiring direct physical connections. Peripheral nodes can be on the same host, across a LAN, or across the internet via a tunnel.

**Capability-driven** — the agent's tool registry is assembled dynamically from the capabilities announced by connected peripheral nodes, rather than being statically configured. New devices join the fleet at runtime without restarting the core agent.

**Transport-agnostic** — peripheral nodes can communicate with the brain over serial, native GPIO, or MQTT, and the agent treats all of them uniformly through the same `Tool` interface.

**Secure by design** — all inter-node communication is authenticated via HMAC-SHA256 pairing tokens. A glob-pattern policy engine controls which tools may be called and under what conditions. An encrypted secrets vault (AES-256-GCM + Argon2id) protects all API keys and credentials.

**Incremental by design** — features compose cleanly. Run Oh-Ben-Claw as a simple CLI chatbot with no peripherals, then add MQTT nodes, channels, and subsystems one at a time as your use case evolves.

---

## System Layers

The system is organized into four distinct layers.

### Layer 1: The Brain (Core Agent)

The core agent is the central reasoning and orchestration engine. It runs on a capable host machine (x86_64 PC, NanoPi Neo3, Raspberry Pi, or NVIDIA Jetson) and is responsible for:

- Maintaining conversational state in a SQLite WAL-mode database
- Interfacing with LLM providers (OpenAI, Anthropic, Gemini, Ollama, OpenRouter, …)
- Routing incoming messages from communication channels to the agent loop
- Resolving and dispatching tool calls to local or remote peripheral nodes
- Writing daily journal entries and responding to proactive HEARTBEAT tasks

### Layer 2: The Spine (MQTT Communication Backbone)

The MQTT spine is the nervous system of the Oh-Ben-Claw system. All communication between the brain and peripheral nodes flows through the spine. The spine uses a hierarchical topic structure:

| Topic Pattern | Direction | Purpose |
|---|---|---|
| `obc/nodes/{node_id}/announce` | Node → Brain | Node announces its capabilities on startup |
| `obc/nodes/{node_id}/heartbeat` | Node → Brain | Periodic liveness beacon |
| `obc/tools/{node_id}/call/{tool}` | Brain → Node | Brain invokes a tool on a node |
| `obc/tools/{node_id}/result/{call_id}` | Node → Brain | Tool call result |
| `obc/broadcast/command` | Brain → All | Global command to all nodes |

**P2P Mesh (optional)** — the `src/spine/p2p.rs` module implements a broker-free mesh network for edge scenarios where no central MQTT broker is available. Nodes discover peers via mDNS and communicate over direct TCP connections.

### Layer 3: The Appendages (Peripheral Nodes)

Peripheral nodes are the sensory and motor organs of the system. Each node runs a lightweight firmware or agent that exposes its hardware capabilities as tools. When a node starts up, it publishes a `NodeAnnouncement` to the MQTT spine. The brain subscribes to these announcements and dynamically registers the node's tools.

### Layer 4: Supporting Subsystems

| Subsystem | Location | Purpose |
|---|---|---|
| Security | `src/security/` | Policy engine, HMAC pairing, encrypted vault |
| Memory | `src/memory/` | SQLite, personality, heartbeat, journal, image, vector |
| Channels | `src/channels/` | Telegram, Discord, Slack, Feishu, IRC, Signal, Matrix, … |
| Providers | `src/providers/` | LLM adapters, failover, retry |
| Tools | `src/tools/` | Shell, file, HTTP, browser, OTA, vision, audio, hardware |
| Deployment | `src/deployment/` | Hardware inventory, planner, swarm (Phase 13) |
| MCP | `src/mcp/` | Model Context Protocol client + server |
| Skill Forge | `src/skill_forge/` | Community skill registry (ClawHub) |
| RAG | `src/rag/` | Hardware datasheet retrieval |
| Vision | `src/vision/` | Camera → LLM vision → action pipeline |
| Audio | `src/audio/` | Microphone → STT → agent → TTS pipeline |
| Dashboard | `src/dashboard/` | Ratatui TUI dashboard (optional feature) |

---

## Component Diagram

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Oh-Ben-Claw Core Agent (Host)                                               │
│                                                                              │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────────────┐  │
│  │  Channels   │──►│  Agent Loop  │──►│  Unified Tool Registry           │  │
│  │  Telegram   │   │  (LLM calls) │   │  (local + peripheral + browser)  │  │
│  │  Discord    │   └──────┬───────┘   └──────────────────────────────────┘  │
│  │  Feishu     │          │                                                  │
│  │  IRC/Signal │          ▼                                                  │
│  │  CLI / GUI  │   ┌───────────────┐   ┌──────────────┐                    │
│  └─────────────┘   │  Spine Client │   │  Deployment  │                    │
│                    └──────┬────────┘   │  Planner     │                    │
└───────────────────────────┼────────────└──────────────┘────────────────────┘
                            │ MQTT
                    ┌───────┴────────┐
                    │  MQTT Broker   │  (Mosquitto / EMQX / HiveMQ)
                    └───────┬────────┘
         ┌─────────────────┼─────────────────┐
         │ MQTT            │ MQTT            │ Serial / Native
         ▼                 ▼                 ▼
┌─────────────────┐ ┌─────────────┐ ┌─────────────────┐
│ ESP32-S3 Node   │ │ NanoPi Neo3 │ │ Raspberry Pi    │
│ (WiFi + MQTT)   │ │ (Native)    │ │ (WiFi + MQTT)   │
│                 │ │             │ │                 │
│ camera_capture  │ │ gpio_read   │ │ gpio_read       │
│ audio_sample    │ │ gpio_write  │ │ gpio_write      │
│ sensor_read     │ │ i2c_scan    │ │ camera_capture  │
│ gpio_read/write │ └─────────────┘ │ audio_sample    │
└─────────────────┘                 └─────────────────┘
```

---

## Node Lifecycle

A peripheral node follows a well-defined lifecycle:

1. **Boot** — the node powers on and initializes its hardware peripherals.
2. **Connect** — the node connects to the WiFi network and the MQTT broker (or P2P peers).
3. **Announce** — the node publishes a `NodeAnnouncement` to `obc/nodes/{node_id}/announce` describing its board type, firmware version, and capability list.
4. **Heartbeat** — the node publishes a heartbeat every 30 seconds to prove liveness.
5. **Listen** — the node subscribes to `obc/tools/{node_id}/call/+` and waits for tool call requests.
6. **Execute** — when a tool call request arrives, the node executes the tool and publishes the result to `obc/tools/{node_id}/result/{call_id}`.
7. **Disconnect** — when the node powers off, its MQTT last-will message signals departure to the brain, which removes the node's tools from the registry.

---

## Tool Call Flow

The following sequence describes how the brain invokes a tool on a peripheral node:

1. The user sends a message to the agent via a channel (e.g., "Take a photo with the kitchen camera").
2. The agent's LLM decides to invoke the `camera_capture` tool on the `esp32-s3-kitchen` node.
3. The agent generates a unique `call_id` and publishes a `ToolCallRequest` to `obc/tools/esp32-s3-kitchen/call/camera_capture`.
4. The ESP32-S3 node receives the request, captures a JPEG image, and publishes a `ToolCallResult` to `obc/tools/esp32-s3-kitchen/result/{call_id}`.
5. The agent receives the result, decodes the base64 JPEG, and returns it to the user.

---

## Security Model

### MQTT Authentication

The MQTT spine supports TLS (via `rumqttc`'s `rustls` backend) and username/password authentication. All credentials are stored in the encrypted secrets vault, not in the config file.

### Node Pairing

Before a peripheral node's tools are accepted into the brain's registry, the node must complete a pairing handshake:

1. The brain generates a random 256-bit pairing secret and shares it with the operator via a QR code or CLI prompt.
2. The node signs its `NodeAnnouncement` with an HMAC-SHA256 tag using the shared secret.
3. The brain verifies the tag and checks a 5-minute replay window to prevent replay attacks.
4. Nodes that fail verification are quarantined and their tools are not registered.

Pairing secrets must be at least 16 characters. The `NodePairingManager::validate_secret()` method enforces this at startup.

### Tool Policy Engine

The policy engine (`src/security/policy.rs`) evaluates every tool call against a list of rules:

- Rules match tool names using glob patterns (e.g., `shell*`, `gpio_write`).
- Rules can inspect argument values with `arg_contains` filters.
- Actions: `allow`, `deny`, or `audit` (log and allow).
- The glob matcher has a maximum recursion depth of 64 to prevent ReDoS attacks.

### Secrets Vault

The encrypted secrets vault (`src/security/vault.rs`) stores API keys and other credentials:

- Encryption: AES-256-GCM with a 96-bit nonce.
- Key derivation: Argon2id with a random 16-byte salt.
- Storage: SQLite WAL-mode database at `~/.oh-ben-claw/vault.db`.
- The vault is locked at startup and requires the master password to unlock.

---

## Deployment Subsystem (Phase 13)

The deployment subsystem (`src/deployment/`) generates a complete multi-agent topology from a `HardwareInventory`:

1. **`HardwareInventory`** — describes available boards, accessories, and desired features (`FeatureDesire` enum: Vision, Listening, Speech, EnvironmentalSensing, DisplayOutput, …).
2. **`HardwareAdvisor`** — gap analyser that checks which features are satisfied and suggests specific boards from the registry for missing capabilities.
3. **`DeploymentPlanner`** — deterministic rule-based planner that maps hardware to agent roles (`Orchestrator`, `VisionAgent`, `AudioAgent`, `SensingAgent`, …) and renders a complete TOML configuration.
4. **`DeploymentSwarm`** — optional LLM-powered multi-agent swarm with three specialised sub-agents (hardware-advisor, architect, requirements-checker) that refine the planner output.

The planner is entirely offline and deterministic. The swarm requires an active LLM provider.

---

## Relationship to ZeroClaw

Oh-Ben-Claw is built on top of the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. The following table summarizes the key differences:

| Feature | ZeroClaw | Oh-Ben-Claw |
|---|---|---|
| Communication | Direct serial / native GPIO | MQTT spine + P2P mesh + serial / native |
| Tool discovery | Static configuration | Dynamic via node announcements |
| Multi-device | Multiple boards, direct connections | Fleet of nodes over network |
| Browser automation | ✗ | ✅ CDP (7 tools) |
| Image memory | ✗ | ✅ SQLite-backed |
| Deployment planner | ✗ | ✅ Rule-based + LLM swarm |
| GUI | ✗ | ✅ Tauri 2 + React 18 |
| Node pairing | ✗ | ✅ HMAC-SHA256 |
| MCP integration | ✗ | ✅ Client + server |
| Vision pipeline | ✗ | ✅ |
| Audio pipeline | ✗ | ✅ |
| Sensor fusion | ✗ | ✅ |
| TUI dashboard | ✗ | ✅ (Ratatui) |
| Personality files | ✗ | ✅ SOUL.md / USER.md |
| Proactive tasks | ✗ | ✅ HEARTBEAT.md |
| ClawHub registry | ✗ | ✅ |
| Model failover | ✗ | ✅ |
| Retry policy | ✗ | ✅ Exponential backoff |
| Human approval | ✗ | ✅ 3 autonomy levels |
| Cost tracking | ✗ | ✅ Daily/monthly budgets |
| Docker sandbox | ✗ | ✅ |
| Reflexion / Plan-and-Execute | ✗ | ✅ |
| Edge-native mode | ✗ | ✅ (ESP32-S3, NanoPi) |

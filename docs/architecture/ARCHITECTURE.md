# Oh-Ben-Claw Architecture

## Overview

Oh-Ben-Claw is a distributed, multi-device AI assistant. It extends the ZeroClaw architecture with a network-based communication bus, enabling a single intelligent agent to orchestrate a fleet of specialized hardware peripherals located anywhere on the local network or internet.

## Design Principles

The system is guided by four core principles. **Distributed by default** means that every component is designed to operate independently and communicate over a network, rather than requiring direct physical connections. **Capability-driven** means that the agent's tool registry is assembled dynamically from the capabilities announced by connected peripheral nodes, rather than being statically configured. **Transport-agnostic** means that peripheral nodes can communicate with the brain over serial, native GPIO, or MQTT, and the agent treats all of them uniformly. **Secure by design** means that all inter-node communication is authenticated, and peripheral nodes are paired before their tools are accepted into the registry.

## System Layers

The system is organized into three distinct layers, each with a clear responsibility.

### Layer 1: The Brain (Core Agent)

The core agent is the central reasoning and orchestration engine. It runs on a capable host machine and is responsible for maintaining conversational state, interfacing with LLMs, making high-level decisions, and delegating tasks to peripheral nodes. The brain never directly controls hardware; it always delegates to the appropriate peripheral node via the communication bus.

### Layer 2: The Bus (MQTT Communication Backbone)

The MQTT bus is the nervous system of the Oh-Ben-Claw system. All communication between the brain and peripheral nodes flows through the bus. The bus uses a hierarchical topic structure to organize messages:

| Topic Pattern | Direction | Purpose |
|---|---|---|
| `obc/nodes/{node_id}/announce` | Node → Brain | Node announces its capabilities on startup |
| `obc/nodes/{node_id}/heartbeat` | Node → Brain | Node sends a periodic heartbeat |
| `obc/tools/{node_id}/call/{tool}` | Brain → Node | Brain invokes a tool on a node |
| `obc/tools/{node_id}/result/{call_id}` | Node → Brain | Node returns the tool call result |
| `obc/broadcast/command` | Brain → All | Brain sends a command to all nodes |

### Layer 3: The Appendages (Peripheral Nodes)

Peripheral nodes are the sensory and motor organs of the system. Each node runs a lightweight firmware or agent that exposes its hardware capabilities as tools. When a node starts up, it publishes a `NodeAnnouncement` to the MQTT bus describing its capabilities. The brain subscribes to these announcements and dynamically registers the node's tools into its unified tool registry.

## Component Diagram

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Oh-Ben-Claw Core Agent (Host)                                               │
│                                                                              │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────────────┐  │
│  │  Channels   │──►│  Agent Loop  │──►│  Unified Tool Registry           │  │
│  │  Telegram   │   │  (LLM calls) │   │  (local + all peripheral tools)  │  │
│  │  Discord    │   └──────┬───────┘   └──────────────────────────────────┘  │
│  │  CLI / GUI  │          │                                                  │
│  └─────────────┘          ▼                                                  │
│                   ┌───────────────┐                                          │
│                   │  Bus Client   │                                          │
│                   └──────┬────────┘                                          │
└──────────────────────────┼───────────────────────────────────────────────────┘
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

## Node Lifecycle

A peripheral node follows a well-defined lifecycle:

1. **Boot**: The node powers on and initializes its hardware peripherals.
2. **Connect**: The node connects to the WiFi network and the MQTT broker.
3. **Announce**: The node publishes a `NodeAnnouncement` to `obc/nodes/{node_id}/announce`.
4. **Heartbeat**: The node publishes a heartbeat every 30 seconds to `obc/nodes/{node_id}/heartbeat`.
5. **Listen**: The node subscribes to `obc/tools/{node_id}/call/+` and waits for tool call requests.
6. **Execute**: When a tool call request arrives, the node executes the tool and publishes the result.
7. **Disconnect**: When the node powers off, it publishes a `last will` message to indicate its departure.

## Tool Call Flow

The following sequence describes how the brain invokes a tool on a peripheral node:

1. The user sends a message to the agent via a channel (e.g., "Take a photo with the kitchen camera").
2. The agent's LLM decides to invoke the `camera_capture` tool on the `esp32-s3-kitchen` node.
3. The agent generates a unique `call_id` and publishes a `ToolCallRequest` to `obc/tools/esp32-s3-kitchen/call/camera_capture`.
4. The ESP32-S3 node receives the request, captures a JPEG image, and publishes a `ToolCallResult` to `obc/tools/esp32-s3-kitchen/result/{call_id}`.
5. The agent receives the result, decodes the base64 JPEG, and returns it to the user.

## Security Model

All inter-node communication is secured through a combination of MQTT authentication (username/password or TLS client certificates) and a pairing protocol. Before a peripheral node's tools are accepted into the brain's registry, the node must complete a pairing handshake that verifies its identity. This prevents rogue devices from injecting malicious tools into the agent's registry.

## Relationship to ZeroClaw

Oh-Ben-Claw is built on top of the `Benji-zeroclaw` fork of `zeroclaw-labs/zeroclaw`. It inherits the core agent loop, provider system, channel system, tool registry, and peripheral framework. The key additions are the MQTT-based communication bus, the dynamic tool discovery mechanism, and the expanded hardware ecosystem.

The following table summarizes the key differences:

| Feature | ZeroClaw | Oh-Ben-Claw |
|---|---|---|
| Communication | Direct serial / native GPIO | MQTT bus + serial / native GPIO |
| Tool discovery | Static configuration | Dynamic via node announcements |
| Multi-device | Multiple boards, direct connections | Fleet of nodes over network |
| GUI | None | Planned (Tauri/egui) |
| Node pairing | None | Planned (HMAC-based) |

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

**Oh-Ben-Claw** is an advanced, distributed, **embodied** AI agent built on the [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) architecture. It began as a multi-device orchestrator — one LLM brain commanding a fleet of hardware peripherals over an MQTT spine — and has grown a full **embodied control stack**: a bitemporal world memory, millisecond reflexes, a predictive foresight layer, deliberative guarded missions, and multi-robot fleet coordination — all bounded by a single uniform safety gate that runs on the host **and** on the microcontroller.

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
- [Heartbeat File](#heartbeat-file)
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
| **Deliberative** | Missions | a multi-step plan with guards | `src/mission` |
| **Coordinated** | Fleet | many robots sharing work | `src/fleet` |

**Reflexes** (`src/agent/reflex`) evaluate conditions (`Sensor`, `GpioEq`, categorical `State`, `And`/`Or`) against world memory every tick and fire actions (`GpioWrite`, `Publish`, `Escalate`, `Move`) with debounce and an escalation budget — System 1, no LLM in the loop. The **safing** library (`src/agent/safing`) adds canonical self-protection rules (battery critical → escalate + Track 0 stop; battery low → shed load; link offline; audio alarm; out-of-range sensor; overheat) that *recover automatically* when conditions normalize.

**Foresight** (`src/foresight`) fits a trend over an entity's recent history and fires *before* the event — `battery ≤ 10% within 60s → return to base` triggers while the pack is still at 20% but draining fast. The forecaster supports exponentially-weighted (online) regression, so it tracks regime changes instead of lagging behind them.

**Missions** (`src/mission`) execute a guarded sequence of steps (`navigate_to` / `wait` / `speak` / `record` / `await_state`). Guards **preempt and halt** the body when a bad mode appears.

> This paragraph also advertised a **behavior-tree engine** (`src/mission/bt`) with
> "a full declarative grammar (sequence / reactive-sequence / fallback / parallel /
> decorators)". `Bt`, `BtSpec`, `BtContext` and `BtRunner` have no reference outside
> that file — 648 lines that nothing runs. What executes is the linear guarded
> sequencer, which is what `docs/SOTA-COMPARISON.md` describes accurately: "a
> reactive Sequence with condition guards — a strict subset of a BT", with "no full
> BT grammar" named as the honest gap. The comparison document was right and this
> sentence was not.

**Self-authored reflexes** (`src/learning`) mine history for antecedents that repeatedly preceded a bad outcome and *propose* new rules with support/confidence — but a proposal only goes live through an explicit **approval gate**, after which it joins the foresight engine's shared rule buffer on the next tick.

### Subsystem suites

Five capability suites share one contract (perceive → remember → act; see [`docs/SUBSYSTEM-SUITE-CONTRACT.md`](docs/SUBSYSTEM-SUITE-CONTRACT.md)), each recording to world memory and exposing gated MCP tools:

| Suite | Module | Perceives / Acts | Mode hook |
|---|---|---|---|
| **Sensing** | `crates/obc-telemetry/src/sensing.rs` | classifies samples vs range/freshness specs → `sensor.{quantity}` with quality | `quality` |
| **Audio** | `src/audio/suite` | hears (reliability-classified events) and speaks (pluggable TTS / spine sink) | `audio.*` |
| **Power** | `crates/obc-telemetry/src/power.rs` | battery SoC + charge state → `power.mode` (`normal`/`low`/`critical`/`charging`) | `power.mode` |
| **Comms** | `crates/obc-telemetry/src/comms.rs` | per-link health → aggregated `net.mode` (`online`/`degraded`/`offline`) | `net.mode` |
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

**Pose Fusion** combines several noisy pose estimates — wheel odometry, GNSS, an IMU or visual estimate — into one best pose: a weighted average of position and a **circular** weighted mean of heading, so 350° and 10° fuse to 0° rather than 180°. It writes the fused pose to the `sensor.pos_x/pos_y/heading` world-memory entities the navigation localizer already reads, so it drops in front of navigation with no changes to it.

> Corrected 2026-08-21. This paragraph described "Sensor Fusion ... averaging, median, min/max, weighted, and a simple Kalman filter". There is no such module and there is no Kalman filter in the tree: `crates/obc-navigation/src/pose_fusion.rs` fuses *poses*, and its own header says it "is not full SLAM ... it is the sensor-fusion layer that a SLAM front-end would feed". A partial correction on 2026-08-19 renamed the claim in one place and left this paragraph and the ZeroClaw comparison row untouched.

**Browser Automation** drives Chrome via the DevTools Protocol with seven tools (`browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_scroll`, `browser_new_tab`, `browser_close_tab`); falls back to plain HTTP fetch when no CDP endpoint is reachable.

**Pluggable LLM Providers** support OpenAI, Anthropic, Google Gemini, Ollama (local), OpenRouter, and any OpenAI-compatible endpoint, with **model failover** chains and exponential-backoff **retry** for transient errors.

**Rich Communication Channels** — Telegram, Discord, Slack, WhatsApp, iMessage, IRC, Matrix, Signal, Mattermost, Feishu/Lark, and a built-in CLI, with typing indicators and a native GUI (Tauri 2 + React).

> **Four of these were unreachable until 2026-07-30.** IRC, Signal, Mattermost and
> Feishu were written and unit-tested — 1,524 lines, twenty-two tests — and never
> added to `start_channels`, so configuring one produced silence. They are wired
> now, and **have not been run against a live server**: no IRC network, no
> signal-cli daemon, no Mattermost or Feishu tenant was to hand. Unit tests only.
> The other seven are exercised in normal use. `tests/channel_wiring.rs` now fails
> the build if an exported channel is not constructed, which is the check that was
> missing.

**Human-in-the-Loop Approval** provides supervised execution with three autonomy levels (`full` / `supervised` / `manual`), a session-scoped allowlist, and a full audit log — and it is what gates high-blast physical actions from the embodied stack.

**Deployment Scheme Generator** analyses your hardware inventory, maps capabilities to agent roles, identifies gaps, and renders a ready-to-use TOML configuration (optionally refined by an LLM-powered planning swarm).

**Skill Forge** synthesizes, improves and rolls out skills the agent writes for itself. Synthesized physical skills are quarantined behind Track 0 until approved.

> **ClawHub installation does not run.** This line claimed Skill Forge and ClawHub
> "discover, vet, and install community skills, with a security install policy
> (consent, allowlist, version pinning, SHA-256 checksums, static manifest
> inspection, JSONL audit log)". Synthesis, improvement and rollout are live.
> Installation is not: `ClawHubClient`, `ClawHubEntry` and `SkillRegistryIndex`
> have no reference outside `skill_forge/registry.rs`, and `install_policy` — the
> module written against the 2026 registry supply-chain threat — is referenced
> **only from that dead file**. `[clawhub]` and `[clawhub.install_policy]` parse
> and configure a path nothing can take.
>
> That is the third security control found in this shape, after node pairing and
> the tool sandbox: built, configured, unit-tested, and never invoked. The pattern
> is worth more attention than any one instance — a control with tests and a config
> key reads as present to every check except running it.

**MCP Integration** exposes all tools as a Model Context Protocol server (stdio + HTTP/SSE, dual-mode for the 2026 spec) and imports tools from external MCP servers. **A2A Protocol** — Google's Agent-to-Agent v1.0, implemented, conformance-tested, and served by `oh-ben-claw a2a-serve`.

> **A2A is served now (2026-08-08).** This block said "**A2A is not served**",
> and it was right for five months. Keeping the finding rather than deleting it,
> because what it measured is the useful part:
>
> - `src/main.rs` named `a2a` **zero** times; `src/gateway/` **zero** times.
> - With `pub mod a2a` flipped to `pub(crate)` so `dead_code` could see it,
>   rustc reported **34 items never constructed or used** — `pub` had been
>   silencing the lint over the whole module.
> - `[a2a]` parsed and was read by nothing: `config.a2a` had **zero** reads in
>   `src/` and `tests/`, so `enabled = true` changed nothing. It was not in
>   `config.example.toml` either, so it was inert *and* undiscoverable.
> - `A2AServer::execute` was a stub that filed the message in history and
>   returned `TASK_STATE_COMPLETED`, having done nothing.
>
> What changed:
>
> - `oh-ben-claw a2a-serve` binds an HTTP listener serving
>   `/.well-known/agent-card.json` and the JSON-RPC endpoint at `/a2a`.
> - `SendMessage` dispatches to `Agent::process` through a `TaskExecutor` trait;
>   the reply comes back as a text artifact on a completed task, and a model or
>   tool failure returns a **failed task**, not a JSON-RPC error — the request
>   was well formed and the protocol worked. `--echo` keeps the old stub
>   reachable on purpose, for conformance runs with no model behind them.
> - `[a2a]` is read: `a2a-serve` refuses to start unless `enabled = true`, and
>   the card is built from `agent_name`, `agent_description`, `agent_url` and
>   `skills`. The block is in `config.example.toml` now.
> - 5 tests drive a real socket — card fetch, version-header refusal, send and
>   retrieve across two requests, and a marker executor proving the transport
>   uses the executor it was given rather than the old hard-coded path.
>
> **Still not true of it:** the endpoint has **no authentication** — every
> caller that reaches it can drive this agent's tools. It binds `127.0.0.1`
> and the address is not configurable, so "reaches it" means a process on this
> machine, or whatever you deliberately put in front of it. Anything you proxy
> it through is doing the authenticating; the endpoint itself is not.
> `A2AClient` is still constructed **nowhere**: this
> agent can be called, and cannot call out. Streaming (`SendStreamingMessage`,
> `SubscribeToTask`) still returns `UNSUPPORTED_OPERATION`, which is a
> conformant answer and not an implementation.
>
> This was the fourth control found in this shape, after node pairing, the tool
> sandbox and ClawHub install. Worth noting which check found it: not the tests
> — they passed, and they were testing the right things. Only asking *who
> constructs this type*, and then removing the `pub` that was hiding the answer.

**Operations** — `oh-ben-claw doctor` health checks (now including subsystem/safing coherence), token **cost tracking** with persistent budgets, **observability** (metrics + spans), scheduled tasks, encrypted secrets **vault**, and a tamper-evident **audit chain**.

> **Nodes are authenticated now; their trust score is still not consulted.**
> This block said "**Nodes are not authenticated**" — that was true when written
> on 2026-07-30 (`1a11d23`) and stopped being true the next day, and nothing
> updated it for two days. Correcting a stale claim with a claim that then goes
> stale is the same defect, so here is the current measurement rather than a
> narrative:
>
> | Claim the old note made | Now (callers outside the defining module's own tests) |
> |---|---|
> | `pair_node` has zero callers | **one** — `spine/mod.rs:273`, inside `admit_announcement` |
> | the `pairing` field is never read | **six** reads across `agent/edge.rs`, `spine/mod.rs`, `spine/p2p.rs` |
> | `require_pairing` gates nothing | it selects `Admission::Admit` / `Refuse` for every inbound `NodeAnnouncement` on MQTT and P2P |
> | `is_trusted` has zero callers | **still zero** |
>
> Fixed by `caca7a3` ("require_pairing finally refuses something") and `2cf2ce0`
> ("verify the MAC on inbound messages"), both 2026-07-31. An unpaired node's
> announcement is refused and its tools never enter the registry; with
> `[security] require_frame_auth`, inbound tool results and calls carry a
> truncated HMAC over `src ‖ ctr ‖ payload` and a 64-frame replay window.
> `tests/pairing_admission_gate.rs` and `tests/frame_auth_wire.rs` drive both,
> including the forged-token and wrong-node cases.
>
> **The gap that remains** is the one the old note named last and got backwards.
> `src/security/trust.rs` computes a behavioural trust score and decays it on
> misbehaviour — and `is_trusted` has no callers, so nothing ever asks. The old
> note said trust decays but nothing establishes it; establishment is what got
> built, and consultation is what is still missing. A score nobody reads is the
> same shape as a gate nobody calls, one layer up.

> **Tools are not sandboxed, and there is a layer that helps.** `src/runtime/`
> held a `native` / `docker` / `wasm` abstraction that nothing ever called —
> `ShellTool` spawned `sh -c` directly, and the `wasm` adapter was a stub with no
> `wasmtime` dependency. It was **removed on 2026-07-30** rather than wired: it
> would have covered one tool out of seventy-six, and the dangerous ones by
> design — `gpio_write`, `i2c_*`, `ota_update`, spine publish — cannot be put in a
> container, because touching hardware is the point.
>
> The layer that does cover every tool is **`[[security.policies]]`** — a
> host-side allowlist evaluated on every tool call and on every hop, so it
> re-checks the target after a skill delegates. Glob patterns, an optional
> substring match on the arguments, and deny / audit / allow. It ships with no
> rules, and `config.example.toml` carries a recommended baseline you can paste:
> deny `shell`, deny `file` deletes, audit `http`, `ota_update` and `skill_forge`.
> An agent with no policies configured now says so at startup.
>
> This narrows what a **hijacked reasoner** can reach — the path §4.4 of
> `docs/SAFETY.md` names, where text recovered by `vision_analyze` arrives in the
> planner as prose. It does not survive host compromise. The deterministic limit
> table on the microcontroller is the only boundary that does, and nothing here
> replaces it.

**Memory** — a bitemporal world model with provenance, a support graph and four ways a belief can be withdrawn (supersession, source liveness, dependency withdrawal, retention).

> The vector store went the same way as the two above, and I left this claim
> standing when I struck them. `VectorStore`, `EmbeddingClient`,
> `VectorSearchTool` and `DocumentIngestTool` all have zero references outside
> `vector.rs`; the two tool impls were deleted on 2026-07-30 when the substrate
> became its own crate, and the store is kept but unwired. It escaped the file
> sweep because several of its type names are generic enough to match unrelated
> code — a reminder that the sweep clears nothing, it only accuses.

> `HEARTBEAT.md` task dispatch and the daily journal were listed here and are
> **not wired**: `HeartbeatStore`'s `has_tasks`, `actionable_tasks`,
> `build_prompt` and `append_task` have no callers outside their own file, and
> `DailyJournal`'s only external reference is its own re-export. Both are found
> by `scripts/file_reachability.py`, along with twelve more like them.

---

## Supported Hardware

### Boards

| Device | Transport | Capabilities |
|---|---|---|
| Waveshare ESP32-S3 Touch LCD 2.1 | Serial / MQTT | GPIO, Camera (OV2640), Microphone (I2S), Touch Display, Speaker |
| Seeed XIAO ESP32S3-Sense | Serial / MQTT | Camera (OV2640), Microphone (PDM), GPIO, Wi-Fi, BLE |
| Sipeed 6+1 Mic Array | USB (UAC1) / Serial | Far-field 6+1 MEMS microphone array |
| ESP32-S3 (generic) | Serial / MQTT | GPIO, Camera, Microphone, Sensors |
| LILYGO T-Deck | Serial / LoRa | LoRa (SX1262), Touch Display, Keyboard, Trackball, Mic, Speaker, microSD |
| LILYGO T-Deck Plus | Serial / LoRa | T-Deck + GPS (u-blox/L76K) + 2000 mAh — handheld fleet console (`firmware/t-deck-terminal`) |
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

Sensors and I/O over I2C / SPI / 1-Wire / GPIO — including BME280, BMP388, AHT20, MPU6050, LSM6DS3, SHT31, ADS1115, MCP4725, INA260, PCF8574, MCP23017, MAX31855, DS18B20, SSD1306, DHT22/DHT11 — plus **embodied actuation & power** accessories added for the control stack: **SG90** servo, **TB6612FNG** / **PCA9685** motor & PWM drivers, **INMP441** mic, **MAX98357A** amp, **MAX17048** fuel gauge, and **SIM7600** cellular. The full machine-readable list is the registry single-source-of-truth (`crates/obc-planner/src/peripherals/registry.rs` → `registry.json`).

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

```

There is no setup wizard, and you do not need one: with a provider key in the
environment and no config file at all, the agent picks the provider whose key is
actually set. Write a config when you want to pin a choice rather than have one
inferred — start from [`config.example.toml`](config.example.toml).

### Quick Start

```bash
# 1. Start your MQTT broker
mosquitto -d

# 2. Set your LLM API key
export OPENAI_API_KEY="sk-..."

# 3. Start the agent (no config file needed — see below for where one goes)
./target/release/oh-ben-claw start

# 4. In another terminal, run diagnostics
./target/release/oh-ben-claw doctor
```

---

## Configuration

Configuration is TOML, and entirely optional. The agent looks in four places,
first hit wins:

1. `--config <PATH>`, or the `OBC_CONFIG` environment variable
2. `config.toml` in the **data root** — see [Data location](#data-location)
3. the platform config directory (`~/.config/oh-ben-claw/` on Linux,
   `%APPDATA%\thewriterben\oh-ben-claw\config\` on Windows)
4. `~/.oh-ben-claw/config.toml`, kept for anyone following older documentation

Start from [`config.example.toml`](config.example.toml), which is annotated and
carries no credentials; [`examples/config-multi-device.toml`](examples/config-multi-device.toml)
is a fuller worked example.

### Data location

Everything this instance writes — `memory.db`, `world.db`, `scheduler.db`,
`vault.db`, the audit chain, approval grants — lives under one root:

```bash
# Platform default: ~/.local/share/oh-ben-claw (Linux),
# %APPDATA%\thewriterben\oh-ben-claw\data (Windows),
# ~/Library/Application Support/com.thewriterben.oh-ben-claw (macOS)

export OBC_DATA_DIR=/srv/obc/kitchen     # or [paths].data_dir in the config file
```

Two agents on one machine need two roots. Set `OBC_DATA_DIR` for the second one
and it is fully separate — its own database, its own audit chain, its own
standing approval grants — and if you drop a `config.toml` in that root it will
pick that up too, without touching anything the first agent reads.

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

# Deliberative missions (named library)
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

## Heartbeat File

> **Not wired.** `HEARTBEAT.md` was described here as "a plain Markdown task list;
> uncompleted items trigger the agent on a schedule". `HeartbeatStore` reads the
> file and `has_tasks`, `actionable_tasks`, `build_prompt` and `append_task` have
> no callers, so nothing is triggered and no schedule consults it. Writing the file
> has no effect today. Kept as a section because the shape is right and the wiring
> is a small job; see `scripts/file_reachability.py`.

Say what the agent is for in `[agent].system_prompt` — see `config.example.toml`.
You do not need to describe the world there: current beliefs, with provenance
and age, are supplied to the model on every turn.
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

> **The agent does not read the `[deployment]` section it is handed.**
> `Config.deployment` is parsed and never accessed: the only reads in the tree are
> in `tests/planner_parity.rs`, asserting the generated TOML is *schema-valid*.
> That test is real and worth having — it is why the emitted config cannot drift
> from the runtime's types — but it checks the shape, not that anything consumes
> it. So `auto_plan = true` below, documented in `DeploymentConfig` as "generate
> and print the deployment scheme at startup", does nothing; the only reference to
> `auto_plan` outside the config struct is the line that *writes* it into
> generated configs. Same for `pre_spawn`.
>
> Found by `scripts/inert_components.py`, which looks for the shape three security
> controls already hid in: constructed, configured, tested, never interrogated.

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
  start            Start the agent (interactive CLI, gateway, and tunnel)
  status           Check the agent and all connected peripheral nodes
  peripheral       Manage peripheral hardware nodes
  history          Manage conversation history
  skill            Manage learned skills and their Track 0 staged rollout
  mcp-serve        Run the MCP server standalone (stdio, or http for gateways
                   and the official conformance suite)
  judge-calibrate  Measure LLM-judge calibration against a gold label set (κ)
  doctor           Run system diagnostics
  help             Print this message or the help of a subcommand

Options:
      --config <PATH>  Path to a config file, overriding the default location
                       and the OBC_CONFIG env var
  -h, --help           Print help
  -V, --version        Print version
```

Environment: `OBC_CONFIG` (config file), `OBC_DATA_DIR` (data root),
`OBC_VAULT_PASSWORD`, `OBC_BROWSER_CDP_URL`. Provider keys are read directly —
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`.

This block is generated from `--help` output rather than remembered. It
previously advertised a `setup` wizard and a `service` manager, neither of which
has ever existed, and omitted four commands that do.

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

### LoRa mesh nodes (`firmware/lora-node`, `firmware/heltec-lora-linktest`)

Radios for the off-grid spine: `lora-node` is a dumb USB-serial ⇄ LoRa bridge
(T-Beam, Heltec V2/V3, RAK4631, **T-Deck**) speaking the host's fleet codec, and
`heltec-lora-linktest` is the Rust base-station gateway for the spine net.

### T-Deck handheld fleet console (`firmware/t-deck-terminal`)

Turns a **LILYGO T-Deck / T-Deck Plus** into a human-carried console on the LoRa
spine: live frame scrollback on the 2.8" screen, QWERTY chat and `/cmd` node
commands (still gated by the target node's on-MCU Track 0 mirror), GPS + battery
heartbeats on the Plus, flood relay, and — tethered over USB — a drop-in
replacement for the Heltec base-station gateway. See
[`firmware/t-deck-terminal/README.md`](firmware/t-deck-terminal/README.md).

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

Almost all of the agent now lives in `crates/` rather than `src/`, extracted one
at a time as each became self-contained enough to compile on its own. What is
left in `src/` is the binary, the deployment generator, the doctor and the test
harness.

The tree below said otherwise until 2026-08-02: it listed `src/memory/`,
`src/sensing/`, `src/power/`, `src/comms/` and `src/security/limits.rs`, none of
which had existed since 2026-07-30. It was corrected by hand, and it drifted
again immediately — by 2026-08-19 it drew 27 `src/` directories that no longer
existed and 6 crates out of 33, because every extraction after that day
invalidated it and nothing said so. A hand-corrected tree is a tree that is
correct on the day someone looks at it.

`scripts/check_tree.py` now resolves every path this tree draws and fails on any
that is absent, requires `crates/` to be listed in full rather than partially —
a short list there reads as a complete one — and holds every
`<!-- unwired: -->` disclosure marker to naming a file that exists, because a
marker that resolves to nothing discloses nothing to the survey that reads it.
It runs in CI. The comments beside the paths are still prose and still
unchecked, which is the argument for keeping them to one line.

```
Oh-Ben-Claw/
├── crates/                   # Extracted, self-contained, vendored into OBC-Prime
│   ├── obc-a2a/              # Agent-to-Agent v1.0: wire types, JSON-RPC lifecycle, HTTP
│   ├── obc-agent/            # The agent loop, dispatcher, and the Track 0 chokepoint
│   ├── obc-approval/         # Autonomy levels, per-call risk check, persisted grants
│   ├── obc-audio/            # Audio pipeline + audio suite (hear / speak)
│   ├── obc-body/             # ClawBot body model -> Track 0 limits (lib.rs unwired - ROADMAP)
│   ├── obc-channels/         # Telegram, Discord, Feishu, IRC, Signal, Matrix, ...
│   ├── obc-config/           # Configuration schema and loading (Config::validate)
│   ├── obc-conscience/       # What the agent may observe and reach; decision log
│   ├── obc-cost/             # Token cost tracking and budget enforcement
│   ├── obc-fleet/            # Multi-robot coordination (registry, auction, conflicts)
│   ├── obc-foresight/        # Predictive control (Track 1) + online forecaster
│   ├── obc-gateway/          # REST/WebSocket API gateway (Axum)
│   ├── obc-learning/         # Self-authored reflexes (mine -> approve -> activate)
│   ├── obc-mcp/              # Model Context Protocol client/server (dual-mode)
│   ├── obc-memory/           # Bitemporal world memory (the embodied substrate)
│   ├── obc-mission/          # Mission sequencer, advancing across restarts
│   ├── obc-movement/         # Track 0-bounded actuation (feedback.rs parked - ROADMAP)
│   ├── obc-navigation/       # Localization, SLAM, mapping, A*+costmap, particle filter
│   ├── obc-observability/    # The agent watching itself: spans + counters
│   ├── obc-paths/            # Config/data directory resolution
│   ├── obc-peripherals/      # Hardware drivers + registry SSOT
│   ├── obc-planner/          # Deployment planner, site optimizer, geo, registry
│   ├── obc-position/         # Geodetic telemetry and NMEA, projected into the site frame
│   ├── obc-providers/        # LLM provider adapters + failover + retry
│   ├── obc-reflex/           # System 1: the rule engine, mirrored on the node
│   ├── obc-safety/           # Track 0 gate, limits, audit chain, pairing, taint
│   ├── obc-scheduler/        # Scheduled tasks and cron jobs
│   ├── obc-skill-forge/      # Skill discovery, synthesis, ClawHub registry
│   ├── obc-spine/            # MQTT spine, LoRa gateway and mesh, P2P transport
│   ├── obc-telemetry/        # The agent watching its body: power / comms / sensing
│   ├── obc-tool-api/         # The Tool contract, with no implementation
│   ├── obc-tools/            # Every built-in tool the model can call
│   ├── obc-tunnel/           # Network tunnels (Cloudflare, ngrok, Tailscale)
│   └── obc-vision/           # Vision pipeline + ClawCam detection ingest
├── src/                      # What has not been extracted, and the binary
│   ├── bin/                  # emit-registry, emit-firmware-templates, mcp-conformance
│   ├── deployment/           # Hardware-driven deployment scheme generator
│   ├── doctor/               # System diagnostics (oh-ben-claw doctor)
│   ├── harness/              # Shared scaffolding for the integration tests
│   ├── lib.rs                # Crate root: re-exports every obc-* crate
│   └── main.rs               # The binary: composes the agent from config
├── firmware/                 # The node end of the conversation
│   ├── obc-esp32-s3/         # ESP32-S3 + on-MCU reflex/safing/Track 0 mirror
│   ├── heltec-lora-linktest/ # Heltec V3 (ESP32-S3 + SX1262) LoRa mesh node
│   ├── lora-node/            # Arduino LoRa bridge sketch
│   └── t-deck-terminal/      # T-Deck handheld terminal sketch
├── scripts/                  # The checks and surveys CI and ROADMAP.md run
├── gui/                      # Tauri 2 + React 18 native desktop application
├── docs/                     # Four worth opening first; there are more
│   ├── EMBODIED-ARCHITECTURE.md # The embodied control stack, end to end
│   ├── SAFETY-CASE.md        # What Track 0 claims, and what backs each claim
│   ├── SUBSYSTEM-SUITE-CONTRACT.md # The perceive->remember->act contract
│   └── playbooks/            # What to do when a node goes quiet
├── registry/                 # Peripheral registry SSOT (registry.json)
├── examples/                 # Annotated reference configurations
├── planner-wasm/             # The WASM build of the planner OBC-Prime vendors
└── tests/                    # Integration tests (embodied_full_stack, embodied_hil_loop, ...)
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
| **Missions** | ✗ | ✅ Guarded sequencer (`bt.rs` is written and unwired — see Missions) |
| **Navigation / SLAM** | ✗ | ✅ Particle filter, pose-graph SLAM, A*+costmap |
| **Self-authored reflexes** | ✗ | ✅ Mine → approve → activate |
| **Fleet coordination** | ✗ | ✅ Auction allocation + conflict avoidance |
| Browser automation | ✗ | ✅ CDP (7 tools) |
| Vision / audio / fusion | ✗ | ✅ |
| Deployment planner | ✗ | ✅ LLM + rule-based swarm |
| GUI | ✗ | ✅ Tauri 2 + React 18 |
| MCP / A2A | ✗ | MCP: ✅ client + server. A2A: ✅ server (`a2a-serve`, no auth on the endpoint); no client — this agent can be called, not call out |
| Human approval | ✗ | ✅ 3 autonomy levels |
| Tool sandboxing | ✗ | ✗ — see Operations. Host-side policy allowlist instead (`[[security.policies]]`) |
| Edge-native mode | ✗ | ✅ (ESP32-S3, NanoPi) |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). It starts with which repo a change
belongs in: this one is the upstream core, and
[OBC-Prime](https://github.com/thewriterben/OBC-Prime) is the public project
that vendors the registry, fixtures and planner WASM emitted from here.

## License

MIT — see [LICENSE](LICENSE) for details.

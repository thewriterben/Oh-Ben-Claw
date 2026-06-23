# Oh-Ben-Claw v2.0 — Software & Firmware Implementation Design

*Companion to `docs/V2-STRATEGY.md` and the v2.0 entries in `ROADMAP.md`. Compiled June 23, 2026.*

This document expands each v2.0 phase into concrete engineering: the **software** changes in the Rust core (`src/`) and the **firmware** changes on peripheral nodes (`firmware/obc-esp32-s3/`, and the Linux-SBC edge path). Rust sketches are illustrative and follow the existing conventions — the async `Tool` trait (`async fn execute(&self, args: Value) -> anyhow::Result<ToolResult>`), `ProviderConfig`, `SpineClient`, serde-driven TOML config, SQLite WAL stores.

---

## 0. Cross-cutting groundwork (do this first)

Two foundational changes underpin several phases; build them once.

### 0.1 The embodiment split: where code runs

Every v2.0 capability has to answer "host or node?" The guiding rule:

| Concern | Runs on | Why |
|---|---|---|
| Heavy LLM reasoning (System 2) | Host / cloud | Needs context + tokens |
| Fast reflexes, **safety limits** | **Node firmware** | Must survive host loss/compromise; latency |
| World memory, durable state, skill synthesis | Host | Storage + compute |
| Wake-word, STT/TTS front-end | Node (or SBC edge) | Privacy + latency |

The non-negotiable consequence: **deterministic actuator safety limits live in firmware, not in the Rust host.** If the host is compromised or offline, the node still refuses an out-of-range command. This is the embodied moat made concrete.

### 0.2 Spine protocol additions

Current topics (`src/spine/mod.rs`): `obc/nodes/{id}/announce|heartbeat|status`, `obc/tools/{id}/call/{tool}`, `obc/tools/{id}/result/{call_id}`, `obc/broadcast/command`.

v2.0 adds:

```
obc/nodes/{id}/limits        # host pushes safety-limit table to node (retained)
obc/nodes/{id}/reflex        # host pushes reflex rules to node (retained, Phase 18)
obc/nodes/{id}/event         # node → host: reflex fired / limit tripped / state change
obc/stream/{id}/audio        # node → host: streamed PCM frames (Phase 19)
obc/stream/{id}/video        # node → host: streamed JPEG frames (Phase 19)
obc/world/state              # host: world-memory state deltas (Phase 18)
```

Extend `NodeAnnouncement` so a node advertises what it can enforce/run locally:

```rust
// src/spine/mod.rs
pub struct NodeAnnouncement {
    // … existing: node_id, capabilities, tools …
    #[serde(default)] pub firmware_version: String,
    #[serde(default)] pub enforces_limits: bool,   // node honors obc/nodes/{id}/limits
    #[serde(default)] pub runs_reflex: bool,        // node has a reflex engine (Phase 18)
    #[serde(default)] pub streams: Vec<String>,     // e.g. ["audio","video"] (Phase 19)
    #[serde(default)] pub edge_inference: Option<String>, // local model id, if any (Phase 20)
}
```

Every new tool-call envelope gains an **idempotency key** (`call_id` already exists; promote it to a dedup key persisted on the node) so Phase 17 resume cannot double-actuate.

---

## Track 0 — Physical-Action Safety & Trust

### Software (`src/security/`, `src/approval/`, `src/tools/`)

**Risk classification.** Add a trait method so every tool declares its physical risk; default is non-physical/reversible so existing tools are unaffected.

```rust
// src/tools/traits.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlastRadius { None, Low, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskClass {
    pub reversible: bool,
    pub blast: BlastRadius,
    pub physical: bool,   // touches an actuator / real-world effect
}
impl Default for RiskClass {
    fn default() -> Self { Self { reversible: true, blast: BlastRadius::None, physical: false } }
}

pub trait Tool: Send + Sync {
    // … existing name/description/parameters/execute …
    fn risk_class(&self) -> RiskClass { RiskClass::default() }
}
```

Actuator tools (`gpio_write`, PWM, relay, lock, future motor tools) override `risk_class()` → `{ reversible:false, blast:High, physical:true }`.

**Approval defaults driven by risk.** `ApprovalManager::from_config` already exists; extend its policy resolution so a tool's `RiskClass` sets the *default* scope when config is silent: `physical && !reversible` ⇒ require per-`Call` approval; `physical && High` ⇒ never auto-grantable to `Forever`. Reuse the existing `ApprovalScope`/`ForeverGrants`/`ApprovedPlan` machinery — this is a defaulting layer, not a rewrite.

**Pre-action authorization + signed audit.** New `src/security/authz.rs`:

```rust
pub struct ActionAuthorizer {
    policy: Arc<PolicyEngine>,        // existing glob/arg policy
    approvals: Arc<ApprovalManager>,  // existing
    signer: Ed25519Signer,            // new: signs the audit record
    obs: Arc<ObsContext>,             // existing observability
}
pub struct SignedAction {
    pub call_id: String, pub tool: String, pub args_hash: String,
    pub risk: RiskClass, pub decision: Decision, pub ts: u64, pub sig: String,
}
impl ActionAuthorizer {
    pub async fn authorize(&self, call: &ToolCall, risk: RiskClass)
        -> anyhow::Result<SignedAction>; // deny | allow | needs-approval, always audited+signed
}
```

The authorizer is invoked in the agent loop (`Agent::process` in `src/agent/mod.rs`) immediately before any `physical` tool executes, and on the edge path (`EdgeAgent::process`). Signed records append to a JSONL audit (mirrors the existing skill-install audit log pattern from Phase 15).

**Staged rollout.** A `MaturityStage { Simulate, Supervised, Autonomous }` field stored per tool/skill in a small SQLite table (`~/.oh-ben-claw/action_maturity.db`). `Simulate` ⇒ authorizer logs the intended actuator command but returns a stub result; `Supervised` ⇒ forces per-call approval regardless of grants; `Autonomous` ⇒ normal. Promotion is manual or earned after N clean supervised runs.

**Config:**

```toml
[safety]
enabled = true
audit_log_path = "~/.oh-ben-claw/action_audit.jsonl"
signing_key_path = "~/.oh-ben-claw/action_signing.key"   # ed25519; auto-generated
default_physical_stage = "supervised"                    # simulate|supervised|autonomous

[[safety.limit]]            # mirrored to firmware over obc/nodes/{id}/limits
node_id = "obc-esp32-s3-001"
tool = "gpio_write"
allow_pins = [17, 18, 27]   # everything else denied
max_rate_hz = 2             # ≤2 writes/sec
interlock = "front_door_unlock requires presence_confirmed"  # human-readable; encoded below
```

### Firmware (`firmware/obc-esp32-s3/src/main.rs`)

This is the most important firmware work in v2.0: **the node enforces limits even if the host lies.**

- New NVS namespace `obc_limits` holding a compact limit table (allowed pins, per-pin min/max value, max rate, required interlock flags). Populated two ways: `set_limits` UART/MQTT command, and the retained `obc/nodes/{id}/limits` topic at connect.
- A `SafetyGate` checked inside `gpio_write` and every future actuator handler **before** touching hardware:

```rust
// firmware/obc-esp32-s3/src/main.rs
struct LimitRule { pin: i32, vmin: u32, vmax: u32, min_interval_ms: u64, last_ms: u64 }
struct SafetyGate { rules: heapless::Vec<LimitRule, 32>, locked_default_deny: bool }
impl SafetyGate {
    fn check(&mut self, pin: i32, value: u32, now_ms: u64) -> Result<(), &'static str> {
        let r = self.rules.iter_mut().find(|r| r.pin == pin)
            .ok_or("pin not in allow-list")?;             // default-deny
        if value < r.vmin || value > r.vmax { return Err("value out of range"); }
        if now_ms - r.last_ms < r.min_interval_ms { return Err("rate limit"); }
        r.last_ms = now_ms; Ok(())
    }
}
```

- `gpio_write` returns a structured `{"error":"safety: rate limit"}` and publishes an `obc/nodes/{id}/event` of kind `limit_tripped` so the host audits the refusal.
- New commands: `set_limits` (write NVS), `get_limits` (report active table). Edge-mode (`agent_chat`) actuator calls go through the same `SafetyGate` — on-device autonomy is still bounded.

**Result:** a compromised host, a poisoned skill, or a hallucinated tool call physically cannot drive pin 99 or toggle a relay faster than 2 Hz, because the MCU refuses. That guarantee is independent of the LLM.

---

## Phase 16 — Experiential Self-Improvement

Mostly host-side; reuses `skill_forge`, `memory`, `agent`.

### Software

**Trajectory capture.** New `src/memory/trajectory.rs` (SQLite WAL, sibling of the existing `vector`/`journal`/`image` stores):

```rust
pub struct TrajectoryStore { /* conn */ }
pub struct Episode {
    pub id: String, pub session_id: String, pub objective: String,
    pub steps: Vec<EpisodeStep>, pub outcome: Outcome, pub ts: u64,
}
pub struct EpisodeStep { pub tool: String, pub args: Value, pub result: String, pub ok: bool }
pub enum Outcome { Success, Failure, Aborted }
impl TrajectoryStore {
    pub fn record(&self, ep: &Episode) -> Result<()>;
    pub fn successful_since(&self, ts: u64) -> Result<Vec<Episode>>;
    pub fn similar(&self, objective: &str, k: usize) -> Result<Vec<Episode>>; // vector-assisted
}
```

The agent loop (`Agent::process`) already produces `ToolCallRecord`s; wrap a run in an `Episode` and persist on completion. Gate by config so capture is opt-in.

**Reflection + skill synthesis.** New `src/skill_forge/synthesis.rs`:

```rust
pub struct SkillSynthesizer { provider: Arc<dyn Provider>, forge: SkillForge }
impl SkillSynthesizer {
    /// Ask the LLM to distil a successful episode into a reusable SkillManifest.
    pub async fn propose(&self, ep: &Episode) -> Result<SkillManifest>;
    /// Run the candidate against a held-out check before trusting it.
    pub async fn verify(&self, m: &SkillManifest, check: &VerificationCheck) -> Result<bool>;
}
```

Crucially, synthesis emits the *existing* `SkillManifest` type — a learned skill is usually a `SkillKind::Delegate` (a saved tool-call recipe with `fixed_args`) or a parameterized `Shell`/`Http` skill, so it drops straight into `SkillForge::install_skill()` and the ClawHub-compatible `.skill.json` format. No new execution path.

**Self-verification gate (the anti-degradation guard).** Reflection is unreliable without a real signal (Huang et al., ICLR 2024), so a synthesized skill is quarantined until it passes a concrete `VerificationCheck`:

```rust
pub enum VerificationCheck {
    Replay { expect_outcome: Outcome },          // re-run on a sandbox/sim
    SensorAssertion { tool: String, predicate: Predicate }, // e.g. temp dropped after AC on
    TestCommand { cmd: String, expect_exit: i32 },
}
```

Until verified, the skill is `enabled:false` and never offered to the LLM.

**Offline trace evolution (GEPA/DSPy-style).** A scheduled job (reuse `Scheduler` + `run_scheduler_loop`) periodically reflects over accumulated traces to improve skill `description`s and the agent's prompts — language traces as the optimization signal. Implemented as a `TaskKind`-driven batch in `src/skill_forge/evolve.rs`; writes proposed description diffs for review (not silent mutation).

**Track 0 interlock.** `SkillSynthesizer::propose` inspects the candidate's tool calls; if any target a `physical` tool, the skill is registered at `MaturityStage::Supervised` and cannot run unattended until promoted.

**Config:**

```toml
[self_improvement]
enabled = true
capture_trajectories = true
auto_synthesize = true          # propose skills from successes
require_verification = true     # never trust unverified skills (recommended)
evolve_interval_hours = 24
max_learned_skills = 500
```

### Firmware

Light touch. Nodes already return tool results; add an optional `seq` and `ok` flag to result envelopes so host-side trajectory steps capture on-device outcomes faithfully. No on-device synthesis (no headroom on an ESP32) — the node is a faithful *reporter* of what happened, the host does the learning. SBC edge nodes (NanoPi/Pi via `EdgeAgent`) may capture local trajectories if a `MemoryStore` is attached.

---

## Phase 17 — Long-Horizon Embodied Autonomy Harness

### Software (`src/runtime/`, `src/agent/`, `src/scheduler/`, `src/memory/`)

**Durable execution.** New `src/runtime/durable.rs`. A long-running objective is a checkpointed state machine persisted to SQLite:

```rust
pub struct DurableRun { pub id: String, pub objectives: Vec<Objective>, pub cursor: usize }
pub struct Objective { pub id: String, pub goal: String, pub status: ObjStatus, pub verify: VerificationCheck }
pub enum ObjStatus { Pending, InProgress, Verifying, Done, Failed }

pub struct DurableExecutor { store: DurableStore, agent: Arc<Agent>, authz: Arc<ActionAuthorizer> }
impl DurableExecutor {
    pub async fn run(&self, run_id: &str) -> Result<()>;   // resumable; idempotent per step
    pub async fn resume(&self, run_id: &str) -> Result<()>;
}
```

**Non-persistable regions.** The executor records a step as *committed* only after the tool result is durably written. A step that has been *dispatched* but not confirmed is, on resume, reconciled using the node's idempotency-key dedup (see §0.2) rather than blindly re-run — so the agent never re-actuates a door it already opened.

**Initializer + worker split.** Reuse the existing orchestrator/sub-agent machinery (`src/agent/orchestrator.rs`, `pool.rs`): an *initializer* sub-agent populates the `objectives` list and a baseline world snapshot; a *worker* sub-agent advances one `Objective` at a time. The externalized progress record is JSON (the model is less likely to clobber JSON than Markdown).

**Mandatory self-verification.** An objective only moves to `Done` after its `VerificationCheck` passes against live sensors/cameras (shared type with Phase 16). On `resume`, every `Done` is *re-attested* cheaply before continuing.

**Resume smoke test.** On startup the executor queries current node states (`SpineClient::known_nodes` + a fresh `obc/nodes/{id}/status` round-trip) to rebuild context before acting.

**Config:**

```toml
[autonomy.durable]
enabled = true
state_db = "~/.oh-ben-claw/durable.db"
checkpoint_every_step = true
reattest_on_resume = true
max_run_hours = 24
```

### Firmware

- **Idempotency dedup.** Node keeps a small ring of recently-seen `call_id`s in RAM (and survives brief disconnects); a repeated `call_id` returns the cached result instead of re-executing. This is what makes host-side resume physically safe.
- **State reporting.** `status` handler returns a structured snapshot (pin states, last sensor reads, uptime, firmware version) so the host's resume smoke test is cheap and accurate.
- **Heartbeat carries a state hash** so the host detects drift (someone toggled a pin physically) and re-attests.

---

## Phase 18 — Dual-System Perception-Action + World Memory

The architectural keystone. System 2 stays on host/cloud; **System 1 moves onto the node**.

### Software (`src/agent/edge.rs`, `src/memory/`, `src/peripherals/fusion.rs`)

**Reflex rule model (host authors, node runs).** New `src/agent/reflex.rs`:

```rust
pub struct ReflexRule {
    pub id: String,
    pub when: Condition,      // e.g. Sensor("temp") > 28.0  OR  Gpio(4) == 1
    pub then: Action,         // GpioWrite{pin,value} | Publish{topic,payload} | Escalate
    pub debounce_ms: u64,
    pub max_rate_hz: f32,
}
pub enum Condition { SensorCmp{sensor:String,op:Cmp,value:f64}, GpioEq{pin:i32,value:u32}, And(Box<..>,Box<..>), Or(..) }
pub enum Action { GpioWrite{pin:i32,value:u32}, Publish{topic:String,payload:Value}, Escalate{reason:String} }
```

Rules are compiled to a compact wire form and pushed to nodes over `obc/nodes/{id}/reflex` (retained). `Action::Escalate` hands control up to System 2: the node publishes an `event`, the host wakes the LLM agent. Every reflex `Action` that writes an actuator is still bounded by the firmware `SafetyGate` (Track 0) — reflexes can't exceed limits either.

**Bitemporal world memory.** New `src/memory/world.rs` — a temporal model of the physical environment, the embodied analogue of Graphiti:

```rust
pub struct WorldMemory { /* sqlite */ }
pub struct Fact {
    pub entity: String,        // "living_room.temp" | "front_door.lock" | "node:esp32-001"
    pub value: Value,
    pub valid_from: u64,       // event time
    pub valid_to: Option<u64>, // None = currently believed true
    pub ingested_at: u64,      // ingestion time (bitemporal)
    pub source: String,        // node id / tool / inference
}
impl WorldMemory {
    pub fn observe(&self, f: Fact) -> Result<()>;          // closes prior open fact for entity
    pub fn current(&self, entity: &str) -> Result<Option<Fact>>;
    pub fn at(&self, entity: &str, ts: u64) -> Result<Option<Fact>>;   // historical
    pub fn query(&self, q: &WorldQuery) -> Result<Vec<Fact>>;
}
```

Perception wiring: `vision`, `audio`, and `peripherals/fusion` outputs call `WorldMemory::observe`; planning (System 2) calls `current`/`at` to ground decisions in real, time-valid state instead of stuffing raw sensor logs into context. State deltas publish on `obc/world/state` for the GUI/dashboard.

**Escalation policy + budget.** `ReflexPolicy { escalate_on: Vec<Condition>, max_escalations_per_hour, system2_token_budget }` guards how often the cheap local loop wakes the expensive cloud loop.

**Config:**

```toml
[perception]
world_memory = true
world_db = "~/.oh-ben-claw/world.db"

[[reflex.rule]]
node_id = "obc-esp32-s3-001"
when = "sensor.temp > 28.0"
then = { gpio_write = { pin = 18, value = 1 } }   # fan on
debounce_ms = 5000
max_rate_hz = 0.2
escalate = false
```

### Firmware

This is where the ESP32 earns "frontier." A real **on-device reflex engine**:

- Parse the reflex wire form into a `heapless::Vec<ReflexRule, 16>` in RAM (+ NVS persistence so reflexes survive reboot and run with no host present).
- A periodic task (existing sensor sampling cadence) evaluates conditions and fires actions through the `SafetyGate`. Pure integer/float comparisons — trivial for the MCU, microsecond latency, fully offline.
- `Action::Escalate` publishes `obc/nodes/{id}/event {kind:"escalate", reason, snapshot}`; if the broker is unreachable, the rule's safe-default action runs locally and the event is queued.
- New commands `set_reflex` / `get_reflex` / `clear_reflex`. This makes a node genuinely autonomous: lose WiFi and the building still regulates temperature, because the reflexes live on the chip.

The dual-system story becomes literally true: **System 1 is silicon on the node; System 2 is the LLM on the host.**

---

## Phase 19 — Real-Time Multimodal Interaction

### Software (`src/channels/`, `src/audio/`, `src/multimodal.rs`)

**Realtime session channel.** New `src/channels/realtime.rs` implementing the existing channel pattern but holding a persistent bidirectional session to OpenAI Realtime (`gpt-realtime`) or Gemini Live:

```rust
pub struct RealtimeChannel {
    provider: RealtimeProvider,            // OpenAiRealtime | GeminiLive
    audio_in: mpsc::Receiver<PcmFrame>,    // from node stream
    audio_out: mpsc::Sender<PcmFrame>,     // to node speaker
    video_in: Option<mpsc::Receiver<JpegFrame>>,
    tools: Arc<ToolRegistry>,              // existing unified registry
    authz: Arc<ActionAuthorizer>,          // Track 0 gate for tool calls mid-convo
}
```

Key behaviors: stream audio frames straight to the model (no separate STT), forward camera frames for "see the room," handle **barge-in** (cancel in-flight TTS when the user speaks), and allow **mid-conversation tool calls** — which route through the same `ActionAuthorizer`, so a spoken "unlock the door" still hits the physical-action gate.

**Transport.** Node ⇄ host frames travel over the new `obc/stream/{id}/audio|video` topics (MQTT for control + small frames) or a direct WebSocket fast-path for higher-bitrate audio; the channel bridges node frames ⇄ provider session.

**Config:**

```toml
[channels.realtime]
enabled = true
provider = "openai_realtime"     # openai_realtime | gemini_live
model = "gpt-realtime"
node_id = "obc-esp32-s3-001"     # the mic/speaker device
video = true                     # stream camera frames into the session
barge_in = true
```

### Firmware

- **Audio streaming mode.** Beyond the existing one-shot `audio_sample`, add a continuous mode: I2S mic → 16 kHz PCM frames published on `obc/stream/{id}/audio`; speaker playback from `audio_out` frames (the Waveshare board has an I2S speaker — already in the registry).
- **Camera streaming.** Periodic low-res JPEG frames on `obc/stream/{id}/video` while a session is active (rate-limited to protect WiFi/heap).
- **Session control commands.** `stream_start{audio,video,rate}` / `stream_stop`. The MCU is a thin A/V transport here — all reasoning is host/cloud — which keeps it within ESP32-S3 heap and bandwidth limits.
- The touch-LCD node can show session state (listening/thinking/speaking) using existing `display`/`touch` capabilities.

---

## Phase 20 — Edge-Native Intelligence

### Software (`src/agent/edge.rs`, `src/providers/`)

**Small-model reflex tier.** `EdgeAgent` already runs a local-Ollama loop on SBCs; v2.0 makes it first-class for System 1 fallback:

```rust
// src/providers/mod.rs — already has Provider trait + failover/retry
pub struct LocalModelProvider { backend: LocalBackend /* Ollama | LlamaCpp */, model: String }
```

**Policy-driven escalation.** New `src/agent/escalation.rs` — deterministic rules for local → cloud handoff, honoring existing `[cost]` and privacy config:

```rust
pub struct EscalationPolicy {
    pub prefer_local: bool,
    pub escalate_if: Vec<EscalateWhen>,   // LowConfidence{th}, ToolUnavailableLocally, Connectivity::Online, BudgetRemaining
    pub privacy_keep_local: Vec<String>,  // tools/data that must never leave the device
}
```

Wired into both `Agent` and `EdgeAgent` so the same brain prefers the local model and reaches for the cloud only when policy says so. Reuses the existing `failover.rs`/`retry.rs` provider wrappers.

**Edge model management.** Extend the deployment planner (`src/deployment/`) to assign a per-node-role local model and provision/update it; surfaced in `[[deployment.hardware]]`.

**Config:**

```toml
[edge]
prefer_local = true
local_backend = "ollama"
local_model = "qwen3:4b-instruct"
escalate_on_low_confidence = 0.55
privacy_keep_local = ["camera_capture", "audio_sample"]   # raw media never leaves the node/SBC
```

### Firmware

- **On-device wake-word (TinyML).** Add a small always-on keyword spotter (Edge Impulse / microWakeWord-style int8 model via ESP-NN) so the ESP32-S3 only opens a realtime session (Phase 19) after "Hey Claw" — no cloud round-trip to start, big privacy win. Runs in a dedicated task; gated by a `wake_word` capability flag in the announcement.
- **On-device STT for commands (optional).** For fixed command grammars, a tiny on-device recognizer can resolve simple intents (e.g., "lights on") and fire a *reflex* (Phase 18) entirely offline; anything ambiguous escalates to System 2.
- SBC nodes (NanoPi/Pi/Jetson) run real small models through `EdgeAgent`; the MCU tier stays at wake-word + reflex scope, which is the right division for the silicon.

---

## Build, test, and rollout notes

- **Workspace.** New host modules slot into the existing `Cargo` workspace; the firmware crate (`firmware/obc-esp32-s3`, `no_std`-ish `esp-idf-svc` + `std`) stays separate. New firmware data structures favor `heapless` to stay off the heap.
- **Feature flags.** Gate heavy host deps behind Cargo features (`realtime`, `world-memory`, `durable`, `edge-local`) so minimal builds stay lean.
- **Evals as the gate (Phase 15 harness).** Each phase ships golden tests in `tests/evals.rs`: Track 0 → out-of-limit command is refused (host *and* firmware-sim); Phase 16 → unverified skill never offered, verified skill reused; Phase 17 → induced-crash resume causes no duplicate actuation; Phase 18 → reflex fires within latency budget offline, world-memory returns time-correct state; Phase 19 → spoken-interaction latency budget on the reference node; Phase 20 → defined task completes fully offline with audited fallback. No release while evals regress.
- **Firmware-in-the-loop sim.** Add a host-side `MockNode` that speaks the spine protocol (incl. `limits`/`reflex`/idempotency) so safety, durability, and reflex behaviors are CI-testable without hardware.
- **Backward compatibility.** All new `NodeAnnouncement` fields are `#[serde(default)]`; older firmware that doesn't set `enforces_limits` is treated as *untrusted for physical autonomy* — the host forces `Supervised` and refuses unattended actuation, which fails safe.

---

## Phase → module map (quick reference)

| Phase | New/changed host modules | Firmware work | New config |
|---|---|---|---|
| Track 0 | `tools/traits.rs` (RiskClass), `security/authz.rs`, `approval/` defaults | **`SafetyGate` enforced on-chip**, NVS limit table, `set/get_limits` | `[safety]`, `[[safety.limit]]` |
| 16 | `memory/trajectory.rs`, `skill_forge/synthesis.rs` + `evolve.rs` | result `seq`/`ok` reporting | `[self_improvement]` |
| 17 | `runtime/durable.rs`, orchestrator reuse | `call_id` idempotency dedup, structured `status` | `[autonomy.durable]` |
| 18 | `agent/reflex.rs`, `memory/world.rs` | **on-device reflex engine**, `set/get/clear_reflex` | `[perception]`, `[[reflex.rule]]` |
| 19 | `channels/realtime.rs`, audio/video bridge | A/V streaming mode, `stream_start/stop` | `[channels.realtime]` |
| 20 | `agent/escalation.rs`, `providers` local backend, `deployment` | **wake-word**, optional on-device STT | `[edge]` |

*The throughline: heavy cognition climbs to the host/cloud; safety and reflexes sink into the silicon. That division is what makes Oh-Ben-Claw a frontier agent that is also a trustworthy embodied one.*

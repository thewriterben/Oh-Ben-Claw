# Changelog

All notable changes to Oh-Ben-Claw are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## Unreleased — Isolated scratch engine for `reflex_tick` (A6 bench clean-up) (2026-07-02)

Follow-up to the on-bench validation: the manual `reflex_tick` command shared
debounce state with the live autonomous loop, so injecting a `now_ms` collided with
the real uptime clock and made individual manual fires look flaky. Fixed so a bench
tick is a clean, deterministic "what would this snapshot trigger?".

### Changed — `firmware/obc-esp32-s3/src/reflex.rs`, `main.rs`

- New `ReflexEngine::evaluate_scratch(&self, snapshot)` — a pure, non-mutating pass
  that returns every rule matching the snapshot, ignoring debounce/rate and never
  reading or writing `last_fire`.
- `reflex_tick` now calls `evaluate_scratch` instead of the stateful `evaluate`, so
  a manual tick no longer contends with the autonomous loop. The injected `now_ms`
  arg is ignored for the decision; any actuation is still gated with the real
  monotonic clock (`now_ms()`), so the Track 0 rate limit behaves correctly.
- The autonomous reflex loop still uses the stateful `evaluate` (debounce intact) —
  only the bench command path changed.
- New unit test `scratch_eval_ignores_debounce_history`: a rule debounced on the
  stateful path still reports through the scratch path.
- Bumped the USB-Serial-JTAG **TX buffer 256 → 4096 B**
  (`UsbSerialConfig::tx_buffer_size`) so multi-rule `reflex_tick` and
  `capabilities` replies fit in one write instead of truncating.

### Validated — on-bench (Seeed XIAO ESP32-S3)

- `reflex_tick {"sensor.battery_soc":6.0}` fires `safe-battery-critical` →
  drives the safe pin (GPIO21, onboard LED **lit**) through the Track 0 gate,
  plus `safe-battery-low` (escalate) — **deterministically, every call**. A6
  fully green: on-MCU battery self-protection confirmed on real silicon.

---

## Unreleased — XIAO ESP32-S3 port + first on-bench validation of the decision core (2026-07-02)

The node firmware ran on real silicon for the first time. Ported the command I/O
to the Seeed XIAO ESP32-S3 (the board actually on the bench), hardened the serial
protocol from the friction found while bringing it up, and validated the entire
on-MCU decision core — GPIO, the Track 0 gate, reflexes, and safing — over the
live link.

### Changed — XIAO ESP32-S3 port (`firmware/obc-esp32-s3/src/main.rs`)

- **Command channel moved from UART0 to the native USB-Serial-JTAG** (`usb_serial`,
  D-=GPIO19 / D+=GPIO20) — the only USB interface the XIAO's USB-C port exposes.
  Dropped the `uart0`/GPIO43-44 `UartDriver` path (unwired on this board). Host
  sends newline-delimited JSON and reads replies on the same connection (DTR must
  be asserted for the JTAG data path to flow).
- **XIAO-safe actuator pins:** `OUTPUT_PINS = [21, 3, 6, 7, 8]` — onboard user LED
  (GPIO21, active-low) plus exposed pads D2/D5/D8/D9. Deliberately avoids GPIO26-37
  (octal PSRAM), I2C (4/5) and I2S (1/2).
- `capabilities` now reports `board: seeed-xiao-esp32-s3`, `transport:
  usb-serial-jtag`, the real GPIO set, and `camera: false`.
- **PSRAM disabled** (`sdkconfig.defaults`) — octal PSRAM auto-init was crashing
  early boot (`Guru Meditation` after `octal_psram: BurstLen`) on this module; not
  needed for the non-camera build. Camera `extra_components` kept commented in
  `Cargo.toml` for lean default builds.

### Changed — serial protocol robustness (`main.rs`)

- **`send_line()` helper** replaces raw `uart.write` on every response path: loops
  over partial USB writes so long replies (e.g. `capabilities`) go out whole, with
  a bounded stall count so the node never blocks when the host isn't reading.
- **Refused/errored commands now always reply.** Wrapped the `handle_request`
  dispatch in a closure so a `?` (e.g. a Track 0 denial) returns into `result` and
  converts to `{"ok":false,"error":...}` instead of silently escaping the handler.
- **Argless commands parse** — `Request.args` is `#[serde(default)]`, so
  `capabilities` and friends no longer fail to deserialize.
- Line reader accepts `\r` as well as `\n` (terminal-agnostic).
- **On-change status reporting:** `link_state` and `power_mode` are emitted only
  when they change, not every tick — the link no longer floods with unchanged
  status, which is also the correct behaviour for a real node reporting to a host.

### Validated — on-bench (Seeed XIAO ESP32-S3, over native USB-Serial-JTAG)

- **GPIO** — host `gpio_write` drives pin 21 (LED). ✓
- **Track 0 gate** — pin-not-in-allow-list and value-out-of-range writes are
  refused with an error reply; allowed writes apply. ✓
- **Reflexes** — a pushed `overheat` rule fires on a `reflex_tick` and its
  `gpio_write` action routes through the gate (`applied:true`). ✓
- **Safing** — the built-in `safe-link-offline` rule fires **autonomously** on the
  link watchdog and again through a manual `reflex_tick`. ✓
- Note: manual `reflex_tick` shares debounce state with the live autonomous loop,
  so injecting arbitrary `now_ms` values collides with the real uptime clock and
  makes individual manual fires look flaky — a bench-harness artifact, not a
  firmware defect (the battery rule is covered by the passing
  `critical_battery_cuts_safe_pin` unit test and is structurally identical to the
  `safe-link-offline` rule that fires). A future option: give `reflex_tick` an
  isolated scratch engine so bench ticks don't contend with the loop.

---

## Unreleased — On-MCU Track 0 gate: host-pushable limits + rate limit (2026-06-30)

The ESP32-S3 firmware's Track 0 actuator gate went from three compile-time
constants to a real, evolvable policy — the last item its own doc comment flagged
as pending v2.0 work.

### Added — on-MCU `SafetyGate` (`firmware/obc-esp32-s3/src/safety.rs`)

- A pure (`std`+`serde`) node-side mirror of the host `security::limits::SafetyGate`:
  pin allow-list (default-deny), inclusive value range, and a **new per-pin rate
  limit** (min interval between writes, against the `esp_timer` monotonic clock).
- Wire-compatible `SafetyLimit` — a limit authored host-side validates identically
  on the MCU. `apply_pushed()` adopts the `gpio_write` limit addressed to this node,
  replacing the active policy but never silently dropping the gate.
- 6 unit tests (default == the old constants, host tightening, rate-limit
  block/allow, no-`gpio_write` push is a no-op, empty allow-list denies all, host
  JSON round-trip). Gate logic independently cross-checked.

### Changed — firmware integration (`firmware/obc-esp32-s3/src/main.rs`)

- The gate lives in `AgentState`; **all four actuation paths** (host `gpio_write`
  command, `reflex_tick`, the autonomous reflex loop, and the LLM edge-tool path)
  route through the one gate — no path can bypass Track 0.
- Boot policy reproduces the old `OUTPUT_PINS`/`0..=1` constants, so behaviour is
  unchanged until a host pushes something stricter.
- New **`set_limits`** command (mirrors `set_reflex_rules`): the host pushes
  `[[safety.limit]]` over `obc/nodes/{id}/limits` to tighten the allow-list / range /
  rate in the field with no reflash; acks the resulting active policy. Added to
  `capabilities`. Removed the old `safety_check_gpio_write` free fn + `GPIO_VALUE_MAX`.
- `.gitignore` added for the crate's `/target` build output.

---

## Unreleased — LoRa-mesh off-grid fleet + Ed25519 signed audit (2026-06-30)

Built the LoRa-mesh transport out from a codec into a complete off-grid coordination
path — real serial radio, node firmware, multi-hop flooding, and a transport-agnostic
assignment egress — and shipped the Ed25519 asymmetric audit that was previously
deferred. The fleet coordinator now coordinates a fleet with no WiFi and no broker,
and stays entirely blind to which transport (MQTT or LoRa) carries its messages.

### Added — Ed25519 signed audit (Accelerapp transfer F)

- **`src/security/audit_sign.rs`** — `AuditSigner` (Ed25519 keypair via
  `ed25519-dalek` v2) produces detached signatures over arbitrary bytes; `verify_hex`
  checks them against the **public** key, so any third party can verify audit
  integrity without holding the secret (non-repudiation). A real, audited crate —
  deliberately not the stub-crypto the cross-pollination analysis flagged.
- **Audit integration** (`src/security/audit.rs`) — `ActionRecord` gains an optional
  `sig` field (`#[serde(default)]`, back-compatible); `ActionAuditor::with_signer`
  signs each record's canonical form; `verify_signatures(path, public_hex)` audits a
  whole log. Additive — the HMAC hash-chain is untouched.
- `Cargo.toml`: `ed25519-dalek = { version = "2", features = ["rand_core"] }`.

### Added — LoRa-mesh: off-grid fleet coordination

- **RX bridge** (`src/spine/lora_mesh.rs`) — `ingest_line`/`bridge_frame` decode a
  received `MeshFrame` heartbeat into a `fleet::NodeState` and `report` it, so the
  auction/exploration logic runs over the mesh unchanged.
- **TX egress** — the coordinator gains a transport-agnostic **assignment outbox**
  (`with_assignment_outbox`/`drain_outbox`, bounded, opt-in); `tick`/`auction_tick`/
  `assign_exploration` enqueue `(node, x, y)` intents. `broadcast_outbox` /
  `send_assignment_frame` emit them as `MeshFrame::Assign`.
- **Multi-hop relay** (`lora_mesh::relay`) — optional `i` (id) + `h` (hops) envelope;
  `MeshRelay::on_receive` processes a new id once and rebroadcasts with `h-1`, drops
  repeats (bounded dedup). Backward-compatible with bare single-hop frames; needs no
  firmware change since the node relays opaque bytes.
- **Serial radio** (`#[cfg(feature = "hardware")]`) — `SerialMeshRadio` (a `MeshRadio`
  over `tokio-serial`) + `run_serial_rx`/`run_serial_rx_relay` RX loops, mirroring the
  existing Arduino driver.
- **Node firmware** (`firmware/lora-node/`) — a transparent USB-serial⇄LoRa byte
  bridge on RadioLib (T-Beam / Heltec / RAK4631), plus a `SELFTEST_HEARTBEAT` mode for
  hostless two-board bring-up.
- **Host wiring** (`src/main.rs`, `[fleet.lora_serial]`) — opens the serial node,
  spawns the relay RX loop, and runs a **unified assignment egress** that drains the
  outbox once and fans each intent to every connected transport (MQTT spine *and/or*
  LoRa mesh). Outbox auto-enables when any transport is present.

### Tests

- Ed25519 sign/verify round-trip, tamper + wrong-key rejection, hex round-trip.
- LoRa: RX heartbeat→coordinator bridge; outbox→`MeshFrame::Assign` broadcast +
  drain; relay flood/dedup/ttl-0; relayed ingest bridges + returns rebroadcast.

---

## Unreleased — SOTA depth, ClawCam bidirectional, Accelerapp cross-pollination (2026-06-30)

A long build-out across four threads: closing the last SOTA-comparison gaps with
production-grade implementations, making ClawCam a fully bidirectional embodied
subsystem, importing nine patterns from the sibling Accelerapp project, and
activating three tested-but-dormant subsystems by building real consumers.

### Added — embodied depth (SOTA parity)

- **Likelihood-field sensor model** (`src/navigation/sensor_model.rs`) — a chamfer
  Euclidean distance field + Thrun §6.4 mixture; `ParticleFilter::update_scan` is
  the real range-sensor measurement update (≈ AMCL), replacing the toy position
  Gaussian.
- **KLD-adaptive particle filter** (`src/navigation/particle.rs`) — Fox-2003
  sample-size bound; the cloud grows when uncertain, shrinks when confident.
- **Fleet task auctions** (`src/fleet`) — market-based sequential-auction
  allocation (`auction_allocate`/`auction_tick`), globally cheaper and
  order-independent vs per-task greedy; bids include battery eligibility.
- **EWLS online forecaster** (`src/foresight`) — `Forecaster::with_decay` turns
  equal-weight OLS into exponentially-weighted least squares so trends track regime
  changes (`decay == 1.0` preserves prior behavior).
- **HIL loop test** (`tests/embodied_hil_loop.rs`) — a ClawCam detection flows
  through the real ingest → world memory → hazard policy → occupancy → A* detour →
  Track 0–bounded drive, nothing mocked.

### Added — ClawCam as a bidirectional embodied subsystem (`src/vision/`)

- **Full perceive ingest** — converters folding ClawCam node health → `clawcam.node.*`
  facts, audio classifications → the audio suite (`audio.clawcam:{node}`), and a
  rolling `vision.count.{subject}` for foresight rate-trending; opt-in via
  `[perception.clawcam_poll] poll_health/poll_audio`.
- **Vision-driven rules** (`clawcam_rules`) — reflex (verified subject → escalate,
  optional capture) + foresight (rising sighting rate → escalate) rule libraries,
  live via `[perception.vision_rules]`.
- **Close the loop** (`clawcam_actuate`) — `ClawCamActionSink` translates
  `clawcam/cmd/*` reflex publishes into ClawCam's gated write tools (capture / arm /
  alert) over the shared MCP bridge; wired into the reflex sink chain.
- **Spatial fusion** (`clawcam_spatial`) — `CameraMap` + `mark_detection_hazard`
  stamp a camera detection into the nav costmap (core; wired on demand).

### Added — Accelerapp cross-pollination (nine transfers)

Grounded in Accelerapp's *real* patterns, avoiding its stubs (see
`docs/ACCELERAPP-CROSS-POLLINATION.md` for the delivered/deferred status table):

- **Dynamic trust scoring** (`src/security/trust.rs`) — per-node behavioral score
  (rolling-mean + 3σ anomaly, failure decay, recovery) → `TrustLevel`; `gate()`
  tightens physical-action approval as trust falls. Wired into `ApprovalManager::decide`
  and the agent dispatch (`[safety] dynamic_trust`).
- **Hardware harvest** (`src/peripherals/registry.rs`) — `mesh`/`ibutton`/`psram`
  tokens, RAK4631 Meshtastic node, board enrichment.
- **LoRa-mesh transport** (`src/spine/lora_mesh.rs`) — compact fleet-frame codec +
  pluggable `MeshRadio`, bridging to the fleet coordinator (off-grid, no broker).
- **No-op-fallback exporter** (`src/observability`) — `ReconcilingExporter` buffers
  metrics offline, reconciles on reconnect; env-gated loop in `main`.
- **Node self-test + MockNode** (`src/peripherals/selftest.rs`) — bring-up contract
  + host-side simulator; composed end-to-end in `tests/offgrid_fleet_loop.rs`.
- **Saga rollback** (`src/deployment/saga.rs`) — compensating-action unwind for
  multi-node deployment.
- **Vendor allowlist** (`src/peripherals/onboarding.rs`), **model registry**
  (`src/providers/model_registry.rs`), **firmware scaffold**
  (`src/deployment/firmware_scaffold.rs`).
- **F** Ed25519 asymmetric audit — now shipped (see the LoRa-mesh + signed-audit
  section above); the earlier offline-cache block was lifted by adding
  `ed25519-dalek`.

### Added — consumers for dormant subsystems

- **`ApprovalManager` activated in the live dispatch** (`src/agent/mod.rs`) — every
  tool call now gated by autonomy level + auto-approve + grants (+ trust) via
  `approval_authorize`/`decide`; `main` attaches it. Default `Full` = behavior-
  neutral; supervised/manual now actually enforce.
- **`VendorAllowlist` → doctor** — `check_hardware_onboarding` flags configured
  boards from unrecognized vendors.
- **`ModelRegistry` → edge** — `EdgeAgentBuilder::prefer_local()` selects the
  on-device model first over the fallback chain.
- **Firmware scaffold → deployment planner** — `DeploymentScheme::firmware_sketches()`
  emits a starter sketch per flashable MCU node.

### Docs

- README rewritten around the embodied control stack; `docs/EMBODIED-ARCHITECTURE.md`
  gained a ClawCam bidirectional section; `docs/ACCELERAPP-CROSS-POLLINATION.md`
  banked with delivered-vs-deferred status; ClawCam `NEXT_PHASE_PLAN.md` records the
  OBC-side integration in lockstep.

---

## Unreleased — Hardware registry: scout 2026-06-29 AI accelerators

Stacks on the tier-1 additions below. Adds the AI-accelerator hardware from the
scout report and wires it into deployment matching so the accelerator tokens
resolve to a feature desire (previously inert).

### Added — accelerator hardware

- **Boards:** Google **Coral USB Accelerator** (`edge_tpu`, VID/PID verified),
  **Coral Dev Board Mini** (`edge_tpu`), **Radxa ROCK 5B** (RK3588, `npu`,
  `ethernet`), **NVIDIA Jetson Orin Nano** (`cuda` + `tensor_rt`; shares the
  Jetson USB id, selected by `name`), **M5Stack Module LLM** (AX630C, `npu`).
- **Accessories:** **Raspberry Pi AI HAT+ 13 TOPS** (Hailo-8L, `hailo`, PCIe,
  RPi 5) and **Seeed Grove Vision AI Module V2** (`nn_accel`, Grove).

### Added — deployment matching (`deployment::inventory`)

- New **`FeatureDesire::AcceleratedInference`** — satisfied by any accelerator
  token (`cuda`/`tensor_rt`/`npu`/`edge_tpu`/`hailo`/`kpu`/`nn_accel`), distinct
  from host-level `EdgeInference` (which stays CPU-satisfiable). Plus
  **`LongRangeRadio`** (`lora`), **`Localization`** (`gps`), **`Actuation`**
  (`actuate`) — the last also makes existing LoRa boards matchable.
- Advisor tests confirm `AcceleratedInference` resolves to an accelerator board
  and produces a suggestion on a CPU-only host.

### Notes

- Regenerate `registry.json` (`cargo run --bin emit-registry -- registry/registry.json`).
- Follow-up: blocked-PID entries (Adafruit Feather ESP32-S3, Sipeed MaixCAM
  `kpu`) once USB IDs are confirmed; optional `Connector::Gravity`.

---

## Unreleased — Hardware registry: scout 2026-06-29 tier-1 additions

From the weekly hardware-scout report (`Knowledge Base/hardware-scout-2026-06-29.md`).
Metadata-only additions on already-supported transports — no firmware change.
Accelerator boards (Hailo / Coral / Jetson Orin / ROCK 5B / M5 LLM) and
blocked-PID entries (Adafruit Feather ESP32-S3, Sipeed MaixCAM) remain follow-ups.

### Added — capability taxonomy

- **`VALID_CAPABILITIES`** in `peripherals::registry` — canonical token set with
  `is_valid_capability()` and an `all_capabilities_are_valid` test that fails the
  build if any board/accessory uses an undocumented token (typo guard). New
  tokens documented in the module header and reserved for upcoming hardware:
  `npu`, `edge_tpu`, `hailo`, `nn_accel`, `kpu`, `tensor_rt`, `ethernet`,
  `thread`, `zigbee`, `battery`.

### Added — boards

- **ESP32-C6**, **ESP32-H2** (BLE + 802.15.4 `thread`/`zigbee`; H2 has no Wi-Fi)
  and **ESP32-P4** (`nn_accel`, MIPI camera/display) Espressif SoCs.
- First **Adafruit** (QT Py ESP32-S3, STEMMA QT), **SparkFun** (Thing Plus
  ESP32-C6, Qwiic) and **DFRobot** (FireBeetle 2 ESP32-S3, `battery`) boards.
- **LILYGO T-Display-S3** and **T-Deck** (ESP32-S3; T-Deck adds LoRa + touch).

### Added — accessories

- Qwiic / STEMMA QT plug-in sensors that exercise connector matching:
  **SCD41** (CO2), **VL53L1X** (ToF), **BNO055** (9-DOF fusion IMU), **SGP40** (VOC).

### Notes

- Native-USB ESP32 parts share `0x303a:0x1001`; selected by `name` per existing
  convention. `registry.json` must be regenerated
  (`cargo run --bin emit-registry -- registry/registry.json`) — it is a build
  artifact, not hand-edited.

---

## Unreleased — Phase 19: Foresight & Autonomy (2026-06-25)

Beyond reactive and deliberative control: a predictive layer, self-improvement,
autonomous exploration, and uncertainty-aware localization. These exploit the
bitemporal world memory and the navigation stack to reach toward state of the art.

### Added — Foresight (Track 1, predictive control)

- **`src/foresight`** — a [`Forecaster`] fits a linear trend over an entity's recent world-memory history (`predict_at`, `time_to_threshold`). **`ForesightRule`** fires when an entity *is, or is predicted within a horizon to be*, `op` a threshold — acting *before* the event (e.g. `battery ≤ 10% within 60s → return to base` while still at 20% but draining). `ForesightEngine`/`ForesightController` dispatch through the reflex `ActionSink` + escalation budget; predictions are recorded to `foresight.{entity}`. The `foresight` tool (read-only) queries any entity's forecast.

### Added — self-authored reflexes (experiential rule synthesis)

- **`src/learning`** — `RuleMiner` scans history for antecedents that repeatedly preceded a configured bad outcome, proposing rules with support + confidence (specificity-filtered). `ProposalStore` is the **approval gate**: an approved proposal is pushed as a conservative (escalate-only) rule into the foresight engine's shared learned-rules buffer — **live on the next tick**, but never without approval. The `learn` tool exposes `mine`/`list`/`approve`/`reject`; an optional auto-mine loop proposes continuously.

### Added — autonomous exploration

- **`src/navigation/exploration`** — frontier detection (`Free` adjacent to `Unknown`) + nearest *reachable* frontier selection (A*-checked). `NavController::explore_step` heads to the next frontier when idle; `[navigation] explore = true` makes the robot map an unknown space on its own, composing SLAM + mapping + planning + drive with no human waypoints.

### Added — belief-state localization

- **`src/navigation/particle`** — a particle filter over SE2 poses: noisy motion proposal, Gaussian measurement reweighting, low-variance resampling, and a weighted/circular estimate **with a position spread** (honest uncertainty). Deterministic PRNG (no new dep). `ParticleLocalizer` records the belief (`sensor.pos_*` + `nav.belief`) so navigation reads the filtered pose and the stack can act on uncertainty.

### Tests

- Per-module unit tests throughout (forecast trend + predictive firing; antecedent mining + approval gate; frontier detection + exploration step; particle convergence + spread shrink + resampling invariants).

---

## Unreleased — Navigation, SLAM & Mission Sequencer (2026-06-25)

The embodied stack's upper layers: a full localization → mapping → planning →
driving navigation column, drift-corrected by pose-graph SLAM, with a
deliberative mission sequencer on top. Capstone reference: `docs/EMBODIED-ARCHITECTURE.md`.

### Added — navigation suite (the fusing subsystem)

- **`src/navigation`** — `NavController` localizes from sensor pose facts and drives toward a goal via a steer servo + drive motor through the (Track 0–bounded) movement controller; records `nav.pose`/`nav.goal`/`nav.status`. Tools: `navigate` (gated, plans around obstacles) + `nav_status` (safe: status/stop) + `nav_map` (safe: mark/free/scan/status). `[navigation]` config.
- **Waypoint paths** — a waypoint queue (`set_path`), `WaypointReached` outcomes, advance-on-arrival; the `navigate` tool accepts a `waypoints` array.
- **Pose fusion** (`navigation/pose_fusion`) — weighted multi-source localization with circular heading mean → canonical `sensor.pos_*` (`[[navigation.pose_source]]`).
- **Closed-loop movement** (`src/movement/feedback`) — bounded `PController` + `ClosedLoopServo` stepping the gated controller toward a target from a feedback fact.

### Added — SLAM, mapping, planning

- **Pose-graph SLAM** (`navigation/slam`) — SE2 `compose`/`relative_between`, `PoseGraph` with odometry + loop-closure edges, anchored Gauss-Seidel relaxation that distributes drift; `SlamBackend` auto-detects revisits and writes the **corrected** pose to world memory.
- **Occupancy grid + A\*** (`navigation/planning`) — `OccupancyGrid` + A* planner producing simplified turn-point waypoints; obstacle-aware `navigate` plans over it.
- **Online mapping** (`navigation/mapping`) — Bresenham ray-cast scans into the grid (clear free, mark hits, sticky obstacles); `nav_map scan` + `NavController::integrate_scan`.

### Added — mission sequencer (deliberation)

- **`src/mission`** — `MissionRunner` executes a guarded `Mission` of `MissionStep`s (`navigate_to`/`wait`/`speak`/`record`/`await_state`), reactive and one-step-per-tick, with reflex-`Condition` guards that **preempt and halt** on a bad mode. Tools: `mission` (gated start) + `mission_status` (safe status/abort/list). `[mission]` config with a named library; `main` runs the tick loop over nav + audio + world memory.

### Tests

- Per-module unit + tool tests across navigation/SLAM/mapping/mission, plus `tests/embodied_full_stack.rs` — a grand scenario exercising mission → obstacle-aware navigate → gated actuation → battery-driven safing engage → guard preemption → recovery, as one composed unit.

---

## Unreleased — Embodied Subsystem Suites + Safing (2026-06-25)

A breadth-then-depth build-out: four new capability suites on the shared
perceive → remember → react → act spine, reflexes that react to categorical
modes, and a self-healing safing layer — all Track 0–bounded, world-memory
recorded, and verified end to end.

### Added — capability suites

- **Sensing suite** (`src/sensing/mod.rs`) — `SensingController` ingests `Sample`s, classifies each against a `QuantitySpec` (range → `out_of_range`, freshness → `stale`), records `sensor.{quantity}` facts with a `quality` flag, and surfaces `anomalies()`. Exposed via the `sense` MCP tool (`src/tools/builtin/sensing.rs`; ingest/current/history/anomalies; `RiskClass::safe`). `[sensing]` config with `[[sensing.quantity]]` specs.
- **Audio suite** (`src/audio/suite.rs`) — bidirectional. *Perceive:* `AudioController::observe` classifies a `HeardEvent` for reliability and records `audio.{stream}`. *Act:* `speak` records `speech.last` and emits through a pluggable `SpeechSink`. Tools `hear` (safe) + `speak` (physical, low-blast, recorded but not approval-gated) in `src/tools/builtin/audio_suite.rs`. `[audio_suite]` config.
- **Power suite** (`src/power/mod.rs`) — `PowerController.ingest(BatteryReading)` derives a `PowerMode` (`normal`/`low`/`critical`/`charging`) from SoC + charge state vs thresholds, recording `power.battery` + a dedicated `power.mode` reflex hook. `power` MCP tool (`src/tools/builtin/power.rs`). `[power]` config.
- **Comms suite** (`src/comms/mod.rs`) — `CommsController.ingest(LinkReading)` classifies each link (`online`/`degraded`/`offline`/`unknown`), records `link.{name}`, and aggregates the best link into a `net.mode` hook. `comms` MCP tool (`src/tools/builtin/comms.rs`). `[comms]` config.

### Added — reflexes & safing (System 1 depth)

- **`Condition::State`** (`src/agent/reflex.rs`) — categorical match on a fact's string value or a nested field, so reflexes can react to the suites' mode hooks (`power.mode`, `net.mode`, `audio` labels, sensor `quality`). New `Snapshot { nums, vals }`; numeric path unchanged.
- **Safing rule library** (`src/agent/safing.rs`) — canonical, debounced rules: power critical (escalate + optional Track 0 `Stop`), power low (shed-load advisory), net offline/degraded, audio-alarm, out-of-range sensor, numeric overheat. Merged into the live controller via `[reflex] safing = true` (+ `safing_stop_actuator`, `safing_alarm_streams`, `safing_unreliable_sensors`, `[[reflex.safing_overheat]]`).
- **Self-healing recovery** — `safe-power-recovered` / `safe-net-recovered` publish `clear_*` advisories when modes return to normal; `SafingState` releases the matching flags automatically.
- **In-process safing executor** — `SafingState` (atomic flags) + `SafingSink` tap `obc/safing` advisories so the host actually backs off; the ClawCam detection poll sheds (skips) while `shed_load` is engaged and resumes on recovery.

### Added — real sinks & closed-loop control

- **`SpineSpeechSink`** / **`TtsSpeechSink`** — emit speech over the spine (`obc/speech`) or render locally via TTS; `main` selects by config/connection, dry-run otherwise. `SpineActuatorSink` drives movement nodes over the spine.
- **Closed-loop movement** (`src/movement/feedback.rs`) — bounded `PController` + `ClosedLoopServo` reads a world-memory feedback entity and steps the gated `MovementController` toward a target (Suite §6 Accelerate, L3).

### Added — registry & docs

- **Subsystem-suite hardware** in the registry SSOT (`src/peripherals/registry.rs`): capability tokens `actuate`, `audio_output`, `cellular`; accessories sg90, tb6612fng, pca9685, inmp441, max98357a, max17048, sim7600. Regenerate `registry.json` via `emit-registry`.
- **`docs/SUBSYSTEM-SUITES-STATUS.md`** — as-built status: suite table, world-memory hooks, MCP tool registry with risk classes, reflex reference, safing table, sink matrix, config block.

### Tests

- Per-suite unit + tool tests; safing end-to-end tests (controller → world memory → reflex fires/recovers); reflex `State` tests; closed-loop convergence test; registry accessory tests; and `tests/embodied_safing_loop.rs` — a 3-scenario integration test (battery drain, network loss, independent recovery) exercising the whole spine as a unit.

---

## Unreleased — Phase 15 Production Hardening, WS1 (2026-06-05)

### Added

- **Skill-Install Security Policy** — ClawHub installs are now gated (`src/skill_forge/install_policy.rs`): explicit operator consent required by default (`InstallConsent`), allowlist ("vetted mirror") mode, per-skill version pinning, SHA-256 checksum verification against catalogue-provided hashes, and static manifest inspection that flags external URLs, `Shell`-kind execution, and download-instruction language (the ClawHavoc-era `SKILL.md` evasion pattern). Every decision — allow, deny, or approval-required — is appended to a JSONL audit log (`~/.oh-ben-claw/skill_install_audit.jsonl`).
- **`[clawhub.install_policy]` config section** — `require_approval`, `require_checksum`, `pinned_versions`, `allowlist`, `audit_log_path` (`src/config/mod.rs`)
- **`ClawHubEntry.sha256`** — optional manifest checksum field, populated by registries that publish signing hashes
- **`ClawHubClient::with_policy()`**, `policy()`, `audit_log()` accessors; `install()` now takes an `InstallConsent` parameter and refuses ungated installs

### Security

- `ClawHubClient::install()` no longer writes any manifest to the skills directory without passing policy evaluation; previously installs were unconditional.

### Added (WS2 — MCP 2026-07-28 dual-mode)

- **`ProtocolMode`** (`legacy-2024` / `stateless-2026`) with per-mode version constants; `protocol_mode` field on `McpServerConfig` (`src/mcp/mod.rs`)
- **2026-mode client** (`src/mcp/client.rs`): skips the removed `initialize` handshake, attaches `_meta.io.modelcontextprotocol/clientInfo` to every request (SEP-2575), sends `MCP-Protocol-Version`/`Mcp-Method`/`Mcp-Name` HTTP headers (SEP-2243), fetches capabilities via `server/discover` (tolerant of servers without it), records `ttlMs` from `tools/list` (SEP-2549); legacy mode no longer declares the deprecated `roots`/`sampling` capabilities (SEP-2577)
- **Bilingual server** (`src/mcp/server.rs`): answers both `initialize` (legacy) and `server/discover` (2026); `tools/list` now carries `ttlMs`/`cacheScope`; HTTP transport validates routing headers — mismatches rejected always, headers required in `stateless-2026` mode; `McpServer::with_mode()` constructor
- 16 new unit tests across client `_meta` merging, mode serde, discover, handshake-less calls, ttl, and header validation

### Changed (WS3 — A2A v1.0 conformance, BREAKING for `src/a2a` consumers)

- **A2A module rewritten against the v1.0 specification** (`src/a2a/mod.rs`). The Phase 14 implementation predated the stable spec and matched neither v0.3.0 nor v1.0 on the wire. Now conformant (JSON-RPC binding subset):
  - `AgentCard` v1.0 shape: `supportedInterfaces[{url, protocolBinding, protocolVersion}]` replaces top-level `url`; required `version`, `capabilities`, `defaultInput/OutputModes`; skills carry required `id` + `tags`; camelCase throughout
  - Discovery moved to `/.well-known/agent-card.json` (was `agent.json`)
  - PascalCase operations: `SendMessage`, `GetTask`, `CancelTask`; everything else returns `UnsupportedOperationError` (-32004)
  - `TaskState`/`Role` serialize as proto names (`TASK_STATE_*`, `ROLE_*`)
  - `Part` oneof with **no `kind` discriminator** (text/raw/url/data by member presence); `mediaType` replaces `mimeType`
  - `Task{id, contextId, status{state,message,timestamp}, artifacts, history}`; `Artifact.artifactId`; `Message{messageId, role, parts}`
  - A2A error codes -32001…-32009 with `google.rpc.ErrorInfo` in `error.data` (`domain: "a2a-protocol.org"`)
  - `A2A-Version: 1.0` header sent by client and validated by server (absent ⇒ 0.3 ⇒ `VersionNotSupportedError` per spec)
  - In-memory task store on the server so GetTask/CancelTask lifecycle is real; 18 conformance unit tests
- Removed pre-spec types `A2ASkill`, `TaskRequest`, `TaskResponse` (replaced by `AgentSkill`, `Message`, `Task`). No code outside `src/a2a` referenced them.

### Added (WS6 — scoped approvals)

- **Approval scopes** (`src/approval/mod.rs`): `ApprovalScope` (call / session / forever); the prompt gains `[f]orever`; forever grants persist to `~/.oh-ben-claw/approval_grants.json` via `ForeverGrants` (grant/revoke/list); `always_ask` still overrides any grant
- **Plan-mode approval**: `ApprovedPlan` of `PlanStep`s with `ArgumentBound`s (`Exact`/`OneOf`/`Range`/`Any`, optional `deny_unlisted_args`); approve once via `approve_plan()`, execution checked step-by-step via `check_plan_call()`; **any violation revokes the plan** (halt on drift) and is audited
- **Approval funnel analytics**: per-tool asked/approved-by-scope/denied/plan-violation counters via `funnel_summary()`
- `record_external_decision()` so chat/dashboard approvals share the same grants, audit, and funnel
- 16 new unit tests (scopes, persistence, plan happy-path/drift/bounds, funnel)
- **ClawCam adapter parity**: `ApprovalGrants` (session in-memory, forever persisted JSON), `call_tool(..., approved, scope)`, approval audit + funnel — verified, 10 new tests, 22/22 with MCP suite

### Added (WS4 — evaluation harness)

- **Eval suite as release gate** (`tests/evals.rs`): agent-loop routing goldens against a deterministic `ScriptedProvider` (direct answer / single-tool with exact args / multi-step ordering / tool-failure recovery / unknown-tool degradation), MCP and A2A wire-shape goldens, approval policy matrix golden. Runs under `cargo test --workspace` in CI — no release while evals regress. LLM-as-judge scoring deferred (advisory-only by design until variance is measured).
- **ClawCam counterpart** (`tests/evals/`, verified 7/7): full approval-policy partition eval (every catalogued tool in exactly one bucket; all 9 gated tools behaviorally ask; auto-approved never do) and golden detection pipeline (event → MockDetector → alert linkage, determinism contract). `tests/evals` added to pytest testpaths so the existing CI workflow gates on it.

### Added (WS5 — observability wiring)

- **Agent loop instrumented** (`Agent::with_obs()`): `agent.process` span per run (session_id + tool_calls attrs), `agent.tool` span per call with error status, turn/tool/error counters recorded at source. The `src/observability` foundation (spans, sink, counters, gateway `/api/v1/metrics`) already existed — the agent loop was the blind spot.
- **`ApprovalManager::with_obs()`** — approval asks counted centrally (`approval_asks_total`); `record_retry`/`record_failover` helpers added to `ObsContext`
- 2 new observability evals in `tests/evals.rs` (spans + exact counter goldens; no-obs path unaffected)
- **ClawCam counterpart** (verified 38/38 with regression scope): `tool_call_audit` SQLite table written inside `dispatch_tool` (both MCP-stdio and REST callers tagged via `source`; SHA-256 args hash; latency; audit never blocks dispatch); new `GET /api/v1/metrics` (entity counts + per-tool call/error/latency stats) and `GET /api/v1/tool-audit`

### Fixed

- **Windows shell skills** — `SkillKind::Shell` execution now uses `cmd /C` on Windows instead of hardcoded `sh -c` (`src/skill_forge/mod.rs`); fixes the two `shell_skill_*` test failures on Windows hosts.
- **Windows builtin ShellTool** — same platform-aware fix for `src/tools/builtin/shell.rs` (was hardcoded `/bin/sh -c`); fixes `shell_echo` on Windows. Found by the first full Windows test run (684/685 → expected 685/685).
- Clippy: `sort_by` → `sort_by_key(Reverse(…))` in `approval` and `rag`; boxed `StdioTransport` in the MCP client transport enum; `McpServer::handle_request` made public as the embedder/eval API.

> **Verification:** `cargo test skill_forge` — 17/17 new install-policy tests pass; 53/55 module tests passed on first Windows run, with the 2 pre-existing `sh`-dependent failures fixed by the platform-aware shell change.

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

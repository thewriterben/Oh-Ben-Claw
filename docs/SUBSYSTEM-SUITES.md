# Embodied Subsystem Suites — Vision (ClawCam), Sensing, Movement

*Compiled June 23, 2026. Companion to `V2-STRATEGY.md`, `V2-IMPLEMENTATION.md`, `ECOSYSTEM-INTEGRATION.md`, `ACCELERAPP-CROSS-POLLINATION.md`.*

This plan does two things: (1) integrate **ClawCam** as Oh-Ben-Claw's full **Vision Suite** — one that learns, remembers, improves, and accelerates — and (2) generalize that into a reusable **Subsystem Suite** architecture so the same is done for **Sensing**, **Movement**, and any future system.

---

## 1. What ClawCam is (the examination)

ClawCam is a ~204-file Python **smart-camera platform**, at Phase 12 (simulator-verified). Three layers:

- **Node** — ESP32-S3-EYE firmware (ESP-IDF C): PIR→capture→deep-sleep, NVS config, capability-group handshake, OTA-via-command. Real code, not yet hardware-validated (pinmap flagged unverified).
- **Gateway** — offline-first FastAPI + SQLite station (runs on a Pi): ~70 REST endpoints, MQTT bridge, OTA queue, alert rules + webhooks, cron schedule engine, detection zones/privacy masks, audio pipeline, multi-tenant auth, pluggable cloud (Noop/S3/GCS). This is the durable system of record.
- **Brain** — **Oh-Ben-Claw itself**, connected over an MCP stdio bridge.

**It is already wired to OBC.** The `ClawCamAdapter` (`brain/oh-ben-claw-adapter/clawcam_adapter.py`) spawns the gateway's stdio MCP server and exposes **32 tools — 23 read (auto-approved) + 9 write (approval-gated)** — into OBC's unified registry via `as_obc_tool_entries()` (shaped for OBC's `McpToolEntry`). It mirrors OBC's approval vocabulary (`call`/`session`/`forever` + `ForeverGrants`), runs dual-mode MCP (legacy 2024 + stateless 2026), writes a `tool_call_audit` row per call, and ships an eval harness (7/7) that behaviorally verifies the read/write partition. ClawCam's Phase 13 is explicitly run "in lockstep with OBC Phase 15."

**The honest maturity read (so we build the right things):**

- **Real & strong:** the gateway, the SQLite schema, the offline-first design, the **detector registry** (`inference/registry.py` — a clean name→factory, lazy-loaded, per-profile/per-device **chain** architecture; genuinely plugin-like), the zone/mask geometry, the cron schedule engine, and the entire OBC/MCP integration contract.
- **Wired but mocked:** MegaDetector v5 and BirdNET are *real wrappers* but fall back to `MockDetector` because no weights/libs ship; face/plate/glassbreak detectors are mock placeholders. So **real ML inference is supported but not actually running by default.**
- **The big gap — there is no learning, memory, or improvement loop at all.** No retraining, no active learning, no feedback-to-model path, **no embeddings / re-identification / world memory** — the same animal is never linked across events. A non-destructive **review-state** model (`unreviewed/verified/corrected/rejected/needs_review`, Camtrap-DP-aligned) is *documented* in `docs/DATA_MODEL.md` but **not implemented** (no DB column, no endpoint).

That gap is not a flaw to apologize for — it's precisely the opening. **ClawCam already is a capability suite plugged into the brain; what it lacks (learn / remember / accelerate) is exactly the v2.0 frontier applied to vision.**

## 2. The thesis: ClawCam is the reference Subsystem Suite

The most useful reframing: don't treat "integrate ClawCam" as a one-off. ClawCam is the **first instance of a general pattern** — an embodied capability suite that the OBC brain orchestrates. Define that pattern once, complete it for vision, then instantiate it for sensing and movement.

A **Subsystem Suite** is a self-contained capability module (its own repo, own language, own hardware) that implements eight contract points against the OBC brain:

| # | Contract point | What it means | ClawCam today |
|---|---|---|---|
| 1 | **Perceive / Act** | Expose domain tools over MCP: reads = perception/query, writes = action | ✅ 23 read + 9 write |
| 2 | **Connect** | Register tools into OBC's registry; advertise capabilities; (optionally) join the MQTT spine | ✅ adapter → `McpToolEntry` |
| 3 | **Remember** | Write domain observations into OBC's shared **bitemporal world memory** (Phase 18); non-destructive corrections | ❌ flat rows, no world memory, no re-ID |
| 4 | **Learn** | Capture feedback/trajectories; run an improvement loop; synthesize skills (Phase 16) | ❌ review-state spec'd, unbuilt |
| 5 | **Improve** | Versioned models with promotion gates + eval; confidence calibration | ◑ model_version columns; registry; no promotion loop |
| 6 | **Accelerate** | Real edge inference; dual-system fast-reflex + slow-reason split (Phase 20/18) | ◑ registry ready; models mocked |
| 7 | **Stay safe** | World-changing actions flow through Track 0 (risk class, approval scope, signed audit) | ✅ writes approval-gated |
| 8 | **Be observable & evaluable** | Per-tool audit + metrics; golden eval gate in CI | ✅ tool_call_audit + evals |

ClawCam already implements 1, 2, 7, 8 and half of 5. The integration completes **3 (remember), 4 (learn), 6 (accelerate)** — and that same contract becomes the template every future suite fills in.

### Architectural decision: keep suites separate, bound by the contract

**Do not merge ClawCam into the OBC repo.** The right architecture is what already exists, deepened: independent suites (Python for the vision/ML ecosystem, Rust for the brain/runtime, C for firmware) that plug into the brain over **MCP (tools) + the spine (telemetry) + shared world memory (state) + Track 0 (safety)**. Merging would forfeit polyglot strengths and independent deployment. The "integration" work is *strengthening the contract*, not collapsing the codebases. This is the embodied-fleet thesis made concrete: one brain, many suites.

## 3. ClawCam → Vision Suite: the plan

Each step maps to a v2.0 capability and builds on ClawCam's existing substrate.

### V0 — Consolidate the existing contract *(low effort, do first)*
Tighten what's already there so the suite contract is clean before extending it.
- Fix documented drift: the stale 5-tool `brain/tools/clawcam_tools.json`, the 16-vs-32 tool listing in the gateway's HTTP `/api/v1/tools`, and the `docs/integration` / `docs/standards` docs that lag the live 32-tool catalog.
- Commit the cross-repo `NEXT_PHASE_PLAN.md` (referenced by both repos but absent) and finish the Phase 13↔15 lockstep items: **plan-mode approval with argument bounds** (the missing piece vs OBC's `ApprovedPlan`) and **wiring `tests/evals` into the CI gate**.
- Write the formal **Subsystem Suite contract** (this doc's §2 table) into both repos as the shared standard.

### V1 — Remember (world memory) → *OBC Phase 18*
Turn flat detections into persistent perceptual memory.
- Implement the documented **review-state** model in ClawCam storage (the non-destructive label schema) — the foundation for both memory and learning.
- Add **re-identification / embeddings** so the same subject (individual animal, known face, vehicle plate) links across events, and feed those as entities into OBC's **bitemporal world memory** (`src/memory/world.rs`): `entity = "deer#7" | "plate ABC123"`, with valid-time intervals ("seen at cam-2, 06:14"). ClawCam becomes the vision sensor that *populates* world memory; the brain queries it ("has this individual been seen before? where? when?").
- ClawCam's append-only audit tables (`state_transitions`, `inference_results.ran_at`, `alert_events`) are a ready event-sourced substrate; add the second (transaction-time) axis for true bitemporality.

### V2 — Learn (active-learning loop) → *OBC Phase 16*
Close the feedback loop so the suite gets better with use.
- Route low-confidence / novel detections into a **review queue** (`needs_review`); a human *or the OBC brain* (via the existing `species-review-workflow` skill) confirms/corrects → labeled ground truth.
- That corrected label is the **self-verification signal** Phase 16 requires (reflection without a real signal degrades): accumulate corrections into a dataset; periodically improve detector thresholds/heads and synthesize reusable skills ("at cam-2 after dusk, raccoon confidence runs low → escalate").
- Track metrics: review throughput, correction rate, confidence calibration over time.

### V3 — Accelerate (real edge inference) → *OBC Phase 20 + 18 dual-system*
Make perception real, fast, and local.
- Register **real models** into the detector registry (ship/resolve MegaDetector + BirdNET weights; add SpeciesNet) and **accelerator-backed detectors** (Hailo / Coral / Jetson on the gateway; ESP-DL / LiteRT-Micro on the ESP32-S3-EYE) — new factory entries, no caller changes (the registry is built for exactly this).
- Implement the **dual-system split**: a fast on-device trigger/filter (System 1: "is there motion that looks animal-shaped?") gates the slow, full gateway/cloud inference (System 2) — saving power and bandwidth, and letting the node act offline.
- This is where the v2.0 AI-accelerator capability tokens (`hailo`, `edge_tpu`, `npu`, `nn_accel`) and the just-seeded camera boards (ESP32-CAM, ESP32-S3-CAM, CYD) become the vision suite's hardware tier.

### V4 — Self-improving & safe by construction
- Every world-changing vision tool (`capture_now`, `apply_config_patch`, `queue_firmware_update`, `set_device_detector_chain`, …) already maps onto Track 0; assign each a `RiskClass` and route through the signed-action audit. A *synthesized* vision skill that triggers capture or reconfigures a node passes Track 0 staged rollout before running unattended.
- Use ClawCam's cron **schedule engine** as a ready instance of the Phase 17 long-horizon autonomy loop (scheduled scans, nightly summaries) with its existing run-audit.

## 4. Generalize: Sensing and Movement suites → *OBC Phase 18/Track 0*

Instantiate the identical contract for the other systems. They compose through **world memory** (shared state) and the **brain** (orchestration).

### Sensing Suite
- **Perceive/act:** read environmental/IMU/bus sensors (OBC already has `peripherals/fusion`, sensor tools, and the seeded sensor accessories); writes are calibration/threshold changes (approval-gated).
- **Remember:** sensor readings → world memory as time-valid facts ("living_room.temp = 21°C, valid 06:00–06:30"); the bitemporal model invalidates stale facts rather than overwriting.
- **Learn/accelerate:** on-node reflex rules (Phase 18 System 1) that fire without the brain; learned thresholds; sensor-fusion models on edge accelerators.
- **Reference:** sensor-fusion + the reflex engine from `V2-IMPLEMENTATION.md` Phase 18; ClawCam's profile/state pattern for sensor "modes."

### Movement Suite
- **Perceive/act:** actuators — GPIO, relays, motor drivers, servos, pan-tilt. These are **the highest-risk tools in the whole system**, so this suite leans hardest on **Track 0** (deterministic on-MCU safety limits, physical risk class, staged rollout, signed audit).
- **Remember:** actuator state + outcomes into world memory ("front_door.lock = locked, since 06:14"); idempotency keys (Phase 17) so resume never double-actuates.
- **Learn/accelerate:** closed-loop control where **vision drives movement** — e.g., the Vision Suite detects a subject, writes it to world memory, the brain (or an on-node reflex) commands the Movement Suite to pan-tilt and track it. Learned movement skills (a synthesized "track-and-center" routine) gated by Track 0.
- **Reference:** Track 0 firmware `SafetyGate`, the dual-system reflex loop, the idempotency/durable-execution work.

**The suites compose.** Vision → world memory → Movement is the canonical embodied loop, and it's only possible because all three speak the same contract and share one memory and one safety layer. That composition — perception in one suite driving action in another through shared memory under one brain — is the thing no pure-software agent and no single-purpose robotics model can do.

## 5. Phased rollout

| Phase | Deliverable | Suite(s) | Maps to |
|---|---|---|---|
| **S0** | Formalize Subsystem Suite contract; fix ClawCam drift; finish Phase 13↔15 lockstep | Vision | — |
| **S1** | Review-state + re-ID → world memory | Vision | Phase 18 |
| **S2** | Active-learning loop (review → correct → improve) | Vision | Phase 16 |
| **S3** | Real + edge/accelerated inference; dual-system split | Vision | Phase 20/18 |
| **S4** | Sensing Suite to contract (perceive → world memory → reflex) | Sensing | Phase 18 |
| **S5** | Movement Suite to contract (Track 0-gated actuation; vision-driven tracking) | Movement | Track 0 / Phase 18 |

Sequencing: complete vision end-to-end (S1–S3) as the proof of the contract, then stamp out Sensing (S4) and Movement (S5). Movement comes last because it needs Track 0 mature and a working perception suite to close the loop with.

## 6. Honest caveats

- **ClawCam's "complete" phases are plumbing-complete, mock-backed.** The detector chain runs, but real inference needs weights/libs that don't ship. Shipping real vision (V3) is a prerequisite before claiming a learning loop has anything real to learn from.
- **The review-state model is spec'd, not built** — V1 starts there, and everything in V2 depends on it.
- **World memory is OBC-side and doesn't exist yet** (Phase 18). The Vision Suite plan and the Phase 18 build must be co-designed: the suite is world memory's first real producer, so build the schema against vision's needs (entities, re-ID, valid-time).
- **Movement is genuinely dangerous.** Don't ship any vision→movement closed loop until Track 0's deterministic on-MCU limits and signed audit are real. Perception driving actuators is the highest-stakes path in the system.

### Bottom line
ClawCam isn't a project to bolt on — it's the **proof and template** of OBC's embodied architecture: a capability suite the brain already orchestrates. Finish it into a vision system that *remembers* (world memory), *learns* (active-learning feedback), and *accelerates* (real edge inference), then instantiate the same contract for Sensing and Movement. One brain, many suites, one shared memory, one safety layer — perception in one suite driving action in another. That is the embodied frontier, and ClawCam is where it starts.

# Phase 17 — Long-Horizon Embodied Autonomy Harness: Design

*Written 2026-07-02, before implementation. Companion to `docs/V2-STRATEGY.md` and the Phase 17 checklist in `ROADMAP.md`.*

## Substrate audit

What already exists and what the harness adds on top:

| Module | What it does | What it lacks for Phase 17 |
|---|---|---|
| `src/mission` | Reactive step sequencer (navigate/speak/wait/record/await) with guard preemption; `tick()`-driven | In-memory only — no persistence, no resume, no LLM, completion is step-mechanical, not verified |
| `src/scheduler` | Cron-style task scheduling | Triggers work; doesn't track multi-step progress |
| `src/memory/heartbeat.rs` | Markdown task list → agent prompt | Unstructured; no status machine, no verification |
| `src/memory/world.rs` | Bitemporal world memory | The *evidence source* for verification — not a progress record |
| `src/agent` | The full chokepointed reasoning loop | Stateless across process restarts |

The harness (`src/harness`) is the durable orchestration layer above all of these: the **Anthropic initializer+worker pattern with the progress file externalized as structured JSON**, and — the embodied twist — completion decided by *physical evidence* (sensor/tool/world-memory checks), never by the model's own say-so.

## Design

**Progress record** (`~/.oh-ben-claw/harness/<mission>.json`, atomic tmp+rename writes — every state change is a checkpoint):

- `Objective { id, description, verify: Vec<HarnessCheck>, status, attempts, max_attempts, note }`
- `Status: Pending → InFlight → (NeedsVerification on resume) → Done | Failed`
- `ProgressRecord { mission, objectives, environment snapshot, run_count, timestamps }`

**Non-persistable regions / no duplicated physical actions:** an objective is checkpointed `InFlight` *before* the agent acts. If the process dies mid-objective, resume moves `InFlight → NeedsVerification` and runs the objective's checks — evidence decides whether the side effect already happened (→ `Done`, actuator untouched) or not (→ reopened `Pending`). An `InFlight` objective **without** verification checks fails closed on resume (`Failed`, "crashed mid-flight, no verification — manual review") rather than risk double actuation.

**Verification (mandatory before Done):** `HarnessCheck` = `ToolContains { tool, args, contains }` (any read tool through the agent chokepoint — sensors, cameras), `Command { cmd, expect_exit }` (host test), `WorldFact { entity, contains }` (current world-memory fact). Objectives with no checks may complete on agent-run success but are marked unverified in their note — and get the fail-closed resume above.

**Initializer:** creates/loads the record, bumps `run_count`, flips in-flight objectives to `NeedsVerification`, snapshots the environment (world-memory entities) into the record.

**Worker:** advances **one objective per pass** — verify-pending objectives first, then the next `Pending`: mark `InFlight` (checkpoint) → focused `Agent::process` prompt (objective + cheap resume context: outstanding objectives + current device facts) → run checks → `Done`, or `attempts+1` → `Pending`/`Failed`. The loop runs until all objectives settle.

**Config** (`[harness]` + `[[harness.mission]]` + `[[harness.mission.objective]]` + nested `verify` entries); autostart missions spawn on agent start. Records live outside the config so operators can inspect/edit them.

**Long-horizon eval** (`tests/harness_long_horizon.rs`): a scripted provider + counting actuator mock run a 3-objective mission across an induced crash (record rebuilt from disk mid-mission): the actuated objective resumes to `Done` via sensor evidence with the actuator invoked **exactly once**; a check-less in-flight objective fails closed; the mission settles with correct statuses.

Deliberately out of scope (follow-ups): gateway/CLI mission-control surface, LLM-decomposed objectives from a free-form goal (initializer currently takes configured objectives), multi-mission concurrency.

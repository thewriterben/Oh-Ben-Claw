# The Subsystem Suite Contract (v0.1)

*Shared standard for Oh-Ben-Claw. Mirror this file into each suite repo (ClawCam, and future Sensing/Movement suites). Compiled June 23, 2026. See `SUBSYSTEM-SUITES.md` for the rationale and roadmap.*

A **Subsystem Suite** is a self-contained embodied capability module — its own repo, language, and hardware — that the Oh-Ben-Claw brain orchestrates. Vision (ClawCam) is the reference implementation; Sensing and Movement follow the same contract. This document is the normative spec: a suite is "conformant at level N" when it satisfies every MUST through level N.

The contract has **8 points**. A suite advertises which it satisfies in its capability handshake (§2).

---

## 1. Perceive / Act — MCP tool surface

- A suite MUST expose its capabilities as **MCP tools** over a stdio (or HTTP) server.
- Each tool MUST be classed **read** (perception/query, side-effect-free) or **write** (action / world-changing).
- Read tools MAY be auto-approved. Write tools MUST be declared `approval_required`.
- Tool definitions MUST carry `name`, `description`, and a JSON-Schema `input_schema`.

*Reference: ClawCam `gateway/.../mcp_server/stdio_server.py` — 23 read + 9 write.*

## 2. Connect — registration & capability handshake

- A suite MUST emit tool entries in OBC's `McpToolEntry` shape: `{ name, description, input_schema, approval_required, source }`.
- A suite MUST advertise a **capability descriptor**: `{ suite, version, domain (vision|sensing|movement|…), protocol_modes, contract_level, capabilities[] }`.
- A suite SHOULD support OBC's dual MCP modes (`legacy-2024`, `stateless-2026`).
- A suite MAY additionally join the MQTT **spine** for streaming telemetry.

*Reference: ClawCam `brain/oh-ben-claw-adapter/clawcam_adapter.py::as_obc_tool_entries()`.*

## 3. Remember — world-memory contribution

- A suite MUST persist domain observations durably (offline-first; cloud optional/additive).
- A suite MUST contribute observations to OBC's **bitemporal world memory** (Phase 18) as entities with **valid-time** intervals and **non-destructive** correction (a correction creates a new record; the prior is invalidated, not deleted).
- Domain entities SHOULD be stable and re-identifiable across events (e.g. vision: an individual subject/face/plate; sensing: a named sensor stream; movement: an actuator + position).

*World-memory contract: `entity_id`, `value`, `valid_from`, `valid_to?`, `ingested_at`, `source`. Reference target: OBC `src/memory/world.rs`.*

## 4. Learn — feedback & improvement loop

- A suite that makes predictions MUST expose a **review/correction** path: low-confidence or novel outputs are queued (`needs_review`) and resolved by a human or the brain into ground-truth labels.
- Corrections MUST be the **self-verification signal** for any self-improvement (no unverified reflection — see Phase 16). Accumulated corrections SHOULD drive threshold/skill improvement.
- A suite SHOULD record learning metrics (review rate, correction rate, calibration over time).

## 5. Improve — versioned, eval-gated models/skills

- Models/detectors/skills MUST be **versioned**; outputs MUST record the producing `model_name` + `model_version`.
- A new version MUST pass an eval before promotion; promotion SHOULD be staged (shadow → active).

## 6. Accelerate — edge inference tier

- A suite SHOULD provide a **fast local tier** (System 1) that runs offline on-node/edge, gating a slower, fuller tier (System 2) on the host/cloud.
- Heavy inference MUST be loadable as pluggable backends (real models, accelerators: `npu`/`edge_tpu`/`hailo`/`nn_accel`) without changing callers.

*Reference: ClawCam `inference/registry.py` — name→factory, lazy-load, per-device chains.*

## 7. Stay safe — Track 0 for world-changing actions

- Every **write/physical** tool MUST declare a `RiskClass` (`reversible`, `blast_radius`, `physical`).
- Irreversible/high-blast actions MUST default to per-call approval and MUST NOT be auto-grantable to `forever`.
- Physical actuation MUST be bounded by **deterministic, model-independent limits enforced at the lowest level** (on-MCU where applicable — see Track 0 `SafetyGate`), and every action MUST produce a tamper-evident audit record.
- A suite MUST honor OBC's approval scopes (`call`/`session`/`forever`) and plan-mode bounds.

*Reference: OBC `src/security/limits.rs` (host), firmware `SafetyGate`, `src/approval`.*

## 8. Observe & evaluate

- A suite MUST write a **per-tool audit** record per invocation (tool, args hash, latency, source, outcome).
- A suite MUST ship a **golden eval** that behaviorally verifies the read/write/approval partition and core domain determinism, runnable as an OBC CI release gate.
- Telemetry MUST degrade to **no-op** when no collector/broker is reachable (edge/air-gapped).

*Reference: ClawCam `tool_call_audit` + `tests/evals/`.*

---

## Conformance levels

| Level | Name | Requires |
|---|---|---|
| **L0** | Connected | §1 Perceive/Act + §2 Connect + §8 audit |
| **L1** | Safe | L0 + §7 Stay-safe (risk class, approval scopes, deterministic limits on physical actions) + §8 eval gate |
| **L2** | Remembering | L1 + §3 world-memory contribution |
| **L3** | Self-improving | L2 + §4 Learn + §5 Improve + §6 Accelerate |

**Status:** ClawCam (Vision) is at **L1** today (connected, safe, eval-gated) with §5 partially present; the integration plan drives it L1 → **L3** (remember → learn → accelerate). New suites target L1 first, then climb.

## Conformance checklist (per suite)

- [ ] MCP server exposes read/write tools with schemas; writes flagged `approval_required` (§1)
- [ ] Emits `McpToolEntry` + capability descriptor with `contract_level` (§2)
- [ ] Durable offline-first store; contributes valid-time, non-destructive records to world memory (§3)
- [ ] Review/correction path feeds a verified improvement loop (§4)
- [ ] Versioned models/skills with eval-gated, staged promotion (§5)
- [ ] Fast offline tier + pluggable accelerator backends (§6)
- [ ] `RiskClass` on every write tool; deterministic on-MCU limits; tamper-evident audit; honors approval scopes (§7)
- [ ] Per-tool audit + golden eval as CI gate; no-op telemetry fallback (§8)

> Versioning: this contract is **v0.1**. Breaking changes bump the minor; suites declare the contract version they target in their capability descriptor so the brain can refuse an incompatible suite loudly.

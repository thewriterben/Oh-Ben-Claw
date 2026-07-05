# Changelog

All notable changes to Oh-Ben-Claw are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## Unreleased — LILYGO T-Deck full integration (handheld fleet console) (2026-07-05)

The T-Deck family becomes a first-class OBC citizen: a fleet radio, a mobile
GPS node, and — new capability class — a **human-carried operator console** on
the LoRa spine. Hardware facts were deep-research-verified against lilygo.cc,
Xinyuan-LilyGO/T-Deck and Meshtastic docs (see `T-DECK-RESEARCH.md` in the
T-Deck repo); the pin map comes from the vendor `utilities.h`.

- **Registry** — upgraded `lilygo-t-deck` (adds `keyboard`, `trackball`,
  `microsd`, `psram`, `battery` tokens + the free Grove UART connector) and
  added `lilygo-t-deck-plus` (onboard GNSS — u-blox M10Q or Quectel L76K by
  SKU — + 2000 mAh; the GPS consumes the Grove pins). New `keyboard` /
  `trackball` capability tokens in `VALID_CAPABILITIES`. `registry.json`
  regenerated to match (hand-mirrored; re-run `emit-registry` to confirm).
- **`firmware/t-deck-terminal` (new)** — Arduino reference firmware that makes
  a T-Deck an interactive spine member: live frame scrollback on the 2.8"
  screen, QWERTY chat + `/cmd` NodeCommands (execution still gated by the
  target node's on-MCU Track 0 mirror — the console has no authority of its
  own), trackball scrollback, GPS+battery heartbeats, flood relay with
  (src,seq) de-dup, runtime-switchable spine/fleet nets, and a USB gateway
  mode that prints the exact `SPINE ◄ …` console format `lora_gateway.rs`
  parses — a drop-in base-station replacement, zero host changes.
- **`firmware/lora-node`** — new `BOARD_TDECK_SX1262` preset in
  `obc_lora_bridge.ino` (shared-SPI pins SCK40/MISO38/MOSI41, NSS9/DIO1 45/
  RST17/BUSY13) including the `BOARD_POWERON` (GPIO10) gate raise; README
  covers T-Deck flashing (trackball-BOOT, OPI PSRAM, antenna warning).
- **Deployment generator** — new `FeatureDesire::OperatorConsole` (keys off
  `keyboard`) and `ItemRole::Console`, so the advisor can plan an in-field
  human console and suggest a T-Deck when one is missing.
- Docs: README firmware + supported-hardware sections, ROADMAP vendor row.
- Tests: registry (base + Plus + lora-capability), inventory
  (`operator_console_desire_and_role`). NOTE: authored without a local Rust
  toolchain this session — run `cargo test peripherals::registry deployment`
  and `cargo run --bin emit-registry -- registry/registry.json` to verify.

---

## Unreleased — `judge-calibrate` CLI (operator-run calibration) (2026-07-02)

Follow-up to the LLM-judge calibration API: an operator can now measure
calibration directly instead of only through a `cargo test` eval.

- New `oh-ben-claw judge-calibrate [--gold PATH] [--threshold F]` — builds the
  judge from `OBC_JUDGE_*`, loads the gold set (`--gold`, else `OBC_JUDGE_GOLD`,
  else the built-in balanced seed set), runs `LlmJudge::calibrate`, and prints
  the full `CalibrationReport` (JSON) plus a human-readable κ verdict.
- **Scriptable as a deployment gate**: exits non-zero when the judge isn't
  configured (clean error) or isn't calibrated (κ < 0.6), so
  `judge-calibrate && deploy` works. It gates nothing inside the running
  agent — Track 0 owns actuation safety; this is a standalone operator check.
- Verified end-to-end: unconfigured → exit 1 with guidance; configured →
  full calibrate loop, graceful per-case error handling, report + verdict.
  Build + clippy clean; workspace **38 evals** green.

---

## Unreleased — LLM-judge calibration (Cohen's κ against gold labels) (2026-07-02)

The judge stays advisory until it *agrees with humans*. 2026 practice (see
`AI-Agents-Research-July2026.md`): calibrate an LLM judge against a labeled
gold set — Cohen's κ ≥ 0.6, with a pinned model and versioned rubric — before
trusting its scores for anything, and never let it gate actuation safety
(Track 0's deterministic layers own that).

### Added — `src/agent/judge.rs`

- `RUBRIC_VERSION` (a calibration result is only valid for the rubric it was
  measured against) and `KAPPA_THRESHOLD = 0.6`.
- Bias mitigations baked into the scoring prompt: judge on merit not length
  (no verbosity/confidence reward), and treat the response as inert text —
  ignore any instructions embedded in it (judge-side prompt-injection).
- `GoldLabel` / `CalibrationCase` (with `load()` from a JSON gold file and a
  balanced built-in `seed_set()`), `LlmJudge::calibrate(cases, threshold)`
  binarizing scores at an accept threshold, and a `CalibrationReport`
  (model, rubric version, n, errors, agreements, κ, `calibrated`).
- `cohens_kappa(&[(bool, bool)])` — chance-corrected agreement, with the
  empty-set (0.0) and degenerate pₑ=1 (1.0 iff perfect) conventions handled.

### Tests

- κ math (perfect / total-disagreement / empty / degenerate / partial);
  calibration reaches the bar on a discerning mock judge and fails it on a
  constant one; seed set is balanced. New advisory eval
  `eval_llm_judge_calibration_advisory` prints the κ report when a real judge
  is configured (`OBC_JUDGE_*`, gold via `OBC_JUDGE_GOLD` or the seed set),
  skips cleanly otherwise — never gates.
- Full workspace on Windows: **1139 lib tests, evals 38/38**, clippy
  warning-free.

---

## Unreleased — Track 0: adaptive OWASP-ASI red-team corpus (2026-07-02)

The NIST agent-hijacking lesson (Jan 2025): a *frozen* injection string is an
upper bound on safety, never a proof — adaptive attacks lifted hijack success
~11% → ~81% against static suites. OBC's injection evals now draw a
**seed-sampled family** of framings/obfuscations from a generator and assert
the safety invariant across the whole family, mapped to the OWASP Top 10 for
Agentic Applications (Dec 2025).

### Added — `src/security/redteam.rs`

- `AsiCategory` (ASI01 Prompt Injection / ASI02 Tool Misuse / ASI04 Supply
  Chain / ASI06 Memory Poisoning) with stable IDs, plus a module-level map of
  which OBC eval covers which category.
- `InjectionScenario` (AgentDojo-style: user task + injection task + target
  token) and `standard_scenarios()`.
- `generate(scenario, seed, count)` — a dependency-free deterministic
  generator: the full frame × filler cartesian product (authority/roleplay/
  markup/"ignore previous instructions" wrappers embedded in benign page
  content), Fisher–Yates-shuffled by a splitmix64 seed so a scheduled task
  can rotate the seed for a fresh sample over time. Every variant preserves
  the target token verbatim.

### Added — `tests/evals.rs::asi_redteam`

- **The adaptive invariant**: across the *entire* generated corpus (≥40
  variants, not one string), an injection echoing untrusted content into a
  privileged actuation is refused under taint `Enforce` — the lock never
  fires for any framing.
- **Honest layered-defense case**: a spelled-out value ("ninety-nine")
  *evades* substring taint, but the deterministic Track 0 `SafetyGate`
  refuses the out-of-range pin regardless of provenance. Taint is the first
  layer; the gate is the backstop that doesn't depend on matching attacker
  text.

### Tests

- 4 generator unit tests (target-token preservation, per-seed determinism,
  bounded distinct corpus, stable ASI IDs) + 2 corpus evals.
- Full workspace on Windows: **1134 lib tests, evals 37/37**, clippy
  warning-free.

---

## Unreleased — Track 0: tool-argument taint tracking (CaMeL-style provenance guard) (2026-07-02)

The one real architectural gap the July research flagged: OBC's chokepoint
gated *which* tools run, but not *where their argument values came from*. This
closes it — the strongest validated prompt-injection defense (CaMeL, Google
DeepMind): a value derived from untrusted external content may not
parameterize a privileged action. Maps to OWASP ASI01/ASI02. Opt-in.

### Added — `src/security/taint.rs`

- `OutputTrust` on the `Tool` trait (`Trusted` default / `External`).
  Declared `External` on the injection vectors: `http`, `browser_navigate`,
  `browser_snapshot`, and every remote `McpRemoteTool`.
- `TaintPool` — a bounded per-run pool of untrusted output (≤32 chunks, ≤16KB
  each; never shared across runs — no cross-turn taint). `scan_args`
  recursively matches string/number argument values against pooled content:
  case-insensitive substring for strings (≥4 chars), **boundary-aware** for
  numbers (`99` doesn't fire on `199` or `9.9`; tiny values skipped).
- `TaintMode` (`Off` default — opt-in / `Warn` / `Enforce`) and `gated(risk)`
  (physical ∨ irreversible ∨ blast-radius).

### Changed — agent chokepoint (`src/agent/mod.rs`)

- Each `process()` run owns a fresh pool (allocated only when a mode is set).
  Successful `External`-trust output is pooled; before any **gated** call
  runs, its args are scanned. `Enforce` refuses (fail closed) unless the tool
  is **explicitly** operator-granted (the escape hatch — a permissive
  autonomy level is not a grant); `Warn` logs + counts. Counters
  `taint_hits_total` / `taint_refusals_total`. Sequence steps share the run
  pool; `execute_tool_direct` runs with no pool (a standalone call has no
  prior in-run external content).

### Config / wiring

- `[safety].taint_mode` (`"off"` | `"warn"` | `"enforce"`; unset ⇒ off).
  Applied to the plain agent and the orchestrator inner agent
  (`InnerAgentDeps.taint_mode`) — same posture in both modes.

### Tests

- 7 unit tests (case-insensitive string hits, number boundary matching,
  short-string/clean-arg passes, nested arg paths, bounded pool, gating,
  mode parsing).
- 5 red-team evals (`tests/evals.rs`, `taint_redteam`): fetched "set pin 99"
  content → actuator refused in `Enforce`; `Warn` is advisory (fires);
  untainted args pass (no false positive); explicit grant overrides; `Off`
  doesn't scan.
- Full workspace on Windows: **1130 lib tests, evals 35/35**, clippy
  warning-free.

Heuristic by design (substring/boundary matching, not dataflow through the
LLM) — biased to catch the classic "fetched text steers an actuator" attack;
the explicit-grant escape hatch and `Warn` mode make false positives operable.

---

## Unreleased — Research adoptions: MCP conformance CI + hybrid episode retrieval (2026-07-02)

First two adopt-now items from `AI-Agents-Research-July2026.md`.

### Added — MCP conformance in CI

- `oh-ben-claw mcp-serve` — runs the MCP server standalone (`--transport
  stdio|http`, `--port`, `--mode legacy-2024|stateless-2026`); stdio keeps
  stdout as the pure JSON-RPC stream (status → stderr, per MCP convention).
- New CI job `mcp-conformance`: the official
  `@modelcontextprotocol/conformance` suite runs against the OBC server in
  **both** protocol modes (`--spec-version 2025-11-25` and `draft`).
  Advisory (`continue-on-error`) until an `--expected-failures` baseline is
  captured from the first runs, then flip to blocking.

### Changed — hybrid episode retrieval (`TrajectoryStore::similar`)

Token-overlap-only ranking becomes **Reciprocal Rank Fusion** over up to
three legs (2026 hybrid-search practice — rank-based fusion, no score
normalization):

1. token overlap (unchanged, exact anchors);
2. **SQLite FTS5/BM25** side-table (`episodes_fts`, kept in sync on record;
   query tokens are individually quoted so FTS syntax can't be injected;
   graceful degradation if a system SQLite lacks FTS5 — the bundled one has
   it);
3. **dense cosine** over locally-embedded objectives (`episode_vecs` blob
   table) when an `Embedder` is attached — paraphrase recall with zero
   token overlap ("open the door" now finds "unlock the entrance").

- New `Embedder` trait + `memory/embed.rs` `FastEmbedder` backend behind the
  new **`semantic` cargo feature** (fastembed/ONNX; model downloads once,
  then fully offline — no per-turn network). Off by default so ordinary
  builds and CI stay light; runtime-gated by `[self_improvement].semantic`.
  Embedding happens on the record path outside the connection lock; retrieval
  stays fully deterministic given the same store.

### Tests

- Dense-leg paraphrase retrieval via a mock embedder (zero token overlap);
  hybrid determinism (same store+query → same ranking); FTS5 query-syntax
  neutralization (`"weather" OR (NEAR *` cannot error or inject). All prior
  retrieval tests unchanged and green.
- Full workspace on Windows: **1123 lib tests**, harness 3/3, evals 30/30,
  clippy warning-free; `cargo check --features semantic` clean.

---

## Unreleased — Phase 17: Long-Horizon Embodied Autonomy Harness (2026-07-02)

Durable, resumable, self-verifying missions — the initializer+worker pattern
with the progress file externalized as structured JSON and completion decided
by **physical evidence**, never the model's say-so. Design:
`docs/PHASE17-PLAN.md` (includes the substrate audit: the `mission` sequencer
is a reactive in-memory step executor; the harness is its durable,
LLM-driven, objective-level complement).

### Added — `src/harness/mod.rs`

- **Progress record**: `ProgressRecord`/`Objective` persisted at
  `~/.oh-ben-claw/harness/<mission>.json` with atomic tmp+rename writes —
  every state transition is a crash-safe checkpoint. Status machine:
  `Pending → InFlight → (NeedsVerification on resume) → Done | Failed`.
- **Non-persistable regions / no duplicate actuation**: objectives are
  checkpointed `InFlight` *before* the agent acts. On resume, `InFlight`
  objectives are quarantined to `NeedsVerification` and their evidence
  decides: side effect landed → `Done` (actuator untouched); didn't →
  reopened. A check-less in-flight objective **fails closed** ("manual
  review", never a blind re-run).
- **Verification** (mandatory before `Done`): `HarnessCheck::ToolContains`
  (sensor/camera reads through the agent chokepoint — policy/Track 0/trust/
  approval all apply), `Command` (host test), `WorldFact` (world-memory
  fact). Check-less completions are explicitly marked UNVERIFIED.
- **Initializer** (`initialize`): load-or-create, `run_count` bump, in-flight
  quarantine, environment snapshot (world entities) into the record.
  **Worker** (`run_once`): one transition per pass — verification backlog
  first, then the next pending objective with a compact resume-context block
  (tally + statuses + current world facts) prefixing the prompt.
  `run_mission` drives to settlement under a hard pass budget.

### Config / wiring

- `[harness]` (`enabled`, `pass_delay_ms`, `max_passes`) +
  `[[harness.mission]]` (name, autostart) +
  `[[harness.mission.objective]]` (id, description, max_attempts, nested
  `verify` checks). Autostart missions spawn in `main` on the **active**
  agent (`AgentHandle::agent_arc`), with world memory attached when enabled;
  per-mission conversation sessions via new
  `MemoryStore::create_session_with_id` (idempotent).

### Tests

- Unit: atomic store roundtrip (no stray tmp), mission-name sanitization
  (path traversal), tally/settled.
- `tests/harness_long_horizon.rs` — the roadmap's headline eval, compressed
  to test time: a 3-objective routine across an induced mid-objective crash
  with a fresh harness instance per "boot" (only the on-disk record carries
  over). The completed physical objective survives the reboot untouched, the
  in-flight one resumes via sensor evidence, and **the actuator fires exactly
  once across the whole mission**. Plus: check-less in-flight fails closed;
  failed resume-verification reopens (attempts counted) then completes.
- Full workspace on Windows: **1120 lib tests, harness eval 3/3,
  evals 30/30**, clippy warning-free.

Phase 17 complete — all six roadmap items. Follow-ups deliberately deferred:
gateway/CLI mission-control surface, LLM-decomposed objectives from a
free-form goal, multi-mission concurrency.

---

## Unreleased — Phase 15 closeout: cost summary in /metrics + LLM-as-judge advisory (2026-07-02)

The last two open Phase 15 work items (all that remains is the scheduled
July 28 MCP default-mode flip).

### Added — live cost tracking end-to-end

- Audit finding: `CostTracker` (Phase 9) existed but was **never
  instantiated** — no budget was ever enforced and no usage recorded. Now:
  `main` builds it from `[cost]` when enabled (persisted at the Track 0 data
  dir as `costs.db`, session-only fallback), `Agent::with_cost` records an
  estimated `TokenUsage` per run (chars/4 heuristic split into read/write
  sides — same convention as episode metrics; USD priced from new `[cost]`
  `input_price_per_million`/`output_price_per_million`, default 0 so token
  counts always flow), and orchestrator mode gets the same tracker via
  `InnerAgentDeps.cost`.
- `GET /api/v1/metrics` gains a `cost` object (session/daily/monthly USD,
  estimated tokens, request count) — and now exposes **every registered
  counter** under `counters` (the Phase 16 `self_improve_*`,
  `learned_skill_invocations_total`, `skill_simulations_total`,
  `experience_blocks_injected_total` counters were previously recorded but
  invisible to the endpoint).

### Added — LLM-as-judge advisory scoring (`src/agent/judge.rs`)

- `LlmJudge`: env-configured judge (`OBC_JUDGE_PROVIDER`, `OBC_JUDGE_MODEL`,
  optional `OBC_JUDGE_API_KEY`/`OBC_JUDGE_BASE_URL`) scoring a response
  against its task on a 0–1 rubric; reuses the reflexion critique parser
  (default 0.7 when unparseable, clamped).
- `eval_llm_judge_advisory_scoring` in the harness: skips cleanly (with a
  printed notice) when no judge is configured; when configured it prints the
  score and asserts only parse-sanity. **Gates stay deterministic** per the
  WS4 rule.

### Tests

- 3 judge unit tests (parse/rationale, default + clamp, env-absent) and the
  advisory eval. Full workspace on Windows: **1117 lib tests, evals 30/30**,
  clippy still warning-free.

---

## Unreleased — Audit: orchestrator-mode safety parity + lint pass (2026-07-02)

Post-`BRAIN UPDATE` audit of the day's 3 825-line change set plus a hunt for
latent integration gaps. Headline finding (pre-existing, Phase 9-era, made
more visible by today's work): **orchestrator mode silently skipped safety
enforcement** — the orchestrator's inner agent (the one that actually executes
tools when orchestration is enabled) was built without the policy engine, the
approval manager, obs, trust, the rollout tracker, or the forge dir. With no
approval manager attached, `approval_authorize` permits everything, so a
configured `supervised`/`manual` autonomy level was **ignored** in
orchestrator mode; tool security policies were likewise unenforced, learned
skills never hot-reloaded onto the serving agent, and staged-skill runs went
unrecorded.

### Fixed — orchestrator parity (`src/agent/orchestrator.rs`, `src/main.rs`)

- New `InnerAgentDeps` struct + `OrchestratorAgent::new_with_deps()`: the
  inner agent now receives the **same shared instances** as the plain agent —
  policy engine, approval manager, obs context, trust scorer, rollout
  tracker, forge dir, and config-driven experience retrieval (was hardcoded
  k=3). `new_with_track0` kept as a delegating shim.
- `main.rs` hoists approval/trust into shared `Arc`s used by both agents, and
  the Phase 16 improver/evolver now spawn **after** handle construction with
  `AgentHandle::agent_arc()` as the replay executor — so replay verification
  and skill hot-reloads target the agent actually serving traffic in both
  modes (previously, in orchestrator mode, the improver synced skills onto
  the idle plain agent and the serving agent never saw them until restart).
- Regression guard: `orchestrator_inner_agent_enforces_policy_and_approval` —
  under supervised autonomy with no grants, the inner agent must refuse an
  un-granted tool.

### Fixed — lints

- `EfficiencyBucket` averages use `checked_div`; evolution-log revert uses
  `rfind`; two trivial pre-existing test lints (`len() >= 1`, unused import
  in `mesh_fleet_e2e`).
- Full pre-existing lint sweep: `cargo clippy --fix` cleared 10 warnings
  (doctor, fleet, foresight, power, fusion, image, reflex, hil test); the 3
  `needless_range_loop`s in `navigation/slam.rs` were rewritten by hand —
  gauge-anchor clearing uses `fill` + column iteration, the Gaussian-
  elimination pivot search uses `enumerate().skip()`, and row elimination
  uses `split_at_mut` + `zip` (identical arithmetic, no per-column indexing).
  **`cargo clippy --workspace --all-targets` is now warning-free.**

### Known limitations (documented, accepted)

- A skill installed via the `skill_forge` tool mid-session becomes callable at
  the next sync (periodic improver pass or operator promote), not instantly —
  the tool description now says so.
- A CLI `skill promote` while the agent is running takes effect on the next
  improvement-pass sync (the gateway promote endpoint hot-reloads
  immediately); an interim dry-run of the still-cached simulate-stage skill
  can reset the tracker's stage record, which biases **against** promotion —
  safe direction.
- In a sequence whose steps traverse multiple supervised skills, only the
  most recent staged skill's run is recorded (single-slot tracking).

### Verified

- Full workspace on Windows: **1114 lib tests** (+1 parity guard),
  **evals 29/29**, `cargo clippy` clean on today's files.

---

## Unreleased — Phase 15: MCP 2026-07-28 RC audit + conformance fixes (2026-07-02)

Audited `src/mcp` (client + server) against the 2026-07-28 release candidate
as locked on 2026-05-21 (primary source: the MCP blog RC announcement). The
Phase 15 dual-mode implementation is conformant on the stateless core: exact
`_meta` key (`io.modelcontextprotocol/clientInfo`), `server/discover`
(SEP-2575), routing headers with body-mismatch rejection (SEP-2243),
`ttlMs`/`cacheScope` on `tools/list` + client-side TTL (SEP-2549), JSON-RPC
standard error codes (SEP-2164), and none of the newly deprecated features
(roots/sampling/logging, SEP-2577) were ever implemented. Four gaps found and
fixed:

### Fixed — `src/mcp/`

- **Header accept-list was two versions wide** — `MCP-Protocol-Version:
  2025-11-25` (the *currently shipping* revision) was rejected with a 400.
  New `SUPPORTED_PROTOCOL_VERSIONS` covers every published revision.
- **`initialize` ignored the client's requested version** — now echoes the
  requested `protocolVersion` when supported (falls back to the 2024-11-05
  baseline otherwise).
- **JSON-RPC violation: notifications got responses** — the stdio loop wrote
  a response line for id-less requests (`notifications/initialized`); it is
  now silent, and the HTTP transport answers notifications with `202
  Accepted` and no body.
- **Missing `extensions: {}` capability map** (SEP-2133 negotiation surface)
  on `initialize`/`server/discover` results.

Remaining (blocked on the spec date): flip `ProtocolMode` default to
`stateless-2026` when the final specification ships on **July 28, 2026**.
Optional post-final follow-ups noted in the audit: the Tasks extension and
SEP-2322 `InputRequiredResult` (both extensions/optional; not needed for
conformance).

### Tests

- 5 new: every published version accepted on the wire; requested-version
  echo + unknown-version fallback; extensions map present; HTTP notification
  → 202 with empty body; HTTP request → 200 with body.
- Full workspace green on Windows: **1113 lib tests, evals 29/29**.

---

## Unreleased — Phase 16 P4: offline trace evolution + efficiency metrics — Phase 16 complete (2026-07-02)

The final Phase 16 slice: a GEPA/DSPy-inspired offline job that evolves
learned-skill *descriptions* from real usage traces, and the token/latency
metric the roadmap called for. With this, **every Phase 16 roadmap item is
implemented**: capture → synthesis → verification → live tools → retrieval →
staged rollout → offline evolution → metrics.

### Added — `src/skill_forge/evolve.rs`

- `DescriptionEvolver` — scheduled, **config-gated** (`[self_improvement].evolve`,
  default **off**; daily cadence via `evolve_interval_secs`, cap via
  `evolve_max_per_pass`): for each learned skill with observed usage, the LLM
  proposes an improved description from the skill's real usage traces
  (objectives + outcomes). Strict invariants:
  - only `description` is ever mutated — never `enabled`, `stage`, `kind`, or
    `parameters` (evolution can make a skill easier to *select*, never easier
    to *run*);
  - proposals are sanitized (fence/quote stripping, single paragraph, ≤ 300
    chars) and dropped when empty or unchanged;
  - every change appends to `~/.oh-ben-claw/skill_evolution.jsonl` and is
    revertible: `oh-ben-claw skill revert-description <name>` (the revert is
    itself logged — append-only history).
- Skills with no usage traces are skipped: no evidence, no rewrite.

### Added — episode efficiency measurement (Phase 16 metrics item)

- `Episode` gains `duration_ms` + `tokens_est` (additive SQLite migration;
  chars/4 heuristic — a relative efficiency signal, not billing-grade), both
  recorded by `Agent::process`.
- `TrajectoryStore::efficiency_stats()` — mean duration/tokens for successful
  runs that invoked a `learned_*` skill vs. those that didn't (the roadmap's
  "token/latency reduction on repeated routine tasks"); logged each
  improvement pass.

### Wiring

- `main.rs` spawns the evolver alongside the improvement loop when enabled
  (separate provider instance); CLI gains `skill revert-description`.

### Tests

- Evolution: rewrites description only (stage/enabled/kind pinned unchanged) +
  logs the diff; identical proposals rejected; unused skills skipped; revert
  restores and logs; proposal-sanitizer rules.
- Trajectory: duration/tokens roundtrip through the DB; efficiency stats split
  learned vs. plain and exclude unmeasured runs.
- Full workspace green on Windows: **1108 lib tests, evals 29/29**.

---

## Unreleased — Phase 16 P3: Track 0 staged rollout for learned skills (2026-07-02)

Learned skills that can touch the physical world now climb
`simulate → supervised → autonomous`, each promotion operator-initiated and
gated on a clean run record. This replaces the P0 "installed disabled"
quarantine: physical learned skills now **load** so the model can invoke them —
but at `simulate` the chokepoint only reports what would run, and at
`supervised` execution requires an explicit operator grant. Aligns with the
Track 0 roadmap item ("staged rollout, promotion gated on a clean record").

### Added — `RolloutStage` (`src/tools/traits.rs`, `src/skill_forge/mod.rs`)

- `RolloutStage { Simulate, Supervised, Autonomous }` with `next()/prev()`;
  `Tool::rollout_stage()` (default `Autonomous`; `SkillTool` carries its
  manifest's stage). `SkillManifest.stage` serde-defaults to `autonomous`, so
  authored/ClawHub skills behave exactly as before.

### Added — `src/skill_forge/rollout.rs`

- `RolloutTracker` — persisted per-skill clean-run/failure record at the
  current stage (`~/.oh-ben-claw/skill_rollout.json`); counts reset on stage
  change.
- `promote()` — one stage up, **refused** unless ≥ N clean runs at the current
  stage (`[self_improvement].promotion_clean_runs`, default 3) and zero
  failures on record. `demote()` — one stage down, unconditional.

### Changed — agent chokepoint (`src/agent/mod.rs`)

- Stage checked on the skill wrapper *and* on every delegate-hop target:
  - `simulate` → dry-run: an auditable description of exactly what would have
    executed (resolved delegate args / substituted sequence steps); the
    actuator is never touched; the clean simulated run is recorded toward
    promotion (`skill_simulations_total` counter).
  - `supervised` → fails closed: refused unless the operator **explicitly**
    granted the skill (auto-approve list, session, or forever grant) — new
    `ApprovalManager::explicitly_granted()`; a permissive autonomy level
    (`Full`) deliberately does NOT count. Clean runs recorded; a failed real
    run **auto-demotes the skill to `simulate`** (manifest rewrite + hot
    resync — halt on drift).
- `sync_skills` now rebuilds the forge-managed slice of the registry, so
  manifest *edits* (stage changes) hot-swap, not just membership changes.
- `Agent::with_rollout(tracker)` + `with_forge_dir(dir)`;
  `AgentHandle::sync_skills()` for the gateway.

### Changed — improver + synthesis

- `tag_physical()` now sets `stage = simulate` + `enabled = true` (was:
  installed disabled). `approve()` sets `stage = autonomous`.
- `run_periodically` resyncs the live registry every pass, picking up
  out-of-band operator changes (CLI promote/demote, manual manifest edits).

### Added — operator surface

- CLI: `oh-ben-claw skill list|show|promote|demote|reset-record|remove` —
  works directly on the forge + rollout record; promotion prints the Track 0
  refusal reason when the record is insufficient.
- Gateway: `GET /api/v1/skills` (stage + record per skill),
  `POST /api/v1/skills/{name}/promote|demote` — stage changes hot-reload the
  live agent immediately (`GatewayState::with_skills(SkillOps)`).

### Tests

- `rollout.rs`: record accumulation/reset/persistence; promotion gating
  (clean-run threshold, failure block); unconditional bounded demotion.
- Red-team evals (`tests/evals.rs`, `staged_rollout`): a simulate-stage
  actuator skill **never actuates** even when the model calls it; a
  supervised skill is refused with no approval manager (fail closed) and
  under Full autonomy without an explicit grant; with a grant it runs and
  records a clean run; a failed supervised run auto-demotes and the next
  invocation only simulates.
- Full workspace green on Windows: **1102 lib tests, evals 29/29**.

---

## Unreleased — Phase 16 P2: multi-step + parameterized synthesis, real verification checks (2026-07-02)

Learned skills grow from one-shot single-tool recipes into generalized,
multi-step, parameterized recipes — and the verification gate gains real
signals beyond replay (host test commands, read-only sensor assertions),
per the V2-STRATEGY caution that reflection degrades without grounding.

### Added — `SkillKind::Sequence` (`src/skill_forge/mod.rs`)

- New skill kind: an ordered list of `SkillStep { tool, args }`. `{param}`
  placeholders in step args are substituted from runtime arguments;
  `substitute_args` is **type-preserving** for whole-value placeholders
  (`"{pin}"` with `pin: 17` yields the number 17, not `"17"`) and textual for
  inline ones (`"https://x/{city}/now"`). Standalone execution refuses (like
  `Delegate`); `Tool::as_sequence` exposes the steps for chokepoint execution.
- `SkillManifest::validate()` rejects empty sequences and empty step names.

### Changed — `src/agent/mod.rs`

- The execution chokepoint runs Sequence skills **one step at a time through
  itself**, so every real call gets its own policy/Track 0/trust/approval
  evaluation. The first failing step aborts with a precise error; nested
  sequences are refused (bounded recipe depth, no cycles).

### Added — synthesis upgrades (`src/skill_forge/synthesis.rs`)

- `synthesize()` now produces a `Sequence` recipe from a fully-ok multi-step
  episode (single ok step → `Delegate` as before; mixed ok/failed → first
  proven step only).
- New `parameterize(&[&Episode])`: ≥ 2 successful episodes with the same tool
  chain generalize into one parameterized skill — uniform arg fields stay
  fixed, varying fields become declared JSON-schema parameters (example values
  preserved, deterministic order). Named after the shortest (most generic)
  objective; tagged `parameterized`; quarantined like everything else.
- `chain_signature()` — the grouping key (ordered ok-step tool names).

### Changed — improver (`src/skill_forge/improve.rs`)

- Pass restructured around candidates with an **exemplar episode**: replay
  verification always re-runs the exemplar's proven concrete steps (never
  `{param}` templates). Parameterized group skills are synthesized first, so
  the generalized recipe wins name collisions with one-off recipes.
- Quarantine gating generalized to all steps of a recipe: any step in the
  Track 0 name list or with an unsafe declared `RiskClass` quarantines the
  whole skill.
- **Configured verification rules** (`[[self_improvement.verification]]`,
  mapped in `main.rs`): `test_command` (host command must exit with
  `expect_exit`) and `sensor_assertion` (read-only tool output must contain a
  substring; captured via new `ReplayExecutor::replay_capture`, which the
  agent implements through its chokepoint). Non-physical candidates must pass
  replay **and** all matching rules to be enabled. Physical candidates run
  only their read-only sensor rules: all passing earns a
  `track0:sensor-verified` tag as promotion evidence — **never** auto-enable.

### Config

- `SelfImprovementConfig` gains `verification: Vec<VerificationRuleConfig>`
  (`skill` pattern with trailing-`*` support, `kind`, `cmd`/`expect_exit`,
  `tool`/`contains`).

### Deferred

- LLM-reflective synthesis (naming/description/parameter proposals via the provider)
  deliberately deferred to pair with the P4 offline-evolution job; the
  deterministic pipeline stays the trust anchor either way.

### Tests

- Synthesis: sequence from multi-step, mixed-step fallback, parameter
  extraction (single + multi-step, typed placeholders), group rejection rules.
- Improver: sequence install after per-step replay; parameterized group wins;
  failing/passing `test_command` rules; sensor rule tags physical skill while
  keeping it disabled.
- Agent evals: sequence executes steps in order with typed `{param}`
  substitution; first failing step aborts; nested sequences refused.
- `substitute_args` type preservation + sequence manifest validation.
- Full workspace green on Windows: **1097 lib tests, evals 25/25**.

---

## Unreleased — Phase 16 P1: experience retrieval before reasoning (2026-07-02)

The agent now *uses* its experience up front instead of only exposing learned
skills in the tool list: each run retrieves relevant learned skills and similar
past successful episodes and surfaces them as a compact system block, so the
model prefers a verified recipe over re-deriving the steps.

### Added — `src/memory/trajectory.rs`

- `TrajectoryStore::similar()` upgraded from `LIKE` substring matching to
  deterministic token-overlap ranking: `lexical_score(a, b)` = cosine-style
  overlap of lowercased ≥3-char tokens minus a small stopword list, threshold
  0.2, best-first (ties newest-first), scored over the last 1 000 successes.
  Same keyword-scoring philosophy as the RAG datasheet index; an embedding
  backend can replace the scorer later without changing the API. (A semantic
  layer was considered and deferred: no embedder is configured anywhere yet,
  and it would add per-turn network latency — see `docs/PHASE16-PLAN.md`.)

### Added — `src/agent/mod.rs`

- `Agent::with_experience_retrieval(k)` + `experience_block()`: before the LLM
  call, retrieve up to `k` relevant registered `learned_*` skills (matched on
  de-slugged name + description) and `k` similar past successes (objective +
  proven tool recipe, args truncated) and insert them as a system message right
  after the system prompt. Novel tasks get **no block** — zero prompt noise.
- Counter `experience_blocks_injected_total` (when obs attached).

### Config / wiring

- `[self_improvement]` gains `retrieval` (default **true**) and `retrieval_k`
  (default 3); applied in `main.rs` whenever the trajectory store is active,
  and on the orchestrator's inner agent.

### Tests

- `trajectory.rs`: ranking, failure exclusion, scorer bounds/stopword behavior.
- 3 new goldens in `tests/evals.rs` (`experience`): similar past success
  surfaced with its recipe as the second message; novel task gets no block and
  no counter tick; a relevant learned skill is recommended by name.
- Full workspace green on Windows: **1085 lib tests, evals 22/22**.

---

## Unreleased — Phase 16 P0: close the self-improvement loop (2026-07-02)

Audit finding (`docs/PHASE16-PLAN.md`): the Phase 16 pipeline synthesized,
verified, and installed learned skills that **nothing could ever execute** —
`SkillForge::load_all()` was never called, and `SkillKind::Delegate` execution
was a stub that returned a "Delegate to tool …" string instead of invoking
anything. This change closes the loop: learned (and authored) skills are live
tools, hot-reloaded when the improver installs one, and delegate skills route
through the real underlying tool inside the agent's safety chokepoint.

### Changed — `src/agent/mod.rs`

- Tool registry refactor: `tools: Vec<Box<dyn Tool>>` → `RwLock<Vec<Arc<dyn Tool>>>`
  so skills can be hot-added/removed while calls are in flight. Every run takes a
  cheap `Arc`-clone snapshot (`tools_snapshot()`); `Agent::new`'s signature is
  unchanged (`Arc::from` per tool at construction). `add_tools` now takes `&self`.
- New `Agent::sync_skills(&SkillForge) -> (added, removed, shadowed)` — diffs the
  forge's **enabled** manifests against the registry: hot-add, hot-remove
  (disabled/deleted on disk), and a shadow guard (a skill may never replace a
  built-in tool name; skipped with a warning).
- **Delegate resolution moved into the execution chokepoint**: `execute_tool`
  resolves `Delegate` skills to their underlying tool *before* the safety layers
  run, so policy (re-evaluated per hop), Track 0, dynamic trust, and approval all
  see the real call (e.g. the actual `gpio_write`), not the wrapper. Delegate
  chains are bounded (3 hops) so cycles terminate deterministically.
- `tool_names()` returns `Vec<String>` (was `Vec<&str>`; can't borrow through the
  lock). All call sites already went through `AgentHandle`, which returned owned
  strings anyway.
- Phase 16 reuse metric: `learned_skill_invocations_total` counter incremented
  for every `learned_*` tool call (when obs is attached).

### Changed — `src/tools/traits.rs`

- New default trait method `Tool::as_delegate() -> Option<(String, Value)>` —
  a `Delegate` skill exposes its target + fixed args for chokepoint resolution.
- New `impl Tool for Arc<dyn Tool>` (pure delegation, including `risk_class` and
  `as_delegate`) so registry snapshots can be boxed for the provider call without
  touching the `Provider` trait.

### Changed — `src/skill_forge/`

- `SkillTool` (`mod.rs`): `as_delegate()` override for enabled `Delegate` skills;
  standalone execution of a `Delegate` skill is now an **explicit error** (it
  previously returned a fake success string — silent no-op).
- `SkillImprover` (`improve.rs`): new `ReplayExecutor::on_skills_changed(&SkillForge)`
  hook (default no-op) — the agent implements it as `sync_skills`, so a pass that
  installs skills hot-reloads the live registry, no restart. New `.with_obs()`:
  each pass records `self_improve_{scanned,candidates,installed,quarantined,rejected}_total`.

### Changed — `src/main.rs`, `src/agent/orchestrator.rs`

- `SkillForgeTool` (list/install/remove) registered as a built-in tool (it was
  never wired in).
- Startup `sync_skills` on the plain agent and the orchestrator's inner agent —
  enabled forge skills are first-class tools from boot.
- The agent now gets the shared `ObsContext` (`with_obs`) in `main.rs` — Phase 15
  wired agent spans/counters but never attached the context at startup; agent-loop
  spans and the new Phase 16 counters are live in `/api/v1/metrics`.
- The improvement loop gets `.with_obs(obs)`.

### Tests

- 4 new goldens in `tests/evals.rs` (`skill_loop`): hot add/remove + shadow guard;
  a learned delegate skill executes the real underlying tool with fixed args
  merged under runtime args; delegate cycles are cut by the hop bound; the
  learned-skill invocation counter increments.
- `improve.rs`: hot-reload hook fires exactly once after an installing pass.
- `skill_forge/mod.rs`: delegate skills expose their target and refuse standalone
  execution; disabled delegates expose no target.
- Full workspace: **1082 lib tests + all integration suites green** (evals 19/19)
  on Windows.

Safety invariants preserved: quarantined (disabled) skills are never registered;
physical learned skills stay operator-gated; delegate resolution *strengthens*
Track 0 (the gate now sees the real actuator call instead of a skill wrapper).

---

## Unreleased — Phase B: LoRa mesh spine (Heltec V3) (2026-07-03)

Off-grid inter-node transport: OBC nodes exchange their spine messages over a LoRa
mesh when there's no WiFi/MQTT backhaul. Two-tier "serial-bridged compute" design —
the XIAO runs the node firmware and mirrors its JSON out a UART to a Heltec V3, which
carries it over LoRa. Validated on hardware (2× Heltec V3, 1× XIAO). Full runbook:
`docs/PHASE-B-LORA-MESH.md`.

### Added — `firmware/heltec-lora-linktest` (new crate, Heltec WiFi LoRa 32 V3)

- **Hand-rolled SX1262 driver** (`sx1262.rs`) over ESP-IDF SPI: reset/standby,
  TCXO(DIO3, 1.8 V), calibration, LoRa modulation/packet params, PA/TX config, TX and
  RX with RSSI/SNR, and a bounded BUSY handshake (a dead radio errors, never hangs).
  915 MHz, SF7/BW125/CR4-5, private sync word 0x1424, +22 dBm. Came up correct on the
  **first flash**; two boards exchange packets at real RSSI.
- **Spine transport** (`spine.rs`): `[src][seq][ttl][payload]` frame + a `SeenSet`
  de-dup ring. Content-agnostic — carries the node's opaque JSON.
- **UART↔LoRa gateway bridge** (`main.rs`): frames newline lines from UART1
  (TX=GPIO4, RX=GPIO2) onto LoRa; forwards received frames to UART1 + console; emits a
  keepalive so the link stays observable.
- **Flood-relay**: a newly-seen frame is rebroadcast with `ttl-1` (`SPINE_TTL`=2),
  original `src`/`seq` preserved so the de-dup stops loops. Verified one-relay-per-node,
  no storm.

### Added — host `src/spine/lora_gateway.rs` (host ⇄ mesh bridge, inbound)

- **LoRa mesh gateway bridge**: the far end of the Phase B spine. Reads a
  base-station Heltec's USB console and ingests the node spine messages it hears over
  the air into **world memory** — so link state, power mode, and reflex/safing reports
  that arrive over LoRa land in the brain's world model, exactly as if the node were on
  the wired MQTT spine.
- `parse_gateway_line` anchors on the gateway's `SPINE ◄ src=.. seq=.. rssi=.. dBm :
  {json}` format (tolerating an ESP-IDF log prefix), and returns only **received**
  frames — TX (`►`), relay (`⇒`), malformed-frame, and boot lines are ignored.
- `ingest_gateway_line` writes two facts per message: `mesh.<node_id>.<type>` (the node
  payload + a `_mesh` envelope carrying `src`/`seq`/`rssi_dbm`) and a `mesh.<node_id>`
  liveness/link rollup (rssi, seq, last type) — so `current("mesh.<node_id>")` answers
  "is this node alive, and how strong is the mesh link?".
- The parse + ingest core is **hardware-free and unit-tested** (6 tests); only the
  serial read loop (`run_gateway_rx`) is gated behind `--features hardware` (tokio-serial),
  matching the other peripheral drivers.
- Config: `[lora_gateway]` (`port`, `baud`) on the root config; `main.rs` spawns the
  bridge when it's set, world memory is on, and the build has the `hardware` feature
  (clear warnings otherwise).

### Added — host ⇄ mesh (outbound return path)

- **`mesh_command` agent tool** (`src/tools/builtin/mesh.rs`): the inverse of the
  inbound bridge — the agent (System 2) addresses a command to a node over LoRa.
  `NodeCommand` (`lora_gateway.rs`) encodes to the node's own request line (`id`/`cmd`/
  `args`) plus a `to` routing field, delivered through a `CommandSink`
  (`SerialCommandSink` writes it to the base-station Heltec's console). The tool
  declares a **physical, high-blast `RiskClass`** so the host approval layer gates it
  per-call. Encode + sink covered by unit tests (mock sink).
- **One shared port, both directions**: `open_split` opens the base-station console
  once and splits it — the RX ingest loop and the outbound writer share the single
  exclusive serial port. `run_gateway_rx` now takes the read half; `main.rs` spawns RX
  (when world memory is on) and registers `mesh_command` from the write half.
- **Node-side intake** (`firmware/obc-esp32-s3`): the XIAO drains its spine UART (UART1
  RX = GPIO44 / D7) for command lines, routes on `to` (its `NODE_ID` or broadcast), and
  dispatches through the **same Track 0-gated `handle_request`** as a wired USB command
  — so a `gpio_write` arriving over the air actuates only within the node's on-MCU
  allow-list/range/rate limits. The reply is written back out the UART to ride LoRa
  home. *(Flash-pending.)*
- **Base-station command origin** (`firmware/heltec-lora-linktest`): a background thread
  reads newline/CR-delimited lines from the Heltec's USB console (UART0 `stdin`) and the
  radio loop frames each onto LoRa. It only *reads* stdin — no UART0 driver install, so
  `EspLogger` console output is untouched. Any Heltec running this firmware can originate
  a mesh command; a host types/pipes a JSON command line into its serial monitor. A
  USB-TTL-to-UART1 path is documented as a no-firmware fallback. *(Flash-pending; needs
  the reverse jumper Heltec GPIO4 → XIAO D7 to reach the node.)*
- **Identifiable command replies**: the node stamps its command response with
  `type:"cmd_result"` and its `node_id` before mirroring it back over the mesh, so a
  reply lands in world memory as `mesh.<node_id>.cmd_result` (correlatable by the echoed
  `id`) rather than a generic src-addressed fact.
- **Host integration test** (`tests/mesh_spine_e2e.rs`): exercises the full outbound →
  gated-execution → reply → world-memory loop on the host with no radio, asserting the
  routing (`to`), identity (`cmd_result`/`node_id`), and correlation (`id`) contract the
  firmware realises. Runs on any machine (`cargo test --test mesh_spine_e2e`).

### Added — mesh supervisor (fold the mesh into the brain)

- **`src/spine/mesh_supervisor.rs`**: a host control loop that turns the mesh facts the
  inbound bridge lands in world memory into *action*. Each tick it derives a per-node
  health view — **online / degraded / offline** — from the `mesh.<node>` rollup and last
  `cmd_result`, writes it back as a `mesh.<node>.health` fact (so reflexes, foresight, and
  the agent can see it), and — when a node goes offline — autonomously issues a
  **rate-limited recovery `mesh_command`** (e.g. a `capabilities` ping) through the same
  mesh command sink the agent uses.
- Perception → decision → action, fully on-host: the decision core (`decide`) is pure and
  **unit-tested** (7 tests — online/offline/degraded classification, churn-free health
  writes, per-node recovery rate-limiting, observe-only mode, and a `tick` integration
  over an in-memory store + mock sink).
- Config `[mesh_supervisor]` (`enabled`, `stale_ms`, `tick_ms`, `recover`,
  `min_recovery_interval_ms`); `main.rs` spawns the loop when enabled and world memory is
  on, sharing the LoRa gateway's command sink so recovery can actually reach a node.
  Without a sink it runs observe-only (health facts, no sends).
- **Escalation (presumed lost)**: a node continuously offline past `escalate_after_ms`
  is escalated — recovery pings stop and a `mesh.<node>.escalation` fact is raised (and
  auto-cleared if the node returns). Stops the mesh pinging a dead node forever and gives
  other reflexes / the agent a "give up, raise the alarm" signal. Unit-tested (offline→
  escalate→stop-pinging→return→clear).
- **`oh-ben-claw status`** now prints a **Mesh nodes** section — per-node health
  (online/degraded/offline), link RSSI, last message type + age, and a "(presumed lost)"
  flag for escalated nodes — read straight from world memory.
- **Health-driven reflex**: the supervisor publishes `mesh.escalated_count` (a plain
  number) and a new standard safing rule **`safe-mesh-node-lost`** (`mesh.escalated_count
  >= 1 → Escalate`) fires when any node is presumed lost — waking System 2 to alert,
  re-plan, or dispatch. This closes the mesh loop end to end: perception over LoRa →
  world memory → System 1 reflex → System 2. Safe by default (the count is absent/zero
  until escalation is configured *and* a node is actually given up on). Integration-tested
  (escalate → count → reflex fires).
- **`mesh_status` agent tool** (read-only): summarizes fleet health from world memory —
  per-node status / escalation / RSSI / last-seen + fleet counts — so **System 2**, once
  the escalation wakes it, can see *which* node is in trouble and act on it (via
  `mesh_command`). Registered whenever mesh is configured; unit-tested. Completes the
  agent's mesh toolkit: perceive (`mesh_status`) → act (`mesh_command`).
- **Mesh triage playbook**: the `safe-mesh-node-lost` escalation reason is now a concise
  triage *directive* (`MESH_LOST_PLAYBOOK`) that names the exact tools — so the wake is
  self-guiding: identify with `mesh_status`, confirm with a `mesh_command` `capabilities`
  ping, then recover-or-alert, all Track-0 gated. Full procedure (with guardrails) in
  `docs/playbooks/mesh-node-lost.md`. Unit-tested that the reason directs the agent to its
  tools.

### Added — escalation notifications (`src/agent/notify.rs`)

- **Escalations now reach humans.** Every reflex `Action::Escalate` (mesh node lost,
  battery critical, alarm heard, …) is fanned out to operator-facing channels via a
  `NotifyingActionSink` **decorator** that notifies, then delegates to the inner sink — so
  the wake-System-2 path is unchanged and notification is additive + best-effort (a down
  webhook never stalls System 1).
- Channels: a **world-memory log-of-record** (`notifications.escalation` — durable and
  queryable via `history`) and an optional **webhook** (`{ "text": … }`, Slack/Discord-
  compatible). Config `[notifications]` (`enabled`, `log_to_world_memory`, `webhook_url`).
- **Speech channel**: with `speak_escalations = true`, escalations are also spoken aloud —
  **headline only** (`speech_headline` takes the first sentence, so a full triage directive
  isn't read out) — through the audio speech sink (TTS / speaker-over-spine / dry-run, same
  selection as `[audio_suite]`). The robot can now *announce* an alarm.
- **Digest / de-dup**: identical escalations within `dedup_window_ms` are suppressed and
  counted across all channels — so a flapping node doesn't spam the log/webhook/speaker every
  tick — and the next alert after the window carries a `[+N repeats suppressed]` note, so
  repeats are collapsed into a digest, never silently dropped. Scoped to notifications: the
  System 2 wake still fires each time (governed by the reflex's own escalation budget), and
  distinct reasons are never deduped against each other.
- **Periodic digest**: with `digest_interval_ms` set (e.g. `86400000` for daily), a
  scheduled loop rolls the escalation log (`notifications.escalation`) up by reason — most
  frequent first, over the same trailing window — and delivers a one-line summary through the
  same channels. A low-noise companion to per-event alerts; prior digests are excluded from
  the next one (no compounding).
- **Severity routing**: escalations are classified (`Severity::classify` — keyword-based;
  danger words → `Critical`, otherwise `Warning`) and each channel carries a **minimum**
  severity (`log_min_severity` / `webhook_min_severity` / `speak_min_severity`; default =
  receive everything). So you can log *all* escalations but only push/speak the critical
  ones. Severity lives entirely in the notification layer — no change to the reflex `Action`
  enum.
- **`oh-ben-claw status`** now also prints a **Recent escalations** section — the last 5 from
  the log-of-record, newest first, each with `[severity]` and age — so mesh health and
  escalations are both visible at a glance.
- Unit-tested: the log-of-record write, the Slack-shaped payload, the speech headlining, the
  de-dup window + suppressed-count rollup, the digest grouping/windowing/formatting, digest
  delivery bypassing de-dup, and that the decorator notifies *and* still delegates the
  escalate downstream.

### Changed — `firmware/obc-esp32-s3` (XIAO node)

- **Spine mirror**: the node's autonomous `link_state` / `power_mode` / `reflex` JSON
  is mirrored out UART1 (TX=GPIO43 / D6 pad) to a LoRa gateway — best-effort (`.ok()`
  plus a boot log reporting whether the uplink UART initialised), so the node still
  runs untethered. Its real on-MCU reflexes can ride the mesh to another node.

Remaining: the XIAO→Heltec physical jumper (continuity check pending) and a true
3-hop relay test (needs a 3rd radio placed out of direct range).

---

## Unreleased — Practical temperature + humidity safing rules (2026-07-02)

Extended the built-in on-MCU safing library with environmental self-protection,
now that the DHT22 feeds real `sensor.temperature` / `sensor.humidity` into the
reflex snapshot. These load at boot — a node self-protects with no host push.

### Added — `firmware/obc-esp32-s3/src/safing.rs`

- Three built-in safing rules (built-in safing count is now 6):
  - `safe-overtemp-critical` — `sensor.temperature ≥ 75 °C` → cut the actuator-enable
    pin (shed heat-producing loads); same protective action as critical battery.
  - `safe-overtemp-warn` — `≥ 60 °C` → escalate a shed-load / cooling advisory.
  - `safe-humidity-high` — `sensor.humidity ≥ 90 %RH` → escalate a condensation-risk
    warning.
- Thresholds exposed as `DEFAULT_OVERTEMP_CRITICAL_C` (75), `DEFAULT_OVERTEMP_WARN_C`
  (60), `DEFAULT_HUMIDITY_HIGH_PCT` (90); new `TEMPERATURE_ENTITY` / `HUMIDITY_ENTITY`
  constants. Unit test `overtemp_and_humidity_safing` covers all three firing plus
  comfortable-room dormancy.

---

## Unreleased — DHT22/AM2302 temperature + humidity driver (2026-07-02)

Added a real single-wire environmental sensor driver, since the MPU6050 on the
bench was lost to an earlier short and the DHT22 is the sensor actually in hand.

### Added — `firmware/obc-esp32-s3/src/dht.rs`

- Bit-banged DHT22/AM2302 reader returning `(temperature_c, humidity)`. The
  40-bit frame is read inside an **interrupt-free critical section** (~5 ms) so
  RTOS/WiFi preemption can't corrupt the µs-level pulse timing, with **every
  edge-wait bounded** (a missing/dead sensor errors instead of hanging — same
  principle as the I²C timeout fix) and **no heap allocation** inside the
  critical section. Result is checksum- **and** range-validated before it's
  trusted; a bit-threshold constant (`HIGH_BIT_THRESHOLD_US`) is exposed for
  on-bench tuning.

### Changed — `firmware/obc-esp32-s3/src/main.rs`

- `sensor_read` handles `sensor:"dht22"` (`temperature`/`humidity`) on its own
  single-wire GPIO, separate from the I²C bus. Data pin: **D10 / GPIO9**
  (`DHT22_GPIO`) — a free pad clear of the actuators, I²C (4/5), and I²S (1/2).
- **Wired into the autonomous reflex snapshot**, rate-limited to the sensor's
  ~2 s minimum (last good value reused between the faster reflex ticks, cached on
  `AgentState`). Real `sensor.temperature` now overrides the stub and drives the
  overheat/safing rules on measured data; `sensor.humidity` is also published.
  A DHT read failure keeps the last good value rather than dropping to the stub.

---

## Unreleased — Finite I²C timeout: a bad sensor can no longer hang the node (2026-07-02)

Bench-found robustness bug. Every I²C transaction in the sensor driver used
`delay::BLOCK` (an infinite timeout), so a stuck bus — e.g. a half-wired sensor
holding SDA low — made `write_read` block forever, freezing the single-threaded
main loop and hanging the entire node until a physical power-cycle. For an embodied
brain, one flaky sensor must never be able to take down System 1.

### Changed — `firmware/obc-esp32-s3/src/sensors.rs`

- Introduced `const I2C_TIMEOUT = TickType::new_millis(50).ticks()` and replaced all
  11 `BLOCK` timeouts (MPU6050 wake + read, MAX17048, BME280 probe/config/measure/read)
  with it. A stuck or absent sensor now returns a clean `{"ok":false,"error":...}`
  read error within 50 ms instead of hanging the node — the reflex/safing loops keep
  running. 50 ms is generous for a 100 kHz bus (each transaction is well under 1 ms).

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

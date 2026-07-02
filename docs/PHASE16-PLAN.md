# Phase 16 — Experiential Self-Improvement: Gap Analysis & Implementation Plan

*Audited 2026-07-02. Companion to `docs/V2-STRATEGY.md` and the Phase 16 checklist in `ROADMAP.md`.*

## Audit: what already exists

More of Phase 16 is built than the roadmap checklist shows. Current state:

| Roadmap item | Status | Where |
|---|---|---|
| Trajectory capture | ✅ Done | `src/memory/trajectory.rs` (SQLite WAL `Episode` store); wired via `Agent::with_trajectory_store` in `main.rs` + orchestrator sub-agents; `[self_improvement]` config |
| Reflection + skill synthesis | 🔶 Partial | `src/skill_forge/synthesis.rs` — deterministic **single-step, zero-parameter** `Delegate` recipes only |
| Self-verification gate | 🔶 Partial | `src/skill_forge/improve.rs` — replay verification through the agent's normal chokepoint (policy + Track 0), `RiskClass` gating. `SensorAssertion` / `TestCommand` checks exist but are **never invoked** |
| Learned-skill library + retrieval | ❌ **Broken loop** | Skills install to disk, but `SkillForge::load_all()` is never called in `main.rs` — **no skill (learned or authored) is ever offered to the agent**. `TrajectoryStore::similar()` is dead code |
| Offline trace evolution (GEPA-style) | ❌ Missing | — |
| Safety interlock (Track 0) | 🔶 Partial | Physical/irreversible skills quarantined (`enabled=false`, `track0:supervised` tag) — solid. But no staged rollout (`simulate→supervised→autonomous`) and **no operator promotion surface** (no CLI cmd, no gateway endpoint, no GUI) |
| Metrics | ❌ Missing | `ImproveReport` is logged, not exported; no reuse-rate or token/latency counters; `Episode` doesn't record tokens/duration |

Adjacent, working: `src/learning/mod.rs` (world-memory rule mining + approval gate → foresight) — a separate loop, not touched here.

**Headline finding:** the improvement loop synthesizes, verifies, and installs skills that nothing can ever execute. Closing that loop is the whole ballgame.

## Implementation plan

Ordered by value; each step is independently shippable and testable.

### P0 — Close the loop *(✅ shipped 2026-07-02 — see CHANGELOG)*

> Landed with one deviation from the sketch below: instead of a `RwLock<Vec<Box<dyn Tool>>>`,
> the registry became `RwLock<Vec<Arc<dyn Tool>>>` + `impl Tool for Arc<dyn Tool>`, keeping the
> `Provider` trait untouched. P0 also surfaced and fixed a fourth gap the audit missed:
> `SkillKind::Delegate` execution was a stub (returned a string, invoked nothing) — delegate
> resolution now happens inside the agent chokepoint so Track 0 sees the real underlying call.

**1. Load skills into the agent.**
At startup: `all_tools.extend(forge.load_all()?)` in `main.rs` (+ register the existing `SkillForgeTool` management tool, also currently unregistered). Namespace guard: a skill may not shadow a built-in tool name.

**2. Hot skill reload.**
`Agent.tools` is a plain `Vec` behind `Arc<Agent>` — learned skills installed mid-run are invisible. Refactor `tools` to `RwLock<Vec<Box<dyn Tool>>>` (touches `chat_completion` call-site and the execute path in `src/agent/mod.rs`), add `Agent::sync_skills(&forge)` that diffs manifests against registered tools. `SkillImprover::run_periodically` calls it after any pass with installs. Orchestrator sub-agents sync on the same signal.

**3. Self-improvement observability.**
Counters: `learned_skills_installed_total`, `learned_skills_quarantined_total`, `learned_skill_invocations_total` (tool name prefix `learned_` in the existing `agent.tool` span). Expose last `ImproveReport` via `/api/v1/metrics`. This is the reuse-rate metric's numerator.

### P1 — Retrieval before reasoning *(✅ shipped 2026-07-02 — see CHANGELOG)*

**4. Experience-aware prompting.**
Before the LLM call, retrieve top-k relevant learned skills + proven episodes for the objective and inject a compact "learned experience" system-prompt block ("you have a verified recipe for this: `learned_x`"). Retrieval: upgrade `TrajectoryStore::similar()` from `LIKE` substring to embeddings via the existing `memory/vector.rs` (`EmbeddingClient` + `VectorStore`) when an embedder is configured; keep substring fallback. Embed episode objectives at record time.

> Landed with one deviation: the audit found `EmbeddingClient`/`VectorStore` are wired nowhere
> (no embedder config exists), so the scorer is deterministic token-overlap (stopword-filtered
> cosine) matching the RAG index's philosophy — no per-turn network latency, fully testable.
> An embedding backend can replace `lexical_score` behind the same `similar()` API when an
> embedder config lands (candidate for P4 alongside the offline-evolution job).

### P2 — Stronger synthesis + real verification *(items 5–6 ✅ shipped 2026-07-02; item 7 deferred to P4 — see CHANGELOG)*

**5. Multi-step + parameterized skills.**
New `SkillKind::Sequence { steps: Vec<SkillStep> }` with `{param}` placeholder substitution per step. Synthesize from all-ok multi-step episodes. Parameter extraction: diff args across ≥2 similar episodes — varying fields become parameters, stable fields stay fixed. Sequence replay-verifies only if **every** step is `safe_to_replay`.

**6. Wire up `SensorAssertion` / `TestCommand` checks.**
Improver accepts per-skill `VerificationCheck`s (from config or LLM proposal); a physical skill verified by an independent sensor reading can be promoted to `supervised` (still not autonomous). Grounds verification in real signal per V2-STRATEGY's Huang-et-al. caution.

**7. LLM reflective synthesis** *(config-gated, `[self_improvement].reflective = true)*.
Use the provider to name, describe, and parameterize candidates from raw episodes. LLM output is a *proposal only* — quarantine and deterministic verification apply unchanged.

### P3 — Staged rollout + operator surface (Track 0) *(✅ shipped 2026-07-02 — see CHANGELOG)*

> Landed with one semantic upgrade over the sketch: physical learned skills are now installed
> **enabled at the `simulate` stage** (instead of disabled), so the model can invoke them and
> build a promotable clean record while the chokepoint guarantees nothing actuates. Supervised
> execution requires an *explicit* grant — `Full` autonomy deliberately doesn't count.

**8. Rollout stages.**
`stage: simulate | supervised | autonomous` on `SkillManifest` (serde-default `autonomous` for authored skills; learned physical skills start at `simulate`). Promotion requires N clean runs at the current stage; any failure demotes. Auto-enabled non-physical learned skills map to `autonomous`.

**9. Operator surface.**
CLI: `oh-ben-claw skill list|show|promote|demote|remove`. Gateway: `GET /api/v1/skills`, `POST /api/v1/skills/{name}/promote` (auth-gated). GUI panel later.

**10. Red-team eval.**
Extend `tests/evals.rs`: a synthesized actuator skill must be unable to run unattended, at every stage below `autonomous`, even when the improver's executor reports success.

### P4 — Offline trace evolution *(✅ shipped 2026-07-02 — Phase 16 complete; see CHANGELOG)*

> All four phases of this plan landed on 2026-07-02 (P0 → P4, 26 new lib tests + 14 new evals
> along the way; workspace at 1108 lib tests / 29 evals, all green). Remaining candidates for a
> future pass, deliberately out of scope: LLM-reflective *synthesis* (naming/parameterization
> proposals — the deterministic pipeline stays the trust anchor), an embedding backend behind
> `lexical_score`, and a GUI panel for the skill rollout surface.

**11. GEPA-style batch job** *(scheduled, config-gated)*.
Periodically feed accumulated episodes + per-skill usage/success stats to the LLM to rewrite skill *descriptions* (improves selection) and propose prompt tweaks. Every change versioned in the manifest, diff-logged, revertible; never touches `enabled`/`stage`.

**12. Token/latency metrics.**
Extend `Episode` with `tokens` and `duration_ms` (additive migration). Report mean tokens/latency for objectives matched to a learned skill vs. before — the roadmap's "token/latency reduction on repeated routine tasks" metric.

## Sequencing & effort

P0 ≈ 1 session (small diffs, one careful `RwLock` refactor). P1 ≈ 1 session. P2 ≈ 2–3 sessions (Sequence kind is the bulk). P3 ≈ 1–2 sessions. P4 ≈ 1 session. Tests accompany every step; `cargo test --workspace` (incl. `tests/evals.rs`) gates each.

## Safety invariants (unchanged throughout)

- A synthesized skill is never enabled without independent verification.
- Physical/irreversible/blast-radius skills are never auto-verified by replay and never run unattended below the `autonomous` stage.
- All skill execution flows through the existing chokepoint: policy → Track 0 gate → trust → approval.

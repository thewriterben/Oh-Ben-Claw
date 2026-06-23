# Oh-Ben-Claw v2.0 — "Embodied Frontier"

*Strategy & feature plan for the next major version. Compiled June 23, 2026.*

*Decision frame (set with the maintainer): push toward **frontier agent capability**, **double down on the hardware/embodied identity**, scope a **full version arc** toward a named v2.0, delivered as this strategy doc plus ROADMAP.md phase entries.*

---

## 1. The thesis

Oh-Ben-Claw has spent 14 phases reaching **parity** with the agent ecosystem — channels, providers, MCP, A2A, browser automation, skills, cost, observability, sandboxing — and Phase 15 is making that parity **trustworthy**. That work is necessary but it is no longer differentiating: every serious agent framework now has those things. The 2026 landscape research (see `AI-Agents-Innovations-June2026.md`) shows the software-agent layer has consolidated and commoditized.

The defensible move is to stop chasing the pure-software frontier and instead **bring the frontier *into the place no software agent can follow*: the physical world.**

Three observations make this the right bet:

1. **Pure-software agents can't act physically.** Hermes Agent, LangGraph, the OpenAI/Microsoft/Google SDKs — all extraordinary, all blind and handless. Their moat ends at the API boundary.
2. **Robotics foundation models aren't competitors — they're a different sport.** NVIDIA GR00T N1.7, Gemini Robotics 1.5, Physical Intelligence π0.5, Figure Helix are *motor-control* models for *expensive humanoid robots*, trained on tens of thousands of hours of teleoperation. They solve "how does this 22-DoF hand grasp a cup," not "how does one intelligent agent orchestrate a fleet of $6 sensors and actuators across a building."
3. **Oh-Ben-Claw already owns the unclaimed middle.** An LLM brain orchestrating a heterogeneous fleet of cheap, dynamically-discovered hardware nodes over a message spine — that is a genuinely under-occupied position. v2.0's job is to make that position *frontier-grade*.

**v2.0 thesis, in one line:** *Take the five frontier agent capabilities of 2026 — experiential self-improvement, long-horizon autonomy, dual-system perception-action, real-time multimodal interaction, and physical-action safety — and realize each one natively for an embodied multi-device fleet.*

---

## 2. What "frontier" means in mid-2026 (and where OBC stands)

From the landscape and embodied-AI research, the capabilities the field treats as the current frontier:

| Frontier capability | State of the art (2026) | Oh-Ben-Claw today | Gap |
|---|---|---|---|
| **Experiential self-improvement** | Voyager skill libraries; Hermes Agent's learn-a-skill-per-task loop (~190K★); DSPy + GEPA reflective trace evolution; Anthropic Skills | `skill_forge` + ClawHub client + personality/journal files — but skills are authored/installed, **not learned from experience** | No trajectory→reflection→skill synthesis loop |
| **Long-horizon autonomy** | METR: ~5 hr 50%-reliable horizon (Opus 4.5), doubling ~every 3 mo since 2024; Anthropic initializer+worker harness; durable-execution infra (Temporal/Azure/Cloudflare) | Agent loop with compaction, scheduler, heartbeat, scoped approvals | No checkpoint/resume, no externalized world-state harness, no self-verification gates |
| **Dual-system perception-action** | Universal robotics pattern: slow VLM reasoner (System 2) + fast action policy (System 1); on-device VLAs | Single-cadence agent loop; vision + audio + sensor-fusion pipelines exist but feed one slow loop | No fast local reflex loop; no persistent spatial/world memory |
| **Real-time multimodal** | OpenAI Realtime API GA (gpt-realtime: audio-native + image input + MCP tools); Gemini Live API GA | Turn-based audio pipeline (STT→agent→TTS) | No streaming bidirectional voice/vision session |
| **Physical-action safety** | OWASP Top 10 for Agentic Apps (Dec 2025); NIST AI Agent Standards (Feb 2026); tool-call-boundary authorization (OAP); least-privilege for agents | Policy engine, scoped/plan approvals, pairing, vault, sandboxing | No deterministic model-independent actuator limits; no physical-risk class; approval is generic, not physical-aware |
| **Agent memory** | Temporal knowledge-graph memory (Zep/Graphiti bitemporal); Mem0 fact extraction; episodic/semantic/procedural taxonomy | SQLite + vector + journal + image memory | No temporal/graph world memory; memory isn't environment-aware |
| **Edge inference** | TinyML on MCUs; small models on Pi/Jetson; Cosmos 3 Edge ("coming soon"); Gemini Robotics On-Device | `edge.rs` (NanoPi local Ollama), ESP32 cloud-LLM edge loop | Edge inference not first-class; no small-model reflex tier; no on-device wake-word/STT |

The pattern: **OBC has the *organs* for every frontier capability but not the *frontier behavior*.** v2.0 is about activating the behavior, and doing it in a way only an embodied fleet can.

---

## 3. Prioritized feature analysis

Each candidate scored on the maintainer's four lenses — **Effective** (does it work / is it real), **Innovative** (novel, especially for embodied), **Useful** (does it serve real OBC deployments), **Demanded** (is the market pulling for it) — plus **Leverage** (how much it reuses existing modules) and **Risk**.

### Tier 1 — flagship (build first)

**A. Experiential self-improvement loop** *(maps to `skill_forge`, `memory`, `agent`)*
- Effective: ⭐⭐⭐⭐ (Voyager/Hermes proven; reflection works *when there's a feedback signal and durable storage* — design around that). Innovative: ⭐⭐⭐⭐⭐ for an *embodied* fleet (a home agent that learns "how to run the morning routine" as a reusable skill is novel). Useful: ⭐⭐⭐⭐⭐. Demanded: ⭐⭐⭐⭐⭐ (this is the single feature driving Hermes Agent's adoption). Leverage: ⭐⭐⭐⭐⭐ (skill_forge + ClawHub + journal already exist). Risk: medium (reflection can degrade — must gate with self-verification + human approval).
- **Why first:** highest demand × highest leverage. It's the headline "frontier" feature and it sits on top of code OBC already has.

**B. Physical-action safety & trust layer** *(maps to `approval`, `security`, `peripherals`)*
- Effective: ⭐⭐⭐⭐⭐. Innovative: ⭐⭐⭐⭐ (deterministic, model-independent safety limits at the actuator boundary is rare even in research). Useful: ⭐⭐⭐⭐⭐. Demanded: ⭐⭐⭐⭐ (OWASP/NIST making it table stakes; uniquely acute for physical agents). Leverage: ⭐⭐⭐⭐. Risk: low.
- **Why first (parallel with A):** you cannot responsibly ship *more* autonomy onto devices that move motors and open locks without this. It's a foundation, not a feature — and it's a genuine differentiator vs. software agents that have no physical blast radius.

### Tier 2 — core v2.0 capabilities

**C. Long-horizon embodied autonomy harness** *(maps to `scheduler`, `agent`, `memory`, `runtime`)*
- Effective ⭐⭐⭐⭐ / Innovative ⭐⭐⭐⭐ / Useful ⭐⭐⭐⭐⭐ / Demanded ⭐⭐⭐⭐ / Leverage ⭐⭐⭐⭐. Durable checkpoint/resume + initializer-worker + externalized progress + mandatory self-verification, so an unattended fleet survives crashes, reboots, and context limits over multi-hour/multi-day operation. The Anthropic harness pattern, adapted so the "progress file" is the *physical world state*.

**D. Dual-system perception-action + world memory** *(maps to `agent/edge`, `vision`, `peripherals/fusion`, `memory`)*
- Effective ⭐⭐⭐ / Innovative ⭐⭐⭐⭐⭐ / Useful ⭐⭐⭐⭐ / Demanded ⭐⭐⭐ / Leverage ⭐⭐⭐. Adopt the robotics-standard **System 2 (slow cloud reasoner) + System 1 (fast local reflex)** split across the fleet, backed by a persistent **bitemporal world memory** (what device/room was in what state, when). This is the most *architecturally* frontier move and the most embodied-native.

**E. Real-time multimodal interaction** *(maps to `channels`, `audio`, `multimodal`)*
- Effective ⭐⭐⭐⭐⭐ / Innovative ⭐⭐⭐ / Useful ⭐⭐⭐⭐⭐ / Demanded ⭐⭐⭐⭐ / Leverage ⭐⭐⭐. A streaming bidirectional voice/vision session (OpenAI Realtime / Gemini Live) turns existing nodes — the ESP32-S3 mic/speaker, the Waveshare touch-LCD already in the board registry — into genuinely conversational ambient devices. High user-visible payoff.

### Tier 3 — enablers / depth

**F. First-class edge inference** *(maps to `agent/edge`, `providers`)* — small-model reflex tier, on-device wake-word/STT, graceful cloud fallback. Strengthens the privacy/latency/offline moat. Pairs naturally with D and E.

**G. Temporal/graph memory upgrade** *(maps to `memory`)* — Graphiti-style bitemporal knowledge graph; largely subsumed by D's world memory, so fold it in rather than building twice.

**H. Planner-executor orchestration** *(maps to `agent/orchestrator`, `deployment`)* — deepen the existing orchestrator into an explicit high-level planner + per-node executors; an enabler for C and D.

### Explicit non-goals (what v2.0 should *not* chase)

- **Don't build a VLA motor-control model.** Grasping, locomotion, and dexterity are GR00T/π0/Helix territory and require robot fleets + teleop data OBC will never have. OBC orchestrates devices; it does not replace the robot's own controller. (If anything, *integrate* an on-device VLA as one more peripheral capability later.)
- **Don't broaden into a general software-agent runtime.** That market is consolidated and commoditized; competing there forfeits the moat.
- **Don't add more channels/providers for their own sake.** Parity is done. New surface area must serve the embodied frontier.

---

## 4. The v2.0 arc

A named major version — **v2.0 "Embodied Frontier"** — built as five phases plus one cross-cutting track. Safety (Track 0) runs underneath everything because it gates physical autonomy.

```
                          v2.0 "Embodied Frontier"
   ┌─────────────────────────────────────────────────────────────────┐
   │  Track 0 (cross-cutting): Physical-Action Safety & Trust          │
   ├─────────────┬─────────────┬─────────────┬───────────┬────────────┤
   │  Phase 16   │  Phase 17   │  Phase 18   │ Phase 19  │  Phase 20  │
   │ Experiential│ Long-Horizon│ Dual-System │ Real-Time │  Edge-     │
   │ Self-       │ Embodied    │ Perception- │ Multimodal│  Native    │
   │ Improvement │ Autonomy    │ Action +    │ Interaction│ Intelligence│
   │ (flagship)  │ Harness     │ World Memory│           │            │
   └─────────────┴─────────────┴─────────────┴───────────┴────────────┘
        ▲ near-term slice ▲
```

**Near-term slice (call this first):** Phase 16 (self-improvement) + the Track 0 safety foundations. Together they deliver the headline frontier feature *and* the safety floor that makes shipping it responsible — and both reuse existing modules heavily, so they're achievable fast.

### Phase 16 — Experiential Self-Improvement *(flagship)*
The agent learns reusable, verified skills from its own successful task trajectories, instead of only running authored/installed skills. Trajectory capture → reflection → skill synthesis with self-verification → store in the local skill library (ClawHub-compatible) → retrieve on similar tasks. Offline, a GEPA/DSPy-style loop evolves prompts and skill descriptions from execution traces. **Every synthesized skill that touches an actuator passes through Track 0 before it can run unattended.**

### Phase 17 — Long-Horizon Embodied Autonomy Harness
Durable, resumable, self-verifying operation across hours/days and across crashes, reboots, and context limits. Initializer agent establishes environment + an externalized **world-state progress record**; worker agent advances one objective at a time, self-verifies via sensors/cameras before marking done, checkpoints to durable storage, and resumes cleanly. The Anthropic harness pattern, but the "feature list" is physical world state.

### Phase 18 — Dual-System Perception-Action + World Memory
Split cognition the way every 2026 robotics stack does: **System 1** = fast, local, near-deterministic reflex loop on edge nodes (millisecond–second latency, runs offline); **System 2** = slow, cloud LLM reasoning for planning and novelty. Back both with a **bitemporal world memory** — a persistent, queryable model of rooms, devices, and their states over time, with validity intervals so stale facts are invalidated, not lost.

### Phase 19 — Real-Time Multimodal Interaction
A streaming bidirectional voice+vision session channel (OpenAI Realtime / Gemini Live) so fleet devices become ambient conversational agents: speak to the room, the agent sees the camera feed and hears continuously, interrupts and responds in real time, and can call tools mid-conversation.

### Phase 20 — Edge-Native Intelligence
Make on-device inference first-class: a small-model reflex tier for System 1, on-device wake-word + STT/TTS, and graceful, policy-driven cloud fallback. Deepens privacy, latency, and offline resilience — the embodied moat.

### Track 0 (cross-cutting) — Physical-Action Safety & Trust
Deterministic, **model-independent** safety limits enforced at the actuator boundary (rate, range, interlocks the LLM cannot override); a **physical-risk classification** for every tool (reversible/irreversible, low/high blast radius) that drives approval defaults; pre-action authorization with signed audit records; staged rollout (simulate → supervised → autonomous). Aligns OBC with the OWASP Top 10 for Agentic Applications and the NIST AI Agent Standards direction.

---

## 5. Sequencing rationale

- **Lead with 16 + Track 0** because they are highest-demand × highest-leverage × safety-foundational, and they unlock responsible autonomy for everything after.
- **17 depends on Track 0** (you won't run multi-hour unattended physical work without deterministic limits) and on 16's externalized-memory primitives.
- **18 is the architectural keystone** — the dual-system split and world memory are what make 17 robust and 19/20 natural; it's sequenced third so the self-improvement and harness primitives exist to build on.
- **19 and 20 are the user-facing payoff** and can proceed largely in parallel once 18's edge/world-memory substrate lands; 20 enables 19 to degrade gracefully offline.

## 6. How we'll know v2.0 worked

- **Self-improvement:** measurable drop in tokens/latency on repeated routine tasks as learned skills replace from-scratch reasoning; learned-skill reuse rate; zero unsafe auto-runs of synthesized actuator skills (Track 0 gate holds).
- **Long-horizon:** an unattended fleet completes a defined multi-hour routine across an induced crash/reboot with correct resume and no duplicated physical actions.
- **Dual-system:** System 1 reflexes meet a latency budget offline; System 2 invoked only on novelty; world-memory queries return temporally-correct device state.
- **Realtime:** end-to-end spoken interaction latency budget met on a reference ESP32-S3 + mic/speaker node.
- **Safety:** every catalogued physical tool has a risk class and a deterministic limit; injected-malicious-skill and injected-prompt tests cannot drive an out-of-limit actuator command; all physical actions produce signed audit records.
- All of it gated by the existing eval harness (Phase 15) extended with embodied golden tasks — no release while evals regress.

## 7. Risks & honest caveats

- **Reflection degrades without a feedback signal** (Huang et al., ICLR 2024). Phase 16 must anchor self-improvement to *real* verification (sensor/camera confirmation, test execution), not the model's own say-so.
- **Frontier autonomy numbers are wide-interval.** METR's ~5 hr horizon has a confidence band roughly 2–3× the point estimate; design Phase 17 for failure and resume, not for an optimistic best case.
- **Physical irreversibility raises the safety bar** far above software agents — Track 0 is non-negotiable, not a nice-to-have.
- **Source hygiene:** parts of the supporting research come from a 2026 web heavily polluted with fabricated model names and benchmark scores; load-bearing claims here are anchored to primary sources (METR, Anthropic, NVIDIA, DeepMind, OWASP/NIST, arXiv). Treat any precise third-party leaderboard figure as unverified until checked at the primary source.

---

*Companion: see the new Phase 16–20 + Track 0 entries appended to `ROADMAP.md` for the execution checklist in the repo's standard format. Supporting research: `AI-Agents-Innovations-June2026.md`.*

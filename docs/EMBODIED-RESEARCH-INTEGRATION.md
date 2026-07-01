# Oh-Ben-Claw — Embodied-Agent Research Integration

*Compiled 2026-07-01. This is the **LLM/VLM-agent companion** to [`SOTA-COMPARISON.md`](SOTA-COMPARISON.md). That doc benchmarks OBC's **classical-robotics** layers (Nav2, SLAM, behavior trees, AMCL, Open-RMF). This doc mines the [Awesome-Embodied-Robotics-and-Agent](https://github.com/zchoi/Awesome-Embodied-Agent-with-LLMs) list — the **VLM/LLM-agent** literature — and maps each research cluster onto a concrete OBC module or roadmap phase, with a prioritized set of enhancements.*

> **How to read this.** Every recommendation names (1) the source cluster in the Awesome list, (2) the OBC module or phase it lands in, (3) a concrete proposal, and (4) an honest note on scope. OBC's thesis is unchanged: we are a **safety-first, interpretable, embodied fleet brain**, not a motor-control VLA. So most of these papers contribute *architecture and method* to the reasoning/learning/coordination layers — **not** a new learned visuomotor policy. Items that would forfeit the embodied moat are collected in [§11 Watch-list](#11-watch-list-explicitly-out-of-scope).

---

## Contents

1. [Executive summary](#1-executive-summary)
2. [Phase 16 — Experiential Self-Improvement ← *Self-Evolving Agents*](#2-phase-16--experiential-self-improvement--self-evolving-agents)
3. [Phase 18 — World Memory & Dual-System ← *LLMs with World Models*](#3-phase-18--world-memory--dual-system--llms-with-world-models)
4. [Phase 17 — Long-Horizon Autonomy ← *Open-world lifelong agents*](#4-phase-17--long-horizon-autonomy--open-world-lifelong-agents)
5. [Agent loop — grounding, reflection & planning primitives](#5-agent-loop--grounding-reflection--planning-primitives)
6. [Navigation & VLN ← *Vision-Language Navigation + 3D Grounding*](#6-navigation--vln--vision-language-navigation--3d-grounding)
7. [Fleet ← *Multi-Agent Learning and Coordination*](#7-fleet--multi-agent-learning-and-coordination)
8. [Learning tiers ← *Reward design & RL-assist*](#8-learning-tiers--reward-design--rl-assist)
9. [Phases 19–20 — Real-time multimodal & edge-native ← *Efficient VLA*](#9-phases-1920--real-time-multimodal--edge-native--efficient-vla)
10. [Evaluation ← *Benchmarks & Simulators*](#10-evaluation--benchmarks--simulators)
11. [Watch-list (explicitly out of scope)](#11-watch-list-explicitly-out-of-scope)
12. [Prioritized backlog](#12-prioritized-backlog)

---

## 1. Executive summary

The Awesome list is organized around fourteen clusters. Nine map directly onto an OBC subsystem that **already exists in skeleton form**, which means most of the value here is *deepening interfaces we already shaped correctly* — the same conclusion `SOTA-COMPARISON.md` reached for the robotics layers.

| Awesome cluster | OBC home | State today | Highest-value borrow |
|---|---|---|---|
| Self-Evolving Agents | `skill_forge/`, `agent/reflexion.rs`, `learning/` | scaffolded (Phase 16 planned) | **Voyager** skill library + **verifier-anchored** reflection |
| LLMs with World Model | `memory/world.rs` | bitemporal store shipped | **LLM-DM** symbolic world-model for planning; **scene-graph memory** |
| Planning & Manipulation | `agent/`, `mission/`, `security/limits` | mission BT shipped | **SayCan** affordance grounding ≙ Track 0 "can"; **Code-as-Policies** |
| Open-world lifelong agents | `scheduler/`, `agent/`, `memory/` | Phase 17 planned | **JARVIS-1** memory-augmented resume; **DEPS** self-explain replanning |
| Vision-Language Navigation | `navigation/`, `vision/` | full nav column shipped | **ESC/L3MVN** commonsense frontier; **VoxPoser** 3D value maps → costmap |
| 3D Grounding | `navigation/costmap.rs`, `vision/clawcam_spatial.rs` | spatial tools shipped | **RoboSpatial/RoboRefer** spatial-referring evals |
| Multi-Agent Coordination | `fleet/` | auction + mutex shipped | **Co-LLM-Agents** modular comms; **desire-alignment** comms budget |
| Reward / RL-assist | `learning/`, `foresight/` | rule-mining shipped | **Eureka/Text2Reward** to *score* mined rules, not train policies |
| Benchmarks & Simulators | `tests/`, Phase 15 eval harness | eval harness planned | **ALFWorld/SQA3D/NavSpace** as text-grounded regression evals |

**The three borrows with the best leverage-to-cost ratio** (all near-term, all reinforce the safety story rather than dilute it):

1. **Verifier-anchored skill synthesis** (Voyager + Code-as-Monitor) into Phase 16 — turns `skill_forge` from "authored/installed skills" into "learned, *proven* skills," with failure detection as the verification signal.
2. **SayCan-style affordance grounding** wired to Track 0 — reframes the safety gate as the *affordance* half of "can + do," giving the planner a principled feasibility filter it currently lacks.
3. **Symbolic world-model extraction** (LLM-DM) over the bitemporal store — lets the LLM emit a checkable transition model for missions, closing the loop between `memory/world.rs` and `mission/`.

---

## 2. Phase 16 — Experiential Self-Improvement ← *Self-Evolving Agents*

**Roadmap home:** Phase 16; **modules:** `src/skill_forge/{synthesis,improve}.rs`, `src/agent/reflexion.rs`, `src/memory/trajectory.rs`, `src/learning/`.

This is OBC's flagship near-term phase and the Awesome list's richest cluster for us. The scaffolding already exists (`trajectory.rs` captures episodes; `skill_forge/synthesis.rs` distils skills; `reflexion.rs` is present) — the papers supply the *method* for each planned bullet.

- **[Voyager: An Open-Ended Embodied Agent with LLMs](https://openreview.net/attachment?id=pAMNKGwja6&name=pdf)** (NeurIPS 2023 WS) — the canonical **automatic curriculum + growing skill library + iterative-prompting** loop. Directly models Phase 16's "learned-skill library + retrieval." *Borrow:* store synthesized skills as retrievable, composable, named entries keyed by precondition (already the plan); add Voyager's **environment-feedback → self-correction** inner loop before a skill is admitted. *Fit:* `skill_forge/registry.rs` + `synthesis.rs`.
- **[Code-as-Monitor: Constraint-aware Visual Programming for Reactive and Proactive Robotic Failure Detection](https://arxiv.org/abs/2412.04455)** (CVPR 2025) — turns task constraints into **monitor code** that detects failure reactively and proactively. This is the missing piece of Phase 16's **self-verification gate**: rather than trust the model's self-report, compile the skill's success criterion into a checkable monitor (sensor/camera/test assertion). *Fit:* the "self-verification gate" bullet; composes with Track 0.
- **[Symbolic Learning Enables Self-Evolving Agents](https://arxiv.org/pdf/2406.18532)** (arXiv 2024.06) and **[Interactive Evolution / ENVISIONS](https://arxiv.org/pdf/2406.11736)** (arXiv 2024.06) — neural-symbolic self-training that optimizes an agent's own prompts/pipelines from trajectories. Maps to Phase 16's **"offline trace evolution (GEPA/DSPy-style)"** bullet. *Borrow:* batch job over accumulated `trajectory.rs` episodes that rewrites skill descriptions and retrieval keys; symbolic (text) gradients keep it interpretable and auditable — on-brand for OBC. *Fit:* `skill_forge/improve.rs`.
- **[Agent-Pro: Learning to Evolve via Policy-Level Reflection and Optimization](https://arxiv.org/abs/2402.17574)** (ACL 2024) — reflection at the **policy** level, not the single-action level. *Borrow:* have `reflexion.rs` reflect over a whole *episode/policy* (which skills to prefer in a context) rather than one tool call, feeding preferences back to retrieval ranking.
- **[AgentGym: Evolving LLM-based Agents across Diverse Environments](https://arxiv.org/pdf/2406.04151)** (arXiv 2024.06) — a platform-level view of evolving one agent across many environments/tasks. *Borrow (design principle):* keep the learned-skill library **environment-tagged** so a skill mined on one node/room is retrieved with the right generalization caution on another. Lightweight metadata, not a new subsystem.

**Verification is the through-line.** Every self-evolving-agent borrow is gated on a *concrete* verification signal (Code-as-Monitor / sensor / test), never intrinsic self-report — exactly what the Phase 16 bullet already stipulates, and what keeps synthesized *physical* skills safe under Track 0's staged rollout.

---

## 3. Phase 18 — World Memory & Dual-System ← *LLMs with World Models*

**Roadmap home:** Phase 18; **modules:** `src/memory/world.rs`, `src/mission/`, `src/agent/reflex.rs`, `src/agent/edge.rs`.

OBC already ships the substrate every one of these papers assumes it needs to build: a **bitemporal, queryable** world model. The borrows are about *what to write into it* and *how missions consume it*.

- **[Leveraging Pre-trained LLMs to Construct and Utilize World Models for Model-based Task Planning (LLM-DM)](https://openreview.net/forum?id=zDbsSscmuj)** (NeurIPS 2023) — the LLM emits an explicit, **checkable** (PDDL-style) transition model that a classical planner then uses; humans/tests correct the model, not the plan. *Highest-value borrow in this section.* *Proposal:* add a `world_model` synthesis step that reads current `world.rs` facts + the objective and emits a small symbolic transition model (preconditions/effects over `sensor.*`, `power.mode`, `nav.pose`, device states); the mission BT executes against it, and mismatches between predicted and observed effects become verification failures (ties into Phase 16). *Fit:* new `src/mission/model.rs`, consuming `memory/world.rs`.
- **[Reasoning with Language Model is Planning with World Model (RAP)](https://arxiv.org/pdf/2305.14992.pdf)** (arXiv 2023) — reuse the LLM as both policy and world model inside an MCTS-style deliberation. *Borrow (bounded):* for the deliberative System 2 tier only, simulate a few candidate mission expansions against the LLM-emitted model before committing — cheap look-ahead, no runtime cost on the reflex path.
- **[Modeling Dynamic Environments with Scene Graph Memory](https://openreview.net/attachment?id=NiUxS1cAI4&name=pdf)** (ICML 2023) and **[A Persistent Spatial Semantic Representation (HLSM)](https://openreview.net/pdf?id=NeGDZeyjcKa)** (CoRL 2021) — structured **scene-graph / spatial-semantic** memory with link prediction over partially-observed environments. *Borrow:* give `world.rs` an optional **relational/scene-graph view** (entity–relation edges: `device in room`, `subject near door`) on top of the flat `namespace.key` facts, so navigation and ClawCam subjects compose spatially. Aligns with the Subsystem-Suites plan to feed ClawCam subjects in as world-memory entities.
- **[Learning to Model the World with Language (Dynalang)](https://openreview.net/pdf?id=eWLOoaShEH)** and **[Robust agents learn causal world models](https://openreview.net/attachment?id=pOoKI3ouv1&name=pdf)** (ICLR 2024) — the theoretical backstop: agents that generalize/anticipate provably rely on (causal) world models. *Use:* cite as the **design justification** for OBC's foresight (Track 1) tier and the world-model synthesis above — anticipatory control is world-modeling, and the literature says that's what robustness requires. Reinforces the framing already in `SOTA-COMPARISON.md`.
- **[Do Embodied Agents Dream of Pixelated Sheep? (DECKARD)](https://openreview.net/attachment?id=Rm5Qi57C5I&name=pdf)** (ICML 2023) — LLM-guided abstract world model hypothesized then **verified by environment interaction**. Same verify-then-trust discipline; good pattern reference for the mission/model mismatch handling.

**Dual-system note.** The System 1/System 2 split itself is already covered in `SOTA-COMPARISON.md` (Talker-Reasoner, Helix). This section's contribution is the **memory content** the two systems share: a symbolic transition model + a scene-graph view, both written into the existing bitemporal store.

---

## 4. Phase 17 — Long-Horizon Autonomy ← *Open-world lifelong agents*

**Roadmap home:** Phase 17; **modules:** `src/scheduler/`, `src/memory/{heartbeat,journal,trajectory}.rs`, `src/agent/`.

Phase 17 is the "durable, resumable, self-verifying operation across hours/days" harness. The open-world Minecraft/ALFRED agents are the closest public analogues to a fleet running an unattended multi-hour routine.

- **[JARVIS-1: Open-world Multi-task Agents with Memory-Augmented Multimodal LMs](https://arxiv.org/abs/2311.05997)** (NeurIPS 2023) — **memory-augmented** long-horizon control with self-improvement over a growing memory. *Borrow:* Phase 17's "externalized world-state progress record" should be *retrieval-backed* (query `memory/` for how a similar objective resumed last time), not just a flat JSON status list. *Fit:* pairs `scheduler` durable state with `memory/vector.rs` retrieval.
- **[Describe, Explain, Plan and Select (DEPS)](https://arxiv.org/abs/2302.01560)** (NeurIPS 2023) — an interactive planner whose **self-explanation of failure** drives replanning, plus a goal **selector** ranking sub-goals by proximity/feasibility. *Borrow:* on resume-after-crash, before re-acting, generate a DEPS-style description+explanation of what failed, and use a selector to pick the cheapest outstanding objective. Directly strengthens the "resume smoke test" and "re-open failed objectives" bullets.
- **[Pre-emptive Action Revision by Environmental Feedback](https://openreview.net/pdf?id=cq2uB30uBM)** (CoRL 2024) and **[Multi-Modal Grounded Planning and Efficient Replanning (FLARE)](https://arxiv.org/pdf/2412.17288)** (AAAI 2025) — revise the *next* action from environment feedback rather than replanning from scratch; replan efficiently from a few examples. *Borrow:* the worker loop should prefer **local action revision** over full replan when a single objective's verification fails — cheaper and more stable across a long unattended run.
- **[SPRING: GPT-4 Out-performs RL by Studying Papers and Reasoning](https://arxiv.org/pdf/2305.15486.pdf)** (arXiv 2023) — reading the *manual/spec* to bootstrap a policy. *Borrow (light):* let the initializer ingest a node/routine's own README/config as context when establishing the objective record — the "study the manual first" pattern for cheap context re-establishment on resume.

---

## 5. Agent loop — grounding, reflection & planning primitives

**Roadmap home:** cross-cutting (Track 0, `src/agent/`, `src/mission/`, `src/tools/`); **modules:** `security/limits`, `agent/reflexion.rs`, `skill_forge/synthesis.rs`.

These are the foundational agent-methods papers in the *Planning/Manipulation* and *Others* clusters. Several map onto OBC primitives so cleanly they're almost validation of existing design.

- **[Do As I Can, Not As I Say: Grounding Language in Robotic Affordances (SayCan)](https://arxiv.org/pdf/2204.01691.pdf)** (arXiv 2022) — **the standout conceptual match.** SayCan scores each candidate action by `p(useful) × p(affordance/can-do)`. **OBC's Track 0 SafetyGate *is* the affordance term** — a deterministic `can this actuation happen here, now, within limits?` filter. *Proposal:* expose Track 0's feasibility verdict to the planner as an explicit **affordance score/mask** so the LLM never proposes actions the gate will reject — turning the gate from a late refusal into an early planning constraint. Cheap, safety-positive, and a clean story ("Track 0 = the *can*, the LLM = the *say*"). *Fit:* surface `security/limits` results into `agent/` tool selection.
- **[Code as Policies: Language Model Programs for Embodied Control](https://arxiv.org/pdf/2209.07753)** (2023) — skills as **generated, composable code** with reactive control flow. *Borrow:* Phase 16 synthesized skills should be representable as small **policy programs** (compose existing gated tools + conditionals) rather than only flat tool sequences — more expressive, still Track-0-bounded because every primitive is a gated tool.
- **[Inner Monologue: Embodied Reasoning through Planning with LMs](https://openreview.net/pdf?id=3R3Pz5i0tye)** (CoRL 2022) — inject success detection, scene, and human feedback back into the LLM as **closed-loop textual feedback**. *Borrow:* standardize the "observation → memory → next-step" feedback string the agent sees each tick, drawing from `world.rs`; pairs with Reflexion (`reflexion.rs`) already present.
- **[LLM-Planner: Few-Shot Grounded Planning](https://arxiv.org/pdf/2212.04088.pdf)** (ICCV 2023) — grounded, **re-planning-on-failure** few-shot planner. Reinforces the Phase 17 replan-locally borrow with a concrete few-shot recipe.
- **Reasoning scaffolds — [ReAct](https://arxiv.org/pdf/2210.03629.pdf), [Tree of Thoughts](https://arxiv.org/pdf/2305.10601.pdf), [Graph of Thoughts](https://arxiv.org/abs/2308.09687.pdf), [Least-to-Most](https://arxiv.org/pdf/2205.10625), [CoT](https://arxiv.org/pdf/2201.11903.pdf)** (Others cluster) — the reasoning-structure toolbox. *Use pragmatically:* ReAct is the baseline loop; reserve ToT/GoT deliberation for the **System 2** tier on genuinely novel objectives only (the reflex/foresight paths must stay cheap and deterministic). Document which scaffold each tier uses so latency budgets stay honest.

---

## 6. Navigation & VLN ← *Vision-Language Navigation + 3D Grounding*

**Roadmap home:** navigation column + Subsystem Suites; **modules:** `src/navigation/{exploration,costmap,planning,mapping}.rs`, `src/vision/clawcam_spatial.rs`.

`SOTA-COMPARISON.md` already scored the *geometric* nav stack (A*, particle filter, pose-graph SLAM) and named its gaps. This cluster adds the **semantic / language-grounded** layer on top.

- **[ESC: Exploration with Soft Commonsense Constraints for Zero-shot Object Navigation](https://openreview.net/attachment?id=GydFM0ZEXY&name=pdf)** (ICML 2023) and **[PONI: Potential Functions for ObjectGoal Navigation](https://openaccess.thecvf.com/content/CVPR2022/papers/Ramakrishnan_PONI_Potential_Functions_for_ObjectGoal_Navigation_With_Interaction-Free_Learning_CVPR_2022_paper.pdf)** (CVPR 2022) — bias frontier selection by **commonsense/semantic potential** ("a mug is likely near the kitchen"), not just nearest-distance. *Borrow:* this is the concrete upgrade for the `SOTA-COMPARISON.md` gap "information-gain / cost-utility frontier selection." Add a semantic potential term to `exploration.rs` frontier scoring, sourced from ClawCam detections / world-memory entities. *Fit:* `navigation/exploration.rs`.
- **[VoxPoser: Composable 3D Value Maps for Robotic Manipulation with LMs](https://arxiv.org/abs/2307.05973)** (arXiv 2023) — LLM composes **3D value/affordance maps** that a planner descends. *Borrow (2D):* OBC's costmap already has an inflation layer (per `SOTA-COMPARISON.md`); let the LLM add **semantic cost layers** ("avoid the nursery," "prefer lit hallways") as composable overlays on `costmap.rs`. Keeps planning classical and bounded, adds language-grounded preferences.
- **[NavGPT: Explicit Reasoning in Vision-and-Language Navigation with LLMs](https://arxiv.org/pdf/2305.16986.pdf)** and **[CANVAS: Commonsense-Aware Navigation](https://arxiv.org/abs/2410.01273)** (ICRA 2025) — LLM as an **explicit-reasoning navigator** that turns instructions into waypoints with interpretable justifications. *Borrow:* a `navigate_to("the room with the open window")` capability that resolves language goals to grid coordinates via world-memory + ClawCam, then hands off to the existing classical planner. Language *in*, classical execution *out* — the safe division of labor.
- **[RoboSpatial](https://arxiv.org/abs/2411.16537)** (CVPR 2025) and **[RoboRefer](https://arxiv.org/pdf/2506.04308)** (2025) — teaching/benchmarking **spatial referring** ("the object to the left of the sink"). *Borrow:* use as the **evaluation spec** for `vision/clawcam_spatial.rs` spatial tools — a ready-made rubric for whether OBC resolves spatial references correctly.
- **[3D-LLM](https://arxiv.org/abs/2307.12981)** / **[An Embodied Generalist Agent in 3D World (LEO)](https://arxiv.org/abs/2311.12871)** (ICML 2024) — inject 3D into the LLM. *Watch-list-adjacent:* relevant only if/when a node carries real depth/3D sensing; noted here so the costmap/scene-graph interfaces are shaped to accept a 3D upgrade later.

---

## 7. Fleet ← *Multi-Agent Learning and Coordination*

**Roadmap home:** `src/fleet/`; **modules:** `fleet/mod.rs`. `SOTA-COMPARISON.md` covers the *market/auction + mutex* mechanics vs Open-RMF; this cluster adds **LLM-mediated** coordination.

- **[Building Cooperative Embodied Agents Modularly with LLMs (Co-LLM-Agents)](https://openreview.net/forum?id=EnXJfQqy0K)** (ICLR 2024) — a **modular** framework (perception/memory/comms/planning/execution) where embodied agents cooperate via natural-language messages, beating planner baselines and improving human-AI cooperation. *Borrow:* add an optional **language-comms channel** between fleet nodes for *explanation and intent-sharing* (not for safety-critical control, which stays on the deterministic auction/mutex path). Improves human legibility of why the fleet did what it did. *Fit:* `fleet/mod.rs` heartbeat payload + a comms budget.
- **[Communication-Efficient Desire Alignment for Embodied Agent-Human Adaptation](https://arxiv.org/abs/2505.22503)** (ACL 2026, Oral) — align on shared goals with **minimal communication**. *Borrow:* a **communication budget** on the language-comms channel above (say only what changes the plan), mirroring the escalation-budget pattern OBC already uses for System 1 → System 2 hand-offs. Same discipline, new channel.
- **[Adaptive Coordination in Social Embodied Rearrangement](https://openreview.net/attachment?id=BYEsw113sz&name=pdf)** (ICML 2023) — coordinating with **unseen partners** without prior joint training. *Design principle:* the coordinator should degrade gracefully when a node it hasn't coordinated with before joins the fleet — relevant to the "new node joins mid-routine" case in Phases 17/Hardware-Expansion.
- **[MetaGPT: Meta-Programming for a Multi-Agent Collaborative Framework](https://openreview.net/forum?id=VtmBAGCN7o)** (ICLR 2024, oral) — **role/SOP-structured** multi-agent collaboration. *Borrow (light):* if OBC ever runs multiple *reasoning* agents (not just robots), MetaGPT's role+SOP structure is the reference; today's single-brain/many-bodies model doesn't need it, so noted as a scaling reference only.

---

## 8. Learning tiers ← *Reward design & RL-assist*

**Roadmap home:** `src/learning/`, `src/foresight/`; **modules:** `learning/mod.rs`, `foresight/mod.rs`.

OBC deliberately uses **rule mining over telemetry**, not RL (`SOTA-COMPARISON.md` §Self-authored rules). These papers are borrowed as **scoring/synthesis tools**, *not* as an invitation to train policies — that would cut against the interpretable-System-1 choice.

- **[Eureka: Human-Level Reward Design via Coding LLMs](https://eureka-research.github.io/assets/eureka_paper.pdf)** (NeurIPS 2023 WS) and **[Text2Reward: Dense Reward Generation with LMs](https://openreview.net/pdf?id=tUM39YTRxH)** (ICLR 2024) — LLMs author and **reflectively refine** reward/scoring functions as code. *Borrow:* use this pattern to have the LLM propose and refine the **utility/priority functions** OBC already needs — frontier information-gain scoring (§6), fleet task-allocation cost, foresight rule ranking — as auditable code reviewed before use. Reward *shaping-as-code*, not policy learning.
- **[Language Reward Modulation / LAMP](https://arxiv.org/pdf/2308.12270.pdf)** and **[Guiding Pretraining in RL with LLMs (ELLM)](https://openreview.net/attachment?id=63704LH4v5&name=pdf)** (ICML 2023) — LLM-suggested objectives to guide exploration. *Borrow (design):* when a node explores autonomously, let the brain suggest **which regions/objectives are worth exploring** (semantic guidance), executed through the classical frontier planner — the same language-in/classical-out split as §6.
- **[Online Continual Learning for Interactive Instruction Following Agents](https://openreview.net/pdf?id=7M0EzjugaN)** (ICLR 2024) — continual learning **without catastrophic forgetting**. *Relevance:* the self-improvement library (Phase 16) must not let a newly-mined skill overwrite a working one; cite as the discipline for versioned, non-destructive skill updates (mirrors world-memory's non-destructive correction principle).

---

## 9. Phases 19–20 — Real-time multimodal & edge-native ← *Efficient VLA*

**Roadmap home:** Phases 19–20; **modules:** `src/channels/`, `src/audio/`, `src/multimodal.rs`, `src/agent/edge.rs`, `src/providers/`.

The list's newest cluster (the maintainers' own **[Survey on Efficient VLA Models](https://arxiv.org/abs/2510.24795)**, 2025.10) is the reference for making perception-action cheap enough to run at the edge — exactly Phase 20's mandate.

- **[Distilling Internet-Scale Vision-Language Models into Embodied Agents](https://openreview.net/pdf?id=6vVkGnEpP7)** (ICML 2023) — distill a big VLM's knowledge into a **small on-device** embodied policy. *Borrow (design):* the Phase 20 "small-model reflex tier" should be framed as **distillation targets** — the cloud System 2 supervises/labels, the edge System 1 model distills. Sets up the local↔cloud fallback as a teacher-student relationship.
- **[FAST: Efficient Action Tokenization for VLA Models](https://arxiv.org/abs/2501.09747)** (2025.01) and the **Efficient-VLA survey** — compression/tokenization/efficient-inference techniques. *Use:* the survey's taxonomy (efficient architecture · efficient training · efficient data) is the **checklist** for evaluating any on-device model OBC provisions per node role (Phase 20 "edge model management"). Not code to import — a rubric to shop by.
- **[RILA: Reflective and Imaginative Language Agent for Zero-Shot Semantic Audio-Visual Navigation](https://peihaochen.github.io/files/publications/RILA.pdf)** (CVPR 2024) and **[MP5: Multi-modal Open-ended Embodied System via Active Perception](https://arxiv.org/pdf/2312.07472.pdf)** (CVPR 2024) — **active** multimodal perception (decide *what to sense next*), audio-visual grounding. *Borrow:* Phase 19's continuous-vision session should support **active perception** — the agent asks a node to look/listen *toward* something under discussion, rather than passively streaming. Pairs the mic/camera nodes with a "sense-on-demand" tool, Track-0-bounded.

---

## 10. Evaluation ← *Benchmarks & Simulators*

**Roadmap home:** Phase 15 evaluation harness (CC/CD) + Subsystem-Suite evals; **modules:** `tests/`, `tests/evals`.

Phase 15 calls for an "evaluation harness (CC/CD)" but doesn't yet specify *what to measure against*. This cluster is a ready-made menu of **text-grounded** benchmarks that run without a physics sim — ideal for CI.

- **[ALFWorld: Aligning Text and Embodied Environments](https://alfworld.github.io/)** (ICLR 2021) — the **text mirror** of ALFRED. Because it's text-based, an ALFWorld-style task suite can run in CI to regression-test the *planning/reflection* loop (does the agent decompose "put a clean mug in the coffee machine" correctly?) with zero hardware. *Highest-value eval borrow.* *Fit:* `tests/evals` as a synthetic task set.
- **[SQA3D: Situated Question Answering in 3D Scenes](https://arxiv.org/pdf/2210.07474.pdf)** (ICLR 2023) and **[NavSpace](https://arxiv.org/abs/2510.08173)** (ICRA 2026) — **situated/spatial reasoning** benchmarks ("from where I am, what's behind me?"; following spatial-intelligence instructions). *Borrow:* adapt as offline evals for the spatial-referring tools (§6) and world-memory queries — a concrete pass/fail for "does OBC reason correctly about *situated* space."
- **[ALFRED](https://arxiv.org/pdf/1912.01734.pdf)** (CVPR 2020) / **[ReALFRED](https://arxiv.org/pdf/2407.18550)** (ECCV 2024) — the photo-realistic instruction-following benchmark and its realistic successor. *Use:* the north-star task taxonomy (navigate + interact + long-horizon) to shape OBC's own routine definitions in the long-horizon eval (Phase 17).
- **Simulators — [Habitat](https://aihabitat.org/)/[Habitat 2.0](https://arxiv.org/pdf/2106.14405), [AI2-THOR](https://arxiv.org/abs/1712.05474), [iGibson](https://svl.stanford.edu/igibson/), [LEGENT](https://arxiv.org/pdf/2404.18243), [UnrealZoo](https://arxiv.org/abs/2412.20977)** — if OBC ever wants a **software-in-the-loop** rehearsal environment for the fleet brain (before touching real actuators — the Track 0 `simulate` stage), these are the standard options. *Recommendation:* keep sim **optional and out-of-core**; a small AI2-THOR/Habitat harness could back Track 0's `simulate → supervised → autonomous` first stage for navigation skills without adding a runtime dependency.
- **[RoboGen](https://arxiv.org/pdf/2311.01455.pdf)** (2023) and **[Learning Interactive Real-World Simulators (UniSim)](https://universal-simulator.github.io/unisim/)** (ICLR 2024) — **generative** simulation / world models that synthesize training scenarios. *Watch-list:* aligns with the roadmap's own "world-model-generated synthetic scenarios for offline skill rehearsal" note — see §11.

---

## 11. Watch-list (explicitly out of scope)

These are genuinely interesting but would **forfeit the embodied moat** or contradict a stated non-goal. Listed so the decision is deliberate, matching the roadmap's own "Beyond v2.0" boundary.

- **On-device motor-control VLAs — [π0](https://arxiv.org/abs/2410.24164), [π0.5](https://www.physicalintelligence.company/download/pi05.pdf), [OpenVLA](https://arxiv.org/pdf/2406.09246), [RT-2](https://robotics-transformer2.github.io/assets/rt2.pdf), [Hi Robot](https://arxiv.org/pdf/2502.19417), [Embodied-CoT](https://openreview.net/pdf?id=S70MgnIA0v).** OBC's non-goal is explicit: *"building our own motor-control VLA."* Correct posture (already in the roadmap): integrate a VLA as a **peripheral capability** only once a node has a real manipulator, and treat it as a gated tool behind Track 0 — never as OBC's core. These papers are the shopping list for *that* future integration, not for the core runtime.
- **Generative world-model simulators — [UniSim](https://universal-simulator.github.io/unisim/), [RoboGen](https://arxiv.org/pdf/2311.01455.pdf), MineDreamer.** Promising for offline skill rehearsal (Track 0 `simulate` stage), but heavyweight; keep on the watch-list exactly as the roadmap states.
- **General computer-use / game agents — [CRADLE](https://arxiv.org/pdf/2403.03186.pdf), [Mobile-Agent-v2](https://arxiv.org/pdf/2406.01014), [CombatVLA](https://arxiv.org/abs/2503.09527), [Voyager (Minecraft specifics)](https://voyager.minedojo.org/).** Borrow the *methods* (skill libraries, multi-agent nav) as done above, but **not** the domain — broadening into a general software-agent runtime is an explicit non-goal.

---

## 12. Prioritized backlog

Tiered by leverage ÷ cost. **P1** = near-term, high leverage, safety-positive. **P2** = valuable, moderate cost. **P3** = design-shaping / later. Watch-list items are excluded by definition.

| # | Enhancement | Source paper(s) | OBC target | Tier |
|---|---|---|---|---|
| 1 | Track 0 feasibility exposed to planner as **affordance mask** | SayCan | `security/limits` → `agent/` | **P1** |
| 2 | **Verifier-anchored** skill synthesis (compile success criterion to a monitor) | Voyager, Code-as-Monitor | `skill_forge/synthesis.rs` (Phase 16) | **P1** |
| 3 | **Symbolic transition model** synthesized over world memory for missions | LLM-DM, RAP | `mission/` + `memory/world.rs` (Phase 18) | **P1** |
| 4 | **Commonsense/semantic frontier** scoring | ESC, PONI | `navigation/exploration.rs` | **P1** |
| 5 | **ALFWorld-style text eval suite** in CI | ALFWorld, SQA3D | `tests/evals` (Phase 15) | **P1** |
| 6 | Offline **trace evolution** of skill descriptions (symbolic gradients) | Symbolic Learning, ENVISIONS, Agent-Pro | `skill_forge/improve.rs` (Phase 16) | **P2** |
| 7 | **Scene-graph view** over the flat world-memory facts | Scene Graph Memory, HLSM | `memory/world.rs` (Phase 18) | **P2** |
| 8 | **Retrieval-backed resume** + DEPS self-explain replanning | JARVIS-1, DEPS, FLARE | `scheduler/` + `memory/` (Phase 17) | **P2** |
| 9 | **Semantic cost layers** on the costmap (language preferences) | VoxPoser | `navigation/costmap.rs` | **P2** |
| 10 | Language-goal `navigate_to("…")` resolving to grid coords | NavGPT, CANVAS | `navigation/` + `vision/` | **P2** |
| 11 | **Language-comms channel** + comms budget between fleet nodes | Co-LLM-Agents, Desire-Alignment | `fleet/mod.rs` | **P2** |
| 12 | Skills as **policy programs** (code) not just tool sequences | Code-as-Policies | `skill_forge/` | **P2** |
| 13 | LLM-authored **utility/scoring functions** (reward-as-code), reviewed before use | Eureka, Text2Reward | `learning/`, `foresight/` | **P3** |
| 14 | **Active perception** ("sense toward X") in live sessions | RILA, MP5 | `multimodal.rs`, `audio/` (Phase 19) | **P3** |
| 15 | Edge System 1 as **distillation target** of cloud System 2 | Distilling VLMs, Efficient-VLA survey | `agent/edge.rs` (Phase 20) | **P3** |
| 16 | Spatial-referring **eval rubric** for ClawCam | RoboSpatial, RoboRefer, NavSpace | `vision/clawcam_spatial.rs` | **P3** |
| 17 | Optional **sim-in-the-loop** for Track 0 `simulate` stage | Habitat, AI2-THOR | out-of-core harness | **P3** |

**Suggested first cut:** items **1–5** (all P1) are individually small, mutually reinforcing, and each *strengthens* the safety/interpretability story rather than diluting it — #1 makes the gate a planning input, #2 makes learned skills provable, #3 makes missions checkable, #4/#5 close named gaps from `SOTA-COMPARISON.md` and Phase 15. They are folded into the ROADMAP as sub-bullets under Phases 15–18.

---

*Sources: every bracketed link resolves to an entry in [Awesome-Embodied-Robotics-and-Agent](https://github.com/zchoi/Awesome-Embodied-Agent-with-LLMs). Companion analysis of the classical-robotics layers: [`SOTA-COMPARISON.md`](SOTA-COMPARISON.md). Architecture: [`EMBODIED-ARCHITECTURE.md`](EMBODIED-ARCHITECTURE.md).*

# Oh-Ben-Claw vs. State of the Art — a grounded comparison

*Compiled 2026-06-25 from current literature and project docs. Each row sanity-checks an OBC design choice against the dominant SOTA family for that component. Verdict up front: OBC's **architecture** sits squarely in the SOTA families across the board; its **implementations** are deliberately simplified, interpretable baselines of each. The genuine differentiators are the unified substrate, the uniform shield, and the anticipatory layer — not the individual algorithms.*

## Component-by-component

### Navigation & planning
- **SOTA — ROS 2 Nav2:** a modular set of servers; global planners include NavFn (Dijkstra) and **SMAC Hybrid‑A\*** (kinematically feasible); local control uses **DWB / MPPI** controllers that replan within milliseconds; a **layered costmap** (static / obstacle / voxel / **inflation**) assigns graded cost; the whole mission is a **behavior tree** (`plan → follow → recover → retry`). ([Nav2 concepts](https://docs.nav2.org/concepts/index.html), [BT walkthrough](https://navigation.ros.org/behavior_trees/overview/detailed_behavior_tree_walkthrough.html))
- **OBC:** A\* over a binary occupancy grid → simplified turn-point waypoints → a proportional steer/drive follower, all Track‑0‑bounded.
- **Verdict:** Same family as NavFn-style global planning; **honest gaps** vs Nav2 — no **inflation layer / cost gradient** (we treat cells as free/occupied with no safety margin), no **kinodynamic feasibility** (Hybrid‑A\*), and no dynamic-obstacle **local controller** (DWB/MPPI). Our A\* + waypoint follower is the correct *baseline*; the safety-margin and local-replanning pieces are the real missing maturity.

### SLAM
- **SOTA — slam_toolbox / Cartographer:** both are **pose-graph** 2D SLAM. slam_toolbox builds on Karto's well-regarded scan matcher with **Sparse Pose Adjustment**; Cartographer pairs a scan-matching front end with a **Ceres** least-squares back end and submap-matching loop closure. A 2025 comparison found slam_toolbox ATE **0.13 m** vs Cartographer **0.21 m** in dynamic scenes. ([slam_toolbox](https://github.com/SteveMacenski/slam_toolbox), [JOSS paper](https://joss.theoj.org/papers/10.21105/joss.02783.pdf), [MDPI comparison](https://www.mdpi.com/2079-9292/14/24/4822))
- **OBC:** 2D SE2 **pose graph** with odometry + loop-closure edges, **anchored Gauss‑Seidel relaxation**, proximity-based loop-closure proposal.
- **Verdict:** Architecturally the **same approach** (pose graph + loop closure is exactly the SOTA family). **Honest gaps** — relaxation is a weaker optimizer than sparse least-squares (Ceres / SPA / g2o); and our loop closure is **spatial-proximity** rather than **scan/feature matching** (we flagged this as a stand-in for a real place-recognition front end). The bones are right; the front end and solver are simplified.

### Localization (belief state)
- **SOTA — Nav2 AMCL:** an **adaptive (KLD-sampling) Monte Carlo** particle filter that varies particle count and uses a laser sensor model against a known map. ([nav2_amcl](https://github.com/ros-navigation/navigation2/blob/main/nav2_amcl/README.md))
- **OBC:** a fixed-N particle filter — noisy motion proposal, Gaussian position-likelihood update, low-variance resampling, weighted estimate **with spread**.
- **Verdict:** **Same family** as AMCL. Gaps — **non-adaptive** particle count (no KLD), and a position-fix measurement model rather than a beam/likelihood-field laser model. Reporting an explicit **spread/uncertainty** is good practice and aligns with the probabilistic intent.

### Task-level control (missions vs behavior trees)
- **SOTA — Behavior Trees:** the dominant interpretable representation for reactive task-level control; **BehaviorTree.CPP** is the de-facto ROS standard (Nav2's BT Navigator uses it). A key BT virtue is **reactivity** — conditions are re-checked every tick; notably py_trees' "Sequence with memory" *loses* that reactivity. ([BehaviorTree.CPP](https://www.behaviortree.dev/), [BT survey](https://www.sciencedirect.com/science/article/pii/S0921889022000513), [On the Implementation of BTs](https://arxiv.org/pdf/2106.15227))
- **OBC:** a **linear** mission sequence with **guards re-checked every tick** that preempt-and-halt.
- **Verdict:** Our mission runner is effectively a **reactive Sequence with condition guards** — a strict subset of a BT, and it *does* honor the reactivity principle (guards every tick). **Honest gap:** no full BT grammar (fallback/selector, parallel, decorators, subtrees). Upgrading missions to a real BT (BehaviorTree.CPP-style) is the clean alignment step, and matches the option we sketched earlier.

### Reactive safety (Track 0 / safing)
- **SOTA — runtime shields / monitors:** a **shield** is a runtime monitor, synthesized offline, that **disallows actions violating a safety property**, treating the ML controller as a **black box** — cheaper than verifying the whole system. Monitors trigger reactive recovery on violation. Related: control barrier functions, runtime verification for ROS. ([Dynamic shielding](https://arxiv.org/pdf/2505.22104), [REDriver runtime enforcement](https://arxiv.org/pdf/2401.02253), [Runtime verification for ROS](https://arxiv.org/html/2404.11498v1))
- **OBC:** the **Track 0 SafetyGate** — a deterministic per-call check (pins/range/rate) that refuses unsafe actuation, host **and** on-MCU; plus reflex **safing** rules that trigger recovery.
- **Verdict:** **Strong alignment.** Track 0 *is* a shield — a black-box runtime enforcer bounding every actuation — and reflex safing matches monitor-triggered recovery. This is arguably OBC's most SOTA-faithful layer. Strengthening it would mean **formal synthesis** of the gate or **control barrier functions** for provable guarantees rather than hand-set bounds.

### Foresight (anticipatory layer)
- **SOTA — predictive maintenance / failure forecasting:** a mature field; multivariate time-series anomaly/failure prediction typically uses ML (e.g. **CNN‑LSTM + quantile regression**) over historical sensor data to anticipate faults within a future window. ([Predicting machine failures](https://www.mdpi.com/2075-1702/12/6/357), [robot health-monitoring review](https://pmc.ncbi.nlm.nih.gov/articles/PMC13061873/))
- **OBC:** a **linear-trend** forecaster (`time_to_threshold`) feeding predictive rules that fire before a crossing.
- **Verdict:** The **forecasting method is a baseline** (linear regression vs LSTM). The interesting part isn't the predictor — it's the **architectural placement**: predictions become a first-class **anticipatory control tier** ("Track 1") over the shared temporal memory, dispatching through the same rule/sink machinery as reflexes. That framing is less common than the predictor itself, which is well-trodden. Swapping in an ML forecaster is a drop-in upgrade.

### Self-authored rules
- **SOTA — automatic BT / rule synthesis:** active research in **learning BTs from demonstration**, **action-condition learning** from human demos, and planning/RL/evolutionary BT generation. ([Learning BTs from Demonstration](https://www.researchgate.net/publication/335143696_Learning_Behavior_Trees_From_Demonstration), [Action conditions for BT generation](https://dl.acm.org/doi/10.1145/3610978.3640673))
- **OBC:** **association-rule mining** of antecedents from telemetry history (not demonstration), with **support/confidence** and a **human approval gate** before activation.
- **Verdict:** Same goal (synthesize control rules automatically), different input — ours mines **operational telemetry** rather than demonstrations, which is simpler and complementary. The **approval gate** before any rule goes live is a sound safety practice the LfD literature also emphasizes.

### Dual-system control (System 1 / System 2)
- **SOTA:** the **dominant** embodied-AI pattern — fast reactive **System 1** + slow deliberative **System 2** (Kahneman framing). Real systems: Talker-Reasoner, **Figure Helix** (fast visuomotor S1 + general planner S2), dual-system **VLA** models. S1 is typically a **learned visuomotor policy**. ([Talker-Reasoner](https://arxiv.org/html/2410.08328v1), [Helix dual-system](https://medium.com/@raktims2210/dual-system-ai-for-embodied-intelligence-how-vision-language-action-models-will-power-the-future-abfe923a779f), [Agentic LLM survey](https://arxiv.org/pdf/2503.23037))
- **OBC:** reflexes (S1) + the LLM agent (S2), **plus** foresight (anticipatory) and missions (deliberative sequencing).
- **Verdict:** OBC matches the prevailing pattern, and arguably extends it with a distinct **anticipatory** mode. **Key trade-off:** OBC's System 1 is **rule-based and deterministic** (interpretable, auditable, formally bound-able) rather than a learned policy — safer and more transparent, but **far less general/dexterous** than a learned visuomotor controller. That is a defensible choice for a safety-first embodied agent, not a deficiency to hide.

### Fleet coordination
- **SOTA — Open-RMF:** the open backbone for **multi-fleet interoperability**; handles **task allocation** and **conflict resolution**, with **Mutex Groups** — virtual "locks" assigning routes/locations to a single robot at a time (air-traffic-control style). ([Open-RMF](https://www.openrobotics.org/blog/2021/10/21/common-language-interop), [Mutex Groups milestone](https://jobtorob.com/open-rmf-hits-key-milestones-in-enabling-multifleet-robot-interoperability))
- **OBC:** a coordinator with nearest-idle-with-battery **allocation**, **claim-based min-separation** (single-occupancy spatial locks), and MQTT heartbeats.
- **Verdict:** Our claims are essentially **lightweight Mutex Groups**, and allocation + conflict resolution mirrors RMF's core. **Honest gaps:** no traffic-lane / lift / door integration, no multi-fleet interop standard. Ours is a tractable in-house coordinator; RMF is the heavyweight interoperability standard.

### Frontier exploration
- **SOTA:** Yamauchi's **frontier** (free/unknown boundary) remains the foundation; modern multi-robot work spans **nearest-frontier**, **information-gain**, and **cost-utility** heuristics, up to RL-driven decentralized coordination. ([Frontier exploration](https://arxiv.org/pdf/1806.03581), [RL multi-robot exploration](https://arxiv.org/abs/2412.20049), [cooperative exploration survey](https://www.frontiersin.org/journals/neurorobotics/articles/10.3389/fnbot.2023.1179033/full))
- **OBC:** classic **nearest reachable frontier** per node, with separation to keep robots apart.
- **Verdict:** A faithful implementation of the **nearest-frontier** SOTA baseline with simple multi-robot deconfliction. Information-gain / cost-utility frontier selection would be the natural upgrade.

## Bottom line

**Where OBC is genuinely SOTA-aligned or differentiated:**
1. **One bitemporal world-memory substrate** couples every layer. Most stacks (Nav2/ROS) couple via topics + `tf`, not a *queryable temporal store* — OBC's choice is what makes foresight, self-authored rules, and cross-layer reasoning natural. This is the strongest architectural differentiator.
2. **A uniform Track‑0 shield across host *and* firmware** — the safety enforcement is the same shield concept the literature endorses, applied at both tiers (defense in depth).
3. **A first-class anticipatory ("Track 1") tier** sitting between reactive and deliberative — less common than the dual-system norm.
4. **Interpretable, deterministic System 1** (rules, not learned policies) — a deliberate safety/auditability trade-off.

**The honest gaps vs SOTA (priority order if pursuing parity):**
1. Costmap **inflation / safety margins** + a real local controller (DWB/MPPI-style) — the most impactful navigation gap.
2. SLAM **least-squares solver** (Ceres/g2o/SPA) + **scan-matching** loop closure instead of relaxation + proximity.
3. **Full behavior-tree grammar** for missions (fallback/parallel/decorators) — BehaviorTree.CPP is the standard to match.
4. **Adaptive (KLD)** particle count + a proper laser sensor model.
5. ML **forecaster** behind foresight; **information-gain** frontier selection.

None of these are architectural rewrites — each is a swap-in upgrade within a layer whose *interfaces* are already SOTA-shaped. The design choices hold up against the literature; the work that remains is depth within components, not a change of architecture.

## Sources
- [Nav2 concepts](https://docs.nav2.org/concepts/index.html) · [Nav2 BT walkthrough](https://navigation.ros.org/behavior_trees/overview/detailed_behavior_tree_walkthrough.html)
- [slam_toolbox (GitHub)](https://github.com/SteveMacenski/slam_toolbox) · [SLAM Toolbox JOSS paper](https://joss.theoj.org/papers/10.21105/joss.02783.pdf) · [slam_toolbox vs Cartographer (MDPI 2025)](https://www.mdpi.com/2079-9292/14/24/4822)
- [nav2_amcl README](https://github.com/ros-navigation/navigation2/blob/main/nav2_amcl/README.md)
- [BehaviorTree.CPP](https://www.behaviortree.dev/) · [py_trees](https://py-trees.readthedocs.io/en/devel/introduction.html) · [On the Implementation of BTs (arXiv)](https://arxiv.org/pdf/2106.15227) · [Survey of BTs (ScienceDirect)](https://www.sciencedirect.com/science/article/pii/S0921889022000513)
- [Open-RMF (Open Robotics)](https://www.openrobotics.org/blog/2021/10/21/common-language-interop) · [Open-RMF Mutex Groups](https://jobtorob.com/open-rmf-hits-key-milestones-in-enabling-multifleet-robot-interoperability) · [RMF (GitHub)](https://github.com/open-rmf/rmf)
- [Frontier exploration (Topiwala)](https://arxiv.org/pdf/1806.03581) · [RL multi-robot exploration (arXiv 2024)](https://arxiv.org/abs/2412.20049) · [Cooperative exploration via task allocation](https://www.frontiersin.org/journals/neurorobotics/articles/10.3389/fnbot.2023.1179033/full)
- [Predicting machine failures (MDPI)](https://www.mdpi.com/2075-1702/12/6/357) · [Robot health-monitoring review (PMC)](https://pmc.ncbi.nlm.nih.gov/articles/PMC13061873/)
- [Dynamic shielding (arXiv)](https://arxiv.org/pdf/2505.22104) · [REDriver runtime enforcement (arXiv)](https://arxiv.org/pdf/2401.02253) · [Runtime verification for ROS (arXiv)](https://arxiv.org/html/2404.11498v1)
- [Talker-Reasoner (arXiv)](https://arxiv.org/html/2410.08328v1) · [Dual-system VLA / Helix](https://medium.com/@raktims2210/dual-system-ai-for-embodied-intelligence-how-vision-language-action-models-will-power-the-future-abfe923a779f) · [Agentic LLM survey (arXiv)](https://arxiv.org/pdf/2503.23037)
- [Learning BTs from Demonstration](https://www.researchgate.net/publication/335143696_Learning_Behavior_Trees_From_Demonstration) · [Action conditions for BT generation (ACM)](https://dl.acm.org/doi/10.1145/3610978.3640673)

# Fly connectomics → OBC: research through building

Date: 2026-09-12. Companion to `RESEARCH-2026-07.md` and `MEMORY-2026-07.md`.
Status: proposal. Nothing here is wired yet.

## 0. The one-paragraph version

The male fly CNS connectome (Cell, 2026-09-03) is the first complete brain-plus-cord wiring diagram, and it sits in the same neuPrint/Neuroglancer stack you already have forks of. But the honest finding of this survey is that **the graph itself is not the prize for AI**: two 2025–26 papers show a connectome alone underdetermines the dynamics it produces, and claimed advantages of "connectome-shaped" networks mostly vanish under fair controls. What *does* transfer are four circuit principles the connectome has now nailed down at cell resolution — sparse-expansion memory, compartmentalized valence learning, ring-attractor heading, and population-coded descending commands over local reflexes — and the first three of the four OBC seams you named have a concrete, measurable version of one of them. The recommended first feature is a mushroom-body-style index inside `obc-memory`, because it is pure Rust, ships in the crate that is already public, needs no new hardware, and fills a gap nobody has published on: applying the fly's memory circuit to an LLM agent's memory. Building it *is* the research if we design the measurements first.

## 1. What the fly gives us (state of the field, Sept 2026)

**The data.** Male CNS v1.0: 166,700 neurons, 11,710 types, central brain + optic lobes + VNC with an intact neck connective, CC-BY, served in neuPrint ([Cell](https://www.cell.com/cell/fulltext/S0092-8674(26)00942-6), [Janelia](https://www.janelia.org/project-team/flyem/male-cns-connectome)). The female equivalent (BANC, Nature, June 2026) is ~188k neurons ([Nature](https://www.nature.com/articles/s41586-026-10735-w)). Both are the first datasets where a sensory input can be traced to a motor output through the "spinal cord."

**Can it be run?** Yes, on a desktop. Shiu et al. 2024 simulated 127k FlyWire neurons as leaky-integrate-and-fire units in Brian2, one free parameter (0.275 mV/synapse), and got 91% of 164 predictions right — including recovering the antennal-grooming circuit purely from sensory input and descending output ([Nature](https://www.nature.com/articles/s41586-024-07763-9), [code, MIT](https://github.com/philshiu/Drosophila_brain_model)). Cost: ~4.4 s wall-clock per simulated second on CPU; NumPy reimplementations run a 1 s trial in seconds on a laptop ([flypoke](https://github.com/vshapenko/flypoke)). Sandia ran the whole thing on 12 Loihi 2 chips at 12–54 ms per simulated second, faster than real time at sparse activity ([arXiv 2508.16792](https://arxiv.org/abs/2508.16792)). Someone has already loaded the *male CNS* into the Shiu constants ([unreviewed repo](https://github.com/TheMrRaGe/flybrain)).

**Does the wiring carry the computation?** Only partly. Beiran & Litwin-Kumar (Nat. Neurosci., Dec 2025) show a connectome "often does not substantially constrain" recurrent dynamics until you add recordings from even a small subset of neurons ([Nature Neuro](https://www.nature.com/articles/s41593-025-02080-4)). BrainTrace (Nat. Commun., Jan 2026) does exactly that — low-rank weight adaptation over the connectome scaffold fitted to calcium activity ([paper](https://www.nature.com/articles/s41467-026-68453-w), [code](https://github.com/chaobrain/fitting_drosophila_whole_brain_spiking_model)). On the ML side, FlyGM (Feb 2026) and FLYNN (June 2026) use the whole connectome as the topology of an RL controller and report sample-efficiency and robustness gains ([FlyGM](https://arxiv.org/abs/2602.17997), [FLYNN](https://arxiv.org/abs/2607.00025)) — but a control study (Apr 2026) finds such topology advantages "largely disappear under fair from-scratch initialization and degree-preserving controls" ([arXiv 2604.04033](https://arxiv.org/abs/2604.04033)). Shiu's own limitations list is long: identical neurons, uniform weights, no neuromodulation, no plasticity.

**Implication for OBC.** Copying the graph into a network and hoping is not a strategy the literature supports. Extracting a circuit *motif*, implementing it small, and measuring whether its known properties (sparsity → low interference; compartments → local credit assignment; ring → drift-bounded heading) show up in OBC's actual workload — that is supported, and it's cheap.

## 2. Seam by seam

### 2.1 Memory substrate ← mushroom body

**Biology.** ~50 projection neurons fan out through a sparse random binary matrix onto ~2,000 Kenyon cells (40× expansion, ~6 inputs each); a single inhibitory neuron (APL) silences all but the top ~5%. The surviving set is the input's tag. Downstream, 15 compartments each get their own dopamine neuron that writes valence onto KC→output synapses *only in that compartment*, with timing relative to the stimulus deciding whether a memory is written, weakened, or flipped ([Aso 2014](https://elifesciences.org/articles/04577), [Aso & Rubin 2016](https://elifesciences.org/articles/16135), [hemibrain MB connectome](https://elifesciences.org/articles/62576)). A 2025 preprint finds the "random" projection is actually biased up to 15× by input type — worth remembering before assuming pure randomness ([bioRxiv](https://www.biorxiv.org/content/10.1101/2025.10.29.684686v2)).

**What AI already took from it.** FlyHash (Dasgupta et al. 2017, Science): sparse binary projection + winner-take-all is a locality-sensitive hash that beat classical LSH most at short code lengths, at ~20× lower projection cost ([Science](https://www.science.org/doi/10.1126/science.aam9868)). The same circuit is a distance- and time-sensitive Bloom filter for **novelty detection** — novelty decays with similarity to the past and recovers with elapsed time ([PNAS 2018](https://www.pnas.org/doi/10.1073/pnas.1814448115)). FlyModel (Shen et al. 2023): sparse codes + freezing every weight except those from the currently active KCs to the current class reduces catastrophic forgetting in online class-incremental learning ([Neural Comp.](https://direct.mit.edu/neco/article/35/11/1797/117579)). BioHash learns the projection with a local plasticity rule instead of random ([ICML 2020](https://arxiv.org/abs/2001.04907)). BioVSS (Dec 2024) turns it into a vector-set index with >50× speedup at ~99% recall on million-scale data ([arXiv](https://arxiv.org/abs/2412.03301)).

**The gap.** No published work applies any of this to LLM agent memory or RAG. Agent-memory surveys don't cite the fly. FlyHash is also absent from ANN-Benchmarks — it does not compete with HNSW on raw recall/QPS, and shouldn't be sold that way. Its value is the *properties*: one-shot online insert, data-independent, cheap, built-in novelty signal, interference-resistant online classification.

**What OBC could build.** A `MushroomBody` index in `obc-memory`: expand each embedding (from the local Ollama embed model) into a sparse binary tag; keep a decaying Bloom-style novelty filter over tags; maintain a small set of "compartments" (valence channels — *useful / not useful / harmful*, matching the conscience layer's vocabulary) trained with FlyModel-style partial freezing from feedback events. That gives the agent loop three things it doesn't have: a cheap "have I seen something like this before, and how recently?" signal per perception event and per recall; a forgetting-resistant online classifier of memory usefulness; and a sub-millisecond pre-filter before the existing vector search.

**Measurements that make it research.** (1) Novelty: feed the ClawCam detection stream and a replayed conversation log; measure whether the novelty score separates first-occurrence from repeat events, and how the time-decay parameter trades false alarms vs misses. (2) Interference: online-train the usefulness compartment on a sequence of memory-feedback events; compare forgetting curves against a plain perceptron and against logistic regression on the dense embedding — FlyModel's claim is that partial freezing wins; check it on *our* data. (3) Prefilter: recall@k of MB-prefilter + exact rerank vs exact search alone, and wall-clock. Any of these coming out negative is a result; the feature ships only for the properties that measure positive.

### 2.2 Fast reflex layer ← VNC and descending neurons

**Biology.** MANC (2024): 1,328 descending neurons, 733 motor neurons, ~23,500 VNC neurons; direct DN→MN connections are rare — most commands route through premotor interneuron communities ([Cheong et al.](https://elifesciences.org/articles/96084)). Command-like DNs actually recruit larger DN *populations* through DN–DN excitation ([Braun 2024, Nature](https://www.nature.com/articles/s41586-024-07523-9)). BANC's authors summarize the whole CNS as distributed control: effectors are dominated by *local* sensory feedback loops, with learning/navigation regions "supervisory but not essential for action" ([Nature 2026](https://www.nature.com/articles/s41586-026-10735-w)). A Sept 2025 firing-rate model of 4,604 MANC neurons isolates a three-neuron central pattern generator replicated in all six legs, with predictions confirmed optogenetically ([bioRxiv](https://www.biorxiv.org/content/10.1101/2025.09.12.675944v1)).

**What AI already has.** Brooks's subsumption is the ancestor. The 2026 versions: NeuroVLA — VLM planner over a cerebellum-like stabilizer over a "spinal" reflex layer on a neuromorphic chip, 0.4 W, safety reflexes under 20 ms ([arXiv](https://arxiv.org/abs/2601.14628)); TypeGo — four concurrent loops, S0 reflex under an LLM sequencer/deliberator ([arXiv](https://arxiv.org/pdf/2607.05482)); MantisBot in the hexapod lineage, where descending commands like "turn 30°" *modulate* thoracic reflexes rather than replace them ([Sci. Direct](https://www.sciencedirect.com/science/article/abs/pii/S1467803917300543)).

**The gap.** Nobody has published the insect DN architecture as the explicit model for the *interface* between an LLM and motor primitives — specifically the two findings above: commands are population-coded and get converted to discrete actuation locally, and the deliberative layer is supervisory rather than in the loop.

**What OBC could build.** A "spinal" tier on the ESP32 nodes: local sensor→actuator reflexes that run whether or not the gateway is reachable, and a *descending command* message type that is a small vector of modulations (gains, setpoints, inhibitions), not an action verb. The LLM tier sends modulations; the node converts them to actuation. The conscience gates sit at the descending boundary, which is where the fly's "supervisory" layer sits too. **Blocked on spine authentication** (SPINE-AUTH.md) — a descending command channel without auth is exactly the rogue-agent surface you're guarding against. The LoRa payload measurement that SPINE-AUTH needs first is also what sizes the modulation vector. Second feature, not first.

### 2.3 Navigation / heading ← central complex

**Biology.** The EB/PB compass is a ring attractor (local excitation, global inhibition) that holds heading in the dark and can be overwritten optogenetically ([Kim 2017](https://www.science.org/doi/10.1126/science.aal4835)); the hemibrain CX connectome exposes heavy recurrence and "abundant feedback from descending neurons" ([Hulse 2021](https://elifesciences.org/articles/66039)).

**What robotics already did.** Stone et al. 2017's anatomically constrained bee circuit — 8 compass cells, 16 memory, 16 steering, ternary weights — was run on a Dagu Rover 5 + Arduino Mega and homed successfully ([Current Biology](https://www.cell.com/current-biology/fulltext/S0960-9822(17)31090-4)); Stankiewicz & Webb 2020 flew it on a micro aerial vehicle with ~1.5 m error per 100 m outbound ([Springer](https://link.springer.com/chapter/10.1007/978-3-030-64313-3_31)). Ring attractors have been run on Loihi and BrainScaleS-2, with the bee circuit emulated 1000× faster than biology ([arXiv 2401.00473](https://arxiv.org/abs/2401.00473)). Cost is trivial: on the order of 10³ MACs per tick, ~10⁵ MAC/s at 100 Hz — negligible on a 240 MHz ESP32 with FPU (my arithmetic, no published MCU timing found; measure before claiming).

**The gap and the OBC question.** No published ring attractor on ESP32/Cortex-M, and no physical robot using the FlyWire/hemibrain CX wiring directly. But the question for OBC is simpler: **does any node move?** Until one does, this is the one seam with nothing to measure. Park it; when a mobile node exists, Stone's circuit is a weekend port and the measurement (heading drift vs time, homing error vs outbound distance) is already standardized by the bee papers.

### 2.4 The reasoner itself

The strongest reframing the connectome offers is not "replace the LLM with a fly" but BANC's line: the deliberative layer is *supervisory*. In the fly, most behaviour closes locally; the brain biases it. In OBC today, the LLM is in the loop for everything, which is why latency and cost dominate. The three features above are, together, a re-architecture toward the fly's shape: memory decides what is novel and worth the LLM's attention (2.1), reflexes act without it (2.2), and the LLM's output becomes modulation rather than command (2.2). The reasoner's role changes as a *consequence* of shipping those, which is the right order — no rewrite of the reasoner until there is something below it to supervise.

A separate, cheaper experiment worth running in a sandbox (not as an OBC feature): put the Shiu LIF fly brain in the loop with a ClawCam-derived sensory drive and read its descending-neuron output. It won't control anything useful, but it is the fastest way to learn what a population-coded descending signal *looks like* before designing the modulation vector in 2.2. Python, Brian2 or flypoke, one afternoon.

## 3. Recommended first feature: `obc-memory` mushroom body

Why this one: pure Rust, no new dependencies (a sparse random projection and a bit-set are stdlib work), no hardware, no auth blocker, lands in the crate that is already public and CI-tested, and its research questions are answerable with data OBC already produces. It also directly serves the conscience layer — a novelty signal on perception events is the natural trigger for "this needs a consent check."

**Proposed shape** (plan, not code — per the multi-file rule this waits for a go-ahead):

`crates/obc-memory/src/mushroom.rs` — `MushroomBody { projection: SparseProjection, k: usize, novelty: DecayingBloom, compartments: Vec<Compartment> }`. `tag(&[f32]) -> Tag` (sparse binary, top-k after expansion); `novelty(&Tag) -> f32` (distance-and-time-sensitive, PNAS 2018 form); `judge(&Tag) -> Valence` and `reinforce(&Tag, Compartment, sign)` (FlyModel partial freezing: only active-KC → this-compartment weights move). Seeded RNG for the projection, seed recorded — same input, same tag, byte-identical.

Wired at two points: the ClawCam ingest path (novelty score attached to each detection before the conscience filter) and the memory recall path (tag prefilter before vector search, valence attached to recalls). Both are existing call sites, so nothing is unreachable.

**Measurements** are section 2.1's three, each a test with a stated pass threshold decided before the run. Expansion ratio, k, and the decay constant are explicit parameters in config, never constants in code.

**ADR needed.** Choice of random vs learned projection (FlyHash vs BioHash) is expensive to reverse once tags are persisted; start random+seeded, record it.

## 4. Sequence

1. Mushroom body in `obc-memory` (this doc §3). Software only. Research output: three measurements on OBC's own data, and the first published-anywhere application of the MB circuit to LLM-agent memory if the numbers hold.
2. Sandbox: Shiu LIF fly with ClawCam drive, read descending output (§2.4). Informs 3.
3. Spinal tier + descending modulation channel on the ESP32 nodes (§2.2). After spine auth. Research output: an explicit insect-DN model of the LLM↔actuator interface, which no one has published.
4. Ring-attractor heading (§2.3), only when a node moves.

> **Where it stands, 2026-09-13.** 1, 2 and 3 are built and measured
> (`CHANGELOG.md`, walkthrough §A5b–A5g). 3 went further than written: the
> spinal tier is on the air behind authenticated frames, the first
> slot-bound rule runs on a real quantity (the node's own die temperature),
> and the MB→DN link is closed as a deterministic policy —
> `obc_agent::posture`: the mushroom body's novelty lowers a node's
> thresholds before the model has said a word, and a familiar objective
> restores them. Bench: novel → LED on, familiar → LED off, over LoRa, node
> reply confirmed. The body has since been measured on the brain's own 94
> embedded episodes (`crates/obc-memory/tests/mushroom_real_episodes.rs`):
> as first shipped it could not call anything novel (first-seen median
> 0.08, 0 of 74 at threshold 0.7); with warm-up centring, a sparser code
> and a threshold set from that data it separates new objectives (0.31–0.75)
> from rewordings of seen ones (≤ 0.24), repeats still ≤ 0.002. And it has
> run live: in the deployed brain, on 2026-09-13 at 12:52, a fruit-fly
> question with no precedent scored 0.372 → cautious → the node's slot 0 went
> to 0.15 on the first attempt; a familiar escalation at 13:10 (novelty
> 0.03) cleared it (`world.db`, `descending.obc-esp32-s3-001`; CHANGELOG).
> The loop this document proposed in §3 is closed in the running system.
> Not measured: whether the lowered threshold changed any outcome — the
> `posture_real_effect` harness exists for that and has no data yet. 4 waits
> for a moving node.

## 5. What I could not verify

The Cell paper full text would not fetch; the 166,700 figure is from the abstract and Janelia page. FlyModel's headline numbers come from a search summary, not the PDF. Stone 2017's paper doesn't say whether the Arduino or the phone ran the network. Community repos (flypoke, flybrain, fly-brain) are unreviewed and their timings self-reported. The 2026 arXiv controller papers (FlyGM, FLYNN, NeuroVLA, TypeGo) are preprints with author-reported results, and 2604.04033 is direct counter-evidence to the first two. The ESP32 cost figure in §2.3 is arithmetic, not a measurement.

## Sources

Google blog: https://research.google/blog/a-connectomics-milestone-mapping-the-complete-male-fruit-fly-brain/ · Male CNS (Cell): https://www.cell.com/cell/fulltext/S0092-8674(26)00942-6 · Dataset: https://www.janelia.org/project-team/flyem/male-cns-connectome · BANC (Nature 2026): https://www.nature.com/articles/s41586-026-10735-w · Shiu 2024: https://www.nature.com/articles/s41586-024-07763-9 · Shiu code: https://github.com/philshiu/Drosophila_brain_model · flypoke: https://github.com/vshapenko/flypoke · Loihi 2 fly brain: https://arxiv.org/abs/2508.16792 · Beiran & Litwin-Kumar 2025: https://www.nature.com/articles/s41593-025-02080-4 · BrainTrace: https://www.nature.com/articles/s41467-026-68453-w · FlyGM: https://arxiv.org/abs/2602.17997 · FLYNN: https://arxiv.org/abs/2607.00025 · Topology control study: https://arxiv.org/abs/2604.04033 · flyvis: https://www.nature.com/articles/s41586-024-07939-3 · FlyHash: https://www.science.org/doi/10.1126/science.aam9868 · Fly Bloom filter: https://www.pnas.org/doi/10.1073/pnas.1814448115 · FlyModel: https://direct.mit.edu/neco/article/35/11/1797/117579 · BioHash: https://arxiv.org/abs/2001.04907 · BioVSS: https://arxiv.org/abs/2412.03301 · Aso 2014: https://elifesciences.org/articles/04577 · Aso & Rubin 2016: https://elifesciences.org/articles/16135 · MB connectome: https://elifesciences.org/articles/62576 · PN→KC bias preprint: https://www.biorxiv.org/content/10.1101/2025.10.29.684686v2 · MANC (Cheong): https://elifesciences.org/articles/96084 · Braun 2024: https://www.nature.com/articles/s41586-024-07523-9 · MANC CPG model: https://www.biorxiv.org/content/10.1101/2025.09.12.675944v1 · NeuroVLA: https://arxiv.org/abs/2601.14628 · TypeGo: https://arxiv.org/pdf/2607.05482 · MantisBot: https://www.sciencedirect.com/science/article/abs/pii/S1467803917300543 · Kim 2017: https://www.science.org/doi/10.1126/science.aal4835 · Hulse 2021: https://elifesciences.org/articles/66039 · Stone 2017: https://www.cell.com/current-biology/fulltext/S0960-9822(17)31090-4 · Stankiewicz & Webb 2020: https://link.springer.com/chapter/10.1007/978-3-030-64313-3_31 · BrainScaleS-2 bee: https://arxiv.org/abs/2401.00473 · NeuroMechFly v2: https://www.nature.com/articles/s41592-024-02497-y · ZAPBench: https://arxiv.org/abs/2503.02618 · Fish Fire&Wire: https://www.janelia.org/fish-firewire

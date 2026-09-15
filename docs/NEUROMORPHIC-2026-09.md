# Neuromorphic reasoning → OBC: the tier that is missing, and what would justify spikes

> **Written 2026-09-15, nothing built.** Prompted by the first real run of
> `posture_real_effect` against the live record, which found the descending
> loop working and starved. This is the architectural argument for where
> neuromorphic hardware and spiking networks would earn their place in OBC,
> which is not where the thread has been pointing. Companion to
> [CONNECTOME-2026-09.md](CONNECTOME-2026-09.md) (the fly seams) and
> [WILD-2026-09.md](WILD-2026-09.md) (the detector → vet → act loop).

## 0. The one-paragraph version

OBC now has an insect-shaped *system*: a slow reasoner, a sparse-expansion
memory that judges novelty, a descending channel that modulates node
thresholds before the model speaks, and a rule tier on the node that acts
without waking anything. Every boundary is instrumented. Measured on
2026-09-15, that descending channel has made **four decisions in its
lifetime**, one of them cautious, and the rule it exists to modulate fired
**zero times** while cautious was held. The loop is not broken; it is
starved. A fly makes more descending decisions in twenty milliseconds. The
architecture is right and the input rate is wrong by roughly five orders of
magnitude, which means the next increment is *sensory*, not deeper in the
memory — and that is exactly where spiking hardware is not a curiosity.

## 1. The measurement that prompted this

`cargo test -p obc-memory --test posture_real_effect -- --ignored` against
`world.db` and `trajectories.db`, 2026-09-15, first run with real data:

```
posture changes wanted: {Default: 3, Cautious: 1}
confirmed time under each posture:
  obc-esp32-s3-001     default      7.50 h
  obc-esp32-s3-001     cautious     0.30 h
M1 firing rate and M2 refusal fraction, per rule and confirmed posture:
  rule                    posture   fires   /hour   refused  frac
  die-cool                default       6    0.80        3  0.50
  safe-link-offline       default      26    3.46       26  1.00
  safe-link-offline       cautious      1    3.33        1  1.00
M3 novel objectives by whether the node actually held cautious:
  held cautious                      N=  1  success   1  failure   0  rate 1.00
node-side dataset shape: 33 reflex reports with a known posture
```

Read it carefully, because three separate things are true.

**The instrument is honest.** `safe-link-offline` is a literal-threshold
rule that posture does not touch, and it ran at 3.46/hour under default
against 3.33/hour under cautious — flat, which is the control working. If
posture had moved *that*, the wiring would be wrong.

**The signal is absent.** `die-cool` is the only slot-bound rule, and it
fired six times under default and **zero** under cautious. Not "fewer" —
zero. Cautious was held for eighteen minutes, total, ever. M3 has N = 1.
Nothing here supports or refutes any claim about whether the descending
posture helps; there is no sample.

**The cause is upstream.** The trajectory store holds 99 episodes across
2.19 days — 45/day — but only *one* since 09-13. Most of what the body saw
was the same System 2 escalation text, which it correctly scores at
0.001–0.03 every time, because it is genuinely the same thing. A novelty
signal fed a near-constant input produces a near-constant output. The body
is doing its job.

So the honest state: the connectome thread's central claim — the MB → DN
link as a deterministic policy — is *built and verified on the bench*
(novelty 0.372 → slot 0.15 → node `applied 1`), and *unmeasurable in
service* for want of things to be surprised by.

## 2. What is actually neuromorphic here, and what is not

Worth stating plainly, because it is easy to over-claim to someone else.

**Neuromorphic in architecture, not in substrate.** The mushroom body is
three published algorithms derived from a spiking circuit — FlyHash's
sparse random expansion and winner-take-all, the fly Bloom filter's
decaying per-cell weights, FlyModel's compartmental valence. None of it
spikes. It runs as f32 arithmetic on a host CPU and is deterministic by
design, which the project's invariants require. That was the right call:
the algorithms carry the properties the spikes were there to produce
(one-shot insert, distance- and time-sensitive novelty, interference
resistance), and spiking them would cost reproducibility and buy nothing
on hardware that has a CPU.

**The descending channel is a real architectural borrowing.** Sixteen
slots, sparse `(slot, level)` pairs, RAM-only, all-or-nothing, Track-0
gated — a low-dimensional modulation vector from brain to spinal tier. The
LIF sandbox is what justified the shape: on FlyWire v783, sugar recruited
61 of 1303 DNs and JON 48, with cosine 0.038 between them — sparse, graded,
and nearly orthogonal per context.

**The only thing that spikes is the sandbox.** `experiments/lif-fly/` runs
the Shiu 2024 model at about 55 s per simulated second on numpy and
reproduces MN9 at 63.3 Hz against the paper's 67.0. It is a reference, not
a component; nothing in the running body calls it.

**The reflex tier is not a network at all.** It is rules with thresholds,
deliberately, because rules are auditable and Track-0 gateable. It occupies
the functional slot of a spinal reflex arc without imitating its
implementation.

That combination — insect architecture, conventional substrate — is a more
defensible thing to have built than a spiking network would have been on
its own, and it is unusual. It should be described that way and not as
"a neuromorphic system."

## 3. The tier that is missing

The fly has an optic lobe. OBC does not.

Between photons and the 384-dimensional sentence embedding the mushroom
body hashes, OBC currently has: a camera, a detector, a vision-language
model, a *text string*, and an embedder. Five lossy, slow, power-hungry
stages, and the text string in the middle is a particularly strange place
for a nervous system to route its vision through.

The fly puts roughly sixty cell types between the retina and the central
brain and computes motion in about thirty milliseconds, with no learning at
inference time. `flyvis` (Nature 2024, already in the CONNECTOME source
list) is a connectome-constrained model of exactly that stage. It is the
one seam in section 2 of that document which was never opened.

This is also, precisely, ClawCam's original question: a camera watching for
something to happen, where 99.99 % of frames are nothing. And it is the
same shape as the WILD loop already chosen — cheap always-on detector, then
a vet, then act.

**The case for event-driven sensing here is not accuracy, and not even
power. It is duty cycle.** An always-on tier at milliwatts is the only way
to give the descending loop enough events that a learned policy could ever
beat a one-bit rule. It fixes §1's starvation at the source rather than
working around it.

A second property matters for the conscience layer: an event sensor does
not produce frames. "The camera physically cannot emit a photograph" is a
stronger privacy claim than "the frame is gated after capture," and
`docs/CONSCIENCE.md` currently has to make the weaker one.

## 4. Where spikes earn their keep, and where they would be cargo cult

**Earn it**

- *Event-driven vision on a node.* Microsecond latency, high dynamic
  range, output proportional to change rather than to time. The workload is
  natively sparse and asynchronous, which is the one case where spiking
  silicon is not competing with a well-optimised MAC array.
- *Always-on anomaly and motion gating at sub-milliwatt.* The WILD
  detector slot exactly: run forever, wake the expensive tier rarely.
- *Temporal patterns where timing is the signal.* Gait, vibration,
  acoustic signature. Rate-coded features throw away what spike timing
  carries; this is the class of problem where that loss is the whole
  problem.

**Cargo cult**

- *Spiking the mushroom body.* Costs determinism, buys nothing on a host.
- *Spiking the reflex rules.* They are auditable and gateable because they
  are rules. A learned tier belongs *beside* them, not instead of them.
- *A spiking replacement for the reasoner.* No.
- *Neuromorphic hardware bought before §5 rung 2 exists.* Without a data
  rate there is nothing to run on it, and the measurement in §1 is what
  that sentence is standing on.

One constraint to name early rather than discover late: the project
requires reproducible output. A fixed-point, fixed-timestep LIF network is
deterministic and satisfies that; a network relying on hardware
asynchrony or analogue dynamics does not. Any node-side spiking policy has
to be the former, and that should be decided before silicon is chosen, not
after.

## 5. The ladder

Cheapest first, each rung measurable on its own.

**Rung 1 — make the descending vector more than one bit.** Software only,
no hardware, no new dependency. The body already produces graded novelty
*and* an outcome prior; sixteen slots exist, one is used, at a constant
level. This is the prerequisite for everything below: if the rate rises
while the signal stays binary, the result is many identical decisions.
Detailed as the plan in §6.

**Rung 2 — raise the experience rate before buying anything.** The offline
half first: replay a large episode corpus through body and policy in the
`mushroom_real_episodes` harness and look at the level distribution — this
needs no hardware and tells you whether the rung-1 map is sane before it
touches the mesh. Then the live half: a perception source that produces
episodes continuously.

**Checked 2026-09-15, and the obvious corpus is not one.** The WILD Zenodo
record (10.5281/zenodo.18879184) is licensed **CC-BY-4.0** — attribution
only, no non-commercial and no no-derivatives clause — so the licence gate
this document and `WILD-2026-09.md` §7 both flagged is **clear**. The content
gate is not. The record describes itself as "the figure data for manuscript
[…]", its resource type is *Computational notebook*, and its fourteen files
are `Fig2.zip`…`Fig6.zip` and
`ExtendedDataFigure1.zip`…`ExtendedDataFigure10.zip`, 238.1 MB in total.
That is per-panel processed arrays, not the continuous multi-stream
recording with behavioural labels an episode stream needs. Two archives are
large enough to hold real traces (`ExtendedDataFigure9.zip` 128.6 MB,
`Fig3.zip` 44.7 MB) and might carry something usable, but nothing has been
downloaded and saying more than that would be a guess. WILD's raw binaries
are "on request" per its own §1, which is a human-latency path rather than a
next step.

**The ClawCam seed data is not a corpus either — checked the same hour, and
this one was my own recommendation.** `clawcam_gateway.db` holds 187
detections spanning 2026-06-23 to 07-06, which vindicates the "fourteen days
of recorded detections" line in OBC-Prime's README on duration and nothing
else. Every row's `source` and `model_name` is **`scenario-sim` v1.0.0**:
they are generated, not recorded. Across all 187 there is **exactly one
distinct bounding box** (`0.4, 0.4, 0.6, 0.6`) and **exactly one detection
per frame**; hour-of-day is flat, where any real camera on any real animal
would show a diel cycle. The feature space is therefore species (4 values:
deer 93, fox 42, coyote 37, person 15) × confidence (175 distinct values
between 0.63 and 1.00) × review state (3). Feeding that to the mushroom body
would measure `scenario-sim`'s random number generator. It is a fixture for
exercising the pipeline, which is what it was built for, and it was
recommended here on the strength of a README sentence rather than a look at
the table.

**So: there is no corpus.** Two candidates, both checked against the data
rather than the prose, both rejected for different reasons. That is worth
stating as a finding rather than a setback, because it sharpens §3 instead of
contradicting it — and it exposes something neither this document nor
`CONNECTOME-2026-09.md` had noticed:

> **The mushroom body has no sensory input at all.** In the fly it sits
> downstream of the antennal lobe: roughly fifty projection neurons of
> olfactory drive, continuously, whether or not anything is happening. In
> OBC it sits downstream of the *chat prompt*. Outside tests, `experience`
> has exactly two callers — `TrajectoryStore::record` and
> `attach_mushroom`'s replay — and both take episodes, which are agent
> turns. The body does not perceive; it tastes what the operator typed. Its
> 45 episodes a day at the busiest, and one a day since, are not a
> measurement problem — they are the whole of its sensory life.

That reorders the ladder. Rung 2 as written ("raise the experience rate")
assumed a corpus could be borrowed, and none can be: a body with no senses
cannot be handed someone else's. **Before any sensor in rung 3 could feed
the body, the body needs an input path that is not an agent turn** — a way
for perception to produce an episode. That is a software change, it is
cheap, it is a strict prerequisite for rung 3, and nobody had written it
down. It is the real next rung.

**Rung 3 — event-driven sensing on a node.** Only once rung 2 has shown
what a real event rate does to the descending signal. Needs a current
survey of neuromorphic silicon, which this repository does not have:
`OBC-Prime/docs/EDGE-LM-2026-09.md` covers language models on nodes and
says nothing about spiking sensors or inference parts. Families worth
surveying — **all unverified, none costed, availability unknown**:
Prophesee's low-power edge event sensors; SynSense Speck (sensor and
spiking inference in one package, which is the ClawCam shape almost
exactly); Innatera; BrainChip Akida. Intel Loihi 2 is research-access and
almost certainly out of scope. `TODO(source)` on every one of these until
somebody reads current datasheets.

**Rung 4 — a node-side policy that is neither a rule nor a language
model.** A few-dozen-weight sensor→slot map. The `No language model on a
node` ADR already reframed it this way and named the blocker as metric and
data, not capability. Rungs 1–3 exist to produce that metric and that data.
Whether the map is a small fixed-point LIF network or a lookup table is an
implementation question to answer *after* the dataset exists, and the
answer may well be the lookup table.

## 6. Rung 1 in detail (the plan, not built)

**What changes.** `Posture` stops being `{Default, Cautious}` and becomes a
computed level per owned slot. The firmware already accepts arbitrary
levels — `descend {"m":[[slot, level]]}`, sixteen slots, level mapping to
`30 + level·40 °C` on the die rule — so *the wire format needs no change at
all*. The one-bit behaviour lives entirely in the host.

**The map.** Two inputs the body already produces and the policy currently
ignores one of:

- `novelty` (0–1, graded) — today thresholded at 0.25 and discarded.
- `success_prior` (`Option<f32>`) — today feeds only the prompt. An
  objective that *resembles past failures* deserves caution even when it is
  familiar. This is the FlyModel half of the body having no effect on the
  body's own output, which is a gap worth closing on its own.

Combine to a single `caution ∈ [0,1]`, then `level = lerp(rule_default,
novel_level, caution)`. Endpoints stay in config. **The exact combining
function is chosen from the level distribution over the real 99 episodes,
not guessed** — that is step A below.

**The trap, which the one-bit design got for free.** A graded level means
almost every turn is a *change*, and the policy sends on change. On a
duty-cycle-limited radio that is a frame per turn. Rung 1 therefore needs a
deadband: quantise the level (step in config) and keep a floor below which
the slot is cleared and nothing is sent. Without this, rung 1 makes the
mesh noisier for no gain — and §1's whole problem is that there is nothing
to measure, so spending airtime to measure it badly would be the worst
outcome.

**Steps, each verifiable.**

- **A.** *Done 2026-09-15 as `5428a41`; read in §6.1.* The map as pure
  functions with no caller, plus a replay over the stored 99 episodes
  printing the airtime and the levels each candidate would have emitted.
  This is where the evidence for every constant in this feature comes from.
- **B.** Wire it: `PostureConfig` gains the endpoints, step and floor;
  `descending.<node>` records `level`, `novelty` and `prior` so the effect
  harness can regress against a continuous variable; validation and the
  shipped-keys test; `config.example.toml`.
- **C.** `posture_real_effect` buckets M1 by level rather than by
  `{default, cautious}`; bench on the node with `bench_descend.py` to
  confirm a graded level reaches slot 0 and moves the LED threshold where
  arithmetic says it should.

### 6.1 Step A result (2026-09-15)

**Built by a parallel session as `5428a41`**, not by the author of this
document, and placed better than §6 proposed: `LevelMap` and `Descent` are
pure functions in `obc_agent::posture`, with the replay at
`crates/obc-agent/tests/posture_level_replay.rs`. §6 assumed the harness
would sit in `obc-memory`'s tests; that was wrong, because a dev-dependency
from `obc-memory` back onto `obc-agent` would spend the near-leaf property
that crate was extracted for. The replay belongs where the policy is. Its
numbers are in the CHANGELOG; what follows is a second reading of the same
run, and one thing its table does not price.

**Independently reproduced.** A separate replay, written against the same
store before `5428a41` was found in the tree, agreed on every corpus fact:
99 episodes over 2.19 days, 79 after warm-up, 34 of them novel, an outcome
prior on 74. Two implementations, one answer, which is worth more than
either alone.

**Novelty has ample range to grade — and one claim in `5428a41` needs
correcting.** The doc comment on `LevelMap::caution` describes the corpus as
"piled at novelty 0.001–0.03 with four points anywhere else." Measured, it is
p25 0.011, p50 0.204, p75 0.367, p95 0.555, max 0.750. The pile is real, but
it is the bottom quartile rather than the corpus: half of these episodes sit
above 0.20. That description fits the *pre-fix* body (09-13, first-seen
median 0.082), not this one, and the replay prints no distribution, so
nothing caught it. The argument built on it — that a curve fitted to four
points would be a guess wearing evidence, so keep the shape linear and
falsifiable — is still right. Its premise is not.

**What the frame table does not price: how much of the time the node is held
away from its own defaults.** The cheapest corner in `5428a41` is knee 0.00 /
full 0.25, which saturates caution at novelty 0.25 — so the median turn
(novelty 0.204) sits at caution ≈ 0.82. Emitting `Clear` only below caution
0.05 means clearing only below novelty ≈ 0.013, about a quarter of turns:
**the node would be modulated roughly three turns in four.** A wider bracket
(full ≈ 0.55) with floor 0.15 clears below novelty ≈ 0.25 and leaves the node
at its own defaults for about 55 % of turns, at 48 frames against 31.

That difference is not aesthetic, and it is not really about airtime:

> `posture_real_effect`'s M1 compares a rule's firing rate **under cautious
> against under default**. A configuration that holds the node away from its
> defaults three turns in four leaves that comparison with almost no control
> group — and M1 is the entire reason rung 1 exists. The cheapest corner in
> the table would buy four frames a day and cost the measurement.

So frames are a constraint, not the objective. Step B should take the widest
bracket whose frame cost is tolerable rather than the cheapest one, and
should report the default/cautious split beside the frame count — which
neither replay currently prints, and which is the one number that says
whether rung 1 achieved anything.

**The prior, read from both runs, lands in the same place.** It is
*available*: present on 74 of 79 turns and on 29 of the 34 novel ones, which
refutes the step-A plan's prediction that it would be missing exactly where
it mattered. It is also, on this corpus, *uninformative*: priors run min
0.52, p50 0.98, so `1 − prior` contributes 0.02–0.05 and never lifted an
episode into caution that novelty had not already reached. A term added now
would be a weighting fitted to a body that has almost never failed. Both
readings agree on the action: **step B carries novelty and prior separately
on `descending.<node>`**, and the term waits for a corpus with real failures
to fit against.

**What would count as success — and what would not.** Not "die-cool fires
more under cautious." The honest deliverable of rung 1 is *a dataset with
variance*: enough distinct level buckets carrying non-zero fire counts that
M1 could be fitted at all. Rung 1 alone will probably not produce even
that, because the rate problem in §1 dominates; expect four graded
decisions where there were four binary ones. That is not failure, and it
should not be written up as one. Rung 1 makes the signal continuous; rung 2
makes it frequent; only both together produce something learnable.

**Files touched.** `crates/obc-agent/src/posture.rs`,
`crates/obc-config/src/lib.rs`, `config.example.toml`,
`crates/obc-memory/tests/{mushroom_real_episodes,posture_real_effect}.rs`,
`CHANGELOG.md`, and possibly `tests/posture_live.rs` (it asserts specific
levels). Six files, so: plan first, go-ahead, then small steps.

## 7. Refused, and why

- **Buying neuromorphic hardware now.** There is no data rate to run on
  it. §1 is the argument; rung 2 is the precondition.
- **Spiking the mushroom body.** Determinism is an invariant, and the
  algorithms already carry the properties.
- **Slot *selection* in rung 1** — different contexts recruiting different
  slot subsets, as the DN readout suggests. Only one slot is bound to a
  real quantity today. Choosing subsets among one bound slot is
  speculative abstraction; this waits for a second real slot.
- **The outcome prior in rung 1's map** — measured in §6.1 and deferred.
  Unsupported on a corpus that is almost all successes, and it costs a
  noise floor. Not a permanent no; a question this data cannot answer.
- **Feeding `security/trust.rs` from any of this.** Settled 2026-09-14 for
  the auth alarm and the reasoning carries: trust scores actuating nodes by
  their own behaviour, which is a different question.

## 8. Open items, smallest first

1. ~~Rung 1 step A~~ — done 2026-09-15 as `5428a41`, read in §6.1. **Step B**
   next, and it needs one number neither replay prints: the share of turns
   spent at the node's own defaults. Choose the widest bracket whose frames
   are tolerable (full ≈ 0.55, floor 0.15 ≈ 55 % default at 48 frames), not
   the cheapest (full 0.25, floor 0.05 ≈ 25 % default at 31), because M1
   needs a control group. Carry novelty and prior separately on the fact.
   Fix the stale corpus description on `LevelMap::caution` while there.
2. ~~Read the WILD Zenodo record's licence~~ / ~~use the ClawCam seed data~~
   — both checked 2026-09-15 and both rejected (§5 rung 2). The item that
   replaces them: **give the mushroom body an input path that is not an
   agent turn.** `TrajectoryStore::record` is the only caller of
   `experience`, so perception cannot reach the body at all. Until that
   exists, no sensor in rung 3 can feed it and no corpus can be borrowed
   into it. Smallest honest next step in the whole ladder.
3. The second bound slot. Slot 0 is the die-temperature LED; nothing else
   on the node is slot-bound, which is why §7 defers slot selection.
4. A neuromorphic-silicon survey to sit beside `EDGE-LM-2026-09.md`.
   Datasheets, availability, price, determinism, toolchain. None of §5
   rung 3's names should be trusted until this exists.
5. `flyvis` as a reference implementation of the missing tier — worth
   reading before designing anything in rung 3.

## 9. What I could not verify

The ClawCam and Zenodo findings in §5 rung 2 are from the records and the
database themselves and are as solid as anything here. Nothing was
downloaded from Zenodo, so "two archives are large enough to hold real
traces" is an inference from file size, not a look inside.

Every hardware name in §5 rung 3 is from memory and none of it was checked
against a current datasheet, price list or stock status; that is what item
4 above is for. The `flyvis` citation is from the CONNECTOME source list,
not re-read. The claim that event sensors suit the conscience layer better
than frame sensors is an architectural argument, not a legal or policy
opinion, and has not been tested against `docs/CONSCIENCE.md`'s actual
wording. The §1 numbers are the only thing here that was measured, and they
were measured once, on one body, on a bench that has been unplugged since
the 13th.

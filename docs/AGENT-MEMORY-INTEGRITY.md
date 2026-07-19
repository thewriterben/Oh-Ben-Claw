# Agent memory integrity

How the agent's own output is kept from being read back as perception, and why the
escalation playbooks are written the way they are.

Both rules here were bought with one incident. The incident is worth reading first,
because the rules look like ordinary hygiene until you see what they cost.

---

## The incident (bench, 2026-07-17)

A `safe-mesh-node-lost` reflex fired every 5 seconds for 100 minutes while every node on
the mesh was visibly answering at −57 dBm. It was reporting, accurately, a node that did
not exist:

```
"node": "escalation_status",  "escalated": true,  "age_s": 5987
```

The chain:

1. The mesh playbook's step 3 said *"record the loss to world memory."* Steps 1 and 2
   named tools (`mesh_status`, `mesh_command`); step 3 named a **store**.
2. System 2, woken by a genuine escalation, did exactly that — and had to invent an
   entity name and a payload shape at runtime. It chose `mesh.escalation_status`.
3. `mesh_supervisor::snapshot()` discovered nodes **lexically**: any entity under `mesh.`
   splitting into two dot-parts was a node.
4. So the note became a node. A note has no radio, so it went offline, escalated after
   two minutes, and pinned `mesh.escalated_count >= 1` permanently.
5. The reflex fired on that count, waking System 2, which recorded more notes.

The agent manufactured an emergency out of its own notes and reported it in a loop. It
also spent four real LoRa recovery commands pinging a note.

**Nothing malfunctioned.** The model followed its instructions correctly, and the note it
wrote was *true when written* — `mesh.gw-40.escalation` shows gw-40 really was escalated
at that moment. The judgement was sound. The interface was wrong: it asked for a decision
about *storage* from something that only had an opinion about *the world*.

### What the audit found

Reading every fact System 2 wrote that session (2,544 facts, 23 wakes):

- **Two** freehand writes in 23 wakes — about 9%. Rare enough to survive testing,
  frequent enough to eventually poison.
- Two writes, **two different naming conventions and two different schemas**:
  `mesh.gw-40.failure` (3-part, per-node) and `mesh.escalation_status` (2-part,
  aggregate). With *N* writes you get *N* conventions and nothing downstream can query
  any of them.
- Both were **double-encoded** — a JSON string containing JSON — so `value.get("status")`
  returned `None` on both.
- One carried `"last_seen":"2023-05-10T19:06:17.874Z"`: a fabricated timestamp three
  years off, in ISO format where the system uses epoch millis, sitting beside a
  genuinely-observed RSSI. Part real, part invented, which is the hardest kind to catch.
- **The near-miss was worse than the hit.** `mesh.gw-40.failure` landed in the
  message-type namespace of a *real* node. `.failure` happens to be inert — but
  `.health`, `.escalation`, `.recovery` and `.cmd_result` all live in exactly that shape
  and are all load-bearing. One different noun and the agent would have been writing a
  live node's health status, silently outranking the supervisor.
- **Provenance was self-declared.** `world_memory.observe` took `source` as a caller
  parameter and advertised it to the model. An agent could have written
  `source: "lora-gateway"` — the exact value the supervisor trusts as evidence that a
  radio exists.

---

## Principle 1 — provenance is stamped, and every consumer declares what it trusts

> A fact's origin must be assigned by the framework, never chosen by the writer. Any
> consumer that acts on facts must state which origins it will act on.

Discovery in the supervisor is now **authoritative, not lexical**: a node exists because
the LoRa gateway *heard* it, so the rollup fact must carry the gateway's source. The
shape of an entity name is not evidence of a radio.

That check is only sound because the agent cannot forge the field. `world_memory` stamps
`AGENT_SOURCE` and no longer accepts `source` from the caller.

The old parameter was also quietly doing two jobs, and separating them clarifies the
whole design:

| | meaning | who sets it | trust signal? |
|---|---|---|---|
| **provenance** | who *wrote* this fact | the framework | yes |
| **attribution** | who the agent *claims* told it | the agent | no — it's content |

"A PIR told me it's 21.5" is an agent assertion carrying a device claim. It is not a
reading from a PIR. Attribution now folds into the value as `reported_by`; provenance
stays the agent's.

The classical statement of this is **Biba's integrity model** (1977): a high-integrity
process must not consume low-integrity data, because doing so silently downgrades
everything it produces. The mesh supervisor alarms and actuates — high integrity. Agent
notes are assertions — low integrity. The phantom happened because both lived in one
undifferentiated pool.

**Status: half done.** One consumer (the supervisor) gates on provenance. The general
version is open — see *Open work* below.

## Principle 2 — every playbook step names a tool with a contract

> Never an action with a free-form target.

Steps 1 and 2 of the mesh playbook could not have produced this failure; there was
nothing to invent. Step 3 handed over an untyped store.

Concretely:

- **Findings are recorded only through `record_incident`** (`subject`, `status`,
  `detail`, `evidence`). It owns the entity name, schema, timestamp and provenance. The
  status is a **closed set** — the audited writes invented `"critical"` and
  `"presumed_lost"` on two separate occasions, and free-form status is how a log stops
  being queryable. Dotted subjects are refused, because `incident.gw-40.failure` would
  forge a namespace level — the exact shape of the near-miss, one namespace over.
- **`mesh.*` is reserved.** The tool refuses writes there and points at
  `record_incident`.
- **No step tells the model to alert anyone.** Escalations already fan out to the
  notification log-of-record and any webhook *before* System 2 is woken. Asking the model
  to alert someone invites it to invent a channel it does not have.
- **Playbooks say to record only what was observed.** A missing field is recoverable; an
  invented one is not.

### The rule is enforced, not just documented

Prose rots. `every_escalation_names_its_tools_and_a_typed_place_to_record` iterates every
escalating rule and fails the build if one carries a bare reason, points at `world_memory`
for recording, or omits that alerting is automatic.

This mattered immediately: auditing the reasons revealed `MESH_LOST_PLAYBOOK` was not the
weak playbook — it was **the only one**. The other four escalations (power critical, audio
alarm, sensor unreliable, overheat) woke the reasoner with a bare statement of fact and no
contract at all. The phantom happened on the *best*-documented path.

---

## Principle 3 — origin is typed, and every consumer declares what it acts on

*Shipped 2026-07-19.* Every fact carries an `Origin` distinct from the descriptive
`source`:

| | meaning | example writer |
|---|---|---|
| `Observed` | a sensor, radio or driver reported it — the world said so | LoRa gateway |
| `Derived` | the framework computed it from other facts | mesh supervisor |
| `Asserted` | an agent concluded it — a claim, not a reading | `record_incident` |
| `Instructed` | a human said so — authority for intent, not evidence | operator input |

Consumers hold an `OriginSet` and say what they will act on. It is a **set, not a
threshold**, because trust here is not one ordering: "is this evidence about the world?"
ranks `Observed` over `Asserted`, while "does this carry authority to act?" puts
`Instructed` near the top and still leaves it useless as a reading. A consumer collapsing
both questions into one level would be wrong about one of them.

Declared so far:

- **Reflex engine** — `EVIDENCE` (`Observed` + `Derived`). Reflexes are automatic physical
  responses to sensed conditions, so an agent's claim and a human's typed value are both
  excluded. Enforced at the one point facts enter the snapshot; every leaf condition uses
  `is_some_and`, so a withheld fact evaluates false and nothing fires — withholding is
  fail-safe by construction. Withheld facts are **logged**, because "the sensor is fine"
  and "I refused to believe the sensor" must not look identical.
- **Mesh supervisor** — a node exists because something was `Observed`, replacing a
  comparison against the gateway's source string. Strictly stronger: a source check cannot
  see a trusted component relaying untrusted content.
- **Foresight** — `EVIDENCE`, and reports `excluded_samples` rather than silently fitting
  fewer points. Letting asserted values into a trend closes a short loop where a model's
  guess about a value bends the prediction *of that value*, and the output still reads
  like measurement.
- **Siteplan** — no declaration needed; it reads no world memory.

### How the four design questions were answered

1. **Is `derived` its own class?** Yes — the framework computing a health rollup is a
   different kind of claim from a radio reporting a frame, and consumers may want one
   without the other.
2. **Mixed inputs → low-water mark.** A conclusion is at most as trusted as its
   least-trusted input, or derivation launders assertions into observations. *Not yet
   enforced mechanically* — nothing currently computes a fact from mixed origins, but
   nothing stops it either. See Open work.
3. **Default for a consumer that forgets.** `observe()` keeps its signature and defaults
   to `Derived` — honest for most of the ~100 existing callers, and crucially *not*
   `Observed`, so a caller who has not thought about provenance cannot manufacture
   evidence by accident. `Origin::parse` returns `Asserted` for anything unrecognised,
   including `"OBSERVED"`: no case-insensitive uplift, no forward-compatible guessing.
4. **Migration.** Additive `ALTER TABLE` guarded by a `pragma_table_info` check, with a
   one-time backfill by source. The backfill is explicitly *not* the ongoing rule — see
   below — and unknown sources keep the `asserted` default rather than being guessed
   upward, because for old rows we genuinely do not know and guessing up would launder
   history into evidence. The phantom note now reads `asserted` in hindsight.

---

## Open work

### Confirmed hazard: a trusted writer relaying untrusted content

Scoping the taxonomy work on 2026-07-19 turned "a reflex will fire on a fact an agent
asserted" from a prediction into a two-call reproduction, entirely through legitimate
paths:

```
LLM: power {action:"report", soc_pct: 5}
  → PowerController::ingest
  → world.observe("power.mode", …, source: "power")   ← a framework constant
  → safe-power-critical-escalate fires
  → safe-power-critical-stop issues Stop to a physical actuator
```

Same shape for `sense {action:"ingest", quantity:"temperature", value:99}` →
`sensor.temperature` → `safe-overheat-{quantity}`, and `hear` → `audio.{stream}` →
`safe-audio-alarm-{stream}`.

**Severity, stated plainly.** Today the impact is *spurious safing* and another
self-poisoning wake loop — not dangerous actuation. Stop is the fail-safe direction, the
overheat rule only escalates, and Track 0 still bounds everything that reaches a pin. The
structure is what matters: it is unsafe for any future rule whose action is not fail-safe.

**The root cause is not a bug in those modules.** `sensing/mod.rs` and `power/mod.rs`
implement the provenance/attribution split correctly — provenance is their own constant
(`"sensing"`, `"power"`), and the caller's claimed source goes into the value as content.
What they cannot express is that a *trusted writer relayed untrusted content*. Their fact
is indistinguishable from a real fuel-gauge driver's. That is the confused-deputy shape,
and it is a limitation of the vocabulary, not of the code.

**This is why origin cannot be derived from `source`.** The obvious implementation — one
mapping function, zero call-site churn — would classify these as observed. Origin has to
be set at the boundary where content enters, and travel with the reading: tool paths
assert, driver paths observe, and the same controller serves both honestly.

**Status: closed.** `SensingController::ingest`, `PowerController::ingest` and
`AudioController::observe` now take an explicit `Origin` that travels with the reading.
The three tools (`sense`, `power`, `hear`) pass `Asserted`; the ClawCam poller, which
feeds real camera classifications into the same audio controller, passes `Observed`.

The design question that looked hard turned out not to exist. Threading *caller identity*
through the tool layer would have meant either changing `Tool::execute` across 83
implementations, a race-prone side-channel, or — the tempting one — a flag injected into
`args` at the gateway, which recreates the forgery bug fixed in `027ef07` exactly, since
the model emits `args`. None of that is needed: **a tool does not have to ask who called
it, because the tool *is* the agent boundary.** `SenseTool` is the agent's path by
construction. Only `audio` had real ambiguity, because ClawCam and the `hear` tool
genuinely share a controller — and that is resolved by the origin travelling with the
call.

The answer came from reading the wiring rather than reasoning from the abstraction; the
elaborate options were all solving a problem that the code did not have.

Tests cover both directions, which matters more than the first: `power report {soc_pct:5}`
produces `power.mode == "critical"` (the derivation stays honest) as an `Asserted` fact
that does **not** stop an actuator, while the same value from a fuel gauge still escalates.
Closing a hazard by breaking the feature is not closing it.

*Still open:* what an **operator** reading should be. A human typing a real meter value is
`Instructed` — authoritative about intent, still not a sensor — and there is no operator
path through these tools today. When one is wanted, it should be a separate authenticated
route that writes `Instructed` directly, rather than an attempt to distinguish callers
inside a shared tool.

**Also open — low-water mark not enforced.** Nothing currently derives a fact from mixed
origins, but nothing prevents it either. When something does, it must take the
least-trusted input's class, or derivation becomes a laundering step.

**Also open — no *"is my intervention working?"* check.** System 2 woke 23 times over 3.2
hours with the same reason, acted, changed nothing, and never noticed the condition was
unchanged. The novelty gate and rate limit worked exactly as designed — they suppressed
the noise. Nothing was watching for a stuck loop underneath it.

---

## Reading

### Foundations

- **Biba, "Integrity Considerations for Secure Computer Systems"** (1977) — the integrity
  dual of Bell-LaPadula; the formal version of Principle 1.
- **Buneman, Khanna & Tan, "Why and Where: A Characterization of Data Provenance"**
  (ICDT 2001) — vocabulary for what a fact's origin even means.
- **Green, Karvounarakis & Tannen, "Provenance Semirings"** (PODS 2007) — how provenance
  composes when facts are combined; question 2 above.
- **Doyle, "A Truth Maintenance System"** (1979) — retracting conclusions when a premise
  is withdrawn. World memory is bitemporal but does not track justification, so a
  retracted premise currently leaves its conclusions standing.

### Current work (surveyed 2026-07-19; titles from search, not yet read in full)

- [From Untrusted Input to Trusted Memory: A Systematic Study of Memory Poisoning Attacks
  in LLM Agents](https://arxiv.org/html/2606.04329v1) — the closest published framing of
  this incident: memory assembled from content that is "later retrieved as part of the
  agent's internal context and treated as trusted knowledge", with the observation that
  current systems have "no robust mechanism to track the provenance of stored entries".
- [From Agent Traces to Trust: A Survey of Evidence Tracing and Execution Provenance in
  LLM Agents](https://arxiv.org/pdf/2606.04990) — best entry point for the open work
  below; a map of what has been tried.
- [SMSR: Certified Defence Against Runtime Memory Poisoning in Persistent LLM Agent
  Systems](https://arxiv.org/html/2606.12703) — write-time HMAC provenance plus randomised
  ablation. Worth reading, worth *not* copying: HMAC attestation defends against a
  compromised writer. Ours is not compromised, it is confused. A stamped constant is the
  right strength here.
- [LoopTrap: Termination Poisoning Attacks on LLM Agents](https://arxiv.org/html/2605.05846v1)
  — agents judging their own progress from signals they also process; structurally the
  "23 wakes, nothing noticed the loop was stuck" gap, minus an attacker.
- [MemoryGraft](https://arxiv.org/html/2512.16962v1) and
  [AgentPoison](https://www.researchgate.net/publication/397214044_AgentPoison_Red-teaming_LLM_Agents_via_Poisoning_Memory_or_Knowledge_Bases)
  — the attack side, for the threat picture.

### Why this incident is worth more than it looks

Essentially all of the current literature assumes an **adversary**: injected web content,
poisoned documents, attacker-controlled tool output. This incident had no attacker. The
agent poisoned itself by following its instructions correctly, and the note it wrote was
true when written.

The proposed defences mostly transfer, but the threat model is stated too narrowly, and we
have direct evidence for the stronger claim:

> **Untyped agent memory fails without anyone attacking it.**

Provenance typing is therefore not a security control to add when adversaries are
expected. It is a correctness property. That changes the default question from *"is this
input hostile?"* — which you ask about untrusted sources — to *"is this the kind of thing
I am allowed to act on?"*, which must be answered for every fact, always, including the
ones the agent wrote itself in good faith.

---

*Origin: bench session 2026-07-17/19. Commits `5159720` (lexical discovery → sourced at
the radio), `027ef07` (stamped provenance), `ec6c9b5` (`record_incident`), `5077554` (the
four missing playbooks + the enforcing test), then the origin taxonomy: the `Origin`
column and migration, discovery by `Observed`, and the reflex/foresight trust gates.*

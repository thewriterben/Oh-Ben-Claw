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

## Open work

**Origin taxonomy + per-consumer trust declarations** (ROADMAP / task #10). Add a trusted
origin class distinct from the descriptive `source` — *observed* (a sensor or radio said
it), *derived* (the framework computed it), *asserted* (an agent concluded it),
*instructed* (a human said it) — and have every consumer declare which classes it acts on.

Four questions to settle before writing the migration:

1. **Is `derived` its own class, or does it inherit from its inputs?**
2. **What happens on mixed inputs?** The defensible answer is a low-water mark: a
   conclusion is at most as trusted as its least-trusted input. Otherwise derivation
   launders assertions into observations.
3. **What is the default for a consumer that forgets to declare?** Fail-open reproduces
   this incident in miniature.
4. **Migration.** Existing rows have no origin. Map known sources
   (`lora-gateway` → observed, `mesh-supervisor` → derived, `agent` → asserted) and decide
   what an *unrecognised* source becomes. Lowest trust is the safe answer and the opposite
   of today's behaviour.

Today a reflex rule will fire on a fact an agent asserted. That is the same hazard as the
phantom node, in a component that can actuate — it simply has not been triggered yet.

**Also open:** the escalation path has no *"is my intervention working?"* check. System 2
woke 23 times over 3.2 hours with the same reason, acted, changed nothing, and never
noticed the condition was unchanged. The novelty gate and rate limit worked exactly as
designed — they suppressed the noise. Nothing was watching for a stuck loop underneath it.

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
four missing playbooks + the enforcing test).*

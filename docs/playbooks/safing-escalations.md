# Safing escalation playbooks

The four non-mesh safing rules that wake System 2. Each section is the long form of the
triage directive carried in the rule's `Action::Escalate` reason (`src/agent/safing.rs`);
the mesh playbook has its own file, [mesh-node-lost.md](mesh-node-lost.md).

## The rule these follow

**Every step names a tool with a contract, never an action with a free-form target.**

That rule was bought with an incident. On 2026-07-17 the mesh playbook's step 3 said
"record the loss to world memory" — an unbounded write with no named destination or
schema. Steps 1 and 2 named `mesh_status` and `mesh_command` and could not fail this way.
Step 3 handed the model an untyped store and asked it to invent an entity name and a
payload shape at runtime; it filed a note at `mesh.escalation_status`, the mesh supervisor
read it back as a live node, and the fleet alarmed on a phantom for 100 minutes. The
model's judgement was sound and the note was true. The interface was wrong.

Two consequences that apply to all of these:

- **Findings are recorded only through `record_incident`.** It owns the entity name,
  schema, timestamp and provenance. Never hand-write a finding into `world_memory`.
- **Nobody needs to be alerted by hand.** Every escalation fans out to the notification
  log-of-record and any configured webhook *before* System 2 is woken. There is no
  operator-alert tool, and asking for one invites the model to invent a channel.

And one that applies to the evidence itself: **record only what was actually observed.**
The audited note carried `"last_seen":"2023-05-10T19:06:17.874Z"` — a fabricated date,
three years off, in ISO format where the system uses epoch millis, sitting beside a real
RSSI reading. A missing field is recoverable; an invented one is not.

---

## Battery critical

**Rule:** `safe-power-critical-escalate` — `power.mode == critical`.

Safing has **already acted**: `safe-power-critical-stop` stops the configured actuator on
the same condition. Nothing needs re-actuating, and re-actuating is the one clearly wrong
move here.

1. **Read the pack.** `power` action `status` — latest reading plus derived mode. Then
   `power` action `history`: a pack under genuine load declines; a single dipped sample
   that recovered is a different story.
2. **Record.** `record_incident`, subject = the pack or device.
   - Decline is real → `status: confirmed`.
   - Already recovered, one bad sample → `status: false_alarm`.
   - Evidence: the `soc_pct` readings and their times.

**Guardrails.** Do not re-enable the stopped actuator. Do not infer a hardware fault from
one sample.

---

## Audio alarm

**Rule:** `safe-audio-alarm-{stream}` — `audio.{stream}` label == `alarm`.

An alarm sound is evidence about **the environment**, not a device fault. The system heard
something; it does not know what is happening.

1. **Read the detection.** `hear` action `current` on that stream — label and confidence
   (events are classified reliable only once confidence clears the floor). Then `hear`
   action `history`: one hit versus a repeating pattern is the whole question.
2. **Record.** `record_incident`, subject = the stream.
   - Reliable and recurring → `status: confirmed`.
   - Isolated, low confidence → `status: false_alarm`.
   - Evidence: confidence values and timestamps.

**Guardrails.** **Do not actuate anything in response to a sound.** A sound is not a
command, and a misclassification that moves hardware is a much worse failure than a
misclassification that gets written down.

---

## Sensor unreliable

**Rule:** `safe-sensor-unreliable-{quantity}` — `sensor.{quantity}` quality ==
`out_of_range`.

The distinctive thing here: **the reading is the suspect.** Every other escalation asks
what the world is doing; this one asks whether the instrument can be believed. So nothing
downstream of that value should be acted on until it is settled.

1. **Read the quality.** `sense` action `current` for live quality, action `history` for
   when it degraded.
2. **Check the blast radius.** `sense` action `anomalies` lists every quantity currently
   out-of-range or stale. Several going bad at the same moment points at the node or its
   bus, not at one sensor — the same "suspect the shared thing" reasoning as a multi-node
   mesh loss.
3. **Record.** `record_incident`, subject = the quantity.
   - Genuine sensor fault → `status: confirmed`.
   - Quality already back to ok → `status: false_alarm`.

**Guardrails.** Do not act on the suspect value, and do not act on any conclusion that
depends on it — including an overheat escalation for the same quantity (see below).

---

## Overheat

**Rule:** `safe-overheat-{quantity}` — `sensor.{quantity}` numeric value over a threshold.

**Check the sensor before believing the heat.** This rule reads a raw number; it does not
consult quality. A failed sensor pegged high looks exactly like a real thermal event.

1. **Qualify the reading.** `sense` action `current` for that quantity. Quality
   `out_of_range` or `stale` → this is a sensor fault, not a heat event; treat it as
   [Sensor unreliable](#sensor-unreliable). Then `sense` action `history`: a steady climb
   favours real heat, a single-step jump to an implausible value favours the sensor.
2. **If the reading is trustworthy, treat the heat as real.** Diagnose with reads. Any
   actuation stays Track-0 gated by the node's own limits.
3. **Record.** `record_incident`, subject = the quantity.
   - Real heat → `status: confirmed`.
   - Bad sensor → `status: false_alarm`.
   - Evidence: the values, their times, and the quality flag.

**Guardrails.** Never bypass Track 0 to respond to a temperature. A node you cannot
observe reliably is a node you should not actuate — which is exactly the case when the
sensor is the thing that failed.

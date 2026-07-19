# Playbook — Mesh node presumed lost

**Trigger:** the `safe-mesh-node-lost` reflex escalated to System 2 because the mesh
supervisor marked one or more LoRa nodes *presumed lost* (`mesh.escalated_count >= 1`).

This is the standing procedure the agent follows when that wake arrives. The escalation
reason itself carries the short form of steps 1–3; this document is the full version.

## Context you already have

- **`mesh_status`** (read-only) — a fleet summary from world memory: per-node health
  (`online` / `degraded` / `offline`), whether it's `escalated` (presumed lost), link
  RSSI, last message type, seconds since last heard, and last command outcome, plus
  counts.
- **`mesh_command`** — send a command to a node over LoRa (`node_id`, `command`, `args`).
  The node executes it under its own on-MCU **Track 0** gate; the reply returns over the
  mesh as a `mesh.<node>.cmd_result` fact.
- **`world_memory`** — query time-valid facts, e.g. `mesh.<node>` (liveness),
  `mesh.<node>.health`, `mesh.<node>.escalation`, `mesh.<node>.cmd_result`. Read-only for
  the mesh: `mesh.*` is written by the LoRa gateway and the supervisor, and anything
  written there is read back as real node state. The tool will refuse writes to it.
- **`record_incident`** — write down what you concluded (`subject`, `status`, `detail`,
  `evidence`). It owns the entity name, schema, timestamp and provenance, so this is the
  only thing you should use to record a finding.

## Procedure

1. **Identify.** Call `mesh_status`. Note every node whose `escalated` is true (and any
   `offline`). For each, look at `age_s` (how long since last heard), `rssi_dbm` (was the
   link already weak?), and `last_type` / `last_cmd_ok` (what was it last doing?). Note
   `last_cmd_ok` is a *health* reading, not the raw reply: a node that refused a command
   on its Track 0 policy shows as ok, because refusing is the node working correctly.

2. **Confirm reachability.** For each lost node, `mesh_command` it a lightweight
   `capabilities` ping:
   `{ "node_id": "<node>", "command": "capabilities" }`.
   Once escalated the supervisor drops to a slow background probe (5 min by default), so
   this is your deliberate, immediate check rather than a duplicate of its retry loop.

3. **Branch on the result** (a reply appears as a fresh `mesh.<node>.cmd_result` /
   `mesh.<node>` fact within a few seconds):
   - **It answers** → the link recovered. The supervisor will clear the escalation on its
     next tick. `record_incident` with `status: resolved` and stop. No further action.
   - **Silence** → treat as a real loss. `record_incident` with `status: unresolved`, the
     `mesh_status` readings as `evidence`, and a one-line `detail`. Do **not** keep
     hammering the node — one confirming ping is enough.

   An operator is alerted automatically: every escalation already fans out to the
   notification log-of-record and any configured webhook before you are woken. You do not
   need to send an alert, and there is no tool for you to do so.

   Record only what you actually observed. If you did not see a last-seen time, leave it
   out rather than supplying a plausible one — an invented value in the record is worse
   than a missing one.

4. **Consider the blast radius.** If *several* nodes went lost at once, suspect the
   gateway/base-station or the shared RF environment rather than the nodes themselves —
   check whether the gateway Heltec is still emitting keepalives before declaring a fleet
   outage.

## Guardrails

- **Never bypass Track 0.** Every `mesh_command` is gated on the node's own allow-list /
  range / rate limits. A lost node coming back does not relax that.
- **No actuation to "wake" a node.** Diagnose with reads/pings (`capabilities`,
  `sensor_read`), not `gpio_write` — a node you can't observe is a node you shouldn't
  actuate.
- **One confirming ping, then decide.** The value here is a fast, bounded verdict
  (recovered vs. lost, recorded), not an open-ended retry loop — the supervisor already
  owns the retry policy.
- **Record findings only through `record_incident`.** Never hand-write a finding into
  world memory. On 2026-07-17 a note filed at `mesh.escalation_status` was read back by
  the supervisor as a live node, escalated as lost, and alarmed on for 100 minutes.

## Operator note

To make this procedure standing context for the agent (rather than delivered per-wake in
the escalation reason), paste the Procedure + Guardrails sections into your `SOUL.md`
under an "Operating playbooks" heading.

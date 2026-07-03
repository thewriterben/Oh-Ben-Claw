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
- **`world_memory`** — query/record time-valid facts, e.g. `mesh.<node>` (liveness),
  `mesh.<node>.health`, `mesh.<node>.escalation`, `mesh.<node>.cmd_result`.

## Procedure

1. **Identify.** Call `mesh_status`. Note every node whose `escalated` is true (and any
   `offline`). For each, look at `age_s` (how long since last heard), `rssi_dbm` (was the
   link already weak?), and `last_type` / `last_cmd_ok` (what was it last doing?).

2. **Confirm reachability.** For each lost node, `mesh_command` it a lightweight
   `capabilities` ping:
   `{ "node_id": "<node>", "command": "capabilities" }`.
   The supervisor already stopped auto-pinging once it escalated, so this is your
   deliberate check.

3. **Branch on the result** (a reply appears as a fresh `mesh.<node>.cmd_result` /
   `mesh.<node>` fact within a few seconds):
   - **It answers** → the link recovered. The supervisor will clear the escalation on its
     next tick; note the recovery and stop. No further action.
   - **Silence** → treat as a real loss. Record a short note to `world_memory` (what was
     lost, last-known state, time) and **alert an operator** through whatever notification
     channel is configured. Do **not** keep hammering the node — one confirming ping is
     enough.

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
  (recovered vs. lost + alert), not an open-ended retry loop — the supervisor already owns
  the retry policy.

## Operator note

To make this procedure standing context for the agent (rather than delivered per-wake in
the escalation reason), paste the Procedure + Guardrails sections into your `SOUL.md`
under an "Operating playbooks" heading.

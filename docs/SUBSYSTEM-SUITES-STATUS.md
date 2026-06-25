# Subsystem Suites — As-Built Status

*Companion to `SUBSYSTEM-SUITES.md` (the plan) and `SUBSYSTEM-SUITE-CONTRACT.md` (the contract). This file records what is **implemented and tested** in the `oh-ben-claw` crate, as of the current build.*

Six capability suites now share one spine: **perceive → remember → react → act**. Each suite perceives its domain, records bitemporal facts into world memory, and (where applicable) acts through a Track 0–bounded sink. System 1 reflexes read the recorded facts and fire deterministic safing actions with no LLM in the loop.

```
 perceive            remember                 react (System 1)        act
 ────────            ─────────                ────────────────        ───
 suite controller →  WorldMemory fact      →  ReflexEngine.tick    →  ActionSink / Sink
 (ingest/observe)    (entity, value, time)    (Condition → Action)    (spine / gate / dry-run)
```

## Suites

| Suite | Module | Perceive | World-memory hooks | Act |
|---|---|---|---|---|
| **Vision** | (ClawCam, external) | detections | `subject:*` | — |
| **Movement** | `src/movement` | — | `actuator.{name}` | `move_actuator` tool + reflex `Move`; `ActuatorSink` (`LoggingActuatorSink` / `SpineActuatorSink`); Track 0 gate |
| **Sensing** | `src/sensing` | quality-classified readings | `sensor.{quantity}` (incl. `quality`) | — |
| **Audio** | `src/audio/suite` | heard events (reliability) | `audio.{stream}`, `speech.last` | `speak` tool; `SpeechSink` (`LoggingSpeechSink` / `SpineSpeechSink`) |
| **Power** | `src/power` | battery telemetry | `power.battery`, `power.mode` | — |
| **Comms** | `src/comms` | per-link telemetry + aggregate | `link.{name}`, `net.mode` | — |

Each suite records into world memory non-destructively (bitemporal `observe`), so every fact is a historical observation, never an overwrite.

## New MCP tools (this work)

All registered in `src/main.rs` behind their suite's `[config]` flag; gated by the unified approval layer via `risk_class()`.

| Tool | Suite | Actions | Risk class | Approval |
|---|---|---|---|---|
| `sense` | Sensing | ingest, current, history, anomalies | `safe()` | none |
| `hear` | Audio | observe, current, history | `safe()` | none |
| `speak` | Audio | (speak) | `physical(reversible, Low)` | recorded, not per-call |
| `power` | Power | report, status, history | `safe()` | none |
| `comms` | Comms | report, status, history | `safe()` | none |
| `move_actuator` | Movement | servo_angle, motor_speed, stop | `physical(reversible, High)` | **per-call** |

Reads and reversible memory appends are `safe()`. Physical effects declare a blast radius: speech is `Low` (recorded, allowed); actuation is `High` (per-call approval, never auto-granted to `forever`).

## Reflex engine (System 1)

`src/agent/reflex.rs`. A rule is *when `Condition` holds, do `Action`*, subject to debounce/rate and an escalation budget. `tick()` snapshots the referenced world-memory entities and evaluates.

**Conditions**
- `Sensor { entity, op, value }` — numeric compare (reads a number, bool, numeric string, or the `value` field of an object fact).
- `GpioEq { entity, value }` — integer equality.
- `State { entity, field?, equals }` — **categorical** match on a fact's string value or a nested string field. This is how reflexes match the suites' mode hooks (`power.mode`, `net.mode`, `audio.{stream}` label, sensor `quality`).
- `And { all }` / `Or { any }`.

**Actions:** `GpioWrite`, `Publish`, `Escalate`, `Move` (typed, gate-bounded).

**Snapshot** carries both `nums` (for `Sensor`/`GpioEq`) and `vals` (raw fact values, for `State`).

## Safing rules

`src/agent/safing.rs`. Canonical, debounced rules that turn the mode hooks into reactions. Enabled with `[reflex] safing = true`; merged into the operator's configured rules in `main`.

| Rule id | Trigger | Action |
|---|---|---|
| `safe-power-critical-escalate` | `power.mode == critical` | Escalate |
| `safe-power-critical-stop` | `power.mode == critical` | `Move::Stop` (only if `[reflex.safing_stop_actuator]` set) |
| `safe-power-low` | `power.mode == low` | Publish `obc/safing` `{action: shed_load}` |
| `safe-net-offline` | `net.mode == offline` | Publish `obc/safing` `{action: degraded_mode}` |
| `safe-net-degraded` | `net.mode == degraded` | Publish `obc/safing` `{action: reduce_bandwidth}` |
| `safe-audio-alarm-{stream}` | `audio.{stream}` label `== alarm` | Escalate (per `safing_alarm_streams`) |
| `safe-sensor-unreliable-{q}` | `sensor.{q}` quality `== out_of_range` | Escalate (per `safing_unreliable_sensors`) |
| `safe-overheat-{q}` | `sensor.{q}` value `>` threshold | Escalate (per `[[reflex.safing_overheat]]`) |

Escalations are rate-capped by the controller's `EscalationBudget`; `Move::Stop` passes through the movement controller's Track 0 gate. Every safing rule is debounced (default 5 s) so a persistent bad mode does not spam actions.

## Sinks (act side)

Both follow the same pattern: the controller owns policy (record, then emit); the sink owns the physical channel. `main` selects the live sink when the spine is connected, else a dry-run logging sink.

| Sink | Live | Dry-run | Channel |
|---|---|---|---|
| Movement `ActuatorSink` | `SpineActuatorSink` | `LoggingActuatorSink` | `invoke_tool(node, servo_angle/motor_speed/stop)` |
| Audio `SpeechSink` | `SpineSpeechSink` | `LoggingSpeechSink` | publish `obc/speech` |
| Reflex `ActionSink` | `SpineActionSink` (+ `MovementActionSink`) | `LoggingActionSink` | gpio/publish/escalate/move over spine |

Live sinks are best-effort: a publish/invoke failure is logged, not propagated, so a transient spine outage never stalls a controller or reflex tick.

## Configuration summary

```toml
[perception]
world_memory = true            # required by all suites below

[sensing]
enabled = true
[[sensing.quantity]]
name = "temperature"; min = -40; max = 85; max_staleness_ms = 10000

[audio_suite]
enabled = true; voice = "nova"; min_confidence = 0.6

[power]
enabled = true; low_pct = 20; critical_pct = 10

[comms]
enabled = true; max_latency_ms = 500

[reflex]
enabled = true
safing = true
safing_alarm_streams = ["mic0"]
safing_unreliable_sensors = ["temperature"]
[reflex.safing_stop_actuator]
name = "arm"; channel = 0
[[reflex.safing_overheat]]
quantity = "cpu_temp"; threshold = 80
```

## Test coverage

Each suite ships unit tests (controller classification + world-memory recording) and tool tests (action routing + risk class). `safing.rs` adds **end-to-end** tests that drive a real suite controller → world memory → `ReflexEngine::tick` and assert the correct safing action fires (and that healthy state fires nothing). The reflex engine adds `State`-condition tests including a world-memory fire.

## Open follow-ups

- Render `obc/speech` on a speaker node / TTS bridge; render `obc/safing` advisories into actual load-shedding.
- Promote `move_actuator` from gate-checked dispatch to closed-loop (feedback) movement (suite §6 Accelerate, L3).
- Regenerate the cross-repo tool registry SSOT (`registry.json`) to include `sense`/`hear`/`speak`/`power`/`comms`.

# Oh-Ben-Claw — Embodied Control Architecture

*Capstone reference for the embodied stack. Companion to `SUBSYSTEM-SUITES-STATUS.md` (the suites) and `SUBSYSTEM-SUITE-CONTRACT.md` (the contract).*

Oh-Ben-Claw's embodied system is **four layers of control over one shared memory**. Each layer runs on its own timescale and degrades independently: if a higher layer stalls, the lower ones keep the platform safe. Everything reads and writes **bitemporal world memory**; nothing actuates without passing a **Track 0** safety gate.

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ Layer 4 — DELIBERATION   mission sequencer (guarded multi-step missions)  │  ~0.5 s
 ├──────────────────────────────────────────────────────────────────────────┤
 │ Layer 3 — NAVIGATION     SLAM pose-graph · occupancy mapping · A* · drive │  ~0.5 s
 ├──────────────────────────────────────────────────────────────────────────┤
 │ Layer 2 — REFLEX         System 1 rules + safing (+ recovery), escalation │  ~1 s / sub-tick
 ├──────────────────────────────────────────────────────────────────────────┤
 │ Layer 1 — SUITES         vision · sensing · audio · power · comms · move  │  event / poll
 ├──────────────────────────────────────────────────────────────────────────┤
 │ Layer 0 — FIRMWARE       on-MCU reflex + self-safing (battery, link)      │  ms, host-independent
 └──────────────────────────────────────────────────────────────────────────┘
            shared substrate:  WORLD MEMORY (bitemporal)  +  TRACK 0 SAFETY GATE
```

## The shared substrate

**World memory** (`src/memory/world`) is a bitemporal fact store: every observation is `(entity, value, valid_from, ingested_at, source)`, appended never overwritten. It is the *only* coupling between layers — suites write facts, reflexes and missions read them, navigation writes pose and reads goals. Key entities:

| Entity | Writer | Reader |
|---|---|---|
| `sensor.{quantity}` (+ `quality`) | sensing | reflex, navigation |
| `power.mode`, `power.battery` | power | reflex safing, mission guards |
| `net.mode`, `link.{name}` | comms | reflex safing |
| `audio.{stream}`, `speech.last` | audio | reflex, mission |
| `actuator.{name}` | movement | reflex, agent |
| `sensor.pos_*` | SLAM / pose fusion | navigation |
| `nav.pose`, `nav.status`, `nav.slam` | navigation | mission, agent |
| `mission.status` | mission | agent, operator |

**Track 0** (`src/security/limits`) is the deterministic safety gate: `SafetyGate::check(node, tool, pin, value, now)` enforces allowed pins, value ranges, and rate limits *before* any actuation. It runs host-side **and** mirrored on the MCU. No layer can bypass it — a reflex `Move`, a navigation steer command, and a mission's drive all pass the same gate.

## Layer 1 — Suites (perceive · remember · act)

Seven capability suites, each conformant with the Subsystem Suite Contract: vision (ClawCam), sensing, audio, power, comms, movement, and navigation (the *fusing* suite). Each perceives its domain, records bitemporal facts, and — where it acts — does so through a pluggable, Track 0–bounded sink. See `SUBSYSTEM-SUITES-STATUS.md` for the full table, tool registry, and risk classes.

## Layer 2 — Reflexes + safing (System 1)

`src/agent/reflex` evaluates rules against a world-memory snapshot each tick: numeric `Sensor`/`GpioEq` conditions and categorical `State` conditions (matching the suites' mode hooks). Actions: `GpioWrite`, `Publish`, `Escalate` (rate-capped by an escalation budget), `Move` (gate-bounded).

`src/agent/safing` adds the canonical, debounced safing rules — power critical/low, net offline/degraded, audio-alarm, out-of-range sensor, overheat — and their **recovery** counterparts that release safing when modes normalize. A `SafingSink` taps the `obc/safing` advisories *in process*, flipping a shared `SafingState` so the host actually backs off (e.g. the ClawCam poll sheds load on low battery and resumes on recharge). Fire counts surface on the gateway `/metrics`.

## Layer 3 — Navigation (the localization → mapping → planning → driving column)

A full, drift-corrected navigation stack, all over world memory:

- **SLAM back end** (`navigation/slam`) — a 2D pose-graph with odometry + loop-closure edges; anchored Gauss-Seidel relaxation distributes accumulated drift when a revisited place is recognized, and writes the **corrected** pose to `sensor.pos_*`.
- **Pose fusion** (`navigation/pose_fusion`) — weighted multi-source localization (circular heading mean) into the same canonical pose entities.
- **Online mapping** (`navigation/mapping`) — Bresenham ray-casting turns range scans into the occupancy grid (clear free space, mark hits, sticky obstacles).
- **A* planning** (`navigation/planning`) — plans an obstacle-free path over the grid, simplified to turn-point waypoints.
- **Driving** (`navigation`) — a steer/drive controller follows the waypoint queue toward each goal, every command gate-bounded; `nav.status` carries a categorical state reflexes can watch.

The `navigate` tool plans around obstacles transparently; `nav_status` observes/stops; `nav_map` builds the map.

## Layer 4 — Deliberation (mission sequencer)

`src/mission` runs one guarded mission at a time: an ordered list of steps (`navigate_to`, `wait`, `speak`, `record`, `await_state`) executed reactively, one per tick. Every tick first checks **guards** (the reflex `Condition` grammar) and a tripped guard **preempts** the mission and halts the platform. Missions compose the suites with no new machinery — `navigate_to` drives Layer 3, `speak` drives audio, `await_state` blocks on world memory. The `mission` tool (approval-gated) starts a named mission; `mission_status` (always safe) observes or aborts.

## Layer 0 — Firmware autonomy

The MCU is not a dumb actuator. It runs a mirror of the reflex engine and **built-in safing** that needs no host: a battery watchdog cuts power-hungry loads on critical charge, and a link watchdog enters offline-safing when the host goes silent. Both are ordinary numeric reflex rules bounded by the on-MCU Track 0 gate; host-pushed rules merge *after* the built-ins so a node never loses self-protection.

## Safety model (defense in depth)

1. **Risk-classed tools** — physical/high-blast actions (`move_actuator`, `navigate`, `mission`) are approval-gated; reads and stops are always allowed.
2. **Track 0 gate** — deterministic per-call bounds on every actuation, host and MCU.
3. **Reflex safing** — sub-second, LLM-free reactions to bad modes, escalation-budgeted.
4. **Mission guards** — deliberative preemption of multi-step plans.
5. **Firmware self-safing** — the node protects itself when the host or spine is gone.

A failure at any layer is contained by the one below it.

## Phase 19 — predictive & autonomous layers

Three control *modes* now sit over the suites, each on a different relationship to time:

- **Reactive** (Layer 2 reflexes) — act on the present.
- **Anticipatory** (`src/foresight`, "Track 1") — act on the *predicted* future. A `Forecaster` fits trends over the bitemporal history and rules fire before a threshold crossing (e.g. battery predicted critical). The same `ActionSink`/escalation-budget machinery as reflexes; predictions recorded to `foresight.{entity}`.
- **Deliberative** (Layer 4 missions) — execute multi-step plans.

And two capabilities make the system *self-improving* and *self-directed*:

- **Self-authored reflexes** (`src/learning`) — mine the history for conditions that repeatedly preceded a bad outcome, propose predictive rules with support/confidence, and — **only after approval** — activate them live into the foresight engine. The system learns what to anticipate, with a human in the approval seat.
- **Autonomous exploration** (`src/navigation/exploration`) — frontier-based self-mapping: head to the nearest reachable known/unknown boundary, scan, repeat, until the reachable space is mapped. Composes SLAM + mapping + A* + drive with no human waypoints.

Localization is also now *uncertainty-aware*: a particle filter (`src/navigation/particle`) carries a belief cloud and reports a position **spread**, so the stack can act on how sure it is about where it is, rather than treating pose as exact.

## ClawCam — a bidirectional embodied subsystem

ClawCam (the vision subsystem, reached over the MCP stdio/HTTP bridge) is wired as a full **perceive → remember → react → act** participant, not just a detection feed. One shared `clawcam_client` carries both directions (`src/vision/`):

**Read (perceive → remember):**
- *Detections* fold into `vision.subject.{species}` facts (`clawcam_ingest`), and a rolling `vision.count.{subject}` counter is maintained so foresight can trend the **detection rate**.
- *Node health* (`poll_health`) folds into namespaced `clawcam.node.{id}` facts — kept deliberately **separate** from the robot's own power/comms suites, since a camera is a distinct body whose battery must not flap the robot's `power.mode`. Per-source converters (`node_health_to_battery`/`_to_link`) exist for running a per-camera controller when wanted.
- *Audio classifications* (`poll_audio`) feed the audio suite as distinct `audio.clawcam:{node}` streams, so a glassbreak becomes a safing-classifiable alarm.
- *Analytics reports* (`poll_analytics`) fold the gateway's daily aggregates into `clawcam.analytics.*` facts on a slow cadence (`clawcam_analytics`): the latest day's anomaly z-score, independent-encounter totals, and confidence-calibration state. Absence of data records **no fact** — an empty report is not a calm day.

**React (vision drives behavior):** `clawcam_rules` authors three libraries that merge into the live engines — `vision_security_rules` (a confirmed sighting of an alert subject escalates through Track 0, optionally firing a capture), `vision_foresight_rules` (a rising sighting rate escalates *ahead* of the peak), and `vision_analytics_rules` ("today is weird": an unusually **quiet** day escalates as a possible knocked-over/obstructed camera, a **spike** as an activity surge, and calibration drift as a threshold-retune prompt). All reuse the exact reflex/foresight rule types, bounded by the escalation budget.

**Act (close the loop):** `ClawCamActionSink` wraps the reflex sink and intercepts `clawcam/cmd/*` publishes (`map_command`), translating them into ClawCam's gated write tools (`capture_now`, `set_device_state`, `create_alert_rule`) over the same bridge — still passing ClawCam's own approval model. OBC can now *command* the cameras, not only read them.

**Spatial (built, wired on demand):** `clawcam_spatial` maps fixed cameras to world positions and stamps a hazard disc into the nav grid on a detection (`mark_detection_hazard`), so a static camera reshapes a mobile robot's path via costmap inflation. The core is tested; it is wired only for deployments where a mobile robot shares space with the cameras.

All of the above is config-gated (`[perception.clawcam_poll]`, `[perception.vision_rules]`), default off, and composes on the one world memory behind the one Track 0 gate.

## End-to-end verification

`tests/embodied_full_stack.rs` exercises the whole stack in one scenario: a mission to cross a room plans around a mapped wall and issues a gated drive command; as the battery drains, safing engages load-shedding; at critical charge the mission guard preempts and halts navigation; on recharge, safing recovers — all through one world memory. `tests/embodied_hil_loop.rs` proves the vision seam end to end with nothing mocked: a ClawCam detection flows through the real ingest into world memory, the hazard policy turns a verified in-corridor animal into occupancy, navigation re-plans a detour, and a Track 0–bounded drive command is issued. Each layer also carries its own unit + integration tests.

# Bench Walkthrough (Advanced) — Beyond the Minimum-Viable Bench

Stages **8–12**, continuing `BENCH-WALKTHROUGH.md` (Stages 0–7) onto the BOM's
advanced stations: **D GNSS**, **E backhaul**, **F aerial**, **G edge-AI**, and a
full-grid rehearsal. Wiring for everything here: **`BENCH-ADVANCED-WIRING.svg`**
(sections ④–⑦ match the stage numbers below).

**Prerequisites:** Stages 0–5 signed off (dry bench, mesh spine into world memory,
System 2 wakes, two-way mesh under Track 0). Stage 6 (camera) is only needed for
Stage 11's acceleration payload.

**Honesty about status** (mirrors the BOM's status column): Station D's decoder,
the comms suite, and the aerial adapter are **DRIVER** — real code, tested. The
satellite path and the live MAVLink ingest bridge are **PLANNED** — those stages
tell you exactly where the walkthrough ends and the firmware work begins.

**Bench rules carry over** — plus four new ones:

1. ⚠️ Antenna-before-power now also applies to **LTE and SiK** radios.
2. The SIM7600 needs its **own 5 V ≥ 2 A supply** — brownouts from USB power look
   like modem bugs.
3. **Props never go on indoors.** The aerial stage is done entirely props-off
   (and SITL-first, for free).
4. ⚠️ **915 MHz is shared.** The SiK telemetry pair (Stage 10) and the LoRa mesh
   (Stages 1–5) occupy the same US ISM band a metre apart on a bench — expect
   mutual interference. Don't run SiK hardware during a mesh soak; for
   concurrent operation (Stage 12) use SITL for the aerial arc, or buy the
   **433 MHz** SiK variant.

---

## Stage 8 — Station D: GNSS into the shared frame *(wiring ④)*

*The point: a bare GPS module becomes a fleet node in the **same site frame** as
everything else — the G0 anchor from Stage 0.5 pays off here.*

**8.1 Dry run first (no hardware).** The decoder is pure code — feed the agent a
canned sentence in chat:

> Use gnss_fix with sentence "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47"

Expect a decoded fix (48.1173 N, 11.5186 E, 8 sats) and `"frame": "site"` — the
fix landed in the anchored bench frame automatically. (If it says `"fix"`, no
site is anchored — redo Stage 0.5.)

**8.2 Wire the NEO-M8N** per wiring ④: VCC→3V3, GND→GND, module TX→CP2102N RXD.
**3V3 logic only.** Active antenna on, near a window.

**8.3 Cold start.** Open the CP2102N's COM port at 9600. NMEA streams immediately;
a first fix takes 1–10 min with sky view. You have a fix when `$GPGGA` shows
quality `1` and ≥4 satellites.

**8.4 Live fix through the tool.** Paste a real `$GPGGA` line from your stream
into `gnss_fix` (as in 8.1). The returned `node_state` x/y are metres from your
bench-site origin — sanity-check against reality (your desk should be metres,
not kilometres, from where you anchored).

**8.5 Integrated path (optional).** A T-Beam runs GPS + LoRa in one: flash the
lora-node bridge and the fix rides the existing mesh spine — no extra wiring.

**PASS Stage 8:** live GGA decoded; `frame: "site"`; ENU offset plausible for
your bench's real position.

---

## Stage 9 — Station E: backhaul health drives safing *(wiring ⑤)*

*The point: the brain treats connectivity as a sensed quantity — link decay walks
through `net.mode` → safing reflexes → (if it stays bad) a System 2 wake.*

**9.1 Wire the SIM7600** per wiring ⑤: own 5 V ≥ 2 A supply, SIM inserted before
power, LTE + GPS antennas on, USB to the host (it enumerates several serial
ports; the AT port is typically the second).

**9.2 Modem alive.** Any serial terminal on the AT port:

```
AT          → OK
AT+CSQ      → +CSQ: <rssi>,<ber>     (rssi 10–31 is usable)
AT+CREG?    → registered on network
```

**9.3 Feed the comms suite.** With `[comms]` enabled, the `comms` tool ingests
`LinkReading`s (rssi/latency/loss) and derives `net.mode` in world memory. Feed
it your real `+CSQ` value (via chat/tool) and confirm `net.mode = online`.

**9.4 The bench trick — degrade it.** Unscrew the LTE antenna:

1. `+CSQ` collapses → ingest the new reading → `net.mode` → `degraded`, then `offline`.
2. The standard safing rules fire (`net_offline_safe`): the SafingState sheds
   load; `status` shows the advisory.
3. Leave it offline past your thresholds → escalation → notification log →
   **one** System 2 wake (Stage 4 machinery, new trigger).
4. Antenna back on → readings recover → `net_recovered_clear` clears the advisory.

**9.5 Satellite (exploratory — PLANNED).** RockBLOCK/Swarm hardware wires the
same way (serial + sky view) but has **no code path yet** (`sat_gateway.rs` is a
G6 design item). Bench goal today is only: modem powers, registers, sends one
test SBD/packet from its vendor tool. Ingest comes with G6.

**PASS Stage 9:** the full arc — online → degraded → offline → safing → single
System 2 wake → recovery clear — driven by a real antenna pull.

---

## Stage 10 — Station F: the aerial tier, SITL first *(wiring ⑥)*

*The point: a drone is just another fleet node — its telemetry maps into the same
NodeState and the anchored Site boundary is its geofence. Validate all of that
for $0 in SITL before any hardware.*

**10.1 SITL (do this first, hardware optional).** Run PX4 SITL (jMAVSim or
Gazebo) on the host. Take a telemetry sample (lat/lon/alt/battery/armed/mode)
and feed it through the aerial adapter path — in chat:

> Build an AerialTelemetry for id "uav-1" at [lat just outside the bench site boundary], battery 78, armed true, mode "AUTO", and check it against the anchored site.

Expected: the adapter maps it to a fleet `NodeState` in the site's ENU frame, and
the geofence check (`Site::contains`) reports **outside** → this is your alert
condition. Repeat with a point inside → contained.

**10.2 Geofence reflex.** Add a reflex rule on the published geofence fact so an
outside-boundary report escalates — then verify the System 2 wake carries the
"uav-1 outside site" reason. (Same novelty gating: circling outside the fence
doesn't spam wakes.)

**10.3 Hardware (when ready — props off).** Pixhawk 6C per wiring ⑥: GPS/compass
on GPS1, SiK air radio on TELEM1 (57600), ground SiK on host USB, bench power
via USB. Confirm the ground radio streams MAVLink (QGroundControl sees the
vehicle).

**10.4 The honest boundary:** a live **MAVLink→AerialTelemetry ingest bridge is
PLANNED** — the adapter and geofence are done (10.1 proves them), but wiring the
MAVLink stream in is the next firmware/host task. When it lands, 10.1's check
runs continuously against the real link.

**PASS Stage 10:** SITL/synthetic telemetry lands in the fleet frame; outside-
boundary triggers exactly one wake; (hardware) ground station sees MAVLink.

---

## Stage 11 — Station G: edge-AI acceleration *(wiring ⑦)*

*The point: perception gets fast enough to be a reflex input. Start with the
cheapest accelerator (Coral USB) on the Stage 6 gateway.*

**11.1 Attach one accelerator** to the Pi 5 per wiring ⑦ (Coral on USB 3.0 — the
blue port — or the AI HAT+ on the GPIO header with its standoffs).

**11.2 Vendor smoke test.** Run the vendor's classify/detect demo (Coral's
`pycoral` examples / Hailo's `hailortcli benchmark`). Note images/sec vs CPU —
that ratio is the whole reason this station exists.

**11.3 ClawCam payload.** Point the ClawCam detector pipeline at the accelerator
(MegaDetector-class model) and re-run a Stage 6 capture: detection latency drops
from seconds to tens of milliseconds; `clawcam.*` analytics keep flowing to the
brain unchanged (the contract is the same — only faster).

**11.4 Brain-side checks.**
- Fleet app → Boards → **Refresh from Gateway**: the live registry catalogues
  your accelerator (`edge_tpu` / `hailo` / `vpu` token).
- New-scheme wizard: add the accelerator + desire **AcceleratedInference** → the
  planner reports the desire satisfied (no gap suggestion).

**PASS Stage 11:** measured speed-up over CPU; ClawCam runs on it; the planner
recognizes the capability.

---

## Stage 12 — Full-grid rehearsal + sign-off

*Everything above, exercised together as a miniature Conservation Grid.*

**12.1 Plan the bench as a site.** In chat:

> Use plan_site with the bench site boundary and budget 4.

You get an optimized, mesh-connected placement with per-node lat/lon + ENU.
Physically move your nodes to (a scaled version of) those spots on the bench.

**12.2 Scheme with positions.** Generate a deployment scheme in the app with the
site layout attached — each agent card shows its position; push it to the
gateway (staged for review, Operate token).

**12.3 One-hour full-stack soak.** Everything on: mesh telemetry + GNSS fixes in
the site frame, backhaul online, camera trapping, accelerator classifying. Run
the mesh in its **Stage 3b topology** — base ↔ `heltec-relay` ↔ gateway — so the
soak exercises multi-hop, not just point-to-point. (Aerial arc via SITL per
bench rule 4, unless you have 433 MHz SiK.) Then inject exactly three faults,
ten minutes apart:

1. unplug **`heltec-relay`** — a mid-mesh partition: every node behind it drops
   at once (harder than losing a leaf; Stage 4 arc, plural),
2. pull the LTE antenna (Stage 9 arc),
3. feed one outside-boundary telemetry (Stage 10 arc).

**Expected:** three escalations, **three** System 2 wakes (distinct situations —
the novelty gate must not merge them, and repeats within each must not add
wakes), all three visible in the Fleet app, all physical commands you issue
along the way appearing in the signed action audit.

| # | Milestone | Stage | ✔ |
|---|---|---|---|
| 1 | Live GNSS fix in the anchored site frame | 8 | ☐ |
| 2 | Antenna-pull arc: degraded → offline → safing → 1 wake → clear | 9 | ☐ |
| 3 | SITL telemetry in fleet frame + geofence alert | 10 | ☐ |
| 4 | Accelerator speed-up measured + planner recognizes it | 11 | ☐ |
| 5 | Optimized placement generated and applied | 12 | ☐ |
| 6 | 3-fault soak: exactly 3 wakes, all audited | 12 | ☐ |

A bench that passes Stage 12 has rehearsed every arc the outdoor grid depends
on. The remaining gaps are known and named: `sat_gateway.rs` (G6) and the
MAVLink ingest bridge (G8) — both have their hardware validated and waiting.

---

## Appendix — config deltas over the Stage 0 template

```toml
# Stage 9 — comms suite (thresholds per your link; these are bench-lenient)
[comms]
enabled = true

# Stage 10 — geofence escalation (example; adapt entity to your publish topic)
#[[reflex.rules]]
#id = "uav-outside-site"
#when = { type = "state", entity = "aerial.uav-1.geofence", equals = "outside" }
#then = { type = "escalate", reason = "uav-1 outside site boundary" }
#debounce_ms = 30000
```

Everything else (mesh supervisor, notifications, System 2, gateway tokens) is
already armed by the Stage 0 `bench-config.toml`.

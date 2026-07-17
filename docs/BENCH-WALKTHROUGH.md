# Bench Walkthrough — Minimum-Viable Bench to Full Stack

A staged, checkable bring-up for the **Minimum-Viable Bench** kit
(`BENCH-TEST-HARDWARE.md` §⭐, ~$470): from an empty desk to a two-way LoRa mesh
whose escalations wake the LLM, with every physical action behind Track 0.

Each stage is independent and ends with a **PASS** gate — stop at any stage and
you still have a working milestone. Later stages assume earlier ones.

**Companion documents** (this walkthrough orders the work; these carry the depth):

| Doc | Use it for |
|---|---|
| `BENCH-TEST-HARDWARE.md` | The BOM — what to buy, per-station checklists |
| `BENCH-MVB-WIRING.svg` | The one-page wiring picture for everything below |
| `BENCH-MVB-PINOUT.svg` | The one-page pin-level diagram (bridge, LED, DHT22, I2C, PIR) |
| `BENCH-PINOUT-CARDS.md` | Print-and-tape pinout cards per board |
| `HARDWARE-TEST-WALKTHROUGH.md` | Exhaustive per-command detail for the control node (Phase A), mesh (B), fleet (C) |
| `PHASE-B-LORA-MESH.md` | LoRa mesh runbook: radio config, frame format, bridge wiring, troubleshooting |
| `playbooks/mesh-node-lost.md` | What System 2 (and you) should do when a node drops |
| `BENCH-WALKTHROUGH-ADVANCED.md` + `BENCH-ADVANCED-WIRING.svg` | Stages 8–12: GNSS, backhaul, aerial, edge-AI, full-grid rehearsal |

**Bench rules (always):**

1. ⚠️ **Never power a LoRa board without its antenna.** TX into an open port can
   destroy the SX1262's PA. Antenna first, every time.
2. **One change at a time.** Flash one board, verify, then the next.
3. **Label everything.** Node IDs on tape (`heltec-base`, `heltec-gw`,
   `heltec-relay`, `xiao-1`, `ws-ctl`, `eye-1`), and note each board's COM port
   on the label.
4. LiPo cells: charge supervised, store half-charged, nothing conductive on the bench mat.

---

## Stage 0 — Day zero: the dry bench (no new hardware)

Prove the entire host stack before anything arrives. Everything here runs on the
workstation.

**0.0 Toolchain pre-flight** — five checks so Stage 1 doesn't stall on tooling:

```powershell
cargo --version        # if "not recognized": use "$env:USERPROFILE\.cargo\bin\cargo.exe"
                       # or add that dir to PATH — applies to every cargo command below
espflash --version     # flashes both ESP32-S3 firmware targets (cargo run does it)
rustup target list --installed | Select-String xtensa   # esp toolchain (espup install)
python --version       # 3.11+ for the ClawCam gateway venv (Stage 6)
```

Plus, Windows only: the **Silicon Labs CP210x VCP driver** (Heltec USB serial —
Device Manager shows "CP210x" per board, not an unknown device). Arduino IDE is
needed **only** for the optional T-Beam path (Stage 8.5); nothing in Stages 0–7.

**0.1 Build + full verification**

```powershell
cd F:\Documents\GitHub\Oh-Ben-Claw
cargo test --workspace     # all suites green
cargo clippy --all-targets # zero warnings
```

**0.2 Bench config.** Create `bench-config.toml` from the template in Appendix A
(everything enabled but pointing at dry-run sinks; no serial ports yet).

**0.3 Start the brain + gateway**

```powershell
cargo run -- start --config bench-config.toml
```

Expect in the log: world memory active, reflex controller spawned, mesh status
tool active, System 2 wake channel armed, `Gateway listening`.

**0.4 Fleet console (read-only).** In the OBC-deployment-generator app → **Fleet**
tab → enter `http://<workstation-ip>:8080` → Connect. Status card shows the agent
running; metrics tick.

**0.5 Anchor the bench site.** In a chat session with the agent:

> Use the site_anchor tool to anchor a site with id "bench" at origin [your lat, your lon].

Then `oh-ben-claw status` / a `world_memory` query shows `geo.site` current. Every
pose and plan from here on shares this frame.

**PASS Stage 0:** tests green · gateway reachable from the app · `geo.site` anchored.

---

## Stage 1 — First light: the LoRa spine (3× Heltec V3)

*Kit rows 1–2 — the kit deliberately includes **three** Heltecs: base station,
gateway (field), and **relay**. The relay is what turns point-to-point into a
mesh (Stage 3b). Deep detail: `PHASE-B-LORA-MESH.md`.*

**1.1** Screw antennas onto **all three** Heltecs. Label them `heltec-base`,
`heltec-gw`, `heltec-relay`.

**1.2 Flash the linktest firmware** (each board, one at a time):

```powershell
$env:CARGO_TARGET_DIR="F:\t\fw"   # REQUIRED on Windows — esp-idf-sys fails with
                                  # "Too long output directory" under deep paths.
                                  # Once per shell, before ANY firmware build.
cd firmware\heltec-lora-linktest
cargo run --release        # flashes + opens serial monitor
```

Boot + OLED alive = board good.

**1.3 Two-board ping.** Power both; the linktest exchanges frames
(915 MHz, SF7/BW125/CR4-5, sync `0x1424`). Watch both consoles: TX on one, RX with
**RSSI** on the other. Typical desk RSSI: −30 to −60 dBm. Walk one board to another
room; RSSI should fall, frames keep arriving.

**1.4 Relay sanity (all three powered).** Frames carry `[src][seq][ttl]` with a
de-dup ring — with `heltec-relay` powered alongside the other two, each frame is
relayed **once** (no duplicate floods, no storms). The relay does nothing else
until Stage 3b, when it earns its keep.

**PASS Stage 1:** bidirectional frames with plausible RSSI on all pairs; no
duplicate floods with all three powered.

---

## Stage 2 — The control node (Waveshare ESP32-S3 Touch LCD)

*Kit rows 4, 7–9. This is `HARDWARE-TEST-WALKTHROUGH.md` **Phase A** — run it as
written there. Summary gates:*

**2.1** Flash `firmware/obc-esp32-s3` **with the board feature** — the default
build is the XIAO pin map and would drive this board's LCD lines:

```powershell
$env:CARGO_TARGET_DIR="F:\t\fw"   # if not already set in this shell (Stage 1.2)
cd firmware\obc-esp32-s3
cargo run --release --features board-waveshare-21
```

Confirm the boot banner and `{"id":"1","cmd":"capabilities"}` → `ok:true`.
*(Running Stage 2 on a XIAO instead? Default build, allow-list 21/3/6/7/8,
onboard LED = GPIO21 **active-low**, I2C 4/5, DHT22 on 9.)*

**2.2 LED smoke test** on an allow-listed pin — **GPIO43** (12-pin header, TXD
pin): LED + 330 Ω to GND; `gpio_write` pin 43 → 1/0, LED tracks.

**2.3 Track 0 on-MCU gate — the critical test.** All three must refuse:
- pin outside allow-list (`pin 99`) → `safety: pin not in allow-list`
- value out of range
- rate-limit burst

This is the deterministic gate the LLM cannot override — if any of these pass
through, **stop and fix before continuing**.

**2.4 Sensors.** DHT22 data → header pin **IO0** (GPIO0) with a 10 kΩ pull-up to
3V3 (required — it also holds the BOOT strap high), then BME280/MPU-6050 on the
**I2C connector** (SDA=15, SCL=7; the bus already carries the board's touch/IMU/
RTC — no address conflicts): `sensor_read` returns real values (breathe on the
DHT22, tilt the IMU).

**2.5 On-MCU safing.** Confirm the boot log's `on-MCU safing rules loaded` and the
reflex behaviors per Phase A.

**PASS Stage 2:** capabilities + LED + **all three Track 0 refusals** + one live sensor.

---

## Stage 3 — Mesh → brain: facts flow into world memory

*Kit row 3 + Stage 1 boards. Wiring: `BENCH-MVB-WIRING.svg`; pins:
`BENCH-MVB-PINOUT.svg` §② (or Appendix B of the BOM doc). This closes the two
flash-pending items from the Phase B runbook.*

**3.1 Flash the XIAO spine mirror** (`firmware/obc-esp32-s3` XIAO profile) — it
emits its spine JSON on UART1/**D6** autonomously.

**3.2 Forward jumper:** XIAO **D6 → heltec-gw GPIO2** (continuity-check the jumper
first — this is the pending item). Gateway Heltec bridges UART↔LoRa.

**3.3 Base station to host:** `heltec-base` on USB; note its COM port. In
`bench-config.toml`:

```toml
[lora_gateway]
port = "COM6"        # heltec-base's port
baud = 115200
```

Restart the brain **with hardware enabled**:

```powershell
cargo run --features hardware -- start --config bench-config.toml
```

**3.4 Verify ingest:** within a minute,

```powershell
cargo run -- status
```

shows a **Mesh nodes** section with the XIAO (health, RSSI, last-seen). World
memory now carries `mesh.<node>` facts — the physical world is in the brain.

**PASS Stage 3:** XIAO's telemetry appears in `status` via LoRa with live RSSI.

### Stage 3b — True 3-hop relay *(closes the open Phase B roadmap item)*

*This is the roadmap's last unchecked mesh box: "True 3-hop relay (needs a 3rd
radio out of direct range)". Topology per the wiring SVG's RELAY inset:*

```
heltec-base ↔ heltec-relay ↔ heltec-gw ← UART ← xiao-1
```

**3b.1 Break the direct path.** Move `heltec-base` (with the host) out of
`heltec-gw`'s direct range — different floor, far room, concrete between them.
Confirm the break honestly: power **off** the relay and verify the XIAO's facts
**stop** arriving in `status` (if they still arrive, the path isn't broken —
add distance/obstacles).

**3b.2 Close it through the relay.** Power `heltec-relay` on, positioned where
it can hear both ends. Facts resume within a supervisor tick.

**3b.3 Verify it's really two hops:** RSSI shown in `status` is now the
**relay→base** hop (typically weaker/different from Stage 3's direct value),
and the relay's console shows frames forwarded with TTL decremented, each
exactly once (de-dup).

**3b.4 Round trip over two hops.** Repeat a Stage 5-style `mesh_command` ping
(`capabilities`) once Stage 5's return path is wired — the reply must also ride
relay → base.

**PASS Stage 3b:** with the direct path proven dead, telemetry flows only via
the relay; kill the relay → flow stops; restore → resumes. *Then tick the
roadmap box.*

---

## Stage 4 — Reflex → System 2: the brain notices and *thinks*

*No new hardware — this validates this week's software on the Stage 3 rig.*

**4.1** Confirm `bench-config.toml` has the supervisor + System 2 sections armed
(Appendix A): `[mesh_supervisor]` with `escalate_after_ms`, `[reflex]` with
`safing = true`, `[notifications]`, `[system2] enabled = true`.

**4.2 Kill the field node.** Unplug the XIAO (or its Heltec). Then watch, in order:

1. Supervisor marks it offline, auto-pings (`recover = "capabilities"`), then past
   `escalate_after_ms` raises `mesh.<node>.escalation` — `status` shows
   **presumed lost**.
2. The `safe-mesh-node-lost` reflex escalates; the notification log-of-record
   (`notifications.escalation` in world memory) records it. `status` shows it under
   recent escalations.
3. **System 2 wakes**: the log shows `System 2: waking the slow reasoner`, and
   world memory gains `system2.last_wake` with the node-lost reason and the
   agent's diagnosis (it has `mesh_status` + `world_memory` + the
   `mesh-node-lost` playbook directive).

**4.3 Novelty gate.** Leave the node unplugged. Repeated escalations must **not**
re-wake System 2 within the novelty window — the log shows suppressed repeats;
`system2_suppressed_repeat_total` climbs in `/api/v1/metrics` (visible in the
Fleet app metrics card).

**4.4 Recovery.** Plug the node back in: escalation auto-clears, health returns to
online.

**PASS Stage 4:** exactly **one** System 2 wake for the whole outage; escalation
cleared on return.

---

## Stage 5 — Two-way mesh under Track 0: command a node over the air

*Closes the remaining Phase B outbound items (reverse jumper + base-station
console firmware).*

**5.1 Reverse jumper:** heltec-gw **GPIO4 → XIAO D7 (GPIO44)**.

**5.2 Flash the base-station console build** on `heltec-base` (USB stdin → LoRa TX;
see the Phase B runbook — this was flash-pending).

**5.3 Command over the air.** Ask the agent to run `mesh_command` targeting the
XIAO with a `gpio_write` on an allow-listed pin. Two Track 0 layers must both show:

1. **Host approval:** `gpio_write` over the mesh is physical — with an
   irreversible/high-blast risk class it now asks **per-call even under Full
   autonomy**. Approve it from the Fleet app's Operate section (elevate with the
   operate token; the row shows the **PHYSICAL** badge and the concrete effect),
   or grant session scope. The grant lands in the signed remote-action audit.
2. **Node authority:** the command dispatches through the node's own gated
   `handle_request`. Prove it: send `pin 99` over the air → the **node** refuses
   (`safety: pin not in allow-list`) and the refusal rides LoRa home.

**PASS Stage 5:** an approved allow-listed write actuates over the air; an
off-list write is refused **by the node**, not just the host.

---

## Stage 6 — Camera trap: ESP32-S3-EYE + PIR → ClawCam → brain

*Kit rows 5–6, 10, 14. The camera side lives in the ClawCam repo (its docs govern
flashing/ingest); this stage is the OBC-side contract.*

**6.1** Flash an S3-EYE with the ClawCam node firmware; wire the PIR
(HC-SR501/AM312) as the wake trigger; microSD in.

**6.2** Wave → PIR wake → capture lands on microSD.

**6.3** Stand up the ClawCam gateway (Pi 5, or the workstation to defer kit row 14);
node ingests captures.

**6.4 Brain contract:** with `[perception.clawcam_poll]` armed, `clawcam.*`
analytics facts appear in world memory, and the `vision_analytics_rules` reflexes
guard for a knocked-over/silent camera (which, per Stage 4, would wake System 2).

**PASS Stage 6:** PIR-triggered capture ingested; `clawcam.*` facts in world memory.

---

## Stage 7 — Sign-off + burn-in

| # | Milestone | Stage | ✔ |
|---|---|---|---|
| 1 | Workspace tests green, gateway + Fleet app connected | 0 | ☐ |
| 2 | Bench site anchored (`geo.site`) | 0 | ☐ |
| 3 | Two-Heltec LoRa ping with plausible RSSI | 1 | ☐ |
| 4 | Control node: capabilities + LED + sensor | 2 | ☐ |
| 5 | **Track 0 on-MCU: all three refusals** | 2 | ☐ |
| 6 | Mesh telemetry in world memory (`status` Mesh nodes) | 3 | ☐ |
| 6b | True 3-hop relay: direct path dead, flow via relay only | 3b | ☐ |
| 7 | Node loss → escalation → **one** System 2 wake → auto-clear | 4 | ☐ |
| 8 | Over-the-air command: host per-call approval + node-side refusal of off-list pin | 5 | ☐ |
| 9 | PIR capture → ClawCam → `clawcam.*` facts | 6 | ☐ |
| 10 | Overnight burn-in (below) | 7 | ☐ |

**Burn-in:** leave Stages 3–4 running overnight with
`digest_interval_ms = 86400000`. Next morning: no unexplained escalations, the
daily digest is one tidy roll-up, `system2` wake count ≤ budget, and the mesh
nodes are still online. That's a bench you can trust — and the go/no-go for
moving any of it outdoors.

---

## Appendix A — `bench-config.toml` template

```toml
[agent]
name = "obc-bench"

[provider]                      # any provider works; System 2 uses it on wakes
name = "openai"
model = "gpt-4o-mini"
# api_key via env

[perception]
world_memory = true

# Stage 3+ — uncomment once heltec-base is on USB
#[lora_gateway]
#port = "COM6"
#baud = 115200

[mesh_supervisor]
enabled = true
stale_ms = 30000
tick_ms = 5000
recover = "capabilities"        # auto-ping an offline node
escalate_after_ms = 120000      # 2 min continuously offline => presumed lost

[reflex]
enabled = true
safing = true                   # standard rules incl. safe-mesh-node-lost

[notifications]
enabled = true
log_to_world_memory = true
dedup_window_ms = 60000
digest_interval_ms = 86400000   # daily roll-up for the burn-in
# webhook_url = "https://..."   # optional push

[system2]
enabled = true                  # escalations wake the LLM (novelty-gated)
novelty_window_ms = 600000
max_wakes_per_hour = 6

[gateway]
enabled = true
host = "0.0.0.0"                # reachable from the phone on the bench LAN
port = 8080
api_token = "bench-read-token"
operate_token = "bench-operate-token"   # Fleet app Operate elevation

[autonomy]
level = "full"                  # irreversible/high-blast still asks per-call (Track 0)
```

## Appendix B — quick crib

| Do | Command |
|---|---|
| Flash Waveshare control node | `cd firmware\obc-esp32-s3; cargo run --release --features board-waveshare-21` |
| Flash XIAO node | `cd firmware\obc-esp32-s3; cargo run --release` (default = XIAO pin map) |
| Flash Heltec linktest | `cd firmware\heltec-lora-linktest; cargo run --release` |
| Windows firmware builds | set `$env:CARGO_TARGET_DIR="F:\t\fw"` first, every shell (esp-idf path-length limit) |
| Brain with serial bridge | `cargo run --features hardware -- start --config bench-config.toml` |
| Bench state | `cargo run -- status` (mesh health, escalations) |
| Talk to a node (espflash monitor is display-only!) | `powershell -File F:\Documents\GitHub\Oh-Ben-Claw\scripts\serial-json-repl.ps1 -Port COM7` |
| Node capabilities (serial) | `{"id":"1","cmd":"capabilities"}` |
| LED on/off (serial) | `{"id":"2","cmd":"gpio_write","args":{"pin":43,"value":1}}` (Waveshare; XIAO onboard LED: pin 21, **0=on**) |
| Track 0 refusal probe | `{"id":"3","cmd":"gpio_write","args":{"pin":99,"value":1}}` |
| Live metrics | `GET http://<host>:8080/api/v1/metrics` (or Fleet app) |

Troubleshooting: `HARDWARE-TEST-WALKTHROUGH.md` §Troubleshooting and
`PHASE-B-LORA-MESH.md` §Troubleshooting.

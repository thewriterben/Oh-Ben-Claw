# Oh-Ben-Claw ESP32-S3 — hardware bring-up runbook

A precise, ordered procedure to take the firmware from a fresh board to a verified
embodied node: flash → smoke-test every capability → verify the Track 0 safety gate
and the on-MCU reflex/safing loops → join the LoRa mesh. Each step lists the exact
serial command to send and what a healthy board returns, so a failure is obvious and
localized.

Target board: Waveshare ESP32-S3 Touch LCD 2.1 (and compatible ESP32-S3 boards).
Everything here is over the USB serial link (UART0, TX=GPIO43, RX=GPIO44, 115200 8N1).

> **Status of on-board peripherals.** `gpio_read`/`gpio_write`, the reflex/safing
> engine, and the Track 0 gate are real. `sensor_read`, `camera_capture`, and
> `audio_sample` currently return **placeholder values** (they are stubs pending the
> real ESP-IDF drivers — see "Known stubs" at the end). Bring-up still validates the
> full control path; the perception values are just canned until those drivers land.

---

## 0. Prerequisites

1. **Espressif Rust toolchain** (Xtensa): `cargo install espup && espup install`, then
   source the export script (`~/export-esp.sh`, or `%USERPROFILE%\export-esp.ps1` on
   Windows) in the shell you build from.
2. **Flasher**: `cargo install espflash` (provides `espflash`/`cargo espflash`).
3. **Windows path-length gotcha.** `esp-idf-sys` aborts if the build output path is
   long (`Too long output directory …`). Either build under WSL2 on its native
   filesystem, **or** relocate the target dir to a short root before building:
   ```powershell
   $env:CARGO_TARGET_DIR = "C:\e"
   ```
   `subst`/junctions do **not** work — the check resolves the real path.
4. A serial monitor for hand-testing: `espflash monitor`, or any 115200 8N1 terminal.
   Commands are newline-terminated JSON; responses are newline-terminated JSON.

---

## 1. sdkconfig (only needed for the perception drivers)

The control path needs no sdkconfig changes. The perception drivers, once built, need
an `sdkconfig.defaults` in this directory. Recommended starting flags (verify against
your board's PSRAM type and flash size):

```
# PSRAM — required for the camera frame buffer. Pick the mode your board uses;
# most ESP32-S3 modules with 8 MB PSRAM are octal (OPI), some are quad (QSPI).
CONFIG_SPIRAM=y
CONFIG_SPIRAM_MODE_OCT=y          # or CONFIG_SPIRAM_MODE_QUAD=y
CONFIG_SPIRAM_SPEED_80M=y

# Camera (OV2640) — pulled in via the esp32-camera IDF component; see task 173.
CONFIG_OV2640_SUPPORT=y

# Larger main task stack for the agent HTTP path.
CONFIG_ESP_MAIN_TASK_STACK_SIZE=8192
```

Getting PSRAM mode wrong is the #1 cause of a board that boots but fails camera init —
match it to your module before chasing driver bugs.

---

## 2. Wiring — pick your board's pin map

The firmware has two pin maps. **Default build = XIAO ESP32-S3 (Sense).** Build with
`--features board-waveshare-21` for the Waveshare ESP32-S3-Touch-LCD-2.1 (its round
RGB LCD consumes most GPIOs — only the 12-pin header + I2C connector are exposed;
see `docs/datasheets/waveshare-esp32-s3-touch-lcd-2.1.md`).

| Bus / device        | Default (XIAO)              | `board-waveshare-21`             |
|---------------------|-----------------------------|----------------------------------|
| Command I/O         | native USB-Serial-JTAG      | native USB-Serial-JTAG (19/20)   |
| Spine uplink (UART1)| TX=43 (D6), RX=44 (D7)      | **disabled** (pins repurposed)   |
| I2C sensor bus      | SDA=4, SCL=5                | SDA=15, SCL=7 (hardwired conn.)  |
| DHT22 data          | 9 (D10)                     | 0 (header IO0, 10 kΩ pull-up)    |
| I2S microphone      | SCK=0, WS=1, SD=2           | n/a (stub)                       |
| Camera (OV2640)     | opt-in `camera` feature     | n/a — no connector (stub)        |
| Output GPIOs (safe) | **21, 3, 6, 7, 8**          | **43, 44**                       |

The output pins are the only pins the Track 0 gate permits `gpio_write` on by
default (boot allow-list). Wire your actuator-enable line / LED to one of them —
e.g. **3** on the XIAO (D2 pad; GPIO21 is the onboard LED, active-LOW), or **43**
on the Waveshare — for the safety tests below. Examples below use the XIAO map.

> **Windows path-length gotcha:** `esp-idf-sys` fails with "Too long output
> directory" under deep paths. Set a short target dir first:
> `$env:CARGO_TARGET_DIR="F:\t\obc"` (any ≤ ~10-char base works).

---

## 3. Flash & first contact

```bash
cd firmware/obc-esp32-s3
cargo espflash flash --release --monitor
```

On boot the monitor should show:

```
Oh-Ben-Claw ESP32-S3 firmware v0.1.0 ready
Node ID: obc-esp32-s3-001
on-MCU safing rules loaded (3 built-in)
```

Then confirm the command surface:

```json
{"id":"1","cmd":"capabilities"}
```
Expect `ok:true` with a `result` listing `gpio_read`, `gpio_write`, `sensor_read`,
`set_reflex_rules`, `set_limits`, `agent_chat`, … and `"edge_agent":true`.

---

## 4. Smoke-test each capability

Send each line; the healthy response is noted. `ok:false` with an `error` is a failure
to localize.

**GPIO (real):**
```json
{"id":"2","cmd":"gpio_write","args":{"pin":3,"value":1}}   → ok:true, result:"done"
{"id":"3","cmd":"gpio_read","args":{"pin":3}}              → ok:true, result:"1"
{"id":"4","cmd":"gpio_write","args":{"pin":3,"value":0}}   → ok:true, result:"done"
```

**Sensors / camera / audio (currently stubs — confirm they respond, not the value):**
```json
{"id":"5","cmd":"sensor_read","args":{"sensor":"bme280","field":"temperature"}}  → "22.5" (placeholder)
{"id":"6","cmd":"audio_sample","args":{"duration_ms":100}}                        → "0.05" (placeholder RMS)
{"id":"7","cmd":"camera_capture","args":{"quality":5}}                            → "STUB:camera_capture:…"
```
These prove the command routing; the values become real when tasks 172/173 land.

---

## 5. Verify the Track 0 safety gate (the important one)

The gate is the on-MCU guarantee that no host, skill, or LLM can drive a pin out of
policy. Verify default-deny, then a host-pushed tightening, then the rate limit.

**Default policy refuses a non-allow-listed pin:**
```json
{"id":"10","cmd":"gpio_write","args":{"pin":99,"value":1}}
→ ok:false, error:"safety: pin 99 not in allow-list"
```

**…and an out-of-range value:**
```json
{"id":"11","cmd":"gpio_write","args":{"pin":3,"value":5}}
→ ok:false, error:"safety: value 5 out of range (min=Some(0), max=Some(1))"
```

**Host pushes a stricter policy — one pin, 500 ms rate limit:**
```json
{"id":"12","cmd":"set_limits","args":{"limits":[
  {"node_id":"obc-esp32-s3-001","tool":"gpio_write","allowed_pins":[3],
   "value_min":0,"value_max":1,"min_interval_ms":500}]}}
→ ok:true, result includes "applied":true,"allowed_pins":[3],"min_interval_ms":500
```

**Now a previously-allowed pin is refused (policy replaced):**
```json
{"id":"13","cmd":"gpio_write","args":{"pin":21,"value":1}}
→ ok:false, error:"safety: pin 21 not in allow-list"
```

**And the rate limit bites on rapid re-fire of pin 3:**
```json
{"id":"14","cmd":"gpio_write","args":{"pin":3,"value":1}}   → ok:true
{"id":"15","cmd":"gpio_write","args":{"pin":3,"value":0}}   → ok:false, error:"safety: rate limit (…ms since last, min 500ms)"
```
(Send 15 within ~half a second of 14.) Wait >500 ms and it succeeds again.

---

## 6. Verify the on-MCU reflex + safing loops (System 1)

The node reacts within ~1 s to sensor thresholds even with no host in the loop.

**Push a rule that cuts pin 3 when a sensor crosses a threshold:**
```json
{"id":"20","cmd":"set_reflex_rules","args":{"rules":[
  {"id":"overheat","when":{"type":"sensor","entity":"sensor.temperature","op":"gt","value":60.0},
   "then":{"type":"gpio_write","node_id":"self","pin":3,"value":0},"debounce_ms":1000}]}}
→ ok:true, result includes "builtin_safing" ≥ 3  (your rule is merged *behind* the built-in safing rules)
```

**Fire it deterministically via a synthetic snapshot (bypasses the stubbed sensor):**
```json
{"id":"21","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":75.0},"now_ms":1000}}
→ ok:true, result "fired":[{"rule_id":"overheat","applied":true,…}]
```
`applied:true` means the gated `gpio_write` succeeded (pin 3 is allow-listed). If you
left the tight policy from §5 active, that's consistent; otherwise reset with a fresh
`set_limits` or reboot.

**Battery safing (built-in, no rule needed):**
```json
{"id":"22","cmd":"reflex_tick","args":{"snapshot":{"sensor.battery_soc":6.0},"now_ms":2000}}
→ fires "safe-battery-critical" (gpio cut) and "safe-battery-low" (escalate)
```

**Link watchdog:** stop sending serial for >30 s and watch the monitor — the autonomous
tick emits a `link_state:"offline"` report and the built-in `safe-link-offline`
escalation, proving offline self-protection.

---

## 7. LoRa node + fleet-over-mesh (optional, needs a LoRa board)

The ESP32-S3 is the compute node; the LoRa mesh uses a separate radio node
(`firmware/lora-node`, RadioLib). Bring the radio link up first, independently:

1. Flash **two** LoRa boards from `firmware/lora-node/` with `#define SELFTEST_HEARTBEAT 1`.
   Each self-transmits every 5 s; each should print the other's
   `{"t":"hb","n":"selftest",…}` on its serial monitor. If the lines cross, radio
   params + wiring are good. Set the flag back to `0`.
2. On the OBC **host** (not this MCU), set `[fleet] enabled = true` and
   `[fleet.lora_serial] port="…"` and run with `--features hardware`. The host opens
   the LoRa node, ingests heartbeats into the fleet coordinator, and broadcasts
   assignments back — exactly the path covered by `tests/mesh_fleet_e2e.rs`.

---

## 8. Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `Too long output directory` at build | Windows path length — `CARGO_TARGET_DIR=C:\e` or WSL2 (§0.3). |
| `rustup` picks `stable`, no Xtensa target | Build from *this* dir (its `rust-toolchain.toml` pins `esp`); don't override the channel. |
| Board boots, camera init fails | PSRAM mode mismatch (§1) — set OCT vs QUAD to match your module. |
| `sensor_read` returns the same value always | Expected — it's still a stub (task 172). |
| `gpio_write` always `ok:false: pin … not in allow-list` | Pin isn't in the boot allow-list (default/XIAO: 21,3,6,7,8 · `board-waveshare-21`: 43,44) or a tighter `set_limits` is active — reboot to reset, or push a policy that includes your pin. |
| No `link_state` reports | Reflex tick only runs with ≥1 rule loaded; the built-in safing rules load at boot, so this should always tick — check the boot log for "safing rules loaded". |

---

## Known stubs (the real-hardware follow-on)

Replacing these with real ESP-IDF drivers is the substance of the hardware track:

- **`sensor_read`** (task 172) — real I2C (BME280/MPU6050/SHT31) via `esp-idf-hal`'s
  `I2cDriver`. Highest impact: it feeds `read_sensor_snapshot`, so real reflex/safing
  decisions depend on it (real temperature, real battery SoC).
- **`audio_sample`** (task 173) — I2S mic read + RMS.
- **`camera_capture`** (task 173) — OV2640 via the `espressif/esp32-camera` IDF
  component (needs the `idf_component.yml` + the PSRAM/camera sdkconfig from §1).

Until then, `reflex_tick` with a synthetic `snapshot` (as in §6) is the deterministic
way to exercise the reaction path independent of the sensor stubs.

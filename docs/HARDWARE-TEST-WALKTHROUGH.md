# Oh-Ben-Claw — Full Hardware Test Walkthrough

A complete, ordered, checkable procedure to validate every real-hardware capability
of OBC on physical devices: the ESP32-S3 compute node, the LoRa mesh, and host-side
fleet coordination. Each step gives the **exact command**, the **expected result**,
and a **PASS** criterion. Work top to bottom; later phases build on earlier ones.

The test is organised in three phases so you can stop at any milestone:

- **Phase A — single ESP32-S3 node** (no radios). Validates the whole control path:
  GPIO, the Track 0 safety gate, reflexes, safing, and the sensor/mic/camera drivers.
- **Phase B — LoRa mesh link** (two LoRa boards). Validates the radio transport.
- **Phase C — fleet over the mesh** (host brain + LoRa node). Validates heartbeat →
  auction → assignment end to end on real hardware.

> Throughout, "send X" means type the one-line JSON at the serial monitor and press
> Enter. Responses are one-line JSON. `ok:true` = success; `ok:false` with an `error`
> is a failure to localise.

---

## Bill of materials

| Item | Qty | Notes |
|------|-----|-------|
| ESP32-S3 board — XIAO ESP32-S3 (default fw pin map) or Waveshare Touch LCD 2.1 (build `--features board-waveshare-21`) | 1 | The compute node. Examples below use the **default/XIAO map**; Waveshare deltas: outputs 43/44, DHT22=IO0, I2C 15/7 (see BRINGUP.md §2). |
| USB-C cable (data) | 1 | For flashing + serial. |
| I2C sensors (optional but recommended) | — | MAX17048 fuel gauge (@0x36), MPU6050 IMU (@0x68), BME280 (@0x76/0x77). Default: SDA=GPIO4, SCL=GPIO5 · Waveshare: SDA=15, SCL=7. |
| I2S MEMS mic (INMP441 / SPH0645) | 1 opt | SCK=GPIO0, WS=GPIO1, SD=GPIO2 (default build only; n/a on Waveshare). |
| OV2640 camera (FPC) | 1 opt | Only for the camera test; needs PSRAM on the board (n/a on Waveshare — no connector). |
| LED + resistor (or a scope/meter) | 1 | On an allow-listed output pin — default build: **21, 3, 6, 7, 8** (21 = XIAO onboard LED, active-LOW) · Waveshare build: **43, 44**. |
| LoRa boards (Heltec WiFi LoRa 32 V3, T-Beam, or RAK4631), **915 MHz (US)** | 2 | Phase B/C. Buy the 915 MHz variant; attach antennas before powering. |
| Linux/Mac host or Windows PC | 1 | Runs the OBC brain in Phase C. |

> ⚠️ **Never power a LoRa board without its antenna attached** — transmitting with no
> antenna can destroy the RF amplifier.

---

## Phase A — single ESP32-S3 node

### A0. Environment setup (one time)

1. **Espressif Rust toolchain:**
   ```powershell
   cargo install espup
   espup install
   # then source the export script in each new shell:
   #   Windows:  . $HOME\export-esp.ps1
   #   Unix:     . $HOME/export-esp.sh
   ```
2. **Flasher:**
   ```powershell
   cargo install espflash
   ```
3. **Windows only — path-length + git long paths** (skip on WSL2/Linux/Mac):
   ```powershell
   git config --global core.longpaths true
   ```
   The firmware's `.cargo/config.toml` already sets `CARGO_TARGET_DIR`-friendly
   options, but also export a short target dir in each shell:
   ```powershell
   $env:CARGO_TARGET_DIR = "C:\e"
   ```
   > If the ESP-IDF clone ever fails with "Filename too long", enable Windows long
   > paths (admin, one time):
   > ```powershell
   > New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name LongPathsEnabled -Value 1 -PropertyType DWORD -Force
   > ```
   > **WSL2 avoids all of this** — if Windows fights you, build under WSL2's native
   > filesystem instead.

**PASS A0:** `espup` and `espflash` install without error.

### A1. Build the firmware

```powershell
cd firmware\obc-esp32-s3
cargo build --release
```

**PASS A1:** `Finished release profile`. (First build downloads ESP-IDF — several
minutes. If you see camera-component downloads and you don't want them in the default
build, comment out the `[[package.metadata.esp-idf-sys.extra_components]]` block in
`Cargo.toml` and the PSRAM lines in `sdkconfig.defaults`.)

### A2. Flash + first contact

Plug in the board, then:
```powershell
cargo run --release        # builds (cached) + flashes + opens the serial monitor
```
Watch the boot log. You should see:
```
Oh-Ben-Claw ESP32-S3 firmware v0.1.0 ready
Node ID: obc-esp32-s3-001
on-MCU safing rules loaded (3 built-in)
I2C sensor bus ready (SDA=4, SCL=5)      # only if sensors wired (Waveshare build: SDA=15, SCL=7)
I2S mic ready (SCK=0, WS=1, SD=2)        # only if mic wired
```

Confirm the command surface:
```json
{"id":"1","cmd":"capabilities"}
```
**PASS A2:** `ok:true`, and `result` lists `gpio_read`, `gpio_write`, `set_limits`,
`set_reflex_rules`, `sensor_read`, `audio_sample`, `camera_capture`, and
`"edge_agent":true`.

### A3. GPIO (real actuation)

*Why this test matters: it's the first time a JSON command moves real electrons.
Every physical action later — reflex cuts, safing, over-the-mesh commands — goes
through exactly this `gpio_write` path (and its Track 0 gate, tested next in A4).
Get A3 solid and the rest of the physical stack is plumbing on top of it.*

**How commands reach the board.** The firmware reads **newline-delimited JSON**
on its USB serial port. ⚠ **`espflash monitor` is display-only** — it does NOT
forward your typing to the device. Close it (Ctrl+C; it holds the port), then use
the repo's REPL:

```powershell
# path is repo-root-relative — from a firmware dir use the full path:
powershell -File F:\Documents\GitHub\Oh-Ben-Claw\scripts\serial-json-repl.ps1 -Port COM7
```

Wait for the green **`Connected to COMx`** banner, *then* type the command — the
whole JSON object on one line — and press **Enter**; replies print inline.
⚠ Don't type JSON at a normal `PS >` prompt — that's PowerShell, which will try
to parse it as script and error. JSON only goes into the running REPL (no `PS >`
prompt visible).

**Which COM port?** List them with friendly names:
```powershell
Get-CimInstance Win32_PnPEntity | Where-Object { $_.Name -match 'COM\d' } | Select-Object -ExpandProperty Name
```
"USB Serial Device" / "USB JTAG/serial debug unit" = the ESP32-S3 node's native
USB (**this one**) · "Silicon Labs CP210x" = a Heltec console · "CH343" = the
Waveshare's UART Type-C. Ambiguous? Unplug/replug the node — the number that
vanishes and returns is yours. (Any interactive serial terminal also works —
PuTTY with *local echo* + *local line editing* forced on, or the Arduino IDE
Serial Monitor with newline endings.)

**Anatomy of a command:** `id` is your correlation tag — any string, echoed back
in the reply so you can match answers to requests. `cmd` is the operation.
`args.pin` / `args.value` are plain numbers (`value` must be 0 or 1 — anything
else is refused by the gate, which you'll prove in A4). Every reply is one line:
`{"id":"2","ok":true,"result":"done"}` or `{"id":"2","ok":false,"error":"safety: …"}`.

**Step 0 — zero-wiring smoke test (XIAO only).** The XIAO's onboard user LED is
GPIO**21**, allow-listed, and **active-LOW** — write **0** to light it:
```json
{"id":"1","cmd":"gpio_write","args":{"pin":21,"value":0}}   → ok:true — LED ON
{"id":"2","cmd":"gpio_write","args":{"pin":21,"value":1}}   → ok:true — LED off
```
If this works, your command path is proven before you touch a jumper wire.

**Step 1 — wire an external LED** (or just a multimeter):
- **XIAO:** pad silk-labeled **D2** = GPIO**3**.
- **Waveshare 2.1:** 12-pin header pin 9 (TXD) = GPIO**43** — use `"pin":43` in
  every command below.
- Circuit: `GPIO ── 330 Ω ── LED anode (long leg) ── LED cathode (short leg) ── GND`.
  Meter instead: probe GPIO-to-GND, expect ~3.3 V ↔ 0 V.

**Step 2 — drive it and read it back:**
```json
{"id":"3","cmd":"gpio_write","args":{"pin":3,"value":1}}   → ok:true, "done"   (LED on,  ~3.3 V)
{"id":"4","cmd":"gpio_read","args":{"pin":3}}              → ok:true, "1"
{"id":"5","cmd":"gpio_write","args":{"pin":3,"value":0}}   → ok:true, "done"   (LED off, ~0 V)
```
The read-back matters: `gpio_read` samples the **actual pin level**, not a cached
value — so `"1"` after a write proves the pin is really driving, not just that
the firmware accepted the command.

**If it doesn't work:**

| Symptom | Likely cause |
|---|---|
| No reply at all | Monitor isn't forwarding input — use a separate serial terminal; or wrong COM port |
| `ok:false … pin not in allow-list` | Typo, or wrong build for your board (default/XIAO list: 21,3,6,7,8 · `board-waveshare-21`: 43,44) |
| `ok:true` but LED never lights | LED backwards (long leg to resistor), missing resistor, wrong pad — or you're on pin 21 which is active-LOW |
| Write 1 but `gpio_read` returns `"0"` | Pin shorted/overloaded, or probing the wrong pad — meter it against GND |

**PASS A3:** LED (or meter) tracks the writes; `gpio_read` returns the level you set.

### A4. Track 0 safety gate — the critical safety test

This proves nothing can drive a pin outside policy — the core safety guarantee.

**(a) Default-deny — non-allow-listed pin refused:**
```json
{"id":"10","cmd":"gpio_write","args":{"pin":99,"value":1}}
→ ok:false, error:"safety: pin 99 not in allow-list"
```
**(b) Value range enforced:**
```json
{"id":"11","cmd":"gpio_write","args":{"pin":3,"value":5}}
→ ok:false, error:"safety: value 5 out of range (min=Some(0), max=Some(1))"
```
**(c) Host tightens the policy (one pin + 5 s rate limit):**

⚠ **Send as ONE line** — the protocol is newline-delimited, so a pretty-printed
multi-line paste arrives as broken fragments and the policy silently never
applies (each fragment just errors). Verify the raw reply contains
`"applied":true` before moving to (d).

*(Why 5000 ms, not 500: the node's main loop takes ~1 s per iteration — reflex
tick, sensors, heartbeat — so console commands are naturally ≥1 s apart and a
500 ms limit can never be observed from the console. Bench-verified.)*
```json
{"id":"12","cmd":"set_limits","args":{"limits":[{"node_id":"obc-esp32-s3-001","tool":"gpio_write","allowed_pins":[3],"value_min":0,"value_max":1,"min_interval_ms":5000}]}}
→ ok:true, result includes "applied":true, "allowed_pins":[3], "min_interval_ms":5000
```
**(d) Previously-allowed pin now refused (policy replaced):**
```json
{"id":"13","cmd":"gpio_write","args":{"pin":21,"value":1}}
→ ok:false, error:"safety: pin 21 not in allow-list"
```
**(e) Rate limit bites.** Paste **both lines as one block** and press Enter once —
the embedded newline sends 14, your Enter sends 15, landing them ~100 ms apart
(hand-pacing two pastes usually exceeds the window; bench-verified):
```json
{"id":"14","cmd":"gpio_write","args":{"pin":3,"value":1}}  → ok:true
{"id":"15","cmd":"gpio_write","args":{"pin":3,"value":0}}  → ok:false, error:"safety: rate limit (...ms since last, min 5000ms)"
```
Wait >5 s and pin 3 works again. **Reboot** to restore the default allow-list
(21,3,6,7,8 — Waveshare build: 43,44) before the reflex test.

**PASS A4:** all five sub-cases behave as shown. This is the most important test — the
gate refuses every out-of-policy write.

### A5. Reflexes (System 1)

Push a rule that cuts GPIO3 when a temperature threshold is crossed (**one line** —
see the warning in A4c):
```json
{"id":"20","cmd":"set_reflex_rules","args":{"rules":[{"id":"overheat","when":{"type":"sensor","entity":"sensor.temperature","op":"gt","value":60.0},"then":{"type":"gpio_write","node_id":"self","pin":3,"value":0},"debounce_ms":1000}]}}
→ ok:true, result includes "builtin_safing" ≥ 3   (your rule merges *behind* the built-in safing rules)
```
Fire it deterministically with a synthetic snapshot (works even without a real sensor):
```json
{"id":"21","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":75.0},"now_ms":1000}}
→ ok:true — "fired" lists the BUILT-IN overtemp rules first (safe-overtemp-critical
  cuts its pin with applied:true, safe-overtemp-warn escalates), THEN your
  "overheat" rule with applied:true — built-ins always run ahead of pushed rules
```
`applied:true` means the gated GPIO write succeeded (pin 3 is allow-listed after reboot).

**PASS A5:** the `overheat` reflex fires and `applied:true`.

### A6. Safing (self-protection)

**(a) Battery safing (built-in, no rule needed):**
```json
{"id":"22","cmd":"reflex_tick","args":{"snapshot":{"sensor.battery_soc":6.0},"now_ms":2000}}
→ fires "safe-battery-critical" (gpio cut) AND "safe-battery-low" (escalate)
```
**(b) Link watchdog:** stop sending serial for **>30 seconds** and watch the monitor.
The autonomous tick emits a `link_state:"offline"` report and the built-in
`safe-link-offline` escalation.

**PASS A6:** critical battery fires both safing rules; 30 s of silence produces the
offline `link_state` + escalation.

### A7. I2C sensors (needs sensors wired)

```json
{"id":"30","cmd":"sensor_read","args":{"sensor":"max17048","field":"soc"}}       → live % (e.g. "87.5")
{"id":"31","cmd":"sensor_read","args":{"sensor":"mpu6050","field":"accel_z"}}    → ~"9.8" at rest
```
Tilt the board: `accel_z` drops as it leaves horizontal.

**PASS A7:** `soc` reflects real charge; `accel_z` ≈ 9.8 flat and changes with tilt.
(Without a MAX17048, `soc` errors — expected; battery safing stays dormant.)

### A8. BME280 environment (needs a BME280 wired)

```json
{"id":"32","cmd":"sensor_read","args":{"sensor":"bme280","field":"temperature"}}  → room temp, e.g. "22.4"
{"id":"33","cmd":"sensor_read","args":{"sensor":"bme280","field":"humidity"}}     → e.g. "41.0"
{"id":"34","cmd":"sensor_read","args":{"sensor":"bme280","field":"pressure"}}     → ~"1013" hPa
```
Breathe on the sensor: temperature + humidity rise within a second or two.

**PASS A8:** all three read plausible values and respond to breath. **Real-data
reflex check:** with the `overheat` rule loaded (A5), warm the BME280 above 60 °C
(hair dryer, briefly) — the reflex should fire from the *real* reading, not a
synthetic snapshot.

### A9. I2S microphone (needs a mic wired)

```json
{"id":"35","cmd":"audio_sample","args":{"duration_ms":100}}   → small RMS in a quiet room, e.g. "0.0031"
```
Speak/clap near the mic and repeat — the value rises toward 1.0.

**PASS A9:** quiet ≈ near-zero; sound raises the RMS.

### A10. OV2640 camera (opt-in; needs PSRAM + camera + the `camera` feature)

Follow `firmware/obc-esp32-s3/CAMERA.md` (the `idf_component.yml`/`extra_components`
and PSRAM sdkconfig are already in place from setup). Build + flash with the feature:
```powershell
cargo run --release --features camera
```
Boot log should show `OV2640 camera initialised`. Then:
```json
{"id":"36","cmd":"camera_capture","args":{"quality":10}}
→ ok:true, result = a long base64 string (NOT the "STUB:" placeholder)
```
Decode the base64 to a `.jpg` and open it.

**PASS A10:** a real base64 JPEG returns and decodes to a viewable image.
> Note: the camera feature disables the I2C sensor bus (shared SCCB pins 4/5), so
> A7/A8 and battery safing use stubs in a camera build — that's expected.

**✅ Phase A complete** when A2–A9 pass (A10 if you have the camera). The entire
embodied control path is validated on real silicon.

---

## Phase B — LoRa mesh link (two LoRa boards)

Goal: prove two radios talk before involving any host. Uses the Arduino node firmware
in `firmware/lora-node/`, not the ESP32-S3 compute firmware.

### B1. Flash both LoRa boards in self-test mode

1. Arduino IDE (or `arduino-cli`) + install **RadioLib** (Jan Gromes, 6.x).
2. Open `firmware/lora-node/obc_lora_bridge/obc_lora_bridge.ino`.
3. Uncomment your board (`BOARD_HELTEC_V3_SX1262`, `BOARD_TBEAM_SX1276`, …) and
   comment the others.
4. Set `RADIO_FREQ_MHZ = 915.0` (US) and `#define SELFTEST_HEARTBEAT 1`.
5. Verify the pin map against your board silkscreen. Flash **both** boards.

### B2. Watch the cross-talk

Open each board's serial monitor at 115200. Each self-transmits a heartbeat every 5 s.

**PASS B2:** each monitor prints the *other* board's line:
```
{"t":"hb","n":"selftest","m":"idle"}
```
If the lines cross, the radio params + wiring are correct. Set
`SELFTEST_HEARTBEAT 0` and reflash both for normal operation.

> If nothing crosses: confirm both boards share **identical** freq / BW / SF / CR /
> syncword, both have antennas, and both are the 915 MHz variant.

**✅ Phase B complete** when the two boards exchange heartbeats.

---

## Phase C — fleet over the mesh (host brain + LoRa node)

Goal: a heartbeat heard over LoRa becomes a fleet `NodeState`, gets auctioned, and the
assignment goes back out over the mesh — end to end on hardware.

### C1. Build the host with hardware support

On the OBC host (the brain), from the repo root:
```bash
cargo build --release --features hardware
```
**PASS C1:** `Finished release`.

### C2. Configure the LoRa serial bridge

In your host config TOML:
```toml
[fleet]
enabled = true

[fleet.lora_serial]
port = "COM7"          # or /dev/ttyUSB0 — the serial port of a LoRa node
baud = 115200
relay_hops = 3
```
Connect one LoRa node (flashed with `firmware/lora-node`, self-test **off**) to the
host via USB.

### C3. Run the brain

```bash
cargo run --release --features hardware -- <your normal args>
```
**PASS C3:** the log shows `Fleet: LoRa-mesh serial bridge attached` and
`Fleet coordinator active`.

### C4. Inject a heartbeat + observe the assignment

From a **second** LoRa node (or the ESP32-S3 flashed to emit a `MeshFrame` heartbeat),
put a heartbeat on the air for a node, e.g. `rover-a` at (0,0). On the host, queue a
task near it (via the `fleet` tool / your normal task path) and watch the logs.

**PASS C4 (the end-to-end proof):**
1. The host logs that it ingested `rover-a`'s heartbeat (it becomes a `NodeState`).
2. The coordinator auctions the queued task to `rover-a`.
3. The host logs `Fleet: broadcast assignments over LoRa mesh` — a `MeshFrame::Assign`
   for `rover-a` goes back out over the radio.

That closes the loop: **heartbeat in over LoRa → auction → assignment out over LoRa**,
the same logic that runs over MQTT, with no broker and no WiFi.

> This mirrors the automated `tests/mesh_fleet_e2e.rs` and `tests/spine_fleet_e2e.rs`
> — Phase C is those tests, on metal.

**✅ Phase C complete** when the heartbeat→auction→assignment round-trips over LoRa.

---

## Full sign-off checklist

| # | Test | Pass? |
|---|------|-------|
| A2 | Boot + `capabilities` | ☐ |
| A3 | GPIO read/write | ☐ |
| A4 | Track 0 gate (deny pin / range / push / rate limit) | ☐ |
| A5 | Reflex fires (`overheat`) | ☐ |
| A6 | Safing (battery critical + link watchdog) | ☐ |
| A7 | I2C sensors (MAX17048 SoC, MPU6050 accel) | ☐ |
| A8 | BME280 temp/humidity/pressure (+ real-data reflex) | ☐ |
| A9 | I2S mic RMS responds to sound | ☐ |
| A10 | OV2640 camera returns a JPEG (opt-in) | ☐ |
| B2 | Two LoRa boards exchange heartbeats | ☐ |
| C3 | Host attaches LoRa bridge + coordinator active | ☐ |
| C4 | Heartbeat → auction → assignment over LoRa | ☐ |

---

## Troubleshooting quick reference

| Symptom | Fix |
|---|---|
| `Too long output directory` / `Filename too long` | Windows path length — `$env:CARGO_TARGET_DIR="C:\e"`, `git config --global core.longpaths true`, or build under WSL2. |
| `rustup` picks `stable`, no Xtensa target | Build from `firmware/obc-esp32-s3` (its `rust-toolchain.toml` pins `esp`). |
| esp-idf-sys ignores sdkconfig / `extra_components` | `CARGO_WORKSPACE_DIR = { value = "", relative = true }` must be in `.cargo/config.toml` (it is). |
| `no such command: espflash` | `cargo install espflash`, then `cargo run --release`. |
| `gpio_write` always `pin ... not in allow-list` | Not in the boot set (default/XIAO: 21,3,6,7,8 · Waveshare build: 43,44), or a tighter `set_limits` is active — reboot to reset. |
| `sensor_read ... Unknown sensor/field` | That part isn't wired / not supported — expected for a bare board. |
| Camera `esp_camera_init failed` | PSRAM mode — swap `CONFIG_SPIRAM_MODE_OCT` ↔ `QUAD` in `sdkconfig.defaults`. |
| LoRa boards don't hear each other | Match freq/BW/SF/CR/syncword on both; antennas attached; both 915 MHz. |

---

*This walkthrough consolidates `firmware/obc-esp32-s3/BRINGUP.md`,
`firmware/obc-esp32-s3/CAMERA.md`, and `firmware/lora-node/README.md` into one
end-to-end procedure. See those files for per-component detail.*

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
| I2C sensors (optional but recommended) | — | MAX17048 fuel gauge (@0x36), MPU6050 IMU (@0x68), BME280 (@0x76/0x77). Default: SDA=GPIO5 (silk D4), SCL=GPIO6 (silk D5) · Waveshare: SDA=15, SCL=7. |
| I2S MEMS mic (INMP441 / SPH0645) | 1 opt | SCK=GPIO0, WS=GPIO1, SD=GPIO2 (default build only; n/a on Waveshare). |
| OV2640 camera (FPC) | 1 opt | Only for the camera test; needs PSRAM on the board. **n/a on Waveshare — no camera connector.** Settled 2026-08-21 by looking at the board: its only FPC connector is the screen's. This row was right; `camera.rs` was the one claiming otherwise and has been corrected. `--features camera` with `board-waveshare-21` is now a compile error. |
| LED + resistor (or a scope/meter) | 1 | On an allow-listed output pin — default build: **21, 3, 7, 8** (21 = XIAO onboard LED, active-LOW) · Waveshare build: **43, 44**. |
| LoRa boards (Heltec WiFi LoRa 32 V3, T-Beam, or RAK4631), **915 MHz (US)** | 2 | Phase B/C. Buy the 915 MHz variant; attach antennas before powering. |
| Linux/Mac host or Windows PC | 1 | Runs the OBC brain in Phase C. |

> ⚠️ **Never power a LoRa board without its antenna attached** — transmitting with no
> antenna can destroy the RF amplifier.

> **Changed 2026-08-21.** GPIO 6 left the default allow-list and the I2C bus moved
> from 4/5 to 5/6. GPIO 6 is silk D5, the pad Seeed marks SCL: it was both a Track 0
> output and — once the bus moved onto the labelled pads — half the sensor bus. If you
> have an older printout, the two rows above are the ones that changed.
>
> The dates live here rather than in the table rows on purpose.
> `scripts/check_bench_constants.py` treats a dated line as a historical record and
> skips it, so putting "changed on <date>" beside a live pin list would switch the
> check off for exactly the line that most needs it. That happened once, within an
> hour of the script being written.

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
on-MCU safing rules loaded (6 built-in)
I2C sensor bus ready (SDA=5, SCL=6)      # only if sensors wired (Waveshare build: SDA=15, SCL=7)
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

### A5b. Descending modulation (the spinal tier)

*Needs firmware from 2026-09-13 or later (`descend` in `main.rs`, **and** the
4 KB USB RX buffer — see the note at the end of this section); reflash first
if the node predates it.* `scripts/bench_descend.py --port COMx` runs every
step below and writes the replies to `results/`; it asks for the power cycle
in (h). Same rule as A5, but its threshold is bound to
**slot 3** over a range the rule owns (40–80 °C), default level 0.5 — so it
starts at 60 °C, exactly where A5 had it. Nothing here needs a sensor: every
tick is a synthetic snapshot, so the pass/fail is deterministic. The built-in
`safe-overtemp-warn` (≥ 60) and `-critical` (≥ 75) rules still run ahead of
yours; the lines below say what to look for so they do not confuse the read.

**(a) Push the slot-bound rule** (one line):
```json
{"id":"24","cmd":"set_reflex_rules","args":{"rules":[{"id":"overheat","when":{"type":"sensor_slot","entity":"sensor.temperature","op":"gt","slot":3,"min":40.0,"max":80.0,"default":0.5},"then":{"type":"gpio_write","node_id":"self","pin":3,"value":0},"debounce_ms":1000}]}}
→ ok:true, "loaded":1
```
**(b) Default level = 60 °C, as A5:**
```json
{"id":"25","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":65.0}}}
→ "fired" lists safe-overtemp-warn AND "overheat" (applied:true)
```
**(c) Descend slot 3 to 1.0 → threshold 80 °C.** Same reading, your rule now
stays quiet while the built-in warn still fires — the brain moved *your*
threshold and could not touch the safing one:
```json
{"id":"26","cmd":"descend","args":{"m":[[3,1.0]]}}
→ ok:true, {"applied":1,"active":[[3,1.0]]}
{"id":"27","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":65.0}}}
→ "fired" lists safe-overtemp-warn only — NO "overheat"
```
**(d) Descend to 0.0 → threshold 40 °C.** A reading no built-in reacts to now
fires your rule:
```json
{"id":"28","cmd":"descend","args":{"m":[[3,0.0]]}}
→ ok:true, "active":[[3,0.0]]
{"id":"29","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":45.0}}}
→ "fired" lists "overheat" only (applied:true)
```
**(e) A bad message changes nothing.** All-or-nothing: the good pair in the
first line does not land either. Check `active` afterwards:
```json
{"id":"30","cmd":"descend","args":{"m":[[3,0.5],[16,0.5]]}}
→ ok:false, error:"descend refused: slot 16 out of range (max 15)"
{"id":"31","cmd":"descend","args":{"m":[[3,1.5]]}}
→ ok:false, error:"descend refused: slot 3: level 1.5 not in [0, 1]"
{"id":"32","cmd":"descend","args":{"m":[]}}
→ ok:true, "active":[[3,0.0]]        (still 0.0 from (d) — nothing above landed)
```
**(f) Clear → back to the rule's default (60 °C):**
```json
{"id":"33","cmd":"descend","args":{"clear":true}}
→ ok:true, "active":[]
{"id":"34","cmd":"reflex_tick","args":{"snapshot":{"sensor.temperature":65.0}}}
→ "overheat" fires again (with safe-overtemp-warn)
```
**(g) A rule the node cannot hold is refused at the door** — and the rule set
is left as it was (rule count unchanged, (f) still behaves):
```json
{"id":"35","cmd":"set_reflex_rules","args":{"rules":[{"id":"bad","when":{"type":"sensor_slot","entity":"sensor.temperature","op":"gt","slot":16,"min":0.0,"max":1.0,"default":0.5},"then":{"type":"escalate","reason":"x"},"debounce_ms":0}]}}
→ ok:false, error:"set_reflex_rules refused: rule bad: slot 16 out of range (max 15)"
```
**(h) Reboot** and repeat (b): the level is RAM-only, so a fresh boot is back
at the default. That is the safe posture, and it is the one thing on this list
worth seeing rather than reading.

**PASS A5b:** (b) fires, (c) does not, (d) fires, (e) refuses both and `active`
is unchanged, (f) fires again, (g) refuses, (h) fires after reboot. What this
proves: the brain moves a threshold inside a range the rule owns, never an
actuator, and a reboot or a bad message leaves the node exactly as safe as it
was. What it does not prove: the same over LoRa — that is Part B below.

**Run 2026-09-12: 18/18** (`scripts/bench_descend.py`, record in `results/`).

### A5c. Descending modulation over the mesh (Part B)

Same rule, same steps, but the `descend` lines go in on the **base station's
console** (COM3), cross LoRa to gw-40, arrive at the node on the UART jumper,
and the node's reply comes back the same way as a `SPINE ◄ … "type":"cmd_result"`
line. The node's USB stays connected so each threshold move is verified with
a `reflex_tick` independently of the reply.

```powershell
python scripts/bench_descend_lora.py --node COM6 --base COM3
```

Preconditions the first attempt got wrong, each now enforced or documented
in the script:

- **gw-40 must run `heltec-lora-linktest`.** A lit OLED means it does not
  (this firmware never drives the display); the factory demo was on it.
- **Both Heltecs built with `--features bench-low-power`** (−9 dBm). At +22
  dBm two radios on one desk read −8 to −20 and the receiver overdrives —
  121 B replies arrived 1 in 5 while 55 B keepalives passed. Target
  −45…−60; the record carries the RSSI of every reply.
- **The base built with `no-relay`.** A sink that re-broadcasts everything it
  hears is deaf for the next frame; it has no one to relay to.
- **Open the base's port with DTR/RTS low.** A default open resets it, its
  8-bit seq restarts at 0, and gw-40's 32-entry de-dup ring drops the next
  commands as duplicates. The script does this; anything else on COM3 must
  too. Reflashing the base has the same effect — reboot gw-40 afterwards
  (a default open of *its* port does it) or wait until the base's seq is
  clear of the ring.
- **Keep the node's USB drained** while waiting on the base. The XIAO's
  native USB-Serial-JTAG blocks on write once the host holds the port open
  and stops reading, and the node writes every reply to USB *and* UART1, so
  the over-the-air reply arrives 8–17 s late. The script reads both ports.

Two firmware defects this uncovered, both fixed the same night and both
field bugs rather than bench artefacts:

1. **Single-shot RX.** `Sx1262::receive` re-armed a 600 ms one-shot receive
   from standby on every call; a frame *starting* in the last airtime of the
   window was aborted by the chip's own timeout. With two stations' 5 s
   keepalive clocks in lock-step (they were: +0.6 s apart for minutes) that
   lost 6 of 11 frames at −50 dBm, SNR 12, zero CRC errors. Now continuous RX,
   armed once, left on between polls: 10 of 11, and the one loss was a true
   simultaneous transmission.
2. **Keepalive in the reply window.** After sending a command the base's
   next keepalive could land 1–2 s later — exactly when the node's reply
   arrives — making it deaf to the frame it was waiting for. The base now
   holds its keepalive 3 s after any console-originated command.

What remains is real: the mesh has no ACK, and a plain half-duplex
collision still takes roughly one command or reply in five. The script
resends an unanswered `descend` (idempotent; new id per attempt; attempts
recorded), which is what the host's `mesh_command` sink should do and does
not yet.

**PASS A5c:** every over-the-air step answered and verified.
**Run 2026-09-12: 10/10**, `b1` on its second attempt, replies at −50/−51 dBm
(`results/bench_descend_lora-20260912-233135.json`).

> **Run 2026-09-13, XIAO ESP32-S3 `obc-esp32-s3-001` on COM6: 18/18 steps as
> stated** (`results/bench_descend-20260912-221327.json`), (h) done as a USB
> power cycle rather than a reset — a stronger form of the same claim.
>
> **It did not pass the first two times, and the reason was not this
> feature.** Both runs lost the `set_reflex_rules` reply *and the command
> after it*, while the same push from a hand-rolled probe answered in 0.16 s.
> `scripts/probe_linelen.py` settled it: the node answered every padded line
> up to 256 bytes and none above (one lucky 290), and an over-long line took
> the next one with it. The USB-Serial-JTAG **RX** buffer had been left at the
> driver default of 256 B when TX was raised to 4096 — the overflowed tail has
> no newline, fuses with the following line, and a line that fails to parse is
> answered with nothing. A5's rule line is 250 bytes with a two-character id,
> which is why A5 always worked; A5b's slot rule is ~300 and never could.
> Fixed in `main.rs` (`rx_buffer_size(4096)`); the probe then answered every
> length to 500 and the run above followed. If a command over the wire ever
> goes silent again, run the probe before suspecting the command.

### A5d. Authenticated frames between the stations (SPINE-AUTH step 4)

**What it proves:** every frame on the air between the Heltecs is
`[src][seq][ttl][ctr:u32][payload ≤ 228][mac:8]` — tagged with HMAC-SHA256
under a key derived from the deployment's root secret, counted, and judged
by a receive window that survives a reboot. A station built with a
different root hears nothing but rejections. Nothing unverified reaches the
UART, the console line the host parses, or the relay.

**What it does not prove:** an on-air replay. The host cannot inject raw
radio frames, so the window's replay refusal is proven on the host
(`tests/spine_auth_vectors.rs`, `tests/firmware_spine_framing.rs`) and its
*persistence* is proven here by the reboot gap. It also does not prove
anything about the host's trust in the base station: the host reads the
base's console over USB and trusts what the base has verified. The host
verifies no tags itself (SPINE-AUTH.md §3.4 is still open).

**Preconditions.** Both Heltecs on PC USB (base COM3, bridge COM5), built with
the same `OBC_SPINE_ROOT` — a build without one fails with the message that
says so. Keep the secret outside the repository (`~/.obc/spine_root` on the
bench). The boot log prints a two-byte fingerprint of the root so two boards
can be compared at a glance.

```powershell
. $env:USERPROFILE\export-esp.ps1; $env:CARGO_TARGET_DIR='C:\e'
$env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
cd firmware\heltec-lora-linktest
cargo build --release --features bench-low-power,no-relay   # base
espflash flash --port COM3 C:\e\xtensa-esp32s3-espidf\release\heltec-lora-linktest
cargo build --release --features bench-low-power            # bridge
espflash flash --port COM5 C:\e\xtensa-esp32s3-espidf\release\heltec-lora-linktest
cd ..\..
python scripts\bench_spine_auth.py observe --seconds 60
python scripts\bench_spine_auth.py reboot-gap
python scripts\bench_descend_lora.py --node COM6 --base COM3   # Part B, now authenticated
# then rebuild the bridge with a different OBC_SPINE_ROOT, flash it, and:
python scripts\bench_spine_auth.py wrong-root --seconds 40
# …and flash it back.
```

`observe` wants every `SPINE ◄` line to carry `ctr=`, counters strictly
increasing per source with `seq` as the low byte, and no `REJECTED` line.
`reboot-gap` resets the bridge through its CP2102 circuit and measures the
bounded silence SPINE-REPLAY.md §3 promises: the receiver resumes at its
persisted ceiling `h + M` and refuses at most `M = 8` legitimate frames while
the sender catches up, silently (they are `Seen`, which is also what a relay
duplicate is). It also checks the rebooted bridge's counter resumed above
anything the base had accepted. `wrong-root` wants every frame each side
hears from the other rejected as a bad tag and none accepted.

**Run 2026-09-13** (final, jitter in; records `results/bench_spine_auth-*`
and `bench_descend_lora-20260913-005559.json`). `observe` 60 s: base 13/13
frames from the bridge (ctr 993–1005), bridge 10/10 from the base
(1745–1754), 0 rejected. Part B over the authenticated link: **10/10, every
step on its first attempt** (before the jitter: 10/10 with `b1` on its
second), −56 dBm.
`reboot-gap`: bridge counter resumed at 928 after the base had last accepted
903; base accepted 929, 930, 931, 932; bridge re-accepted the base 22.5 s after
the reset with **3 frames skipped** (bound 8); 0 loud rejections; fingerprint
`c4cd` on both boot logs. `wrong-root`: base rejected 7/7 bridge frames and
the bridge 5/5 base frames, all `bad tag`, 0 accepted.

> **It found a link defect on the way, not an auth defect.** The first
> `reboot-gap` run left the bridge deaf to the base for the full 120 s
> window, and the base hearing one bridge frame in eight. Every `REJECTED`
> counter was zero; the tag was fine. The consoles showed the cause: after
> the reboot the bridge's keepalive landed **70 ms** after the base's, every
> 5 s, indefinitely — two identical 5-second timers on identical firmware,
> quantised by the same 600 ms receive poll, transmitting into each other's
> frames with nothing to break the phase because neither heard the other.
> Resetting the base alone (`scripts/probe_reset_station.py COM3`) restored
> the link at once, which is the confirmation. Fix: each keepalive interval
> now carries up to 1.5 s of jitter derived from the frame counter
> (`keepalive_interval_ms`), so a collision cannot repeat. The run above is
> with the jitter in. The ~1-in-5 loss measured on 2026-09-12 was very
> likely this mechanism in a milder phase; watch the retry counts.

### A5e. The host verifies what the base station heard

**What it proves:** the trust boundary is no longer a USB cable. The base
station prints each frame's `ctr=` and `mac=`; the host's
`lora_gateway::LoraAuth` verifies the tag again under the deployment root,
judges the counter against a per-station window persisted in world memory
(`spine.auth.gw-XX`, M = 1), and only then ingests. Under a root the
stations do not have, nothing lands. This is SPINE-AUTH.md §3.4's last
bullet, and the test runs the production pieces — `open_split`,
`run_gateway_rx`, world memory — not a re-implementation.

**What it does not prove:** that the node ↔ bridge serial wire is
authenticated (it is not; the bridge signs what it forwards), or anything
about a base station that lies *consistently* — one that holds the root
can sign what it likes. Track 0 on the node is the boundary for that.

**Preconditions.** A5d done (both stations on the same root, base on COM3),
the root at `~/.obc/spine_root`.

```powershell
$env:OBC_BASE_PORT = 'COM3'
$env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
cargo test --features hardware --test lora_gateway_live -- --ignored --nocapture --test-threads 1
```

The first test listens 40 s and wants ≥ 3 frames verified, 0 refused, and
a `mesh.*` fact in world memory. The second listens 40 s under a root of
zeros and wants 0 verified, ≥ 3 refused as `bad tag`, and *no* `mesh.*`
fact. The host's log line `host root fingerprint XXXX` must match the
stations' boot logs.

**Run 2026-09-13: 2/2.** Under the stations' root (fingerprint `c4cd`):
gw-40 `accepted 9, rejected 0`, high-water mark 1447, `mesh.gw-40` and
`mesh.obc-esp32-s3-001` in world memory. Under zeros (fingerprint `60e0`):
`accepted 0, rejected 9`, every reason `bad tag`, no `mesh.*` fact. The
second test's port open needed a retry loop: the first test's serial
thread lets go of the port only when its next line fails to send, up to
one keepalive later.

### A5f. The first real slot-bound rule: die temperature → onboard LED

**What it proves:** the spinal tier acting on a real signal, not a synthetic
snapshot. The XIAO's on-die temperature sensor (no wiring; ~1 °C
quantisation, moves with load and room — `scripts/probe_die_temp.py` shows
it) is in the node's reflex snapshot as `sensor.die_temperature`. Two rules
bound to slot 0 — `die-hot` (`>` threshold → GPIO21 = 0, LED on) and
`die-cool` (`<=` threshold → GPIO21 = 1, LED off), threshold `30 + level·40`
°C, default level 0.5 — are pushed over USB. The script reads the real
temperature, then slides the slot *around it* over the authenticated LoRa
link: threshold above the reading (LED must go off), below it (on), above
again (off), each verified by `gpio_read 21` and the node's own `reflex`
report with `applied: true`. The brain does the same thing with
`mesh_command descend {"m": [[0, level]]}`.

**What it does not prove:** that the brain has a reason to move it. The
modulation path is proven end to end on a real quantity; *when* the reasoner
should lower a node's thresholds (novelty from the mushroom body, say) is
the next feature, not this one. (When first run, a holding rule re-fired
every `debounce_ms` — 10 s here — so the node reported every 10 s while the
condition held; A5h closed that the same day, and the hold below vets the
transition itself.)

**Preconditions.** Node on COM6 with this firmware (die sensor +
`MAX_LINE_LEN` 2048 — see the box), base on COM3 with A5d's build.

```powershell
python scripts\bench_die_rule.py --node COM6 --base COM3
```

**Run 2026-09-13: 11/11** (`results/bench_die_rule-20260913-020024.json`).
Die 38.3 °C; thresholds 44 °C (level 0.357) and 32 °C (0.057). LED off →
on → off with a `die-cool` / `die-hot` / `die-cool` report each, all
`applied: true`, each transition within 10 s of the descend; one descend
needed its second attempt (collision, as usual).

> **It did not pass the first time, and again the reason was the wire.**
> `set_reflex_rules` with the two rules is ~620 bytes; the node's
> `MAX_LINE_LEN` was 512, and an over-long line was cleared *silently* —
> the same defect class as the 256-byte RX ring the day before, one layer
> up. Now 2048, and an over-long or unparseable command line is answered
> (`{"ok":false,"error":"command line longer than 2048 bytes — discarded
> whole"}` / `"request not understood: …"`) rather than met with silence.
> `scripts/probe_linelen.py` is the tool if a line ever goes quiet again.

**Run 2026-09-13, with `hold_ms` (12/12,
`results/bench_die_rule-20260913-115412.json`).** The die rules now carry
`hold_ms: 3000`: the reading is ~1 °C quantised and ticked at 1 Hz, so at
the threshold it flickers across a tick at a time, and a rule must see its
condition hold for three ticks before it acts — the persistence vet, the
cheapest one there is (no model, no window, one timestamp per rule; WILD's
ripple detector applies the same 20–600 ms duration criterion). Die
33.3 °C; every transition still reported exactly once and `applied: true`,
now **5 s** after the descend rather than 3 — the hold's cost, in the open.
Nothing reported while holding. One descend needed its second attempt.

### A5g. The brain's novelty reaches the node: descending posture

**What it proves:** the loop the connectome thread was aiming at, closed on
hardware. `obc_agent::posture::PosturePolicy` turns the mushroom body's
assessment of each objective into a posture — *cautious* when the objective
has no close precedent, the rules' *defaults* when it is familiar — and
sends it down the spine as `descend` before the model has said a word: the
fly's MB→DN bias. Sent on change only, confirmed by the node's reply
(resent on silence, recorded on `descending.<node>` with `answered`,
`attempts`, the node's `active` table). On the bench body that means: a
novel objective lowers slot 0 to `novel_level` (0.15 → 36 °C on the
die-temperature rule), which lights the LED at a 38 °C die; a familiar one
clears it and the LED goes out.

**What it does not prove:** that a *real* assessment did it. The test feeds
the policy the two assessments the mushroom body would produce; producing
them from real episodes needs `[self_improvement] semantic = true` and a
body that has seen some objectives, which the bench brain has not been run
with yet. The policy → mesh → node → reflex → pin path is what is measured.

**Preconditions.** A5f run first (its rules and the pin-21 limit are loaded
on the node and the slot is cleared); base on COM3; die temperature in the
36–50 °C band (38 °C on the bench).

```powershell
$env:OBC_BASE_PORT = 'COM3'
$env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
cargo test --features hardware --test posture_live -- --ignored --nocapture
```

**Run 2026-09-13: pass** (from a cleared slot, LED off). Novel objective →
`descend [[0,0.15]]` sent, node's reply confirmed by the policy on the first
attempt, LED read back **0** (on) over the mesh; familiar objective →
`descend {clear}` confirmed, LED **1** (off); a second familiar turn sent
nothing. An earlier run needed the policy's own retry when the first frame
was lost, which is what the retry is for: under this bench's chatter (a
holding rule reporting every 10 s, keepalives, relays) the base's commands
reached the bridge 4 times in 6 in a spot check — the edge-triggering gap
from §A5f is now a link-load problem too.

> **It found the biggest node defect of the day before it could run.** The
> first attempt got no reply to anything over the mesh — not even
> `gpio_read` — while `bench_spine_auth.py observe` showed the link
> perfect. `scripts/probe_mesh_cmd.py --bridge COM5` showed the bridge
> receiving the command and handing it to the node's UART; nothing came
> back. `scripts/probe_usb_drain.py` settled it: with a reader on the node's
> USB port every command was answered; without one, none. The XIAO's
> USB-Serial-JTAG stalls every write when the cable is plugged in and
> nothing reads, and `send_line` waited **2 s** per line for the host to
> drain — so a holding reflex report every 10 s, the beacon and safing
> reports parked the main loop for seconds at a time, the UART intake
> starved, and the mesh node was deaf whenever a laptop was merely
> *attached*. A5b–A5f never saw it because every one of those scripts held
> the port open and read it. Fixed: `send_line` gives up inside ~50 ms (a
> connected host drains a 4 KiB ring far faster than that) and the UART RX
> ring is 2 KiB. A mesh node must never wait on its USB.

### A5h. Edge-triggered reflexes: what a holding rule costs the mesh

**What it proves:** that a rule which merely keeps holding costs the link
nothing. Until this, the node re-fired a holding rule every `debounce_ms`
and put a ~200-byte report on the mesh each time; with the two die rules
loaded that was one frame every 10 s, plus the built-in `safe-link-offline`
escalation every 10 s for as long as no USB host was attached — which for a
node on the mesh is always. `fire_on_change` on the node now fires a rule
once on its condition's false→true transition and not again until the
condition has dropped and returned (debounce still applies on top; the same
field as the host's, judged by the only evidence the node has). The die
rules and `safe-link-offline` carry it; the battery and over-temperature
*cuts* deliberately do not, so a cut keeps re-asserting.

**Measured with** `scripts/bench_chatter.py`: rules pushed, node's USB
closed so it runs unattended, 90 s of everything the base hears by kind,
then six `gpio_read`s over the mesh.

```powershell
python scripts\bench_chatter.py --node COM6 --base COM3 --seconds 90 --probes 6
```

**Run 2026-09-13.** Before (re-fire at debounce): the node's reflex reports
were **8.6/min** — the die rule 5.3/min, `safe-link-offline` 3.3/min — 29
frames heard in 90 s, and mesh commands answered **3/6**
(`results/bench_chatter-20260913-101007.json`). After (edge-triggered die
rules and link-offline): reflex reports **0.7/min** (the one link-offline
report when the node's USB was closed, then nothing), 15 frames in 90 s,
commands answered **6/6**. The functional run (§A5f's script, now
expecting edge semantics) is **12/12**, every transition reported once and
within 3 s, nothing reported while holding
(`results/bench_die_rule-20260913-101610.json`).

### A5i. Link silence counts mesh contact

**What it proves:** that `safe-link-offline` measures the link it names.
Until this, the node's silence clock was reset by USB bytes only
(`main.rs`, the USB intake) — the spine-UART intake that receives mesh
commands never touched it. So a node with USB closed, commanded over the
authenticated LoRa link, measured silence from the moment USB closed and
held "host link lost" for good; A5h's edge-triggering made that *quiet*,
not true. A mesh command that parses as a request addressed to us now
resets the clock.

Where the reset sits matters. The bridge forwards **every** verified frame
to the node's UART, the base's own 5 s `gw_keepalive` included, and that
frame has no `to`, so `command_targets_us` accepts it as broadcast. The
first attempt reset the clock there, and the node never went offline at
all — station liveness counted as the host. The reset is on a line
`handle_request` accepted: a command is host contact, a keepalive is not.

**Measured with** `scripts/bench_link_contact.py`: open the node over USB
and close it (silence starts), listen for the offline edge that ~30 s of
silence should still produce, send one mesh command and expect `online`,
then a command every 10 s for 60 s and expect no `offline`. `--watch-usb`
keeps the node's USB open and drained — no bytes sent, so silence still
accrues — and records the node's own `link_state` lines beside the mesh's,
which tells "never went offline" from "the mesh lost the report".

```powershell
python scripts\bench_link_contact.py --node COM6 --base COM3 --watch-usb
```

**Run 2026-09-13.** Baseline firmware: offline at 30.5 s, then a command
answered and **no** `online` ever, on either side. Reset at
`command_targets_us`: no `offline` in 45 s of silence on either side (the
keepalives were contact). Reset on an accepted request: `offline` at
**30.9 s** (USB and mesh agree), `online` after the first command (USB at
once, the mesh copy 7 s later), 60 s of commands with no `offline` —
**PASS** (`results/bench_link_contact-20260913-114342.json`). Note the
cost the truthful rule carries: a node whose host is quiet goes offline
30 s after its last command and reports each transition — the host does
not send keepalives to nodes, so command silence *is* host silence here.

### A5j. A threshold on the signal's own history: `sensor_baseline`

**What it proves:** that a node can ask "has this reading *departed* from
where it has been?" rather than "is it past a number?" —
`Condition::SensorBaseline { entity, op, offset, tau_s }` is true when
`value op baseline + offset`, the baseline being the engine's time-aware
exponential moving average of the entity (τ in seconds of signal, not
ticks: α = 1 − e^(−dt/τ) from the real elapsed time, so a missed tick or a
different cadence leaves τ meaning the same thing; host and node agree to
1e-12 in the unit tests). Offset rather than ratio, because °C is not a
ratio scale. The first sample is the baseline, so the leaf is false until
the signal has moved; a slow drift never fires, since the baseline follows
it (a 0.01 °C/s ramp for 1000 s — five offsets of climb — fires nothing,
against a fixed threshold that would have fired at 200 s: `a_slow_drift_
never_fires_because_the_baseline_follows_it`, host and firmware). This is
the shape of WILD's detector — threshold relative to the baseline mean —
in the form that fits a sensor scale.

**Measured with** `scripts/bench_baseline_rule.py`: pushes `die-rising`
(die temperature > baseline(τ 60 s) + 2 °C, `hold_ms` 3000,
`fire_on_change`, LED on) over USB and watches. *Steady* (unattended): 90 s
in which the rule must not fire. *Warm* (`--warm`, attended): the operator
warms the module — a thumb on the XIAO's metal can does it in ~20 s — and
the rule must fire exactly once, `applied: true`, and not again while the
die stays warm; a fixed 36 °C threshold on a die that sits at 34 °C in this
room and at 38 °C in a warm one cannot say that.

```powershell
python scripts\bench_baseline_rule.py --node COM6 --steady 90 --warm
```

**Run 2026-09-13, steady only: PASS** — die 34.3 °C for the whole 90 s, no
report (`results/bench_baseline_rule-20260913-120857.json`). A flat signal
is weak evidence for a rule about departures; the warm phase is the one
that proves it acts, and it needs a hand on the board.

### A5k. Every reflex report carries the evidence it fired on

**What it proves:** that the transition log exists from the day the rule
does. A node's `type: reflex` report now carries `ev` — one number per
entity the rule's condition reads, in the condition's depth-first order,
`null` where the snapshot lacked one — and, for a `sensor_baseline` rule,
`bl`, the baseline it compared against. This is what a later vetting stage
(the CNN+GRU WILD runs on the 0.5 s before each candidate, or a person
labelling false fires by hand) needs from the moment of the fire; the host
already stores every report whole as `mesh.<node>.reflex`, so nothing on
the host changed.

Arrays by position, not maps by name, because the report rides a 228-byte
LoRa line: an entity name is ~24 bytes, a number ~6, and the rule id
already names the rule the host holds. Values ride at three decimals — a
reading widened from the sensor's f32 printed as `34.29999923706055` on
the first run, 13 bytes of digits the sensor never had — and `error` rides
only when there is one: `"error":null` was 13 bytes of every report, and
with the evidence aboard the escalate shape sat 7 bytes under the line.
`tests/spine_payload_budget.rs` now measures the three report shapes; the
escalate report with one `ev` value is the tightest thing on the mesh at
208 B, 20 spare, and the gate names it.

**Run 2026-09-13:** §A5f's script, 12/12; each `die-cool` / `die-hot`
report carries its `ev` (`[34.3]`, `[33.3]` — the die cooled a degree
mid-run) and no `error` key
(`results/bench_die_rule-20260913-122058.json`).

### A5l. A node that boots gets its limits back — and the base could not carry them

**What it proves:** that the 2026-08-22 decision is finished. The node boots
deny-all and announces it (`policy_state`, `boot_id`) so that "a host that
remembers the boot_id it pushed against can detect the reset" — and for 22
days no host code listened. Now the mesh supervisor does: `hydrate_limits`
reads every node's latest `boot_id` (from the announcement, from the
beacon, or from any reply), and when it differs from the boot the host last
pushed limits for, sends that node's `[[safety.limits]]` as `set_limits`,
recording `mesh.<node>.limits_pushed {boot_id, id, attempts}`. The command
id is `lim<boot_id hex>`, retries `r{n}`. A node with no configured limits is
left deny-all on purpose.

Two things the bench found on the way, both now fixed:

- **The boot announcement is one frame and it was lost, twice in a row**,
  while the `link_state` a second later arrived. So the node now carries
  `boot_id` on every beacon and `policy: "deny-all"` on every beacon until a
  push lands; the host treats a beacon that still says deny-all 20 s after a
  push as the push having been lost, and pushes again with `r{n}`. The
  beacon is the node's standing word, and it stops the moment limits land.
  (`capabilities` also did not carry `boot_id`, though its comment said it
  did. It does now.)
- **The base station could not carry a `set_limits` at all.** Its console
  read stdin from the ROM UART's 128-byte hardware FIFO: 127-byte lines
  crossed, 128-byte lines lost their tail and wedged the framer until the
  station was reset. Every `set_limits` the host ever "sent" over the mesh
  (202–205 B, inside the 228-byte *radio* budget the census measured
  against) died there; `descend` and `gpio_read` crossed by luck of size.
  `scripts/probe_mesh_frame_size.py` measures it. The station now installs
  the UART0 driver with a 2 KiB ring; 90 / 150 / 205 / 228 B cross, 229 is
  refused as designed, and the console no longer wedges.

**Measured with** `scripts/bench_boot_hydrate.py` against the LIVE brain: reads
`boot_id` over USB, resets the node (RTS asserted, DTR not), confirms a new
`boot_id`, closes USB, then watches world memory for the push and for the
node's next beacon to have dropped `policy: "deny-all"`.

```powershell
python scripts\bench_boot_hydrate.py --node COM6 --within 120
```

**Run 2026-09-13 14:42: PASS in 62 s** — first push lost on the air, retry
`lim5c578fe5r1` landed, node replied `applied: true, allowed_pins [12,13]`,
next beacon carried no `policy`
(`results/bench_boot_hydrate-20260913-144237.json`). The two runs before it
(`…-141814`, `…-144023`) are the evidence for the base defect: 14 pushes
recorded, none transmitted.

> **Watch the pins.** The host pushes what `[[safety.limits]]` says — on this
> bench `[12, 13]`, while the die-temperature rules drive pin 21. After any
> node reset the LED rules will now be *refused* by the gate the host itself
> pushed, visibly (`applied: false`, `safety: pin 21 not in allow-list`),
> until the config lists 21. That is the feature working.

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

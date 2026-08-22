# Bench run — the four things only a board can settle

**Status: one of four settled, three to go.** §4 needed only eyes and is done —
the Waveshare has no camera connector. The remaining three need a powered board.
This document exists so that one sitting settles them, rather than three separate
afternoons discovering them one at a time.

Everything checkable without hardware already is: 33 firmware tests run on the
host, both gates agree on the same limit table, the compensation arithmetic
matches the datasheet's independent algorithm, and every pin number in this
document is checked against the firmware by
`scripts/check_bench_constants.py`. What is left genuinely needs a board.

Record results in [§5](#5-recording). A row filled in with "worked" and no date,
commit or observation is the kind of claim this project has spent a month
removing.

**There is a runner: `scripts/bench_run.py`.** It sends every command below,
captures the raw replies, asks you for the things only eyes can settle, and
writes the §5 record filled in — including the exchange verbatim. It never
infers a physical observation from a reply, because "the reply said refused" and
"the pin did not move" are different claims and step 1 exists because of the
gap between them.

```
python scripts/bench_run.py --dry-run    # simulated node; proves the script runs
python scripts/bench_run.py              # auto-detects the port
python scripts/bench_run.py --port COMn       # only if auto-detect picks wrong
```

Prefer the auto-detecting form, and note that it detects by *chip*, not by
position. The node is a XIAO ESP32-S3 speaking over the ESP32-S3's native
USB-Serial-JTAG, so it enumerates under Espressif's VID `0x303A`. The Heltecs
each have a CP210x USB-UART bridge (`0x10C4`). The runner refuses to talk to a
bridge port rather than picking it because it is the only one present.

That refusal exists because the alternative happened. On 2026-08-22 the only
port on the bench was **COM3, a CP210x** — and `BENCH-PINOUT-CARDS.md` Card 0
records COM3 as the LoRa **base station `gw-D8`**, not the node. The node
firmware was flashed onto the base station. No port number in any document
would have caught that; the USB descriptor does, every time, without a bench
note to keep current.

Run `--dry-run` once before you wire anything, so the first time you meet the
prompts is not while holding a soldering iron. It talks to a simulated node that
enforces the same policy, so refusals go down the refusal path; it says nothing
about your firmware or your board.

`espup`, `espflash` and the `esp` rust toolchain are installed on this machine
and flashing works. **pyserial is the one to check**, because "installed" and
"importable" are different claims:

```powershell
python -c "import serial; print(serial.__version__)"
```

On 2026-08-22 that failed in the flashing shell with `ModuleNotFoundError` while
the same package imported fine elsewhere — pyserial was installed with `--user`,
into `%APPDATA%\Python\Python314\site-packages`, which Python skips inside a
virtualenv or when `PYTHONNOUSERSITE` is set. Installing it against the
interpreter by full path avoids the whole question:

```powershell
python -c "import sys; print(sys.executable)"   # then use that path:
& "C:\Python314\python.exe" -m pip install pyserial
```

`bench_run.py` now says this itself, with your interpreter's real path filled
in, instead of a bare traceback.

---

## 0. Before you start

| | |
|---|---|
| Board | ESP32-S3, default build (the XIAO pin map, whatever board it is on) |
| Flash | see below — **on Windows this needs one env var or it fails** |
| Serial | 115200, native USB-Serial-JTAG, newline-delimited JSON |
| Wire | LED + 330 Ω to ground on **GPIO 3** (control), **GPIO 8** (refusal), optionally **GPIO 7** |
| Sensor | BME280 on **SDA=GPIO5, SCL=GPIO6** — the pads the silkscreen marks |

**GPIO numbers, not header labels.** The silk differs per board: on a XIAO,
GPIO 3 and 7 are `D2` and `D8`; on a FireBeetle they are elsewhere. Find the pad
that carries GPIO 3 on your board's vendor pinout. A `D`-number copied from
another board's card is the mistake this whole document exists to avoid.

### Flashing

**Check which board you are about to flash first.** `espflash` writes to
whatever port it finds, and every board on this bench is an ESP32-S3 with 8 MB
of flash, so nothing in the flash log distinguishes them — chip type, revision
and flash size all match. The USB descriptor does:

```powershell
python -c "from serial.tools import list_ports; [print(p.device, hex(p.vid or 0), p.description) for p in list_ports.comports()]"
```

`0x303a` is the node (native USB-Serial-JTAG). `0x10c4` is a CP210x bridge —
one of the Heltecs, and flashing node firmware onto one overwrites its gateway
build. That is not hypothetical: it happened on 2026-08-22.

```powershell
. $env:USERPROFILE\export-esp.ps1          # LIBCLANG_PATH etc. for the esp toolchain
$env:CARGO_TARGET_DIR = "C:\e"             # REQUIRED on Windows -- see below
cd firmware\obc-esp32-s3
cargo run                                  # flashes, then opens the monitor
```

The `CARGO_TARGET_DIR` line is not optional and not a preference. Without it
`esp-idf-sys` aborts with *"Too long output directory … Shorten your project
path down to no more than 10 characters"* after several minutes of compiling,
which is a slow and confusing way to learn it. `subst` and junctions do not
help: the check resolves the real path. It is documented in BRINGUP.md §0.3 and
is repeated here because a person following this document at a bench should not
have to cross-reference another one to get past step zero.

Set it per shell; it does not persist. Two other one-time settings are already
done on this machine and are listed only so a fresh machine knows to do them:
`git config --global core.longpaths true`, and the ESP-IDF framework installed
to a short root (`ESP_IDF_TOOLS_INSTALL_DIR = custom:C:/esp`, set in
`.cargo/config.toml`).

One known flake, so it does not read as a broken toolchain the first time it
happens: on 2026-08-22 the first `cargo run` after setting `CARGO_TARGET_DIR`
died with `internal compiler error: Segmentation fault` from the xtensa gcc
compiling esp-idf's `esp_lcd_panel_rgb.c`, `during RTL pass: ira`. Re-running
the identical command succeeded and it has not repeated. It is a compiler crash
inside a vendored C file, not anything this repo controls. Retry once. If it
reproduces on the same file twice in a row, that is a different problem and
worth recording rather than retrying.

**Exit the monitor with Ctrl+C before running `bench_run.py`.** It holds the
serial port, and the script will otherwise fail to open it in a way that looks
like a hardware fault.

Note the firmware commit before you begin: `git rev-parse --short HEAD`.

---

## 1. Does a refusal stop the wire moving?

This is the load-bearing one. The safety case rests on the node holding its own
copy of the limit table, so that a compromised or absent host cannot talk it
round. Six host-side tests assert the gate refuses correctly. None of them can
see a wire.

1. **Identify the node.** It must be `obc-esp32-s3-001`. Anything else and every
   step below tests the boot policy instead of the pushed one.

   Ask it, do not read it off the monitor:

   ```powershell
   python scripts\bench_run.py --probe
   ```

   `--probe` sends `capabilities`, prints the node id, board name, output pins
   and I2C pins the node reports about itself, and stops. Nothing is wired,
   nothing is written, no record file is produced. Exit 0 means the node id
   matched.

   A truncated boot log is not evidence either way, and this step used to lean
   on one. Observed 2026-08-22: a flash whose log ended mid-word at
   `main_task: Calling ap`, followed by a ROM banner. That reads as a crash and
   is not one.

   `main` takes both serial interfaces before it prints a single line —
   `UsbSerialDriver` on the native USB-Serial-JTAG (GPIO 19/20), then
   `UartDriver` on UART1 at **GPIO 43/44**, which are the ESP32-S3's default
   UART0 pins. So whichever interface a given board uses for its console, the
   firmware has taken it over by the time the first `info!` runs:

   - **XIAO** — console is the native USB, which `UsbSerialDriver` claims.
   - **Heltec V3** — console is UART0 through the CP210x on GPIO 43/44, which
     the UART1 spine uplink claims.

   The 2026-08-22 log was the second case: it was a Heltec, flashed by mistake
   (see the port note above), and its console died exactly where the spine
   uplink took its pins. A console the program under test seizes cannot report
   on that program in either direction — it can show neither a crash nor health.

   `--probe` asks over the channel that survives, and silence there is a finding
   about the node rather than an artefact of the instrument.

2. **Push the table and read back what stuck.** `set_limits` does not
   acknowledge — it returns the active policy:

   ```json
   {"applied":true,"allowed_pins":[3,7],"value_min":0,"value_max":1,"min_interval_ms":500}
   ```

   `"applied":false` means no limit matched this node. Stop and fix that first.

3. **The control.** `gpio_write` pin 3, value 1. **The LED lights.** If it does
   not, every refusal below is meaningless — a dark LED would prove nothing,
   because a disconnected wire and a working gate look identical.

4. **The refusal.** `gpio_write` pin 8. Expect a refusal in the reply, the LED
   dark, and `gpio_read` on pin 8 returning 0.

   GPIO 8 is the honest choice because it is a real output — it is in
   `OUTPUT_PINS`, so the firmware configures it at boot — but it is *not* in the
   limit table this body pushes (`allowed_pins = [3, 7]`). So a dark LED there
   means the gate refused, not that the pin was never driveable. **It needs an
   LED on it.** Watching a bare pin stay dark proves nothing at all; this
   document told you to wire 3 and 7 and then asked about 8 until 2026-08-22.

   **Watch the pin, not the reply.** A gate that refuses in the log while the
   pin twitches is the exact failure this step exists to rule out. Use a meter
   or a scope if you have one.

5. **Without the host.** Stop the agent. Send the same refused command straight
   down the serial line. It must still refuse. This is the only step anywhere
   that tests the property the safety case actually claims.

6. **The rate limit.** Two writes to pin 3 inside 500 ms. The second is refused
   and the pin holds its value rather than flickering.

---

## 2. Does the BME280 work on the corrected bus?

Until 2026-08-21 it could not have. The firmware opened I²C on GPIO 4/5 while
the XIAO's labelled bus is SDA=GPIO5, SCL=GPIO6 — so a sensor wired to the pads
marked SDA and SCL was not on the bus the firmware was driving, and GPIO 6 was
simultaneously a Track 0 output. It failed as a silent stub read, which is
indistinguishable from a sensor that is not fitted.

1. **Which bus is it actually driving.** `python scripts\bench_run.py --probe`
   prints the node's own `i2c_bus`; it must be `[5, 6]`. `[4, 5]` means an old
   build and every reading below would be a stub. The banner line
   `I2C sensor bus ready (SDA=5, SCL=6)` says the same thing when you can see it
   — see step 1.1 for why, on this board, you often cannot.

2. **Scan.** The BME280 answers at `0x76` or `0x77`. If nothing answers, the
   wiring or the pull-ups are the first suspects, not the firmware.

3. **Read.** `sensor_read` `bme280` / `humidity`. A plausible room reading is
   30–60 %RH.

4. **Breathe on it.** The value should climb within a second or two and fall
   back. This is the check that separates a real reading from a stub: a stub is
   plausible and constant.

5. **The reflex.** The Benchtop body downstream in OBC-Prime (`bodies/benchtop/config.toml` there) fires on `sensor.humidity > 75`. Breathing
   on the sensor should trip it with no model wake and no network.

> A reading that is plausible but wrong is the failure mode here. The
> compensation arithmetic is verified against the datasheet's own
> double-precision algorithm, so if the number is wrong the cause is upstream of
> the maths: the address, the calibration read, the oversampling config, or the
> wiring.

---

## 3. Are the I²C addresses and register maps right?

`sensors.rs` says "untested on metal by its author" and it is still true of this
half. The arithmetic is checked; the device conversation is not.

- **MAX17048** fuel gauge at `0x36`, SOC register `0x04`. `sensor.battery_soc`
  should read a plausible percentage. **This one matters beyond the reading**:
  the built-in safing rules shed load below 10 %, so a wrong decode here means
  the node either never protects itself or does so at the wrong moment.
- **MPU6050** at `0x68`. `sensor.accel_z` should read ≈ +9.8 m/s² with the board
  flat, and invert when you turn it over. Sign and byte order are the things to
  watch — both produce plausible numbers when wrong.

---

## 4. ~~Does the Waveshare have a camera connector?~~ **Settled 2026-08-21**

**No.** Its only FPC connector is the screen's — observed directly, no power
needed. This was the one question on the list that needed eyes rather than a
bench, and it took under a minute.

`camera.rs` was the sole source claiming otherwise; `Cargo.toml`,
`BENCH-PINOUT-CARDS.md` Card 3 and `HARDWARE-TEST-WALKTHROUGH.md` had all said
"no camera connector" since July. It has been corrected, and
`--features camera` with `board-waveshare-21` is now a compile error: that map's
pins are the Waveshare's LCD lines.

It opened a new question rather than closing one cleanly. If the map is not the
Waveshare's, whose is it? XCLK=15, SCCB=4/5 and PCLK=13 match the ESP32-S3-EYE
v2.2, but the data bus does not. Until someone checks it against a datasheet for
a board they are holding, every pin in `camera.rs` is unattributed.

**Three claims remain**, and all three need a powered board.

---

## 5. Recording

Fill this in as you go, not afterwards.

```
date:              ____________________
firmware commit:   ____________________
board:             ____________________  (XIAO / FireBeetle / Waveshare / other)
build:             default | --features board-waveshare-21 | --features camera

1. refusal stops the wire
   node reported (capabilities):____________________
   set_limits returned applied: ____________________
   control (pin 3 lights):      pass / fail
   refusal (pin 8 dark):        pass / fail   measured with: eye / meter / scope
   refuses with host stopped:   pass / fail
   rate limit holds:            pass / fail

2. BME280 on 5/6
   i2c_bus reported:            ____________________
   address that answered:       ____________________
   humidity at rest:            ________ %RH
   responds to breath:          yes / no
   reflex fired:                yes / no

3. addresses and decode
   battery_soc:                 ________ %   plausible: yes / no
   accel_z flat:                ________ m/s²
   accel_z inverted:            ________ m/s²

4. waveshare camera connector
   FPC connector present:       yes / no / no waveshare board to hand
   which file is wrong:         walkthrough / camera.rs

anything that failed plausibly — looked fine, wasn't:
____________________________________________________
```

When it is done, two things can cite this run — with the date and the commit,
not the word "worked": the `docs/SAFETY-CASE.md` control here, and the rows in
OBC-Prime's `bodies/benchtop/README.md` that currently read **not verified**.

(Both repositories are named on purpose. Those `bodies/` paths are downstream
in OBC-Prime, not here, and this document cited them bare in its first draft --
the same mistake that produced OBC-Prime's `check_cited_paths.py`, made again
in a new document an hour later. `check_tree.py` did not catch it because its
claim-document list does not include this file.)

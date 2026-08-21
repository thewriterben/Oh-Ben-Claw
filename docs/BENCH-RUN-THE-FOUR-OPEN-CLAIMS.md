# Bench run — the four things only a board can settle

**Status: not yet run.** Every claim below is unverified. This document exists so
that one sitting with a board settles all four, rather than four separate
afternoons discovering them one at a time.

Everything checkable without hardware already is: 33 firmware tests run on the
host, both gates agree on the same limit table, the compensation arithmetic
matches the datasheet's independent algorithm, and every pin number in this
document is checked against the firmware by
`scripts/check_bench_constants.py`. What is left genuinely needs a board.

Record results in [§5](#5-recording). A row filled in with "worked" and no date,
commit or observation is the kind of claim this project has spent a month
removing.

---

## 0. Before you start

| | |
|---|---|
| Board | ESP32-S3, default build (the XIAO pin map, whatever board it is on) |
| Flash | `cd firmware/obc-esp32-s3 && cargo run` — see BRINGUP.md §1 |
| Serial | 115200, native USB-Serial-JTAG, newline-delimited JSON |
| Wire | LED + 330 Ω from **GPIO 3** to ground; a second on **GPIO 7** |
| Sensor | BME280 on **SDA=GPIO5, SCL=GPIO6** — the pads the silkscreen marks |

**GPIO numbers, not header labels.** The silk differs per board: on a XIAO,
GPIO 3 and 7 are `D2` and `D8`; on a FireBeetle they are elsewhere. Find the pad
that carries GPIO 3 on your board's vendor pinout. A `D`-number copied from
another board's card is the mistake this whole document exists to avoid.

Note the firmware commit before you begin: `git rev-parse --short HEAD`.

---

## 1. Does a refusal stop the wire moving?

This is the load-bearing one. The safety case rests on the node holding its own
copy of the limit table, so that a compromised or absent host cannot talk it
round. Six host-side tests assert the gate refuses correctly. None of them can
see a wire.

1. **Boot.** The banner must read `obc-esp32-s3-001`. Anything else and every
   step below tests the boot policy instead of the pushed one.

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

1. **The banner.** `I2C sensor bus ready (SDA=5, SCL=6)`. If it says 4/5 you are
   running an old build.

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

## 4. Does the Waveshare have a camera connector?

Two files in this repository disagree, and nobody has looked at the board.

- `docs/HARDWARE-TEST-WALKTHROUGH.md` has said "n/a on Waveshare — no
  connector" since 2026-07-30.
- `firmware/obc-esp32-s3/src/camera.rs` says its pin map **is** the Waveshare's:
  "OV2640 via the FPC connector", SCCB on GPIO4/5.

This needs no power — look at the board and say whether an FPC connector is
present. Whichever way it goes, one of those two files is wrong and should be
corrected in the same sitting.

While you are there: the camera feature disables the I²C sensor bus, and the
stated reason ("the same pins as the sensor bus") is no longer true of either
build. If the connector exists and you can run a camera build, check whether the
sensor bus can simply stay on.

---

## 5. Recording

Fill this in as you go, not afterwards.

```
date:              ____________________
firmware commit:   ____________________
board:             ____________________  (XIAO / FireBeetle / Waveshare / other)
build:             default | --features board-waveshare-21 | --features camera

1. refusal stops the wire
   boot banner said:            ____________________
   set_limits returned applied: ____________________
   control (pin 3 lights):      pass / fail
   refusal (pin 8 dark):        pass / fail   measured with: eye / meter / scope
   refuses with host stopped:   pass / fail
   rate limit holds:            pass / fail

2. BME280 on 5/6
   banner SDA/SCL:              ____________________
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

# Bench Pinout Cards — MVB Boards

One card per Minimum-Viable-Bench board, focused on the pins **this project's firmware
actually drives** (for soldering/probing the bench), plus power/USB/boot and which pins are
free vs reserved. Companion to `BENCH-TEST-HARDWARE.md` and `BENCH-MVB-WIRING.svg`.

**Source tags:** `[fw]` = assigned in our firmware (file cited) · `[board]` = vendor board
reference (fixed by the board, not our code). **Always confirm against the vendor pinout /
silkscreen before soldering** — GPIO↔header mapping and safe pins vary by board revision.

---

## Card 0 — Which board is which

The three Heltecs are physically identical. Every other doc refers to them by *role*
(`heltec-base`, `heltec-gw`, `heltec-relay`), but the logs never say that — the firmware
derives its id from the MAC (`node = mac[5]`, `main.rs`) and reports `gw-40`, `gw-90`,
`gw-D8`. Nobody wrote the mapping down, and on 2026-07-19 that cost four separate
detours: the XIAO was assumed to be on the bridge when it was on the base, a jumper swap
silently reversed the working leg, the base's identity was inferred wrongly from traffic,
and two docs pointed at a COM port belonging to the XIAO.

Node ids are MAC-derived, so they are permanent per board. Roles are tape. **When the two
disagree, believe the node id.**

Confirmed by boot banner and physically labelled, 2026-07-19:

| Role | Node id | Port | Power | Wiring |
|---|---|---|---|---|
| base (host link) | **gw-D8** | **COM3** | PC USB — *must* stay on the host | none |
| bridge (field) | **gw-40** | — | wall or power bank | the XIAO jumper pair |
| relay (Stage 3b) | **gw-90** | — | USB power only | none — radio only |
| node | `obc-esp32-s3-001` | **COM6** | USB or bank | jumpers to **gw-40** |

All three radios self-test clean (`status=0xA2`, syncword readback `0x1424`).

**Keep the relay unpowered outside Stage 3b.** On 2026-07-19 a frame carrying a
host-originated command was observed with `src=90` — the relay — which means all three
radios were live and the topology under test was a three-radio flood, not the two-radio
path everyone was reasoning about. Frame paths that "don't add up" are usually this.

To re-confirm identity after any swap, read the banner rather than inferring from
traffic (`main.rs`):

```
Gateway 40 — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.
```

Power each board in turn, note the two hex digits, write them on tape *and* in this
table. Ten minutes once, versus inferring it wrongly every time.

⚠ `BENCH-WALKTHROUGH.md` §3.3 and `PHASE-B-LORA-MESH.md` both showed the base on **COM6**.
That was stale — COM6 is the XIAO on this bench. Ports re-enumerate; confirm before trusting
any port in any doc. Node ids do not.

**The base cannot move to battery.** It is the brain's serial link, not just a radio. To
separate the radios, move the *bridge* (and the XIAO with it — they are jumpered together).

**TX always lands on RX.** Both jumpers are directional and swapping the pair kills both
directions at once, which looks exactly like a dead node:

```
XIAO D6 (GPIO43, TX)  ──►  gw-40 GPIO2 (RX)     node → mesh
gw-40 GPIO4 (TX)      ──►  XIAO D7 (GPIO44, RX) mesh → node
XIAO GND              ◄─►  gw-40 GND            common reference
```

**Two RF facts that keep resurfacing:**

- Target keepalive RSSI **−45 to −60 dBm**. Above about −35 the receiver overdrives:
  ~55 B keepalives still pass while 120 B+ frames vanish, so the link looks healthy and
  commands silently disappear. Cost an evening on 2026-07-17 and recurred 2026-07-19.
  `snr=` in the `SPINE ◄` line separates the two cases — weak-and-clean is range,
  strong-and-dirty is saturation.
- A Heltec **wired directly to the node** transmits that frame and then de-dups its own
  echo, so it never logs a `SPINE ◄` line for it. The host sees silence from a perfectly
  healthy node. Only the bridge should carry the jumpers.

---

## Card 1 — Heltec WiFi LoRa 32 V3  (ESP32-S3 + SX1262)

Role: **all three Station B radios** — `heltec-base`, `heltec-relay`, `heltec-gw` (field).
`firmware/heltec-lora-linktest`. Same card for all three: identical radio pinout and config.
The **relay** is radio-only — antenna + USB power, **no external wiring at all** (the UART
bridge below applies only to `heltec-gw`); it forwards frames (TTL−1, de-dup) and earns its
keep in walkthrough **Stage 3b** (true 3-hop test).

**SX1262 radio (SPI)** — `[fw]` `src/sx1262.rs`
| Signal | GPIO | | Signal | GPIO |
|---|---|---|---|---|
| NSS (CS) | 8 | | RST | 12 |
| SCK | 9 | | BUSY | 13 |
| MOSI | 10 | | DIO1 (IRQ) | 14 |
| MISO | 11 | | TCXO | via **DIO3** (1.8 V) |
| | | | RF switch | via **DIO2** |

**Phase-B UART bridge to the XIAO node** (`heltec-gw` **only**) — `[fw]` `docs/PHASE-B-LORA-MESH.md`
| Signal | Heltec GPIO | Direction |
|---|---|---|
| RX (from XIAO TX) | **GPIO2** | XIAO GPIO43 → here |
| TX (to XIAO RX) | **GPIO4** | here → XIAO GPIO44 |
| GND | GND | common ground (verify with meter) |

**Board reference** — `[board]`
| Function | GPIO |
|---|---|
| OLED I2C SDA / SCL / RST | 17 / 18 / 21 |
| Vext power control (active-LOW, powers OLED) | 36 |
| User LED | 35 |
| ADC battery divider (VBAT) | 1 (via on-board divider, board rev dependent) |
| USB-C (CP210x on some clones / native S3 on V3) + BOOT(GPIO0) + RST | — |

⚠ **Attach the 915/868 MHz antenna before any TX.** Radio config: SF7 / BW125 / CR4-5 /
syncword 0x1424 / +22 dBm. Free GPIO for probing: avoid the SX1262 + OLED + Vext pins above.

---

## Card 2 — Seeed XIAO ESP32-S3 Sense

Role: **mesh sensor/camera node** behind the field Heltec (Station B). Runs `obc-esp32-s3`.

**UART bridge to Heltec** — `[fw]` `docs/PHASE-B-LORA-MESH.md`
| Silk | GPIO | Function |
|---|---|---|
| **D6** | **43** | TX → Heltec GPIO2 |
| **D7** | **44** | RX ← Heltec GPIO4 |
| GND | GND | common ground |

**Board reference** — `[board]` (14-pin, both sides; 3V3 / GND / 5V on the power end)
| Silk | GPIO | | Silk | GPIO |
|---|---|---|---|---|
| D0 | 1 | | D6 | 43 (UART0 TX) |
| D1 | 2 | | D7 | 44 (UART0 RX) |
| D2 | 3 | | D8 | 7 (SCK) |
| D3 | 4 | | D9 | 8 (MISO) |
| D4 | 5 (SDA) | | D10 | 9 (MOSI) |
| D5 | 6 (SCL) | | 3V3 / 5V / GND | power |

**Sense expansion board** `[board]`: OV2640 camera + PDM mic are wired to the S3 via the
Sense daughterboard (DVP + PDM), plus a microSD slot — not broken out to the 14-pin header.
Free header pins for probing: D0–D5, D8–D10 (mind D4/D5 = I2C, D8–D10 = SPI if used).

---

## Card 3 — Waveshare ESP32-S3-Touch-LCD-2.1

Role: **primary control / reflex-safing + sensing node** (Station A). Runs `obc-esp32-s3`
built with **`--features board-waveshare-21`** — the default build is the XIAO pin map and
would drive this board's LCD lines as GPIO. *(Card corrected 2026-07-16 against the
Waveshare wiki/schematic — the old card's 3/14/26/33/46 outputs and 4/5 I2C don't exist here.)*

**The board exposes exactly three connectors** — `[board]`
| Connector | Pins |
|---|---|
| 12-pin header | GND ·5V· **GPIO19/20** (native-USB D−/D+) · 3V3 · **SCL=7 / SDA=15** (I2C-only) · **TXD=43 / RXD=44** · NC · **IO0=GPIO0** |
| I2C connector | GND / 3V3 / SCL=**7** / SDA=**15** (same bus as header) |
| UART connector | GND / 3V3 / 43 / 44 — dead while the UART Type-C is plugged in |

**No silkscreen on the header** — fingerprint it: (power off) two pins have continuity to the
USB shell = GNDs at positions 1 & 5, and the end where a GND is *outermost* is the pin-1 end;
hold **BOOT** → one more pin gains GND continuity = **IO0, pin 12** (the DHT22 pin). Power on:
pin 2 ≈ 5 V, pin 6 = 3.3 V, pins 7/8 idle ≈ 3.3 V. Full steps: datasheet §fingerprint.

**Firmware-assigned (`board-waveshare-21`)** — `[fw]` `firmware/obc-esp32-s3`
| Subsystem | Pins (GPIO) |
|---|---|
| **Safe output pins** (Track-0 GPIO writes) | **43, 44** (UART1 spine uplink disabled on this build) |
| DHT22 data | **0** (header pin IO0) + 10 kΩ pull-up to 3V3 |
| I2C bus (sensors) | SDA **15** · SCL **7** (hardwired connector) |
| Command I/O | native USB-Serial-JTAG (GPIO19/20 — the "USB" Type-C) |
| Camera / I2S mic | **not possible on this board** — stubs |

**Station-A sensor hookups**
| Peripheral | Wiring |
|---|---|
| BME280 @0x76 / MPU-6050 @0x68 | I2C connector SDA=15, SCL=7 — bus already carries touch/IMU/RTC at 0x15/0x20/0x51/0x6B/0x7E; no conflict |
| DHT22 | + →3V3 · out → **IO0** (+10 kΩ→3V3, keeps BOOT strap high) · − →GND |
| LED + 330 Ω | **GPIO43** (header TXD pin) → LED → GND · `gpio_write pin 43` |

**Board reference** — `[board]` (`docs/datasheets/waveshare-esp32-s3-touch-lcd-2.1.md`):
round 480×480 RGB LCD (ST7701), CST820 touch, QMI8658 IMU + PCF85063 RTC onboard (free
extra sensors on the I2C bus!), TCA9554 expander internal-only, two Type-C ports.
⚠ The LCD consumes GPIO 1–3, 5–14, 16–18, 21, 38–41, 45–48 — never drive those as GPIO.
⚠ GPIO0 is the BOOT strap — anything on it must idle HIGH at reset (the DHT22 + pull-up does).

---

## Card 4 — Espressif ESP32-S3-EYE v2.2

Role: **ClawCam camera-trap node** (Station C). `firmware/clawcam_node_espidf`,
`boards/esp32_s3_eye_v22.json`. *Pinmap unverified until bench tests pass.*

**Camera (OV2640, DVP)** — `[fw]` `esp32_s3_eye_v22.json`
| Signal | GPIO | | Signal | GPIO |
|---|---|---|---|---|
| XCLK | 15 (16 MHz) | | D0–D7 | 11, 9, 8, 10, 12, 18, 17, 16 |
| SIOD (SDA) | 4 | | VSYNC | 6 |
| SIOC (SCL) | 5 | | HREF | 7 |
| | | | PCLK | 13 |

**Storage (microSD, SDMMC 1-bit)** — `[fw]`
| Signal | GPIO | Mount |
|---|---|---|
| D0 | 40 | `/sdcard` |
| CMD | 38 | (FATFS) |
| CLK | 39 | |

**Motion / power** — `[fw]`
| Function | Value |
|---|---|
| PIR wake (EXT0) | **unassigned** (`pir_gpio = -1`) → **wire an HC-SR501/AM312 to a free RTC-capable GPIO** |
| Battery ADC | `battery_adc_channel = -1` (battery pads only; no on-board gauge) |
| Low-battery threshold | 3.55 V |

⚠ The S3-EYE has **no built-in PIR** — the camera-trap wake path needs an external PIR on an
EXT0-capable pin. Confirm the pinmap on first bench flash (status: `unverified`).

---

## Probing quick-tips
- **Common ground first.** Every cross-board link (UART bridge, PIR, sensors) needs a shared
  GND — meter it before powering.
- **Don't probe LoRa RF pins live**; keep the antenna on.
- **Boot/flash:** ESP32-S3 boards enter download mode via BOOT(GPIO0)+RST; native-USB S3
  boards usually auto-reset with `espflash`.
- **Safe outputs only** for the Waveshare GPIO smoke test — 3/14/26/33/46 (Track-0 allow-list).
- When in doubt, cross-check the vendor pinout — these cards cover the *project-used* pins,
  not every header pin.

*Sources: `Oh-Ben-Claw/firmware/heltec-lora-linktest/src/sx1262.rs`,
`Oh-Ben-Claw/firmware/obc-esp32-s3/{BRINGUP.md,CAMERA.md}`,
`Oh-Ben-Claw/docs/{PHASE-B-LORA-MESH.md, datasheets/waveshare-esp32-s3-touch-lcd-2.1.md}`,
`ClawCam/firmware/clawcam_node_espidf/boards/esp32_s3_eye_v22.json`. Board-reference rows
are vendor pinouts — verify against current silkscreen/datasheet.*

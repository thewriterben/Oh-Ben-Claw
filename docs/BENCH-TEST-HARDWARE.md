# Bench-Test Hardware — Full Rig (BOM + Checklist)

Everything needed to bench-test the OBC / ClawCam ecosystem end to end: the shared core
rig plus a station per test area (MCU control, LoRa mesh, camera-trap node, GNSS, cellular/
satellite backhaul, drone, edge-AI, gateway host). Every part here is referenced in the
code or firmware — see the citation after each station.

**How to read this**
- Each station has a **checklist** (what + why + qty) then a **BOM table** (part, example
  model, qty, approx cost, status).
- **Status:** `DRIVER` = code exists · `REGISTRY` = catalogued, no host driver ·
  `PLANNED` = referenced in docs only.
- **Costs are approximate USD for planning only — verify with a current supplier.** Region
  matters for LoRa (915 MHz US ISM / 868 MHz EU) and antennas.
- If you only want to keep the current tracks moving, build **Core + Station A + Station B**
  first (that's the validated path); the rest map to the not-yet-built phases (G3/G6/G8).

---

## ⭐ Minimum-Viable Bench (start here — one order)

The smallest kit that exercises the **validated + in-progress** tracks: MCU control &
sensing (Station A), the LoRa mesh spine (Station B), and the ClawCam camera node (Station
C), reporting to a gateway host. Buy this first; add Stations D–G only when you reach those
phases (GNSS/satellite/drone/edge-AI).

**Assumes you already have:** a host workstation with the toolchains, a multimeter, and
USB-C data cables. If not, add the *Core rig* rows from §0.

| # | Item | Qty | ~Cost | Unlocks |
|---|---|---|---|---|
| 1 | Heltec WiFi LoRa 32 V3 (SX1262) | 3 | $66 | Mesh spine, relay/multi-hop (Station B) |
| 2 | 915/868 MHz LoRa antenna (match region) | 3 | $12 | **Required before any TX** |
| 3 | Seeed XIAO ESP32-S3 Sense | 2 | $28 | Mesh sensor/camera node + UART bridge |
| 4 | Waveshare ESP32-S3-Touch-LCD-2.1 | 1 | $28 | Primary control/reflex/safing node (Station A) |
| 5 | Espressif ESP32-S3-EYE v2.2 | 2 | $100 | ClawCam camera-trap node (Station C) |
| 6 | PIR sensor (HC-SR501/AM312) | 3 | $6 | Camera wake trigger (S3-EYE has none) |
| 7 | DHT22 (temp/humidity — has a driver) | 2 | $8 | First sensor bring-up (Station A) |
| 8 | BME280 + MPU-6050 (Qwiic) | 1 ea | $11 | Next sensor + IMU drivers |
| 9 | LED + resistor assortment | 1 | $6 | Track-0 GPIO output smoke test |
| 10 | microSD 16 GB Class 10 + reader | 4 | $30 | Capture storage |
| 11 | LiPo 3.7V 1000 mAh + charger | 3 | $34 | Battery / deep-sleep paths |
| 12 | Breadboard + jumper wire kit | 1 | $12 | Wiring the bridge + sensors |
| 13 | USB–UART adapter (CP2102N) | 1 | $8 | Base-station console feed |
| 14 | Raspberry Pi 5 (8 GB) kit + SSD | 1 | $120 | Gateway host (ClawCam + brain) |

**Rough total: ~$470** (drop the Pi 5 if you'll run the gateway on your workstation → ~$350;
add a $12 logic analyzer from §0 if you don't have a scope).

**Suggested bring-up order once it arrives:**
1. Flash one **Heltec V3**, confirm boot/OLED, then a **two-Heltec ping** (Station B, antennas on).
2. Bring up the **Waveshare ESP32-S3** control path — LED smoke test, then **DHT22** read (Station A).
3. Add the **XIAO ↔ Heltec UART bridge** (Appendix B pins) → a sensor summary over the mesh.
4. Flash an **ESP32-S3-EYE**, wire a **PIR**, verify capture → microSD → wake (Station C).
5. Stand up the **Pi 5 gateway**, point a node at it, confirm end-to-end ingest.

**Wiring:** see `docs/BENCH-MVB-WIRING.svg` — base Heltec ↔ (LoRa) ↔ field Heltec ↔ XIAO
(UART bridge), the Waveshare control node + I2C/DHT22/LED, the ESP32-S3-EYE + PIR/microSD,
all reporting to the Pi 5 gateway.

*(Detailed per-station tables, statuses, and citations are in §1–§8 below.)*

---

## 0. Core bench rig — needed for every station

- [ ] **Host workstation** — Windows/Linux/Mac with the toolchains: `espflash`/`espup`
      (ESP32), `probe-rs` or ST-Link (STM32), Python 3.11 + venv (ClawCam gateway), Rust
      (OBC). You already have this.
- [ ] **USB-C + micro-USB data cables** (not charge-only) — one per board on the bench.
- [ ] **Powered USB hub** — multiple boards draw more than a laptop port likes.
- [ ] **USB–UART adapter(s)** — CP2102N / FT231X / CH340 — for the base-station console
      feed and any board without native USB. Windows needs the **Silicon Labs CP210x VCP
      driver** for Heltec.
- [ ] **Multimeter** (continuity/voltage — verifying shared GND on UART bridges is called
      out explicitly in Phase B) and, ideally, a **basic oscilloscope or logic analyzer**
      for UART/SPI/I2S bring-up.
- [ ] **Breadboard + DuPont jumper wires** (M-M, M-F) + **micro-USB/USB-C breakouts**.
- [ ] **Bench power** — 5V/3.3V supply or good USB power; **LiPo cells + chargers** for the
      battery/deep-sleep paths.
- [ ] **microSD cards** (several, 8–32 GB, Class 10) + a card reader.
- [ ] **Qwiic/STEMMA-QT + Grove cables** — the sensor ecosystems the registry uses, so you
      can plug sensors without soldering.

| Item | Example | Qty | ~Cost | Status |
|---|---|---|---|---|
| USB-C data cables | — | 4–6 | $3 ea | — |
| Powered USB hub (7-port) | — | 1 | $25 | — |
| USB–UART adapter | CP2102N / FT231X | 2 | $8 ea | REGISTRY (chips catalogued) |
| Multimeter | any | 1 | $25 | BENCH |
| Logic analyzer (8-ch) | Saleae-clone 24 MHz | 1 | $12 | BENCH |
| Breadboard + jumper kit | 830-pt + 120 wires | 2 | $12 | — |
| LiPo 3.7V 1000–2000 mAh + charger | JST-PH | 3 | $8 ea | — |
| microSD 16 GB Class 10 + reader | — | 4 | $6 ea | DRIVER (storage) |
| Qwiic/STEMMA-QT + Grove cable packs | Adafruit/Seeed | 1 | $12 | REGISTRY |

---

## 1. Station A — MCU control & sensing node (reflex/safing, I2C sensors, actuators)

The validated ESP32-S3 control path (GPIO/reflex/safing) plus the sensor drivers you're
bringing up (DHT22 done; BME280/MPU6050 next).

- [ ] **Primary node board** — Waveshare ESP32-S3-Touch-LCD-2.1 (main `obc-esp32-s3`
      target) and/or a **Seeed XIAO ESP32-S3 Sense** (camera+mic+mesh node).
- [ ] **DHT22/AM2302** temp+humidity — the one sensor with a finished driver (`dht.rs`).
- [ ] **BME280** (temp/humidity/pressure, I2C @0x76) and **MPU-6050** (IMU @0x68) — next
      sensor drivers to validate; the `sensors.rs` host driver already reads them.
- [ ] **LED + resistor** on an allow-listed output pin (3/14/26/33/46) — the Track-0 GPIO
      output smoke test.
- [ ] **Actuator set** for the Movement suite — **SG90 servo**, **TB6612FNG** motor driver
      + small DC motor, and/or a **PCA9685** 16-ch servo driver.
- [ ] Optional display/audio for suite tests: **INMP441** I2S mic, **MAX98357A** I2S amp +
      small speaker (both catalogued as accessories).

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Waveshare ESP32-S3-Touch-LCD-2.1 | Waveshare | 1 | $28 | DRIVER (primary target) |
| Seeed XIAO ESP32-S3 Sense (OV2640 + PDM mic) | Seeed 102010496 | 2 | $14 ea | DRIVER (Phase B node) |
| DHT22 / AM2302 | AOSONG | 2 | $4 ea | **DRIVER** (`dht.rs`) |
| BME280 breakout (Qwiic) | Adafruit/SparkFun | 2 | $6 ea | DRIVER (host read) |
| MPU-6050 IMU (Qwiic) | — | 1 | $5 | DRIVER (host read) |
| LED + 330Ω resistors | assortment | 1 pk | $6 | BENCH (GPIO smoke test) |
| SG90 micro servo | TowerPro | 2 | $3 ea | REGISTRY (Movement) |
| TB6612FNG motor driver + DC motor | Toshiba | 1 | $8 | REGISTRY (Movement) |
| PCA9685 16-ch PWM/servo driver | NXP @0x40 | 1 | $6 | REGISTRY (Movement) |
| INMP441 I2S mic + MAX98357A amp + speaker | — | 1 | $12 | REGISTRY (Audio suite) |

*Cite: `firmware/obc-esp32-s3/{BRINGUP.md,CAMERA.md}`, `src/peripherals/{sensors.rs,registry.rs}`, tasks 172/173/178.*

---

## 2. Station B — LoRa mesh (Phase B spine + G2 camera-on-mesh)

The other hardware-validated path (Heltec V3 SX1262 link). Multi-node needs ≥3 radios so
you can test relay/multi-hop, not just point-to-point.

- [ ] **3× Heltec WiFi LoRa 32 V3** (ESP32-S3 + SX1262 + OLED) — one base station + two
      mesh nodes. **915 MHz** (US) or **868 MHz** (EU) to match your region.
- [ ] **LoRa antennas** — **one per radio, attached before any TX** (transmitting without
      an antenna can damage the PA). Match the band.
- [ ] **XIAO ESP32-S3** as the sensor/camera node behind a Heltec, for the **Phase B UART
      bridge** (XIAO GPIO43 → Heltec GPIO2, shared GND; reverse Heltec GPIO4 → XIAO GPIO44).
- [ ] Optional GPS-bearing mesh handhelds if you want position + mesh in one: **LILYGO
      T-Beam** (NEO-6M GPS) or **T-Deck Plus** (u-blox MIA-M10Q).
- [ ] Jumper wires for the UART bridge; multimeter to confirm the shared ground.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Heltec WiFi LoRa 32 V3 (SX1262) | Heltec HTIT-WB32LA | 3 | $22 ea | **DRIVER** (`sx1262.rs`, validated) |
| 915/868 MHz LoRa antenna (u.FL/SMA) | — | 3 | $4 ea | **MANDATORY before TX** |
| Seeed XIAO ESP32-S3 | Seeed | 1 (shared w/ Station A) | $10 | DRIVER (bridge node) |
| LILYGO T-Beam (SX1262/76 + NEO-6M GPS) | LILYGO | 1 | $35 | REGISTRY + firmware |
| LILYGO T-Deck Plus (SX1262 + GNSS + kbd) | LILYGO | 1 | $75 | DRIVER (`t-deck-terminal`) |
| RAK4631 WisBlock (nRF52840 + SX1262) | RAKwireless | 1 (optional) | $20 | REGISTRY + firmware |

*Cite: `firmware/{heltec-lora-linktest/src/sx1262.rs, lora-node/, t-deck-terminal/}`, `docs/PHASE-B-LORA-MESH.md`. Radio: 915 MHz US, SF7/BW125/CR4-5, syncword 0x1424, +22 dBm.*

---

## 3. Station C — ClawCam camera-trap node (G2 physical camera + perception)

The dedicated camera-trap firmware target. Nothing here is hardware-verified yet — this is
the bench work that promotes it.

- [ ] **Espressif ESP32-S3-EYE v2.2** — the first ClawCam board profile (ESP32-S3, OV2640,
      8 MB PSRAM, microSD, battery pads).
- [ ] **External PIR sensor** — the S3-EYE has **no built-in PIR**; wire an **HC-SR501**
      (or AM312) to the EXT0 wake pin. (Firmware `pir_gpio` is -1 until you assign one.)
- [ ] **microSD** (capture storage) + **LiPo cell** on the battery pads for the deep-sleep/
      low-battery path.
- [ ] Optional spare **ESP32-S3-CAM / ESP32-CAM (OV2640)** as a second camera profile.
- [ ] *(Planned add-ons, only after baseline capture works):* GPS module, **BME280/BMP280**,
      light sensor, LoRa — these map to the unbuilt `clawcam_sensors` component.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Espressif ESP32-S3-EYE v2.2 (OV2640, PSRAM) | Espressif | 2 | $50 ea | DRIVER-fw (**unverified**) |
| PIR motion sensor | HC-SR501 / AM312 | 3 | $2 ea | DRIVER-fw (external wire) |
| microSD 16–32 GB | Class 10 | (shared w/ Core) | — | DRIVER (storage) |
| LiPo 3.7V + JST | 1000 mAh | 2 | $8 ea | DRIVER (power/deep-sleep) |
| ESP32-S3-CAM (OV2640) spare | Freenove/AI-Thinker | 1 | $12 | REGISTRY |
| BME280 / BMP280 (planned node env) | — | 1 | $5 | PLANNED (`clawcam_sensors`) |

*Cite: `ClawCam/firmware/clawcam_node_espidf/{boards/esp32_s3_eye_v22.json, BUILD_ESP32_S3_EYE.md}`, `ClawCam/docs/{HARDWARE_GUIDE.md, MIGRATION_FROM_WILDCAM.md}`.*

---

## 4. Station D — Positioning / GNSS (Conservation Grid G3)

Real lat/lon to replace the dormant geo columns. `gps` is a registry capability but there's
no GNSS driver yet — this station builds it.

- [ ] **A GNSS-bearing board or module** feeding NMEA over UART. Cheapest path: a bare
      **u-blox NEO-6M/NEO-M8N** module. Integrated path: **LILYGO T-Beam** (GPS + LoRa in
      one) or **T-Deck Plus** (u-blox MIA-M10Q). Cellular+GNSS combo: **SIM7600**.
- [ ] **Active GPS antenna** (most modules need one) + a window/outdoor run for sky view.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| u-blox NEO-M8N GPS module + antenna | GY-NEO8M | 2 | $12 ea | PLANNED (no driver) |
| LILYGO T-Beam (GPS+LoRa) | LILYGO | 1 (shared w/ B) | $35 | REGISTRY |
| Active GPS antenna (SMA/u.FL) | 28 dB | 2 | $6 ea | — |

*Cite: `src/peripherals/registry.rs` (`gps` token, T-Beam/T-Deck-Plus/SIM7600), `docs/CONSERVATION-GRID-STRATEGY.md` §G3.*

---

## 5. Station E — Cellular & satellite backhaul (G6)

Off-grid uplink. Cellular has a registry entry; satellite is doc-only (no code).

- [ ] **SIM7600** LTE Cat-4 modem (UART, also carries GNSS) + a **data SIM** + LTE antenna
      — the cellular backhaul path.
- [ ] **Satellite modem** for true off-grid: **RockBLOCK 9603 (Iridium SBD)** or a **Swarm
      M138** — plus their antennas and a service/credit plan. *(G6 is unbuilt — this is
      exploratory hardware; expect firmware/`sat_gateway.rs` work.)*

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| SIM7600 LTE + GNSS module | SIMCom | 1 | $35 | REGISTRY (no host driver) |
| LTE antenna + data SIM | — | 1 | $15 + plan | — |
| Iridium SBD modem | RockBLOCK 9603 | 1 | $250 | PLANNED (no code) |
| Swarm satellite modem (alt) | Swarm M138 | 1 | $120 + plan | PLANNED |

*Cite: `docs/CONSERVATION-GRID-STRATEGY.md` §G6 (Iridium/Swarm, proposed `src/spine/sat_gateway.rs`).*

---

## 6. Station F — Drone / aerial tier (G8)

The `aerial` adapter maps drone telemetry into the fleet; this station feeds it a real
MAVLink link. This is the most involved/expensive station — treat as a later phase.

- [ ] **Flight controller** running PX4 or ArduPilot — **Holybro Pixhawk 6C** (or a small
      **SpeedyBee F405**). MAVLink is what `AerialTelemetry` will be filled from.
- [ ] **Telemetry radio pair** (SiK 915/433 MHz) or MAVLink-over-Wi-Fi (ESP8266 bridge) to
      get telemetry to the host.
- [ ] **GPS/compass module** for the FC, **companion computer** (RPi Zero 2 W / any Station
      G board) if running the adapter on-craft, LiPo + safe **props-off test rig**.
- [ ] For pure software-in-the-loop first: **PX4 SITL / jMAVSim / Gazebo** on the host — no
      hardware, validates the MAVLink→NodeState mapping before you fly.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Flight controller (PX4/ArduPilot) | Holybro Pixhawk 6C | 1 | $200 | PLANNED (adapter done) |
| GPS/compass for FC | M8N/M9N | 1 | $25 | PLANNED |
| SiK telemetry radio pair | Holybro 915 MHz | 1 | $35 | PLANNED |
| (SITL first — software only) | PX4 SITL | — | $0 | recommended before hardware |

*Cite: `src/aerial/mod.rs`, `docs/CONSERVATION-GRID-STRATEGY.md` §G8.*

---

## 7. Station G — Edge-AI acceleration (optional, perception speed-up)

Registry-catalogued accelerators for running detectors (MegaDetector/BirdNET/SpeciesNet)
faster than an ESP32. Pick **one** to start — the Coral USB is the cheapest way in.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Google Coral USB Accelerator (Edge TPU) | Coral | 1 | $60 | REGISTRY |
| Raspberry Pi AI HAT+ (Hailo-8L 13 TOPS) | RPi | 1 | $70 | REGISTRY |
| Raspberry Pi AI Camera (Sony IMX500) | RPi | 1 | $70 | REGISTRY |
| Luxonis OAK-D Lite (Myriad X + depth) | Luxonis | 1 | $90 | REGISTRY |
| NVIDIA Jetson Orin Nano (Super) | NVIDIA | 1 | $250 | REGISTRY (also gateway) |

*Cite: `src/peripherals/registry.rs` (Coral / Hailo / Jetson / OAK / IMX500 entries).*

---

## 8. Gateway & brain host (Tier C/D)

Off-node compute running the ClawCam gateway (FastAPI + SQLite) and the OBC brain adapter.

- [ ] **Gateway host** — Raspberry Pi 5 (8 GB) or NVIDIA Jetson Orin Nano (if doing heavier
      on-gateway inference) or any Linux mini-PC. Needs Python 3.11 + the venv.
- [ ] **Brain host** — can be the same machine or your workstation; runs the OBC runtime.
- [ ] PoE/DC power + storage (SSD/large SD) for detection media + the SQLite DB.

| Item | Example model | Qty | ~Cost | Status |
|---|---|---|---|---|
| Raspberry Pi 5 8 GB + PSU + case + SSD | RPi | 1 | $120 | DRIVER (`rpi.rs`) |
| NVIDIA Jetson Orin Nano (alt, inference) | NVIDIA | 1 | $250 | REGISTRY |

---

## Appendix A — Consolidated shopping list

**Minimum-viable bench (keep current tracks moving — control + mesh + camera):**
Core rig · 1× Waveshare ESP32-S3 · 2× XIAO ESP32-S3 Sense · 3× Heltec V3 + 3 antennas ·
DHT22 + BME280 + MPU6050 · 2× ESP32-S3-EYE + 3 PIRs · microSD ×4 · LiPo ×3 · RPi 5.
**Rough total: ~$400–500.**

**Full rig (adds the unbuilt phases):** the above + GNSS (Station D ~$40) + cellular
(SIM7600 ~$50) + satellite modem (Station E $120–250) + drone/FC (Station F ~$260) +
one edge accelerator (Station G $60–90). **Rough total: ~$950–1,150** (satellite +
drone dominate; both are later-phase).

---

## Appendix B — Wiring quick-reference (from firmware pin maps)

**Heltec V3 ↔ SX1262** (`sx1262.rs`): NSS=8, SCK=9, MOSI=10, MISO=11, RST=12, BUSY=13,
DIO1=14; TCXO on DIO3 (1.8 V); RF switch on DIO2.

**Phase B LoRa UART bridge** (`PHASE-B-LORA-MESH.md`): XIAO **D6/GPIO43 → Heltec GPIO2**
(+ shared GND); reverse command path **Heltec GPIO4 → XIAO D7/GPIO44**. Confirm the shared
ground with a multimeter before powering.

**Waveshare ESP32-S3 bring-up** (`obc-esp32-s3/BRINGUP.md`): UART0 TX=43/RX=44 · I2C SDA=4/
SCL=5 · I2S mic SCK=0/WS=1/SD=2 · OV2640 XCLK=15/SIOD=4/SIOC=5, D0–D7=39–42,16–19, VSYNC=21,
HREF=38, PCLK=13 · safe output pins 3,14,26,33,46.

**ClawCam ESP32-S3-EYE** (`esp32_s3_eye_v22.json`): camera XCLK=15, SIOD=4, SIOC=5,
D0–D7=11/9/8/10/12/18/17/16, VSYNC=6, HREF=7, PCLK=13 · SD (SDMMC 1-bit) D0=40/CMD=38/
CLK=39 → `/sdcard` · **PIR: unassigned (wire to an EXT0-capable GPIO)** · low-batt 3.55 V.

**T-Deck** (`t-deck-terminal`): power-gate **GPIO10 HIGH first** · shared SPI SCK40/MISO38/
MOSI41 → ST7789 (CS12,DC11,BL42), SX1262 (CS9,DIO1 45,RST17,BUSY13), microSD (CS39) ·
keyboard I2C SDA18/SCL8 · GPS UART 43/44 (Plus).

---

*Sources: `Oh-Ben-Claw/src/peripherals/{registry.rs,sensors.rs,bus_tools.rs,stm32.rs,rpi.rs}`,
`Oh-Ben-Claw/firmware/{heltec-lora-linktest,lora-node,t-deck-terminal,obc-esp32-s3}`,
`Oh-Ben-Claw/docs/{PHASE-B-LORA-MESH.md,HARDWARE-TEST-WALKTHROUGH.md,T-DECK-RESEARCH.md,CONSERVATION-GRID-STRATEGY.md}`,
`ClawCam/firmware/clawcam_node_espidf/**`, `ClawCam/docs/{HARDWARE_GUIDE.md,MIGRATION_FROM_WILDCAM.md}`.*

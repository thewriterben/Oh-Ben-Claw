# Hardware Scout Report — 2026-07-06

*Automated `obc-hardware-scout` run. Proposals only — `registry.rs` is NOT edited.*

> **MERGED 2026-07-07:** shortlist items #1, 3, 6, 8, 9, 10, 11, 13, 15 plus the
> Feather ESP32-S3 **TFT** variant (`239a:811d`, verified — substituted for the
> carried-over plain Feather) and both §2b accessories are now in `registry.rs`,
> with the `vpu` token and 12 new tests. Nano ESP32 verified as `2341:0070`;
> Pico 2 W merged on the shared SDK-CDC id `2e8a:000a` (select-by-name). Still
> held for verification: #4 reCamera, #5 BeagleY-AI, #7 SenseCAP Watcher,
> #12 Feather RP2350, plain Feather ESP32-S3, MaixCAM.
*Scope: ESP32 family, MCUs/SBCs, AI accelerators, sensors, displays, radios, connector ecosystems.*

**Status vs last run (2026-06-29):** nearly all of last week's shortlist was merged
(Coral USB, ROCK 5B, Orin Nano, Module LLM, C6/H2/P4, QT Py S3, Thing Plus C6,
FireBeetle 2, T-Display-S3, T-Deck/T-Deck Plus, Coral Dev Board Mini, AI HAT+ 13T,
Grove Vision AI V2, Qwiic/STEMMA QT sensors) along with the new capability tokens.
Still unmerged from last week: **Adafruit Feather ESP32-S3** and **Sipeed MaixCAM
(K230)** — both blocked on VID/PID verification; carried in §4 below.

This week's theme: the **`vpu` token** (last accelerator gap in the taxonomy),
the **GenAI-class Hailo-10H**, the **ESP32-C5** (first dual-band Wi-Fi 6 SoC),
and first entries for **Pimoroni**, **Luxonis**, **Sophgo/reCamera**, and modern
**Arduino** — plus the popular handhelds (Cardputer, T-Lora Pager, Tab5).

---

## 1. Ranked shortlist

Legend — Transport: `serial` / `native` / `probe` / `bridge`. ⚠ = NEW capability
token (§3). 🔎 = VID/PID or spec needs verification (§4).

| # | Product | Vendor | MCU/SoC + arch | VID:PID | Transport | Connectors | Capabilities | Rationale |
|---|---------|--------|----------------|---------|-----------|------------|--------------|-----------|
| 1 | **OAK-D Lite** | Luxonis | Intel Movidius Myriad X VPU, 1.4 TOPS + 4K RGB + stereo depth | `03e7:2485` ✅ VERIFIED | serial (USB 3) | Bare | `vpu` ⚠, camera_capture | Completes the accelerator taxonomy (`vpu` was the last reserved-but-unused class in the ecosystem doc); new vendor; verified VID/PID; adds stereo *depth* — genuinely new sensing |
| 2 | **AI HAT+ 2 (Hailo-10H)** | Raspberry Pi | Hailo-10H, 40 TOPS INT4 / 20 TOPS INT8 @ ~2.5 W, PCIe (launched Jan 2026, $130) | n/a (PCIe accessory) | accessory→RPi5 | HatPi | `hailo` | First **GenAI-class** (on-device LLM/VLM) accelerator; distinct from the 13-TOPS Hailo-8L HAT already merged |
| 3 | **ESP32-C5-DevKitC-1** | Espressif | ESP32-C5 RISC-V @ 240 MHz, **dual-band 2.4/5 GHz Wi-Fi 6**, BLE 5, 802.15.4 (MP since Apr 2025, ~$15) | `303a:1001` (shared, by name) | serial | Bare | gpio, analog_read, i2c, spi, wifi, ble, thread, zigbee | New SoC family; industry-first dual-band RISC-V MCU; congestion-free 5 GHz spine links |
| 4 | **reCamera 2002w (8GB)** | Seeed Studio | Sophgo SG2002: RISC-V C906 @1 GHz + 0.7 GHz + 8051, **1 TOPS NPU**, Linux, OV5647 5MP | USB-C gadget 🔎 | native | Bare | npu, camera_capture, audio_sample, wifi, ble, microsd | First Sophgo/CVITEK-class device; self-contained Linux AI-camera node (reCamera OS + Node-RED); modular sensor/baseboard system |
| 5 | **BeagleY-AI** | BeagleBoard | TI AM67A quad Cortex-A53 @ 1.4 GHz + 4 TOPS vision accelerators (AArch64) | native SBC (no fixed USB id) 🔎 | native | HatPi | gpio, i2c, spi, pwm, npu, ethernet | Refreshes BeagleBoard (only BBB present); TI's accelerator family; Pi-HAT-compatible 40-pin |
| 6 | **M5Stack Tab5** | M5Stack | ESP32-P4 dual RISC-V @ 400 MHz + ESP32-C6 radio co-proc; 5" 1280×720 MIPI-DSI multi-touch (GT911), SC2356 2MP camera, dual mic + speaker, RS-485 | `303a:1001` 🔎 (shared, by name) | serial | MBus, Grove | gpio, i2c, spi, wifi, ble, display, touch, camera_capture, audio_sample, audio_output, microsd, psram, battery, nn_accel | Flagship ESP32-P4 product (May 2025); richest single ESP32 node yet — display+camera+audio HMI in one |
| 7 | **SenseCAP Watcher** | Seeed Studio | ESP32-S3 + Himax WiseEye2 HX6538 (Cortex-M55 + Ethos-U55), 1.45" round touch LCD, camera, mic, speaker | 🔎 | serial | Grove | gpio, i2c, wifi, ble, nn_accel, camera_capture, audio_sample, audio_output, display, touch, battery, microsd | A purpose-built "physical AI agent" — exactly the OBC node concept; pairs on-device WiseEye2 detection with LLM escalation |
| 8 | **Cardputer (v1.1)** | M5Stack | StampS3 (ESP32-S3) @ 240 MHz, 56-key keyboard, 1.14" ST7789 135×240, mic, speaker, IR TX | `303a:1001` (shared, by name) | serial | Grove | gpio, i2c, spi, wifi, ble, display, keyboard, audio_sample, audio_output, infrared, microsd, battery | Hugely popular pocket terminal; second keyboard device (after T-Deck); IR blaster for actuation. Cardputer-Adv (Sep 2025) exists — verify which SKU to model |
| 9 | **T-Lora Pager** | LILYGO | ESP32-S3 + SX1262 LoRa, 2.33" display, keyboard + rotary encoder, GPS, NFC, IMU, audio (2025) | `303a:1001` 🔎 (shared, by name) | serial | Bare | gpio, i2c, spi, wifi, ble, lora, mesh, gps, nfc, imu, display, keyboard, audio_output, battery, microsd | Newest Meshtastic handheld; first node combining LoRa + NFC + GPS + keyboard; natural fleet-pager role |
| 10 | **Presto** | Pimoroni | RP2350 dual Cortex-M33 @ 150 MHz + RM2 (CYW43439) Wi-Fi/BLE; 4" 480×480 IPS touch, piezo, SD, battery conn | `2e8a:` PID 🔎 | serial | Qwiic (Pimoroni Qw/ST) | gpio, i2c, spi, wifi, ble, display, touch, microsd, battery, audio_output | First Pimoroni entry (named vendor-coverage gap); desk-display node; Qw/ST port = Qwiic-compatible |
| 11 | **Raspberry Pi Pico 2 W** | Raspberry Pi | RP2350 dual Cortex-M33 @ 150 MHz + CYW43439 Wi-Fi/BLE | `2e8a:` PID 🔎 (see §4) | serial | Bare | gpio, analog_read, i2c, spi, pwm, wifi, ble | Registry has Pico/Pico W/Pico 2 but not the wireless RP2350 — the one users actually deploy as a spine node |
| 12 | **Feather RP2350 (HSTX)** | Adafruit | RP2350 dual Cortex-M33 @ 150 MHz, 8 MB PSRAM, HSTX DVI port, LiPo charge | `239a:` PID 🔎 | serial | FeatherWing, StemmaQt | gpio, analog_read, i2c, spi, pwm, display, battery, psram | First true Feather-format board in the registry (anchors FeatherWing matching); HSTX drives DVI displays |
| 13 | **Nano ESP32** | Arduino | ESP32-S3 (u-blox NORA-W106) @ 240 MHz, Nano form factor | `2341:0070` 🔎 | serial | Bare | gpio, analog_read, i2c, spi, wifi, ble | First wireless-era Arduino in the registry (existing entries are all AVR); bridges Arduino users into the fleet |
| 14 | **AI Camera (IMX500)** | Raspberry Pi | Sony IMX500 12.3MP sensor with **on-sensor** NN inferencing accelerator | n/a (CSI accessory) | accessory→RPi | Bare (CSI) | npu, camera_capture | Inference happens on the sensor itself — zero host load; complements HATs; works on any CSI Pi incl. Zero 2 W |
| 15 | **XIAO ESP32-C5** | Seeed Studio | ESP32-C5 RISC-V @ 240 MHz, dual-band Wi-Fi 6 (Jan 2026) | `303a:1001` 🔎 (shared, by name) | serial | Bare | gpio, analog_read, i2c, spi, wifi, ble, thread, zigbee, battery | Thumb-size form of #3; fits XIAO ecosystem already in registry; only if #3 merges |

---

## 2. Ready-to-paste Rust entries

> #1 uses the NEW `vpu` token — add it to `VALID_CAPABILITIES`, the doc-comment
> table, and a registry test before merging (see §3). All other entries reuse the
> existing taxonomy. Entries with `0x0000` placeholders must not merge until §4
> resolves them.

### 2a. `BoardInfo` additions (append to `KNOWN_BOARDS`)

```rust
    // ── Hardware-scout 2026-07-06 ─────────────────────────────────────────────
    //
    // ── Luxonis OAK-D Lite (Movidius Myriad X VPU) ────────────────────────────
    // USB3 AI stereo-depth camera. VID/PID VERIFIED: enumerates 0x03e7:0x2485
    // (Intel Movidius MyriadX) in unbooted state; DepthAI runtime loads firmware
    // over USB. First `vpu` device and first Luxonis entry.
    BoardInfo {
        vid: 0x03e7,
        pid: 0x2485,
        name: "oak-d-lite",
        architecture: Some(
            "Intel Movidius Myriad X VPU, 1.4 TOPS; 12.3MP RGB + 2x 480p stereo depth (USB 3)",
        ),
        transport: "serial",
        capabilities: &["vpu", "camera_capture"],
        vendor: "Luxonis",
        ecosystem: "OAK",
        connectors: &[Connector::Bare],
    },
    // ── Espressif ESP32-C5-DevKitC-1 ──────────────────────────────────────────
    // First dual-band (2.4/5 GHz) Wi-Fi 6 MCU; RISC-V @ 240 MHz, BLE 5,
    // 802.15.4 (Thread/Zigbee). Native USB-Serial/JTAG = 0x303a:0x1001 (shared
    // across ESP32 native-USB parts; selected by name, per existing convention).
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-c5",
        architecture: Some(
            "ESP32-C5 RISC-V single-core @ 240 MHz (dual-band 2.4/5 GHz Wi-Fi 6, BLE 5, 802.15.4; native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "analog_read", "i2c", "spi", "wifi", "ble", "thread", "zigbee",
        ],
        vendor: "Espressif",
        ecosystem: "ESP32-C5",
        connectors: &[Connector::Bare],
    },
    // ── Seeed reCamera 2002w (Sophgo SG2002, 1 TOPS) ─────────────────────────
    // Modular Linux AI camera: Core (SG2002 + 8 GB eMMC) + Sensor (OV5647) +
    // Baseboard (USB-C, microSD; PoE/CAN variants exist). Runs reCamera OS;
    // exposes a USB gadget over USB-C. USB gadget VID/PID UNVERIFIED — see §4.
    BoardInfo {
        vid: 0x0000, // ⚠ VID/PID UNVERIFIED — replace before merge (see §4)
        pid: 0x0000,
        name: "seeed-recamera-2002w",
        architecture: Some(
            "Sophgo SG2002 RISC-V C906 @ 1 GHz + 0.7 GHz, 1 TOPS NPU, 256 MB RAM, OV5647 5MP, Linux (reCamera OS)",
        ),
        transport: "native",
        capabilities: &["npu", "camera_capture", "audio_sample", "wifi", "ble", "microsd"],
        vendor: "Seeed Studio",
        ecosystem: "reCamera",
        connectors: &[Connector::Bare],
    },
    // ── BeagleY-AI (TI AM67A, 4 TOPS) ─────────────────────────────────────────
    // Credit-card SBC, Pi-HAT-compatible 40-pin header. Native Linux; no fixed
    // runtime USB-device id (USB-C is host/power) — see §4 for flash-mode id.
    BoardInfo {
        vid: 0x0000, // ⚠ no fixed USB-device id — native SBC; see §4
        pid: 0x0000,
        name: "beagley-ai",
        architecture: Some(
            "TI AM67A quad Cortex-A53 @ 1.4 GHz + C7x DSP/MMA, 4 TOPS, 4 GB LPDDR4 (AArch64)",
        ),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "npu", "ethernet"],
        vendor: "BeagleBoard",
        ecosystem: "BeagleY",
        connectors: &[Connector::HatPi],
    },
    // ── M5Stack Tab5 (ESP32-P4 + ESP32-C6 co-processor) ───────────────────────
    // 5" 1280x720 MIPI-DSI multi-touch (GT911), SC2356 2MP MIPI-CSI camera,
    // dual-mic ES7210 + NS4150B speaker, RS-485, microSD, M-Bus + Grove.
    // Wi-Fi 6 / BLE via ESP32-C6-MINI module. P4 native USB assumed
    // 0x303a:0x1001 (shared) — VERIFY on hardware; selected by name.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "m5stack-tab5",
        architecture: Some(
            "ESP32-P4 dual RISC-V @ 400 MHz + ESP32-C6 radio co-proc; 5\" 1280x720 MIPI-DSI touch, 2MP camera (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "display", "touch", "camera_capture",
            "audio_sample", "audio_output", "microsd", "psram", "battery", "nn_accel",
        ],
        vendor: "M5Stack",
        ecosystem: "Tab",
        connectors: &[Connector::MBus, Connector::Grove],
    },
    // ── Seeed SenseCAP Watcher ────────────────────────────────────────────────
    // ESP32-S3 + Himax WiseEye2 HX6538 (Cortex-M55 + Ethos-U55 microNPU):
    // on-device detection with LLM escalation (SenseCraft). 1.45" round touch
    // LCD, camera, mic, speaker, Grove port. USB id UNVERIFIED — see §4.
    BoardInfo {
        vid: 0x0000, // ⚠ VID/PID UNVERIFIED — replace before merge (see §4)
        pid: 0x0000,
        name: "sensecap-watcher",
        architecture: Some(
            "ESP32-S3 @ 240 MHz + Himax WiseEye2 HX6538 (Cortex-M55 + Ethos-U55), 1.45\" round touch LCD, camera",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "wifi", "ble", "nn_accel", "camera_capture", "audio_sample",
            "audio_output", "display", "touch", "battery", "microsd",
        ],
        vendor: "Seeed Studio",
        ecosystem: "SenseCAP",
        connectors: &[Connector::Grove],
    },
    // ── M5Stack Cardputer (v1.1, StampS3) ─────────────────────────────────────
    // Pocket terminal: 56-key keyboard, 1.14" ST7789 135x240, SPM1423 mic,
    // NS4168 speaker, IR transmitter, microSD, 120+1400 mAh battery, Grove.
    // StampS3 native USB = 0x303a:0x1001 (shared; selected by name).
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "m5stack-cardputer",
        architecture: Some(
            "M5StampS3 (ESP32-S3) @ 240 MHz, 56-key keyboard, 1.14\" ST7789, mic + speaker, IR TX (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "display", "keyboard", "audio_sample",
            "audio_output", "infrared", "microsd", "battery",
        ],
        vendor: "M5Stack",
        ecosystem: "Cardputer",
        connectors: &[Connector::Grove],
    },
    // ── LILYGO T-Lora Pager ───────────────────────────────────────────────────
    // Newest LILYGO Meshtastic handheld: ESP32-S3 + SX1262 LoRa, keyboard +
    // rotary encoder, GPS, NFC (ST25/PN532-class), BHI260 IMU, audio, microSD.
    // Native USB assumed 0x303a:0x1001 (shared) — VERIFY; exact display size,
    // GPS module, and NFC chip need confirmation from lilygo.cc (§4).
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "lilygo-t-lora-pager",
        architecture: Some(
            "ESP32-S3 @ 240 MHz + SX1262 LoRa, keyboard + rotary encoder, GPS, NFC, IMU (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "lora", "mesh", "gps", "nfc", "imu",
            "display", "keyboard", "audio_output", "battery", "microsd",
        ],
        vendor: "LILYGO",
        ecosystem: "T-Lora",
        connectors: &[Connector::Bare],
    },
    // ── Pimoroni Presto ───────────────────────────────────────────────────────
    // First Pimoroni entry. RP2350 + RM2 (CYW43439) Wi-Fi/BLE; 4" 480x480 IPS
    // touch, 7-zone RGB backlight, piezo, microSD, battery connector, 2x Qw/ST
    // (Qwiic/STEMMA QT-compatible) ports. Ships MicroPython (0x2e8a:0x0005,
    // shared); Pimoroni-specific PID UNVERIFIED — see §4.
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0005, // ⚠ generic MicroPython PID (shared) — see §4
        name: "pimoroni-presto",
        architecture: Some(
            "RP2350 dual Cortex-M33 @ 150 MHz + RM2 Wi-Fi/BLE, 4\" 480x480 IPS touch (MicroPython VID/PID, shared)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "display", "touch", "microsd",
            "battery", "audio_output",
        ],
        vendor: "Pimoroni",
        ecosystem: "Presto",
        connectors: &[Connector::Qwiic],
    },
    // ── Raspberry Pi Pico 2 W ─────────────────────────────────────────────────
    // RP2350 + CYW43439 Wi-Fi/BLE. SDK CDC PID for RP2350-W believed 0x0009
    // (raspberrypi/usb-pid allocation) — VERIFY before merge (§4). MicroPython
    // enumerates 0x2e8a:0x0005 (shared).
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0009, // ⚠ VERIFY against raspberrypi/usb-pid (see §4)
        name: "raspberry-pi-pico2-w",
        architecture: Some(
            "RP2350 dual-core ARM Cortex-M33 @ 150 MHz + CYW43439 (Wi-Fi 4, BLE 5.2)",
        ),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm", "wifi", "ble"],
        vendor: "Raspberry Pi",
        ecosystem: "Pico",
        connectors: &[Connector::Bare],
    },
    // ── Adafruit Feather RP2350 (HSTX) ────────────────────────────────────────
    // First true Feather-format host in the registry (FeatherWing + STEMMA QT).
    // 8 MB PSRAM; HSTX port drives DVI. CircuitPython PID under VID 0x239a
    // UNVERIFIED — see §4.
    BoardInfo {
        vid: 0x239a,
        pid: 0x0000, // ⚠ PID UNVERIFIED — replace before merge (see §4)
        name: "adafruit-feather-rp2350",
        architecture: Some(
            "RP2350 dual Cortex-M33 @ 150 MHz, 8 MB PSRAM, HSTX/DVI port, LiPo charge (Feather; STEMMA QT)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "analog_read", "i2c", "spi", "pwm", "display", "battery", "psram",
        ],
        vendor: "Adafruit",
        ecosystem: "Feather",
        connectors: &[Connector::FeatherWing, Connector::StemmaQt],
    },
    // ── Arduino Nano ESP32 ────────────────────────────────────────────────────
    // First wireless-era Arduino: ESP32-S3 in u-blox NORA-W106, Nano footprint.
    // PID 0x0070 under Arduino VID 0x2341 believed correct — VERIFY (§4).
    BoardInfo {
        vid: 0x2341,
        pid: 0x0070, // ⚠ VERIFY against Arduino boards.txt (see §4)
        name: "arduino-nano-esp32",
        architecture: Some(
            "ESP32-S3 (u-blox NORA-W106) Xtensa LX7 dual-core @ 240 MHz, Nano form factor",
        ),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble"],
        vendor: "Arduino",
        ecosystem: "Nano",
        connectors: &[Connector::Bare],
    },
    // ── Seeed XIAO ESP32-C5 ───────────────────────────────────────────────────
    // Thumb-size dual-band Wi-Fi 6 node (Jan 2026); merge only alongside the
    // esp32-c5 devkit entry. Native USB assumed 0x303a:0x1001 (shared) — VERIFY.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "xiao-esp32c5",
        architecture: Some(
            "ESP32-C5 RISC-V @ 240 MHz, dual-band 2.4/5 GHz Wi-Fi 6, BLE 5, 802.15.4 (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "analog_read", "i2c", "spi", "wifi", "ble", "thread", "zigbee", "battery",
        ],
        vendor: "Seeed Studio",
        ecosystem: "XIAO",
        connectors: &[Connector::Bare],
    },
```

### 2b. `AccessoryInfo` additions (append to `KNOWN_ACCESSORIES`)

```rust
    // ── Hardware-scout 2026-07-06: AI-accelerator add-ons ─────────────────────
    AccessoryInfo {
        name: "rpi-ai-hat-plus-2",
        description: "Raspberry Pi AI HAT+ 2 — Hailo-10H NPU, 40 TOPS INT4 / 20 TOPS INT8, GenAI-capable (RPi 5, PCIe)",
        bus: "pcie",
        default_i2c_addr: None,
        capabilities: &["hailo"],
        compatible_boards: &["raspberry-pi-5"],
        connector: Connector::HatPi,
    },
    AccessoryInfo {
        name: "rpi-ai-camera-imx500",
        description: "Raspberry Pi AI Camera — Sony IMX500 12.3MP sensor with on-sensor NN inference accelerator (CSI)",
        bus: "csi",
        default_i2c_addr: None,
        capabilities: &["npu", "camera_capture"],
        compatible_boards: &["raspberry-pi-4", "raspberry-pi-5"],
        connector: Connector::Bare,
    },
```

---

## 3. Proposed NEW capability tokens

| Token | Definition | First used by |
|---|---|---|
| `vpu` | Vision processing unit — Intel Movidius Myriad-class accelerator (Luxonis OAK, depth + NN inference) | OAK-D Lite (#1) |

`vpu` is already reserved in `docs/V2-HARDWARE-ECOSYSTEM.md` §2.1 and is the
**last accelerator class in the doc with zero registry coverage**. Add to
`VALID_CAPABILITIES`, the doc-comment table, and a `boards_with_capability("vpu")`
test per the definition-of-done.

No new connector proposed this week. Note: Pimoroni's "Qw/ST" port is
electrically Qwiic/STEMMA QT — modeled as `Connector::Qwiic`, no enum change
needed. The `bus: "csi"` string on the AI Camera accessory is new as a *bus*
value (like existing `pcie`/`sccb`); no code change required since `bus` is a
free-form `&str`.

---

## 4. Needs verification

Placeholder `0x0000` entries must not merge until resolved.

| Item | What's unconfirmed | Next step |
|---|---|---|
| **OAK-D Lite** | ✅ VERIFIED `03e7:2485` (Intel Movidius MyriadX, unbooted) | Luxonis USB deployment guide confirms; note device re-enumerates after DepthAI firmware boot — consider whether to also key the booted id |
| **reCamera 2002w** | USB-C gadget VID/PID | Check reCamera OS gadget config (Seeed wiki / OSHW-reCamera-Series repo) |
| **BeagleY-AI** | Flash/recovery USB id | TI AM67A DFU mode id from BeagleBoard docs; runtime is native SBC |
| **M5Stack Tab5** | Native-USB id assumed `303a:1001`; battery-SKU differences | Confirm from m5-docs Tab5 page / lsusb on hardware |
| **SenseCAP Watcher** | USB id (native S3 vs bridge chip); Grove port presence | Seeed wiki Watcher hardware page |
| **Cardputer** | Which SKU (v1.1 vs Cardputer-Adv, Sep 2025); Adv specs differ | m5-docs; consider separate `cardputer-adv` entry if IMU/display upgraded |
| **T-Lora Pager** | USB id; display size/controller; GPS + NFC chip models | lilygo.cc product page + schematic |
| **Pimoroni Presto** | Pimoroni-specific PID under `2e8a` (currently generic MicroPython `0005`) | Pimoroni GitHub (presto firmware USB descriptor) |
| **Pico 2 W** | SDK CDC PID `0x0009` for RP2350-W | raspberrypi/usb-pid allocation table on GitHub |
| **Feather RP2350** | CircuitPython PID under `239a` | CircuitPython `creation_ids` / board def |
| **Nano ESP32** | PID `0x0070` under `2341` | Arduino esp32 core boards.txt |
| **XIAO ESP32-C5** | Native USB id assumed `303a:1001` | Seeed wiki XIAO ESP32-C5 page |
| *(carried over)* **Adafruit Feather ESP32-S3** | PID under `239a` still unresolved from 2026-06-29 | CircuitPython board def (`adafruit_feather_esp32s3_*`) |
| *(carried over)* **Sipeed MaixCAM (K230)** | USB id still unresolved; Kendryte EOL | CanMV-K230 USB descriptor; decide whether EOL status demotes it |

---

## 5. Skipped (out of scope / no new capability)

- **NVIDIA Jetson AGX Thor dev kit** ($3,499, Blackwell) — taxonomy already covered by `cuda`+`tensor_rt`; price puts it outside the hobbyist fleet. Revisit if a user owns one.
- **M5Stack StackChan / AI Pyramid / StickS3 / CardputerZero** (Jan–May 2026) — StackChan & Pyramid are ESP32-S3 devices with no new capability; CardputerZero is a Linux handheld worth a look **next run** once specs firm up.
- **ESP32-C5-WIFI6-KIT** (third-party, Apr 2026) — clone-class; the DevKitC-1 entry covers the SoC.
- **ESP32-C61** — cost-reduced C6 derivative; no devkit traction yet.
- **FeatherS3[D] (Unexpected Maker)** — Feather-format ESP32-S3; adds no capability beyond planned Adafruit entries.
- **T-Echo Lite** — nRF52840+LoRa already represented by RAK4631; add later for popularity if Meshtastic users request it.
- **XIAO Vision AI Camera** — packaging of Grove Vision AI V2 (already merged) + XIAO ESP32-C3; no new capability.

---

*Sources: espressif.com, developer.espressif.com, cnx-software.com, seeedstudio.com (+wiki, blog), shop.m5stack.com, docs.m5stack.com, lilygo.cc, meshtastic.org, adafruit.com, learn.adafruit.com, shop.pimoroni.com, raspberrypi.com, forums.raspberrypi.com, github.com/raspberrypi/usb-pid, shop.luxonis.com, docs.luxonis.com, nvidia.com, geniatech.com, hackster.io, beagleboard.org (via cnx-software).*

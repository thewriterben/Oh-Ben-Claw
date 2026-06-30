# Hardware Scout Report — 2026-06-29

*Automated `obc-hardware-scout` run. Proposals only — `registry.rs` is NOT edited.*
*Scope: ESP32 family, MCUs/SBCs, AI accelerators, sensors, displays, radios, connector ecosystems.*

These are reviewed proposals for the maintainer to merge. Every entry that uses a
**new capability token** is flagged; new tokens are defined in §3. VID/PID values
that could not be confirmed against a primary/vendor source are listed in §4.

This is the first scout report, so the shortlist deliberately closes the largest
standing gaps in the registry: **AI-accelerator tokens** (`npu`, `edge_tpu`,
`hailo`, `nn_accel`, `kpu`, `tensor_rt`), **new vendor ecosystems** (Adafruit,
SparkFun, DFRobot, Radxa, Google Coral), and **new ESP32 SoCs** (C6, H2, P4).

---

## 1. Ranked shortlist (top additions)

Legend — Transport: `serial` / `native` / `probe` / `bridge`. ⚠ = uses a NEW
capability token (see §3). 🔎 = VID/PID or a spec needs verification (see §4).

| # | Product | Vendor | MCU/SoC + arch | VID:PID | Transport | Connectors | Capabilities | Rationale |
|---|---------|--------|----------------|---------|-----------|------------|--------------|-----------|
| 1 | **AI HAT+ (13 TOPS)** | Raspberry Pi | Hailo-8L NPU, 13 TOPS, PCIe Gen3 | n/a (PCIe accessory) | accessory→RPi5 | HatPi | `hailo` ⚠ | First Hailo accelerator; hugely popular RPi 5 add-on; unlocks edge-inference nodes |
| 2 | **Coral USB Accelerator** | Google | Edge TPU ASIC, 4 TOPS, USB 3 | `18d1:9302` (post-init); `1a6e:089a` pre-init | serial/native (USB) | Bare | `edge_tpu` ⚠ | First Edge TPU; **VID/PID verified**; plug-and-play on any host node |
| 3 | **ROCK 5B** | Radxa / Rockchip | RK3588 octa-core A76+A55, Mali-G610, 6 TOPS NPU (AArch64) | maskrom `2207:350a` 🔎 | native | HatPi | `npu` ⚠, gpio, i2c, spi, pwm, ethernet ⚠ | First Radxa/Rockchip-NPU board; flagship hobbyist AI SBC |
| 4 | **Jetson Orin Nano Super Dev Kit** | NVIDIA | 6-core A78AE + 1024-core Ampere + tensor cores, 67 TOPS | `0955:7020` (shares Jetson Nano ID) 🔎 | native | Bare | `cuda`, `tensor_rt` ⚠, gpio, i2c, spi, pwm, camera_capture | Modern Jetson; TensorRT class; select by `name` (VID/PID collides w/ jetson-nano) |
| 5 | **Module LLM (AX630C)** | M5Stack | AXera AX630C dual A53 @1.2 GHz + 3.2 TOPS NPU, 4 GB LPDDR4, 32 GB eMMC | CH340N `1a86:7523` (shared) | bridge | MBus | `npu` ⚠, ethernet ⚠, audio_sample, audio_output | On-device LLM/ASR/TTS module; new M5 M-Bus accelerator; offline voice node |
| 6 | **Grove Vision AI Module V2** | Seeed Studio | Himax WiseEye2 HX6538, dual Cortex-M55 + Ethos-U55 microNPU | (USB-C/Grove) 🔎 | accessory (i2c/Grove) | Grove | `nn_accel` ⚠, camera_capture, audio_sample | $16 Grove-native smart-camera; first Ethos-U/Helium device; very popular |
| 7 | **ESP32-C6-DevKitC-1** | Espressif | ESP32-C6 RISC-V @160 MHz, Wi-Fi 6 + BLE 5 + 802.15.4 | `303a:1001` (native USB-Serial/JTAG) | serial | Bare | gpio, i2c, spi, wifi, ble, thread ⚠, zigbee ⚠ | New SoC; Wi-Fi 6 + Thread/Zigbee/Matter; first 802.15.4 ESP32 |
| 8 | **ESP32-P4-Function-EV-Board** | Espressif | ESP32-P4 dual RISC-V @400 MHz, MIPI-CSI/DSI, no radio | `303a:1001` (native USB-Serial/JTAG) | serial | Bare | gpio, i2c, spi, camera_capture, display, nn_accel ⚠ | New high-perf SoC; HP MCU w/ AI vector ops + camera/display pipeline |
| 9 | **QT Py ESP32-S3** | Adafruit | ESP32-S3 LX7 dual @240 MHz | `239a:8143` (CircuitPython) 🔎 / `303a:1001` (Arduino) | serial | StemmaQt | gpio, analog_read, i2c, spi, wifi, ble | First Adafruit board + STEMMA QT port; tiny popular dev board |
| 10 | **Feather ESP32-S3** | Adafruit | ESP32-S3 LX7 dual @240 MHz, LiPo charge | `239a:` PID 🔎 / `303a:1001` (Arduino) | serial | FeatherWing, StemmaQt | gpio, analog_read, i2c, spi, wifi, ble, battery ⚠ | Anchors the Feather/FeatherWing ecosystem; STEMMA QT |
| 11 | **Thing Plus ESP32-C6** | SparkFun | ESP32-C6 RISC-V, Wi-Fi 6 + 802.15.4 | CH340 `1a86:7523` (shared) | serial | Qwiic, FeatherWing | gpio, i2c, spi, wifi, ble, thread ⚠, microsd | First SparkFun board + Qwiic port; pairs C6 radios with Qwiic catalog |
| 12 | **T-Deck** | LILYGO | ESP32-S3 + SX1262 LoRa, 2.8" touch, keyboard, trackball | `303a:1001` (native) | serial | Bare | gpio, i2c, spi, wifi, ble, lora, display, touch, audio_sample, audio_output | Popular Meshtastic handheld; LoRa + full I/O in one node |
| 13 | **T-Display-S3** | LILYGO | ESP32-S3 LX7 dual @240 MHz, 1.9" ST7789 LCD | `303a:1001` (native) | serial | Bare | gpio, i2c, spi, wifi, ble, display | Extremely common ESP32-S3 display board |
| 14 | **FireBeetle 2 ESP32-S3** | DFRobot | ESP32-S3 LX7 dual @240 MHz, LiPo charge | `303a:1001` (native) | serial | Bare (Gravity 🔎) | gpio, analog_read, i2c, spi, wifi, ble, battery ⚠ | First DFRobot board; opens the Gravity sensor ecosystem (see §3 connector note) |
| 15 | **MaixCAM (K230)** | Sipeed | Kendryte K230 dual RISC-V C908 + KPU (~13× K210) | (USB-C) 🔎 | serial/native | Bare | `kpu` ⚠, camera_capture, display | Only `kpu` coverage; widely owned. ⚠ Kendryte line EOL June 2025 |
| 16 | **ESP32-H2-DevKitM-1** | Espressif | ESP32-H2 RISC-V @96 MHz, BLE 5 + 802.15.4 (no Wi-Fi) | `303a:1001` (native USB-Serial/JTAG) | serial | Bare | gpio, i2c, spi, ble, thread ⚠, zigbee ⚠ | New SoC; dedicated Thread/Zigbee/Matter radio node |
| 17 | **Coral Dev Board Mini** | Google | MediaTek 8167S quad A35 + Edge TPU, 4 TOPS (AArch64) | recovery `0525:a4a7` 🔎 | native | HatPi | `edge_tpu` ⚠, gpio, i2c, spi, wifi, ble | Standalone Edge-TPU SBC form of #2; Pi-compatible GPIO |

---

## 2. Ready-to-paste Rust entries

> ⚠ Entries marked with a new capability token will **not compile against the
> current taxonomy doc-comment** until the tokens in §3 are added. The capability
> strings themselves are plain `&str` so they compile, but merge should add the
> token to the doc-comment table (and a registry test) per the V2 ecosystem doc
> "definition of done."

### 2a. `BoardInfo` additions (append to `KNOWN_BOARDS`)

```rust
    // ── Google Coral USB Accelerator (Edge TPU) ───────────────────────────────
    // Enumerates as 1a6e:089a (Global Unichip, pre-firmware) then re-enumerates
    // as 18d1:9302 (Google) once the Edge TPU runtime loads firmware. We key on
    // the post-init Google ID; the maintainer may add a second entry for the
    // pre-init ID if auto-ID before first inference is needed. VID/PID VERIFIED.
    BoardInfo {
        vid: 0x18d1,
        pid: 0x9302,
        name: "coral-usb-accelerator",
        architecture: Some("Google Edge TPU ASIC, 4 TOPS @ 2 W (USB 3.0 coprocessor)"),
        transport: "serial",
        capabilities: &["edge_tpu"],
        vendor: "Google",
        ecosystem: "Coral",
        connectors: &[Connector::Bare],
    },
    // ── Radxa ROCK 5B (Rockchip RK3588, 6 TOPS NPU) ───────────────────────────
    // Native Linux SBC. No runtime USB-device ID; maskrom/flash mode enumerates
    // under Rockchip VID 0x2207 (PID 0x350a) — recorded for flashing only.
    BoardInfo {
        vid: 0x2207,
        pid: 0x350a,
        name: "radxa-rock-5b",
        architecture: Some(
            "Rockchip RK3588 octa-core (4x Cortex-A76 + 4x Cortex-A55), Mali-G610, 6 TOPS NPU (AArch64)",
        ),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "npu", "ethernet"],
        vendor: "Radxa",
        ecosystem: "ROCK",
        connectors: &[Connector::HatPi],
    },
    // ── NVIDIA Jetson Orin Nano Super Developer Kit ───────────────────────────
    // 67 TOPS (Super software boost). NOTE: USB recovery/serial enumerates as
    // 0955:7020 — the SAME id as the existing jetson-nano entry, so VID/PID can't
    // uniquely distinguish them. Selected by `name` in deployment config;
    // lookup_board returns the first VID/PID match.
    BoardInfo {
        vid: 0x0955,
        pid: 0x7020,
        name: "jetson-orin-nano",
        architecture: Some(
            "NVIDIA Jetson Orin Nano: 6-core Arm Cortex-A78AE + 1024-core Ampere GPU w/ tensor cores, 67 TOPS (AArch64)",
        ),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture", "cuda", "tensor_rt"],
        vendor: "NVIDIA",
        ecosystem: "Jetson",
        connectors: &[Connector::Bare],
    },
    // ── M5Stack Module LLM (AXera AX630C, 3.2 TOPS NPU) ───────────────────────
    // Stacks via M-Bus; AX630C runs Linux and does on-device KWS/ASR/LLM/TTS.
    // Built-in CH340N USB-serial for debug (1a86:7523, shared) + RJ45 100M.
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "m5stack-module-llm",
        architecture: Some(
            "AXera AX630C dual Cortex-A53 @ 1.2 GHz + 3.2 TOPS NPU, 4 GB LPDDR4, 32 GB eMMC (CH340N; shared VID/PID)",
        ),
        transport: "bridge",
        capabilities: &["npu", "ethernet", "audio_sample", "audio_output"],
        vendor: "M5Stack",
        ecosystem: "Module",
        connectors: &[Connector::MBus],
    },
    // ── Espressif ESP32-C6-DevKitC-1 ──────────────────────────────────────────
    // Native USB-Serial/JTAG = 303a:1001 (shared across C3/S3/C6/H2/P4).
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-c6",
        architecture: Some("ESP32-C6 RISC-V single-core @ 160 MHz (Wi-Fi 6, BLE 5, 802.15.4; native USB)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble", "thread", "zigbee"],
        vendor: "Espressif",
        ecosystem: "ESP32-C6",
        connectors: &[Connector::Bare],
    },
    // ── Espressif ESP32-P4-Function-EV-Board ──────────────────────────────────
    // High-performance MCU, no built-in radio; MIPI-CSI camera + MIPI-DSI display.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-p4",
        architecture: Some("ESP32-P4 dual-core RISC-V @ 400 MHz (AI vector ext., MIPI-CSI/DSI; native USB)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "camera_capture", "display", "nn_accel"],
        vendor: "Espressif",
        ecosystem: "ESP32-P4",
        connectors: &[Connector::Bare],
    },
    // ── Espressif ESP32-H2-DevKitM-1 ──────────────────────────────────────────
    // 802.15.4 + BLE only (no Wi-Fi); Thread/Zigbee/Matter radio node.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-h2",
        architecture: Some("ESP32-H2 RISC-V single-core @ 96 MHz (BLE 5, 802.15.4, no Wi-Fi; native USB)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "ble", "thread", "zigbee"],
        vendor: "Espressif",
        ecosystem: "ESP32-H2",
        connectors: &[Connector::Bare],
    },
    // ── Adafruit QT Py ESP32-S3 ───────────────────────────────────────────────
    // CircuitPython enumerates as 239a:8143 (bootloader 239a:0143); Arduino/ESP-IDF
    // build uses native 303a:1001. PID 0x8143 from CircuitPython board def — verify.
    BoardInfo {
        vid: 0x239a,
        pid: 0x8143,
        name: "adafruit-qtpy-esp32s3",
        architecture: Some("ESP32-S3 Xtensa LX7 dual-core @ 240 MHz (CircuitPython VID/PID; STEMMA QT)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble"],
        vendor: "Adafruit",
        ecosystem: "QT Py",
        connectors: &[Connector::StemmaQt],
    },
    // ── Adafruit Feather ESP32-S3 ─────────────────────────────────────────────
    // VID 0x239a (Adafruit) under CircuitPython; PID UNVERIFIED — see §4.
    // Arduino/ESP-IDF build uses native 303a:1001.
    BoardInfo {
        vid: 0x239a,
        pid: 0x0000, // ⚠ PID UNVERIFIED — replace before merge (see §4)
        name: "adafruit-feather-esp32s3",
        architecture: Some("ESP32-S3 Xtensa LX7 dual-core @ 240 MHz, LiPo charger (Feather; STEMMA QT)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble", "battery"],
        vendor: "Adafruit",
        ecosystem: "Feather",
        connectors: &[Connector::FeatherWing, Connector::StemmaQt],
    },
    // ── SparkFun Thing Plus ESP32-C6 ──────────────────────────────────────────
    // USB-C variant uses CH340 (1a86:7523, shared). Qwiic + Feather-format header.
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "sparkfun-thing-plus-esp32-c6",
        architecture: Some("ESP32-C6 RISC-V @ 160 MHz (Wi-Fi 6, BLE 5, 802.15.4; CH340, shared VID/PID)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble", "thread", "microsd"],
        vendor: "SparkFun",
        ecosystem: "Thing Plus",
        connectors: &[Connector::Qwiic, Connector::FeatherWing],
    },
    // ── LILYGO T-Deck ─────────────────────────────────────────────────────────
    // ESP32-S3 + SX1262 LoRa, 2.8" touch LCD, keyboard, trackball, mic, speaker.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "lilygo-t-deck",
        architecture: Some(
            "ESP32-S3 LX7 dual @ 240 MHz + SX1262 LoRa, 2.8\" IPS touch, keyboard/trackball (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "lora", "display", "touch", "audio_sample", "audio_output",
        ],
        vendor: "LILYGO",
        ecosystem: "T-Deck",
        connectors: &[Connector::Bare],
    },
    // ── LILYGO T-Display-S3 ───────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "lilygo-t-display-s3",
        architecture: Some(
            "ESP32-S3 LX7 dual @ 240 MHz, 1.9\" ST7789 320x170 LCD, 16 MB flash / 8 MB PSRAM (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble", "display"],
        vendor: "LILYGO",
        ecosystem: "T-Display",
        connectors: &[Connector::Bare],
    },
    // ── DFRobot FireBeetle 2 ESP32-S3 ─────────────────────────────────────────
    // Opens the DFRobot Gravity ecosystem. Gravity is a non-standard connector
    // (not yet in the Connector enum) — modeled as Bare for now; see §3.
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "dfrobot-firebeetle2-esp32s3",
        architecture: Some(
            "ESP32-S3 LX7 dual @ 240 MHz, LiPo charge, onboard GDI (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "wifi", "ble", "battery"],
        vendor: "DFRobot",
        ecosystem: "FireBeetle",
        connectors: &[Connector::Bare],
    },
    // ── Sipeed MaixCAM (Kendryte K230, KPU) ───────────────────────────────────
    // Only board giving `kpu` coverage. NOTE: Kendryte line EOL June 2025; keep
    // for the large installed base. USB-C device ID UNVERIFIED — see §4.
    BoardInfo {
        vid: 0x0000, // ⚠ VID/PID UNVERIFIED — see §4
        pid: 0x0000,
        name: "sipeed-maixcam",
        architecture: Some("Kendryte K230 dual RISC-V C908 + KPU (~13x K210), RVV 1.0"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "kpu", "camera_capture", "display"],
        vendor: "Sipeed",
        ecosystem: "MaixCAM",
        connectors: &[Connector::Bare],
    },
    // ── Google Coral Dev Board Mini (Edge TPU SBC) ────────────────────────────
    // Standalone SBC form of the USB accelerator. Native Linux (Mendel). Recovery
    // (fastboot) enumerates under 0525:a4a7 — UNVERIFIED, see §4.
    BoardInfo {
        vid: 0x0525,
        pid: 0xa4a7,
        name: "coral-dev-board-mini",
        architecture: Some(
            "MediaTek MT8167S quad Cortex-A35 + Google Edge TPU, 4 TOPS, 2 GB LPDDR3 (AArch64)",
        ),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble", "edge_tpu", "camera_capture"],
        vendor: "Google",
        ecosystem: "Coral",
        connectors: &[Connector::HatPi],
    },
```

### 2b. `AccessoryInfo` additions (append to `KNOWN_ACCESSORIES`)

```rust
    // ── AI accelerators (host add-ons) ────────────────────────────────────────
    AccessoryInfo {
        name: "rpi-ai-hat-plus-13t",
        description: "Raspberry Pi AI HAT+ (13 TOPS) — Hailo-8L NPU over PCIe Gen3 (RPi 5)",
        bus: "pcie",
        default_i2c_addr: None,
        capabilities: &["hailo"],
        compatible_boards: &["raspberry-pi-5"],
        connector: Connector::HatPi,
    },
    AccessoryInfo {
        name: "grove-vision-ai-v2",
        description: "Seeed Grove Vision AI Module V2 — Himax WiseEye2 (Cortex-M55 + Ethos-U55 microNPU) smart camera",
        bus: "i2c",
        default_i2c_addr: Some(0x62),
        capabilities: &["nn_accel", "camera_capture"],
        compatible_boards: &[],
        connector: Connector::Grove,
    },
    // ── High-value Qwiic / STEMMA QT plug-in sensors (broaden connector matching) ──
    AccessoryInfo {
        name: "scd41",
        description: "Sensirion SCD41 — true CO2 (NDIR), temperature, humidity",
        bus: "i2c",
        default_i2c_addr: Some(0x62),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Qwiic,
    },
    AccessoryInfo {
        name: "vl53l1x",
        description: "ST VL53L1X — time-of-flight distance sensor (up to 4 m)",
        bus: "i2c",
        default_i2c_addr: Some(0x29),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::StemmaQt,
    },
    AccessoryInfo {
        name: "bno055",
        description: "Bosch BNO055 — 9-DOF IMU with on-chip sensor fusion (absolute orientation)",
        bus: "i2c",
        default_i2c_addr: Some(0x28),
        capabilities: &["imu", "sensor_read"],
        compatible_boards: &[],
        connector: Connector::StemmaQt,
    },
    AccessoryInfo {
        name: "sgp40",
        description: "Sensirion SGP40 — VOC air-quality gas sensor",
        bus: "i2c",
        default_i2c_addr: Some(0x59),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Qwiic,
    },
```

---

## 3. Proposed NEW capability tokens

All of these are pre-described in `docs/V2-HARDWARE-ECOSYSTEM.md` §2.1 but are
**not yet in the `registry.rs` doc-comment taxonomy table**. Adding them is the
gating dependency for the AI-accelerator and radio entries above.

| Token | Definition | First used by |
|---|---|---|
| `npu` | Generic on-SoC neural processing unit (e.g. RK3588, AXera AX630C) | ROCK 5B, Module LLM |
| `edge_tpu` | Google Coral Edge TPU accelerator | Coral USB Accelerator, Coral Dev Board Mini |
| `hailo` | Hailo-8 / 8L / 10 accelerator (PCIe/M.2/USB) | RPi AI HAT+ |
| `nn_accel` | MCU-class NN acceleration (Arm Ethos-U / Helium, ESP32 vector ops) | Grove Vision AI V2, ESP32-P4 |
| `kpu` | Kendryte / Sipeed KPU (K210 / K230) | MaixCAM |
| `tensor_rt` | NVIDIA TensorRT-capable accelerator (Orin/Thor); complements `cuda` | Jetson Orin Nano |
| `ethernet` | Wired networking interface | ROCK 5B, Module LLM |
| `thread` | 802.15.4 Thread stack | ESP32-C6/H2, SparkFun Thing Plus C6 |
| `zigbee` | 802.15.4 Zigbee stack | ESP32-C6/H2 |
| `battery` | On-board LiPo/Li-ion charging + power management | Adafruit Feather, FireBeetle 2 |

**Proposed NEW connector (lower priority):** `Gravity` (DFRobot) — DFRobot's
4-pin I2C/UART/analog ecosystem, analogous to Grove. Not added in the entries
above (FireBeetle modeled as `Bare`); flagged for the maintainer to decide
whether to extend the `Connector` enum or treat Gravity I2C modules as
Grove-equivalent.

> Per the ecosystem doc, actuator/physical tokens auto-tag a `RiskClass`. None of
> this batch adds a new actuator token, but `ethernet`/`npu` SBC nodes that run
> inference are **edge-inference nodes** (System 1 tier) — file a ROADMAP item if
> firmware/EdgeAgent wiring is needed for Coral/Hailo/Jetson/K230.

---

## 4. Needs verification

Confirm against a primary/vendor source before merge. Boards with a placeholder
`0x0000` PID must not be merged until resolved.

| Item | What's unconfirmed | Notes / next step |
|---|---|---|
| **Coral USB Accelerator** | ✅ VERIFIED `1a6e:089a` → `18d1:9302` | Confirmed via coral.ai docs + edgetpu issue #536. Decide whether to also register the pre-init `1a6e:089a` id |
| **Adafruit Feather ESP32-S3** | PID under VID `0x239a` | Pull from CircuitPython `board` definition / Arduino `boards.txt`; entry currently has `0x0000` placeholder |
| **Adafruit QT Py ESP32-S3** | PID `0x8143` (CircuitPython) | Likely correct from CP board def; confirm bootloader `0x0143`. Arduino-mode build is `303a:1001` |
| **Sipeed MaixCAM (K230)** | USB-C device VID/PID | CanMV-K230/MaixCAM USB descriptor; entry has `0x0000` placeholder. Kendryte EOL June 2025 |
| **Coral Dev Board Mini** | Recovery/fastboot `0525:a4a7` | Confirm against Coral flashing docs; runtime is a native SBC (no fixed USB-device ID) |
| **Radxa ROCK 5B** | Maskrom `2207:350a` | Rockchip maskrom ID for flashing only; native SBC otherwise. Confirm exact PID |
| **Jetson Orin Nano** | `0955:7020` collides w/ jetson-nano | Verified as the Jetson USB-serial ID, but NOT unique — must be selected by `name` |
| **ESP32-C6 / H2 / P4 devkits** | `303a:1001` shared | Verified these enumerate as the Espressif USB-Serial/JTAG unit; not unique across ESP32 native-USB parts (existing registry convention: select by `name`) |
| **M5Stack Module LLM** | CH340N `1a86:7523` shared | Verified CH340N onboard; VID/PID not unique. Confirm AX630C NPU TOPS (3.2) from M5 docs |
| **Grove Vision AI V2** | I2C address `0x62` | Default SSCMA/Grove I2C address — confirm; note it collides with SCD41 `0x62` on a shared bus |

---

## 5. Skipped (out of scope / no new capability)

- Generic ESP32-WROOM clones, no-name CYD variants → duplicate of existing `esp32` / `cyd-esp32-2432s028r`.
- ESP32-S2 dev boards → older SoC, no radio advantage over covered parts; low marginal value (can add later for completeness).
- Hailo-8 **26 TOPS** AI HAT+ → same `hailo` token as the 13 TOPS entry; add as a second AccessoryInfo only if the maintainer wants the TOPS distinction.
- Bare USB-UART bridges (new CH/CP variants) → already covered by the bridge entries.

---

*Sources: coral.ai, raspberrypi.com, espressif.com / docs.espressif.com, seeedstudio.com, m5stack.com, lilygo.cc, adafruit.com, sparkfun.com, dfrobot.com, radxa.com, nvidia.com, sipeed/kendryte docs, cnx-software.com. Full URLs in the chat summary for this run.*

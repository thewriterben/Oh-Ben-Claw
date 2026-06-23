# Oh-Ben-Claw v2.0 — Hardware Ecosystem Expansion

*Companion to `docs/V2-STRATEGY.md` and `docs/V2-IMPLEMENTATION.md`. Compiled June 23, 2026.*

The embodied moat is only as wide as the hardware Oh-Ben-Claw can talk to. v2.0 makes **maximal hardware breadth** a first-class, continuously-maintained goal: support as many ESP32 boards, ESP32-based electronics, sensors, microcontrollers, SBCs, AI accelerators, displays, radios, actuators, accessories, and connector ecosystems as possible — and keep a standing, automated process that seeks and adds the latest hardware every week.

This sits alongside the five v2.0 capability phases as a permanent **Hardware Ecosystem** track (see `ROADMAP.md`).

---

## 1. Where we are today

The registry (`src/peripherals/registry.rs`) currently models hardware with two tables:

- **`BoardInfo`** — `{ vid, pid, name, architecture, transport, capabilities }`, keyed by USB VID/PID for auto-identification on plug-in.
- **`AccessoryInfo`** — known I²C/SPI/GPIO sensors and modules (BME280, MPU6050, SSD1306, DHT22, …), looked up by name / I²C address / capability.

Coverage today (~38 boards, ~16 accessories) is strong on STM32 Nucleo, Arduino, generic ESP32/ESP32-S3/-C3, Raspberry Pi (Pico/4/5), NanoPi, Teensy, BeagleBone, Jetson Nano, plus Waveshare, Seeed XIAO, and Sipeed. Capability tokens: `gpio, analog_read, analog_write, i2c, spi, pwm, camera_capture, audio_sample, sensor_read, rtt, flash, ble, wifi, can, dac, cuda, display, touch`.

**The gaps v2.0 closes:**

- **Whole vendor ecosystems missing:** no M5Stack, LILYGO, Adafruit, or SparkFun entries; thin on Pimoroni, DFRobot, Radxa, Espressif's newest SoCs.
- **AI accelerators undermodeled:** only `cuda`. No Hailo, Google Coral Edge TPU, Rockchip NPU (RK3588), Kendryte/Sipeed K210/K230, or ESP32-S3 vector-NN distinction.
- **No connector-ecosystem concept:** Grove, Qwiic, STEMMA QT, and M-Bus are how these vendors make hardware composable — but the registry can't express "this board exposes a Qwiic port, so any Qwiic accessory plugs in."
- **No radios/protocol taxonomy:** LoRa/LoRaWAN, Zigbee, Thread, Matter, sub-GHz, NFC, GPS are common on these boards and unmodeled.
- **No standing intake process:** hardware was added in bursts during audits; nothing keeps it current.

---

## 2. Registry model upgrades

### 2.1 New capability tokens

Add to the capability taxonomy (doc comment + usage). Grouped:

**AI acceleration** — *the highest-value additions:*
| Token | Meaning |
|---|---|
| `npu` | Generic neural processing unit (on-SoC, e.g. RK3588, ESP32-P4) |
| `edge_tpu` | Google Coral Edge TPU |
| `hailo` | Hailo-8/8L/10 accelerator |
| `vpu` | Vision processing unit (e.g. Movidius/OAK) |
| `kpu` | Kendryte/Sipeed KPU (K210/K230) |
| `tensor_rt` | NVIDIA TensorRT-capable (Jetson Orin/Thor); complements `cuda` |
| `nn_accel` | MCU-class NN acceleration (ESP32-S3 vector ops, Arm Helium/Ethos-U) |

**Radios / connectivity:**
| Token | Meaning |
|---|---|
| `lora` / `lorawan` | LoRa PHY / LoRaWAN stack |
| `zigbee` / `thread` / `matter` | 802.15.4 stacks + Matter |
| `subghz` | Sub-GHz ISM radio |
| `nfc` | NFC reader/tag |
| `gps` | GNSS receiver |
| `ethernet` | Wired networking |
| `cellular` | LTE/NB-IoT/4G modem |

**I/O / form:**
| Token | Meaning |
|---|---|
| `epaper` | E-paper/e-ink display |
| `rgb_led` / `neopixel` | Addressable LED |
| `microsd` | SD storage |
| `rtc` | Battery-backed real-time clock |
| `battery` / `pmic` | On-board power management/charging |
| `motor_driver` | H-bridge / stepper / servo driver |
| `relay` | Relay output (physical, high blast radius — see Track 0) |
| `imu` | Inertial measurement (accel+gyro[+mag]) |
| `microphone` / `speaker` | Audio in / out (distinct from generic `audio_sample`) |

> Actuator-class tokens (`relay`, `motor_driver`, GPIO-as-output) auto-tag the board's tools with a physical `RiskClass` (see `docs/V2-IMPLEMENTATION.md`, Track 0), so safety limits apply by default.

### 2.2 New: connector ecosystem field

The composability story for Seeed/M5Stack/Adafruit/SparkFun is the *connector*. Add a connector enum to both boards and accessories so the advisor can match them:

```rust
// src/peripherals/registry.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Connector {
    Grove,        // Seeed / M5Stack 4-pin
    Qwiic,        // SparkFun I²C
    StemmaQt,     // Adafruit I²C (Qwiic-compatible)
    Stemma,       // Adafruit 3-pin JST
    MBus,         // M5Stack stacking bus
    FeatherWing,  // Adafruit Feather header
    Pmod,         // Digilent
    HatPi,        // Raspberry Pi HAT (40-pin)
    Bare,         // header pins / solder
}

pub struct BoardInfo {
    // … existing fields …
    pub connectors: &'static [Connector],   // ports this board exposes
    pub vendor: &'static str,               // "Seeed", "M5Stack", "LILYGO", "Adafruit", "SparkFun", …
    pub ecosystem: &'static str,            // "XIAO", "Feather", "Qwiic", "M5", "T-Series", …
}

pub struct AccessoryInfo {
    // … existing fields …
    pub connector: Connector,               // how it attaches
}
```

Qwiic and STEMMA QT are electrically I²C-compatible, so a `Qwiic` accessory is usable on a `StemmaQt` board (and vice versa) — encode that equivalence in the advisor's matching, not as duplicate entries.

### 2.3 Advisor & deployment planner reuse

The existing `HardwareAdvisor` and `DeploymentPlanner` (`src/deployment/`) gain connector-aware matching: given a board's `connectors` and a desired `FeatureDesire`, suggest compatible accessories from the registry by connector + capability, not just by capability alone. New `FeatureDesire` variants where useful: `EdgeInference` (already exists) can now resolve to a specific accelerator token; add `LongRangeRadio`, `Localization`, `Actuation`.

---

## 3. Target vendor & product coverage

A living target list. The weekly scout (Section 4) works this list; the table is the standing definition of "breadth."

| Vendor | Ecosystems / lines to cover | Notable targets (examples) |
|---|---|---|
| **Espressif** | All ESP32 SoCs + devkits | ESP32, -S2, -S3, -C3, -C6, -H2, -P4, ESP32-P4 devkit, ESP-EYE, ESP32-S3-BOX |
| **Seeed Studio** | XIAO, Grove, reComputer, AI kits | XIAO ESP32-C3/C6/S3, XIAO RP2350, Grove sensors (large family), reComputer Jetson, **Grove Vision AI v2**, reComputer + **Hailo-8L** |
| **M5Stack** | Core, Stick, Atom, Units, M-Bus | Core2/CoreS3, StickC PLUS2, AtomS3, M5 Units (ENV, ToF, PIR, …), **M5Stack LLM Module (AX630C)** |
| **LILYGO** | T-Display, T-Watch, T-Beam, LoRa | T-Display-S3, T-Watch-S3, T-Beam (LoRa+GPS), T-Deck, T-Echo |
| **Adafruit** | Feather, QT Py, ItsyBitsy, STEMMA QT | Feather ESP32-S3, QT Py, RP2040/RP2350 boards, large STEMMA QT sensor catalog |
| **SparkFun** | Thing Plus, Qwiic | Thing Plus ESP32/RP2350, Qwiic sensor catalog, MicroMod |
| **Raspberry Pi** | Pico, SBCs, accessories | Pico 2 / 2 W, RPi 5 + **AI HAT+ (Hailo)**, AI Camera (IMX500), Sense HAT |
| **Pimoroni** | Pico bases, Enviro, displays | Pico-class boards, Enviro, Inky e-paper, Tufty |
| **DFRobot** | Beetle, FireBeetle, Gravity | FireBeetle ESP32, Gravity sensor line |
| **Radxa / Rockchip** | SBCs with NPU | Radxa ROCK 5 (RK3588, `npu`), Radxa Zero |
| **NVIDIA** | Jetson | Orin Nano/NX, **AGX Thor** (`cuda`+`tensor_rt`) |
| **Hailo / Google Coral** | Accelerators | Hailo-8 / 8L / 10 (M.2/USB), Coral USB / M.2 / Dev Board (`edge_tpu`) |
| **Sipeed / Kendryte** | RISC-V + KPU | Maix (K210), MaixCAM (K230, `kpu`), Tang FPGA |
| **Arduino** | Uno/Nano/Portenta/Nicla | Nano ESP32, Nicla Vision/Voice, Portenta (`nn_accel`) |
| **Waveshare** | Displays, ESP32 boards, HATs | ESP32-S3 displays, e-paper, LoRa HATs |
| **Tindie** | Long-tail / niche | Community boards adding genuinely new capability |

> The aim is not to enumerate every clone, but to cover every **vendor ecosystem**, every **AI accelerator**, every **ESP32-family SoC**, and the popular boards real users own — plus a steady intake of the long tail.

---

## 4. The continuous intake process (recurring)

Breadth decays without maintenance, so hardware seeking is a **standing weekly job**, not a one-off.

### 4.1 Automated weekly scout

A scheduled task (`obc-hardware-scout`, **Mondays 09:00 local**) runs every week and:

1. Reads the current registry so it never proposes duplicates.
2. Web-scans the vendor list (Section 3) for new/newly-noticed products in scope.
3. For each candidate collects: name, vendor, MCU/SoC + architecture, USB VID/PID (verified against vendor/datasheet, or flagged unknown), transport, connector ecosystem, and capability tokens (reusing the taxonomy; flagging any genuinely new token).
4. Writes a dated report to `Knowledge Base/hardware-scout-YYYY-MM-DD.md` containing a ranked shortlist, **ready-to-paste `BoardInfo`/`AccessoryInfo` Rust entries**, any proposed new capability tokens, and a "needs verification" list for unconfirmed VID/PID or specs.
5. Posts a chat summary. It **does not edit `registry.rs`** — every addition is a reviewed proposal.

### 4.2 Triage rubric (what gets merged)

Each proposal is scored:

- **New capability** (a token or accelerator the registry lacks) → high priority.
- **New vendor ecosystem / connector** (first M5Stack Unit, first Qwiic match) → high priority.
- **Popularity** (a board many users own) → medium-high.
- **Long-tail clone with no new capability** → skip.
- **Out of scope** (not embedded/SBC/accelerator/sensor) → skip.

Merge requires a **verified VID/PID** (for USB-enumerating boards) or an explicit "non-enumerating / bridge" note; unverified specs stay in the "needs verification" list until confirmed against a primary source.

### 4.3 Definition of done per addition

- Entry added to `BOARDS` or `KNOWN_ACCESSORIES` with all fields, including `connectors`/`connector`, `vendor`, `ecosystem`.
- Capability tokens valid (or new token documented in the taxonomy comment + tests).
- A registry unit test asserts the entry resolves (mirrors existing `cuda`/accessory tests).
- If it implies firmware work (a new transport, a new on-device sensor driver), a linked ROADMAP item is filed.

---

## 5. Firmware implications

Most additions are **host-side registry metadata** and need no firmware change — the board is identified, its capabilities advertised, and tools routed over the spine. Firmware work is needed only when an addition introduces something the node must *execute*:

- **New transport** (e.g., a CAN-only or RS-485 node, an Ethernet SBC) → a spine transport adapter.
- **New on-device sensor/peripheral driver** (a sensor not in the existing I²C/SPI handlers) → add to the ESP32-S3 (and SBC) driver set; reuse the existing `sensor_read` dispatch where the bus protocol already exists.
- **AI accelerator nodes** (Coral, Hailo, Jetson, K230) → run as **edge-inference nodes** via `EdgeAgent`/the deployment planner (System 1 tier in `docs/V2-IMPLEMENTATION.md` Phase 18/20), advertising `npu`/`edge_tpu`/`hailo`/`kpu`; the accelerator does local inference and the node exposes it as a tool over the spine.
- **Radio nodes** (LoRa/Zigbee/Thread) → typically a gateway node bridging the radio to the spine; modeled as a board with the radio capability plus a bridge transport.

New `BoardInfo`/`AccessoryInfo` fields are added with `#[serde(default)]`-friendly defaults so existing configs and older firmware keep working; a board that doesn't declare `connectors` simply matches nothing connector-specific.

---

## 6. Success measures

- **Vendor coverage:** at minimum Seeed, M5Stack, LILYGO, Adafruit, SparkFun, Espressif, Raspberry Pi, Pimoroni, DFRobot, Radxa, NVIDIA, Hailo, Coral, Sipeed, Arduino, Waveshare all represented by ≥1 entry.
- **Accelerator coverage:** every AI-accelerator token (`npu, edge_tpu, hailo, vpu, kpu, tensor_rt, nn_accel`) has ≥1 board.
- **Connector coverage:** Grove, Qwiic, STEMMA QT, M-Bus, FeatherWing all modeled, with the advisor matching accessories across the Qwiic/STEMMA QT equivalence.
- **Freshness:** the weekly scout runs and produces a dated report; no quarter passes without new merges.
- **Quality:** every merged board has a verified VID/PID (or explicit bridge note) and a passing registry test.

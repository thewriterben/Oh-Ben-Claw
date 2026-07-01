# PR: Hardware registry — scout 2026-06-29 tier-1 additions

**Branch:** `hardware-scout-2026-06-29-tier1` → `main`
**Source:** weekly `obc-hardware-scout` run (`Knowledge Base/hardware-scout-2026-06-29.md`)

## Summary

Adds the metadata-only ("tier-1") hardware from this week's scout: new ESP32
SoCs, the first boards from four new vendor ecosystems, and Qwiic/STEMMA QT
plug-in sensors — plus a capability-token validity guard. All additions ride
already-supported transports, so there is **no firmware change** and no new
deployment-time behavior.

Accelerator boards and blocked-VID/PID entries from the same report are
deliberately **out of scope** here (see Follow-ups).

## What's included

**Capability taxonomy + guard**
- `VALID_CAPABILITIES` + `is_valid_capability()` in `peripherals::registry`.
- `all_capabilities_are_valid` test fails the build if any board/accessory uses
  an undocumented token. New tokens documented in the module header and reserved
  for upcoming hardware: `npu`, `edge_tpu`, `hailo`, `nn_accel`, `kpu`,
  `tensor_rt`, `ethernet`, `thread`, `zigbee`, `battery`.

**Boards (8)**
- Espressif **ESP32-C6**, **ESP32-H2** (BLE + 802.15.4 `thread`/`zigbee`; H2 has
  no Wi-Fi), **ESP32-P4** (`nn_accel` + MIPI camera/display).
- **Adafruit QT Py ESP32-S3** (first Adafruit board; first STEMMA QT host).
- **SparkFun Thing Plus ESP32-C6** (first SparkFun board; first Qwiic host).
- **DFRobot FireBeetle 2 ESP32-S3** (first DFRobot board; `battery`).
- **LILYGO T-Display-S3**, **T-Deck** (T-Deck adds LoRa + touch).

**Accessories (4)** — Qwiic / STEMMA QT plug-ins that exercise connector matching:
- **SCD41** (CO2), **VL53L1X** (ToF), **BNO055** (9-DOF fusion IMU), **SGP40** (VOC).

**Tests**
- Resolve test per new board/accessory, vendor-coverage test, and a
  Qwiic ↔ STEMMA QT cross-mate test.
- Updated `accessories_for_board_includes_bare_modules` (it previously assumed
  every accessory was `Bare`, which the new connector-specific modules break).

**Generated**
- `registry/registry.json` regenerated via `cargo run --bin emit-registry`.

## Notes for reviewers

- Native-USB ESP32 parts all enumerate as `0x303a:0x1001` (Espressif
  USB-Serial/JTAG); they are selected by `name`, consistent with the existing
  registry convention. `lookup_board(0x303a, 0x1001)` still resolves to
  `esp32-s3` (first match) — no existing test changes.
- VID/PID provenance for each entry is in the scout report's §4.

## Verification

```
cargo test peripherals::registry        # all green
cargo run --bin emit-registry -- registry/registry.json
git diff --exit-code registry/registry.json   # drift guard: clean after regen
```

## Follow-ups (separate PR)

1. **AI-accelerator boards** — RPi AI HAT+ (`hailo`), Coral USB / Dev Board Mini
   (`edge_tpu`), Radxa ROCK 5B (`npu`), Jetson Orin Nano (`tensor_rt`), M5Stack
   Module LLM (`npu`), Grove Vision AI V2 (`nn_accel`). Tokens are already
   reserved in `VALID_CAPABILITIES`, so this is purely additive.
2. **`FeatureDesire::EdgeInference` wiring** — currently returns `&[]`; point it
   at the accelerator tokens so accelerator boards actually resolve to the
   desire (and add `LongRangeRadio`/`Localization`/`Actuation` desires).
3. **Blocked-PID entries** — Adafruit Feather ESP32-S3 and Sipeed MaixCAM, once
   their USB IDs are confirmed against a primary source.
4. **Optional** — `Connector::Gravity` (DFRobot) if the enum should model it
   rather than treating Gravity-I2C as Qwiic-equivalent.

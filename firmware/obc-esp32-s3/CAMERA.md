# Enabling the OV2640 camera

The camera is **opt-in**. The default firmware build needs no PSRAM and no camera
component — `camera_capture` returns a stub. This guide turns on a real OV2640
capture via the `espressif/esp32-camera` IDF component. Three files + a feature flag.

> **Board caveat.** On the Waveshare ESP32-S3 Touch LCD 2.1 the camera's SCCB
> (its I2C control bus) is on **GPIO4/5 — the same pins as the I2C sensor bus**.
> The firmware therefore **disables the sensor I2C bus when the `camera` feature is
> on** (`#[cfg(not(feature = "camera"))]`). You get sensors *or* the camera on this
> board unless you rewire one of them. Battery safing (MAX17048) reverts to the stub
> in camera builds.

## 1. Pull the camera component

Create `idf_component.yml` in this directory:

```yaml
dependencies:
  espressif/esp32-camera: "^2.0.4"
```

`esp-idf-sys` reads this, downloads the component, compiles it, and generates the
`esp_camera_*` / `camera_config_t` bindings this firmware's `src/camera.rs` uses.

## 2. Configure PSRAM (required for the frame buffer)

Create (or extend) `sdkconfig.defaults` in this directory:

```
# PSRAM holds the JPEG frame buffer. Match the mode to YOUR module — most 8 MB
# ESP32-S3 modules are octal (OPI); some are quad (QSPI). Wrong mode = the board
# boots but esp_camera_init fails.
CONFIG_SPIRAM=y
CONFIG_SPIRAM_MODE_OCT=y
CONFIG_SPIRAM_SPEED_80M=y

# A little more main-task stack for the capture path.
CONFIG_ESP_MAIN_TASK_STACK_SIZE=8192
```

## 3. Build with the feature

```powershell
$env:CARGO_TARGET_DIR = "C:\e"    # Windows path-length workaround
cargo build --release --features camera
cargo espflash flash --release --features camera --monitor
```

On boot you should see `OV2640 camera initialised` (or a warning if init failed).

## 4. Smoke test

```json
{"id":"1","cmd":"camera_capture","args":{"quality":10}}
```
A healthy board returns `ok:true` with a long base64 JPEG string (no longer the
`STUB:` placeholder). Decode it to a `.jpg` to confirm the image.

## Troubleshooting / caveats

- **`esp_camera_init failed`** — almost always PSRAM mode (step 2). Try QUAD vs OCT.
- **FFI field-name mismatch at compile time** — `src/camera.rs` uses the esp32-camera
  ≥ 2.0 names `pin_sccb_sda` / `pin_sccb_scl`. Older components spelled them
  `pin_sscb_sda` / `pin_sscb_scl`; if the compiler complains about unknown fields,
  pin the component to a 2.x version (step 1) or rename to match.
- **Enum names** (`pixformat_t_PIXFORMAT_JPEG`, `framesize_t_FRAMESIZE_QVGA`, …) are
  the bindgen-generated names; if one differs, the compiler names the correct symbol.
- **Frame size** defaults to QVGA (320×240) to fit a single PSRAM buffer; raise
  `frame_size` in `src/camera.rs::init` if you have the PSRAM headroom.
- **This module is untested on metal** — treat the pin map and config as a starting
  point and verify against your board's schematic.

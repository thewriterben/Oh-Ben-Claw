# Enabling the OV2640 camera

The camera is **opt-in**. The default firmware build needs no PSRAM and no camera
component — `camera_capture` returns a stub. This guide turns on a real OV2640
capture via the `espressif/esp32-camera` IDF component. It takes **two** cargo
features (`camera` plus a board), **one** environment variable, and **one**
uncommented block in `Cargo.toml` that cannot be feature-gated — read §1 before
you uncomment it.

> **2026-09-16: `--features camera` alone no longer compiles.** A camera build
> must also name its board, because the pin map is a property of the board and
> nothing used to force anyone to say which one they meant. That is how the map
> below was wrong twice. The choices:
>
> | feature | board | sdkconfig overlay | pin map source |
> |---|---|---|---|
> | `board-xiao-sense` | Seeed XIAO ESP32S3 **Sense** | `sdkconfig.defaults.camera-xiao-sense` (PSRAM **OCT**, measured) | Seeed wiki camera-slot table, 2026-09-16, cited in `camera.rs` |
> | `board-lilygo-tcam-s3-v11` | LILYGO T-CameraPlus-S3 **V1.0/V1.1 only** | `sdkconfig.defaults.camera-lilygo-tcam-v11` (PSRAM **QUAD**, vendor-cited) | vendor `pin_config.h` + README, 2026-09-16, cited in `camera.rs` |
> | `board-unverified-map` | **none known** | — | the historical unattributed set — quarantined, do not bring up hardware with it |
>
> Omitting a board, or naming two, is a `compile_error!` that names the choice.
> `board-waveshare-21` + `camera` remains a `compile_error!` (no camera connector).
>
> **The overlay is not interchangeable.** PSRAM mode is a property of the module:
> the XIAO is octal, the Lilygo is quad. Pairing a board with the wrong overlay
> does not fail cleanly — the board boots and `esp_camera_init` fails, or PSRAM
> misbehaves quietly. Nothing enforces the pairing at compile time, because
> sdkconfig is invisible to `cfg`; the table above is the enforcement, which is to
> say it is you.
>
> **V1.2 is a different board.** Only three camera pins differ (VSYNC, PWDN,
> RESET — GPIO3 and GPIO4 swap roles), which is exactly what makes it dangerous,
> and the vendor ships `pin_config.h` with V1.2 selected. There is no
> `board-lilygo-tcam-s3-v12` feature; if you have that revision, add one with its
> own citation rather than reusing V1.1's.

> **Board caveat, corrected twice on 2026-08-21 — the second time by looking
> at the board.** This document and `camera.rs` both described the pin map
> below as the Waveshare ESP32-S3-Touch-LCD-2.1's, "OV2640 via the FPC
> connector". **That board has no camera connector.** Its only FPC connector
> is the screen's. `Cargo.toml` and `BENCH-PINOUT-CARDS.md` Card 3 had said so
> since July; the firmware said otherwise and nothing compared them.
>
> Which board that map is for is still unknown — see `camera.rs`. It now lives
> behind `board-unverified-map` and is not what a camera build gets by default,
> because there is no default. Building it against `board-waveshare-21` is a
> compile error, because those pins are that board's LCD lines.
>
> The earlier correction the same day: the sensor-bus overlap ("the same pins as
> the I2C sensor bus") was the stated reason for disabling that bus in camera
> builds, and it is not true of either build. The Waveshare's bus is 15/7 and
> the default XIAO bus moved from 4/5 to **5/6** (the pads the silkscreen marks
> SDA and SCL).
>
> The firmware still **disables the sensor I2C bus when the `camera` feature
> is on** (`#[cfg(not(feature = "camera"))]`), so you still get sensors *or*
> the camera, and battery safing (MAX17048) still reverts to the stub in
> camera builds. That gate is now conservative rather than necessary, and
> removing it means running the bus in a build that has never had it — a
> bench job, not an edit. `main.rs` and `camera.rs` also disagree about
> whether this board has a camera connector at all; resolve that first.

## 1. Pull the camera component — **this step dirties the whole tree**

Uncomment the four `[[package.metadata.esp-idf-sys.extra_components]]` lines at
the bottom of `Cargo.toml`. `esp-idf-sys` generates `idf_component.yml` from them,
downloads the component, compiles it into the ESP-IDF, and generates the
`esp_camera_*` / `camera_config_t` bindings that `src/camera.rs` uses.

> **There is no way to gate this on a cargo feature, and it is not for want of
> looking.** `extra_components` is passed to `try_from_env()` as an *exclude*
> (`cargo_driver/config.rs:72-74`) and its own doc comment says "This option is
> not available as an environment variable." Worse, the `cargo metadata` call that
> reads it passes no feature flags at all (`config.rs:107-113`), and
> `CARGO_FEATURE_*` is never read anywhere in esp-idf-sys's `build/`. The build
> script structurally cannot see which of your features are on.
>
> So with those lines uncommented, a **default** build — camera feature off —
> still downloads the component, still compiles it, still emits the bindings
> module, and produces a different binary from the one on the live node.
>
> Containment is therefore social, and deliberately visible: **the block stays
> commented on `main`, and camera bring-up happens on a `camera-bringup`
> branch.** A tree that would flash the wrong thing to `obc-esp32-s3-001` is then
> a branch name you can see in your prompt, not a note in a document you are not
> reading. Do not commit an uncommented block to `main`. See `DECISIONS.md`
> (OBC-Prime), 2026-09-16.

## 2. Configure PSRAM (required for the frame buffer)

**Do not edit `sdkconfig.defaults`.** That file is read by *every* build from this
tree, so forcing PSRAM on there changes the binary a default build produces —
including one flashed to the live mesh node, whose PSRAM mode is unverified and
whose boot that can break. The overlay already exists as
`sdkconfig.defaults.camera-<board>`; you select it per-build with an environment
variable:

```powershell
$env:ESP_IDF_SDKCONFIG_DEFAULTS = "sdkconfig.defaults;sdkconfig.defaults.camera-xiao-sense"
```

Semicolon-separated, later files win, and the variable **replaces** the default
list rather than appending — so `sdkconfig.defaults` must be named explicitly or
you lose the 32 KB main-task stack. (`esp-idf-sys` 0.37.2: `build/config.rs:25-27`
declares the var, `parse::list` at `config.rs:187-202` splits on `;`,
`set_when_none` at `config.rs:139-144` is why it replaces.) It is declared
`cargo:rerun-if-env-changed`, so setting or clearing it re-triggers the build
script on its own.

Unset the variable — or open a fresh shell — before building anything for the
live node. Its absence is what makes a default build a default build.

**Measured 2026-09-16 on the XIAO, `TODO(source)` closed.** `MODE_OCT` is correct
on the XIAO ESP32S3 Sense (MAC `64:E8:33:7E:7E:04`, chip rev v0.2, ESP-IDF
v5.3.2): boot log reports `esp_psram: SPI SRAM memory test OK` and `Adding pool of
8192K of PSRAM memory to heap allocator`. QUAD was never needed **on that board**
— the Lilygo is the opposite and that is the whole reason these overlays are
per-board.

## 3. Build with the features

```powershell
git switch -c camera-bringup                  # §1: the tree is dirty, make it visible
$env:CARGO_TARGET_DIR = "C:\e"                # Windows path-length workaround
$env:ESP_IDF_SDKCONFIG_DEFAULTS = "sdkconfig.defaults;sdkconfig.defaults.camera-xiao-sense"
cargo build --release --features camera,board-xiao-sense
cargo espflash flash --release --features camera,board-xiao-sense --monitor
```

Substitute the board feature for your board. `--features camera` on its own stops
at a `compile_error!` that lists the options — that is the point of it.

On boot you should see `OV2640 camera initialised` (or a warning if init failed).

## 4. Smoke test

```json
{"id":"1","cmd":"camera_capture","args":{"quality":10}}
```
A healthy board returns `ok:true` with a long base64 JPEG string (no longer the
`STUB:` placeholder). Decode it to a `.jpg` to confirm the image.

## Troubleshooting / caveats

- **`esp_camera_init failed`** — almost always PSRAM mode (step 2). Swap
  the PSRAM mode in your board's overlay, and record which one worked — that `TODO(source)`
  closes the moment a board proves it.
- **No PSRAM at all in the build** — check `$env:ESP_IDF_SDKCONFIG_DEFAULTS` is
  set in *this* shell. It is per-shell by design; a fresh terminal is a default
  build.
- **Boot loops or a short main-task stack after setting the variable** — you
  probably wrote only `sdkconfig.defaults.camera` into it. The variable replaces
  the list, it does not extend it, so the 32 KB stack silently reverted to the
  IDF default. Name both files.
- **Dead end, so nobody re-derives it:** `sdkconfig.defaults.<profile>` is
  expanded automatically (`common.rs:263-300`), but `profile` comes from cargo's
  `PROFILE`, which is only ever `debug` or `release` (`common.rs:259-261`). A
  custom `[profile.camera]` does *not* produce `sdkconfig.defaults.camera`. The
  chip suffix (`.esp32s3`) applies to both builds and so discriminates nothing.
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

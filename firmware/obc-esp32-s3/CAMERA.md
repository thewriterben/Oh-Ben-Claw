# Enabling the OV2640 camera

The camera is **opt-in**. The default firmware build needs no PSRAM and no camera
component — `camera_capture` returns a stub. This guide turns on a real OV2640
capture via the `espressif/esp32-camera` IDF component.

**Since 2026-09-26 a camera build is made from `main`**, from its own crate,
`firmware/obc-esp32-s3-camera`, which has no sources of its own: it builds this
crate's `src/main.rs` with the camera component added. One command does it:

```powershell
cd C:\src\obc
.\scripts\build_camera.ps1 -Board xiao-sense                 # build only
.\scripts\build_camera.ps1 -Board xiao-sense -Port COM11     # build, flash, monitor
```

§1–§3 say why it is shaped like that. The `camera-bringup` branch is retired:
its greyscale capture, software JPEG encoder and on-node detector
(`camera_detect`) are on `main` as of 2026-09-26.

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
> does not fail cleanly on hardware — the board boots and `esp_camera_init`
> fails, or PSRAM misbehaves quietly. **Since 2026-09-26 it fails at compile
> time instead.** This used to say sdkconfig is invisible to `cfg`; it is not.
> esp-idf-sys turns every enabled kconfig bool into an `esp_idf_<name>` cfg and
> the crate's `build.rs` passes them on, so `camera.rs` refuses a camera build
> without `esp_idf_spiram`, a Sense without `esp_idf_spiram_mode_oct`, and a
> Lilygo without `esp_idf_spiram_mode_quad`. Nothing here has compiled that
> check on hardware yet; the first camera build from `main` is its test.
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

## 1. The camera component, and why it has a crate of its own

`esp-idf-sys` adds the component from a
`[[package.metadata.esp-idf-sys.extra_components]]` block, generates
`idf_component.yml` from it, downloads and compiles the component into the
ESP-IDF, and generates the `esp_camera_*` / `camera_config_t` bindings that
`src/camera.rs` uses.

> **There is no way to gate that block on a cargo feature, and it is not for want
> of looking.** `extra_components` is passed to `try_from_env()` as an *exclude*
> (`cargo_driver/config.rs:72-74`) and its own doc comment says "This option is
> not available as an environment variable." Worse, the `cargo metadata` call that
> reads it passes no feature flags at all (`config.rs:107-113`), and
> `CARGO_FEATURE_*` is never read anywhere in esp-idf-sys's `build/`. The build
> script structurally cannot see which of your features are on.
>
> What it *can* see is which crate is the root. The block is read from the root
> crate's manifest and its direct dependencies
> (`cargo_driver/config.rs:240-320`). So the block lives, uncommented, in
> `firmware/obc-esp32-s3-camera/Cargo.toml`, and this crate's copy stays
> commented — `scripts/check_camera_component_gate.py --enforce` holds that on
> `main`. A build of this crate is the live node's build, whatever branch you are
> on; a build of the camera crate is a camera build. Which one you get is the
> directory you build in, not a branch name or a variable you have to remember.
>
> Two options that were rejected, for the record (2026-09-26): a helper crate
> carrying the block as an *optional* dependency never appears in the
> featureless `cargo metadata` resolve, and a non-optional one reaches every
> build; selecting the root with `ESP_IDF_SYS_ROOT_CRATE` works, but a forgotten
> variable is a silently different binary, which is the failure this exists to
> prevent.

The camera crate must stay the same program as this one. Its dependencies,
features, bin name, `.cargo/config.toml` and `rust-toolchain.toml` are compared
with this crate's by `scripts/check_camera_crate_drift.py` in CI.

## 2. Configure PSRAM (required for the frame buffer)

**Do not edit `sdkconfig.defaults`.** That file is read by *every* build,
including one flashed to the live mesh node, whose boot PSRAM can break. The
overlays are `sdkconfig.defaults.camera-<board>` in this directory, and the camera
crate names them itself:

- Its manifest's `[package.metadata.esp-idf-sys] esp_idf_sdkconfig_defaults`
  names `sdkconfig.defaults` plus the **Sense** overlay (two of the three camera
  boards on the bench are Senses).
- `ESP_IDF_SDKCONFIG_DEFAULTS`, when set, **wins** over the manifest and
  **replaces** its list rather than appending (`set_when_none`,
  `config.rs:139-144`). `build_camera.ps1` always sets it, to both files by
  absolute path, for the board you name — so a value left in the shell from an
  earlier build is overwritten rather than trusted.

Naming only the overlay drops the 32 KB main-task stack and boot-loops for a
reason that looks nothing like its cause; naming none leaves PSRAM off. The
second is now a compile error (see the table above). The variable is
`cargo:rerun-if-env-changed`, so switching boards re-runs the esp-idf build on
its own.

The reverse leak is guarded too: a build of **this** crate with PSRAM on — the
variable left set from a camera build, or an esp-idf build a camera build made —
is a `compile_error!` in `main.rs`, because it is not the live node's binary.

**Measured 2026-09-16 on the XIAO, `TODO(source)` closed.** `MODE_OCT` is correct
on the XIAO ESP32S3 Sense (MAC `64:E8:33:7E:7E:04`, chip rev v0.2, ESP-IDF
v5.3.2): boot log reports `esp_psram: SPI SRAM memory test OK` and `Adding pool of
8192K of PSRAM memory to heap allocator`. QUAD was never needed **on that board**
— the Lilygo is the opposite and that is the whole reason these overlays are
per-board.

## 3. Build (and flash)

```powershell
cd C:\src\obc
.\scripts\which_esp32.ps1                                          # which port is which board
.\scripts\build_camera.ps1 -Board xiao-sense -Port COM11
.\scripts\build_camera.ps1 -Board lilygo-tcam-v11 -Port COMx       # the Lilygo
```

What the script does, so it can be done by hand if it has to be:

```powershell
. $env:USERPROFILE\export-esp.ps1
$env:CARGO_TARGET_DIR = "C:\ec-cam"     # NOT the node build's target dir -- see below
$env:ESP_IDF_SDKCONFIG_DEFAULTS = "C:\src\obc\firmware\obc-esp32-s3\sdkconfig.defaults;C:\src\obc\firmware\obc-esp32-s3\sdkconfig.defaults.camera-xiao-sense"
cd C:\src\obc\firmware\obc-esp32-s3-camera
cargo run --release --features board-xiao-sense -- --port COM11
```

`camera` is the camera crate's default feature; the board feature is still
required. With `-Port`, the script reads the chip's MAC with
`espflash board-info` first and refuses `obc-esp32-s3-001`.

**The target dir must be the camera crate's own.** esp-idf-sys's build output is
keyed by esp-idf-sys's features, not by which crate is the root, and its build
script does not re-run when a manifest's metadata changes — so two crates in one
target dir share one esp-idf build (on 2026-09-16 that destroyed the bring-up
branch's camera bindings). The shared `build.rs` records which crate first built
in a target dir and refuses the other with that explanation. A target dir that
already held camera builds of *this* crate from the `camera-bringup` days (for
example `C:\ec`) has a camera esp-idf in it: `cargo clean` it, or the next node
build stops at the PSRAM `compile_error!` in `main.rs`.

On boot you should see `OV2640 camera initialised` (or a warning if init failed).

## 4. Smoke test

```json
{"id":"1","cmd":"camera_capture","args":{"quality":10}}
```
A healthy board returns `ok:true` with a long base64 JPEG string (no longer the
`STUB:` placeholder). Decode it to a `.jpg` to confirm the image. It is
**monochrome**: the sensor captures `PIXFORMAT_GRAYSCALE` for the detector
(ADR 2026-09-17, `camera.rs`), and `fmt2jpg_cb` encodes that frame in software.
The console logs `capture: frame len=76800 B, 320x240, format=3` first, which is
what `scripts/probe_sense_capture.py` counts.

```json
{"id":"2","cmd":"camera_detect"}
```
Compares this frame with the previous one. The first reply is `no_reference`,
then `warming_up` while auto-exposure settles, then a `class`. Every reply
carries `thresholds_provisional: true` until the thresholds are measured
on-node (`docs/VISION-DETECTOR-2026-09.md`). `scripts/probe_detect.py` runs it.

## Troubleshooting / caveats

- **`esp_camera_init failed`** — almost always PSRAM mode (step 2). Swap
  the PSRAM mode in your board's overlay, and record which one worked — that `TODO(source)`
  closes the moment a board proves it.
- **Do not use the boot log's `app_init: Compile time:` to tell whether your
  change is on the board.** It is the ESP-IDF app descriptor's timestamp and does
  not move when only Rust changes — observed 2026-09-16, where two different
  binaries minutes apart both reported `Sep 16 2026 12:05:12`. Verify with
  something your change actually alters: the `fb_count` work was confirmed by
  counting `cam_hal: Allocating ... frame buffer in PSRAM` lines (one per
  buffer). Checking the wrong signal here costs you a measurement on a board you
  only think you flashed — which happened once this session already, when a
  timed-out command left the old binary running and the "result" was noise.
- **No PSRAM at all in the build** — now a `compile_error!` naming the fix
  rather than a camera that never captures. Build through `build_camera.ps1`, or
  from `firmware/obc-esp32-s3-camera`, whose manifest names the overlay.
- **Boot loops or a short main-task stack after setting the variable by hand** —
  you probably wrote only the overlay into it. The variable replaces the list, it
  does not extend it, so the 32 KB stack silently reverted to the IDF default.
  Name both files, as the script does.
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

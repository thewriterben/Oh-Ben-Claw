// Extra bindings for esp-idf-sys — exposes the esp32-camera managed component's
// API (camera_config_t, esp_camera_init, esp_camera_fb_get, …) to Rust in the
// `esp_idf_sys::camera` module. Referenced by Cargo.toml
// [[package.metadata.esp-idf-sys.extra_components]].
//
// Guarded on the component-enabled cfg so it's a no-op if the component isn't built.
#if defined(ESP_IDF_COMP_ESPRESSIF__ESP32_CAMERA_ENABLED)
#include "esp_camera.h"
// The software JPEG encoder. Needed because this node captures
// PIXFORMAT_GRAYSCALE for the detector (ADR 2026-09-17) and the sensor
// therefore never produces a JPEG of its own -- `fmt2jpg_cb` is how a frame
// becomes a picture a human can look at.
//
// The callback form, not `fmt2jpg`: that one mallocs a hardcoded 128 KiB and
// its output stream silently clamps on overflow, with the warning that would
// have told you commented out in the vendor source. Silent truncation is the
// degradation this project's rules forbid.
#include "img_converters.h"
#endif

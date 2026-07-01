// Extra bindings for esp-idf-sys — exposes the esp32-camera managed component's
// API (camera_config_t, esp_camera_init, esp_camera_fb_get, …) to Rust in the
// `esp_idf_sys::camera` module. Referenced by Cargo.toml
// [[package.metadata.esp-idf-sys.extra_components]].
//
// Guarded on the component-enabled cfg so it's a no-op if the component isn't built.
#if defined(ESP_IDF_COMP_ESPRESSIF__ESP32_CAMERA_ENABLED)
#include "esp_camera.h"
#endif

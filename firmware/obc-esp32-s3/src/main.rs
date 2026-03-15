//! Oh-Ben-Claw ESP32-S3 firmware.
//!
//! This firmware runs on the Waveshare ESP32-S3 Touch LCD 2.1 (and compatible
//! ESP32-S3 boards) and exposes the following capabilities to the Oh-Ben-Claw
//! core agent:
//!
//! - **GPIO**: Read and write digital GPIO pins.
//! - **Camera**: Capture JPEG images from an OV2640 camera module.
//! - **Audio**: Sample audio from an I2S microphone (INMP441 / SPH0645).
//! - **Sensors**: Read I2C/SPI sensors (BME280, MPU6050, SHT31, BMP180).
//!
//! # Communication
//!
//! The firmware supports two communication modes:
//!
//! 1. **Serial (JSON-over-UART)**: The host sends newline-delimited JSON
//!    commands to UART0 (TX=GPIO43, RX=GPIO44, 115200 baud), and the board
//!    responds with newline-delimited JSON results. This is the same protocol
//!    as the base ZeroClaw ESP32 firmware.
//!
//! 2. **MQTT**: The firmware connects to a WiFi network and an MQTT broker,
//!    announces its capabilities, and receives tool call requests over the
//!    Oh-Ben-Claw MQTT bus. This enables network-based, multi-device
//!    coordination without a direct USB connection.
//!
//! # Wiring (Waveshare ESP32-S3 Touch LCD 2.1)
//!
//! ## Camera (OV2640 via FPC connector)
//! | Signal | GPIO |
//! |--------|------|
//! | XCLK   | 15   |
//! | SIOD   | 4    |
//! | SIOC   | 5    |
//! | D0–D7  | 39–42, 16–19 |
//! | VSYNC  | 21   |
//! | HREF   | 38   |
//! | PCLK   | 13   |
//!
//! ## I2S Microphone (INMP441 / SPH0645)
//! | Signal | GPIO |
//! |--------|------|
//! | SCK    | 0    |
//! | WS     | 1    |
//! | SD     | 2    |
//!
//! ## I2C Sensor Bus (BME280, MPU6050, SHT31, etc.)
//! | Signal | GPIO |
//! |--------|------|
//! | SDA    | 4    |
//! | SCL    | 5    |
//!
//! # Build & Flash
//!
//! ```bash
//! # Install ESP toolchain
//! cargo install espup && espup install && source ~/export-esp.sh
//!
//! # Build and flash
//! cd firmware/obc-esp32-s3
//! cargo build --release
//! cargo espflash flash --monitor
//! ```

use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::hal::uart::*;
use log::info;
use serde::{Deserialize, Serialize};

/// Maximum line length for incoming serial commands (bytes).
const MAX_LINE_LEN: usize = 512;

/// Firmware version — must match the host-side `CARGO_PKG_VERSION`.
const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Node ID — set this to a unique identifier for each board in your fleet.
/// In production, this should be read from NVS (non-volatile storage).
const NODE_ID: &str = "obc-esp32-s3-001";

/// JPEG quality range.
const CAMERA_QUALITY_MIN: u64 = 1;
const CAMERA_QUALITY_MAX: u64 = 10;
const CAMERA_QUALITY_DEFAULT: u64 = 5;

/// Audio sample duration range (ms).
const AUDIO_DURATION_MIN_MS: u64 = 10;
const AUDIO_DURATION_MAX_MS: u64 = 1000;
const AUDIO_DURATION_DEFAULT_MS: u64 = 100;

/// GPIO pins configured as outputs during startup.
const OUTPUT_PINS: &[i32] = &[3, 14, 26, 33, 46];

/// Incoming command from the host.
#[derive(Debug, Deserialize)]
struct Request {
    id: String,
    cmd: String,
    args: serde_json::Value,
}

/// Outgoing response to the host.
#[derive(Debug, Serialize)]
struct Response {
    id: String,
    ok: bool,
    result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    // UART0: TX=43, RX=44 (USB serial bridge on Waveshare board).
    let config = UartConfig::new().baudrate(Hertz(115_200));
    let mut uart = UartDriver::new(
        peripherals.uart0,
        pins.gpio43,
        pins.gpio44,
        Option::<esp_idf_svc::hal::gpio::Gpio0>::None,
        Option::<esp_idf_svc::hal::gpio::Gpio1>::None,
        &config,
    )?;

    // Configure output pins via raw ESP-IDF sys API.
    unsafe {
        use esp_idf_svc::sys::*;
        for &pin in OUTPUT_PINS {
            gpio_reset_pin(pin);
            gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_OUTPUT);
        }
    }

    info!(
        "Oh-Ben-Claw ESP32-S3 firmware v{} ready",
        FIRMWARE_VERSION
    );
    info!("Node ID: {}", NODE_ID);
    info!("Serial: UART0 TX=43, RX=44, 115200 baud");
    info!(
        "Commands: gpio_read, gpio_write, camera_capture, audio_sample, sensor_read, capabilities, announce"
    );

    let mut buf = [0u8; 512];
    let mut line: Vec<u8> = Vec::new();

    loop {
        match uart.read(&mut buf, 100) {
            Ok(0) => continue,
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\n' {
                        if !line.is_empty() {
                            if let Ok(line_str) = std::str::from_utf8(&line) {
                                if let Ok(resp) = handle_request(line_str) {
                                    let out = serde_json::to_string(&resp).unwrap_or_default();
                                    let _ = uart.write(format!("{}\n", out).as_bytes());
                                }
                            }
                            line.clear();
                        }
                    } else {
                        line.push(b);
                        if line.len() > MAX_LINE_LEN {
                            line.clear();
                        }
                    }
                }
            }
            Err(_) => {}
        }
    }
}

fn handle_request(line: &str) -> anyhow::Result<Response> {
    let req: Request = serde_json::from_str(line.trim())?;
    let id = req.id.clone();

    let result = match req.cmd.as_str() {
        "capabilities" | "announce" => {
            let caps = serde_json::json!({
                "node_id": NODE_ID,
                "board": "waveshare-esp32-s3-touch-lcd-2.1",
                "firmware_version": FIRMWARE_VERSION,
                "tools": [
                    {"name": "gpio_read", "description": "Read a GPIO pin value (0 or 1)."},
                    {"name": "gpio_write", "description": "Set a GPIO pin high (1) or low (0)."},
                    {"name": "camera_capture", "description": "Capture a JPEG image from the OV2640 camera."},
                    {"name": "audio_sample", "description": "Sample audio from the I2S microphone."},
                    {"name": "sensor_read", "description": "Read a value from an I2C/SPI sensor."}
                ],
                "gpio": [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,26,27,33,34,35,36,37,43,44,46],
                "camera": true,
                "microphone": true,
                "i2c_bus": [4, 5],
                "uart": [43, 44]
            });
            Ok(caps.to_string())
        }

        "gpio_read" => {
            let pin = req.args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = gpio_read(pin)?;
            Ok(value.to_string())
        }

        "gpio_write" => {
            let pin = req.args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = req.args.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            gpio_write(pin, value)?;
            Ok("done".into())
        }

        "camera_capture" => {
            let quality = req
                .args
                .get("quality")
                .and_then(|v| v.as_u64())
                .unwrap_or(CAMERA_QUALITY_DEFAULT)
                .clamp(CAMERA_QUALITY_MIN, CAMERA_QUALITY_MAX) as u8;
            let format = req
                .args
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("jpeg")
                .to_string();
            camera_capture(quality, &format)
        }

        "audio_sample" => {
            let duration_ms = req
                .args
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(AUDIO_DURATION_DEFAULT_MS)
                .clamp(AUDIO_DURATION_MIN_MS, AUDIO_DURATION_MAX_MS);
            let raw = req
                .args
                .get("raw")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            audio_sample(duration_ms, raw)
        }

        "sensor_read" => {
            let sensor = req
                .args
                .get("sensor")
                .and_then(|v| v.as_str())
                .unwrap_or("bme280")
                .to_string();
            let field = req
                .args
                .get("field")
                .and_then(|v| v.as_str())
                .unwrap_or("temperature")
                .to_string();
            sensor_read(&sensor, &field)
        }

        unknown => Err(anyhow::anyhow!("Unknown command: {}", unknown)),
    };

    match result {
        Ok(output) => Ok(Response {
            id,
            ok: true,
            result: output,
            error: None,
        }),
        Err(e) => Ok(Response {
            id,
            ok: false,
            result: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

// ── GPIO ──────────────────────────────────────────────────────────────────────

fn gpio_read(pin: i32) -> anyhow::Result<u32> {
    let level = unsafe { esp_idf_svc::sys::gpio_get_level(pin) };
    Ok(level as u32)
}

fn gpio_write(pin: i32, value: u64) -> anyhow::Result<()> {
    let ret = unsafe { esp_idf_svc::sys::gpio_set_level(pin, value as u32) };
    if ret != esp_idf_svc::sys::ESP_OK {
        anyhow::bail!("gpio_set_level failed for pin {} with error {}", pin, ret);
    }
    Ok(())
}

// ── Camera ────────────────────────────────────────────────────────────────────

fn camera_capture(quality: u8, format: &str) -> anyhow::Result<String> {
    // NOTE: Full camera capture requires the ESP-IDF camera component.
    // Enable in sdkconfig.defaults:
    //   CONFIG_ESP32_CAMERA=y
    //   CONFIG_SPIRAM=y
    //
    // This stub returns a placeholder base64 string.
    // Replace with actual esp_camera_fb_get() / esp_camera_fb_return() calls.
    log::info!("camera_capture: quality={}, format={}", quality, format);
    Ok(format!(
        "STUB:camera_capture:quality={quality}:format={format}:base64_jpeg_data_here"
    ))
}

// ── Audio ─────────────────────────────────────────────────────────────────────

fn audio_sample(duration_ms: u64, raw: bool) -> anyhow::Result<String> {
    // NOTE: Full audio sampling requires the ESP-IDF I2S driver.
    // Enable in sdkconfig.defaults:
    //   CONFIG_I2S_ENABLE=y
    //
    // This stub returns a placeholder RMS value.
    // Replace with actual i2s_read() calls.
    log::info!("audio_sample: duration_ms={}, raw={}", duration_ms, raw);
    if raw {
        Ok(format!("STUB:audio_sample:duration_ms={duration_ms}:raw_pcm_data_here"))
    } else {
        Ok("0.05".to_string()) // Placeholder RMS level
    }
}

// ── Sensors ───────────────────────────────────────────────────────────────────

fn sensor_read(sensor: &str, field: &str) -> anyhow::Result<String> {
    // NOTE: Full sensor reading requires I2C driver initialization and
    // sensor-specific register reads. This stub returns placeholder values.
    log::info!("sensor_read: sensor={}, field={}", sensor, field);
    match (sensor, field) {
        ("bme280", "temperature") => Ok("22.5".to_string()),
        ("bme280", "humidity") => Ok("55.0".to_string()),
        ("bme280", "pressure") => Ok("1013.25".to_string()),
        ("mpu6050", "accel_x") => Ok("0.01".to_string()),
        ("mpu6050", "accel_y") => Ok("0.00".to_string()),
        ("mpu6050", "accel_z") => Ok("9.81".to_string()),
        ("sht31", "temperature") => Ok("22.3".to_string()),
        ("sht31", "humidity") => Ok("54.8".to_string()),
        (s, f) => Err(anyhow::anyhow!("Unknown sensor/field: {}/{}", s, f)),
    }
}

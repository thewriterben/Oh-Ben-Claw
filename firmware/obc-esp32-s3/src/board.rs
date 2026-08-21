//! What this node is, as data rather than as `#[cfg]`.
//!
//! Every field here varies by build, and every one of them was a literal
//! written out inside the `capabilities` reply until 2026-08-21 — so a
//! Waveshare node announced the XIAO's name, the XIAO's output pins, an I2C bus
//! it does not open, and a microphone it does not have. A host has no other
//! source for any of it.
//!
//! Making the variants ordinary consts, and `describe` an ordinary function, is
//! what lets a host-side test check *all* of them. `#[cfg]` cannot be tested
//! from a shim: the firmware's features do not exist in the workspace crate
//! that includes this file, so a cfg-shaped self-report can only ever be
//! observed in its default form — the harness would confirm the one case that
//! was already right and miss every wrong one. The single `#[cfg]` left picks
//! `ACTIVE`.
//!
//! See `tests/firmware_node_selfreport.rs`.

use serde_json::{json, Value};

/// The build-varying facts a host cannot discover any other way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// Reported as `board`.
    pub name: &'static str,
    /// The Track 0 allow-list the on-MCU gate is seeded with, and the pins
    /// actually set to OUTPUT at boot. A host told otherwise addresses pins the
    /// node refuses, and never learns about the ones it accepts.
    pub output_pins: &'static [i32],
    /// I2C sensor bus as `(SDA, SCL)`, when this board has one free.
    pub i2c: Option<(i32, i32)>,
    /// Whether an I2S microphone is wirable and compiled in.
    pub has_mic: bool,
}

/// Seeed XIAO ESP32-S3 (Sense) — the default build.
///
/// `i2c` is the OV2640's SCCB pair, **not** the board's labelled I2C pads,
/// which are SDA=GPIO5 (silk D4) and SCL=GPIO6 (silk D5). GPIO6 is in
/// `output_pins` and is the pad marked SCL. Unresolved and needs a bench — see
/// the note on the Track 0 outputs in `main.rs`.
#[allow(dead_code)] // the board this build is not; the host harness asserts both
pub const XIAO_ESP32_S3: Board = Board {
    name: "seeed-xiao-esp32-s3",
    output_pins: &[21, 3, 6, 7, 8],
    i2c: Some((4, 5)),
    has_mic: true,
};

/// Waveshare ESP32-S3-Touch-LCD-2.1 (`--features board-waveshare-21`).
///
/// The round LCD consumes most GPIO; only the 12-pin header and the hardwired
/// I2C connector are exposed. No mic is wirable: GPIO0 is the DHT22 here and
/// GPIO1/2 are LCD lines.
#[allow(dead_code)] // the board this build is not; the host harness asserts both
pub const WAVESHARE_ESP32_S3_TOUCH_LCD_21: Board = Board {
    name: "waveshare-esp32-s3-touch-lcd-2.1",
    output_pins: &[43, 44],
    i2c: Some((15, 7)),
    has_mic: false,
};

/// The board this build targets. The only `#[cfg]` in this module.
#[cfg(not(feature = "board-waveshare-21"))]
pub const ACTIVE: Board = XIAO_ESP32_S3;
#[cfg(feature = "board-waveshare-21")]
pub const ACTIVE: Board = WAVESHARE_ESP32_S3_TOUCH_LCD_21;

/// The `capabilities` / `announce` reply.
///
/// `camera_on` is a parameter rather than a `cfg!` so that both answers are
/// reachable from a test. The camera owns the default board's I2C pins, so a
/// build with it reports no bus — null, rather than a bus that is not there.
pub fn describe(board: &Board, camera_on: bool, node_id: &str, firmware_version: &str) -> Value {
    let i2c: Option<[i32; 2]> = if camera_on {
        None
    } else {
        board.i2c.map(|(sda, scl)| [sda, scl])
    };
    json!({
        "node_id": node_id,
        "board": board.name,
        "firmware_version": firmware_version,
        "edge_agent": true,
        "tools": [
            {"name": "gpio_read", "description": "Read a GPIO pin value (0 or 1)."},
            {"name": "gpio_write", "description": "Set a GPIO pin high (1) or low (0)."},
            {"name": "camera_capture", "description": "Capture a JPEG image from the OV2640 camera."},
            {"name": "audio_sample", "description": "Sample audio from the I2S microphone."},
            {"name": "sensor_read", "description": "Read a value from an I2C/SPI sensor."},
            {"name": "set_reflex_rules", "description": "Push the on-MCU reflex (System 1) rule set."},
            {"name": "set_limits", "description": "Push the Track 0 actuator safety limits (allow-list, range, rate)."},
            {"name": "agent_chat", "description": "Chat with the on-device LLM agent."},
            {"name": "agent_config", "description": "Configure WiFi and LLM settings."},
            {"name": "agent_clear", "description": "Clear the agent conversation history."}
        ],
        "gpio": board.output_pins,
        "camera": camera_on,
        "microphone": board.has_mic,
        "i2c_bus": i2c,
        "transport": "usb-serial-jtag",
        "wifi": true
    })
}

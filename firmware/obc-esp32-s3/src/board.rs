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
/// `i2c` is the board's **labelled** bus: SDA=GPIO5 (silk D4), SCL=GPIO6 (silk
/// D5). A sensor wired to the pads marked SDA and SCL is on this bus, which is
/// the only arrangement anyone reading the silkscreen will produce.
///
/// It was `(4, 5)` until 2026-08-21 — silk D3 and D4, where D3 is not an I2C
/// pad at all and D4, the board's SDA, was driven as SCL. GPIO6 was
/// simultaneously in `output_pins`, so the pad marked SCL was a Track 0
/// actuator output, configured as an output at boot. An external sensor could
/// not work, and it failed as a silent stub read — indistinguishable from a
/// sensor that is not fitted.
///
/// The reason given for 4/5 was the OV2640's SCCB. That pin map belongs to the
/// Waveshare board (see `camera.rs`), not this one, and the sensor bus is
/// compiled out under `--features camera` regardless — so those pins were only
/// ever opened in the build where the camera is absent.
///
/// **Not verified on hardware.** Nothing in CI can build this crate, and the
/// change is a claim about a board rather than about code.
#[allow(dead_code)] // the board this build is not; the host harness asserts both
pub const XIAO_ESP32_S3: Board = Board {
    name: "seeed-xiao-esp32-s3",
    output_pins: &[21, 3, 7, 8],
    i2c: Some((5, 6)),
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

/// The `tools` array, pre-rendered.
///
/// It is the bulk of the reply and it never varies: ten objects, twenty string
/// fields, identical on every board and every build. Building it as a `Value`
/// and serialising it cost more stack than the node had (see `describe_json`).
/// As a `&'static str` it lives in flash and costs a memcpy.
const TOOLS_JSON: &str = concat!(
    r#"[{"name":"gpio_read","description":"Read a GPIO pin value (0 or 1)."},"#,
    r#"{"name":"gpio_write","description":"Set a GPIO pin high (1) or low (0)."},"#,
    r#"{"name":"camera_capture","description":"Capture a JPEG image from the OV2640 camera."},"#,
    r#"{"name":"audio_sample","description":"Sample audio from the I2S microphone."},"#,
    r#"{"name":"sensor_read","description":"Read a value from an I2C/SPI sensor."},"#,
    r#"{"name":"set_reflex_rules","description":"Push the on-MCU reflex (System 1) rule set."},"#,
    r#"{"name":"set_limits","description":"Push the Track 0 actuator safety limits (allow-list, range, rate)."},"#,
    r#"{"name":"agent_chat","description":"Chat with the on-device LLM agent."},"#,
    r#"{"name":"agent_config","description":"Configure WiFi and LLM settings."},"#,
    r#"{"name":"agent_clear","description":"Clear the agent conversation history."}]"#,
);

/// The `capabilities` / `announce` reply, formatted straight into a `String`.
///
/// This is what the firmware puts on the wire. `describe` below returns the same
/// document as a `serde_json::Value` and is what the tests assert against;
/// `describe_and_json_agree` in `tests/firmware_node_selfreport.rs` holds the two
/// together, so there is still one source of truth for what this node claims.
///
/// The split exists because of a measurement, 2026-08-22. Building the reply as
/// a `Value` and calling `to_string` used **4388 bytes of stack and left zero**:
///
///     capabilities: 1024 bytes, headroom 4388 -> 0 (used 4388)
///
/// It overflowed the main task on every call, printed the stack-overflow banner,
/// and rebooted — which is what made the node forget its pushed safety limits
/// mid-bench and look like a gate that ignored its own policy. A 1 KB document
/// was costing 4.4 KB of stack because `json!` builds a recursive tree and the
/// serialiser walks it with a formatter on top. Raising the stack was tried
/// first, twice, by picking a number; this removes the peak instead.
pub fn describe_json(
    board: &Board,
    camera_on: bool,
    node_id: &str,
    firmware_version: &str,
) -> String {
    use core::fmt::Write as _;

    let mut s = String::with_capacity(1200);
    // Every interpolated value is either a compile-time constant of ours or an
    // integer, so none of them needs escaping. `node_id` and `firmware_version`
    // are `const &str` in main.rs; if either ever becomes host-supplied this has
    // to go back through a real serialiser.
    let _ = write!(
        s,
        concat!(
            r#"{{"node_id":"{}","board":"{}","firmware_version":"{}","#,
            r#""edge_agent":true,"tools":{},"gpio":["#
        ),
        node_id, board.name, firmware_version, TOOLS_JSON
    );
    for (i, pin) in board.output_pins.iter().enumerate() {
        let _ = write!(s, "{}{}", if i > 0 { "," } else { "" }, pin);
    }
    let _ = write!(
        s,
        r#"],"camera":{},"microphone":{},"i2c_bus":"#,
        camera_on, board.has_mic
    );
    match if camera_on { None } else { board.i2c } {
        Some((sda, scl)) => {
            let _ = write!(s, "[{sda},{scl}]");
        }
        None => s.push_str("null"),
    }
    s.push_str(r#","transport":"usb-serial-jtag","wifi":true}"#);
    s
}

/// The `capabilities` / `announce` reply.
///
/// `camera_on` is a parameter rather than a `cfg!` so that both answers are
/// reachable from a test. The camera owns the default board's I2C pins, so a
/// build with it reports no bus — null, rather than a bus that is not there.
///
/// Not what goes on the wire any more — see `describe_json` — but still the
/// definition of the answer, and what the self-report tests read.
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

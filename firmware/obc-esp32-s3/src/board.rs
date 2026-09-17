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

/// LILYGO T-CameraPlus-S3, revision **V1.0/V1.1** (`--features board-lilygo-tcam-s3-v11`).
///
/// Added 2026-09-16 because the default below is the XIAO, and inheriting it here
/// would have been actively destructive. The XIAO's `output_pins` are
/// `[21, 3, 7, 8]`; on this board those are **SD_CS, camera RESET, camera XCLK
/// and camera D6**. Track 0 sets every `output_pins` entry to OUTPUT at boot, so
/// a Lilygo build on the default profile would drive its own sensor clock as an
/// actuator line before the camera ever initialised. The pin map being right in
/// `camera.rs` would not have saved it.
///
/// `output_pins` is **empty, and that is a `TODO(source)`, not a design.** Nobody
/// has checked which GPIOs this board exposes or leaves free: the vendor's
/// `pin_config.h` accounts for 1-17, 21 and 33-37, 45-48 across the camera, LCD,
/// SPI/SD, touch, PMIC and the IR-cut switch, and whether anything else is
/// broken out is a schematic question
/// (`project/T-CameraPlus-S3_V1.0-V1.1_20241109.pdf`). An empty allow-list means
/// Track 0 refuses every actuator write, which is the same as the boot policy
/// and is the safe direction to be wrong in. Fill it from the schematic, with
/// the pin numbers cited, before wiring anything to this node.
///
/// `i2c: None` — on V1.0/V1.1 the only exposed bus is GPIO1/2, which is the
/// camera's SCCB (shared with the CST816S touch controller at 0x15 and the
/// SY6970 PMIC at 0x6A). Handing that to the sensor driver would fight the
/// camera for the bus. V1.2 splits them onto 33/37; this profile is not V1.2.
///
/// `has_mic: false` — the firmware's I2S mic is GPIO0/1/2, and 1/2 are that same
/// SCCB bus. This board does have a PDM microphone, but on pins this firmware
/// has never been told about; `TODO(source)`.
///
/// Facts from `Xinyuan-LilyGO/T-CameraPlus-S3` (`pin_config.h` + README),
/// retrieved 2026-09-16. **No board of ours has been plugged in yet.**
#[allow(dead_code)] // the board this build is not; the host harness asserts all of them
pub const LILYGO_T_CAMERA_PLUS_S3_V11: Board = Board {
    name: "lilygo-t-camera-plus-s3-v1.1",
    output_pins: &[],
    i2c: None,
    has_mic: false,
};

/// The board this build targets. The only `#[cfg]` in this module.
///
/// The XIAO remains the default because it is what the fleet's nodes are and
/// what a plain `cargo build` should produce. Every *other* board must name
/// itself — and a camera build must name a board at all (see `camera.rs`), so the
/// dangerous combination, a camera board running the XIAO's pin policy, cannot
/// be reached silently.
#[cfg(feature = "board-waveshare-21")]
pub const ACTIVE: Board = WAVESHARE_ESP32_S3_TOUCH_LCD_21;
#[cfg(feature = "board-lilygo-tcam-s3-v11")]
pub const ACTIVE: Board = LILYGO_T_CAMERA_PLUS_S3_V11;
#[cfg(not(any(feature = "board-waveshare-21", feature = "board-lilygo-tcam-s3-v11")))]
pub const ACTIVE: Board = XIAO_ESP32_S3;

/// The `tools` array, pre-rendered.
///
/// It is the bulk of the reply and it never varies: eleven objects, twenty-two
/// string fields, identical on every board and every build. Building it as a `Value`
/// and serialising it cost more stack than the node had (see `describe_json`).
/// As a `&'static str` it lives in flash and costs a memcpy.
const TOOLS_JSON: &str = concat!(
    r#"[{"name":"gpio_read","description":"Read a GPIO pin value (0 or 1)."},"#,
    r#"{"name":"gpio_write","description":"Set a GPIO pin high (1) or low (0)."},"#,
    // Sensor-neutral since 2026-09-16: this string is shared by every board, and
    // the Lilygo T-CameraPlus-S3 has an OV5640 (`Camera PID=0x5640`, measured).
    // A node announcing "OV2640" to a host on that board is simply wrong.
    r#"{"name":"camera_capture","description":"Capture a JPEG image from the camera."},"#,
    // Added 2026-09-17. The node decides for itself whether something happened,
    // because G2 is a summary on the air and 228 bytes will not carry a picture.
    // The description says "reports" rather than "detects motion" on purpose: a
    // lamp switching and the camera being knocked are reportable states, not
    // detections, and the reply distinguishes them.
    r#"{"name":"camera_detect","description":"Compare this frame to the last one and report what changed."},"#,
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
    boot_id: u32,
) -> String {
    use core::fmt::Write as _;

    let mut s = String::with_capacity(1200);
    // Every interpolated value is either a compile-time constant of ours or an
    // integer, so none of them needs escaping. `node_id` and `firmware_version`
    // are `const &str` in main.rs; if either ever becomes host-supplied this has
    // to go back through a real serialiser.
    //
    // `boot_id`: the 2026-08-22 note on `boot_id()` said it "rides on every
    // set_limits reply and on capabilities". It rode on set_limits. Found on
    // 2026-09-13 when the host started listening for it: the supervisor's
    // recovery probe is `capabilities`, and its reply is the one place a reset
    // shows up when the boot announcement itself was lost to the air.
    let _ = write!(
        s,
        concat!(
            r#"{{"node_id":"{}","board":"{}","firmware_version":"{}","boot_id":{},"#,
            r#""edge_agent":true,"tools":{},"gpio":["#
        ),
        node_id, board.name, firmware_version, boot_id, TOOLS_JSON
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
pub fn describe(
    board: &Board,
    camera_on: bool,
    node_id: &str,
    firmware_version: &str,
    boot_id: u32,
) -> Value {
    let i2c: Option<[i32; 2]> = if camera_on {
        None
    } else {
        board.i2c.map(|(sda, scl)| [sda, scl])
    };
    json!({
        "node_id": node_id,
        "board": board.name,
        "firmware_version": firmware_version,
        "boot_id": boot_id,
        "edge_agent": true,
        "tools": [
            {"name": "gpio_read", "description": "Read a GPIO pin value (0 or 1)."},
            {"name": "gpio_write", "description": "Set a GPIO pin high (1) or low (0)."},
            {"name": "camera_capture", "description": "Capture a JPEG image from the camera."},
            {"name": "camera_detect", "description": "Compare this frame to the last one and report what changed."},
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

//! Host-side harness for the ESP32-S3 node's self-description.
//!
//! `firmware/obc-esp32-s3` builds for `xtensa-esp32s3-espidf`. Nothing in CI can
//! compile it, so until 2026-08-21 nothing checked what the node says about
//! itself — and what it said was wrong on every build but one. A
//! `board-waveshare-21` node announced the XIAO's name; the XIAO's output pins
//! while its own Track 0 gate allowed `[43, 44]`; an I2C bus it does not open;
//! and a microphone that is compiled out of that build. A `--features camera`
//! build reported no camera.
//!
//! `board.rs` has no ESP dependencies, so including the source here compiles it
//! for the host and asserts the real answers under the workspace's ordinary
//! `cargo test`. Same trick as `firmware_spine_framing.rs`, and for the same
//! reason: the path include keeps one copy of the source, so the firmware and
//! this harness cannot drift.
//!
//! The reason `board.rs` declares its variants as consts rather than `#[cfg]`
//! is this file. The firmware's features do not exist in this crate, so a
//! cfg-shaped self-report could only ever be observed in its default form — the
//! harness would confirm the one case that was already right and miss every
//! wrong one.

// `board.rs` selects `ACTIVE` with `#[cfg(feature = "board-waveshare-21")]` —
// a feature of the *firmware* crate, which this one does not have. Under the
// shim that cfg is simply false, so `ACTIVE` is the XIAO here; the tests assert
// both boards by name regardless, which is the entire reason the variants are
// consts. The lint is correctly reporting that this crate has no such feature,
// and that is intended rather than something to fix by inventing one.
#![allow(unexpected_cfgs)]

#[path = "../firmware/obc-esp32-s3/src/board.rs"]
mod board;

use board::{
    Board, LILYGO_T_CAMERA_PLUS_S3_V11 as LILYGO, WAVESHARE_ESP32_S3_TOUCH_LCD_21 as WAVESHARE,
    XIAO_ESP32_S3 as XIAO,
};

/// Every declared board profile. Enumerations below iterate THIS, not a literal
/// pair -- on 2026-09-16 a third profile (the Lilygo) was added and every loop
/// here still said `[&XIAO, &WAVESHARE]`, so the new board was covered by
/// nothing while the suite stayed green.
const ALL_BOARDS: &[&Board] = &[&XIAO, &WAVESHARE, &LILYGO];

fn describe(b: &Board, camera: bool) -> serde_json::Value {
    board::describe(b, camera, "obc-esp32-s3-001", "0.4.2", 0xC0FFEE)
}

/// The node stopped putting `describe`'s output on the wire on 2026-08-22.
///
/// Serialising it cost 4388 bytes of stack and left zero, so `capabilities`
/// overflowed the main task and rebooted the node on every call — which is what
/// silently discarded its pushed safety limits mid-bench and made a working gate
/// look like one that ignored its own policy. The firmware now formats the reply
/// directly with `describe_json`.
///
/// That leaves two renderings of the same claim, which is exactly the drift this
/// whole file exists to prevent. This test is the seam: whatever the node says on
/// the wire must parse to precisely what `describe` defines, for every board and
/// both camera settings. If someone adds a field to one and not the other, this
/// fails rather than a host quietly learning something untrue.
#[test]
fn the_wire_format_and_the_definition_are_the_same_document() {
    for board in ALL_BOARDS {
        let name = board.name;
        for camera in [false, true] {
            let wire = board::describe_json(board, camera, "obc-esp32-s3-001", "0.4.2", 0xC0FFEE);
            let parsed: serde_json::Value = serde_json::from_str(&wire).unwrap_or_else(|e| {
                panic!("{name} camera={camera}: describe_json emitted invalid JSON: {e}\n{wire}")
            });
            assert_eq!(
                parsed,
                describe(board, camera),
                "{name} camera={camera}: the wire format and the definition disagree"
            );
        }
    }
}

/// The reply has to fit the buffer it is written into and the stack it is built
/// on. 1024 bytes measured on hardware; the USB TX buffer is 4096.
#[test]
fn the_wire_format_stays_small() {
    let wire = board::describe_json(&XIAO, false, "obc-esp32-s3-001", "0.4.2", 0xC0FFEE);
    assert!(
        wire.len() < 2048,
        "capabilities grew to {} bytes; it is built on a main task with about \
         4 KB of headroom and travels through a 4096-byte USB buffer",
        wire.len()
    );
}

/// The bug this file exists for. Every one of these was the XIAO's value on a
/// Waveshare node, announced to a host with no other way to know better.
#[test]
fn a_waveshare_node_does_not_announce_itself_as_a_xiao() {
    let caps = describe(&WAVESHARE, false);
    assert_eq!(caps["board"], "waveshare-esp32-s3-touch-lcd-2.1");
    assert_eq!(caps["gpio"], serde_json::json!([43, 44]));
    assert_eq!(caps["i2c_bus"], serde_json::json!([15, 7]));
    assert_eq!(caps["microphone"], false);
}

/// The load-bearing one. A host uses `gpio` to decide which pins it may drive;
/// announcing the other board's list means every write is refused by the node's
/// own gate, and the two pins it *would* accept are invisible.
#[test]
fn the_announced_gpio_list_is_the_list_the_gate_is_seeded_with() {
    for b in ALL_BOARDS {
        for camera in [false, true] {
            let caps = describe(b, camera);
            let announced: Vec<i64> = caps["gpio"]
                .as_array()
                .expect("gpio must be an array")
                .iter()
                .map(|v| v.as_i64().expect("pins are integers"))
                .collect();
            let actual: Vec<i64> = b.output_pins.iter().map(|&p| p as i64).collect();
            assert_eq!(announced, actual, "board {} camera={camera}", b.name);
        }
    }
}

/// The camera owns the default board's I2C pins, so a build with it has no
/// sensor bus. Reporting a bus that is not there is the same failure as
/// reporting the wrong one.
#[test]
fn a_camera_build_reports_no_i2c_bus_rather_than_one_it_does_not_open() {
    assert_eq!(describe(&XIAO, true)["i2c_bus"], serde_json::Value::Null);
    assert_eq!(
        describe(&WAVESHARE, true)["i2c_bus"],
        serde_json::Value::Null
    );
    assert!(!describe(&XIAO, false)["i2c_bus"].is_null());
}

/// `camera_on` is a parameter precisely so both answers are reachable. It was a
/// hardcoded `false`, which a `--features camera` build also reported.
#[test]
fn the_camera_flag_tracks_the_build_rather_than_a_constant() {
    assert_eq!(describe(&XIAO, true)["camera"], true);
    assert_eq!(describe(&XIAO, false)["camera"], false);
}

/// No two boards may agree on the fields a host uses to tell them apart — if
/// they do, the report cannot do the job it exists for.
#[test]
fn the_two_boards_are_distinguishable_by_their_report() {
    let a = describe(&XIAO, false);
    let b = describe(&WAVESHARE, false);
    for field in ["board", "gpio", "i2c_bus", "microphone"] {
        assert_ne!(
            a[field], b[field],
            "field `{field}` cannot tell the boards apart"
        );
    }
}

/// The XIAO's bus is the one its silkscreen marks, and its Track 0 outputs do
/// not overlap it. Both halves of that changed on 2026-08-21 and both are
/// asserted here, because the failure they replace was silent: a sensor on the
/// labelled pads simply read as a stub, and pin 6 — the pad marked SCL — was an
/// actuator output.
#[test]
fn the_xiao_bus_is_the_labelled_one_and_no_output_pin_sits_on_it() {
    assert_eq!(
        XIAO.i2c,
        Some((5, 6)),
        "SDA=GPIO5 (silk D4), SCL=GPIO6 (silk D5)"
    );
    for b in ALL_BOARDS {
        // Not every board has a free sensor bus. The Lilygo T-CameraPlus-S3
        // V1.0/V1.1 declares `None` because its only exposed bus, GPIO1/2, IS the
        // camera's SCCB (shared with the CST816S touch controller and the SY6970
        // PMIC). This used to `expect()` a bus on every board, which was true of
        // exactly the two boards that existed when it was written.
        let Some((sda, scl)) = b.i2c else { continue };
        for &pin in b.output_pins {
            assert_ne!(pin, sda, "{}: output pin {pin} is the bus SDA", b.name);
            assert_ne!(pin, scl, "{}: output pin {pin} is the bus SCL", b.name);
        }
    }
}

/// A board that declares no sensor bus must not then announce one.
///
/// The counterpart to the check above: `i2c: None` is a real answer, and the
/// wire format has to carry it as `null` rather than as an invented pair. A host
/// told a bus exists will wire a sensor to it.
#[test]
fn a_board_with_no_bus_announces_null_rather_than_inventing_one() {
    for b in ALL_BOARDS {
        if b.i2c.is_some() {
            continue;
        }
        for camera in [false, true] {
            assert_eq!(
                describe(b, camera)["i2c_bus"],
                serde_json::Value::Null,
                "{} declares no bus but announced one (camera={camera})",
                b.name
            );
        }
    }
}

/// No board may put a Track 0 actuator pin on its own camera's wiring.
///
/// This is the invariant that nearly shipped broken on 2026-09-16. `board::ACTIVE`
/// defaulted to the XIAO for any build that was not the Waveshare, so the first
/// Lilygo build would have inherited `output_pins: [21, 3, 7, 8]` — on that board
/// SD_CS, camera RESET, camera **XCLK** and camera D6. Track 0 sets every entry to
/// OUTPUT at boot, so the node would have driven its own sensor clock as an
/// actuator line before the camera initialised, and a correct pin map in
/// `camera.rs` would not have saved it.
///
/// It was caught by reading a vendor pin table, which is not a mechanism. This is.
///
/// **Partial by construction, and that is the point to fix next.** The camera pin
/// maps live in `camera.rs` behind `#[cfg(feature = ...)]`, so a host test cannot
/// see them; the numbers below are transcribed. Transcription is exactly what
/// this repo keeps getting wrong. The real fix is to move the pin maps into a
/// pure `camera_map.rs` the way `identity_map.rs` and `sensor_math.rs` are split,
/// so this test reads the shipped table instead of a copy of it.
#[test]
fn no_board_drives_its_own_camera_pins_as_actuators() {
    // Transcribed from camera.rs 2026-09-16. See the caveat above.
    let camera_pins: &[(&str, &[i32])] = &[
        // LILYGO T-CameraPlus-S3 V1.0/V1.1: xclk 7, pclk 10, vsync 4, href 5,
        // sccb 1/2, reset 3, data 12/14/15/13/11/9/8/6.
        (
            "lilygo-t-camera-plus-s3-v1.1",
            &[7, 10, 4, 5, 1, 2, 3, 12, 14, 15, 13, 11, 9, 8, 6],
        ),
        // Seeed XIAO ESP32S3 Sense: xclk 10, pclk 13, vsync 38, href 47,
        // sccb 40/39, data 15/17/18/16/14/12/11/48.
        (
            "seeed-xiao-esp32-s3",
            &[10, 13, 38, 47, 40, 39, 15, 17, 18, 16, 14, 12, 11, 48],
        ),
    ];

    for (name, cam) in camera_pins {
        let board = ALL_BOARDS
            .iter()
            .find(|b| b.name == *name)
            .unwrap_or_else(|| panic!("no board profile named {name}"));
        for &pin in board.output_pins {
            assert!(
                !cam.contains(&pin),
                "{name}: Track 0 output pin {pin} is one of this board's camera \
                 pins. Track 0 configures it as an output at boot, before the \
                 camera initialises."
            );
        }
        if let Some((sda, scl)) = board.i2c {
            for p in [sda, scl] {
                assert!(
                    !cam.contains(&p),
                    "{name}: sensor bus pin {p} is a camera pin; the two drivers \
                     would fight for it."
                );
            }
        }
    }
}

/// Whatever build this harness is compiled under, `ACTIVE` must be one of the
/// boards declared here rather than a third thing assembled by `#[cfg]`.
#[test]
fn the_active_board_is_one_of_the_declared_ones() {
    assert!(
        ALL_BOARDS.iter().any(|b| **b == board::ACTIVE),
        "ACTIVE is a board this harness does not know about: {}",
        board::ACTIVE.name
    );
}

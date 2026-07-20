//! Host-side harness for the Heltec bridge's spine module.
//!
//! `firmware/heltec-lora-linktest` builds for `xtensa-esp32s3-espidf`, so `cargo
//! test` inside that crate compiles its tests for the MCU and cannot run them.
//! The `#[cfg(test)]` block in `spine.rs` was therefore decorative — written,
//! never executed.
//!
//! `spine.rs` has no ESP dependencies (it is `Vec` and integers), so including
//! the source here compiles it for the host and runs its tests for real, under
//! the workspace's ordinary `cargo test`. The path include keeps one copy of the
//! source: the firmware and this harness cannot drift.
//!
//! What this actually protects: the frame codec, the relay de-duplication ring,
//! and — the reason it exists — the line framer that turns a byte stream into
//! commands. On 2026-07-19 the bridge transmitted two mid-string fragments after
//! the host wrote two commands back to back, because each origin framed lines by
//! hand and neither could survive a burst. Both now share `LineFramer`, and the
//! burst case is asserted below via the module's own tests.

#[path = "../firmware/heltec-lora-linktest/src/spine.rs"]
mod spine;

use spine::{Framed, LineFramer, MAX_PAYLOAD};

/// Belt and braces: the in-module tests cover framing semantics, so this one
/// asserts the property the incident was actually about — bytes arriving as a
/// single undifferentiated burst must yield exactly the commands that were sent,
/// byte for byte, no matter where the read boundaries fall.
#[test]
fn a_burst_split_at_every_possible_boundary_still_yields_both_commands() {
    let a = br#"{"args":{},"cmd":"capabilities","to":"obc-esp32-s3-001"}"#;
    let b = br#"{"args":{"pin":99,"value":1},"cmd":"gpio_write","to":"obc-esp32-s3-001"}"#;

    let mut burst = Vec::new();
    burst.extend_from_slice(a);
    burst.push(b'\n');
    burst.extend_from_slice(b);
    burst.push(b'\n');

    // Every split point stands in for a different read boundary. The framer must
    // be indifferent to all of them.
    for split in 0..=burst.len() {
        let mut framer = LineFramer::new();
        let mut lines: Vec<Vec<u8>> = Vec::new();
        for chunk in [&burst[..split], &burst[split..]] {
            for &byte in chunk {
                match framer.push(byte) {
                    Framed::Line(l) => lines.push(l.to_vec()),
                    Framed::Overflow => panic!("unexpected overflow at split {split}"),
                    Framed::Pending => {}
                }
            }
        }
        assert_eq!(lines.len(), 2, "split at {split} lost a command");
        assert_eq!(lines[0], a, "split at {split} corrupted the first command");
        assert_eq!(lines[1], b, "split at {split} corrupted the second command");
    }
}

/// The budget is a property of the wire format, not of this harness. If someone
/// raises `MAX_PAYLOAD` past what a single SX1262 frame can carry, the framer
/// will happily accept lines the radio cannot send.
#[test]
fn the_payload_budget_still_fits_one_radio_frame() {
    assert!(
        MAX_PAYLOAD + spine::HEADER <= 255,
        "MAX_PAYLOAD + header must fit an SX1262 frame; got {}",
        MAX_PAYLOAD + spine::HEADER
    );
}

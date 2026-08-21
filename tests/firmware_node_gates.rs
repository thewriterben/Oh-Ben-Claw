//! Host-side harness for the ESP32-S3 node's on-MCU gates.
//!
//! `firmware/obc-esp32-s3` builds for `xtensa-esp32s3-espidf`, so `cargo test`
//! inside that crate compiles its tests for the MCU and cannot run them. Three
//! of its modules carry `#[cfg(test)]` blocks — twenty `#[test]` functions
//! between them — and not one had ever executed:
//!
//! | module      | tests | what it decides                                  |
//! |-------------|-------|--------------------------------------------------|
//! | `safety.rs` | 6     | the on-MCU Track 0 gate: pin, range, rate         |
//! | `reflex.rs` | 7     | System 1 rule evaluation, debounce, fire-on-change |
//! | `safing.rs` | 7     | built-in battery self-protection                  |
//!
//! `safety.rs` is the load-bearing one. It is the copy of the limit table the
//! node holds so that a compromised or absent host cannot talk it round, and
//! six tests assert it refuses correctly. They were written, checked in, and
//! never run.
//!
//! None of the three touches esp-idf — they are `serde`, `HashMap` and
//! integers — so including the sources here compiles them for the host and runs
//! their tests for real under the workspace's ordinary `cargo test`. Same trick
//! and same reasoning as `firmware_spine_framing.rs`, which exists because the
//! Heltec firmware had the identical problem: *"the `#[cfg(test)]` block in
//! `spine.rs` was therefore decorative — written, never executed."* That fix
//! was applied to one firmware and not the other.
//!
//! The path include keeps one copy of each source, so the firmware and this
//! harness cannot drift.
//!
//! `safing.rs` refers to `crate::reflex`, which is why both are declared here:
//! in an integration test the file *is* the crate root, so `crate::reflex`
//! resolves to the module below.

// `allow(dead_code)`: compiled in isolation these modules expose accessors that
// only `main.rs` calls (`SafetyGate::policy`, `PowerMode::as_str`). Unused here
// is not unused in the firmware, and the warning would be noise that CI turns
// into an error.
#[path = "../firmware/obc-esp32-s3/src/reflex.rs"]
#[allow(dead_code)]
mod reflex;

#[path = "../firmware/obc-esp32-s3/src/safety.rs"]
#[allow(dead_code)]
mod safety;

#[path = "../firmware/obc-esp32-s3/src/safing.rs"]
#[allow(dead_code)]
mod safing;

use safety::SafetyGate;

/// The property the whole safety case rests on, asserted here rather than only
/// inside the module: a gate seeded with an allow-list refuses a pin outside
/// it. `bodies/benchtop` in OBC-Prime allows pins 3 and 7; the bench procedure
/// asks for an unlisted pin and expects the wire not to move.
#[test]
fn a_pin_outside_the_boot_allow_list_is_refused() {
    let mut gate = SafetyGate::with_output_pins(&[3, 7]);
    assert!(gate.check(3, 1, 0).is_ok(), "3 is in the list");
    assert!(gate.check(7, 0, 0).is_ok(), "7 is in the list");
    assert!(gate.check(8, 1, 0).is_err(), "8 is not in the list");
    assert!(
        gate.check(6, 1, 0).is_err(),
        "6 left the list on 2026-08-21"
    );
}

/// A gate with an empty allow-list refuses everything rather than allowing it.
/// Default-deny is the claim; this is the case where a fail-open bug would hide.
#[test]
fn an_empty_allow_list_refuses_rather_than_permits() {
    let mut gate = SafetyGate::with_output_pins(&[]);
    for pin in 0..16 {
        assert!(
            gate.check(pin, 1, 0).is_err(),
            "pin {pin} was allowed by a gate that lists nothing"
        );
    }
}

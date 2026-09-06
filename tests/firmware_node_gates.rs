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

/// The boot posture, chosen 2026-08-22: a node that has heard from no host
/// drives nothing.
///
/// This is the fix for the fail-open the bench found. The node used to boot into
/// `with_output_pins(OUTPUT_PINS)` — `[21,3,7,8]`, no rate limit — which is
/// *wider* than any policy a host pushes, and every reset silently restored it.
/// `scripts/reboot_amnesia.py` reproduces the old behaviour against hardware.
#[test]
fn a_node_that_has_heard_from_no_host_drives_nothing() {
    let mut gate = SafetyGate::deny_until_told();
    for pin in [3i64, 7, 8, 21, 43, 99] {
        assert!(
            gate.check(pin, 1, 0).is_err(),
            "pin {pin} was allowed by a node that has been told nothing"
        );
        assert!(gate.check(pin, 0, 0).is_err(), "including writing it low");
    }
}

/// Deny-all is a starting posture, not a lock: the host can still open it, and
/// what it opens is exactly what it asked for and nothing more.
#[test]
fn a_host_can_open_the_deny_all_gate_but_only_as_far_as_it_asked() {
    let mut gate = SafetyGate::deny_until_told();
    assert!(gate.check(3, 1, 0).is_err(), "closed before the push");

    let pushed: Vec<safety::SafetyLimit> = serde_json::from_str(
        r#"[{"node_id":"obc-esp32-s3-001","tool":"gpio_write",
             "allowed_pins":[3,7],"value_min":0,"value_max":1,
             "min_interval_ms":500}]"#,
    )
    .unwrap();
    assert!(gate.apply_pushed(pushed, "obc-esp32-s3-001"));

    assert!(gate.check(3, 1, 1_000).is_ok(), "3 was asked for");
    assert!(gate.check(7, 1, 1_000).is_ok(), "7 was asked for");
    assert!(gate.check(8, 1, 1_000).is_err(), "8 was not");
    assert!(gate.check(21, 1, 1_000).is_err(), "nor 21, which the old boot policy allowed");
}

/// The distinction the whole posture rests on: an *absent* allow-list permits
/// everything, an *empty* one permits nothing. If `deny_until_told` ever ends up
/// building `None` here, the gate silently becomes wide open and every other
/// test in this file still passes.
#[test]
fn an_absent_allow_list_and_an_empty_one_are_not_the_same_thing() {
    let mut wide: safety::SafetyLimit = serde_json::from_str(
        r#"{"node_id":"","tool":"gpio_write","value_min":0,"value_max":1}"#,
    )
    .unwrap();
    assert!(
        wide.allowed_pins.is_none(),
        "a limit with no allowed_pins field parses as None"
    );
    wide.min_interval_ms = None;

    let mut gate = SafetyGate::deny_until_told();
    assert!(gate.check(99, 1, 0).is_err(), "empty list denies");

    gate.apply_pushed(vec![wide], "obc-esp32-s3-001");
    assert!(
        gate.check(99, 1, 0).is_ok(),
        "a pushed limit with NO allowed_pins permits any pin -- which is why the \
         boot posture must build Some(empty) and never None"
    );
}

/// The exact sequence the bench performed on 2026-08-22, replayed on the host
/// from the literal JSON `scripts/bench_run.py` puts on the wire.
///
/// On the board that sequence failed: `set_limits` returned `applied:true` and
/// echoed `allowed_pins [3,7]`, and the node then accepted a write to pin 8,
/// accepted it again with no host attached, and accepted two writes to pin 3
/// inside the 500 ms interval. Three of the gate's rules did not fire.
///
/// This test exists to divide that failure in half. If it passes, the sources
/// in `firmware/obc-esp32-s3/src` are not the thing that is wrong, and the
/// variable is what is actually running on the board — which is a different
/// investigation from "the gate logic is broken", and the two were being
/// conflated. If it fails, the bug is here and this is where it gets fixed.
///
/// The limit is parsed rather than constructed so the deserialisation is under
/// test too: a field name that does not match, or an `allowed_pins` that lands
/// as `None`, would produce exactly the observed behaviour — a policy that
/// reports itself correctly and constrains nothing.
#[test]
fn the_bench_limit_table_refuses_what_the_bench_asked_it_to_refuse() {
    // Verbatim from LIMITS in scripts/bench_run.py.
    let pushed: Vec<safety::SafetyLimit> = serde_json::from_str(
        r#"[{
            "node_id": "obc-esp32-s3-001",
            "tool": "gpio_write",
            "allowed_pins": [3, 7],
            "value_min": 0,
            "value_max": 1,
            "min_interval_ms": 500
        }]"#,
    )
    .expect("the bench's limit table must deserialise into SafetyLimit");

    let mut gate = SafetyGate::with_output_pins(&[21, 3, 7, 8]);
    assert!(
        gate.check(8, 1, 0).is_ok(),
        "pin 8 is in OUTPUT_PINS, so the BOOT policy allows it -- that is what \
         makes it the honest refusal pin once the table is pushed"
    );

    let applied = gate.apply_pushed(pushed, "obc-esp32-s3-001");
    assert!(applied, "set_limits reported applied:true on the board");

    let policy = gate.policy();
    assert_eq!(
        policy.allowed_pins.as_deref(),
        Some(&[3i64, 7][..]),
        "the gate must hold the list it just reported"
    );
    assert_eq!(policy.min_interval_ms, Some(500));

    // §1b and §1c: the pin outside the pushed list.
    assert!(
        gate.check(8, 1, 1_000).is_err(),
        "pin 8 is not in allowed_pins [3,7]; the board accepted this write"
    );
    // A pin in no list at all.
    assert!(gate.check(99, 1, 1_000).is_err());
    // Value outside 0..=1.
    assert!(gate.check(3, 5, 1_000).is_err());

    // §1d: two writes to an allowed pin inside min_interval_ms.
    assert!(gate.check(3, 1, 2_000).is_ok(), "first write, interval clear");
    assert!(
        gate.check(3, 0, 2_100).is_err(),
        "second write 100 ms later must be rate-limited; the board allowed it"
    );
    assert!(
        gate.check(3, 0, 2_600).is_ok(),
        "and allowed again once the interval has elapsed"
    );
}

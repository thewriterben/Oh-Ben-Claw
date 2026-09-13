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

// Host-pushed rules across a reboot (2026-09-13): the record, its version
// guard, and the boot verdict. Same arrangement — pure module, tests run here.
#[path = "../firmware/obc-esp32-s3/src/rules_store.rs"]
#[allow(dead_code)]
mod rules_store;

use safety::SafetyGate;

/// The node's slot count and the host's are the same number in two workspaces
/// that cannot link. `NodeCommand::descend` refuses by the host's; the node
/// refuses by its own; they must agree or a message one accepts, the other
/// drops.
#[test]
fn the_host_and_the_node_agree_on_how_many_modulation_slots_there_are() {
    assert_eq!(reflex::MAX_SLOTS, obc_reflex::MAX_SLOTS);
}

/// The node's beacon cadence lives as a local `const` inside the firmware's
/// main loop, so the host's copy (`obc_spine::NODE_BEACON_INTERVAL_MS`) cannot
/// be imported — it is pinned to the *source text* instead. The supervisor's
/// default staleness is a multiple of it; on 2026-09-13 a deployed
/// `stale_ms` of one beacon interval flapped the node every 2–5 minutes.
#[test]
fn the_hosts_copy_of_the_node_beacon_interval_is_what_the_firmware_says() {
    let main_rs = include_str!("../firmware/obc-esp32-s3/src/main.rs");
    let line = main_rs
        .lines()
        .find(|l| {
            l.trim_start()
                .starts_with("const BEACON_INTERVAL_MS: u64 = ")
        })
        .expect("firmware main.rs declares BEACON_INTERVAL_MS");
    let value: u64 = line
        .split('=')
        .nth(1)
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .replace('_', "")
        .parse()
        .unwrap();
    assert_eq!(value, obc_spine::NODE_BEACON_INTERVAL_MS, "{line}");
    assert!(
        obc_spine::MeshSupervisorConfig::default().stale_ms >= 3 * value,
        "the default staleness must be at least three beacons, or a late beacon is an outage"
    );
}

/// The host's `SensorSlot` wire form is exactly what the node deserializes —
/// the pair of tests in each module pins the JSON, this pins them to each other.
#[test]
fn a_slot_rule_the_host_emits_is_the_rule_the_node_loads() {
    let host = obc_reflex::Condition::SensorSlot {
        entity: "sensor.temp".into(),
        op: obc_reflex::Cmp::Gt,
        slot: 3,
        min: 20.0,
        max: 60.0,
        default: 0.5,
    };
    let json = serde_json::to_string(&host).unwrap();
    let node: reflex::Condition = serde_json::from_str(&json).unwrap();
    assert_eq!(
        node,
        reflex::Condition::SensorSlot {
            entity: "sensor.temp".into(),
            op: reflex::Cmp::Gt,
            slot: 3,
            min: 20.0,
            max: 60.0,
            default: 0.5,
        }
    );
}

/// The first real slot-bound rule (walkthrough §A5f, `config.example.toml`):
/// the die temperature drives the onboard LED through a threshold the brain
/// slides. Two rules on one slot, as the host writes them, loaded by the
/// node's engine and evaluated on the node's own snapshot. The bench proves
/// the LED; this pins the arithmetic the bench relies on — which rule fires
/// at which level around a given reading — so a slot-range edit cannot
/// quietly turn the light inside out.
#[test]
fn the_die_temperature_rules_fire_the_way_the_bench_expects() {
    let rule = |id: &str, op: obc_reflex::Cmp, value: i64| obc_reflex::ReflexRule {
        id: id.into(),
        when: obc_reflex::Condition::SensorSlot {
            entity: "sensor.die_temperature".into(),
            op,
            slot: 0,
            min: 30.0,
            max: 70.0,
            default: 0.5,
        },
        then: obc_reflex::Action::GpioWrite {
            node_id: "obc-esp32-s3-001".into(),
            pin: 21,
            value,
        },
        debounce_ms: 10_000,
        max_rate_hz: None,
        // Edge-triggered on the node since 2026-09-13: one report per
        // transition, not one per debounce interval while holding.
        fire_on_change: true,
        hold_ms: 0,
    };
    let host_rules = vec![
        rule("die-hot", obc_reflex::Cmp::Gt, 0),
        rule("die-cool", obc_reflex::Cmp::Le, 1),
    ];
    // Across the wire exactly as `set_reflex_rules` carries them.
    let json = serde_json::to_string(&host_rules).unwrap();
    let node_rules: Vec<reflex::ReflexRule> = serde_json::from_str(&json).unwrap();
    let mut engine = reflex::ReflexEngine::default();
    engine
        .set_rules(node_rules)
        .expect("both rules bind slot 0, which exists");

    let snapshot = |t: f64| {
        let mut s = std::collections::HashMap::new();
        s.insert("sensor.die_temperature".to_string(), t);
        s
    };
    let fired = |engine: &mut reflex::ReflexEngine, t: f64, now: u64| -> Vec<(String, i64)> {
        engine
            .evaluate(&snapshot(t), now)
            .into_iter()
            .map(|f| match f.action {
                reflex::Action::GpioWrite { pin, value, .. } => {
                    assert_eq!(pin, 21);
                    (f.rule_id, value)
                }
                other => panic!("unexpected action {other:?}"),
            })
            .collect()
    };

    // The bench reading was 38.3 °C. Default level 0.5 → 50 °C: cool, LED off.
    assert_eq!(
        fired(&mut engine, 38.3, 1_000),
        vec![("die-cool".to_string(), 1)]
    );
    // Holding at 50 °C for a minute: not one more report (this is the edge
    // triggering; before it, one every 10 s).
    for t in 2..60u64 {
        assert!(
            fired(&mut engine, 38.3, t * 1_000).is_empty(),
            "re-fired at {t}s"
        );
    }
    // Threshold slid below the reading (level for 32 °C): hot, LED on.
    engine.descend(&[(0, 0.05)]).unwrap();
    assert_eq!(
        fired(&mut engine, 38.3, 70_000),
        vec![("die-hot".to_string(), 0)]
    );
    // And above it again (44 °C): off. Only ever one rule at a time.
    engine.descend(&[(0, 0.35)]).unwrap();
    assert_eq!(
        fired(&mut engine, 38.3, 90_000),
        vec![("die-cool".to_string(), 1)]
    );
    // Without the entity in the snapshot — sensor not running — nothing fires:
    // an absent reading is not a cool reading.
    assert!(engine
        .evaluate(&std::collections::HashMap::new(), 60_000)
        .is_empty());
}

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
    assert!(
        gate.check(21, 1, 1_000).is_err(),
        "nor 21, which the old boot policy allowed"
    );
}

/// The distinction the whole posture rests on: an *absent* allow-list permits
/// everything, an *empty* one permits nothing. If `deny_until_told` ever ends up
/// building `None` here, the gate silently becomes wide open and every other
/// test in this file still passes.
#[test]
fn an_absent_allow_list_and_an_empty_one_are_not_the_same_thing() {
    let mut wide: safety::SafetyLimit =
        serde_json::from_str(r#"{"node_id":"","tool":"gpio_write","value_min":0,"value_max":1}"#)
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
    assert!(
        gate.check(3, 1, 2_000).is_ok(),
        "first write, interval clear"
    );
    assert!(
        gate.check(3, 0, 2_100).is_err(),
        "second write 100 ms later must be rate-limited; the board allowed it"
    );
    assert!(
        gate.check(3, 0, 2_600).is_ok(),
        "and allowed again once the interval has elapsed"
    );
}

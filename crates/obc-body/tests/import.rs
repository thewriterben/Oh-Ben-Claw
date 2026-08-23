//! What the importer must never do, mostly.
//!
//! The one test to read first is
//! `no_angle_the_gate_admits_can_exceed_the_declared_travel`. Everything else
//! guards a way the conversion could quietly produce a limit that looks like a
//! bound and is not one.

use clawbot::{
    CableRun, Channel, ChannelId, Degrees, Harness, Joint, JointLimits, JointType, Link,
    Millimetres, Mimic, Power, Radians, Robot, Transform, Vec3,
};
use obc_body::{safety_limits, servo_command};
use obc_movement::MovementCommand;

const ORIGIN: Transform = Transform {
    xyz_mm: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
    rpy_rad: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
};

const Z: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 1.0 };
const QUARTER: f64 = core::f64::consts::FRAC_PI_2;

const fn limits(lower: f64, upper: f64) -> JointLimits {
    JointLimits {
        lower: Some(Radians(lower)),
        upper: Some(Radians(upper)),
        lower_mm: None,
        upper_mm: None,
        effort_nm: None,
        velocity_rad_s: None,
    }
}

const fn joint(id: &'static str, lim: Option<JointLimits>) -> Joint {
    Joint {
        id,
        kind: JointType::Revolute,
        parent: "base",
        child: "arm",
        origin: ORIGIN,
        axis: Some(Z),
        limits: lim,
        mimic: None,
        actuator_id: None,
        gear_ratio: None,
    }
}

const LINKS: &[Link] = &[Link {
    id: "base",
    part_id: Some("mechanical/x"),
    provenance_sha256: None,
    make_size_mm: None,
    make_material: None,
    mass_g: None,
}];

const fn robot(joints: &'static [Joint]) -> Robot {
    Robot {
        id: "arm",
        kind: "arm",
        make: None,
        model: None,
        base_link: "base",
        links: LINKS,
        joints,
    }
}

const fn channel(joint_id: &'static str, id: Option<ChannelId>, inverted: bool,
                 offset: Option<Radians>) -> Channel {
    Channel { joint_id, channel: id, bus: None, bus_address: None, inverted,
              zero_offset: offset }
}

const NO_POWER: Power = Power {
    supply_volts: None,
    supply_current_a: None,
    shared_with_logic: None,
};

const fn harness(channels: &'static [Channel], routing: &'static [CableRun]) -> Harness {
    Harness {
        id: "loom",
        robot_id: "arm",
        controller_part_id: None,
        channels,
        routing,
        power: NO_POWER,
    }
}

// --------------------------------------------------------- the property that matters

#[test]
fn no_angle_the_gate_admits_can_exceed_the_declared_travel() {
    // The gate admits a command when degrees.round() is within [min, max], so an
    // integer max of 90 admits anything under 90.5. The bounds must absorb that,
    // not sit inside it.
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.is_complete(), "{:?}", report.unbounded);

    let converted = &report.converted[0];
    let (lo, hi) = converted.actuator_degrees;
    let (min, max) = converted.enforced;

    // Every value the gate lets through, at its widest interpretation.
    assert!(
        max as f64 + 0.5 <= hi + 1e-9,
        "gate admits up to {}°, declared travel ends at {}°",
        max as f64 + 0.5,
        hi
    );
    assert!(
        min as f64 - 0.5 >= lo - 1e-9,
        "gate admits down to {}°, declared travel starts at {}°",
        min as f64 - 0.5,
        lo
    );

    // And the cost of that guarantee is reported rather than absorbed.
    assert!(converted.travel_given_up_deg > 0.0);
    assert!(converted.travel_given_up_deg < 2.0, "under a degree per bound");
}

#[test]
fn a_ninety_degree_joint_enforces_eighty_nine() {
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert_eq!(report.converted[0].enforced, (-89, 89));
    // The gate admits [-89.5, 89.5]; the joint declares [-90, 90]. Half a degree
    // lost at each end, measured against what the gate ADMITS rather than against
    // the integers it stores.
    assert!((report.converted[0].travel_given_up_deg - 1.0).abs() < 1e-9);

    let limit = &report.limits[0];
    assert_eq!(limit.tool, "servo_angle");
    assert_eq!(limit.allowed_pins, Some(vec![3]));
    assert_eq!(limit.value_min, Some(-89));
    assert_eq!(limit.value_max, Some(89));
    assert_eq!(limit.min_interval_ms, None, "velocity is not a command rate");
}

// ------------------------------------------------------------- refusals, not defaults

#[test]
fn a_joint_with_unknown_limits_gets_no_limit_at_all() {
    // The failure this crate exists to prevent: a permissive SafetyLimit that
    // looks like a bound and is none.
    const J: &[Joint] = &[joint("shoulder", None)];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.limits.is_empty(), "no limit may be invented");
    assert!(!report.is_complete());
    assert_eq!(report.unbounded[0].joint_id, "shoulder");
    assert!(report.unbounded[0].reason.contains("never unlimited"));
}

#[test]
fn a_mimicking_joint_with_a_channel_is_reported_as_a_wiring_error() {
    const J: &[Joint] = &[Joint {
        mimic: Some(Mimic { joint: "shoulder", multiplier: -1.0, offset: 0.0 }),
        ..joint("right-jaw", Some(limits(-QUARTER, QUARTER)))
    }];
    const C: &[Channel] = &[channel("right-jaw", Some(ChannelId::Number(4)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.limits.is_empty());
    assert!(report.unbounded[0].reason.contains("not independently commandable"));
}

#[test]
fn a_named_channel_cannot_become_an_integer_pin() {
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] =
        &[channel("shoulder", Some(ChannelId::Name("AX-12/ID3")), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.limits.is_empty());
    assert!(report.unbounded[0].reason.contains("integer pins"));
}

#[test]
fn a_prismatic_joint_is_not_an_angle() {
    const J: &[Joint] = &[Joint {
        kind: JointType::Prismatic,
        limits: Some(JointLimits {
            lower: None,
            upper: None,
            lower_mm: Some(Millimetres(0.0)),
            upper_mm: Some(Millimetres(50.0)),
            effort_nm: None,
            velocity_rad_s: None,
        }),
        ..joint("slide", None)
    }];
    const C: &[Channel] = &[channel("slide", Some(ChannelId::Number(5)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.limits.is_empty());
    assert!(report.unbounded[0].reason.contains("prismatic"));
}

#[test]
fn exactly_one_degree_of_travel_bounds_tightly_rather_than_being_refused() {
    // ±0.5°. The absorbed rounding lands on [0, 0], which admits degrees in
    // (-0.5, 0.5) — precisely the declared travel and nothing more. Worth
    // asserting because it is the boundary case of the arithmetic, and getting
    // it wrong in the other direction would refuse a joint that is fine.
    const HALF_DEG: f64 = 0.008_726_646_259_971_648;
    const J: &[Joint] = &[joint("wrist", Some(limits(-HALF_DEG, HALF_DEG)))];
    const C: &[Channel] = &[channel("wrist", Some(ChannelId::Number(6)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.is_complete(), "{:?}", report.unbounded);
    assert_eq!(report.converted[0].enforced, (0, 0));
    assert!(report.converted[0].travel_given_up_deg.abs() < 1e-9, "nothing given up");
}

#[test]
fn a_range_too_narrow_to_bound_yields_no_limit_rather_than_an_inverted_one() {
    // ±0.25°, half a degree of travel. Absorbing the gate's rounding puts the
    // computed minimum above the computed maximum, and an inverted range would
    // deny everything while looking like a permission.
    const QUARTER_DEG: f64 = 0.004_363_323_129_985_824;
    const J: &[Joint] = &[joint("wrist", Some(limits(-QUARTER_DEG, QUARTER_DEG)))];
    const C: &[Channel] = &[channel("wrist", Some(ChannelId::Number(6)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert!(report.limits.is_empty());
    assert!(report.unbounded[0].reason.contains("too narrow"));
}

#[test]
fn a_harness_for_a_different_robot_imports_nothing() {
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];
    const OTHER: Harness = Harness { robot_id: "some-other-arm", ..harness(C, &[]) };

    let report = safety_limits("node-1", &robot(J), &OTHER);
    assert!(report.limits.is_empty());
    assert!(report.notes[0].contains("nothing imported"));
}

// ----------------------------------------------------------------- inversion

#[test]
fn inversion_swaps_which_end_is_which() {
    // Getting this backwards produces a limit that denies the real range and
    // permits its mirror. Asymmetric travel makes the mistake visible.
    const J: &[Joint] = &[joint("elbow", Some(limits(0.0, QUARTER)))];
    const STRAIGHT: &[Channel] =
        &[channel("elbow", Some(ChannelId::Number(1)), false, None)];
    const FLIPPED: &[Channel] =
        &[channel("elbow", Some(ChannelId::Number(1)), true, None)];

    let straight = safety_limits("n", &robot(J), &harness(STRAIGHT, &[]));
    let flipped = safety_limits("n", &robot(J), &harness(FLIPPED, &[]));

    let (s_lo, s_hi) = straight.converted[0].actuator_degrees;
    let (f_lo, f_hi) = flipped.converted[0].actuator_degrees;

    assert!((s_lo - 0.0).abs() < 1e-9 && (s_hi - 90.0).abs() < 1e-9);
    assert!((f_lo + 90.0).abs() < 1e-9 && (f_hi - 0.0).abs() < 1e-9);
    assert!(f_lo < f_hi, "the range must stay ordered after mapping");
}

#[test]
fn a_zero_offset_shifts_both_ends_together() {
    const J: &[Joint] = &[joint("elbow", Some(limits(0.0, QUARTER)))];
    const OFFSET: &[Channel] = &[channel(
        "elbow",
        Some(ChannelId::Number(1)),
        false,
        Some(Radians(QUARTER)),
    )];

    let report = safety_limits("n", &robot(J), &harness(OFFSET, &[]));
    let (lo, hi) = report.converted[0].actuator_degrees;
    assert!((lo - 90.0).abs() < 1e-9);
    assert!((hi - 180.0).abs() < 1e-9);
}

// ------------------------------------------------------------------- the seam

#[test]
fn servo_command_is_the_one_place_degrees_appear() {
    let ch = channel("elbow", Some(ChannelId::Number(2)), true, Some(Radians(0.1)));
    let command = servo_command("elbow", &ch, 2, Radians(0.5));

    match command {
        MovementCommand::ServoAngle { name, channel, degrees } => {
            assert_eq!(name, "elbow");
            assert_eq!(channel, 2);
            // -1 * 0.5 + 0.1 = -0.4 rad
            let expected: Degrees = Radians(-0.4).into();
            assert!((degrees - expected.0).abs() < 1e-9);
        }
        other => panic!("expected a ServoAngle, got {other:?}"),
    }
}

#[test]
fn a_command_at_the_limit_passes_the_limit_it_was_derived_from() {
    // End to end: derive a bound from the model, then build a command at the
    // joint's declared extreme and check the gate's own integer view of it sits
    // inside the bound.
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    let limit = &report.limits[0];

    let at_extreme = servo_command("shoulder", &C[0], 3, Radians(QUARTER));
    let value = at_extreme.safety_value();
    assert_eq!(value, 90);
    assert!(
        value > limit.value_max.unwrap(),
        "the declared extreme sits just outside the enforced bound, by design — \
         that is the 1.5° the conversion gives up so the gate cannot overshoot"
    );

    // And a command comfortably inside is admitted.
    let inside = servo_command("shoulder", &C[0], 3, Radians(1.0));
    assert!(inside.safety_value() <= limit.value_max.unwrap());
    assert!(inside.safety_value() >= limit.value_min.unwrap());
}

// ------------------------------------------------------------------- the notes

#[test]
fn an_unchecked_cable_run_is_surfaced_as_a_note() {
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];
    const RUNS: &[CableRun] = &[CableRun {
        id: "wrist-loom",
        crosses: &["shoulder"],
        permits_full_travel: None,
        travel_limit: &[],
    }];

    let report = safety_limits("node-1", &robot(J), &harness(C, RUNS));
    assert!(report.is_complete(), "an unchecked run does not block a limit");
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("wrist-loom") && n.contains("wider than the mechanism")));
}

#[test]
fn a_missing_supply_voltage_is_noted_without_blocking_travel_limits() {
    const J: &[Joint] = &[joint("shoulder", Some(limits(-QUARTER, QUARTER)))];
    const C: &[Channel] = &[channel("shoulder", Some(ChannelId::Number(3)), false, None)];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert_eq!(report.limits.len(), 1);
    assert!(report.notes.iter().any(|n| n.contains("what it can hold")));
}

#[test]
fn a_partial_import_is_not_complete_and_says_which_joint() {
    const J: &[Joint] = &[
        joint("shoulder", Some(limits(-QUARTER, QUARTER))),
        joint("elbow", None),
    ];
    const C: &[Channel] = &[
        channel("shoulder", Some(ChannelId::Number(3)), false, None),
        channel("elbow", Some(ChannelId::Number(4)), false, None),
    ];

    let report = safety_limits("node-1", &robot(J), &harness(C, &[]));
    assert_eq!(report.limits.len(), 1);
    assert!(!report.is_complete());
    assert_eq!(report.unbounded.len(), 1);
    assert_eq!(report.unbounded[0].joint_id, "elbow");
}

//! Turn a cited body model into Track 0 limits, and joint angles into commands.
//!
//! Every limit the deterministic gate enforces today is hand-written: a pin
//! allowlist and a value range, typed into config by whoever set the node up.
//! The gate is exactly as good as those numbers and nothing checks them against
//! the mechanism they bound.
//!
//! [ClawBot](https://github.com/thewriterben/ClawBot) is the platform's mechanism
//! model, and its joint limits are citation-gated — a limit with no source fails
//! its validator. This crate reads that model and produces [`SafetyLimit`]s from
//! it, so a bound enforced on hardware traces to a datasheet.
//!
//! # Two things this crate refuses to do
//!
//! **It never invents a bound.** A joint whose travel nobody sourced arrives here
//! as `None`, and the answer is that it cannot be bounded — reported by
//! [`ImportReport::unbounded`], never emitted as a permissive limit. A wide-open
//! `SafetyLimit` for a joint whose limits are unknown would be worse than no
//! limit at all, because it looks like one.
//!
//! **It never widens a bound to make it fit.** The gate compares
//! `degrees.round()` against integer `value_min`/`value_max`, so the integer
//! bounds are chosen to *absorb* that rounding rather than sit inside it. See
//! [`safety_limits`] for the arithmetic and what it costs.
//!
//! # The degrees/radians seam
//!
//! ClawBot stores radians (its ADR-0005, and REP-103 puts URDF there too). This
//! repo's [`MovementCommand::ServoAngle`] takes degrees. Two systems each correct
//! in their own frame is how a mechanism gets commanded to 57 times the intended
//! angle, and ClawBot's binding makes the mistake impossible to make silently:
//! `Radians` and `Degrees` are distinct types with an explicit conversion.
//!
//! [`servo_command`] is the one place in this repo that conversion happens.

use clawbot::{Channel, ChannelId, Degrees, Harness, Radians, Robot};
use obc_movement::MovementCommand;
use obc_safety::limits::SafetyLimit;

/// A joint that could not be given a safety limit, and why.
///
/// This is the important half of an [`ImportReport`]. Each entry is a joint the
/// gate will not bound from the model — so either the operator sources the
/// missing fact, or that channel runs under whatever limit was typed by hand,
/// and they should know which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unbounded {
    pub joint_id: String,
    pub reason: String,
}

/// What a joint's declared travel became after conversion, and what that cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub joint_id: String,
    pub channel: i64,
    /// The joint's own limits, in degrees at the actuator (inversion and zero
    /// offset applied).
    pub actuator_degrees: (f64, f64),
    /// What the gate will actually enforce.
    pub enforced: (i64, i64),
    /// Degrees of legitimate travel given up to guarantee the gate cannot pass a
    /// command outside the declared limits. Never negative.
    pub travel_given_up_deg: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub limits: Vec<SafetyLimit>,
    pub converted: Vec<Converted>,
    pub unbounded: Vec<Unbounded>,
    pub notes: Vec<String>,
}

impl ImportReport {
    /// True when every driven joint in the harness got a limit. **Check this.**
    /// A partial import is the normal case for a mechanism that is still being
    /// sourced, and it is not a failure — but running as though it were complete
    /// is.
    pub fn is_complete(&self) -> bool {
        self.unbounded.is_empty()
    }
}

/// The angle this actuator must be driven to in order to put its joint at
/// `joint`, as a command this repo's gate and sinks understand.
///
/// The conversion happens here and nowhere else. `Channel::actuator_angle`
/// applies inversion and zero offset and returns radians on purpose, so this
/// function is the single visible boundary between the two unit systems.
pub fn servo_command(
    name: impl Into<String>,
    channel: &Channel,
    channel_number: i64,
    joint: Radians,
) -> MovementCommand {
    let degrees: Degrees = channel.actuator_angle(joint).into();
    MovementCommand::ServoAngle {
        name: name.into(),
        channel: channel_number,
        degrees: degrees.0,
    }
}

/// Derive Track 0 limits for every driven joint of `robot` under `harness`.
///
/// # The rounding, and why it is not the obvious one
///
/// The gate admits a command when `degrees.round()` lies in
/// `[value_min, value_max]` — so an integer `value_max` of 90 actually admits any
/// angle below 90.5. Emitting `floor(true_upper)` would therefore let through
/// commands up to half a degree past a limit somebody sourced from a datasheet.
///
/// So the bounds absorb the gate's rounding instead of sitting inside it:
///
/// ```text
/// value_min = ceil (lower_deg + 0.5)
/// value_max = floor(upper_deg - 0.5)
/// ```
///
/// No command that the gate admits can then exceed the declared travel. It costs
/// under a degree of legitimate range per bound — and exactly nothing when the
/// declared travel happens to land on a half-degree — with the cost reported per
/// joint in [`Converted::travel_given_up_deg`] rather than absorbed quietly. An
/// operator who needs that degree back should widen the limit deliberately, where
/// the widening is visible.
///
/// A declared travel of exactly one degree survives it and bounds tightly — the
/// gate ends up admitting precisely that degree and nothing more. Anything
/// narrower yields no limit at all and is reported as [`Unbounded`]. Emitting an inverted or empty range would be
/// a limit that denies everything while looking like a limit that permits
/// something.
pub fn safety_limits(node_id: &str, robot: &Robot, harness: &Harness) -> ImportReport {
    let mut report = ImportReport::default();

    if harness.robot_id != robot.id {
        report.notes.push(format!(
            "harness '{}' wires robot '{}', not '{}' — nothing imported",
            harness.id, harness.robot_id, robot.id
        ));
        return report;
    }

    for channel in harness.channels {
        let joint_id = channel.joint_id.to_string();
        let mut refuse = |reason: &str| {
            report.unbounded.push(Unbounded {
                joint_id: joint_id.clone(),
                reason: reason.to_string(),
            });
        };

        let Some(joint) = robot.joint(channel.joint_id) else {
            refuse("the harness drives a joint the robot record does not declare");
            continue;
        };

        if joint.mimic.is_some() {
            refuse(
                "this joint mimics another and is not independently commandable, \
                 so a channel driving it is a wiring error rather than a limit to derive",
            );
            continue;
        }

        let Some(ChannelId::Number(pin)) = channel.channel else {
            refuse(
                "the controller identifies this output by name rather than number, \
                 and the gate keys on integer pins",
            );
            continue;
        };

        let Some(limits) = joint.limits else {
            refuse(
                "no limits on this joint. UNKNOWN is never unlimited — a permissive \
                 SafetyLimit here would look like a bound and be none",
            );
            continue;
        };

        let (Some(lower), Some(upper)) = (limits.lower, limits.upper) else {
            if limits.lower_mm.is_some() || limits.upper_mm.is_some() {
                refuse("this joint is prismatic; servo_angle bounds an angle");
            } else {
                refuse("limits are present but carry no angular travel");
            }
            continue;
        };

        // Inversion flips which end is which, so map both and re-sort. Getting
        // this backwards produces a limit that denies the whole real range and
        // permits the mirror of it.
        let a: Degrees = channel.actuator_angle(lower).into();
        let b: Degrees = channel.actuator_angle(upper).into();
        let (lo_deg, hi_deg) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };

        let value_min = (lo_deg + 0.5).ceil();
        let value_max = (hi_deg - 0.5).floor();

        if value_min > value_max {
            refuse(&format!(
                "declared travel is {:.3}° wide, too narrow to bound at the gate's \
                 whole-degree resolution without inverting the range",
                hi_deg - lo_deg
            ));
            continue;
        }

        // What the gate ACTUALLY admits is the integer bound widened by its own
        // rounding, so the range given up is measured against that — not against
        // the integers. Measuring the integer gap instead overstates the loss and,
        // worse, reports a cost at the boundary case where there is none.
        let given_up = ((value_min - 0.5) - lo_deg) + (hi_deg - (value_max + 0.5));
        report.converted.push(Converted {
            joint_id: joint_id.clone(),
            channel: pin,
            actuator_degrees: (lo_deg, hi_deg),
            enforced: (value_min as i64, value_max as i64),
            travel_given_up_deg: given_up.max(0.0),
        });

        report.limits.push(SafetyLimit {
            node_id: node_id.to_string(),
            tool: "servo_angle".to_string(),
            allowed_pins: Some(vec![pin]),
            value_min: Some(value_min as i64),
            value_max: Some(value_max as i64),
            // ClawBot carries `velocity_rad_s`, which is how fast the joint may
            // move — not how often it may be commanded. They are different
            // quantities and mapping one onto the other would be inventing a
            // rate limit nobody specified.
            min_interval_ms: None,
        });
    }

    if harness.power.supply_volts.is_none() {
        report.notes.push(
            "the harness declares no supply voltage, so ClawBot cannot derive a static \
             capacity for this mechanism. That does not affect these limits, which bound \
             travel rather than load — but nothing here knows what it can hold."
                .to_string(),
        );
    }

    let unchecked: Vec<&str> = harness.unchecked_runs().map(|r| r.id).collect();
    if !unchecked.is_empty() {
        report.notes.push(format!(
            "{} cable run(s) cross joints with no established travel: {}. The harness may \
             bind before these limits do, so the enforced range can be wider than the \
             mechanism actually permits.",
            unchecked.len(),
            unchecked.join(", ")
        ));
    }

    report
}

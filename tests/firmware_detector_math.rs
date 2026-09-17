//! The detector exists twice, so the two copies are compared on real pixels.
//!
//! `scripts/vision/classify.py` is the reference: it is the implementation the
//! 148-frame bench fixture was scored with, and the three thresholds were chosen
//! against its output. `firmware/obc-esp32-s3/src/detector_math.rs` is a port of
//! it into the firmware, because the node has to make the decision itself.
//!
//! This repo already knows what happens to two copies of one fact that nothing
//! compares — `camera.rs` claimed a board its own `Cargo.toml` contradicted, two
//! directories away, for weeks. So this file runs the Rust against frames the
//! Python has already scored and fails if they disagree.
//!
//! **What this does NOT check.** It says nothing about whether the thresholds are
//! right, and nothing about whether they survive the move from host-decoded JPEG
//! to the sensor's raw Y8 — the 2026-09-17 ADR is explicit that they must be
//! re-measured on the node. This is an agreement test between two pieces of
//! arithmetic, which is a smaller claim and the only one it can support.

use std::fs;
use std::path::PathBuf;

#[path = "../firmware/obc-esp32-s3/src/detector_math.rs"]
#[allow(dead_code)]
mod detector_math;

use detector_math::{
    classify, score, Class, Readiness, Scores, Thresholds, WarmUp, HOST_FIXTURE_2026_09_16,
    SETTLE_DELTA, SETTLE_RUN,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vision-grey-pairs-2026-09-17")
}

fn read(name: &str) -> Vec<u8> {
    let p = fixture_dir().join(name);
    fs::read(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

#[derive(Debug)]
struct Expected {
    label: String,
    width: usize,
    height: usize,
    pixel_eps: f32,
    frac: f32,
    edge: f32,
    mean: f32,
    class: String,
}

fn expected() -> Vec<Expected> {
    let raw = fs::read_to_string(fixture_dir().join("expected.json"))
        .expect("expected.json missing -- regenerate with scripts/vision/make_grey_pairs.py");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("expected.json is not JSON");
    v["pairs"]
        .as_array()
        .expect("expected.json has no `pairs` array")
        .iter()
        .map(|p| Expected {
            label: p["label"].as_str().unwrap().to_string(),
            width: p["width"].as_u64().unwrap() as usize,
            height: p["height"].as_u64().unwrap() as usize,
            pixel_eps: p["pixel_eps"].as_f64().unwrap() as f32,
            frac: p["frac"].as_f64().unwrap() as f32,
            edge: p["edge"].as_f64().unwrap() as f32,
            mean: p["mean"].as_f64().unwrap() as f32,
            class: p["class"].as_str().unwrap().to_string(),
        })
        .collect()
}

// ── the comparison ───────────────────────────────────────────────────────────

/// `frac` is a count of pixels over a threshold, so the two implementations
/// should agree **exactly**, not approximately. Both compute `cur - prev` in
/// f32 over small integers, where the subtraction is exact; a disagreement here
/// is a disagreement about the rule, not about floating point.
#[test]
fn frac_matches_the_reference_exactly() {
    let pairs = expected();
    assert!(!pairs.is_empty(), "fixture has no pairs");
    for e in &pairs {
        let prev = read(&format!("{}_prev.gray", e.label));
        let cur = read(&format!("{}_cur.gray", e.label));
        let s = score(&prev, &cur, e.width, e.height, e.pixel_eps)
            .unwrap_or_else(|| panic!("{}: score() refused the geometry", e.label));
        assert!(
            (s.frac - e.frac).abs() < 1e-6,
            "{}: frac {} but classify.py says {}",
            e.label,
            s.frac,
            e.frac
        );
    }
}

/// `edge` is a mean of square roots, so the two sums differ in the last bits:
/// numpy reduces float32 pairwise, this accumulates in f64. The tolerance is
/// there for that and nothing else — it is far tighter than any threshold gap
/// (the nearest boundary pair is `edge_light` 4.20 against `edge_nudge` 9.00),
/// so no plausible rounding difference can move a frame between classes.
#[test]
fn edge_matches_the_reference_within_float_noise() {
    for e in &expected() {
        let prev = read(&format!("{}_prev.gray", e.label));
        let cur = read(&format!("{}_cur.gray", e.label));
        let s = score(&prev, &cur, e.width, e.height, e.pixel_eps).unwrap();
        assert!(
            (s.edge - e.edge).abs() < 1e-3,
            "{}: edge {} but classify.py says {} (delta {})",
            e.label,
            s.edge,
            e.edge,
            (s.edge - e.edge).abs()
        );
        assert!(
            (s.mean - e.mean).abs() < 1e-3,
            "{}: mean {} but the reference says {}",
            e.label,
            s.mean,
            e.mean
        );
    }
}

/// The scores agreeing is not the same as the verdicts agreeing. This is the
/// claim that actually matters to a caller.
#[test]
fn the_two_implementations_reach_the_same_verdict() {
    for e in &expected() {
        let prev = read(&format!("{}_prev.gray", e.label));
        let cur = read(&format!("{}_cur.gray", e.label));
        let s = score(&prev, &cur, e.width, e.height, e.pixel_eps).unwrap();
        let got = classify(&s, &HOST_FIXTURE_2026_09_16);
        assert_eq!(
            got.as_str(),
            e.class,
            "{}: firmware says {}, classify.py says {} (frac {}, edge {})",
            e.label,
            got.as_str(),
            e.class,
            s.frac,
            s.edge
        );
    }
}

/// The fixture must keep exercising more than one verdict, or the test above
/// passes by having nothing to disagree about.
#[test]
fn the_fixture_spans_more_than_one_class() {
    let classes: std::collections::BTreeSet<String> =
        expected().into_iter().map(|e| e.class).collect();
    assert!(
        classes.len() >= 2,
        "every fixture pair classifies the same way ({classes:?}); the agreement \
         test would pass on a constant function"
    );
}

// ── the rule itself, at its boundaries ───────────────────────────────────────
//
// The fixture covers the arithmetic on real pixels. These cover the four
// branches on synthetic scores, including the exact boundary values, which no
// photograph is going to land on.

fn s(frac: f32, edge: f32) -> Scores {
    Scores {
        frac,
        edge,
        mean: 110.0,
    }
}

#[test]
fn the_rule_is_inclusive_where_the_reference_is_inclusive() {
    let t = HOST_FIXTURE_2026_09_16;
    // classify.py: `if frac <= FRAC_HI: quiet`. At the boundary, quiet.
    assert_eq!(classify(&s(t.frac_hi, 100.0), &t), Class::Quiet);
    assert_eq!(classify(&s(t.frac_hi + 1e-4, 0.0), &t), Class::Light);
    // `if edge <= EDGE_LIGHT: light`
    assert_eq!(classify(&s(0.9, t.edge_light), &t), Class::Light);
    // `if edge >= EDGE_NUDGE: nudge`
    assert_eq!(classify(&s(0.9, t.edge_nudge), &t), Class::Nudge);
    // between the two, motion
    assert_eq!(
        classify(&s(0.9, (t.edge_light + t.edge_nudge) / 2.0), &t),
        Class::Motion
    );
}

/// Only `Motion` is a detection. A light switch and a bumped tripod are states.
///
/// This is the property the whole detector exists for: the bench produced 66
/// frames of quiet room and lighting swing and the rule reported no detections
/// in any of them. If `Light` or `Nudge` ever starts counting as a detection,
/// that result stops being true without a single threshold changing.
#[test]
fn only_motion_counts_as_a_detection() {
    assert!(Class::Motion.is_detection());
    assert!(!Class::Quiet.is_detection());
    assert!(!Class::Light.is_detection());
    assert!(!Class::Nudge.is_detection());
}

/// `mean()` and `score()` must agree about brightness, or the warm-up gate and
/// the scores are describing different frames.
#[test]
fn standalone_mean_agrees_with_the_score_it_replaces() {
    for e in &expected() {
        let cur = read(&format!("{}_cur.gray", e.label));
        let prev = read(&format!("{}_prev.gray", e.label));
        let s = score(&prev, &cur, e.width, e.height, e.pixel_eps).unwrap();
        let m = detector_math::mean(&cur).expect("non-empty frame has a mean");
        assert!(
            (m - s.mean).abs() < 1e-3,
            "{}: mean() says {m}, score() says {}",
            e.label,
            s.mean
        );
    }
    assert_eq!(detector_math::mean(&[]), None);
}

#[test]
fn score_refuses_a_geometry_it_cannot_trust() {
    let a = vec![0u8; 16];
    // Right length, wrong shape for the other slice.
    assert!(score(&a, &a[..15], 4, 4, 12.0).is_none());
    assert!(score(&a, &a, 4, 5, 12.0).is_none());
    assert!(score(&[], &[], 0, 0, 12.0).is_none());
    // And the one that works, so the refusals above are not vacuous.
    assert!(score(&a, &a, 4, 4, 12.0).is_some());
}

#[test]
fn an_unchanged_frame_scores_zero() {
    let a: Vec<u8> = (0..(8 * 8)).map(|i| (i * 3 % 251) as u8).collect();
    let got = score(&a, &a, 8, 8, 12.0).unwrap();
    assert_eq!(got.frac, 0.0);
    assert_eq!(got.edge, 0.0);
    assert_eq!(classify(&got, &HOST_FIXTURE_2026_09_16), Class::Quiet);
}

/// A uniform brightness shift moves every pixel and no edge — which is the
/// single fact the whole two-score design rests on.
#[test]
fn a_uniform_shift_is_light_not_motion() {
    let w = 32;
    let h = 32;
    // A scene with real structure, so `edge` has something it could report.
    let base: Vec<u8> = (0..w * h)
        .map(|i| if (i / w) % 4 < 2 { 40u8 } else { 160u8 })
        .collect();
    // +50 everywhere, clamped nowhere.
    let lit: Vec<u8> = base.iter().map(|p| p + 50).collect();

    let got = score(&base, &lit, w, h, 12.0).unwrap();
    assert_eq!(got.frac, 1.0, "every pixel should have moved");
    assert_eq!(
        got.edge, 0.0,
        "a uniform additive shift leaves every central difference identical"
    );
    assert_eq!(classify(&got, &HOST_FIXTURE_2026_09_16), Class::Light);
}

// ── the warm-up gate ─────────────────────────────────────────────────────────

/// Frame 000 of the bench fixture scored 17× the quiet floor with nothing
/// happening, because auto-exposure was still converging after the port-open
/// reset. A detector armed at boot calls that a major event every boot.
#[test]
fn judgement_is_withheld_until_brightness_settles() {
    let mut w = WarmUp::new();
    assert_eq!(w.observe(62.9), Readiness::NoReference);
    // The big exposure step out of the warm-up frame.
    assert_eq!(w.observe(108.7), Readiness::WarmingUp);
    // Settling, but one settled delta is indistinguishable from passing through.
    assert_eq!(w.observe(109.4), Readiness::WarmingUp);
    assert_eq!(w.observe(110.1), Readiness::Ready);
    assert_eq!(w.observe(110.9), Readiness::Ready);
}

#[test]
fn a_real_exposure_step_restarts_the_count() {
    let mut w = WarmUp::new();
    w.observe(110.0);
    w.observe(110.5);
    assert_eq!(w.observe(110.9), Readiness::Ready);
    // Someone switches a lamp: the frames either side are not comparable.
    assert_eq!(w.observe(110.9 + SETTLE_DELTA + 0.1), Readiness::WarmingUp);
    // And it takes the full run again, not one frame.
    for _ in 0..(SETTLE_RUN - 1) {
        assert_eq!(w.observe(114.0), Readiness::WarmingUp);
    }
    assert_eq!(w.observe(114.0), Readiness::Ready);
}

/// After a nudge the reference frame is a picture of somewhere else.
#[test]
fn reset_drops_the_reference_rather_than_comparing_across_a_nudge() {
    let mut w = WarmUp::new();
    w.observe(110.0);
    w.observe(110.2);
    assert_eq!(w.observe(110.4), Readiness::Ready);
    w.reset();
    assert_eq!(w.observe(110.4), Readiness::NoReference);
}

/// `warming_up` and `quiet` must not be the same word on the wire. Reporting
/// "nothing happened" while blind is the silent degradation this project's
/// rules forbid.
#[test]
fn not_knowing_is_spelled_differently_from_nothing_happening() {
    assert_ne!(Readiness::WarmingUp.as_str(), Class::Quiet.as_str());
    assert_ne!(Readiness::NoReference.as_str(), Class::Quiet.as_str());
    let names = [
        Readiness::NoReference.as_str(),
        Readiness::WarmingUp.as_str(),
        Readiness::Ready.as_str(),
    ];
    let unique: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "two readiness states share a name"
    );
}

// ── the thresholds are carried, not invented ─────────────────────────────────

/// The firmware's constants must be the reference's constants.
///
/// Read out of `classify.py` rather than retyped here, because retyping them is
/// how the two would drift. If the Python moves a threshold and the firmware
/// does not, the fleet is running a rule nobody measured.
#[test]
fn firmware_thresholds_match_the_reference_script() {
    let src = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/vision/classify.py"),
    )
    .expect("scripts/vision/classify.py is missing -- it is the reference");

    let py = |name: &str| -> f32 {
        let line = src
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{name} =")))
            .unwrap_or_else(|| panic!("{name} not found in classify.py"));
        line.split('=')
            .nth(1)
            .unwrap()
            .split('#')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("cannot parse {name}: {e}"))
    };

    let t: Thresholds = HOST_FIXTURE_2026_09_16;
    assert_eq!(t.frac_hi, py("FRAC_HI"), "FRAC_HI drifted");
    assert_eq!(t.edge_light, py("EDGE_LIGHT"), "EDGE_LIGHT drifted");
    assert_eq!(t.edge_nudge, py("EDGE_NUDGE"), "EDGE_NUDGE drifted");
    assert_eq!(t.pixel_eps, py("PIXEL_EPS"), "PIXEL_EPS drifted");
}

//! The motion classifier against the camera-bench fixture, in CI.
//!
//! This is `python scripts/vision/classify.py --check` without numpy or
//! Pillow, over the same 148 frames (`tests/fixtures/vision-bench-2026-09-16/`,
//! node `obc-esp32-s3-4b95f8`, LILYGO T-CameraPlus-S3, OV5640). It is two
//! checks rather than one, for a reason found while porting:
//!
//! `camera_nudge/024.jpg` scores edge 4.199912 under Pillow's JPEG decoder,
//! 0.00009 inside `light`, and 4.200503 under zune-jpeg, inside `motion`. So
//! the recorded counts depend on the decoder at the fourth decimal, and
//! comparing counts after a different decoder would fail on the decoder, not on
//! the classifier. Hence:
//!
//! 1. **The rule.** The Python's own per-pair scores (`scores.txt`, written by
//!    `classify.py --scores`) go through [`Scores::classify`], which must
//!    reproduce every recorded class and the `EXPECTED` counts exactly. No
//!    decoder involved.
//! 2. **The scoring.** This side decodes each JPEG, converts it to grey as
//!    Pillow's `convert("L")` does, and scores it with [`score`]. Every pair
//!    must land within a small tolerance of the Python's scores, and may
//!    disagree on class only when the Python's score is itself within that
//!    tolerance of a boundary.
//!
//! On Pillow's exact pixels the Rust scores match the Python's to 1e-6 (checked
//! while porting, by exporting Pillow's greyscale); the tolerance below covers
//! only the two decoders' differences (measured: frac 0.0002, edge 0.0017).
//!
//! `OBC_MOTION_DUMP=1 cargo test -p obc-vision --test motion_fixture -- --nocapture`
//! prints this side's per-pair scores in the format of `scores.txt`.

use obc_vision::motion::{score, FrameClass, Scores, EDGE_LIGHT, EDGE_NUDGE, FRAC_HI};
use std::collections::BTreeMap;

mod common;
use common::{fixture, frame_files, grey};

/// Decoder differences, with headroom over the measured maximum.
const FRAC_TOL: f32 = 0.001;
const EDGE_TOL: f32 = 0.01;

const SETS: [&str; 4] = ["baseline_still", "light_change", "person", "camera_nudge"];
const CLASSES: [&str; 4] = ["quiet", "light", "motion", "nudge"];

/// One frame pair as `classify.py --scores` recorded it.
struct Recorded {
    label: String,
    file: String,
    class: String,
    scores: Scores,
}

fn recorded() -> Vec<Recorded> {
    let path = fixture().join("scores.txt");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 5, "malformed line in scores.txt: {l}");
            Recorded {
                label: f[0].into(),
                file: f[1].into(),
                class: f[2].into(),
                scores: Scores {
                    frac: f[3].parse().unwrap(),
                    edge: f[4].parse().unwrap(),
                },
            }
        })
        .collect()
}

/// `EXPECTED` in classify.py, known misclassifications included: 3 person
/// frames in `light`, 2 pre-nudge frames in `motion` that are probably a hand
/// reaching for the camera.
fn expected(label: &str) -> [usize; 4] {
    match label {
        "baseline_still" => [28, 0, 0, 0],
        "light_change" => [38, 0, 0, 0],
        "person" => [15, 3, 20, 0],
        "camera_nudge" => [26, 2, 2, 8],
        other => panic!("unknown set {other}"),
    }
}

/// This side's scores for every pair in one set, keyed by the later frame's
/// file name. Frame 000 is discarded: auto-exposure converging after the
/// port-open reset (see `classify.py`).
fn decoded_scores(label: &str) -> BTreeMap<String, Scores> {
    let files = &frame_files(label)[1..];
    let frames: Vec<_> = files.iter().map(|p| grey(p)).collect();
    let dump = std::env::var_os("OBC_MOTION_DUMP").is_some();
    let mut out = BTreeMap::new();
    for (i, pair) in frames.windows(2).enumerate() {
        let ((prev, w, h), (cur, w2, h2)) = (&pair[0], &pair[1]);
        assert_eq!((w, h), (w2, h2), "frame size changed within {label}");
        let s = score(prev, cur, *w, *h).unwrap();
        let name = files[i + 1]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if dump {
            println!(
                "{label} {name} {} {:.6} {:.6}",
                s.classify().as_str(),
                s.frac,
                s.edge
            );
        }
        out.insert(name, s);
    }
    out
}

/// Whether a recorded score is close enough to a boundary that the decoders'
/// difference could move it across.
fn near_a_boundary(s: Scores) -> bool {
    (s.frac - FRAC_HI).abs() <= FRAC_TOL
        || (s.frac > FRAC_HI
            && ((s.edge - EDGE_LIGHT).abs() <= EDGE_TOL || (s.edge - EDGE_NUDGE).abs() <= EDGE_TOL))
}

#[test]
fn the_rule_reproduces_every_recorded_class_and_count() {
    let rec = recorded();
    assert_eq!(
        rec.len(),
        142,
        "scores.txt should have one line per frame pair"
    );
    for label in SETS {
        let mut got = [0usize; 4];
        for r in rec.iter().filter(|r| r.label == label) {
            let class = r.scores.classify().as_str();
            assert_eq!(
                class, r.class,
                "{label}/{}: the Python called {:?} '{}', the Rust rule '{class}'",
                r.file, r.scores, r.class
            );
            got[CLASSES.iter().position(|c| *c == class).unwrap()] += 1;
        }
        assert_eq!(
            got,
            expected(label),
            "{label}: [quiet, light, motion, nudge]"
        );
    }
}

#[test]
fn decoding_and_scoring_agree_with_pillow_within_decoder_noise() {
    let rec = recorded();
    let mut fragile = Vec::new();
    for label in SETS {
        let ours = decoded_scores(label);
        let theirs: Vec<&Recorded> = rec.iter().filter(|r| r.label == label).collect();
        assert_eq!(ours.len(), theirs.len(), "{label}: pair count");
        for r in theirs {
            let s = ours
                .get(&r.file)
                .unwrap_or_else(|| panic!("{label}/{}: not scored", r.file));
            assert!(
                (s.frac - r.scores.frac).abs() <= FRAC_TOL
                    && (s.edge - r.scores.edge).abs() <= EDGE_TOL,
                "{label}/{}: ours {s:?}, Pillow {:?}",
                r.file,
                r.scores
            );
            if s.classify().as_str() != r.class {
                assert!(
                    near_a_boundary(r.scores),
                    "{label}/{}: class differs ({} vs {}) away from any boundary",
                    r.file,
                    s.classify().as_str(),
                    r.class
                );
                fragile.push(format!("{label}/{}", r.file));
            }
        }
    }
    // Pinned, so a new decoder-dependent frame is noticed rather than absorbed.
    assert!(
        fragile.is_empty() || fragile == ["camera_nudge/024.jpg"],
        "decoder-dependent classes changed: {fragile:?}"
    );
}

/// The property that matters most, on its own line and on this side's own
/// decode: a quiet room and a lighting change produce no detections.
#[test]
fn a_quiet_room_and_a_light_switch_detect_nothing() {
    for label in ["baseline_still", "light_change"] {
        let loud: Vec<_> = decoded_scores(label)
            .into_iter()
            .filter(|(_, s)| matches!(s.classify(), FrameClass::Motion | FrameClass::Nudge))
            .collect();
        assert!(loud.is_empty(), "{label} produced detections: {loud:?}");
    }
    assert!(FrameClass::Motion.is_detection());
    assert!(!FrameClass::Nudge.is_detection());
}

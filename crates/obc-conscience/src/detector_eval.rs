//! Detector false-negative measurement — the residual risk the gate can't cover.
//!
//! The perception gate ([`crate::consent::PerceptionGate`]) acts on the labels
//! the upstream detector *emits*. It is honest about its blind spot (see §6 of
//! the spec and the doc on [`crate::classifier`]): **it cannot gate what the
//! detector never reports.** If a person is present and the detector emits no
//! label the classifier maps to `"human"`, the gate never sees a human, so
//! nothing is refused — the frame is stored and reaches the reasoner. That is a
//! *detector* false negative, upstream of the conscience, and it is the one
//! failure mode the fail-closed classifier cannot save you from.
//!
//! This module measures it. Given human-annotated ground truth (what was really
//! present in each frame) and the detector's output for the same frames, it
//! computes, per consent class, how often a truly-present class went undetected
//! — with the **restricted class (`"human"`) foregrounded**, because a missed
//! deer is a lost sighting and a missed person is a privacy breach.
//!
//! ## Why an upper confidence bound, not just a rate
//!
//! "Zero human misses in 20 frames" is not a 0% miss rate — it is *too little
//! data to claim one*. Safety cares about the worst case still consistent with
//! the evidence, so every miss rate is reported with a **Wilson upper 95%
//! bound**: with 0 misses in 20 present-frames the point estimate is 0% but the
//! upper bound is ~16%, which is the number an operator should actually plan
//! against. A low headline rate on a tiny sample is the trap; the bound is the
//! guardrail.
//!
//! ## What this needs to run for real
//!
//! Synthetic frames exercise the math (see the tests). A real number needs a
//! labeled evaluation set from the deployment: a sample of frames a human has
//! annotated with the classes actually present, paired with the detector's
//! output for those same frames. Feed them in as [`EvalFrame`]s (they
//! deserialize from JSON) and read the [`FnReport`]. Until that set exists this
//! reports the *method*, not a field number — and says so.

use crate::classifier::SubjectClassifier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The 95% two-sided normal quantile (z), for the Wilson interval.
const Z_95: f64 = 1.959_963_984_540_054;

/// One evaluation frame: what a human says was present, and what the detector
/// reported for the same frame. Labels are the detector's/annotator's raw
/// vocabulary; both are classified onto consent classes before comparison, so
/// the measurement is at the level the gate actually acts on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalFrame {
    /// Frame / event identifier (for traceability; not used in the math).
    #[serde(default)]
    pub frame_id: String,
    /// Raw labels a human annotator says were truly present in the frame.
    pub truth: Vec<String>,
    /// Raw labels the detector emitted for the frame.
    pub detected: Vec<String>,
}

/// Per-class confusion counts and the derived miss rate.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassStats {
    /// Consent class (e.g. `"human"`, `"wildlife"`).
    pub class: String,
    /// Frames where the class was truly present (`TP + FN`) — the sample size
    /// that matters for the miss rate.
    pub present: u64,
    /// Present AND detected (true positives).
    pub detected_when_present: u64,
    /// Present AND missed (false negatives) — the safety-critical count for the
    /// restricted class.
    pub missed: u64,
    /// Detected but NOT truly present (false positives). Not a safety failure for
    /// a default-deny body — a spurious human detection only over-refuses — but
    /// reported so a noisy detector is visible.
    pub false_positive: u64,
}

impl ClassStats {
    /// Fraction of present-frames the detector missed (`FN / (TP + FN)`), i.e.
    /// `1 - recall`. `None` when the class never appeared in ground truth (no
    /// sample to speak of — do not report 0%, report "no data").
    pub fn miss_rate(&self) -> Option<f64> {
        (self.present > 0).then(|| self.missed as f64 / self.present as f64)
    }

    /// Recall: fraction of present-frames the detector caught. `None` when the
    /// class never appeared.
    pub fn recall(&self) -> Option<f64> {
        (self.present > 0).then(|| self.detected_when_present as f64 / self.present as f64)
    }

    /// Wilson score **upper** 95% bound on the miss rate — the worst miss rate
    /// still consistent with the evidence, and the number safety should plan
    /// against. `None` when the class never appeared.
    ///
    /// This is deliberately the upper bound, not a symmetric interval: for a
    /// privacy failure the question is "how bad could the miss rate really be?"
    pub fn miss_rate_upper95(&self) -> Option<f64> {
        (self.present > 0).then(|| wilson_upper(self.missed, self.present, Z_95))
    }
}

/// The full false-negative report.
#[derive(Debug, Clone)]
pub struct FnReport {
    /// Number of frames evaluated.
    pub frames: u64,
    /// The restricted (most-sensitive) class — the one whose misses are a breach.
    pub restricted_class: String,
    /// Per-class stats, sorted with the restricted class first, then by name.
    pub per_class: Vec<ClassStats>,
    /// Ground-truth labels the classifier did not recognize. These are excluded
    /// from the math (counting them would fabricate a class that was present)
    /// and surfaced so the operator can fix their annotation vocabulary — an
    /// unrecognized *truth* label means the eval set is speaking a language the
    /// classifier config doesn't, and every frame carrying it is uncounted.
    pub unrecognized_truth_labels: Vec<String>,
}

impl FnReport {
    /// The stats row for the restricted class, if it appeared in ground truth.
    pub fn restricted(&self) -> Option<&ClassStats> {
        self.per_class
            .iter()
            .find(|c| c.class == self.restricted_class)
    }

    /// A one-line safety headline for the restricted class, honest about small
    /// samples. This is the sentence to put in front of a human.
    pub fn headline(&self) -> String {
        // A row with zero present-frames is as unmeasured as a missing row: the
        // detector was never tested against a real instance of the class.
        match self.restricted().filter(|s| s.present > 0) {
            None => format!(
                "No frames with a truly-present '{}' in this eval set — the \
                 safety-critical miss rate is UNMEASURED (add frames known to \
                 contain one).",
                self.restricted_class
            ),
            Some(s) => {
                let rate = s.miss_rate().unwrap_or(0.0) * 100.0;
                let upper = s.miss_rate_upper95().unwrap_or(1.0) * 100.0;
                let caveat = if s.present < 30 {
                    "  ⚠ sample too small to conclude — widen the eval set."
                } else {
                    ""
                };
                format!(
                    "'{}' miss rate: {:.1}% ({} missed of {} present); 95% upper \
                     bound {:.1}%.{}",
                    self.restricted_class, rate, s.missed, s.present, upper, caveat
                )
            }
        }
    }
}

impl std::fmt::Display for FnReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Detector false-negative report ({} frames)", self.frames)?;
        writeln!(f, "{}", self.headline())?;
        if !self.unrecognized_truth_labels.is_empty() {
            writeln!(
                f,
                "  ⚠ {} unrecognized ground-truth label(s) excluded: {}",
                self.unrecognized_truth_labels.len(),
                self.unrecognized_truth_labels.join(", ")
            )?;
        }
        writeln!(
            f,
            "  {:<12} {:>7} {:>7} {:>6} {:>8} {:>10}",
            "class", "present", "missed", "FP", "missrate", "upper95"
        )?;
        for c in &self.per_class {
            let mr = c
                .miss_rate()
                .map(|r| format!("{:.1}%", r * 100.0))
                .unwrap_or_else(|| "—".into());
            let up = c
                .miss_rate_upper95()
                .map(|r| format!("{:.1}%", r * 100.0))
                .unwrap_or_else(|| "—".into());
            writeln!(
                f,
                "  {:<12} {:>7} {:>7} {:>6} {:>8} {:>10}",
                c.class, c.present, c.missed, c.false_positive, mr, up
            )?;
        }
        Ok(())
    }
}

/// Wilson score interval **upper** bound for `k` successes (here: misses) in `n`
/// trials at quantile `z`. Returns a value in `[0, 1]`. `n == 0` yields `1.0`
/// (nothing is known, so assume the worst).
///
/// Unlike the naive `k/n ± z·sqrt(p(1-p)/n)`, the Wilson interval stays inside
/// `[0, 1]` and is well-behaved at `k == 0` and `k == n`, which is exactly the
/// regime a good detector on a small eval set lives in.
pub fn wilson_upper(k: u64, n: u64, z: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let n = n as f64;
    let p = k as f64 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = p + z2 / (2.0 * n);
    let margin = z * ((p * (1.0 - p) / n) + z2 / (4.0 * n * n)).sqrt();
    ((center + margin) / denom).min(1.0)
}

/// Measure the detector's per-class false-negative rate over a set of evaluation
/// frames, classifying both truth and detector labels onto consent classes via
/// `classifier` (so the measurement matches the level the gate acts on).
///
/// Detector labels the classifier doesn't recognize are treated as the
/// restricted class — because that is exactly what the gate does with them
/// (fail-closed), so an unrecognized detection *does* cover the restricted class
/// for the purpose of "would the gate have caught a person here." Unrecognized
/// *truth* labels are excluded and reported separately (see
/// [`FnReport::unrecognized_truth_labels`]).
pub fn measure(frames: &[EvalFrame], classifier: &SubjectClassifier) -> FnReport {
    use std::collections::BTreeMap;

    // (present, detected_when_present, missed, false_positive) per class.
    let mut acc: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    let mut unrecognized_truth: BTreeSet<String> = BTreeSet::new();
    let restricted = classifier.classify("\u{0}unlikely\u{0}").class; // the fail-closed class

    for frame in frames {
        // Truth classes: recognized labels only; an unrecognized truth label is
        // a data problem, not a class that was present, so we do not invent one.
        let mut truth_classes: BTreeSet<String> = BTreeSet::new();
        for label in &frame.truth {
            let c = classifier.classify(label);
            if c.recognized {
                truth_classes.insert(c.class);
            } else {
                unrecognized_truth.insert(label.clone());
            }
        }
        // Detected classes: an unrecognized detected label maps to the restricted
        // class, mirroring the gate's fail-closed behavior — the gate would treat
        // that detection as the restricted class, so it counts as covering it.
        let detected_classes: BTreeSet<String> = frame
            .detected
            .iter()
            .map(|label| classifier.classify(label).class)
            .collect();

        // Every class mentioned on either side gets a cell this frame.
        let classes: BTreeSet<&String> = truth_classes.iter().chain(detected_classes.iter()).collect();
        for class in classes {
            let e = acc.entry(class.clone()).or_insert((0, 0, 0, 0));
            let in_truth = truth_classes.contains(class);
            let in_det = detected_classes.contains(class);
            match (in_truth, in_det) {
                (true, true) => {
                    e.0 += 1;
                    e.1 += 1;
                } // present + detected (TP)
                (true, false) => {
                    e.0 += 1;
                    e.2 += 1;
                } // present + missed (FN)
                (false, true) => e.3 += 1, // false positive
                (false, false) => {}       // true negative — not tracked
            }
        }
    }

    let mut per_class: Vec<ClassStats> = acc
        .into_iter()
        .map(|(class, (present, tp, missed, fp))| ClassStats {
            class,
            present,
            detected_when_present: tp,
            missed,
            false_positive: fp,
        })
        .collect();
    // Restricted class first, then alphabetical — the safety row leads.
    per_class.sort_by(|a, b| {
        let ra = (a.class != restricted, &a.class);
        let rb = (b.class != restricted, &b.class);
        ra.cmp(&rb)
    });

    FnReport {
        frames: frames.len() as u64,
        restricted_class: restricted,
        per_class,
        unrecognized_truth_labels: unrecognized_truth.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: &str, truth: &[&str], detected: &[&str]) -> EvalFrame {
        EvalFrame {
            frame_id: id.to_string(),
            truth: truth.iter().map(|s| s.to_string()).collect(),
            detected: detected.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn wilson_upper_is_positive_even_with_zero_misses() {
        // The whole point: 0 misses in 20 is not a 0% ceiling.
        let up = wilson_upper(0, 20, Z_95);
        assert!(up > 0.10 && up < 0.20, "0/20 upper bound ~16%, got {up}");
        // And it shrinks as the sample grows.
        assert!(wilson_upper(0, 200, Z_95) < up);
    }

    #[test]
    fn wilson_upper_of_no_data_is_one() {
        assert_eq!(wilson_upper(0, 0, Z_95), 1.0);
    }

    #[test]
    fn perfect_detector_has_zero_miss_rate_but_nonzero_bound() {
        let c = SubjectClassifier::with_defaults();
        // Person present and detected in every frame.
        let frames: Vec<EvalFrame> = (0..10)
            .map(|i| frame(&format!("f{i}"), &["person"], &["person"]))
            .collect();
        let r = measure(&frames, &c);
        let human = r.restricted().expect("human present");
        assert_eq!(human.present, 10);
        assert_eq!(human.missed, 0);
        assert_eq!(human.miss_rate(), Some(0.0));
        assert!(human.miss_rate_upper95().unwrap() > 0.0, "bound must be > 0");
        assert!(r.headline().contains("too small")); // <30 frames
    }

    #[test]
    fn a_missed_person_is_counted_as_a_false_negative() {
        let c = SubjectClassifier::with_defaults();
        let frames = vec![
            frame("f1", &["person"], &["person"]), // caught
            frame("f2", &["person"], &[]),         // MISSED — detector emitted nothing
            frame("f3", &["person"], &["deer"]),   // MISSED — saw a deer, not the person
        ];
        let r = measure(&frames, &c);
        let human = r.restricted().unwrap();
        assert_eq!(human.present, 3);
        assert_eq!(human.detected_when_present, 1);
        assert_eq!(human.missed, 2);
        assert!((human.miss_rate().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn an_unrecognized_detected_label_covers_the_restricted_class() {
        // The gate treats an unrecognized detection as the restricted class
        // (fail-closed), so it counts as catching a present person.
        let c = SubjectClassifier::with_defaults();
        let frames = vec![frame("f1", &["person"], &["drone"])]; // "drone" unrecognized
        let r = measure(&frames, &c);
        let human = r.restricted().unwrap();
        assert_eq!(human.missed, 0, "unrecognized detection covers 'human'");
        assert_eq!(human.detected_when_present, 1);
    }

    #[test]
    fn unrecognized_truth_labels_are_excluded_and_reported() {
        let c = SubjectClassifier::with_defaults();
        // "gremlin" isn't in the vocabulary — it must not fabricate a class.
        let frames = vec![frame("f1", &["gremlin"], &[])];
        let r = measure(&frames, &c);
        assert!(r.per_class.is_empty(), "no class fabricated from a bad label");
        assert_eq!(r.unrecognized_truth_labels, vec!["gremlin".to_string()]);
    }

    #[test]
    fn false_positive_is_recorded_but_is_not_a_miss() {
        let c = SubjectClassifier::with_defaults();
        // No person present, but the detector cried person — over-refusal, not a breach.
        let frames = vec![frame("f1", &[], &["person"])];
        let r = measure(&frames, &c);
        let human = r.restricted().unwrap();
        assert_eq!(human.present, 0);
        assert_eq!(human.false_positive, 1);
        assert_eq!(human.miss_rate(), None); // no present-frames → no rate
        assert!(r.headline().contains("UNMEASURED"));
    }

    #[test]
    fn restricted_class_sorts_first() {
        let c = SubjectClassifier::with_defaults();
        let frames = vec![
            frame("f1", &["deer"], &["deer"]),
            frame("f2", &["person"], &["person"]),
        ];
        let r = measure(&frames, &c);
        assert_eq!(r.per_class[0].class, "human", "safety row leads");
    }

    #[test]
    fn frames_deserialize_from_json() {
        let json = serde_json::json!([
            {"frame_id": "e1", "truth": ["person"], "detected": ["person"]},
            {"truth": ["deer"], "detected": []}
        ]);
        let frames: Vec<EvalFrame> = serde_json::from_value(json).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame_id, "e1");
        assert_eq!(frames[1].frame_id, ""); // defaulted
        let r = measure(&frames, &SubjectClassifier::with_defaults());
        // deer present once, missed once → 100% wildlife miss on n=1 (huge bound).
        let wildlife = r.per_class.iter().find(|c| c.class == "wildlife").unwrap();
        assert_eq!(wildlife.missed, 1);
    }
}

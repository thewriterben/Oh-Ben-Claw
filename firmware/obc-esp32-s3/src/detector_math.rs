//! The frame-difference detector's arithmetic. No esp-idf, no camera, no state.
//!
//! Split from the capture path for the same reason `sensor_math` is split from
//! `sensors` and `identity_map` from `identity`: a rule about what counts as a
//! detection is a rule about judgement, and one that nothing can execute on a
//! host is a rule on trust. `tests/firmware_detector_math.rs` runs these
//! functions against frames the host classifier has already scored, and fails if
//! the two implementations disagree.
//!
//! **This is a second implementation of `scripts/vision/classify.py`, and that is
//! the hazard.** Two copies of one rule is exactly the shape that let `camera.rs`
//! contradict its own `Cargo.toml` for weeks. The Python is the one with 148
//! frames behind it, so it is the reference; this file is the port, and the test
//! compares them on real pixels rather than trusting that they were written from
//! the same paragraph.
//!
//! # Why `frac` and `edge`, and not a brightness score
//!
//! Measured on the bench 2026-09-16 and written up in
//! `docs/VISION-DETECTOR-2026-09.md`. Three things in a room produce a large
//! whole-frame delta and only one is a detection: a lamp switching, the camera
//! being nudged, and a person walking in. Two illumination-normalising scores
//! were proposed from physics and both lost — mean-subtraction had nothing to
//! cancel, and dividing by the mean amplified what it was meant to remove.
//!
//! What worked was localisation and structure: **how many** pixels changed
//! (`frac`) says something is there, and **whether structure changed** (`edge`)
//! says what. A lighting change leaves edges alone; a nudge moves every edge at
//! once.

/// One frame pair's two scores, plus the brightness that decides whether they
/// can be trusted yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scores {
    /// Fraction of pixels whose grey level moved by more than `pixel_eps`.
    pub frac: f32,
    /// Mean absolute change in gradient magnitude.
    pub edge: f32,
    /// Mean grey level of the current frame. Not a score — the warm-up gate's
    /// input, and the number that made frame 000 explicable.
    pub mean: f32,
}

/// What a frame pair is judged to be.
///
/// `Light` and `Nudge` are **reportable states, not detections**. `det=1` for a
/// light switch is a lie, and so is `det=1` for someone bumping the tripod; the
/// honest summary of a repositioned camera is "I cannot compare to before".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Nothing moved.
    Quiet,
    /// Many pixels changed, structure did not. A lamp, the sun, a cloud.
    Light,
    /// Structure changed in a patch. **This is the detection.**
    Motion,
    /// Structure changed everywhere. The camera moved; the past is not comparable.
    Nudge,
}

impl Class {
    /// The wire spelling, kept in one place so the host and the node agree.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Quiet => "quiet",
            Class::Light => "light",
            Class::Motion => "motion",
            Class::Nudge => "nudge",
        }
    }

    /// Whether this class is something happening, as opposed to something
    /// changing. Only `Motion` is.
    pub fn is_detection(self) -> bool {
        matches!(self, Class::Motion)
    }
}

/// The three numbers the rule is made of.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub frac_hi: f32,
    pub edge_light: f32,
    pub edge_nudge: f32,
    pub pixel_eps: f32,
}

/// The host fixture's thresholds — **provisional on this node, by construction.**
///
/// These came from `scripts/vision/classify.py`, measured against 148 frames that
/// made a round trip through the sensor's JPEG encoder and a host-side decode.
/// This node differences the sensor's Y8 output directly and never encodes
/// anything, so it is looking at different pixels: no chroma subsampling, no DCT
/// quantisation, no ringing at edges — and `edge` is precisely the score that
/// would notice ringing.
///
/// Whether that moves the numbers a little or a lot is **not known**, and the
/// 2026-09-17 ADR says so. Carrying them here is how the node produces
/// comparable numbers to measure against; it is not a claim that they are right.
/// Re-measure on-node before anything acts on `Motion`.
pub const HOST_FIXTURE_2026_09_16: Thresholds = Thresholds {
    frac_hi: 0.35,
    edge_light: 4.20,
    edge_nudge: 9.00,
    pixel_eps: 12.0,
};

/// Gradient magnitude at one pixel, by central difference.
///
/// Mirrors `classify.py`'s `grad()` exactly, borders included: the vertical
/// difference is zero on the first and last row and the horizontal difference is
/// zero on the first and last column, because numpy's slice assignment leaves
/// those entries at their initialised zero. That is an artefact of how the
/// reference was written rather than a decision — but the reference is what the
/// thresholds were measured against, so reproducing the artefact is the whole
/// point. Changing it is a change to the detector, not a tidy-up.
#[inline]
fn grad_at(a: &[u8], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let gy = if y > 0 && y + 1 < h {
        a[(y + 1) * w + x] as f32 - a[(y - 1) * w + x] as f32
    } else {
        0.0
    };
    let gx = if x > 0 && x + 1 < w {
        a[y * w + x + 1] as f32 - a[y * w + x - 1] as f32
    } else {
        0.0
    };
    (gx * gx + gy * gy).sqrt()
}

/// Score one frame pair.
///
/// Both slices must be `w * h` bytes of 8-bit grey. Returns `None` rather than
/// scoring a mismatch — a detector fed the wrong geometry should stop, not
/// produce a number that looks like an answer.
///
/// Allocates nothing. Gradients are recomputed per pixel from the two byte
/// slices instead of being buffered, which trades a little arithmetic for not
/// holding 300 KB of floats on a node that has other uses for its PSRAM.
pub fn score(prev: &[u8], cur: &[u8], w: usize, h: usize, pixel_eps: f32) -> Option<Scores> {
    let n = w.checked_mul(h)?;
    if n == 0 || prev.len() != n || cur.len() != n {
        return None;
    }

    let mut changed: u32 = 0;
    let mut edge_sum: f64 = 0.0;
    let mut mean_sum: u64 = 0;

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = cur[i] as f32;
            let p = prev[i] as f32;
            if (c - p).abs() > pixel_eps {
                changed += 1;
            }
            let gc = grad_at(cur, w, h, x, y);
            let gp = grad_at(prev, w, h, x, y);
            edge_sum += (gc - gp).abs() as f64;
            mean_sum += cur[i] as u64;
        }
    }

    let n_f = n as f64;
    Some(Scores {
        frac: (changed as f64 / n_f) as f32,
        edge: (edge_sum / n_f) as f32,
        mean: (mean_sum as f64 / n_f) as f32,
    })
}

/// Mean grey level of one frame.
///
/// `score` already returns this for the current frame, but the very first frame
/// has nothing to score against and still has to feed the warm-up gate —
/// otherwise the node spends its first two frames unable to say whether it is
/// warming up, which is the state the gate exists to report.
pub fn mean(frame: &[u8]) -> Option<f32> {
    if frame.is_empty() {
        return None;
    }
    let sum: u64 = frame.iter().map(|&p| p as u64).sum();
    Some((sum as f64 / frame.len() as f64) as f32)
}

/// Apply the rule. Four lines, and they are the whole detector.
pub fn classify(s: &Scores, t: &Thresholds) -> Class {
    if s.frac <= t.frac_hi {
        return Class::Quiet;
    }
    if s.edge <= t.edge_light {
        return Class::Light;
    }
    if s.edge >= t.edge_nudge {
        return Class::Nudge;
    }
    Class::Motion
}

// ── The warm-up gate ─────────────────────────────────────────────────────────
//
// `baseline_still` frame 000 scored `raw` 80.6 with 87% of pixels changed and
// nothing happening, at brightness 62.9 against a steady 108.7-111.0. That is
// auto-exposure converging after the port-open reset, and a detector armed at
// boot calls it a major event EVERY boot.
//
// So the node withholds judgement until brightness stops moving -- and says that
// it is doing so, rather than silently reporting `quiet`. Reporting `quiet` while
// blind is the silent degradation this project's rules exist to forbid; the
// caller is entitled to know the difference between "nothing happened" and "I
// cannot tell yet".

/// How settled the exposure is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// No previous frame at all. Nothing to compare to.
    NoReference,
    /// Brightness is still moving. Scores exist but must not be classified.
    WarmingUp,
    /// Brightness has been stable for long enough to judge.
    Ready,
}

impl Readiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::NoReference => "no_reference",
            Readiness::WarmingUp => "warming_up",
            Readiness::Ready => "ready",
        }
    }
}

/// Brightness must move less than this, in grey levels, to count as settled.
///
/// **Chosen from the host fixture, not measured on this node.** The empty bench
/// held 108.7-111.0 across 29 frames, a spread of 2.3, while the warm-up frame
/// sat 46 grey levels below. Anything between about 3 and 40 separates those two
/// populations, so the exact value is not delicate — but it is one number from
/// one afternoon in one room, and a darker scene may well drift more. The node
/// reports `mean` on every reply so this can be replaced by a measurement.
pub const SETTLE_DELTA: f32 = 3.0;

/// Consecutive settled deltas required before judging.
///
/// Two, because one is indistinguishable from passing through. Not measured.
pub const SETTLE_RUN: u8 = 2;

/// Tracks exposure settling across frames. `no_std`-friendly; holds no pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct WarmUp {
    last_mean: Option<f32>,
    settled_run: u8,
}

impl WarmUp {
    /// `const` because the firmware keeps one detector state for the whole node,
    /// in a `static Mutex`, and a const constructor is what lets it live there
    /// without a lazy initialiser.
    pub const fn new() -> Self {
        Self {
            last_mean: None,
            settled_run: 0,
        }
    }

    /// Feed one frame's mean brightness; get back whether judgement is allowed.
    pub fn observe(&mut self, mean: f32) -> Readiness {
        let prev = match self.last_mean.replace(mean) {
            None => return Readiness::NoReference,
            Some(p) => p,
        };
        if (mean - prev).abs() < SETTLE_DELTA {
            self.settled_run = self.settled_run.saturating_add(1);
        } else {
            // Not a decay: a real exposure step restarts the count, because the
            // frames either side of it are not comparable.
            self.settled_run = 0;
        }
        if self.settled_run >= SETTLE_RUN {
            Readiness::Ready
        } else {
            Readiness::WarmingUp
        }
    }

    /// Forget the reference. Used after a `Nudge`, where "compare to before" has
    /// stopped meaning anything.
    pub fn reset(&mut self) {
        self.last_mean = None;
        self.settled_run = 0;
    }
}

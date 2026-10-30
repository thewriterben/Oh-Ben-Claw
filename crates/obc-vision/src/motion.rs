//! The frame-difference motion classifier, in Rust.
//!
//! A port of `scripts/vision/classify.py`, which is where the measurements, the
//! refuted alternatives and the honest limits are written down; read that
//! docstring before changing a number here. In one paragraph: a naive
//! `mean|f - prev|` scores a lamp switching on, a camera being nudged and a
//! person walking in all alike, and on the bench the first two are the common
//! case. Two scores separate them: `frac`, how *many* pixels changed (presence),
//! and `edge`, whether *structure* changed (a lamp leaves edges alone, a nudge
//! moves every one of them at once).
//!
//! # Why a port, and why this shape
//!
//! The Python is the instrument the thresholds were measured with, and its
//! `--check` is not in CI, because CI has no numpy or Pillow. This module is the
//! same rule with the same thresholds, checked against the same 148-frame
//! fixture by `cargo test` (`tests/motion_fixture.rs`), so the regression gate
//! runs on every push.
//!
//! It takes greyscale pixel buffers, not JPEGs, and has no dependencies, so
//! the node can include this file by `#[path]` the way the firmware shares
//! other host modules, and classify on the MCU with the code CI tests. Decoding
//! is the caller's business: the fixture test decodes JPEGs; the node would
//! read greyscale from the sensor. The Python docstring warns that thresholds
//! measured on host-decoded JPEG may not carry over to sensor greyscale, so
//! the node needs its own measurement before these constants act there.
//!
//! # What is not here yet
//!
//! The warm-up gate. The fixture's frame 000 is auto-exposure converging after
//! a reset (brightness 62.9 against a steady ~110, 87% of pixels changed), and
//! both the Python and the fixture test simply drop it. A live detector has to
//! withhold judgement until brightness settles and say it is warming up; that
//! needs a stream, not a frame pair, and belongs to whoever owns the stream.

/// Above `light_change`'s max (0.309), well under `person`'s p50 (0.652).
pub const FRAC_HI: f32 = 0.35;
/// Above the baseline max (3.40) and `light_change`'s max (3.28).
pub const EDGE_LIGHT: f32 = 4.20;
/// Between `person`'s max (7.66) and `camera_nudge`'s max (13.11).
pub const EDGE_NUDGE: f32 = 9.00;
/// A pixel counts as changed when it moved by more than this many grey levels.
pub const PIXEL_EPS: f32 = 12.0;

/// What changed between two frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameClass {
    /// Too few pixels changed to be anything.
    Quiet,
    /// Many pixels changed but structure did not: illumination. Reportable,
    /// never a detection.
    Light,
    /// Many pixels and some structure changed: the event the detector is for.
    Motion,
    /// Every edge moved: the camera itself moved. Re-baseline and say so;
    /// not a detection.
    Nudge,
}

impl FrameClass {
    /// Lower-case name, matching the Python's class labels.
    pub fn as_str(self) -> &'static str {
        match self {
            FrameClass::Quiet => "quiet",
            FrameClass::Light => "light",
            FrameClass::Motion => "motion",
            FrameClass::Nudge => "nudge",
        }
    }

    /// Whether this is something to report as an event in the scene. Light
    /// and nudge are states of the *camera's view*, not of the world.
    pub fn is_detection(self) -> bool {
        self == FrameClass::Motion
    }
}

/// The two scores the rule is made of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scores {
    /// Fraction of pixels whose grey level moved by more than [`PIXEL_EPS`].
    pub frac: f32,
    /// Mean absolute change in gradient magnitude.
    pub edge: f32,
}

impl Scores {
    /// The class these scores fall in. The whole rule.
    pub fn classify(self) -> FrameClass {
        if self.frac <= FRAC_HI {
            FrameClass::Quiet
        } else if self.edge <= EDGE_LIGHT {
            FrameClass::Light
        } else if self.edge >= EDGE_NUDGE {
            FrameClass::Nudge
        } else {
            FrameClass::Motion
        }
    }
}

/// Why two buffers could not be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MotionError {
    /// A buffer's length is not `width * height`.
    WrongLength { expected: usize, got: usize },
    /// Width or height is zero.
    Empty,
}

impl std::fmt::Display for MotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MotionError::WrongLength { expected, got } => write!(
                f,
                "frame buffer is {got} bytes, expected width*height = {expected}"
            ),
            MotionError::Empty => write!(f, "frame has zero width or height"),
        }
    }
}

impl std::error::Error for MotionError {}

/// Score the change from `prev` to `cur`, two row-major 8-bit greyscale frames
/// of `width` × `height`.
///
/// The gradient is exactly the Python's `grad()`: the vertical central
/// difference is taken on every row but the first and last, *across all
/// columns*, and the horizontal one on every column but the first and last,
/// *across all rows*; each is zero where it is undefined, and the magnitude is
/// `hypot` of the two. So a border pixel still has the component along the
/// border. (A first draft zeroed the whole border and read edge ~1% low on
/// the fixture; feeding both versions Pillow's own pixels is what showed it.)
/// Means are over every pixel, as numpy's `.mean()` is.
pub fn score(prev: &[u8], cur: &[u8], width: usize, height: usize) -> Result<Scores, MotionError> {
    if width == 0 || height == 0 {
        return Err(MotionError::Empty);
    }
    let n = width * height;
    for buf in [prev, cur] {
        if buf.len() != n {
            return Err(MotionError::WrongLength {
                expected: n,
                got: buf.len(),
            });
        }
    }

    let changed = prev
        .iter()
        .zip(cur)
        .filter(|(&p, &c)| (f32::from(c) - f32::from(p)).abs() > PIXEL_EPS)
        .count();

    let mut edge_sum = 0.0f64;
    for y in 0..height {
        for x in 0..width {
            let g = |buf: &[u8]| {
                let at = |yy: usize, xx: usize| f32::from(buf[yy * width + xx]);
                let gy = if y >= 1 && y + 1 < height {
                    at(y + 1, x) - at(y - 1, x)
                } else {
                    0.0
                };
                let gx = if x >= 1 && x + 1 < width {
                    at(y, x + 1) - at(y, x - 1)
                } else {
                    0.0
                };
                gx.hypot(gy)
            };
            edge_sum += f64::from((g(cur) - g(prev)).abs());
        }
    }

    Ok(Scores {
        frac: (changed as f64 / n as f64) as f32,
        edge: (edge_sum / n as f64) as f32,
    })
}

/// Score and classify in one call.
pub fn classify(
    prev: &[u8],
    cur: &[u8],
    width: usize,
    height: usize,
) -> Result<(FrameClass, Scores), MotionError> {
    let s = score(prev, cur, width, height)?;
    Ok((s.classify(), s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 8;
    const H: usize = 8;

    fn flat(v: u8) -> Vec<u8> {
        vec![v; W * H]
    }

    #[test]
    fn identical_frames_are_quiet_with_zero_scores() {
        let f = flat(100);
        let (class, s) = classify(&f, &f, W, H).unwrap();
        assert_eq!(class, FrameClass::Quiet);
        assert_eq!(s.frac, 0.0);
        assert_eq!(s.edge, 0.0);
    }

    #[test]
    fn a_uniform_brightness_jump_is_light_not_motion() {
        // Every pixel changes, no structure does: the lamp case.
        let (class, s) = classify(&flat(60), &flat(110), W, H).unwrap();
        assert_eq!(s.frac, 1.0);
        assert_eq!(s.edge, 0.0);
        assert_eq!(class, FrameClass::Light);
        assert!(!class.is_detection());
    }

    #[test]
    fn the_rule_boundaries_are_where_the_python_puts_them() {
        let at = |frac, edge| Scores { frac, edge }.classify();
        assert_eq!(at(FRAC_HI, 100.0), FrameClass::Quiet); // `<=` is quiet
        assert_eq!(at(0.9, EDGE_LIGHT), FrameClass::Light); // `<=` is light
        assert_eq!(at(0.9, EDGE_NUDGE), FrameClass::Nudge); // `>=` is nudge
        assert_eq!(at(0.9, 6.0), FrameClass::Motion);
    }

    #[test]
    fn the_gradient_runs_along_the_border_as_numpy_slicing_does() {
        // A single bright pixel in the interior changes the gradient of its
        // four neighbours by 200 each; the mean is over all 64 pixels.
        let prev = flat(0);
        let mut cur = flat(0);
        cur[3 * W + 3] = 200;
        let s = score(&prev, &cur, W, H).unwrap();
        assert!((s.edge - 4.0 * 200.0 / 64.0).abs() < 1e-4, "{s:?}");
        assert!((s.frac - 1.0 / 64.0).abs() < 1e-6);
        // The same pixel on the left border (row 3, column 0). Its interior
        // neighbour (3,1) sees it horizontally, and its two border neighbours
        // (2,0) and (4,0) see it vertically: `gy` is defined on column 0,
        // because the Python slices rows, not the interior. Three, not one.
        let mut cur = flat(0);
        cur[3 * W] = 200;
        let s = score(&prev, &cur, W, H).unwrap();
        assert!((s.edge - 3.0 * 200.0 / 64.0).abs() < 1e-4, "{s:?}");
    }

    #[test]
    fn mismatched_buffers_are_refused_not_misread() {
        assert_eq!(
            score(&flat(0), &[0; 3], W, H),
            Err(MotionError::WrongLength {
                expected: 64,
                got: 3
            })
        );
        assert_eq!(score(&[], &[], 0, 5), Err(MotionError::Empty));
    }
}

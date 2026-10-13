//! Camera health — the camera's own state as a fact a reflex can act on.
//!
//! The telemetry suites (`obc-telemetry`: power, comms, sensing) each judge a
//! raw reading and write a coarse mode that System 1 can watch without knowing
//! the domain. This is that pattern for a camera. It sits here, not there,
//! because the judgement is the [`motion`](super::motion) classifier's, and
//! telemetry does not depend on vision.
//!
//! [`CameraHealth`] takes greyscale frames (and capture failures) for one
//! camera and records `camera.{node}` with a `mode`:
//!
//! | mode | meaning | what to do with it |
//! |---|---|---|
//! | `warming` | just (re)started; auto-exposure is settling | nothing: it is not looking yet |
//! | `ok` | last frame pair was quiet or motion | normal |
//! | `lighting_shift` | the light changed, nothing moved | a state of the view, never a detection |
//! | `nudged` | every edge moved: the camera itself moved | the next pair is against the new view; tell someone |
//! | `blind` | contrast near zero for [`BLIND_RUN`] frames | covered, dark or unfocused: escalate |
//! | `failed` | [`FAILED_RUN`] capture failures in a row | escalate |
//!
//! A motion pair seen while `ok` is also recorded as `camera.{node}.motion`.
//! Lighting changes and nudges are deliberately not events. A detector that
//! reports them is the alarm-fatigue failure `scripts/vision/classify.py`
//! measured its way out of. [`camera_health_rules`] escalates on `blind` and
//! `failed`.
//!
//! # Where the numbers come from
//!
//! - **Warm-up, [`WARMUP_FRAMES`] = 2.** In all four sets of the 2026-09-16
//!   bench fixture, frame 000 is 35–53 grey levels off the frames that follow
//!   (auto-exposure converging after the port-open reset), and frame 001 has
//!   already settled. So the first frame is ignored, the second becomes the
//!   baseline, and classification starts at the third, which is what the
//!   Python does. It is a frame count, not a "brightness has settled" test,
//!   because a person walking in moves mean brightness by up to 28 levels on
//!   the same bench, and such a test would keep re-opening. The fixture was
//!   captured about one frame a second; a much faster capture rate needs its
//!   own measurement.
//! - **Blind, [`BLIND_CONTRAST`] = 10.** The lowest per-frame standard
//!   deviation anywhere in the fixture is 48.5, including the nudge set's
//!   darkest frame (mean 29.8). That makes contrast a safe signal where
//!   brightness is not. **No covered, dark or defocused recording exists**, so
//!   the threshold is a margin below what *was* seen, not a measurement of
//!   what blind looks like. Record a covered lens before trusting it.
//! - **Failed, [`FAILED_RUN`] = 3.** Not measured, chosen. One failed capture
//!   is the bench's normal (see the `reply_dropped` history); three in a row
//!   is not.
//!
//! # What feeds it
//!
//! `oh-ben-claw camera-watch --port <dev> --node <name>` (2026-10-13) holds
//! a USB link to a camera node open (`obc_peripherals::esp32_link`), captures
//! on an interval, and writes `camera.{node}` into the same world memory the
//! agent uses. `[reflex] safing_cameras = ["<name>"]` adds
//! [`camera_health_rules`] to a running agent. Before that command existed,
//! nothing on the host could read a node's pixels; only
//! `scripts/vision/capture_set.py` could. **It has not yet run against a
//! board**: `tests/camera_watch_e2e.rs` drives it over an in-memory node that
//! speaks the firmware's protocol and serves the bench fixture's frames.

use crate::motion::{self, FrameClass, Scores};
use obc_memory::world::WorldMemory;
use obc_reflex::{Action, Condition, ReflexRule};
use serde::Serialize;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Frames ignored or used only as a baseline after a (re)start.
pub const WARMUP_FRAMES: u32 = 2;
/// Per-frame greyscale standard deviation below which a frame has no picture.
pub const BLIND_CONTRAST: f64 = 10.0;
/// Consecutive low-contrast frames before the camera is called blind.
pub const BLIND_RUN: u32 = 3;
/// Consecutive capture failures before the camera is called failed.
pub const FAILED_RUN: u32 = 3;

/// The camera's state, as recorded in `camera.{node}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMode {
    Warming,
    Ok,
    LightingShift,
    Nudged,
    Blind,
    Failed,
}

impl CameraMode {
    /// The `mode` string written to world memory.
    pub fn as_str(self) -> &'static str {
        match self {
            CameraMode::Warming => "warming",
            CameraMode::Ok => "ok",
            CameraMode::LightingShift => "lighting_shift",
            CameraMode::Nudged => "nudged",
            CameraMode::Blind => "blind",
            CameraMode::Failed => "failed",
        }
    }
}

/// What one frame (or failure) did.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraVerdict {
    pub mode: CameraMode,
    /// The frame pair's class, when one was classified (not while warming,
    /// blind or failed).
    pub class: Option<FrameClass>,
    pub scores: Option<Scores>,
    /// Mean grey level of the frame, when there was one.
    pub brightness: Option<f64>,
    /// Grey-level standard deviation of the frame, when there was one.
    pub contrast: Option<f64>,
    /// Whether this frame was recorded as a motion event.
    pub motion_event: bool,
    pub at_ms: u64,
}

#[derive(Default)]
struct Inner {
    frames_since_start: u32,
    prev: Option<(Vec<u8>, usize, usize)>,
    low_contrast_run: u32,
    failure_run: u32,
    failed: bool,
}

/// Judges one camera's frames and records its health.
pub struct CameraHealth {
    node: String,
    world: Option<Arc<WorldMemory>>,
    source: String,
    inner: Mutex<Inner>,
}

impl CameraHealth {
    /// Health for the camera on `node`. Its fact is `camera.{node}`.
    pub fn new(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            world: None,
            source: "camera".to_string(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Record verdicts into world memory.
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// The world-memory entity this camera's health is recorded under.
    pub fn entity(&self) -> String {
        format!("camera.{}", self.node)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The camera restarted (node reboot, sensor re-init, port reopened):
    /// warm up again before judging anything.
    pub fn reset(&self) {
        *self.lock() = Inner::default();
    }

    /// Judge one 8-bit greyscale frame, row-major, `width` × `height`.
    pub fn observe_frame(
        &self,
        grey: &[u8],
        width: usize,
        height: usize,
        now_ms: u64,
    ) -> anyhow::Result<CameraVerdict> {
        if width == 0 || height == 0 || grey.len() != width * height {
            anyhow::bail!(
                "camera frame is {} bytes, expected {width}x{height}",
                grey.len()
            );
        }
        let (brightness, contrast) = mean_and_std(grey);

        let verdict = {
            let mut s = self.lock();
            // Coming back from `failed` is most likely a restart, and a size
            // change certainly is: either way auto-exposure starts over, and
            // the previous frame is no baseline for this one.
            let resized = s
                .prev
                .as_ref()
                .is_some_and(|(_, w, h)| (*w, *h) != (width, height));
            if s.failed || resized {
                *s = Inner::default();
            }
            s.failure_run = 0;
            s.frames_since_start = s.frames_since_start.saturating_add(1);

            let mut class = None;
            let mut scores = None;
            let mode = if contrast < BLIND_CONTRAST {
                s.low_contrast_run += 1;
                if s.low_contrast_run >= BLIND_RUN {
                    CameraMode::Blind
                } else if s.frames_since_start <= WARMUP_FRAMES {
                    CameraMode::Warming
                } else {
                    // Not blind yet, and not a picture to compare either.
                    CameraMode::Ok
                }
            } else {
                s.low_contrast_run = 0;
                // No baseline yet also means warming: a camera that booted
                // covered has counted its frames but never seen a picture, and
                // its first one is the auto-exposure frame all over again.
                if let (true, Some((p, w, h))) =
                    (s.frames_since_start > WARMUP_FRAMES, s.prev.as_ref())
                {
                    let (c, sc) = motion::classify(p, grey, *w, *h)?;
                    class = Some(c);
                    scores = Some(sc);
                    match c {
                        FrameClass::Quiet | FrameClass::Motion => CameraMode::Ok,
                        FrameClass::Light => CameraMode::LightingShift,
                        FrameClass::Nudge => CameraMode::Nudged,
                    }
                } else {
                    CameraMode::Warming
                }
            };
            // A blank frame is no baseline; keep comparing against the last
            // real picture. Frame 000 is kept as the baseline only until the
            // next frame replaces it, which is the warm-up rule.
            if contrast >= BLIND_CONTRAST {
                s.prev = Some((grey.to_vec(), width, height));
            }
            CameraVerdict {
                mode,
                class,
                scores,
                brightness: Some(brightness),
                contrast: Some(contrast),
                motion_event: mode == CameraMode::Ok && class == Some(FrameClass::Motion),
                at_ms: now_ms,
            }
        };
        self.record(&verdict, None)?;
        Ok(verdict)
    }

    /// A capture that produced no frame (`reason` is recorded with it).
    pub fn observe_failure(&self, reason: &str, now_ms: u64) -> anyhow::Result<CameraVerdict> {
        let mode = {
            let mut s = self.lock();
            s.failure_run += 1;
            if s.failure_run >= FAILED_RUN {
                s.failed = true;
            }
            if s.failed {
                CameraMode::Failed
            } else if s.frames_since_start <= WARMUP_FRAMES {
                CameraMode::Warming
            } else {
                // One failure is the bench's normal; keep the last judgement.
                CameraMode::Ok
            }
        };
        let verdict = CameraVerdict {
            mode,
            class: None,
            scores: None,
            brightness: None,
            contrast: None,
            motion_event: false,
            at_ms: now_ms,
        };
        self.record(&verdict, Some(reason))?;
        Ok(verdict)
    }

    /// One capture attempt, as a frame source reports it: JPEG bytes, or why
    /// there are none. A JPEG that will not decode is a failed capture, not
    /// an error: the camera produced nothing usable, and that is what
    /// `failed` is for.
    pub fn observe_capture(
        &self,
        capture: Result<Vec<u8>, String>,
        now_ms: u64,
    ) -> anyhow::Result<CameraVerdict> {
        match capture {
            Ok(jpeg) => match crate::jpeg::decode_grey(&jpeg) {
                Ok(f) => self.observe_frame(&f.pixels, f.width, f.height, now_ms),
                Err(e) => self.observe_failure(&format!("{e:#}"), now_ms),
            },
            Err(reason) => self.observe_failure(&reason, now_ms),
        }
    }

    fn record(&self, v: &CameraVerdict, failure: Option<&str>) -> anyhow::Result<()> {
        let Some(world) = &self.world else {
            return Ok(());
        };
        let failures = self.lock().failure_run;
        let health = json!({
            "mode": v.mode.as_str(),
            "class": v.class.map(FrameClass::as_str),
            "frac": v.scores.map(|s| s.frac),
            "edge": v.scores.map(|s| s.edge),
            "brightness": v.brightness,
            "contrast": v.contrast,
            "consecutive_failures": failures,
            "failure": failure,
        });
        let fact = world.observe(&self.entity(), health, v.at_ms, v.at_ms, &self.source)?;
        if v.motion_event {
            let s = v.scores.expect("a motion event has scores");
            world.observe_derived_from(
                &format!("{}.motion", self.entity()),
                json!({ "frac": s.frac, "edge": s.edge }),
                v.at_ms,
                v.at_ms,
                &self.source,
                &[fact.id],
            )?;
        }
        Ok(())
    }
}

fn mean_and_std(grey: &[u8]) -> (f64, f64) {
    let n = grey.len() as f64;
    let mean = grey.iter().map(|&p| f64::from(p)).sum::<f64>() / n;
    let var = grey
        .iter()
        .map(|&p| (f64::from(p) - mean).powi(2))
        .sum::<f64>()
        / n;
    (mean, var.sqrt())
}

/// What an escalation about a camera tells the reasoner to do.
pub const CAMERA_UNUSABLE_PLAYBOOK: &str = "A camera reports it cannot see (blind: \
no contrast for several frames — covered, dark or out of focus) or cannot capture \
(failed: several captures in a row returned nothing). Its silence is not evidence \
that nothing is happening. Do not treat a quiet scene from it as quiet. Check the \
`camera.<node>` fact for which: `contrast` near zero means look at the lens and \
the light; `consecutive_failures` and `failure` mean look at the node and its \
link. Tell the operator; do not retry captures in a loop.";

/// Reflexes on one camera's health: `blind` or `failed` escalates. Added to a
/// running agent by `[reflex] safing_cameras`.
pub fn camera_health_rules(node: &str, debounce_ms: u64) -> Vec<ReflexRule> {
    let entity = format!("camera.{node}");
    let mode = |m: CameraMode| Condition::State {
        entity: entity.clone(),
        field: Some("mode".to_string()),
        equals: m.as_str().to_string(),
    };
    vec![ReflexRule {
        id: format!("safe-camera-unusable-{node}"),
        when: Condition::Or {
            any: vec![mode(CameraMode::Blind), mode(CameraMode::Failed)],
        },
        then: Action::Escalate {
            reason: format!("camera {node} is unusable. {CAMERA_UNUSABLE_PLAYBOOK}"),
        },
        debounce_ms,
        max_rate_hz: None,
        // Once per transition into the state, not every debounce interval
        // while it lasts: a covered lens is one problem, not one an hour.
        fire_on_change: true,
        hold_ms: 0,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 16;
    const H: usize = 16;

    /// A frame with real contrast: a vertical gradient, shifted by `offset`.
    fn picture(offset: u8) -> Vec<u8> {
        (0..W * H)
            .map(|i| ((i / W) as u8 * 15).wrapping_add(offset))
            .collect()
    }

    fn health() -> (CameraHealth, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        (
            CameraHealth::new("cam0").with_world_memory(Arc::clone(&world)),
            world,
        )
    }

    fn mode_in(world: &WorldMemory) -> String {
        world.current("camera.cam0").unwrap().unwrap().value["mode"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn the_first_two_frames_warm_up_and_the_third_is_judged() {
        let (h, world) = health();
        // Frame 000 is wildly off, as on the bench; it must not become an event.
        let v0 = h.observe_frame(&picture(100), W, H, 1).unwrap();
        let v1 = h.observe_frame(&picture(0), W, H, 2).unwrap();
        let v2 = h.observe_frame(&picture(0), W, H, 3).unwrap();
        assert_eq!(v0.mode, CameraMode::Warming);
        assert_eq!(v1.mode, CameraMode::Warming);
        assert_eq!(v1.class, None, "the second frame is only a baseline");
        assert_eq!(v2.mode, CameraMode::Ok);
        assert_eq!(v2.class, Some(FrameClass::Quiet));
        assert_eq!(mode_in(&world), "ok");
    }

    #[test]
    fn a_covered_lens_goes_blind_after_the_run_and_recovers() {
        let (h, world) = health();
        for t in 0..3 {
            h.observe_frame(&picture(0), W, H, t).unwrap();
        }
        let flat = vec![3u8; W * H];
        let a = h.observe_frame(&flat, W, H, 10).unwrap();
        let b = h.observe_frame(&flat, W, H, 11).unwrap();
        let c = h.observe_frame(&flat, W, H, 12).unwrap();
        assert_eq!((a.mode, b.mode), (CameraMode::Ok, CameraMode::Ok));
        assert_eq!(c.mode, CameraMode::Blind);
        assert_eq!(mode_in(&world), "blind");
        // Uncovered: compared against the last real picture, not the blank.
        let back = h.observe_frame(&picture(0), W, H, 13).unwrap();
        assert_eq!(back.mode, CameraMode::Ok);
        assert_eq!(back.class, Some(FrameClass::Quiet));
    }

    #[test]
    fn failures_must_run_before_failed_and_recovery_rewarms() {
        let (h, world) = health();
        for t in 0..3 {
            h.observe_frame(&picture(0), W, H, t).unwrap();
        }
        assert_eq!(
            h.observe_failure("timeout", 10).unwrap().mode,
            CameraMode::Ok
        );
        assert_eq!(
            h.observe_failure("timeout", 11).unwrap().mode,
            CameraMode::Ok
        );
        assert_eq!(
            h.observe_failure("timeout", 12).unwrap().mode,
            CameraMode::Failed
        );
        let fact = world.current("camera.cam0").unwrap().unwrap();
        assert_eq!(fact.value["mode"], "failed");
        assert_eq!(fact.value["consecutive_failures"], 3);
        assert_eq!(fact.value["failure"], "timeout");
        // The next frame is probably a restarted camera: warm up again.
        assert_eq!(
            h.observe_frame(&picture(0), W, H, 13).unwrap().mode,
            CameraMode::Warming
        );
    }

    #[test]
    fn a_camera_that_boots_covered_warms_up_on_its_first_picture() {
        // Past the warm-up count, but nothing to compare against: the first
        // real picture is a baseline, not a classification (and not a panic).
        let (h, _) = health();
        let flat = vec![0u8; W * H];
        for t in 0..2 {
            assert_eq!(
                h.observe_frame(&flat, W, H, t).unwrap().mode,
                CameraMode::Warming
            );
        }
        assert_eq!(
            h.observe_frame(&flat, W, H, 2).unwrap().mode,
            CameraMode::Blind
        );
        let first = h.observe_frame(&picture(0), W, H, 3).unwrap();
        assert_eq!(first.mode, CameraMode::Warming);
        assert_eq!(first.class, None);
        let next = h.observe_frame(&picture(0), W, H, 4).unwrap();
        assert_eq!(next.class, Some(FrameClass::Quiet));
    }

    #[test]
    fn a_size_change_is_a_restart_not_a_comparison() {
        let (h, _) = health();
        for t in 0..3 {
            h.observe_frame(&picture(0), W, H, t).unwrap();
        }
        let small: Vec<u8> = picture(0)[..8 * H].to_vec();
        let v = h.observe_frame(&small, 8, H, 4).unwrap();
        assert_eq!(v.mode, CameraMode::Warming);
        assert!(h.observe_frame(&[0; 3], W, H, 5).is_err());
    }

    #[test]
    fn only_motion_while_ok_is_recorded_as_an_event() {
        let (h, world) = health();
        // A uniform brightness jump is light, never an event.
        for t in 0..3 {
            h.observe_frame(&picture(0), W, H, t).unwrap();
        }
        let lit: Vec<u8> = picture(0).iter().map(|p| p.saturating_add(40)).collect();
        let v = h.observe_frame(&lit, W, H, 4).unwrap();
        assert_eq!(v.mode, CameraMode::LightingShift);
        assert!(!v.motion_event);
        assert!(world.current("camera.cam0.motion").unwrap().is_none());
    }

    #[test]
    fn the_rule_escalates_on_blind_and_failed_once_per_transition() {
        let (h, world) = health();
        let engine = obc_reflex::ReflexEngine::new(camera_health_rules("cam0", 0));
        for t in 0..3 {
            h.observe_frame(&picture(0), W, H, t).unwrap();
        }
        assert!(
            engine.tick(&world, 3).unwrap().is_empty(),
            "ok must not fire"
        );
        let flat = vec![0u8; W * H];
        for t in 10..13 {
            h.observe_frame(&flat, W, H, t).unwrap();
        }
        let fired = engine.tick(&world, 13).unwrap();
        assert_eq!(fired.len(), 1);
        assert!(
            matches!(&fired[0].action, Action::Escalate { reason } if reason.contains("cam0")),
            "{fired:?}"
        );
        // Same evidence, later tick: not again.
        assert!(engine.tick(&world, 20).unwrap().is_empty());
    }
}

//! Camera health over the real bench recordings, frame 000 included.
//!
//! The motion fixture test discards frame 000 by hand, the way the Python
//! does. Here nothing is discarded: each recorded sequence is fed to
//! `CameraHealth` exactly as captured, so the warm-up gate has to handle the
//! auto-exposure frame itself, and the blind threshold has to stay quiet on
//! every real picture the bench has produced.

use obc_memory::world::WorldMemory;
use obc_vision::camera_health::{CameraHealth, CameraMode};
use std::collections::BTreeMap;
use std::sync::Arc;

mod common;
use common::{frame_files, grey};

struct Run {
    modes: Vec<CameraMode>,
    motion_events: usize,
    first_event_frame: Option<usize>,
}

fn run(label: &str) -> Run {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let health = CameraHealth::new(label).with_world_memory(Arc::clone(&world));
    let mut modes = Vec::new();
    let mut motion_events = 0;
    let mut first_event_frame = None;
    for (i, path) in frame_files(label).iter().enumerate() {
        let (g, w, h) = grey(path);
        let v = health.observe_frame(&g, w, h, i as u64 * 800).unwrap();
        if v.motion_event {
            motion_events += 1;
            first_event_frame.get_or_insert(i);
        }
        modes.push(v.mode);
    }
    // What a reflex would read is what the last frame said.
    let fact = world.current(&format!("camera.{label}")).unwrap().unwrap();
    assert_eq!(fact.value["mode"], modes.last().unwrap().as_str());
    Run {
        modes,
        motion_events,
        first_event_frame,
    }
}

fn tally(modes: &[CameraMode]) -> BTreeMap<&'static str, usize> {
    let mut t = BTreeMap::new();
    for m in modes {
        *t.entry(m.as_str()).or_default() += 1;
    }
    t
}

#[test]
fn the_auto_exposure_frame_never_becomes_an_event() {
    for label in ["baseline_still", "light_change", "person", "camera_nudge"] {
        let r = run(label);
        assert_eq!(
            &r.modes[..2],
            &[CameraMode::Warming, CameraMode::Warming],
            "{label}"
        );
        assert!(
            r.first_event_frame.is_none_or(|i| i >= 2),
            "{label}: event at frame {:?}",
            r.first_event_frame
        );
    }
}

#[test]
fn no_real_picture_reads_as_blind() {
    for label in ["baseline_still", "light_change", "person", "camera_nudge"] {
        let r = run(label);
        assert!(
            !r.modes.contains(&CameraMode::Blind),
            "{label}: {:?}",
            tally(&r.modes)
        );
    }
}

#[test]
fn a_quiet_room_and_a_light_switch_raise_no_events() {
    for label in ["baseline_still", "light_change"] {
        let r = run(label);
        assert_eq!(r.motion_events, 0, "{label}: {:?}", tally(&r.modes));
        assert!(!r.modes.contains(&CameraMode::Nudged), "{label}");
    }
}

/// The counts follow the classifier's (`scores.txt`): 20 motion pairs in the
/// person set, 8 nudges in the nudge set. `camera_nudge/024.jpg` is the one
/// decoder-dependent frame (light under Pillow, motion here); it is a
/// `lighting_shift`-or-`ok` frame either way, never a nudge, so the nudge
/// count does not depend on it.
#[test]
fn a_person_raises_events_and_a_nudge_says_nudged() {
    let person = run("person");
    assert_eq!(person.motion_events, 20, "{:?}", tally(&person.modes));
    let nudge = run("camera_nudge");
    assert_eq!(
        tally(&nudge.modes).get("nudged"),
        Some(&8),
        "{:?}",
        tally(&nudge.modes)
    );
}

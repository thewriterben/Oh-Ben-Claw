//! A camera detection must become an obstacle the planner routes around.
//!
//! `vision/clawcam_spatial.rs` was one of three files the reachability sweep
//! reported as unwired: complete, tested, referenced by nothing. Its four unit
//! tests exercise geometry — points inside a disc, an unknown camera marking
//! nothing — and never touch a `NavController`. Every one of them could pass
//! while the module was incapable of affecting navigation, which is precisely
//! what it was for as long as it existed.
//!
//! So this drives a real controller and a real grid, and asserts the property
//! the module exists for: a cell the planner could route through stops being
//! one. The geometry is already covered upstairs and is not repeated here.

use std::sync::{Arc, Mutex};

use oh_ben_claw::movement::{LoggingActuatorSink, MovementController};
use oh_ben_claw::navigation::{planning::OccupancyGrid, NavController};
use oh_ben_claw::security::limits::{SafetyGate, SafetyLimit};
use oh_ben_claw::vision::clawcam_spatial::{hazard_points, mark_detection_hazard, CameraMap};

fn nav_with_grid() -> (Arc<NavController>, Arc<Mutex<OccupancyGrid>>) {
    // The gate is never exercised here — nothing actuates — but MovementController
    // requires one, and Track 0 is not the sort of thing to stub out with a
    // permissive wildcard even in a test that will not reach it.
    let mut steer = SafetyLimit::new("rover", "servo_angle");
    steer.allowed_pins = Some(vec![0]);
    steer.value_min = Some(-90);
    steer.value_max = Some(90);
    let gate = SafetyGate::new(vec![steer]);
    let movement = Arc::new(MovementController::new(
        "rover",
        Arc::new(gate),
        Arc::new(LoggingActuatorSink),
    ));
    let grid = Arc::new(Mutex::new(OccupancyGrid::new(0.0, 0.0, 0.25, 40, 40)));
    let nav = Arc::new(
        NavController::new(movement, ("steer".into(), 0), ("drive".into(), 1))
            .with_grid(Arc::clone(&grid)),
    );
    (nav, grid)
}

#[test]
fn a_detection_turns_free_cells_into_obstacles() {
    let (nav, grid) = nav_with_grid();

    // Control: the area is clear before the detection, so "occupied after" is a
    // statement about the hazard rather than about the fixture.
    assert_eq!(
        grid.lock().unwrap().occupied_count(),
        0,
        "the grid started occupied; marking it would prove nothing"
    );

    let mut map = CameraMap::new();
    map.set("cam-1", 2.0, 2.0);

    let cells = mark_detection_hazard(&nav, &map, "cam-1", 0.6, 0.25);
    assert!(
        cells > 0,
        "no cells marked — the hazard never reached the grid"
    );
    assert_eq!(
        grid.lock().unwrap().occupied_count(),
        cells,
        "the count returned disagrees with what actually landed in the grid"
    );
}

/// An unknown camera marks nothing rather than guessing a position. Failing this
/// way round matters: a hazard in the wrong place is worse than no hazard.
#[test]
fn an_unmapped_camera_marks_nothing() {
    let (nav, grid) = nav_with_grid();
    let map = CameraMap::new();
    assert_eq!(
        mark_detection_hazard(&nav, &map, "cam-unknown", 1.0, 0.25),
        0
    );
    assert_eq!(grid.lock().unwrap().occupied_count(), 0);
}

/// The hazard is a disc, not its bounding box.
#[test]
fn the_hazard_is_round_not_square() {
    let pts = hazard_points((0.0, 0.0), 1.0, 0.5);
    assert!(pts.contains(&(0.0, 0.0)));
    assert!(
        !pts.contains(&(1.0, 1.0)),
        "the bounding-box corner is inside the disc, so this is a square"
    );
}

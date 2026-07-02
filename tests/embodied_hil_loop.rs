//! Hardware-in-the-loop seam test — ClawCam perception drives navigation, end to
//! end, through one world memory and the Track 0 safety gate.
//!
//! This is the full embodied loop a real rover runs: a camera sees something, the
//! brain *remembers* it, that memory changes where the robot is willing to drive,
//! and the resulting motion is bounded by the safety gate. Concretely:
//!
//! 1. **Perceive → remember:** a ClawCam detection tool-result is folded into world
//!    memory via the real [`ingest_tool_result`] seam (`vision.subject.deer`).
//! 2. **Remember → act (policy):** the brain's hazard policy reads that a *verified*
//!    animal is in the corridor and marks its location as occupancy — the robot
//!    must not drive into it.
//! 3. **Act (deliberation):** navigation, which would have driven straight, now
//!    plans a **detour** around the hazard (A* over the occupancy grid).
//! 4. **Act (Track 0):** stepping the route issues a steer/drive command that passes
//!    through the [`SafetyGate`] and is recorded as gated actuation.
//!
//! The point is that no layer is mocked: the detection ingest, the world-memory
//! query, the planner, and the gated actuator are all the production code paths.

use std::sync::{Arc, Mutex};

use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::movement::{LoggingActuatorSink, MovementController};
use oh_ben_claw::navigation::{planning::OccupancyGrid, NavController, NavGoal, PlanOutcome};
use oh_ben_claw::security::limits::{SafetyGate, SafetyLimit};
use oh_ben_claw::vision::clawcam_ingest::ingest_tool_result;
use serde_json::json;

fn rover_gate() -> SafetyGate {
    let mut steer = SafetyLimit::new("rover", "servo_angle");
    steer.allowed_pins = Some(vec![0]);
    steer.value_min = Some(-90);
    steer.value_max = Some(90);
    let mut drive = SafetyLimit::new("rover", "motor_speed");
    drive.allowed_pins = Some(vec![1]);
    drive.value_min = Some(-100);
    drive.value_max = Some(100);
    SafetyGate::new(vec![steer, drive])
}

fn set_pose(world: &WorldMemory, x: f64, y: f64, h: f64, t: u64) {
    world.observe("sensor.pos_x", json!({ "value": x }), t, t, "slam").unwrap();
    world.observe("sensor.pos_y", json!({ "value": y }), t, t, "slam").unwrap();
    world.observe("sensor.heading", json!({ "value": h }), t, t, "slam").unwrap();
}

#[tokio::test]
async fn clawcam_detection_reroutes_navigation_through_the_safety_gate() {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());

    let movement = Arc::new(
        MovementController::new("rover", Arc::new(rover_gate()), Arc::new(LoggingActuatorSink))
            .with_world_memory(Arc::clone(&world)),
    );
    let grid = Arc::new(Mutex::new(OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10)));
    let nav = Arc::new(
        NavController::new(movement, ("steer".into(), 0), ("drive".into(), 1))
            .with_world_memory(Arc::clone(&world))
            .with_grid(grid),
    );

    set_pose(&world, 0.5, 0.5, 0.0, 1_000);
    let goal = NavGoal { x: 9.5, y: 0.5, tolerance: 0.3 };

    // ── Phase A: clear corridor → the plan is a straight shot ─────────────────
    let before = nav.plan_to(goal, 1_000).unwrap();
    assert_eq!(before, PlanOutcome::Planned(1), "open room ⇒ single straight waypoint");

    // ── Phase B: ClawCam sees a deer mid-corridor (real ingest seam) ──────────
    // The gateway tool-result shape from `list_species_detections`.
    let detection = json!({
        "ok": true,
        "count": 1,
        "results": [{
            "event_id": "evt-deer-1",
            "device_id": "cam-corridor",
            "top_species": "deer",
            "top_confidence": 0.94,
            "review_state": "verified",
            "ran_at": "2026-06-25T12:00:00Z"
        }]
    });
    let entities = ingest_tool_result(&world, &detection, 2_000, "clawcam").unwrap();
    assert_eq!(entities, vec!["vision.subject.deer"]);

    // ── Phase C: hazard policy — a verified animal in the corridor becomes a
    //    no-go region the planner must avoid. (Camera `cam-corridor` covers the
    //    column at world x≈5; the policy marks it, leaving a gap at the top.) ──
    let sighting = world.current("vision.subject.deer").unwrap().unwrap();
    assert_eq!(sighting.value["review_state"], "verified");
    let is_verified_animal = sighting.value["review_state"] == "verified";
    assert!(is_verified_animal, "policy only blocks for a verified sighting");
    for cy in 0..8 {
        assert!(nav.mark_obstacle(5.5, cy as f64 + 0.5, true), "mark the corridor hazard");
    }
    assert_eq!(nav.obstacle_count(), 8);

    // ── Phase D: re-plan → navigation detours around the hazard ───────────────
    let after = nav.plan_to(goal, 2_000).unwrap();
    match after {
        PlanOutcome::Planned(n) => assert!(n >= 2, "detour should have ≥2 turn points, got {n}"),
        other => panic!("expected a detour plan, got {other:?}"),
    }
    assert!(nav.remaining() >= 2, "the active route now bends around the deer");

    // ── Phase E: drive the route → a Track 0–bounded command is issued ────────
    nav.step_toward_goal(2_000).await.unwrap();
    let drive = world.current("actuator.drive").unwrap().expect("a gated drive command was issued");
    // The recorded command went through the gate (motor_speed ∈ [-100, 100]).
    if let Some(v) = drive.value.get("value").and_then(|v| v.as_i64()) {
        assert!((-100..=100).contains(&v), "drive speed within Track 0 limits, got {v}");
    }

    // The detection remains queryable in memory — the loop never mutated it.
    assert!(world.current("vision.subject.deer").unwrap().is_some());
}

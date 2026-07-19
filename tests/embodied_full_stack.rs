//! Grand full-stack integration test — the whole embodied control stack in one
//! scenario, exercised as a unit.
//!
//! A rover is given a mission to drive across a room with a wall in it. We verify
//! the four control layers compose through one shared world memory:
//!
//! 1. **Perception → memory:** the power suite records `power.mode`; the occupancy
//!    map records an obstacle.
//! 2. **Deliberation (mission):** the mission plans an *obstacle-aware* route
//!    (navigation + A*), and issues a Track 0–bounded steer/drive command.
//! 3. **Reflex + safing:** as the battery drains, the safing reflex engages
//!    in-process load-shedding; at critical the mission **guard preempts** and
//!    halts the platform.
//! 4. **Recovery:** on recharge, safing releases automatically.

use std::sync::{Arc, Mutex};

use oh_ben_claw::agent::reflex::{
    ActionSink, Condition, LoggingActionSink, ReflexController, ReflexEngine,
};
use oh_ben_claw::agent::safing::{standard_safing_rules, SafingOptions, SafingSink, SafingState};
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::mission::{Guard, Mission, MissionRunner, MissionStatus, MissionStep};
use oh_ben_claw::movement::{LoggingActuatorSink, MovementController};
use oh_ben_claw::navigation::{planning::OccupancyGrid, NavController};
use oh_ben_claw::power::{BatteryReading, ChargeState, PowerController, PowerThresholds};
use oh_ben_claw::security::limits::{SafetyGate, SafetyLimit};
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
    let mut stop = SafetyLimit::new("rover", "stop");
    stop.allowed_pins = Some(vec![1]);
    stop.value_min = Some(0);
    stop.value_max = Some(0);
    SafetyGate::new(vec![steer, drive, stop])
}

fn set_pose(world: &WorldMemory, x: f64, y: f64, h: f64, t: u64) {
    world.observe("sensor.pos_x", json!({ "value": x }), t, t, "slam").unwrap();
    world.observe("sensor.pos_y", json!({ "value": y }), t, t, "slam").unwrap();
    world.observe("sensor.heading", json!({ "value": h }), t, t, "slam").unwrap();
}

fn battery(soc: f64, charging: ChargeState) -> BatteryReading {
    BatteryReading { soc_pct: soc, voltage: None, current_a: None, charging, source: None }
}

#[tokio::test]
async fn full_stack_mission_runs_then_safing_preempts_and_recovers() {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());

    // ── Movement + navigation (with an occupancy grid) ────────────────────────
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

    // ── Power suite ───────────────────────────────────────────────────────────
    let power =
        PowerController::new(PowerThresholds::default()).with_world_memory(Arc::clone(&world));

    // ── Reflex + safing (in-process load-shedding) ────────────────────────────
    let safing_state = Arc::new(SafingState::new());
    let opts = SafingOptions { debounce_ms: 1, ..Default::default() };
    let engine = ReflexEngine::new(standard_safing_rules(&opts));
    let sink: Arc<dyn ActionSink> =
        Arc::new(SafingSink::new(Arc::clone(&safing_state), Arc::new(LoggingActionSink)));
    let reflex = ReflexController::new(engine, Arc::clone(&world), sink);

    // ── Mission sequencer (deliberation), guarded on battery critical ─────────
    let mission_runner = Arc::new(MissionRunner::new(Arc::clone(&world)).with_nav(Arc::clone(&nav)));
    let mission = Mission {
        id: "cross-room".into(),
        steps: vec![
            MissionStep::NavigateTo { x: 9.5, y: 0.5, tolerance: 0.3 },
            MissionStep::Record { entity: "mission.reached".into(), value: json!(true) },
        ],
        guards: vec![Guard {
            abort_when: Condition::State {
                entity: "power.mode".into(),
                field: Some("mode".into()),
                equals: "critical".into(),
            },
            reason: "battery critical — abort mission".into(),
        }],
    };

    // A wall down column x≈5 (rows 0..7), leaving a gap near the top.
    for cy in 0..8 {
        assert!(nav.mark_obstacle(5.5, cy as f64 + 0.5, true));
    }
    set_pose(&world, 0.5, 0.5, 0.0, 1_000);

    // ── Phase A: healthy battery, mission starts and plans around the wall ────
    power.ingest(&battery(85.0, ChargeState::Discharging), 1_000, oh_ben_claw::memory::world::Origin::Observed).unwrap();
    reflex.tick_and_dispatch(1_000).await.unwrap();
    assert!(!safing_state.shed_load(), "no shedding while healthy");

    mission_runner.start(mission);
    let status = mission_runner.tick(1_000).await.unwrap(); // enters NavigateTo → plans
    assert!(
        matches!(status, MissionStatus::Running { ref label, .. } if label == "navigate_to"),
        "mission should be navigating, got {status:?}"
    );
    assert!(nav.remaining() >= 2, "the route should detour around the wall");

    // A real, Track 0–bounded steer/drive command is issued toward the route.
    nav.step_toward_goal(1_000).await.unwrap();
    assert!(world.current("actuator.drive").unwrap().is_some(), "gated actuation occurred");

    // ── Phase B: battery low → safing engages in-process load-shedding ────────
    power.ingest(&battery(15.0, ChargeState::Discharging), 2_000, oh_ben_claw::memory::world::Origin::Observed).unwrap();
    reflex.tick_and_dispatch(2_000).await.unwrap();
    assert!(safing_state.shed_load(), "low battery engages shed_load");
    // mission keeps running — low is not yet the abort condition
    let s = mission_runner.tick(2_000).await.unwrap();
    assert!(matches!(s, MissionStatus::Running { .. }), "mission still running at low, got {s:?}");

    // ── Phase C: battery critical → mission guard preempts + halts ────────────
    power.ingest(&battery(5.0, ChargeState::Discharging), 3_000, oh_ben_claw::memory::world::Origin::Observed).unwrap();
    reflex.tick_and_dispatch(3_000).await.unwrap(); // power-critical escalation fires
    let s = mission_runner.tick(3_000).await.unwrap();
    assert!(
        matches!(s, MissionStatus::Aborted { ref reason, .. } if reason.contains("battery")),
        "guard should abort the mission, got {s:?}"
    );
    assert!(nav.current_goal().is_none(), "abort halts navigation (goal cleared)");
    assert!(world.current("mission.reached").unwrap().is_none(), "mission never reached its goal");

    // ── Phase D: recharge → safing recovers automatically ─────────────────────
    power.ingest(&battery(95.0, ChargeState::Charging), 4_000, oh_ben_claw::memory::world::Origin::Observed).unwrap();
    reflex.tick_and_dispatch(4_000).await.unwrap();
    assert!(!safing_state.shed_load(), "recharge releases shed_load");
}

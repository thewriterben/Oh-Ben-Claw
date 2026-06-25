//! End-to-end embodied safing loop.
//!
//! Exercises the whole spine as a unit — perception suites → bitemporal world
//! memory → System 1 reflexes → in-process safing state — rather than testing any
//! one module. A battery drain (power suite) and a network loss (comms suite)
//! each flow through real controllers into world memory; the reflex controller
//! ticks, fires the standard safing rules, and a `SafingSink` flips the shared
//! `SafingState` the host's load-shedding consumers read. The scenarios then
//! restore the modes and assert safing **auto-recovers**.

use std::sync::Arc;

use oh_ben_claw::agent::reflex::{ActionSink, LoggingActionSink, ReflexController, ReflexEngine};
use oh_ben_claw::agent::safing::{standard_safing_rules, SafingOptions, SafingSink, SafingState};
use oh_ben_claw::comms::{CommsController, LinkReading, LinkThresholds};
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::power::{BatteryReading, ChargeState, PowerController, PowerThresholds};

fn battery(soc_pct: f64, charging: ChargeState) -> BatteryReading {
    BatteryReading { soc_pct, voltage: None, current_a: None, charging, source: None }
}

fn link(name: &str, up: bool, latency_ms: f64) -> LinkReading {
    LinkReading {
        link: name.to_string(),
        rssi_dbm: None,
        latency_ms: Some(latency_ms),
        loss_pct: None,
        up: Some(up),
        source: None,
    }
}

/// Build a reflex controller whose sink taps safing advisories into `state`.
fn controller(world: &Arc<WorldMemory>, state: &Arc<SafingState>) -> ReflexController {
    let opts = SafingOptions { debounce_ms: 1, ..Default::default() };
    let engine = ReflexEngine::new(standard_safing_rules(&opts));
    let sink: Arc<dyn ActionSink> =
        Arc::new(SafingSink::new(Arc::clone(state), Arc::new(LoggingActionSink)));
    ReflexController::new(engine, Arc::clone(world), sink)
}

#[tokio::test]
async fn battery_drain_engages_then_recovers_shed_load() {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let power = PowerController::new(PowerThresholds::default()).with_world_memory(Arc::clone(&world));
    let state = Arc::new(SafingState::new());
    let ctl = controller(&world, &state);

    // Healthy charge → nothing shed (the ClawCam poll keeps running).
    power.ingest(&battery(85.0, ChargeState::Discharging), 1_000).unwrap();
    ctl.tick_and_dispatch(1_000).await.unwrap();
    assert!(!state.shed_load(), "no shedding at healthy charge");

    // Drain to low → power.mode=low → safe-power-low → shed_load engages
    // (the poll would now back off).
    power.ingest(&battery(15.0, ChargeState::Discharging), 2_000).unwrap();
    ctl.tick_and_dispatch(2_000).await.unwrap();
    assert!(state.shed_load(), "shed_load engages at low charge");

    // Recharge to normal → power.mode=charging → safe-power-recovered →
    // shed_load releases automatically (load resumes).
    power.ingest(&battery(95.0, ChargeState::Charging), 3_000).unwrap();
    ctl.tick_and_dispatch(3_000).await.unwrap();
    assert!(!state.shed_load(), "shed_load auto-recovers once charge returns");
}

#[tokio::test]
async fn network_loss_engages_then_recovers_net_safing() {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let comms = CommsController::new(LinkThresholds::default()).with_world_memory(Arc::clone(&world));
    let state = Arc::new(SafingState::new());
    let ctl = controller(&world, &state);

    // Link up and healthy → online, no net safing.
    comms.ingest(&link("wifi", true, 30.0), 1_000).unwrap();
    ctl.tick_and_dispatch(1_000).await.unwrap();
    assert!(!state.offline() && !state.degraded_net(), "online: no net safing");

    // Link drops → net.mode=offline → safe-net-offline → degraded + offline engage.
    comms.ingest(&link("wifi", false, 0.0), 2_000).unwrap();
    ctl.tick_and_dispatch(2_000).await.unwrap();
    assert!(state.offline() && state.degraded_net(), "offline engages net safing");

    // Link restored → net.mode=online → safe-net-recovered → flags release.
    comms.ingest(&link("wifi", true, 25.0), 3_000).unwrap();
    ctl.tick_and_dispatch(3_000).await.unwrap();
    assert!(!state.offline() && !state.degraded_net(), "net safing auto-recovers");
}

#[tokio::test]
async fn independent_subsystems_do_not_cross_clear() {
    // Power shedding and net safing are independent: recovering the network must
    // not clear a power-driven shed (and vice-versa).
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let power = PowerController::new(PowerThresholds::default()).with_world_memory(Arc::clone(&world));
    let comms = CommsController::new(LinkThresholds::default()).with_world_memory(Arc::clone(&world));
    let state = Arc::new(SafingState::new());
    let ctl = controller(&world, &state);

    // Low battery (shed) AND a healthy link (online) at the same tick.
    power.ingest(&battery(12.0, ChargeState::Discharging), 1_000).unwrap();
    comms.ingest(&link("wifi", true, 20.0), 1_000).unwrap();
    ctl.tick_and_dispatch(1_000).await.unwrap();

    // Net recovery fired (link online) but power is still low → shed stays on.
    assert!(state.shed_load(), "power shed persists while battery is low");
    assert!(!state.offline(), "healthy link keeps net clear");
}

//! Where a reading came from decides whether it may actuate.
//!
//! This lived inside `agent/reflex.rs`'s test module until 2026-08-12, and it
//! was the last thing keeping that file tied to a module still in this tree —
//! every other name in reflex.rs already resolved to an extracted crate. It
//! constructs `tools::builtin::sensing::SenseTool` because the claim it makes
//! spans the tool layer and the reflex gate: an agent-*reported* temperature
//! must not actuate, a driver-*measured* one must.
//!
//! That is an integration test that had been wearing a unit test's clothes, and
//! it only ever compiled in place because everything happened to live in one
//! crate. Here it can keep naming both sides after `obc-reflex` leaves.

use std::sync::Arc;

use obc_tool_api::Tool;
use oh_ben_claw::agent::reflex::{Action, Cmp, Condition, ReflexEngine, ReflexRule};
use oh_ben_claw::memory::world::{Origin, WorldMemory};
use oh_ben_claw::sensing::{QuantitySpec, Sample, SensingController};
use oh_ben_claw::tools::builtin::sensing::SenseTool;
use serde_json::json;

/// Fan on above 28 degrees. Inlined rather than shared: the helper it came from
/// stayed behind in the crate, and a test that quietly depends on someone
/// else's fixture is a test that breaks for reasons it does not describe.
fn fan_rule() -> ReflexRule {
    ReflexRule {
        id: "fan-on-hot".to_string(),
        when: Condition::Sensor {
            entity: "sensor.temperature".to_string(),
            op: Cmp::Gt,
            value: 28.0,
        },
        then: Action::GpioWrite {
            node_id: "node-1".to_string(),
            pin: 18,
            value: 1,
        },
        debounce_ms: 500,
        max_rate_hz: None,
        fire_on_change: false,
    }
}

#[tokio::test]
async fn content_relayed_by_a_trusted_writer_is_now_gated() {
    // This test previously asserted the OPPOSITE — it pinned the gap while the ingest
    // boundary was still open, and said in its own comment that it should fail when
    // the boundary moved. It did, and this is that update.
    //
    // The distinction the gate needs is not visible in `source`: `sensing` writes
    // under its own label whichever path fed it. It is visible in `origin`, because
    // the `sense` tool — being the agent's boundary by construction — now says so.

    // Two stores, deliberately. `SenseTool` stamps the real clock, so a
    // driver-path write at a small fixed timestamp would land *before* it and
    // `current()` would keep returning the tool's fact — the test would pass or fail
    // for reasons having nothing to do with the gate. Separate worlds keep each half
    // honest about what it is measuring.
    let make = || {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = Arc::new(
            SensingController::new(vec![("temperature".to_string(), QuantitySpec::default())])
                .with_world_memory(Arc::clone(&world)),
        );
        (world, ctrl)
    };

    // Through the real tool: an agent says it is 30 degrees.
    let (agent_world, agent_ctrl) = make();
    let tool = SenseTool::new(Arc::clone(&agent_ctrl), Arc::clone(&agent_world));
    tool.execute(json!({ "action": "ingest", "quantity": "temperature", "value": 30.0 }))
        .await
        .unwrap();
    let fact = agent_world.current("sensor.temperature").unwrap().unwrap();
    assert_eq!(
        fact.origin,
        Origin::Asserted,
        "the tool is the agent boundary — what arrives there is a claim"
    );
    assert!(
        ReflexEngine::new(vec![fan_rule()])
            .tick(&agent_world, fact.valid_from + 1_000)
            .unwrap()
            .is_empty(),
        "an agent-reported temperature must not actuate"
    );

    // The driver path, same controller type, same value: this one is measurement.
    let (driver_world, driver_ctrl) = make();
    driver_ctrl
        .ingest(
            &Sample {
                quantity: "temperature".to_string(),
                value: 30.0,
                unit: None,
                source: Some("bme280".to_string()),
            },
            3_000,
            Origin::Observed,
        )
        .unwrap();
    assert_eq!(
        ReflexEngine::new(vec![fan_rule()])
            .tick(&driver_world, 4_000)
            .unwrap()
            .len(),
        1,
        "a real reading still fires — the gate closed the hazard, not the feature"
    );
}

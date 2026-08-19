//! A mesh node going quiet must reach System 2 through the standard safing rules.
//!
//! This lived in `src/spine/mesh_supervisor.rs`'s test module until 2026-08-13,
//! where it was — measurably — the single most expensive line in the tree.
//!
//! `spine` is 4781 lines and, by that date, named `agent` exactly once: this
//! test's `use crate::agent::safing::{standard_safing_rules, SafingOptions}`.
//! One `use` line, inside `#[cfg(test)]`, inside one test function. It was in
//! five of the nine cycles `scripts/core_endgame.py` reported, because
//! `agent -> spine` is real and productive and the return arrow made every path
//! through `spine` circular.
//!
//! The claim is worth keeping exactly as it was, which is why it moved rather
//! than being weakened: the supervisor's *aggregate* — `mesh.escalated_count` —
//! must be a thing the shipped safing rules actually watch. A test that built
//! its own rule would prove the engine works and prove nothing about the rules
//! anyone runs. It spans the spine and the safing layer, so it belongs where
//! both can be named without either owning the other.
//!
//! The config is inlined rather than borrowed from the helper it came from, for
//! the reason `tests/reflex_boundary.rs` gives: a test that quietly depends on
//! someone else's fixture breaks for reasons it does not describe.

use obc_reflex::{Action, ReflexEngine};
use oh_ben_claw::agent::safing::{standard_safing_rules, SafingOptions};
use oh_ben_claw::config::MeshSupervisorConfig;
use oh_ben_claw::memory::world::{Origin, WorldMemory};
use oh_ben_claw::spine::lora_gateway::SOURCE;
use oh_ben_claw::spine::mesh_supervisor::tick;
use serde_json::json;

/// Observe-only: no recovery command, and escalation is time-based so it still
/// fires. `escalate_after_ms` is the only field this test actually depends on.
fn escalating_config() -> MeshSupervisorConfig {
    MeshSupervisorConfig {
        enabled: true,
        stale_ms: 5_000,
        tick_ms: 5_000,
        recover: None,
        min_recovery_interval_ms: 30_000,
        escalate_after_ms: 20_000,
        escalated_probe_interval_ms: 300_000,
    }
}

#[tokio::test]
async fn escalation_raises_the_count_that_drives_a_reflex() {
    let world = WorldMemory::open_in_memory().unwrap();
    world
        .observe_as(
            "mesh.n",
            json!({ "last_type": "link_state" }),
            1_000,
            1_000,
            SOURCE,
            Origin::Observed,
        )
        .unwrap();
    world
        .observe(
            "mesh.n.health",
            json!({ "status": "offline" }),
            1_000,
            1_000,
            "test",
        )
        .unwrap();

    // Supervisor escalates the long-offline node → publishes the aggregate count.
    tick(&world, None, &escalating_config(), 30_000).await;
    assert_eq!(
        world
            .current("mesh.escalated_count")
            .unwrap()
            .unwrap()
            .value
            .as_u64(),
        Some(1)
    );

    // A standard safing engine reads world memory and fires the health-driven
    // escalate — the mesh's presumed-lost node wakes System 2.
    let engine = ReflexEngine::new(standard_safing_rules(&SafingOptions::default()));
    let fired = engine.tick(&world, 40_000).unwrap();
    assert!(
        fired
            .iter()
            .any(|f| f.rule_id == "safe-mesh-node-lost"
                && matches!(f.action, Action::Escalate { .. })),
        "mesh health drives a reflex escalation"
    );
}

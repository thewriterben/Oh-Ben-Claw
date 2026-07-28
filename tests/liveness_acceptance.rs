//! Acceptance test for source liveness, run against a **copy** of a real world store.
//!
//! Unit tests build the situation they then check, which is exactly the weakness here:
//! the bug this feature exists for was not something anyone constructed. It accumulated
//! in a live store over ten days of bench work, and every previous attempt to reason
//! about it from memory got some detail wrong. So this test takes whatever store it is
//! pointed at, retires the mesh supervisor, and asserts on what actually happens.
//!
//! Point it at a store with `OBC_ACCEPTANCE_WORLD_DB=<path to a copy>`. Without that it
//! skips: CI has no bench store, and a test that silently invented one would be
//! measuring its own fixture again.
//!
//! **Copy the database first.** The test writes. Passing it the store OBC has open
//! would retract live beliefs and corrupt the WAL out from under a running process.

use oh_ben_claw::memory::liveness::{entity_for, is_marked_stopped, stopped, Stopped};
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::spine::mesh_supervisor::SUPERVISOR_SOURCE;

fn store() -> Option<(WorldMemory, String)> {
    let path = std::env::var("OBC_ACCEPTANCE_WORLD_DB").ok()?;
    if !std::path::Path::new(&path).exists() {
        panic!("OBC_ACCEPTANCE_WORLD_DB={path} does not exist");
    }
    let w = WorldMemory::open(&path).expect("open world db copy");
    Some((w, path))
}

#[test]
fn retiring_the_supervisor_retracts_what_it_left_standing() {
    let Some((w, path)) = store() else {
        eprintln!("skipped: set OBC_ACCEPTANCE_WORLD_DB to a *copy* of a world.db");
        return;
    };

    let before_open = w.open_sources().expect("sources");
    println!("store {path}");
    println!("  open sources: {before_open:?}");
    println!("  support coverage: {:?}", w.support_coverage().unwrap());

    let standing = w.open_facts_by_source(SUPERVISOR_SOURCE).expect("open facts");
    println!("  open mesh-supervisor facts: {}", standing.len());
    for f in &standing {
        println!("    #{} {} derived_from={:?}", f.id, f.entity, f.derived_from);
    }
    assert!(
        !standing.is_empty(),
        "nothing to retract — is this the right store?"
    );

    assert!(!is_marked_stopped(&w, SUPERVISOR_SOURCE).unwrap(), "already swept");

    let now = 2_000_000_000_000; // fixed, well past any bench timestamp
    let sweep = stopped(
        &w,
        SUPERVISOR_SOURCE,
        Stopped::Retired,
        now,
        "[mesh_supervisor] enabled = false",
    )
    .expect("sweep");
    println!(
        "  retracted {} facts, {} lost justification",
        sweep.closed.len(),
        sweep.unsupported.len()
    );

    // The headline: the number the reflex was firing on is no longer believed.
    assert!(
        w.current("mesh.escalated_count").unwrap().is_none(),
        "mesh.escalated_count is still believed after its author was retired"
    );

    // Undercut, not rebutted. Nothing now claims the count is zero, and the value we
    // used to hold is still answerable as history.
    let hist = w.history("mesh.escalated_count").unwrap();
    let last = hist.last().expect("history survives");
    assert_eq!(last.valid_to, Some(now), "closed, not deleted");
    assert!(
        w.at("mesh.escalated_count", last.valid_from).unwrap().is_some(),
        "what we believed is still readable as of when we believed it"
    );

    // Every supervisor fact is gone, and only supervisor facts are.
    assert!(w.open_facts_by_source(SUPERVISOR_SOURCE).unwrap().is_empty());
    for s in before_open.iter().filter(|s| *s != SUPERVISOR_SOURCE) {
        assert!(
            !w.open_facts_by_source(s).unwrap().is_empty(),
            "sweep took facts from an unrelated source: {s}"
        );
    }

    // The disappearance is written down, not inferred.
    let marker = w.current(&entity_for(SUPERVISOR_SOURCE)).unwrap().expect("marker");
    assert_eq!(marker.value["state"], serde_json::json!("retired"));
    assert!(marker.value["last_write_ms"].is_number(), "records when it last spoke");
    assert!(is_marked_stopped(&w, SUPERVISOR_SOURCE).unwrap());

    // Idempotent: a second boot with the supervisor still off changes nothing.
    let again = stopped(&w, SUPERVISOR_SOURCE, Stopped::Retired, now + 1, "again").unwrap();
    assert!(again.is_empty());
}

#[test]
fn the_phantom_rows_are_reported_even_though_this_cannot_close_them() {
    // The July 17 orphans: an agent note that discovery misread as a node, plus the
    // facts the supervisor derived about the phantom. Retiring the supervisor reaches
    // the derived ones. It does *not* reach the note itself, which is sourced `agent`
    // and which no liveness rule should touch — retiring the agent because it has not
    // written recently would retract every conclusion it has ever drawn.
    //
    // This test asserts that limit rather than papering over it, so the residue is
    // visible instead of quietly assumed handled.
    let Some((w, _)) = store() else {
        eprintln!("skipped: set OBC_ACCEPTANCE_WORLD_DB to a *copy* of a world.db");
        return;
    };

    stopped(&w, SUPERVISOR_SOURCE, Stopped::Retired, 2_000_000_000_000, "off").unwrap();

    let leftover: Vec<_> = w
        .entities()
        .unwrap()
        .into_iter()
        .filter(|e| e.starts_with("mesh.escalation_status"))
        .filter(|e| w.current(e).unwrap().is_some())
        .collect();

    println!("  phantom entities still believed: {leftover:?}");
    assert!(
        leftover.iter().all(|e| e == "mesh.escalation_status"),
        "only the agent's own note should survive the sweep, got {leftover:?}"
    );
}

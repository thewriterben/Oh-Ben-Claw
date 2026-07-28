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

/// Open a private copy of the store under test.
///
/// Every test gets its own copy, for two reasons. These tests retract things, so sharing
/// one file would make them order-dependent — and order-dependent tests against real
/// data are how you end up asserting on the previous test's damage. It also means that
/// if someone points the variable at a live database despite the warning, the writes
/// land on the copy.
fn store(tag: &str) -> Option<(WorldMemory, String)> {
    let src = std::env::var("OBC_ACCEPTANCE_WORLD_DB").ok()?;
    if !std::path::Path::new(&src).exists() {
        panic!("OBC_ACCEPTANCE_WORLD_DB={src} does not exist");
    }
    let dst = std::env::temp_dir().join(format!("obc-acceptance-{tag}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", dst.display()));
    }
    std::fs::copy(&src, &dst).expect("copy the store under test");
    let w = WorldMemory::open(&dst).expect("open world db copy");
    Some((w, dst.display().to_string()))
}

#[test]
fn retiring_the_supervisor_retracts_what_it_left_standing() {
    let Some((w, path)) = store("retire-supervisor") else {
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
    // A store that has already been swept has nothing left to demonstrate on, and that
    // is the *expected* state of any store OBC has booted since this shipped. Skipping
    // is the honest outcome; failing would report the fix working in production as a
    // broken test, and the usual response to that is to weaken the assertions.
    //
    // To exercise this against real data again, point the variable at a copy taken
    // before the first boot that swept it.
    if standing.is_empty() || is_marked_stopped(&w, SUPERVISOR_SOURCE).unwrap() {
        eprintln!(
            "skipped: {path} has already been swept — mesh-supervisor is marked stopped \
             and holds no open facts. This is what a healthy store looks like after the \
             fix; use a pre-sweep copy to reproduce the original state."
        );
        return;
    }

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

/// The whole feature, end to end, on real data.
///
/// Runs a supervisor tick against the real store so the chain is built from the actual
/// bench nodes and their real timestamps, then takes the radio away. This is the closest
/// thing available to reproducing the original bug and watching it not happen.
#[tokio::test]
async fn a_real_tick_builds_the_chain_and_losing_the_radio_unwinds_it() {
    let Some((w, _)) = store("rebuild-chain") else {
        eprintln!("skipped: set OBC_ACCEPTANCE_WORLD_DB to a *copy* of a world.db");
        return;
    };
    use oh_ben_claw::config::MeshSupervisorConfig;
    use oh_ben_claw::spine::lora_gateway::SOURCE as GATEWAY_SOURCE;

    // First, what the next boot will do: the supervisor is configured off, so its
    // beliefs are retired. This also clears the legacy rows — the ones written before
    // support was recorded — which is what lets the ticks below rebuild the chain
    // properly. Without it the supervisor writes nothing at all: health is already
    // "offline" and both nodes are already escalated, and it only writes on change. A
    // no-op tick is correct behaviour and useless as a test, which is a distinction an
    // earlier version of this test got wrong.
    stopped(&w, SUPERVISOR_SOURCE, Stopped::Retired, 1_999_000_000_000, "boot: configured off")
        .unwrap();

    let before = w.support_coverage().unwrap();
    println!("  support coverage before: {before:?}");

    let cfg = MeshSupervisorConfig {
        enabled: true,
        stale_ms: 30_000,
        tick_ms: 5_000,
        recover: None, // observe-only: never transmit during a test
        min_recovery_interval_ms: 30_000,
        escalate_after_ms: 120_000,
        escalated_probe_interval_ms: 300_000,
    };
    // Two ticks: the first records health, the second escalates once that health has
    // stood long enough. Timestamps well past the bench data so the nodes read as long
    // gone, which they are — the boards have been off for over a week.
    let t0 = 2_000_000_000_000u64;
    oh_ben_claw::spine::mesh_supervisor::tick(&w, None, &cfg, t0).await;
    oh_ben_claw::spine::mesh_supervisor::tick(&w, None, &cfg, t0 + 200_000).await;

    let after = w.support_coverage().unwrap();
    println!("  support coverage after ticks: {after:?}");
    assert!(after.0 > before.0, "the tick recorded no support at all");

    let count = w
        .current("mesh.escalated_count")
        .unwrap()
        .expect("a count after escalating long-dead nodes");
    println!("  mesh.escalated_count = {} support={:?}", count.value, count.derived_from);
    assert!(count.derived_from.is_some(), "the count did not declare its support");

    // Now the radio goes away. The supervisor is untouched — still enabled, still
    // running. Everything it concluded rested on facts the gateway observed.
    let sweep = stopped(&w, GATEWAY_SOURCE, Stopped::Retired, t0 + 400_000, "no COM port").unwrap();
    println!(
        "  retiring the radio: closed {} of its own, {} lost justification",
        sweep.closed.len(),
        sweep.unsupported.len()
    );
    assert!(
        !sweep.unsupported.is_empty(),
        "nothing propagated — the chain was not built"
    );
    assert!(
        w.current("mesh.escalated_count").unwrap().is_none(),
        "the count outlived the radio it was ultimately derived from"
    );
    // History intact: this is a retraction, not a delete.
    assert!(!w.history("mesh.escalated_count").unwrap().is_empty());
}

/// What a candidate retention policy would take, without taking it.
///
/// Retention is the one withdrawal whose blast radius is chosen by a human typing a
/// string, so the first thing to do with a new rule is find out what it eats. This
/// prints; it asserts only that the rule stays inside its namespace.
#[test]
fn a_candidate_retention_policy_reports_what_it_would_take() {
    let Some((w, _)) = store("expiry-dryrun") else {
        eprintln!("skipped: set OBC_ACCEPTANCE_WORLD_DB to a *copy* of a world.db");
        return;
    };
    use oh_ben_claw::memory::expiry::{due, ExpiryPolicy};

    let policies = vec![
        ExpiryPolicy {
            prefix: "incident.".into(),
            max_age_ms: 7 * 24 * 60 * 60 * 1000,
            origins: vec!["asserted".into()],
        },
        // `mesh.` is a shared namespace: the radio's evidence lives there too. Only the
        // agent's assertions under it age out, which is what the origin filter is for.
        ExpiryPolicy {
            prefix: "mesh.".into(),
            max_age_ms: 7 * 24 * 60 * 60 * 1000,
            origins: vec!["asserted".into()],
        },
    ];

    // Wall-clock, and this matters more than it looks.
    //
    // An earlier version passed a far-future timestamp "so age is not the variable under
    // test" — which quietly turned the question from *what expires now* into *what would
    // eventually expire*. It listed six facts; the real sweep took three, because the
    // other three notes had been rewritten hours earlier and were nowhere near seven days
    // old. The report was not wrong so much as answering a different question, which is
    // worse, because it looked like an answer to this one.
    //
    // Age is the variable under test. A dry run whose clock is not the clock the real
    // sweep will use is not a dry run.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let would = due(&w, &policies, now).expect("dry run");
    println!("  would expire {} facts (as of now):", would.len());
    for f in &would {
        println!(
            "    #{:<6} {:<38} age {:>6.2} d  {}",
            f.id,
            f.entity,
            (now.saturating_sub(f.ingested_at)) as f64 / 86_400_000.0,
            f.value.to_string().chars().take(48).collect::<String>()
        );
    }

    // What the policies cover but is not yet old enough — the other half of the answer,
    // and the half whose absence made the first report misleading.
    let mut safe_for_now = Vec::new();
    for p in &policies {
        for f in w.open_facts_with_prefix(&p.prefix).unwrap() {
            if f.origin == oh_ben_claw::memory::world::Origin::Asserted
                && !would.iter().any(|x| x.id == f.id)
            {
                safe_for_now.push((f.id, f.entity.clone(), now.saturating_sub(f.ingested_at)));
            }
        }
    }
    println!("  in scope but too young ({}):", safe_for_now.len());
    for (id, entity, age) in &safe_for_now {
        println!(
            "    #{:<6} {:<38} age {:>6.2} d",
            id,
            entity,
            *age as f64 / 86_400_000.0
        );
    }

    for f in &would {
        assert!(
            f.entity.starts_with("incident.") || f.entity.starts_with("mesh."),
            "policy reached outside its namespace: {}",
            f.entity
        );
        assert_eq!(f.source, "agent", "only the agent's own assertions");
    }
    // A dry run writes nothing.
    assert!(due(&w, &policies, now).unwrap().len() == would.len());
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
    let Some((w, _)) = store("phantom-residue") else {
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

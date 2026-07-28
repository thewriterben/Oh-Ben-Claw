//! Source liveness — writing down that a source stopped reporting.
//!
//! # Why this is a module and not an `if`
//!
//! A belief written by a sensor, a radio or a supervisor is only as good as that
//! source still being there. When the source goes away, three things are true at once
//! and every one of them has to be represented separately:
//!
//! - the source is no longer reporting (a fact about the *source*),
//! - the beliefs it wrote are no longer being maintained (facts about the *world*),
//! - and the beliefs computed *from* those are no longer justified (facts about
//!   *inference*).
//!
//! Collapsing any two of these produces a bug we have already shipped.
//! `mesh.escalated_count = 2` sat open in the bench store because the supervisor that
//! computed it was switched off in config: nothing was false, nothing had changed, and
//! the `safe-mesh-node-lost` reflex kept firing on a number whose author no longer
//! existed.
//!
//! # The rule this module implements
//!
//! Prometheus 2.0 settled the shape of the answer in 2017, after its own version of
//! this bug (alerts firing on targets that had vanished, because a 5-minute lookback
//! cannot tell "gone" from "quiet"). Its fix was a **staleness marker**: when a target
//! disappears, that disappearance is *written into the series* rather than inferred at
//! read time. The transferable principle:
//!
//! > Absence of data is itself a datum, and must be written — distinguishable from
//! > "not yet arrived" and from "reported false".
//!
//! So a retirement here is not a deletion and not a zero. It is a fact, at
//! `source.<name>.liveness`, that says the source stopped and when. Flink's
//! `withIdleness` is the same idea from the other direction: a source that will not
//! speak again has to *say so*, or the rest of the system waits for it forever.
//!
//! # Retraction is undercutting, not rebutting
//!
//! Pollock's distinction, and the reason this module closes facts rather than writing
//! new values. A *rebutting* defeater asserts ¬p — the sensor now reads 0. An
//! *undercutting* defeater says the inference no longer holds, while asserting nothing
//! about p — the sensor is gone, so we have no grounds either way.
//!
//! "The supervisor was switched off" is undercutting. The correct state for
//! `mesh.escalated_count` is therefore neither `2` nor `0`; it is *not currently
//! believed*. Closing the valid-time interval says exactly that and no more, and the
//! value stays readable through [`WorldMemory::at`] and
//! [`WorldMemory::history`](WorldMemory::history) for anyone asking what we used to
//! think. Writing `0` would have been a lie with the same shape as the original bug.
//!
//! # What this module does *not* do yet
//!
//! Propagation is **one hop**. A belief loses its justification when a fact in its
//! in-list is closed; beliefs derived from *that* belief are not yet followed. Nor are
//! alternative justifications represented: an in-list is conjunctive (Doyle's JTMS —
//! every entry must hold), so `a·b` is the only semantics available here. Independent
//! corroboration (`a+b`, where a belief survives one support dying) needs a fact to
//! carry *several* in-lists, which is the next step and not this one.
//!
//! Both limits are deliberate and both fail in the same direction: this sweep
//! under-retracts. Facts with unknown support are invisible to
//! [`WorldMemory::dependents`] by construction, so the blast radius is exactly the set
//! of beliefs that explicitly declared what they rest on.

use anyhow::Result;
use serde_json::json;

use super::world::{Fact, Origin, WorldMemory};

/// The source label liveness facts are written under.
///
/// Never itself swept: the bookkeeping about which sources are alive cannot be subject
/// to the same expiry as the sources, or the first sweep erases its own records.
pub const SOURCE: &str = "liveness";

/// Entity holding the liveness state of `source`.
pub fn entity_for(source: &str) -> String {
    format!("source.{source}.liveness")
}

/// Why a source stopped reporting. The distinction is not cosmetic: a silent source may
/// come back and resume writing, a retired one has been turned off deliberately and
/// will not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Configured off, or otherwise known not to be running. Deterministic.
    Retired,
    /// Has not written for longer than the caller's threshold. Inferred, and therefore
    /// capable of being wrong about a source that is merely slow.
    Silent,
}

impl Stopped {
    pub fn as_str(self) -> &'static str {
        match self {
            Stopped::Retired => "retired",
            Stopped::Silent => "silent",
        }
    }
}

/// What a sweep did. Empty fields mean "nothing to do", not "failed".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    /// The source that stopped.
    pub source: String,
    /// Facts written by that source which are no longer believed.
    pub closed: Vec<i64>,
    /// Beliefs that lost a member of their in-list and so lost their justification.
    pub unsupported: Vec<i64>,
    /// Beliefs that named a closed fact but were left alone because something else was
    /// already maintaining them — recorded so under-retraction is visible rather than
    /// silent.
    pub skipped: Vec<i64>,
}

impl Sweep {
    pub fn is_empty(&self) -> bool {
        self.closed.is_empty() && self.unsupported.is_empty()
    }
}

/// Declare that `source` has stopped reporting, and stop believing what rested on it.
///
/// Writes the liveness marker first, then closes the source's own open facts, then
/// closes any belief whose in-list names one of them. The marker is written even when
/// there is nothing to retract: "this source is gone" is worth knowing on its own, and
/// a caller that only wrote markers when something changed would leave the store unable
/// to distinguish a source that never existed from one that stopped quietly.
///
/// Idempotent. A second call finds the facts already closed and reports empty lists.
pub fn stopped(
    world: &WorldMemory,
    source: &str,
    state: Stopped,
    now_ms: u64,
    reason: &str,
) -> Result<Sweep> {
    let mut sweep = Sweep {
        source: source.to_string(),
        ..Default::default()
    };

    let last_write = world.last_write_by_source(source)?;

    // The marker. `Derived` because the framework concluded it — a liveness verdict is
    // not a reading off a wire, and must never be mistaken for one.
    world.observe_as(
        &entity_for(source),
        json!({
            "state": state.as_str(),
            "reason": reason,
            "last_write_ms": last_write,
            "ts_ms": now_ms,
        }),
        now_ms,
        now_ms,
        SOURCE,
        Origin::Derived,
    )?;

    if source == SOURCE {
        // Guard rather than assume: sweeping the liveness source would close the markers
        // that record which sources are alive, including this one.
        return Ok(sweep);
    }

    let orphaned = world.open_facts_by_source(source)?;
    for fact in &orphaned {
        if world.close_fact(fact.id, now_ms)? {
            sweep.closed.push(fact.id);
        }
    }

    // One hop out. Collected before closing anything downstream so the walk sees a
    // consistent picture, and de-duplicated because two closed facts can support the
    // same belief (the escalated_count case exactly: one in-list, two dead members).
    let mut candidates: Vec<Fact> = Vec::new();
    for id in &sweep.closed {
        for dep in world.dependents(*id)? {
            if !candidates.iter().any(|c| c.id == dep.id) {
                candidates.push(dep);
            }
        }
    }

    for dep in candidates {
        if sweep.closed.contains(&dep.id) {
            // The dead source's own rollup, already closed directly above. Common, and
            // worth naming: a component that derives a fact usually writes it under its
            // own source label, so retiring that source retracts the rollup without the
            // in-list being consulted at all. The in-list earns its keep when the
            // derived belief belongs to a *different* source than its inputs — which is
            // the case no amount of source bookkeeping can reach.
            continue;
        }
        if dep.valid_to.is_some() {
            continue; // closed by someone else since the walk started
        }
        if world.close_fact(dep.id, now_ms)? {
            sweep.unsupported.push(dep.id);
        } else {
            sweep.skipped.push(dep.id);
        }
    }

    Ok(sweep)
}

/// Whether `source` is already marked as having stopped.
///
/// The marker doubles as the memory of having swept. Without this check a caller that
/// runs on every boot (or every tick) rewrites the marker each time, and a store whose
/// only change is "still gone, still gone, still gone" is the churn that supersession
/// was designed to avoid.
pub fn is_marked_stopped(world: &WorldMemory, source: &str) -> Result<bool> {
    let Some(marker) = world.current(&entity_for(source))? else {
        return Ok(false);
    };
    let state = marker.value.get("state").and_then(|s| s.as_str());
    Ok(state == Some(Stopped::Silent.as_str()) || state == Some(Stopped::Retired.as_str()))
}

/// Sweep every source that has not written for `stale_ms`, except those in `exempt`.
///
/// The periodic counterpart to [`stopped`] — the scrape-failure case rather than the
/// configured-off case. Inherently a guess: a source that writes only on change is
/// indistinguishable from one that died, which is why the threshold is the caller's and
/// why anything that legitimately reports rarely belongs in `exempt`.
///
/// Sources with no writes at all are skipped rather than swept: never having reported
/// is not the same as having stopped.
pub fn sweep_silent(
    world: &WorldMemory,
    now_ms: u64,
    stale_ms: u64,
    exempt: &[&str],
) -> Result<Vec<Sweep>> {
    let mut out = Vec::new();
    for source in world.open_sources()? {
        if source == SOURCE || exempt.contains(&source.as_str()) {
            continue;
        }
        let Some(last) = world.last_write_by_source(&source)? else {
            continue;
        };
        if now_ms.saturating_sub(last) < stale_ms {
            continue;
        }
        if is_marked_stopped(world, &source)? {
            continue;
        }
        let sweep = stopped(
            world,
            &source,
            Stopped::Silent,
            now_ms,
            &format!("no write for {} ms", now_ms.saturating_sub(last)),
        )?;
        if !sweep.is_empty() {
            out.push(sweep);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The bench bug, reproduced and then fixed, in the shape it actually occurred:
    /// two node escalations observed, a count derived from them, the supervisor
    /// switched off.
    fn bench_store() -> (WorldMemory, i64, i64, i64) {
        let w = WorldMemory::open_in_memory().unwrap();
        let e1 = w
            .observe(
                "mesh.gw-40.escalation",
                json!({"status": "escalated"}),
                1_000,
                1_000,
                "mesh-supervisor",
            )
            .unwrap();
        let e2 = w
            .observe(
                "mesh.obc-esp32-s3-001.escalation",
                json!({"status": "escalated"}),
                1_000,
                1_000,
                "mesh-supervisor",
            )
            .unwrap();
        let count = w
            .observe_derived_from(
                "mesh.escalated_count",
                json!(2),
                1_100,
                1_100,
                "mesh-supervisor",
                &[e1.id, e2.id],
            )
            .unwrap();
        (w, e1.id, e2.id, count.id)
    }

    #[test]
    fn retiring_a_source_retracts_the_count_that_rested_on_it() {
        let (w, e1, _e2, count) = bench_store();
        assert_eq!(w.current("mesh.escalated_count").unwrap().unwrap().value, json!(2));

        let sweep = stopped(
            &w,
            "mesh-supervisor",
            Stopped::Retired,
            9_000,
            "[mesh_supervisor] enabled = false",
        )
        .unwrap();

        // The count is no longer believed — the acceptance test for this whole step.
        assert!(w.current("mesh.escalated_count").unwrap().is_none());
        assert!(sweep.closed.contains(&e1));
        // Note *which* mechanism retracted it: the supervisor wrote the count under its
        // own source label, so liveness alone reaches it and the in-list is not needed.
        // Recorded here so the next reader does not credit the dependency walk for a
        // result source bookkeeping produced on its own. The walk earns its keep in
        // `a_belief_derived_from_another_source_loses_its_justification` below.
        assert!(sweep.closed.contains(&count));
        assert!(sweep.unsupported.is_empty());

        // Undercut, not rebutted: nothing claims the count is now zero.
        assert_eq!(w.at("mesh.escalated_count", 5_000).unwrap().unwrap().value, json!(2));
        assert_eq!(w.history("mesh.escalated_count").unwrap().len(), 1);
        assert_eq!(
            w.history("mesh.escalated_count").unwrap()[0].valid_to,
            Some(9_000)
        );
    }

    #[test]
    fn a_belief_derived_from_another_source_loses_its_justification() {
        // The case source bookkeeping cannot reach, and the reason the in-list exists.
        // The notifier is alive and well; what it concluded is not, because the reading
        // it concluded it from is gone.
        let w = WorldMemory::open_in_memory().unwrap();
        let reading = w
            .observe_as("trail.cam1.motion", json!(true), 1_000, 1_000, "clawcam", Origin::Observed)
            .unwrap();
        let alert = w
            .observe_derived_from(
                "notify.trail_activity",
                json!({"level": "high"}),
                1_100,
                1_100,
                "notifier",
                &[reading.id],
            )
            .unwrap();
        // An unrelated notifier belief with unknown support must be untouched.
        w.observe("notify.digest_due", json!(true), 1_100, 1_100, "notifier")
            .unwrap();

        let sweep = stopped(&w, "clawcam", Stopped::Retired, 9_000, "camera unplugged").unwrap();

        assert_eq!(sweep.closed, vec![reading.id]);
        assert_eq!(sweep.unsupported, vec![alert.id], "the walk crossed the source boundary");
        assert!(w.current("notify.trail_activity").unwrap().is_none());
        assert!(w.current("notify.digest_due").unwrap().is_some());
    }

    #[test]
    fn the_marker_says_when_the_source_last_spoke() {
        let (w, _, _, _) = bench_store();
        stopped(&w, "mesh-supervisor", Stopped::Retired, 9_000, "switched off").unwrap();

        let m = w.current(&entity_for("mesh-supervisor")).unwrap().unwrap();
        assert_eq!(m.value["state"], json!("retired"));
        assert_eq!(m.value["reason"], json!("switched off"));
        // Absence of data written as data: the last time it did report.
        assert_eq!(m.value["last_write_ms"], json!(1_100));
        assert_eq!(m.origin, Origin::Derived, "a verdict is not a reading");
        assert_eq!(m.source, SOURCE);
    }

    #[test]
    fn a_marker_is_written_even_when_nothing_needed_retracting() {
        // "This source is gone" is worth recording on its own. Writing markers only on
        // change would leave a store unable to tell a source that never existed from one
        // that stopped quietly.
        let w = WorldMemory::open_in_memory().unwrap();
        let sweep = stopped(&w, "clawcam", Stopped::Retired, 5_000, "unplugged").unwrap();
        assert!(sweep.is_empty());
        let m = w.current(&entity_for("clawcam")).unwrap().unwrap();
        assert_eq!(m.value["state"], json!("retired"));
        assert_eq!(m.value["last_write_ms"], json!(null), "it never reported");
    }

    #[test]
    fn unknown_support_is_never_retracted() {
        // The fail-closed guarantee. A belief that never declared what it rests on is
        // invisible to the walk, even when it plainly relates to the dead source. Under-
        // retracting is recoverable; a sweep that emptied the store would not be.
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe("mesh.n.escalation", json!({"status": "escalated"}), 1_000, 1_000, "mesh-supervisor")
            .unwrap();
        w.observe("some.rollup", json!(1), 1_000, 1_000, "notifier").unwrap();

        let sweep = stopped(&w, "mesh-supervisor", Stopped::Retired, 9_000, "off").unwrap();
        assert_eq!(sweep.unsupported, Vec::<i64>::new());
        assert!(
            w.current("some.rollup").unwrap().is_some(),
            "a belief with unknown support survives"
        );
    }

    #[test]
    fn facts_from_other_sources_are_left_alone() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as("mesh.n", json!({"seq": 1}), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        w.observe("mesh.n.health", json!({"status": "offline"}), 1_000, 1_000, "mesh-supervisor")
            .unwrap();

        stopped(&w, "mesh-supervisor", Stopped::Retired, 9_000, "off").unwrap();

        assert!(w.current("mesh.n").unwrap().is_some(), "the radio is still live");
        assert!(w.current("mesh.n.health").unwrap().is_none());
    }

    #[test]
    fn sweeping_twice_changes_nothing_the_second_time() {
        let (w, _, _, _) = bench_store();
        let first = stopped(&w, "mesh-supervisor", Stopped::Retired, 9_000, "off").unwrap();
        assert!(!first.is_empty());
        let second = stopped(&w, "mesh-supervisor", Stopped::Retired, 10_000, "off").unwrap();
        assert!(second.is_empty(), "idempotent");
        // The marker is superseded, not duplicated — bitemporal supersession still applies.
        assert_eq!(w.history(&entity_for("mesh-supervisor")).unwrap().len(), 2);
    }

    #[test]
    fn the_liveness_source_never_sweeps_itself() {
        // Otherwise the first sweep closes the markers recording which sources are alive.
        let w = WorldMemory::open_in_memory().unwrap();
        stopped(&w, "clawcam", Stopped::Retired, 5_000, "unplugged").unwrap();
        let sweep = stopped(&w, SOURCE, Stopped::Retired, 6_000, "impossible").unwrap();
        assert!(sweep.closed.is_empty());
        assert!(
            w.current(&entity_for("clawcam")).unwrap().is_some(),
            "the clawcam marker survives"
        );
    }

    #[test]
    fn silence_sweep_respects_the_threshold_and_does_not_repeat() {
        let (w, _, _, count) = bench_store();
        // Not yet stale.
        assert!(sweep_silent(&w, 5_000, 10_000, &[]).unwrap().is_empty());
        assert!(w.current("mesh.escalated_count").unwrap().is_some());

        let swept = sweep_silent(&w, 20_000, 10_000, &[]).unwrap();
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].source, "mesh-supervisor");
        assert!(swept[0].closed.contains(&count));
        assert!(w.current("mesh.escalated_count").unwrap().is_none());

        // Already marked; a later tick must not rewrite the marker or churn.
        assert!(sweep_silent(&w, 30_000, 10_000, &[]).unwrap().is_empty());
        assert_eq!(w.history(&entity_for("mesh-supervisor")).unwrap().len(), 1);
    }

    #[test]
    fn an_exempt_source_is_never_swept() {
        // For sources that legitimately report only on change. Being on this list means
        // "silence is not evidence of death for this one".
        let (w, _, _, _) = bench_store();
        let swept = sweep_silent(&w, 20_000, 10_000, &["mesh-supervisor"]).unwrap();
        assert!(swept.is_empty());
        assert!(w.current("mesh.escalated_count").unwrap().is_some());
    }
}

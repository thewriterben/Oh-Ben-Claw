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
//! An in-list is **conjunctive** — Doyle's JTMS: every entry must hold for the
//! justification to stand. A belief may carry *several alternative* justifications
//! (`a+b`) and survives while any one of them holds; see
//! [`WorldMemory::observe_derived_from_any`]. So the walk reaching a fact is not
//! sufficient reason to withdraw it — the cascade re-asks
//! [`WorldMemory::support_status`] at every step, and a corroborated belief simply
//! stays.
//!
//! The blast radius is worth stating plainly regardless: facts with unknown support are
//! invisible to [`WorldMemory::dependents`] by construction, so the only beliefs any
//! sweep can touch are the ones that explicitly declared what they rest on. Today that
//! is three producers.

use anyhow::Result;
use serde_json::json;

use super::world::{Closure, Origin, WorldMemory};

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

    // Two different closures, marked as such. The source's own facts stopped being
    // maintained; the beliefs downstream lost their grounds. Both set `valid_to`, and an
    // operator reading the store afterwards needs to tell them apart from the ordinary
    // case of an entity simply changing value.
    let stopped_tag = Closure::SourceStopped(source.to_string());
    let unsupported_tag = Closure::Unsupported(source.to_string());

    let orphaned = world.open_facts_by_source(source)?;
    for fact in &orphaned {
        if world.close_fact(fact.id, now_ms, &stopped_tag)? {
            sweep.closed.push(fact.id);
        }
    }

    let (unsupported, skipped) = cascade(world, &sweep.closed, now_ms, &unsupported_tag)?;
    sweep.unsupported = unsupported;
    sweep.skipped = skipped;
    Ok(sweep)
}

/// Withdraw everything that rested on `seeds`, transitively.
///
/// A belief that loses its justification is itself no longer a justification, so the
/// beliefs resting on *it* have to go too — Doyle's dependency-directed backtracking, and
/// the reason a one-hop version would have been a half-measure. The mesh chain is three
/// deep:
///
/// ```text
/// mesh.<node>  →  mesh.<node>.health  →  .escalation  →  mesh.escalated_count
/// ```
///
/// so retiring the gateway with one-hop propagation would close the rollups, mark health
/// unsupported, and leave the escalations and the count standing — the exact bug, one
/// level further down.
///
/// Breadth-first with a seen-set. The set is what makes this terminate: `derived_from` is
/// not schema-constrained to be acyclic, and a cycle (however it got written) must not
/// become an infinite loop inside a startup path.
///
/// Returns `(withdrawn, skipped)`. Shared with expiry rather than reimplemented there:
/// two copies of a graph walk drift, and the one that drifts is always the one without
/// the cycle guard.
pub(crate) fn cascade(
    world: &WorldMemory,
    seeds: &[i64],
    now_ms: u64,
    tag: &Closure,
) -> Result<(Vec<i64>, Vec<i64>)> {
    let mut withdrawn = Vec::new();
    let mut skipped = Vec::new();
    let mut frontier: Vec<i64> = seeds.to_vec();
    let mut seen: Vec<i64> = seeds.to_vec();

    while !frontier.is_empty() {
        let mut next: Vec<i64> = Vec::new();
        for id in &frontier {
            for dep in world.dependents(*id)? {
                if seen.contains(&dep.id) {
                    continue;
                }
                seen.push(dep.id);
                if seeds.contains(&dep.id) {
                    // Already closed directly by the caller. Common, and worth naming: a
                    // component that derives a fact usually writes it under its own
                    // source label, so retiring that source retracts the rollup without
                    // the in-list being consulted at all. The in-list earns its keep when
                    // the derived belief belongs to a *different* source than its inputs
                    // — the case no amount of source bookkeeping can reach.
                    next.push(dep.id);
                    continue;
                }
                if dep.valid_to.is_some() {
                    // Already closed by someone else. Still traversed: whatever rested on
                    // it is no better supported for the retraction having come from
                    // elsewhere.
                    next.push(dep.id);
                    continue;
                }
                // Naming a dead fact is not enough to be withdrawn. A belief with
                // alternative justifications survives while any one of them still
                // stands, so ask the store rather than assuming the walk arrived here
                // for a fatal reason. Without this check `a+b` would behave as `a·b`
                // and the whole distinction would be decorative.
                if !world.support_status(&dep)?.has_failed() {
                    skipped.push(dep.id);
                    continue;
                }
                if world.close_fact(dep.id, now_ms, tag)? {
                    withdrawn.push(dep.id);
                    next.push(dep.id);
                } else {
                    skipped.push(dep.id);
                }
            }
        }
        frontier = next;
    }
    Ok((withdrawn, skipped))
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
    use crate::memory::world::Support;
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
        assert_eq!(
            w.current("mesh.escalated_count").unwrap().unwrap().value,
            json!(2)
        );

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
        assert_eq!(
            w.at("mesh.escalated_count", 5_000).unwrap().unwrap().value,
            json!(2)
        );
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
            .observe_as(
                "trail.cam1.motion",
                json!(true),
                1_000,
                1_000,
                "clawcam",
                Origin::Observed,
            )
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
        assert_eq!(
            sweep.unsupported,
            vec![alert.id],
            "the walk crossed the source boundary"
        );
        assert!(w.current("notify.trail_activity").unwrap().is_none());
        assert!(w.current("notify.digest_due").unwrap().is_some());
    }

    #[test]
    fn retraction_propagates_the_whole_chain_not_one_hop() {
        // The mesh chain, three deep, sourced the way it really is: the radio observes,
        // the supervisor concludes on top of that. One-hop propagation would close the
        // rollup, mark health unsupported, and leave the escalation and the count
        // standing — the original bug, one level down.
        let w = WorldMemory::open_in_memory().unwrap();
        let rollup = w
            .observe_as(
                "mesh.n",
                json!({"seq": 7}),
                1_000,
                1_000,
                "lora-gateway",
                Origin::Observed,
            )
            .unwrap();
        let health = w
            .observe_derived_from(
                "mesh.n.health",
                json!({"status": "offline"}),
                1_100,
                1_100,
                "mesh-supervisor",
                &[rollup.id],
            )
            .unwrap();
        let esc = w
            .observe_derived_from(
                "mesh.n.escalation",
                json!({"status": "escalated"}),
                1_200,
                1_200,
                "mesh-supervisor",
                &[health.id],
            )
            .unwrap();
        let count = w
            .observe_derived_from(
                "mesh.escalated_count",
                json!(1),
                1_300,
                1_300,
                "mesh-supervisor",
                &[esc.id],
            )
            .unwrap();

        // Retire the *radio*, not the supervisor. The supervisor is alive and willing;
        // it simply has nothing left to have concluded from.
        let sweep = stopped(&w, "lora-gateway", Stopped::Retired, 9_000, "no COM port").unwrap();

        assert_eq!(sweep.closed, vec![rollup.id], "only the radio's own fact");
        assert_eq!(
            sweep.unsupported,
            vec![health.id, esc.id, count.id],
            "breadth-first, all three levels"
        );
        for e in ["mesh.n.health", "mesh.n.escalation", "mesh.escalated_count"] {
            assert!(w.current(e).unwrap().is_none(), "{e} still believed");
        }
    }

    #[test]
    fn independent_corroboration_survives_one_source_dying() {
        // The a+b case, and the reason the schema carries alternatives at all. Two
        // cameras each independently establish that the gate is occupied. Unplug one and
        // the belief must stand — under a single conjunctive in-list it would not, and
        // the distinction would be decorative.
        let w = WorldMemory::open_in_memory().unwrap();
        let cam1 = w
            .observe_as(
                "gate.cam1.motion",
                json!(true),
                1_000,
                1_000,
                "clawcam",
                Origin::Observed,
            )
            .unwrap();
        let cam2 = w
            .observe_as(
                "gate.cam2.motion",
                json!(true),
                1_000,
                1_000,
                "clawcam-b",
                Origin::Observed,
            )
            .unwrap();
        let occupied = w
            .observe_derived_from_any(
                "gate.occupied",
                json!(true),
                1_100,
                1_100,
                "fusion",
                &[vec![cam1.id], vec![cam2.id]],
            )
            .unwrap();
        // A belief on the same evidence but needing *both* — the control.
        let stereo = w
            .observe_derived_from(
                "gate.depth",
                json!(2.4),
                1_100,
                1_100,
                "fusion",
                &[cam1.id, cam2.id],
            )
            .unwrap();

        let sweep = stopped(&w, "clawcam", Stopped::Retired, 9_000, "camera 1 unplugged").unwrap();

        assert_eq!(sweep.closed, vec![cam1.id]);
        assert!(
            w.current("gate.occupied").unwrap().is_some(),
            "corroborated belief must survive one source dying"
        );
        assert!(
            sweep.skipped.contains(&occupied.id),
            "and the survival is reported"
        );
        assert_eq!(
            w.support_status(&w.current("gate.occupied").unwrap().unwrap())
                .unwrap(),
            Support::Grounded
        );

        // The conjunctive one goes, because it genuinely needed both.
        assert!(w.current("gate.depth").unwrap().is_none());
        assert!(sweep.unsupported.contains(&stereo.id));

        // Kill the second camera and the corroborated belief finally falls.
        let second = stopped(
            &w,
            "clawcam-b",
            Stopped::Retired,
            10_000,
            "camera 2 unplugged",
        )
        .unwrap();
        assert!(second.unsupported.contains(&occupied.id));
        assert!(w.current("gate.occupied").unwrap().is_none());
    }

    #[test]
    fn the_flat_and_nested_encodings_mean_the_same_thing_for_one_justification() {
        // A store written before alternatives existed holds flat arrays. Both forms have
        // to parse, and a single-justification write must keep producing the flat form —
        // migrating support is exactly the operation whose errors get walked.
        let w = WorldMemory::open_in_memory().unwrap();
        let a = w
            .observe_as("a", json!(1), 1_000, 1_000, "s", Origin::Observed)
            .unwrap();
        let flat = w
            .observe_derived_from("flat", json!(1), 1_100, 1_100, "s", &[a.id])
            .unwrap();
        let nested = w
            .observe_derived_from_any("nested", json!(1), 1_100, 1_100, "s", &[vec![a.id]])
            .unwrap();
        assert_eq!(flat.derived_from, nested.derived_from);
        assert_eq!(
            w.dependents(a.id).unwrap().len(),
            2,
            "both are found by the walk"
        );
    }

    #[test]
    fn a_cycle_in_the_support_graph_terminates() {
        // `derived_from` is not schema-constrained to be acyclic. However a cycle got
        // written, a startup path must not spin on it.
        let w = WorldMemory::open_in_memory().unwrap();
        let root = w
            .observe_as(
                "root",
                json!(1),
                1_000,
                1_000,
                "lora-gateway",
                Origin::Observed,
            )
            .unwrap();
        let a = w
            .observe_derived_from("a", json!(1), 1_100, 1_100, "notifier", &[root.id])
            .unwrap();
        let b = w
            .observe_derived_from("b", json!(1), 1_200, 1_200, "notifier", &[a.id])
            .unwrap();
        // Close the loop: a is also claimed to rest on b.
        w.observe_derived_from("a", json!(2), 1_300, 1_300, "notifier", &[b.id])
            .unwrap();

        let sweep = stopped(&w, "lora-gateway", Stopped::Retired, 9_000, "gone").unwrap();
        assert!(sweep.unsupported.contains(&b.id));
        assert!(w.current("b").unwrap().is_none());
    }

    #[test]
    fn a_withdrawal_is_distinguishable_from_a_supersession() {
        // Both set `valid_to`. Without the tag an operator cannot tell "the gate closed"
        // from "we lost our grounds for believing anything about the gate", and those
        // call for opposite responses.
        let w = WorldMemory::open_in_memory().unwrap();
        let reading = w
            .observe_as(
                "trail.cam1.motion",
                json!(true),
                1_000,
                1_000,
                "clawcam",
                Origin::Observed,
            )
            .unwrap();
        w.observe_derived_from(
            "notify.trail_activity",
            json!({"level": "high"}),
            1_100,
            1_100,
            "notifier",
            &[reading.id],
        )
        .unwrap();
        // An ordinary supersession, for contrast.
        w.observe_as(
            "trail.cam1.motion",
            json!(false),
            1_200,
            1_200,
            "clawcam",
            Origin::Observed,
        )
        .unwrap();

        stopped(&w, "clawcam", Stopped::Retired, 9_000, "unplugged").unwrap();

        let hist = w.history("trail.cam1.motion").unwrap();
        assert_eq!(
            Closure::of(&hist[0]),
            Closure::Superseded,
            "replaced by a newer reading, not withdrawn"
        );
        assert_eq!(
            Closure::of(&hist[1]),
            Closure::SourceStopped("clawcam".into()),
            "its author went away"
        );

        // The alert is *not* withdrawn, and finding that out is the point of writing
        // this test. Its in-list names the first reading, which the second reading had
        // already superseded before the camera was ever retired — so the sweep never
        // reached it: the walk starts from the source's *open* facts.
        //
        // That is not a bug in the sweep, it is the boundary of what a sweep can do. The
        // alert lost its grounds the moment the reading changed, seconds into normal
        // operation, and eagerly chasing that would retract and recompute on every
        // sensor tick forever. So supersession is answered lazily instead.
        let alert = w.current("notify.trail_activity").unwrap().unwrap();
        assert_eq!(
            Closure::of(&alert),
            Closure::Open,
            "still served by current()"
        );
        assert!(
            w.support_status(&alert).unwrap().has_failed(),
            "but its justification does not stand"
        );
        assert_eq!(
            w.support_status(&alert).unwrap(),
            crate::memory::world::Support::Ungrounded {
                missing: vec![reading.id]
            },
            "and it says which fact moved"
        );
        assert_eq!(w.ungrounded().unwrap().len(), 1);

        // The operator's query: what did we stop believing, and why.
        let withdrawn = w.withdrawn_since(0).unwrap();
        assert_eq!(
            withdrawn.len(),
            1,
            "only the reading; the supersession is not in here"
        );
        assert!(withdrawn.iter().all(|f| Closure::of(f).is_withdrawal()));
    }

    #[test]
    fn the_marker_says_when_the_source_last_spoke() {
        let (w, _, _, _) = bench_store();
        stopped(
            &w,
            "mesh-supervisor",
            Stopped::Retired,
            9_000,
            "switched off",
        )
        .unwrap();

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
        w.observe(
            "mesh.n.escalation",
            json!({"status": "escalated"}),
            1_000,
            1_000,
            "mesh-supervisor",
        )
        .unwrap();
        w.observe("some.rollup", json!(1), 1_000, 1_000, "notifier")
            .unwrap();

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
        w.observe_as(
            "mesh.n",
            json!({"seq": 1}),
            1_000,
            1_000,
            "lora-gateway",
            Origin::Observed,
        )
        .unwrap();
        w.observe(
            "mesh.n.health",
            json!({"status": "offline"}),
            1_000,
            1_000,
            "mesh-supervisor",
        )
        .unwrap();

        stopped(&w, "mesh-supervisor", Stopped::Retired, 9_000, "off").unwrap();

        assert!(
            w.current("mesh.n").unwrap().is_some(),
            "the radio is still live"
        );
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

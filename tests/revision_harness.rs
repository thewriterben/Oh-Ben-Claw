//! A scorecard for belief revision, on scenarios OBC actually ingests.
//!
//! # What this measures, and what it does not
//!
//! STALE asks whether a model can *infer* that a belief has been undercut, from
//! dialogue. This asks the question that starts where that one ends: once a producer
//! has *declared* what a belief rests on, does the machinery withdraw exactly the right
//! set — no more, no less?
//!
//! So this is a conformance harness, not a benchmark. It measures whether the
//! implementation matches its specification. It cannot tell you the specification is
//! right, and a high score here says nothing about whether a real producer would have
//! declared the right in-list in the first place. **That** is the open problem, and it
//! is bounded by coverage, not by this code.
//!
//! Stating it plainly because the alternative is a number that looks like a benchmark
//! result and is not one. The companion research document's own first finding is that
//! published memory numbers for a single system span 45 points.
//!
//! # Why the two error directions are reported separately
//!
//! Every design decision in this subsystem chose a direction on purpose:
//!
//! - Unknown support is invisible to the walk — **under**-retract rather than empty the
//!   store.
//! - An in-list is conjunctive — a belief needing two supports goes when either dies.
//! - Alternatives survive while any one justification stands.
//!
//! A single accuracy figure would hide which direction a regression went. Over-retraction
//! destroys beliefs that were fine; under-retraction leaves stale ones standing. They are
//! not interchangeable, and the second is the recoverable one.
//!
//! Deterministic: fixed scenario set, no randomness, no clock. Run with
//! `cargo test --test revision_harness -- --nocapture` to see the scorecard.

use oh_ben_claw::memory::liveness::{stopped, Stopped};
use oh_ben_claw::memory::world::{Origin, Support, WorldMemory};
use std::collections::BTreeSet;

/// One scenario: build a store, name the source that dies, and say which facts must
/// stop being believed.
struct Scenario {
    family: &'static str,
    name: &'static str,
    /// Builds the store and returns (source_to_retire, ids that must end up withdrawn).
    build: fn(&WorldMemory) -> (&'static str, BTreeSet<i64>),
}

/// Propagated conflict, depth 1. The base case: a conclusion whose author is alive and
/// whose grounds are gone.
fn chain_1(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "cam.motion",
            true.into(),
            1_000,
            1_000,
            "clawcam",
            Origin::Observed,
        )
        .unwrap();
    let derived = w
        .observe_derived_from(
            "notify.activity",
            1.into(),
            1_100,
            1_100,
            "notifier",
            &[base.id],
        )
        .unwrap();
    ("clawcam", [base.id, derived.id].into_iter().collect())
}

/// Depth 3 — the mesh shape. One-hop propagation would leave the last two standing.
fn chain_3(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let rollup = w
        .observe_as(
            "mesh.n",
            1.into(),
            1_000,
            1_000,
            "lora-gateway",
            Origin::Observed,
        )
        .unwrap();
    let health = w
        .observe_derived_from("mesh.n.health", 0.into(), 1_100, 1_100, "sup", &[rollup.id])
        .unwrap();
    let esc = w
        .observe_derived_from(
            "mesh.n.escalation",
            1.into(),
            1_200,
            1_200,
            "sup",
            &[health.id],
        )
        .unwrap();
    let count = w
        .observe_derived_from(
            "mesh.escalated_count",
            1.into(),
            1_300,
            1_300,
            "sup",
            &[esc.id],
        )
        .unwrap();
    (
        "lora-gateway",
        [rollup.id, health.id, esc.id, count.id]
            .into_iter()
            .collect(),
    )
}

/// Depth 5, to check the walk does not stop at an arbitrary bound.
fn chain_5(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "s.reading",
            1.into(),
            1_000,
            1_000,
            "sensor",
            Origin::Observed,
        )
        .unwrap();
    let mut expect: BTreeSet<i64> = [base.id].into_iter().collect();
    let mut prev = base.id;
    for i in 0..5 {
        let f = w
            .observe_derived_from(
                &format!("d{i}"),
                1.into(),
                1_100 + i,
                1_100 + i,
                "calc",
                &[prev],
            )
            .unwrap();
        expect.insert(f.id);
        prev = f.id;
    }
    ("sensor", expect)
}

/// Fan-out: several beliefs on one reading, all of which must go.
fn fan_out(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "s.reading",
            1.into(),
            1_000,
            1_000,
            "sensor",
            Origin::Observed,
        )
        .unwrap();
    let mut expect: BTreeSet<i64> = [base.id].into_iter().collect();
    for i in 0..4 {
        let f = w
            .observe_derived_from(&format!("f{i}"), 1.into(), 1_100, 1_100, "calc", &[base.id])
            .unwrap();
        expect.insert(f.id);
    }
    ("sensor", expect)
}

/// Conjunctive: needs both, one dies, belief goes.
fn conjunctive(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let a = w
        .observe_as("a", 1.into(), 1_000, 1_000, "src-a", Origin::Observed)
        .unwrap();
    let b = w
        .observe_as("b", 1.into(), 1_000, 1_000, "src-b", Origin::Observed)
        .unwrap();
    let both = w
        .observe_derived_from("stereo", 1.into(), 1_100, 1_100, "fusion", &[a.id, b.id])
        .unwrap();
    ("src-a", [a.id, both.id].into_iter().collect())
}

/// Corroborated: two alternative justifications, one dies, belief must SURVIVE.
/// The over-retraction test.
fn corroborated(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let a = w
        .observe_as("a", 1.into(), 1_000, 1_000, "src-a", Origin::Observed)
        .unwrap();
    let b = w
        .observe_as("b", 1.into(), 1_000, 1_000, "src-b", Origin::Observed)
        .unwrap();
    w.observe_derived_from_any(
        "either",
        1.into(),
        1_100,
        1_100,
        "fusion",
        &[vec![a.id], vec![b.id]],
    )
    .unwrap();
    ("src-a", [a.id].into_iter().collect())
}

/// Corroborated, then both die. Must fall on the second.
fn corroborated_both_die(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let a = w
        .observe_as("a", 1.into(), 1_000, 1_000, "src-a", Origin::Observed)
        .unwrap();
    let b = w
        .observe_as("b", 1.into(), 1_000, 1_000, "src-a", Origin::Observed)
        .unwrap();
    let either = w
        .observe_derived_from_any(
            "either",
            1.into(),
            1_100,
            1_100,
            "fusion",
            &[vec![a.id], vec![b.id]],
        )
        .unwrap();
    ("src-a", [a.id, b.id, either.id].into_iter().collect())
}

/// Unknown support next to a dying source. Must be untouched — the fail-closed rule.
fn unknown_support(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "s.reading",
            1.into(),
            1_000,
            1_000,
            "sensor",
            Origin::Observed,
        )
        .unwrap();
    w.observe("plausibly.related", 1.into(), 1_100, 1_100, "calc")
        .unwrap();
    ("sensor", [base.id].into_iter().collect())
}

/// A self-standing log of record. Nothing upstream may undercut it.
fn self_standing(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "s.reading",
            1.into(),
            1_000,
            1_000,
            "sensor",
            Origin::Observed,
        )
        .unwrap();
    w.observe_derived_from("log.of.record", 1.into(), 1_100, 1_100, "notifier", &[])
        .unwrap();
    ("sensor", [base.id].into_iter().collect())
}

/// A cycle downstream of the dying source. Must terminate and take the loop.
fn cyclic(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let root = w
        .observe_as("root", 1.into(), 1_000, 1_000, "sensor", Origin::Observed)
        .unwrap();
    let a = w
        .observe_derived_from("a", 1.into(), 1_100, 1_100, "calc", &[root.id])
        .unwrap();
    let b = w
        .observe_derived_from("b", 1.into(), 1_200, 1_200, "calc", &[a.id])
        .unwrap();
    // a is rewritten to rest on b — the loop. The old `a` row is superseded, not open.
    let a2 = w
        .observe_derived_from("a", 2.into(), 1_300, 1_300, "calc", &[b.id])
        .unwrap();
    ("sensor", [root.id, b.id, a2.id].into_iter().collect())
}

/// An unrelated source's beliefs must not be collateral damage.
fn bystander(w: &WorldMemory) -> (&'static str, BTreeSet<i64>) {
    let base = w
        .observe_as(
            "s.reading",
            1.into(),
            1_000,
            1_000,
            "sensor",
            Origin::Observed,
        )
        .unwrap();
    let other = w
        .observe_as(
            "other.reading",
            1.into(),
            1_000,
            1_000,
            "other",
            Origin::Observed,
        )
        .unwrap();
    w.observe_derived_from("other.derived", 1.into(), 1_100, 1_100, "calc", &[other.id])
        .unwrap();
    ("sensor", [base.id].into_iter().collect())
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        family: "propagation",
        name: "chain depth 1",
        build: chain_1,
    },
    Scenario {
        family: "propagation",
        name: "chain depth 3 (mesh shape)",
        build: chain_3,
    },
    Scenario {
        family: "propagation",
        name: "chain depth 5",
        build: chain_5,
    },
    Scenario {
        family: "propagation",
        name: "fan-out",
        build: fan_out,
    },
    Scenario {
        family: "semantics",
        name: "conjunctive (a·b)",
        build: conjunctive,
    },
    Scenario {
        family: "semantics",
        name: "corroborated survives (a+b)",
        build: corroborated,
    },
    Scenario {
        family: "semantics",
        name: "corroborated, both die",
        build: corroborated_both_die,
    },
    Scenario {
        family: "restraint",
        name: "unknown support untouched",
        build: unknown_support,
    },
    Scenario {
        family: "restraint",
        name: "self-standing untouched",
        build: self_standing,
    },
    Scenario {
        family: "restraint",
        name: "bystander source untouched",
        build: bystander,
    },
    Scenario {
        family: "robustness",
        name: "cycle terminates",
        build: cyclic,
    },
];

#[test]
fn revision_scorecard() {
    let mut exact = 0usize;
    let mut over_total = 0usize;
    let mut under_total = 0usize;
    let mut rows: Vec<(String, String, usize, usize, bool)> = Vec::new();

    for s in SCENARIOS {
        let w = WorldMemory::open_in_memory().unwrap();
        let (source, expected) = (s.build)(&w);

        stopped(&w, source, Stopped::Retired, 9_000, "harness").unwrap();

        // What is actually no longer believed, across every entity in the store.
        let mut actual: BTreeSet<i64> = BTreeSet::new();
        for entity in w.entities().unwrap() {
            for f in w.history(&entity).unwrap() {
                if f.valid_to.is_some() && f.closed_reason.is_some() {
                    actual.insert(f.id);
                }
            }
        }

        let over: Vec<i64> = actual.difference(&expected).copied().collect();
        let under: Vec<i64> = expected.difference(&actual).copied().collect();
        let ok = over.is_empty() && under.is_empty();
        if ok {
            exact += 1;
        }
        over_total += over.len();
        under_total += under.len();
        rows.push((
            s.family.to_string(),
            s.name.to_string(),
            over.len(),
            under.len(),
            ok,
        ));
    }

    println!(
        "\n  belief-revision conformance — {} scenarios\n",
        SCENARIOS.len()
    );
    println!(
        "  {:<12} {:<32} {:>5} {:>6}  result",
        "family", "scenario", "over", "under"
    );
    let mut last = "";
    for (family, name, over, under, ok) in &rows {
        let fam = if family == last {
            ""
        } else {
            last = family;
            family.as_str()
        };
        println!(
            "  {:<12} {:<32} {:>5} {:>6}  {}",
            fam,
            name,
            over,
            under,
            if *ok { "ok" } else { "MISMATCH" }
        );
    }
    println!(
        "\n  exact set match: {}/{}   over-retracted: {}   under-retracted: {}",
        exact,
        SCENARIOS.len(),
        over_total,
        under_total
    );
    println!("  (conformance to the specification, not a benchmark score — see the module docs)\n");

    assert_eq!(
        over_total, 0,
        "over-retraction: beliefs withdrawn that should have stood"
    );
    assert_eq!(
        under_total, 0,
        "under-retraction: beliefs left standing without grounds"
    );
    assert_eq!(exact, SCENARIOS.len());
}

/// The lazy half, measured separately because nothing retracts these.
///
/// A supporting fact is *superseded* rather than withdrawn — the ordinary case, on every
/// sensor tick. The dependent stays open and `support_status` must report it ungrounded
/// and name what moved.
#[test]
fn lazy_staleness_scorecard() {
    struct Case {
        name: &'static str,
        /// Returns (fact to ask about, expected support verdict).
        build: fn(&WorldMemory) -> (i64, &'static str),
    }

    fn superseded_support(w: &WorldMemory) -> (i64, &'static str) {
        let a = w
            .observe_as("a", 1.into(), 1_000, 1_000, "s", Origin::Observed)
            .unwrap();
        let d = w
            .observe_derived_from("d", 1.into(), 1_100, 1_100, "c", &[a.id])
            .unwrap();
        w.observe_as("a", 2.into(), 1_200, 1_200, "s", Origin::Observed)
            .unwrap();
        (d.id, "ungrounded")
    }
    fn support_still_current(w: &WorldMemory) -> (i64, &'static str) {
        let a = w
            .observe_as("a", 1.into(), 1_000, 1_000, "s", Origin::Observed)
            .unwrap();
        let d = w
            .observe_derived_from("d", 1.into(), 1_100, 1_100, "c", &[a.id])
            .unwrap();
        (d.id, "grounded")
    }
    fn one_of_two_superseded(w: &WorldMemory) -> (i64, &'static str) {
        let a = w
            .observe_as("a", 1.into(), 1_000, 1_000, "s", Origin::Observed)
            .unwrap();
        let b = w
            .observe_as("b", 1.into(), 1_000, 1_000, "s", Origin::Observed)
            .unwrap();
        let d = w
            .observe_derived_from_any("d", 1.into(), 1_100, 1_100, "c", &[vec![a.id], vec![b.id]])
            .unwrap();
        w.observe_as("a", 2.into(), 1_200, 1_200, "s", Origin::Observed)
            .unwrap();
        (d.id, "grounded") // b still stands
    }
    fn no_in_list(w: &WorldMemory) -> (i64, &'static str) {
        let d = w.observe("d", 1.into(), 1_100, 1_100, "c").unwrap();
        (d.id, "unknown")
    }
    fn premise(w: &WorldMemory) -> (i64, &'static str) {
        let d = w
            .observe_derived_from("d", 1.into(), 1_100, 1_100, "c", &[])
            .unwrap();
        (d.id, "self-standing")
    }

    let cases = [
        Case {
            name: "support superseded",
            build: superseded_support,
        },
        Case {
            name: "support still current",
            build: support_still_current,
        },
        Case {
            name: "one of two superseded",
            build: one_of_two_superseded,
        },
        Case {
            name: "no in-list recorded",
            build: no_in_list,
        },
        Case {
            name: "explicit premise",
            build: premise,
        },
    ];

    println!("\n  lazy staleness — {} cases\n", cases.len());
    let mut pass = 0;
    for c in &cases {
        let w = WorldMemory::open_in_memory().unwrap();
        let (id, expected) = (c.build)(&w);
        let fact = w.fact_by_id(id).unwrap().unwrap();
        let got = match w.support_status(&fact).unwrap() {
            Support::Unknown => "unknown",
            Support::SelfStanding => "self-standing",
            Support::Grounded => "grounded",
            Support::Ungrounded { .. } => "ungrounded",
        };
        let ok = got == expected;
        if ok {
            pass += 1;
        }
        println!(
            "  {:<26} expected {:<14} got {:<14} {}",
            c.name,
            expected,
            got,
            if ok { "ok" } else { "MISMATCH" }
        );
        assert_eq!(got, expected, "{}", c.name);
    }
    println!("\n  {}/{} correct\n", pass, cases.len());
}

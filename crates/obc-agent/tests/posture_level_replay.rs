//! Rung 1 step A — what a graded descending level would have done.
//!
//! `docs/NEUROMORPHIC-2026-09.md` §6 asks for a map from the mushroom body's
//! novelty to a slot level, and says its constants are to be chosen from the
//! brain's own episodes rather than guessed. This is that replay. Nothing
//! here is wired: it reads a copy of the store, runs the body exactly as
//! `attach_mushroom` would, and asks `LevelMap` what each turn *would* have
//! sent.
//!
//! ```powershell
//! $env:OBC_TRAJECTORIES_DB = "$env:APPDATA\thewriterben\oh-ben-claw\data\trajectories.db"
//! cargo test -p obc-agent --test posture_level_replay -- --ignored --nocapture
//! ```
//!
//! **What this corpus can and cannot settle.** §1 of that document measured
//! the problem: 99 episodes over 2.19 days, most of them the same escalation
//! text scoring 0.001–0.03, four posture decisions ever. A sample piled at
//! one end cannot choose the *shape* of the novelty→caution curve, and this
//! harness does not pretend to — the shape is declared linear in
//! `LevelMap::caution` and the doc comment there says so. What the corpus
//! *is* is the traffic, so it can answer the questions that are about
//! traffic: how many frames a candidate map would put on a duty-cycle-limited
//! radio, how many distinct levels it would ever emit, and whether the
//! outcome prior is even present on the turns the map would care about.
//! Those are printed. The rest is left falsifiable.

use obc_agent::posture::{Descent, LevelMap, Posture};
use obc_memory::mushroom::{Assessment, MushroomBody, MushroomConfig, Valence};
use rusqlite::{Connection, OpenFlags};

/// The die rule's own default level, from `config.example.toml` — the only
/// slot-bound rule on the bench node, and so the only `rule_default` that
/// means anything today. A test constant citing its source, not a proposal.
const DIE_RULE_DEFAULT: f64 = 0.5;

/// `[descending] novel_level` as deployed.
const DEPLOYED_NOVEL_LEVEL: f64 = 0.15;

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// Copy db + wal so the read is consistent and the brain's handle is never
/// touched. Same approach as `obc-memory`'s real-episode harnesses.
fn copy_store(src: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-plr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("trajectories.db");
    std::fs::copy(src, &dst).unwrap();
    for suffix in ["-wal", "-shm"] {
        let s = format!("{src}{suffix}");
        if std::path::Path::new(&s).exists() {
            std::fs::copy(&s, dir.join(format!("trajectories.db{suffix}"))).unwrap();
        }
    }
    dst
}

struct Row {
    objective: String,
    outcome: String,
    ts_ms: i64,
    vec: Vec<f32>,
}

fn load(path: &std::path::Path) -> Vec<Row> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT e.objective, e.outcome, e.ts_ms, v.vec FROM episodes e
             JOIN episode_vecs v ON v.id = e.id ORDER BY e.ts_ms ASC, e.id ASC",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(Row {
            objective: r.get(0)?,
            outcome: r.get(1)?,
            ts_ms: r.get(2)?,
            vec: bytes_to_floats(&r.get::<_, Vec<u8>>(3)?),
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

/// Prequential replay: ask the body before each episode goes in, which is
/// exactly what `assess` would have said at that moment.
fn assess_all(rows: &[Row]) -> Vec<Assessment> {
    let mut body = MushroomBody::new(MushroomConfig::default()).unwrap();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(body.assess(&r.vec, r.ts_ms as u64).unwrap());
        let valence = match r.outcome.as_str() {
            "success" => Some(Valence::Rewarded),
            "failure" => Some(Valence::Punished),
            _ => None,
        };
        body.experience(&r.vec, r.ts_ms as u64, valence).unwrap();
    }
    out
}

/// Frames a policy would have put on the air: one per *change*, which is what
/// `PosturePolicy::apply` sends on.
fn changes<T: PartialEq>(seq: impl Iterator<Item = T>) -> usize {
    let mut last: Option<T> = None;
    let mut n = 0;
    for v in seq {
        if last.as_ref() != Some(&v) {
            n += 1;
            last = Some(v);
        }
    }
    n
}

fn describe(d: &Descent) -> String {
    match d {
        Descent::Clear => "clear".to_string(),
        Descent::Level(l) => format!("{l:.3}"),
    }
}

#[test]
#[ignore = "needs OBC_TRAJECTORIES_DB pointing at a real store"]
fn what_a_graded_descending_level_would_have_done() {
    let src = std::env::var("OBC_TRAJECTORIES_DB").expect("OBC_TRAJECTORIES_DB");
    let rows = load(&copy_store(&src));
    assert!(!rows.is_empty(), "no embedded episodes in {src}");
    let warmup = MushroomConfig::default().warmup_episodes;
    let span_days =
        (rows.last().unwrap().ts_ms - rows.first().unwrap().ts_ms) as f64 / 86_400_000.0;
    println!(
        "{} episodes over {span_days:.2} days, {}-d embeddings; warm-up {warmup}",
        rows.len(),
        rows[0].vec.len()
    );

    let per = assess_all(&rows);
    let after = || per.iter().enumerate().skip(warmup).map(|(_, a)| a);

    // ── The cheap check that may delete half the map ────────────────────
    //
    // §6 wants `success_prior` in the caution term. It is `Option<f32>`
    // behind a coverage floor, so the question is not how to weight it but
    // whether it is there at all — and in particular whether it is there on
    // the novel turns, which are the only ones the map changes anything for.
    let n_after = after().count();
    let with_prior = after().filter(|a| a.success_prior.is_some()).count();
    let novel_after = after().filter(|a| a.novel).count();
    let novel_with_prior = after()
        .filter(|a| a.novel && a.success_prior.is_some())
        .count();
    println!(
        "\noutcome prior after warm-up: {with_prior}/{n_after} turns carry one \
         ({:.0}%); of the {novel_after} novel turns, {novel_with_prior} do",
        100.0 * with_prior as f64 / n_after.max(1) as f64
    );
    println!(
        "  -> {}",
        if novel_with_prior == 0 {
            "the prior is absent on every turn the map would act on; a weighting \
             for it would be fitted to nothing. Step A ships novelty-only."
        } else {
            "the prior is present on turns the map acts on; it earns a term, and \
             step B should carry novelty and prior separately on the fact."
        }
    );

    // ── Baseline: what the deployed one-bit policy did ──────────────────
    let one_bit = changes(per.iter().map(Posture::for_assessment));
    println!(
        "\nbaseline (deployed, one bit): {one_bit} posture change(s) over {} turns \
         = {:.1} frames/day",
        rows.len(),
        one_bit as f64 / span_days.max(f64::MIN_POSITIVE)
    );

    // ── Candidate maps ──────────────────────────────────────────────────
    //
    // A sweep, not a proposal. The endpoints are fixed at what is deployed
    // (`rule_default` from the die rule, `novel_level` from `[descending]`);
    // what varies is the knee, the step and the floor — the three the corpus
    // can speak to. Read the frames/day column first: anything that beats
    // the one-bit baseline by much is buying variance with airtime.
    println!("\n  knee   full   step  floor | frames  frames/day | distinct levels emitted");
    let mut best: Option<(usize, LevelMap)> = None;
    let mut frontier: Vec<(usize, usize, LevelMap)> = Vec::new();
    for caution_knee in [0.0f32, 0.05, 0.10] {
        for caution_full in [0.25f32, 0.45, 0.75] {
            for step in [0.05f64, 0.10] {
                for clear_below in [0.05f32, 0.25] {
                    let m = LevelMap {
                        caution_knee,
                        caution_full,
                        rule_default: DIE_RULE_DEFAULT,
                        novel_level: DEPLOYED_NOVEL_LEVEL,
                        step,
                        clear_below,
                    };
                    let descents: Vec<Descent> = per.iter().map(|a| m.descent(a.novelty)).collect();
                    let frames = changes(descents.iter().copied());
                    let mut distinct: Vec<String> =
                        descents.iter().skip(warmup).map(describe).collect();
                    distinct.sort();
                    distinct.dedup();
                    println!(
                        "  {caution_knee:.2}   {caution_full:.2}   {step:.2}   {clear_below:.2} | \
                         {frames:6}  {:10.1} | {} [{}]",
                        frames as f64 / span_days.max(f64::MIN_POSITIVE),
                        distinct.len(),
                        distinct.join(", ")
                    );
                    frontier.push((frames, distinct.len(), m));
                }
            }
        }
    }

    // ── The frontier ────────────────────────────────────────────────────
    //
    // The choice step B has to make is a trade, so print the trade rather
    // than a winner: for each distinct-level count, the cheapest map that
    // achieves it, and what it costs against the one-bit baseline. A row
    // costing *less* than the baseline is buying variance for nothing,
    // which is the only kind of row worth taking without an argument.
    println!("\nfrontier — cheapest map per level count:");
    let mut counts: Vec<usize> = frontier.iter().map(|(_, d, _)| *d).collect();
    counts.sort_unstable();
    counts.dedup();
    for want in counts {
        let (frames, _, m) = frontier
            .iter()
            .filter(|(_, d, _)| *d == want)
            .min_by_key(|(f, _, _)| *f)
            .unwrap();
        let delta = *frames as i64 - one_bit as i64;
        println!(
            "  {want} descent(s): {frames:3} frames ({delta:+}) at knee {:.2}/{:.2}, \
             step {:.2}, floor {:.2}",
            m.caution_knee, m.caution_full, m.step, m.clear_below
        );
        if delta <= 0 {
            best = Some((want, *m));
        }
    }
    if let Some((levels, m)) = best {
        println!(
            "  -> {levels} descents for no more airtime than the deployed one bit: \
             knee {:.2}/{:.2}, step {:.2}, floor {:.2}",
            m.caution_knee, m.caution_full, m.step, m.clear_below
        );
    }

    // ── The turns that would actually differ ────────────────────────────
    //
    // Printed with their text so a person can see whether a graded level is
    // being asked for by anything real, or whether §6's prediction holds and
    // this corpus simply has four decisions in it however it is sliced.
    if let Some((_, m)) = best {
        println!("\nturns where the graded map differs from the one bit:");
        let mut differing = 0;
        for (i, (r, a)) in rows.iter().zip(&per).enumerate().skip(warmup) {
            let graded = m.descent(a.novelty);
            let one = match Posture::for_assessment(a) {
                Posture::Cautious => Descent::Level(DEPLOYED_NOVEL_LEVEL),
                Posture::Default => Descent::Clear,
            };
            if graded != one {
                differing += 1;
                let obj: String = r
                    .objective
                    .chars()
                    .take(60)
                    .collect::<String>()
                    .replace('\n', " ");
                println!(
                    "  {i:4}  novelty {:.3}  one-bit {:>6}  graded {:>6}  {obj:?}",
                    a.novelty,
                    describe(&one),
                    describe(&graded)
                );
            }
        }
        println!(
            "  {differing} of {n_after} turns after warm-up differ. §6 predicted \
             \"four graded decisions where there were four binary ones\"; this \
             number is the test of that prediction, and it is printed rather \
             than judged here because whether it is good news depends on the \
             frames column above, not on its size."
        );
    }
}

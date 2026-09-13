//! Did the descending posture do anything? — the measurement a learned
//! posture policy would have to beat.
//!
//! `obc_agent::posture` is a one-bit policy: novel → every owned slot to
//! `novel_level`, familiar → clear. Before anything learned replaces it, two
//! things have to exist that do not yet: a **metric** that says what a better
//! posture is, and **data** on which the current one has been scored. This
//! harness is the data half, and it names three candidate metrics so the
//! choice is made before there are numbers to be tempted by. Written
//! 2026-09-13 against a store with no live `descending.*` facts at all (the
//! release then running predated the mushroom body), so its first real run
//! is the day after the rebuilt release has been up for a while.
//!
//! Everything it reads is already recorded — nothing here adds instrumentation:
//!
//! - `descending.<node>` facts (source `descending-posture`): one per posture
//!   *change* the brain wanted, re-observed under the same `id` when the node
//!   answers. `answered`/`ok` say whether the node actually holds it.
//! - `mesh.<node>.reflex` facts (source `lora-gateway`): one per rule firing
//!   on the node — `rule_id`, `applied`, `error` (a Track 0 refusal reads
//!   here), `ts_ms`, and `ev`, the readings the rule fired on.
//! - `episodes` in the trajectory store: `objective`, `outcome`, `ts_ms`.
//!
//! ```powershell
//! $env:OBC_WORLD_DB        = "$env:APPDATA\thewriterben\oh-ben-claw\data\world.db"
//! $env:OBC_TRAJECTORIES_DB = "$env:APPDATA\thewriterben\oh-ben-claw\data\trajectories.db"
//! cargo test -p obc-memory --test posture_real_effect -- --ignored --nocapture
//! ```
//!
//! ## The three candidate metrics
//!
//! **M1 — Is the posture mechanically live?** Firing rate (per hour) of each
//! rule while the node is confirmed cautious versus confirmed default.
//! Cautious lowers slot-bound thresholds, so slot-bound rules *should* fire
//! more under it and literal-threshold rules should not change. If nothing
//! moves, posture is inert on this body and no policy — learned or not — can
//! be better than inert. This is a sanity check, not a benefit, and it is the
//! one that has to pass before M2 or M3 mean anything.
//!
//! **M2 — Does caution buy refusals?** Fraction of firings with
//! `applied: false` (the safety gate said no, or the GPIO write failed) under
//! each posture. A lower threshold means earlier, more frequent actuation;
//! if that runs into Track 0's rate or value limits, the brain is paying
//! frames to move a threshold to where the node will not act anyway. The
//! good direction is *fewer* refusals per firing under cautious, or at least
//! not more. Costs nothing extra to measure and is the most likely to
//! actually change a design.
//!
//! **M3 — Did a confirmed posture change an outcome?** Naively conditioning
//! episode success on posture is confounded to uselessness: cautious *is*
//! novel, and novel objectives fail more. The honest comparison is the
//! natural experiment the mesh runs for free — about one frame in five is
//! lost on the bench, so some novel objectives ran with the node confirmed
//! cautious and some ran with the send unanswered or refused. Same novelty,
//! different posture. Success rate of the two arms, with N printed beside it,
//! because N will be small for a long time and the number is worthless
//! without it.
//!
//! The harness prints all three and decides none. It also prints the shape a
//! node-side policy would train on — `(ev readings, posture the brain wanted)`
//! per reflex report — because the on-device object is a sensor-snapshot →
//! slot-levels map of a few dozen weights, not a language model
//! (`OBC-Prime/docs/EDGE-LM-2026-09.md` §7).

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn copy_store(src: &str, name: &str) -> PathBuf {
    // A live WAL store: copy db + wal + shm so the read is consistent and the
    // brain's own handle is never touched.
    let dir = std::env::temp_dir().join(format!("obc-pre-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(name);
    std::fs::copy(src, &dst).unwrap();
    for suffix in ["-wal", "-shm"] {
        let s = format!("{src}{suffix}");
        if Path::new(&s).exists() {
            std::fs::copy(&s, dir.join(format!("{name}{suffix}"))).unwrap();
        }
    }
    dst
}

struct Fact {
    entity: String,
    value: Value,
    valid_from: i64,
}

fn load_facts(conn: &Connection, like: &str) -> Vec<Fact> {
    let mut stmt = conn
        .prepare(
            "SELECT entity, value_json, valid_from FROM world_facts
             WHERE entity LIKE ?1 ORDER BY valid_from ASC, id ASC",
        )
        .unwrap();
    stmt.query_map([like], |r| {
        let json: String = r.get(1)?;
        Ok(Fact {
            entity: r.get(0)?,
            value: serde_json::from_str(&json).unwrap_or(Value::Null),
            valid_from: r.get(2)?,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

struct Episode {
    objective: String,
    outcome: String,
    ts_ms: i64,
}

fn load_episodes(path: &Path) -> Vec<Episode> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut stmt = conn
        .prepare("SELECT objective, outcome, ts_ms FROM episodes ORDER BY ts_ms ASC, id ASC")
        .unwrap();
    stmt.query_map([], |r| {
        Ok(Episode {
            objective: r.get(0)?,
            outcome: r.get(1)?,
            ts_ms: r.get(2)?,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
enum Posture {
    Default,
    Cautious,
}

impl Posture {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Posture::Default),
            "cautious" => Some(Posture::Cautious),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Posture::Default => "default",
            Posture::Cautious => "cautious",
        }
    }
}

/// What the brain wanted, and what became of it. One per `descend` id, the
/// last version of the fact (confirmation re-observes under the same id).
#[derive(Clone, Debug)]
struct Descend {
    node: String,
    id: String,
    posture: Posture,
    novelty: f64,
    objective: String,
    /// When the brain decided (`at_ms` on the fact).
    at_ms: i64,
    /// When the last version of the fact was written — for a confirmed one,
    /// when the node's answer landed.
    settled_ms: i64,
    /// `Some(true)`: node answered ok. `Some(false)`: node answered and
    /// refused. `None`: unanswered, or send failed, or still pending.
    held: Option<bool>,
    sent: bool,
    answered: Option<bool>,
}

fn descends(facts: &[Fact]) -> Vec<Descend> {
    let mut by_id: BTreeMap<String, Descend> = BTreeMap::new();
    for f in facts {
        let v = &f.value;
        let Some(id) = v.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(posture) = v
            .get("posture")
            .and_then(Value::as_str)
            .and_then(Posture::parse)
        else {
            continue;
        };
        let node = f.entity.trim_start_matches("descending.").to_string();
        let answered = v.get("answered").and_then(Value::as_bool);
        let ok = v.get("ok").and_then(Value::as_bool);
        let sent = v.get("sent").and_then(Value::as_bool).unwrap_or(false);
        let held = match (sent, answered, ok) {
            (true, Some(true), Some(true)) => Some(true),
            (true, Some(true), _) => Some(false),
            _ => None,
        };
        by_id.insert(
            id.to_string(),
            Descend {
                node,
                id: id.to_string(),
                posture,
                novelty: v.get("novelty").and_then(Value::as_f64).unwrap_or(f64::NAN),
                objective: v
                    .get("objective")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                at_ms: v
                    .get("at_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(f.valid_from),
                settled_ms: f.valid_from,
                held,
                sent,
                answered,
            },
        );
    }
    let mut out: Vec<Descend> = by_id.into_values().collect();
    out.sort_by_key(|d| (d.node.clone(), d.at_ms));
    out
}

/// Confirmed posture per node over time: `(node, from_ms, posture)`, from
/// the moment the node answered ok until the next confirmed change. Before
/// the first confirmation the posture is unknown — a reboot returns the
/// rules to defaults and nothing here can see a reboot.
fn intervals(ds: &[Descend]) -> Vec<(String, i64, Posture)> {
    ds.iter()
        .filter(|d| d.held == Some(true))
        .map(|d| (d.node.clone(), d.settled_ms, d.posture))
        .collect()
}

fn posture_at(iv: &[(String, i64, Posture)], node: &str, t: i64) -> Option<Posture> {
    iv.iter()
        .rev()
        .find(|(n, from, _)| n == node && *from <= t)
        .map(|(_, _, p)| *p)
}

fn hours(ms: i64) -> f64 {
    ms as f64 / 3_600_000.0
}

#[test]
#[ignore = "needs OBC_WORLD_DB (and optionally OBC_TRAJECTORIES_DB) pointing at real stores"]
fn the_posture_policy_on_the_brains_own_record() {
    let world_src = std::env::var("OBC_WORLD_DB").expect("OBC_WORLD_DB");
    let world_path = copy_store(&world_src, "world.db");
    let world = Connection::open_with_flags(&world_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

    let desc_facts = load_facts(&world, "descending.%");
    let reflex_facts: Vec<Fact> = load_facts(&world, "mesh.%.reflex");
    let ds = descends(&desc_facts);

    println!(
        "{} descending fact versions → {} descend ids; {} reflex reports",
        desc_facts.len(),
        ds.len(),
        reflex_facts.len()
    );
    if ds.is_empty() {
        println!(
            "\nNo `descending.*` facts: the brain has never sent a posture. Nothing to score.\n\
             (Is [descending] enabled, and is the running release built with the mushroom body?)"
        );
        return;
    }

    // ── What the brain wanted, and what the node heard ─────────────────────
    let mut confirmed = 0;
    let mut refused = 0;
    let mut unanswered = 0;
    let mut not_sent = 0;
    let mut pending = 0;
    let mut by_posture: BTreeMap<Posture, usize> = BTreeMap::new();
    for d in &ds {
        *by_posture.entry(d.posture).or_default() += 1;
        match (d.sent, d.answered, d.held) {
            (false, _, _) => not_sent += 1,
            (true, None, _) => pending += 1,
            (true, Some(false), _) => unanswered += 1,
            (true, Some(true), Some(true)) => confirmed += 1,
            (true, Some(true), _) => refused += 1,
        }
    }
    println!("\nposture changes wanted: {by_posture:?}");
    println!(
        "  confirmed {confirmed}  refused-by-node {refused}  unanswered {unanswered}  \
         send-failed {not_sent}  pending-at-copy {pending}"
    );
    println!(
        "\n  node                  at_ms          id        posture   novelty  held   objective"
    );
    for d in &ds {
        let obj: String = d
            .objective
            .chars()
            .take(48)
            .collect::<String>()
            .replace('\n', " ");
        println!(
            "  {:<20}  {:<13}  {:<8}  {:<8}  {:7.3}  {:<5}  {obj:?}",
            d.node,
            d.at_ms,
            d.id,
            d.posture.name(),
            d.novelty,
            match d.held {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "?",
            }
        );
    }

    let iv = intervals(&ds);
    let last_ms = reflex_facts
        .iter()
        .map(|f| f.valid_from)
        .chain(desc_facts.iter().map(|f| f.valid_from))
        .max()
        .unwrap_or(0);

    // Time each node spent confirmed in each posture.
    let mut time_in: BTreeMap<(String, Posture), i64> = BTreeMap::new();
    for (i, (node, from, p)) in iv.iter().enumerate() {
        let end = iv[i + 1..]
            .iter()
            .find(|(n, _, _)| n == node)
            .map(|(_, f, _)| *f)
            .unwrap_or(last_ms);
        *time_in.entry((node.clone(), *p)).or_default() += (end - from).max(0);
    }
    println!("\nconfirmed time under each posture:");
    for ((node, p), ms) in &time_in {
        println!("  {node:<20} {:<8} {:8.2} h", p.name(), hours(*ms));
    }

    // ── M1 / M2: reflex firings by posture ─────────────────────────────────
    // rule_id → posture → (fires, refused)
    let mut fires: BTreeMap<String, BTreeMap<Option<Posture>, (usize, usize)>> = BTreeMap::new();
    let mut ev_widths: BTreeMap<usize, usize> = BTreeMap::new();
    let mut training_rows = 0usize;
    for f in &reflex_facts {
        let node = f
            .entity
            .trim_start_matches("mesh.")
            .trim_end_matches(".reflex")
            .to_string();
        let rule = f
            .value
            .get("rule_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let applied = f
            .value
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let p = posture_at(&iv, &node, f.valid_from);
        let e = fires.entry(rule).or_default().entry(p).or_default();
        e.0 += 1;
        if !applied {
            e.1 += 1;
        }
        if let Some(ev) = f.value.get("ev").and_then(Value::as_array) {
            *ev_widths.entry(ev.len()).or_default() += 1;
            if p.is_some() {
                training_rows += 1;
            }
        }
    }
    println!("\nM1 firing rate and M2 refusal fraction, per rule and confirmed posture:");
    println!("  rule                    posture   fires   /hour   refused  frac");
    for (rule, per) in &fires {
        for (p, (n, r)) in per {
            let (pname, rate) = match p {
                Some(p) => {
                    let h: f64 = time_in
                        .iter()
                        .filter(|((_, q), _)| q == p)
                        .map(|(_, ms)| hours(*ms))
                        .sum();
                    (
                        p.name(),
                        if h > 0.0 {
                            format!("{:6.2}", *n as f64 / h)
                        } else {
                            "   n/a".into()
                        },
                    )
                }
                None => ("unknown", "   n/a".to_string()),
            };
            println!(
                "  {rule:<22}  {pname:<8}  {n:5}  {rate}  {r:7}  {:.2}",
                if *n > 0 { *r as f64 / *n as f64 } else { 0.0 }
            );
        }
    }
    println!(
        "  (`unknown` = fired before any confirmed posture, or after a reboot nothing here can see)"
    );

    // ── M3: the lost-frame natural experiment ──────────────────────────────
    match std::env::var("OBC_TRAJECTORIES_DB") {
        Ok(traj_src) => {
            let traj_path = copy_store(&traj_src, "trajectories.db");
            let eps = load_episodes(&traj_path);
            // Match each cautious descend to the episode it was decided on:
            // same objective prefix (the fact truncates to 120 chars), first
            // episode at or after the decision.
            let mut arms: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new(); // (n, success, failure)
            for d in ds.iter().filter(|d| d.posture == Posture::Cautious) {
                let arm = match d.held {
                    Some(true) => "held cautious",
                    _ => "not held (lost/refused/failed)",
                };
                let prefix: String = d.objective.chars().take(120).collect();
                let ep = eps
                    .iter()
                    .filter(|e| e.ts_ms >= d.at_ms - 1_000 && e.objective.starts_with(&prefix))
                    .min_by_key(|e| e.ts_ms);
                let e = arms.entry(arm).or_default();
                if let Some(ep) = ep {
                    e.0 += 1;
                    match ep.outcome.as_str() {
                        "success" => e.1 += 1,
                        "failure" => e.2 += 1,
                        _ => {}
                    }
                }
            }
            println!("\nM3 novel objectives by whether the node actually held cautious:");
            for (arm, (n, s, f)) in &arms {
                println!(
                    "  {arm:<34} N={n:3}  success {s:3}  failure {f:3}  rate {}",
                    if *n > 0 {
                        format!("{:.2}", *s as f64 / *n as f64)
                    } else {
                        "n/a".into()
                    }
                );
            }
            println!("  (same novelty in both arms by construction; N is the number that matters)");
        }
        Err(_) => println!("\nM3 skipped: OBC_TRAJECTORIES_DB not set."),
    }

    // ── What a node-side policy would train on ─────────────────────────────
    println!("\nnode-side dataset shape: (ev readings → posture the brain held)");
    println!("  reflex reports with a known posture: {training_rows}");
    println!("  ev width distribution: {ev_widths:?}");
    println!(
        "  (a sensor-snapshot → ≤16 slot-levels map; width above × 16 outputs is the whole model)"
    );
}

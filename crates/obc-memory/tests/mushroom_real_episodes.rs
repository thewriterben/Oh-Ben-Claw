//! The mushroom body on real episodes — the measurement the connectome
//! thread was missing.
//!
//! Every number about the body so far came from synthetic streams. The
//! brain has in fact been recording episodes with embeddings since
//! 2026-09-11 (`[self_improvement] semantic = true`), so the body's
//! judgement can be measured *prequentially* on what it would actually have
//! seen: replay the episodes in time order, and before each one is
//! observed, ask the body what it thinks of it — exactly what `assess`
//! would have said at that moment. Reads a copy of the store; needs no
//! embedder, because the vectors are stored.
//!
//! ```powershell
//! $env:OBC_TRAJECTORIES_DB = "$env:APPDATA\thewriterben\oh-ben-claw\data\trajectories.db"
//! cargo test -p obc-memory --test mushroom_real_episodes -- --ignored --nocapture
//! ```
//!
//! Prints one line per episode and a summary: novelty distribution, how many
//! objectives the body would have called novel after warm-up at the default
//! threshold and at two others, how many `descend`s the posture policy would
//! have sent (posture *changes*, not novel turns), which objectives got an
//! outcome prior, the repeat-vs-new gap for the body as first shipped and as
//! it is now, and the objectives nearest the threshold on either side.

use obc_memory::mushroom::{MushroomBody, MushroomConfig, Valence};
use rusqlite::{Connection, OpenFlags};

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn copy_store(src: &str) -> std::path::PathBuf {
    // A live WAL store: copy db + wal so the read is consistent and the
    // brain's own handle is never touched.
    let dir = std::env::temp_dir().join(format!("obc-mre-{}", std::process::id()));
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

/// Prequential replay at one threshold. Returns per-episode (novelty, novel,
/// prior) and the number of posture changes the policy would have made.
fn replay(rows: &[Row], threshold: f32) -> (Vec<(f32, bool, Option<f32>)>, usize) {
    replay_with(
        rows,
        MushroomConfig {
            novel_threshold: threshold,
            ..MushroomConfig::default()
        },
    )
}

/// The same replay under any config: exactly what `attach_mushroom` does on
/// open, with `assess` asked before each episode goes in.
fn replay_with(rows: &[Row], cfg: MushroomConfig) -> (Vec<(f32, bool, Option<f32>)>, usize) {
    let mut body = MushroomBody::new(cfg).unwrap();
    let mut out = Vec::new();
    let mut last_cautious: Option<bool> = None;
    let mut changes = 0;
    for r in rows {
        let a = body.assess(&r.vec, r.ts_ms as u64).unwrap();
        out.push((a.novelty, a.novel, a.success_prior));
        if last_cautious != Some(a.novel) {
            changes += 1;
            last_cautious = Some(a.novel);
        }
        let valence = match r.outcome.as_str() {
            "success" => Some(Valence::Rewarded),
            "failure" => Some(Valence::Punished),
            _ => None,
        };
        body.experience(&r.vec, r.ts_ms as u64, valence).unwrap();
    }
    (out, changes)
}

#[test]
#[ignore = "needs OBC_TRAJECTORIES_DB pointing at a real store"]
fn the_body_on_the_brains_own_episodes() {
    let src = std::env::var("OBC_TRAJECTORIES_DB").expect("OBC_TRAJECTORIES_DB");
    let path = copy_store(&src);
    let rows = load(&path);
    assert!(!rows.is_empty(), "no embedded episodes in {src}");
    let dim = rows[0].vec.len();
    println!(
        "{} episodes with {}-d embeddings, {} → {}",
        rows.len(),
        dim,
        rows.first().unwrap().ts_ms,
        rows.last().unwrap().ts_ms
    );
    let warmup = MushroomConfig::default().warmup_episodes;

    let (per, changes) = replay(&rows, MushroomConfig::default().novel_threshold);
    println!("\n idx  novelty  novel  prior   outcome  objective");
    for (i, (r, (n, novel, prior))) in rows.iter().zip(&per).enumerate() {
        let obj: String = r
            .objective
            .chars()
            .take(64)
            .collect::<String>()
            .replace('\n', " ");
        println!(
            "{i:4}  {n:7.3}  {:5}  {:>5}  {:7}  {obj:?}",
            if *novel { "NOVEL" } else { "" },
            prior
                .map(|p| format!("{:.0}%", p * 100.0))
                .unwrap_or_default(),
            r.outcome
        );
    }

    let mut sorted: Vec<f32> = per.iter().map(|p| p.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    println!(
        "\nnovelty over all {}: min {:.3}  p25 {:.3}  median {:.3}  p75 {:.3}  max {:.3}",
        sorted.len(),
        sorted[0],
        pct(0.25),
        pct(0.5),
        pct(0.75),
        sorted[sorted.len() - 1]
    );
    let after: Vec<&(f32, bool, Option<f32>)> = per.iter().skip(warmup).collect();
    let with_prior = after.iter().filter(|p| p.2.is_some()).count();
    println!(
        "after warm-up ({warmup}): {} episodes, {} with an outcome prior",
        after.len(),
        with_prior
    );
    for t in [0.15f32, MushroomConfig::default().novel_threshold, 0.4] {
        let (p, ch) = replay(&rows, t);
        let novel = p.iter().skip(warmup).filter(|x| x.1).count();
        println!(
            "threshold {t:.2}: {novel}/{} novel after warm-up; the posture policy would have sent {ch} descend(s) over {} turns",
            after.len(),
            rows.len()
        );
    }
    let _ = changes;

    // The question the table raises: can this body separate a repeat from a
    // new topic at all, and does the FlyHash centring or a sparser code help?
    // "Repeat" = an objective whose exact text was already seen; "new" = a
    // text seen for the first time. Report each variant's median novelty for
    // both groups after warm-up, and the gap between them.
    let mut seen = std::collections::HashSet::new();
    let is_repeat: Vec<bool> = rows
        .iter()
        .map(|r| !seen.insert(r.objective.trim().to_string()))
        .collect();
    // `warmup_episodes = 0` is the body as first shipped: no centring. The
    // "after warm-up" cut below is the default warm-up for every variant so
    // the groups are the same episodes.
    let variants: Vec<(&str, MushroomConfig)> = vec![
        (
            "first shipped (2000 KC, 5%, raw)",
            MushroomConfig {
                kenyon_cells: 2000,
                active_fraction: 0.05,
                warmup_episodes: 0,
                ..MushroomConfig::default()
            },
        ),
        (
            "centred only (2000 KC, 5%)",
            MushroomConfig {
                kenyon_cells: 2000,
                active_fraction: 0.05,
                ..MushroomConfig::default()
            },
        ),
        (
            "sparser only (20000 KC, 2%, raw)",
            MushroomConfig {
                warmup_episodes: 0,
                ..MushroomConfig::default()
            },
        ),
        ("now (defaults)", MushroomConfig::default()),
    ];
    println!("\nrepeat vs new, after warm-up (median novelty):");
    for (name, cfg) in variants {
        let (p, _) = replay_with(&rows, cfg);
        let mut rep: Vec<f32> = Vec::new();
        let mut new: Vec<f32> = Vec::new();
        for (i, x) in p.iter().enumerate().skip(warmup) {
            if is_repeat[i] {
                rep.push(x.0)
            } else {
                new.push(x.0)
            }
        }
        let med = |v: &mut Vec<f32>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if v.is_empty() {
                f32::NAN
            } else {
                v[v.len() / 2]
            }
        };
        let (mr, mn) = (med(&mut rep), med(&mut new));
        let new_min = new.first().copied().unwrap_or(f32::NAN);
        let rep_max = rep.last().copied().unwrap_or(f32::NAN);
        println!(
            "  {name:<34} repeats ({}) median {mr:.3} max {rep_max:.3}   new ({}) median {mn:.3} min {new_min:.3}   gap {:.3}",
            rep.len(),
            new.len(),
            mn - mr
        );
    }

    // Where the threshold has to sit: the least-novel first-seen objectives
    // and the most-novel repeats under the defaults, with their text, so a
    // near-repeat ("Note 2 for the record") is judged by eye, not by string
    // equality.
    let (p, _) = replay_with(&rows, MushroomConfig::default());
    let mut scored: Vec<(f32, usize)> = p
        .iter()
        .enumerate()
        .skip(warmup)
        .map(|(i, x)| (x.0, i))
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let show = |(n, i): &(f32, usize)| {
        let obj: String = rows[*i]
            .objective
            .chars()
            .take(70)
            .collect::<String>()
            .replace('\n', " ");
        println!(
            "  {n:.3}  {:6}  {obj:?}",
            if is_repeat[*i] { "repeat" } else { "new" }
        );
    };
    println!("\nleast novel first-seen objectives (defaults):");
    scored
        .iter()
        .filter(|s| !is_repeat[s.1])
        .take(10)
        .for_each(show);
    println!("most novel repeats (defaults):");
    scored
        .iter()
        .rev()
        .filter(|s| is_repeat[s.1])
        .take(5)
        .for_each(show);
}

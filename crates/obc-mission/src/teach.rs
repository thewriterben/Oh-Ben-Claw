//! Teach by demonstration — turn a teleoperated run into a draft mission.
//!
//! The operator drives the platform by hand (T-Deck, GUI joystick, anything that
//! moves it). Every drive path already lands in world memory as a pose trail, and
//! world memory is bitemporal, so a demonstration needs no recorder of its own: it
//! is a **time window** over that history. [`draft_from_world`] reads the trail in
//! the window, collapses the places the operator stopped into dwells, simplifies
//! the driving in between to waypoints, and emits `navigate_to` / `wait` steps.
//!
//! The result is a [`DraftMission`], and drafts are **quarantined**: they live in
//! a [`DraftStore`] apart from the mission library, so the `mission` tool cannot
//! start one. Only [`DraftStore::approve`] — an operator action, exposed on the CLI
//! and never as an agent tool — makes a draft runnable, the same Track 0 interlock
//! synthesised physical skills sit behind. A draft carries no guards; the operator
//! adds them (battery, link) to the file before approving.

use crate::{value_to_f64, Mission, MissionStep};
use obc_memory::world::WorldMemory;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where the pose trail lives in world memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PoseEntities {
    /// One entity whose value is an object with `x` and `y` (e.g. `nav.pose`).
    Combined { entity: String },
    /// Separate scalar entities (e.g. odometry's `sensor.pos_x` / `sensor.pos_y`).
    Split { x: String, y: String },
}

impl Default for PoseEntities {
    /// Odometry's raw entities: they are written whether or not navigation is
    /// running, which during teleop it usually is not.
    fn default() -> Self {
        PoseEntities::Split {
            x: "sensor.pos_x".into(),
            y: "sensor.pos_y".into(),
        }
    }
}

impl PoseEntities {
    fn describe(&self) -> String {
        match self {
            PoseEntities::Combined { entity } => entity.clone(),
            PoseEntities::Split { x, y } => format!("{x}+{y}"),
        }
    }
}

/// How a demonstration is read (`[mission.teach]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeachSettings {
    pub pose: PoseEntities,
    /// Waypoint simplification tolerance (m): driving that deviates less than
    /// this from a straight line between two kept points is dropped.
    pub simplify_m: f64,
    /// A stop at least this long (ms) becomes a `wait` step.
    pub dwell_ms: u64,
    /// Movement within this radius (m) still counts as stopped.
    pub dwell_radius_m: f64,
    /// Arrival tolerance (m) written into each `navigate_to`.
    pub tolerance_m: f64,
}

impl Default for TeachSettings {
    fn default() -> Self {
        Self {
            pose: PoseEntities::default(),
            simplify_m: 0.25,
            dwell_ms: 2_000,
            dwell_radius_m: 0.15,
            tolerance_m: 0.5,
        }
    }
}

/// One pose sample: valid from `t` (ms) at `(x, y)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: u64,
    pub x: f64,
    pub y: f64,
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn xy_of(v: &Value) -> Option<(f64, f64)> {
    Some((value_to_f64(v.get("x")?)?, value_to_f64(v.get("y")?)?))
}

/// The pose trail in `[from_ms, to_ms]`, oldest first: the pose already valid at
/// `from_ms`, every change inside the window, and the pose still valid at `to_ms`
/// (so a stop at the end has a duration).
pub fn trail_from_world(
    world: &WorldMemory,
    pose: &PoseEntities,
    from_ms: u64,
    to_ms: u64,
) -> anyhow::Result<Vec<Sample>> {
    let at = |t: u64| -> anyhow::Result<Option<(f64, f64)>> {
        Ok(match pose {
            PoseEntities::Combined { entity } => world.at(entity, t)?.and_then(|f| xy_of(&f.value)),
            PoseEntities::Split { x, y } => {
                let x = world.at(x, t)?.and_then(|f| value_to_f64(&f.value));
                let y = world.at(y, t)?.and_then(|f| value_to_f64(&f.value));
                x.zip(y)
            }
        })
    };
    let entities: Vec<&str> = match pose {
        PoseEntities::Combined { entity } => vec![entity],
        PoseEntities::Split { x, y } => vec![x, y],
    };
    let mut times = vec![from_ms, to_ms];
    for e in entities {
        times.extend(
            world
                .history(e)?
                .into_iter()
                .map(|f| f.valid_from)
                .filter(|t| (from_ms..=to_ms).contains(t)),
        );
    }
    times.sort_unstable();
    times.dedup();

    let mut trail: Vec<Sample> = Vec::new();
    for t in times {
        if let Some((x, y)) = at(t)? {
            // Keep only changes, plus the closing sample that dates the last stop.
            if t == to_ms || trail.last().is_none_or(|s| (s.x, s.y) != (x, y)) {
                trail.push(Sample { t, x, y });
            }
        }
    }
    Ok(trail)
}

/// A point the draft drives to, and how long the operator stopped there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    x: f64,
    y: f64,
    dwell_ms: u64,
}

/// Collapse every run of samples that stays within `radius` of its first sample
/// for at least `min_ms` into one point carrying the stop's duration.
fn collapse_dwells(trail: &[Sample], radius: f64, min_ms: u64) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < trail.len() {
        let anchor = trail[i];
        let mut j = i;
        while j + 1 < trail.len()
            && dist((anchor.x, anchor.y), (trail[j + 1].x, trail[j + 1].y)) <= radius
        {
            j += 1;
        }
        // The stop lasts until the platform leaves the radius (or the window ends).
        let left = trail.get(j + 1).map_or(trail[j].t, |s| s.t);
        let held = left.saturating_sub(anchor.t);
        if held >= min_ms {
            keys.push(Key {
                x: anchor.x,
                y: anchor.y,
                dwell_ms: held,
            });
            i = j + 1;
        } else {
            keys.push(Key {
                x: anchor.x,
                y: anchor.y,
                dwell_ms: 0,
            });
            i += 1;
        }
    }
    keys
}

fn perpendicular(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let len = dist(a, b);
    if len == 0.0 {
        return dist(p, a);
    }
    ((b.0 - a.0) * (a.1 - p.1) - (a.0 - p.0) * (b.1 - a.1)).abs() / len
}

/// Ramer–Douglas–Peucker over `pts[lo..=hi]`, marking survivors in `keep`.
fn rdp(pts: &[(f64, f64)], lo: usize, hi: usize, eps: f64, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let (mut worst, mut at) = (0.0, lo);
    for k in lo + 1..hi {
        let d = perpendicular(pts[k], pts[lo], pts[hi]);
        if d > worst {
            worst = d;
            at = k;
        }
    }
    if worst > eps {
        keep[at] = true;
        rdp(pts, lo, at, eps, keep);
        rdp(pts, at, hi, eps, keep);
    }
}

/// Turn a pose trail into mission steps. A stop is always kept as a waypoint;
/// the driving between stops is simplified. Stops at the very start and end are
/// recording slack, not part of the task, so they produce no `wait`.
pub fn steps_from_trail(trail: &[Sample], s: &TeachSettings) -> Vec<MissionStep> {
    let keys = collapse_dwells(trail, s.dwell_radius_m, s.dwell_ms);
    if keys.is_empty() {
        return Vec::new();
    }
    let pts: Vec<(f64, f64)> = keys.iter().map(|k| (k.x, k.y)).collect();
    let last = keys.len() - 1;
    let mut keep = vec![false; keys.len()];
    keep[0] = true;
    keep[last] = true;
    for (i, k) in keys.iter().enumerate() {
        if k.dwell_ms > 0 {
            keep[i] = true;
        }
    }
    let anchors: Vec<usize> = (0..keys.len()).filter(|&i| keep[i]).collect();
    for w in anchors.windows(2) {
        rdp(&pts, w[0], w[1], s.simplify_m, &mut keep);
    }

    let mut steps = Vec::new();
    let mut at: Option<(f64, f64)> = None;
    for (i, k) in keys.iter().enumerate().filter(|(i, _)| keep[*i]) {
        let p = (round_cm(k.x), round_cm(k.y));
        if at.is_none_or(|a| dist(a, p) > s.tolerance_m) {
            steps.push(MissionStep::NavigateTo {
                x: p.0,
                y: p.1,
                tolerance: s.tolerance_m,
            });
            at = Some(p);
        }
        if k.dwell_ms > 0 && i != 0 && i != last {
            steps.push(MissionStep::Wait {
                ms: (k.dwell_ms + 50) / 100 * 100,
            });
        }
    }
    steps
}

fn round_cm(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Whether a draft can be started yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    /// Quarantined: not in the mission library.
    Pending,
    /// An operator approved it; loaded into the library at startup.
    Approved,
}

/// Where a draft came from, so the operator can judge it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub from_ms: u64,
    pub to_ms: u64,
    pub pose_source: String,
    pub samples: usize,
    /// Distance the operator drove (m), before simplification.
    pub path_m: f64,
    pub created_ms: u64,
}

/// A mission taught by demonstration, quarantined until approved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftMission {
    pub mission: Mission,
    pub status: DraftStatus,
    pub taught: Provenance,
    #[serde(default)]
    pub approved_ms: Option<u64>,
}

/// Mission ids name draft files, so they are restricted to `[a-z0-9_-]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Build a pending draft from the pose trail in `[from_ms, to_ms]`.
pub fn draft_from_world(
    world: &WorldMemory,
    id: &str,
    from_ms: u64,
    to_ms: u64,
    settings: &TeachSettings,
    now_ms: u64,
) -> anyhow::Result<DraftMission> {
    anyhow::ensure!(
        valid_id(id),
        "mission id '{id}' must be 1-64 chars of a-z, 0-9, _ or -"
    );
    anyhow::ensure!(
        from_ms < to_ms,
        "the demonstration window is empty ({from_ms}..{to_ms})"
    );
    let trail = trail_from_world(world, &settings.pose, from_ms, to_ms)?;
    anyhow::ensure!(
        !trail.is_empty(),
        "no pose in world memory at {} during {from_ms}..{to_ms}",
        settings.pose.describe()
    );
    let steps = steps_from_trail(&trail, settings);
    anyhow::ensure!(
        steps.len() > 1,
        "the platform did not move during {from_ms}..{to_ms}; nothing to teach"
    );
    let path_m = trail
        .windows(2)
        .map(|w| dist((w[0].x, w[0].y), (w[1].x, w[1].y)))
        .sum::<f64>();
    Ok(DraftMission {
        mission: Mission {
            id: id.to_string(),
            steps,
            guards: Vec::new(),
        },
        status: DraftStatus::Pending,
        taught: Provenance {
            from_ms,
            to_ms,
            pose_source: settings.pose.describe(),
            samples: trail.len(),
            path_m: round_cm(path_m),
            created_ms: now_ms,
        },
        approved_ms: None,
    })
}

/// Drafts on disk, one JSON file per mission id. Holding a draft here is what
/// keeps it out of the mission library.
pub struct DraftStore {
    dir: PathBuf,
}

impl DraftStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(valid_id(id), "invalid mission id '{id}'");
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Option<DraftMission>> {
        let path = self.path(id)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&std::fs::read_to_string(
            &path,
        )?)?))
    }

    /// Save a pending draft. Re-teaching a pending id replaces it; an approved
    /// one is refused, since that would change a runnable mission unreviewed.
    pub fn save(&self, draft: &DraftMission) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(
            draft.status == DraftStatus::Pending,
            "only pending drafts are saved; approval goes through approve()"
        );
        if let Some(existing) = self.load(&draft.mission.id)? {
            anyhow::ensure!(
                existing.status == DraftStatus::Pending,
                "mission '{}' is already approved; reject it first to re-teach",
                draft.mission.id
            );
        }
        let path = self.path(&draft.mission.id)?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(&path, serde_json::to_string_pretty(draft)?)?;
        Ok(path)
    }

    /// Every draft, sorted by id. Unreadable files are skipped with a warning.
    pub fn list(&self) -> anyhow::Result<Vec<DraftMission>> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|s| Ok(serde_json::from_str::<DraftMission>(&s)?))
            {
                Ok(d) => out.push(d),
                Err(e) => tracing::warn!("skipping mission draft {}: {e}", path.display()),
            }
        }
        out.sort_by(|a, b| a.mission.id.cmp(&b.mission.id));
        Ok(out)
    }

    /// The approved missions, for the library.
    pub fn approved(&self) -> anyhow::Result<Vec<Mission>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|d| d.status == DraftStatus::Approved)
            .map(|d| d.mission)
            .collect())
    }

    /// Operator only: release a draft into the library (on next start).
    pub fn approve(&self, id: &str, now_ms: u64) -> anyhow::Result<DraftMission> {
        let mut draft = self
            .load(id)?
            .ok_or_else(|| anyhow::anyhow!("no mission draft '{id}'"))?;
        anyhow::ensure!(
            draft.mission.id == id,
            "draft file '{id}' holds mission '{}'",
            draft.mission.id
        );
        draft.status = DraftStatus::Approved;
        draft.approved_ms = Some(now_ms);
        std::fs::write(self.path(id)?, serde_json::to_string_pretty(&draft)?)?;
        Ok(draft)
    }

    /// Delete a draft, approved or not. Returns whether one existed.
    pub fn reject(&self, id: &str) -> anyhow::Result<bool> {
        let path = self.path(id)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings() -> TeachSettings {
        TeachSettings::default()
    }

    fn drive(world: &WorldMemory, path: &[(u64, f64, f64)]) {
        for &(t, x, y) in path {
            world
                .observe("sensor.pos_x", json!({ "value": x }), t, t, "odom")
                .unwrap();
            world
                .observe("sensor.pos_y", json!({ "value": y }), t, t, "odom")
                .unwrap();
        }
    }

    fn navs(steps: &[MissionStep]) -> Vec<(f64, f64)> {
        steps
            .iter()
            .filter_map(|s| match s {
                MissionStep::NavigateTo { x, y, .. } => Some((*x, *y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn straight_drive_simplifies_to_start_and_end() {
        let world = WorldMemory::open_in_memory().unwrap();
        let path: Vec<_> = (0..=10)
            .map(|i| (1_000 + i * 200, i as f64 * 0.3, 0.01 * (i % 2) as f64))
            .collect();
        drive(&world, &path);
        let d = draft_from_world(&world, "line", 1_000, 3_000, &settings(), 9).unwrap();
        assert_eq!(navs(&d.mission.steps), vec![(0.0, 0.0), (3.0, 0.0)]);
        assert_eq!(d.status, DraftStatus::Pending);
        assert!(d.mission.guards.is_empty());
        assert!((d.taught.path_m - 3.0).abs() < 0.05);
    }

    #[test]
    fn a_corner_is_kept_and_a_stop_becomes_a_wait() {
        let world = WorldMemory::open_in_memory().unwrap();
        // Out along x, stop 5 s at the corner (with jitter), then up along y.
        drive(
            &world,
            &[
                (0, 0.0, 0.0),
                (1_000, 1.0, 0.0),
                (2_000, 2.0, 0.0),
                (2_500, 2.05, 0.02),
                (7_000, 2.0, 1.0),
                (8_000, 2.0, 2.0),
            ],
        );
        let d = draft_from_world(&world, "corner", 0, 8_000, &settings(), 9).unwrap();
        assert_eq!(
            d.mission.steps,
            vec![
                MissionStep::NavigateTo {
                    x: 0.0,
                    y: 0.0,
                    tolerance: 0.5
                },
                MissionStep::NavigateTo {
                    x: 2.0,
                    y: 0.0,
                    tolerance: 0.5
                },
                MissionStep::Wait { ms: 5_000 },
                MissionStep::NavigateTo {
                    x: 2.0,
                    y: 2.0,
                    tolerance: 0.5
                },
            ]
        );
    }

    #[test]
    fn idle_slack_at_either_end_is_not_a_wait() {
        let world = WorldMemory::open_in_memory().unwrap();
        // Pose set long before the window; the operator starts driving late and
        // leaves recording on after arriving.
        drive(
            &world,
            &[(0, 0.0, 0.0), (20_000, 1.0, 0.0), (21_000, 2.0, 0.0)],
        );
        let d = draft_from_world(&world, "slack", 10_000, 40_000, &settings(), 9).unwrap();
        assert!(d
            .mission
            .steps
            .iter()
            .all(|s| !matches!(s, MissionStep::Wait { .. })));
        assert_eq!(navs(&d.mission.steps), vec![(0.0, 0.0), (2.0, 0.0)]);
    }

    #[test]
    fn combined_pose_entity_is_read() {
        let world = WorldMemory::open_in_memory().unwrap();
        for (t, x) in [(0u64, 0.0), (1_000, 1.5)] {
            world
                .observe(
                    "nav.pose",
                    json!({ "x": x, "y": 0.0, "heading_deg": 0 }),
                    t,
                    t,
                    "nav",
                )
                .unwrap();
        }
        let s = TeachSettings {
            pose: PoseEntities::Combined {
                entity: "nav.pose".into(),
            },
            ..settings()
        };
        let d = draft_from_world(&world, "nav", 0, 1_000, &s, 9).unwrap();
        assert_eq!(navs(&d.mission.steps), vec![(0.0, 0.0), (1.5, 0.0)]);
        assert_eq!(d.taught.pose_source, "nav.pose");
    }

    #[test]
    fn no_movement_or_no_pose_is_refused() {
        let world = WorldMemory::open_in_memory().unwrap();
        assert!(draft_from_world(&world, "none", 0, 1_000, &settings(), 9).is_err());
        drive(&world, &[(0, 1.0, 1.0), (500, 1.05, 1.0)]);
        let err = draft_from_world(&world, "still", 0, 1_000, &settings(), 9).unwrap_err();
        assert!(err.to_string().contains("did not move"), "{err}");
        assert!(draft_from_world(&world, "../escape", 0, 1_000, &settings(), 9).is_err());
    }

    #[test]
    fn drafts_stay_out_of_the_library_until_approved() {
        let dir = std::env::temp_dir().join(format!("obc-teach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = DraftStore::new(&dir);
        let world = WorldMemory::open_in_memory().unwrap();
        drive(&world, &[(0, 0.0, 0.0), (1_000, 2.0, 0.0)]);
        let draft = draft_from_world(&world, "patrol", 0, 1_000, &settings(), 9).unwrap();

        store.save(&draft).unwrap();
        assert!(store.approved().unwrap().is_empty());
        assert_eq!(store.list().unwrap().len(), 1);

        // Re-teaching a pending draft replaces it.
        store.save(&draft).unwrap();
        let approved = store.approve("patrol", 42).unwrap();
        assert_eq!(approved.approved_ms, Some(42));
        assert_eq!(store.approved().unwrap(), vec![draft.mission.clone()]);

        // An approved mission is not silently overwritten by a new demonstration.
        assert!(store.save(&draft).is_err());
        assert!(store.approve("missing", 1).is_err());
        assert!(store.reject("patrol").unwrap());
        assert!(store.approved().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

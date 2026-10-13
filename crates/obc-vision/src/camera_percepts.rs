//! Camera events as percepts: what the mushroom body is surprised by.
//!
//! `TrajectoryStore::perceive` is the novelty detector's input from the world
//! (`docs/NEUROMORPHIC-2026-09.md` §5), and its docs set the rule this module
//! exists to keep: *a high-rate sensor must be reduced to events before it
//! comes here; it must not be fed a frame at a time.* A camera at one frame a
//! second is exactly that sensor. Until 2026-10-13 no camera reached it; the
//! novelty detector had no real subject at all (§8 item 2: "Nothing perceives
//! yet").
//!
//! [`CameraPercepts`] watches the facts [`CameraHealth`](super::camera_health)
//! writes (`camera.{node}` and `camera.{node}.motion`) and turns them into a
//! handful of sentences:
//!
//! - **entering** `lighting_shift`, `nudged`, `blind` or `failed`. A change
//!   into a state, never the state persisting, and never a return to `ok` or
//!   `warming`: coming back to normal is not news.
//! - **the start of a motion episode**: motion after [`MOTION_EPISODE_GAP_MS`]
//!   without any. A person crossing the room is one percept, not twenty.
//!
//! Percepts that are about *when* carry the hour (UTC), so the embedder can
//! tell "motion at 03:00" from the daytime traffic it has seen a hundred
//! times. That is the point of feeding novelty at all: the lamp again is not
//! news; the 3 a.m. lamp is.
//!
//! It reads world memory rather than being called by `CameraHealth`, because
//! the two run in different processes: `oh-ben-claw camera-watch` writes the
//! facts, and the mushroom body lives in memory in the agent (`start`).

use obc_memory::trajectory::TrajectoryStore;
use obc_memory::world::WorldMemory;

/// Quiet time that ends a motion episode: the next motion starts a new one.
pub const MOTION_EPISODE_GAP_MS: u64 = 60_000;

/// One event worth being surprised by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Percept {
    /// Stable per source; with `ts_ms` it is the percept's identity, so a
    /// re-read of the same fact is not a second percept.
    pub subject: String,
    /// The sentence the embedder sees.
    pub text: String,
    /// When it happened (the fact's valid-from), not when it was read.
    pub ts_ms: u64,
}

/// Turns one camera's health facts into percepts. Keep one per camera and
/// call [`tick`](Self::tick) on a cadence.
pub struct CameraPercepts {
    node: String,
    last_mode: Option<String>,
    last_motion_fact: Option<i64>,
    last_motion_ms: Option<u64>,
}

fn hour_utc(ts_ms: u64) -> u64 {
    (ts_ms / 3_600_000) % 24
}

impl CameraPercepts {
    pub fn new(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            last_mode: None,
            last_motion_fact: None,
            last_motion_ms: None,
        }
    }

    /// What changed since the last tick, as percepts. Reads only; feed the
    /// result to [`TrajectoryStore::perceive`] (or use [`Self::feed`]).
    pub fn tick(&mut self, world: &WorldMemory) -> anyhow::Result<Vec<Percept>> {
        let mut out = Vec::new();
        let node = &self.node;
        let health = format!("camera.{node}");

        if let Some(fact) = world.current(&health)? {
            let mode = fact.value["mode"].as_str().unwrap_or("").to_string();
            let changed = self.last_mode.as_deref() != Some(mode.as_str());
            let ts = fact.valid_from;
            let h = hour_utc(ts);
            let text = match mode.as_str() {
                "lighting_shift" => Some(format!(
                    "the light changed at camera {node} around {h:02}:00 UTC"
                )),
                "nudged" => Some(format!("camera {node} was moved around {h:02}:00 UTC")),
                "blind" => Some(format!("camera {node} went blind")),
                "failed" => Some(format!("camera {node} stopped capturing")),
                // ok, warming, and anything newer: a return to normal is not news.
                _ => None,
            };
            if let (true, Some(text)) = (changed, text) {
                out.push(Percept {
                    subject: health.clone(),
                    text,
                    ts_ms: ts,
                });
            }
            self.last_mode = Some(mode);
        }

        if let Some(fact) = world.current(&format!("{health}.motion"))? {
            if self.last_motion_fact != Some(fact.id) {
                let ts = fact.valid_from;
                let new_episode = self
                    .last_motion_ms
                    .is_none_or(|prev| ts.saturating_sub(prev) >= MOTION_EPISODE_GAP_MS);
                if new_episode {
                    out.push(Percept {
                        subject: format!("{health}.motion"),
                        text: format!("motion at camera {node} around {:02}:00 UTC", hour_utc(ts)),
                        ts_ms: ts,
                    });
                }
                self.last_motion_fact = Some(fact.id);
                self.last_motion_ms = Some(ts);
            }
        }
        Ok(out)
    }

    /// [`tick`](Self::tick), then hand each percept to the mushroom body.
    /// Returns how many were new.
    pub fn feed(&mut self, world: &WorldMemory, traj: &TrajectoryStore) -> anyhow::Result<usize> {
        let mut fed = 0;
        for p in self.tick(world)? {
            if traj.perceive(&p.subject, &p.text, p.ts_ms)? {
                fed += 1;
            }
        }
        Ok(fed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOUR: u64 = 3_600_000;

    fn health(world: &WorldMemory, mode: &str, ts: u64) {
        world
            .observe("camera.cam0", json!({ "mode": mode }), ts, ts, "camera")
            .unwrap();
    }

    fn motion(world: &WorldMemory, ts: u64) {
        world
            .observe(
                "camera.cam0.motion",
                json!({ "frac": 0.6, "edge": 6.0 }),
                ts,
                ts,
                "camera",
            )
            .unwrap();
    }

    #[test]
    fn only_entering_a_notable_state_is_a_percept() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut p = CameraPercepts::new("cam0");
        health(&world, "warming", 1);
        assert!(p.tick(&world).unwrap().is_empty());
        health(&world, "ok", 2);
        assert!(p.tick(&world).unwrap().is_empty());
        health(&world, "lighting_shift", 3 * HOUR);
        let got = p.tick(&world).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].text,
            "the light changed at camera cam0 around 03:00 UTC"
        );
        // Persisting is not news, and neither is coming back.
        assert!(p.tick(&world).unwrap().is_empty());
        health(&world, "ok", 3 * HOUR + 1);
        assert!(p.tick(&world).unwrap().is_empty());
        health(&world, "blind", 4 * HOUR);
        assert_eq!(p.tick(&world).unwrap()[0].text, "camera cam0 went blind");
    }

    #[test]
    fn a_motion_episode_is_one_percept_until_a_quiet_minute() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut p = CameraPercepts::new("cam0");
        let t0 = 14 * HOUR;
        let mut texts = Vec::new();
        // Twenty motion frames a second apart: one person crossing.
        for i in 0..20 {
            motion(&world, t0 + i * 1_000);
            texts.extend(p.tick(&world).unwrap().into_iter().map(|x| x.text));
        }
        assert_eq!(texts, ["motion at camera cam0 around 14:00 UTC"]);
        // After a quiet minute, the next motion is a new episode.
        motion(&world, t0 + 19_000 + MOTION_EPISODE_GAP_MS);
        assert_eq!(p.tick(&world).unwrap().len(), 1);
    }

    #[test]
    fn feed_reaches_the_trajectory_store_once_per_event() {
        let world = WorldMemory::open_in_memory().unwrap();
        let traj = TrajectoryStore::open_in_memory().unwrap();
        let mut p = CameraPercepts::new("cam0");
        health(&world, "failed", 5);
        assert_eq!(p.feed(&world, &traj).unwrap(), 1);
        // A fresh reader re-reading the same fact (an agent restart) does not
        // perceive it twice: the id is the subject and the fact's own time.
        let mut again = CameraPercepts::new("cam0");
        assert_eq!(again.feed(&world, &traj).unwrap(), 0);
        let rows = traj.recent(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].objective, "camera cam0 stopped capturing");
    }
}

//! Track 0 staged rollout for learned skills (Phase 16 P3).
//!
//! A synthesized skill that can touch the physical world climbs
//! `simulate → supervised → autonomous`. The **stage lives in the skill
//! manifest** (single source of truth, ClawHub-compatible extra field); this
//! module owns the **run record** — clean runs and failures per skill at its
//! current stage, persisted as JSON — and the promotion/demotion operations
//! that rewrite the manifest, gated on that record.
//!
//! Promotion is always operator-initiated (CLI `oh-ben-claw skill promote`,
//! or the gateway endpoint) and is refused unless the skill has accumulated
//! the required number of clean runs at its current stage with zero failures.
//! Demotion is unconditional; the agent also auto-demotes a supervised skill
//! to `simulate` on its first real-run failure.

use super::SkillForge;
use crate::tools::traits::RolloutStage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Per-skill run record at its current stage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RolloutRecord {
    /// The stage the counts below were accumulated at.
    pub stage: RolloutStage,
    /// Clean (successful) runs at `stage` since the last stage change/reset.
    pub clean_runs: u32,
    /// Failures at `stage` since the last stage change/reset.
    pub failures: u32,
    /// Wall-clock ms of the last recorded run.
    pub last_run_ms: u64,
}

/// Persisted clean-run/failure record per skill (JSON file, mutex-guarded).
pub struct RolloutTracker {
    path: PathBuf,
    state: Mutex<HashMap<String, RolloutRecord>>,
}

impl RolloutTracker {
    /// Load (or start empty at) the given path.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    /// The default record path (`~/.oh-ben-claw/skill_rollout.json`).
    pub fn default_path() -> PathBuf {
        directories::UserDirs::new()
            .map(|d| d.home_dir().join(".oh-ben-claw"))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("skill_rollout.json")
    }

    fn persist(&self, state: &HashMap<String, RolloutRecord>) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(state) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    tracing::warn!(error = %e, "failed to persist skill rollout record");
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to serialize skill rollout record"),
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Record a clean run of `skill` at `stage`. If the recorded stage
    /// differs (skill was promoted/demoted out-of-band), counts reset first.
    pub fn record_clean(&self, skill: &str, stage: RolloutStage) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let rec = state.entry(skill.to_string()).or_default();
        if rec.stage != stage {
            *rec = RolloutRecord {
                stage,
                ..Default::default()
            };
        }
        rec.clean_runs += 1;
        rec.last_run_ms = Self::now_ms();
        self.persist(&state);
    }

    /// Record a failed run of `skill` at `stage` (same stage-reset rule).
    pub fn record_failure(&self, skill: &str, stage: RolloutStage) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let rec = state.entry(skill.to_string()).or_default();
        if rec.stage != stage {
            *rec = RolloutRecord {
                stage,
                ..Default::default()
            };
        }
        rec.failures += 1;
        rec.last_run_ms = Self::now_ms();
        self.persist(&state);
    }

    /// The record for `skill`, if any.
    pub fn record(&self, skill: &str) -> Option<RolloutRecord> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(skill)
            .cloned()
    }

    /// Reset the record for `skill` to a fresh count at `stage` (used after a
    /// stage change, or by the operator to clear failures).
    pub fn reset(&self, skill: &str, stage: RolloutStage) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.insert(
            skill.to_string(),
            RolloutRecord {
                stage,
                ..Default::default()
            },
        );
        self.persist(&state);
    }

    /// All records (for `skill list` / the gateway view).
    pub fn all(&self) -> HashMap<String, RolloutRecord> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

fn find_manifest(forge: &SkillForge, name: &str) -> anyhow::Result<super::SkillManifest> {
    forge
        .list_manifests()?
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| anyhow::anyhow!("no skill named '{name}' in {}", forge.skill_dir.display()))
}

/// Promote `name` one stage up, gated on its clean-run record: refused unless
/// the skill has ≥ `required_clean` clean runs **at its current stage** and
/// zero failures on record. Rewrites the manifest and resets the record at
/// the new stage. Returns the new stage.
pub fn promote(
    forge: &SkillForge,
    tracker: &RolloutTracker,
    name: &str,
    required_clean: u32,
) -> anyhow::Result<RolloutStage> {
    let mut manifest = find_manifest(forge, name)?;
    let current = manifest.stage;
    let next = current
        .next()
        .ok_or_else(|| anyhow::anyhow!("'{name}' is already autonomous"))?;

    let rec = tracker.record(name).unwrap_or_default();
    let (clean, failures) = if rec.stage == current {
        (rec.clean_runs, rec.failures)
    } else {
        (0, 0) // record predates the current stage
    };
    if failures > 0 {
        anyhow::bail!(
            "'{name}' has {failures} failure(s) on record at stage {} — investigate, then \
             `skill reset-record` to re-earn a clean record",
            current.as_str()
        );
    }
    if clean < required_clean {
        anyhow::bail!(
            "'{name}' has {clean}/{required_clean} clean runs at stage {} — promotion refused \
             (Track 0: promotion is gated on a clean record)",
            current.as_str()
        );
    }

    manifest.stage = next;
    forge.install_skill(&manifest)?;
    tracker.reset(name, next);
    tracing::info!(skill = %name, from = current.as_str(), to = next.as_str(), "skill promoted");
    Ok(next)
}

/// Demote `name` one stage down (unconditional — demotion is always safe).
/// Rewrites the manifest and resets the record at the new stage.
pub fn demote(
    forge: &SkillForge,
    tracker: &RolloutTracker,
    name: &str,
) -> anyhow::Result<RolloutStage> {
    let mut manifest = find_manifest(forge, name)?;
    let current = manifest.stage;
    let prev = current
        .prev()
        .ok_or_else(|| anyhow::anyhow!("'{name}' is already at the lowest stage (simulate)"))?;
    manifest.stage = prev;
    forge.install_skill(&manifest)?;
    tracker.reset(name, prev);
    tracing::info!(skill = %name, from = current.as_str(), to = prev.as_str(), "skill demoted");
    Ok(prev)
}

/// Convenience for the tracker file inside an explicit directory (tests).
pub fn tracker_in(dir: &Path) -> RolloutTracker {
    RolloutTracker::load(dir.join("skill_rollout.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_forge::{SkillKind, SkillManifest};
    use serde_json::json;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("obc-rollout-{tag}-{nanos}"))
    }

    fn simulate_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: "physical learned skill".to_string(),
            kind: SkillKind::Delegate {
                tool: "gpio_write".to_string(),
                fixed_args: json!({"pin": 17, "value": 1}),
            },
            parameters: json!({ "type": "object", "properties": {} }),
            version: Some("0.1.0-learned".to_string()),
            stage: RolloutStage::Simulate,
            tags: vec!["learned".to_string(), "track0:supervised".to_string()],
            enabled: true,
            timeout_secs: 30,
        }
    }

    #[test]
    fn record_accumulates_and_resets_on_stage_change() {
        let dir = tmp_dir("rec");
        let tracker = tracker_in(&dir);
        tracker.record_clean("s", RolloutStage::Simulate);
        tracker.record_clean("s", RolloutStage::Simulate);
        assert_eq!(tracker.record("s").unwrap().clean_runs, 2);
        // A run at a different stage resets the count.
        tracker.record_clean("s", RolloutStage::Supervised);
        let rec = tracker.record("s").unwrap();
        assert_eq!(rec.stage, RolloutStage::Supervised);
        assert_eq!(rec.clean_runs, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_persists_across_reload() {
        let dir = tmp_dir("persist");
        {
            let tracker = tracker_in(&dir);
            tracker.record_clean("s", RolloutStage::Simulate);
        }
        let reloaded = tracker_in(&dir);
        assert_eq!(reloaded.record("s").unwrap().clean_runs, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn promote_requires_clean_record_and_no_failures() {
        let dir = tmp_dir("promote");
        let forge = SkillForge::new(&dir);
        forge.install_skill(&simulate_manifest("learned_unlock")).unwrap();
        let tracker = tracker_in(&dir);

        // No record at all → refused.
        assert!(promote(&forge, &tracker, "learned_unlock", 3).is_err());

        // Two clean runs, need three → refused.
        tracker.record_clean("learned_unlock", RolloutStage::Simulate);
        tracker.record_clean("learned_unlock", RolloutStage::Simulate);
        let err = promote(&forge, &tracker, "learned_unlock", 3).unwrap_err();
        assert!(err.to_string().contains("2/3"), "{err}");

        // Third clean run → promoted to supervised; record reset.
        tracker.record_clean("learned_unlock", RolloutStage::Simulate);
        let next = promote(&forge, &tracker, "learned_unlock", 3).unwrap();
        assert_eq!(next, RolloutStage::Supervised);
        let m = forge
            .list_manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "learned_unlock")
            .unwrap();
        assert_eq!(m.stage, RolloutStage::Supervised);
        assert_eq!(tracker.record("learned_unlock").unwrap().clean_runs, 0);

        // A failure at the new stage blocks the next promotion.
        tracker.record_clean("learned_unlock", RolloutStage::Supervised);
        tracker.record_clean("learned_unlock", RolloutStage::Supervised);
        tracker.record_clean("learned_unlock", RolloutStage::Supervised);
        tracker.record_failure("learned_unlock", RolloutStage::Supervised);
        let err = promote(&forge, &tracker, "learned_unlock", 3).unwrap_err();
        assert!(err.to_string().contains("failure"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn demote_is_unconditional_and_bounded() {
        let dir = tmp_dir("demote");
        let forge = SkillForge::new(&dir);
        let mut m = simulate_manifest("learned_x");
        m.stage = RolloutStage::Autonomous;
        forge.install_skill(&m).unwrap();
        let tracker = tracker_in(&dir);

        assert_eq!(demote(&forge, &tracker, "learned_x").unwrap(), RolloutStage::Supervised);
        assert_eq!(demote(&forge, &tracker, "learned_x").unwrap(), RolloutStage::Simulate);
        assert!(demote(&forge, &tracker, "learned_x").is_err(), "floor reached");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn promote_unknown_skill_errors() {
        let dir = tmp_dir("missing");
        let forge = SkillForge::new(&dir);
        let tracker = tracker_in(&dir);
        assert!(promote(&forge, &tracker, "nope", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

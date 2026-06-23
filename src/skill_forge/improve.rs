//! Self-improvement loop (Phase 16).
//!
//! Closes the experiential learning loop: scan successful trajectory episodes →
//! synthesize candidate skills → verify the non-physical ones by replay →
//! install the verified ones (enabled), and surface physical ones as
//! quarantined, operator-promotable candidates (Track 0 interlock).
//!
//! Safety invariants:
//! - **Physical skills are never auto-verified by replay** (re-running an
//!   actuator would re-actuate the world). They are tagged `track0:supervised`
//!   and installed **disabled** for staged operator promotion; `load_all` skips
//!   disabled skills, so they cannot run until a human enables them.
//! - Only non-physical skills are verified-by-replay and auto-enabled, and only
//!   up to `max_learned`.

use super::synthesis::{approve, synthesize, tag_physical, touches_actuator};
use super::{SkillForge, SkillKind};
use crate::memory::trajectory::{Outcome, TrajectoryStore};
use crate::tools::traits::{BlastRadius, RiskClass};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

/// Something that can re-run a tool to verify a synthesized skill.
#[async_trait::async_trait]
pub trait ReplayExecutor: Send + Sync {
    /// Replay a tool call and report whether it succeeded.
    async fn replay(&self, tool: &str, args: &Value) -> Outcome;

    /// The declared physical-risk of a tool by name. Defaults to safe (so an
    /// executor that doesn't track risk treats tools as replayable).
    fn risk_of(&self, _tool: &str) -> RiskClass {
        RiskClass::safe()
    }
}

/// The agent itself is a replay executor (runs the tool through its normal
/// chokepoint, including policy + Track 0).
#[async_trait::async_trait]
impl ReplayExecutor for crate::agent::Agent {
    async fn replay(&self, tool: &str, args: &Value) -> Outcome {
        match self.execute_tool_direct(tool, args.clone()).await {
            Ok(r) if r.success => Outcome::Success,
            _ => Outcome::Failure,
        }
    }

    fn risk_of(&self, tool: &str) -> RiskClass {
        self.tool_risk(tool)
    }
}

/// Whether a tool is safe to re-run for verification: no physical effect,
/// reversible, and no real-world blast radius.
fn safe_to_replay(risk: RiskClass) -> bool {
    risk.reversible && !risk.physical && matches!(risk.blast, BlastRadius::None)
}

/// Summary of one improvement pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImproveReport {
    /// Successful episodes scanned.
    pub episodes_scanned: usize,
    /// Candidate skills synthesized.
    pub candidates: usize,
    /// Verified, non-physical skills installed (enabled).
    pub installed: Vec<String>,
    /// Physical skills installed disabled, awaiting operator promotion.
    pub pending_supervised: Vec<String>,
    /// Candidates that failed verification or hit the capacity limit.
    pub rejected: Vec<String>,
    /// Candidates skipped because a skill of that name already exists.
    pub skipped_existing: usize,
}

/// Drives the scan → synthesize → verify → install cycle.
pub struct SkillImprover {
    trajectory: Arc<TrajectoryStore>,
    forge: SkillForge,
    /// Tool names treated as physical/actuator (Track 0 interlock).
    physical_tools: Vec<String>,
    /// Cap on auto-installed learned skills.
    max_learned: usize,
}

impl SkillImprover {
    /// Create an improver.
    pub fn new(
        trajectory: Arc<TrajectoryStore>,
        forge: SkillForge,
        physical_tools: Vec<String>,
        max_learned: usize,
    ) -> Self {
        Self {
            trajectory,
            forge,
            physical_tools,
            max_learned,
        }
    }

    /// Run one improvement pass over successful episodes recorded at or after
    /// `since_ts_ms`. Returns a report; never enables a physical skill.
    pub async fn run_once(
        &self,
        executor: &dyn ReplayExecutor,
        since_ts_ms: u64,
    ) -> anyhow::Result<ImproveReport> {
        let mut report = ImproveReport::default();
        let episodes = self.trajectory.successful_since(since_ts_ms)?;
        report.episodes_scanned = episodes.len();

        let existing: HashSet<String> = self
            .forge
            .list_manifests()
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.name)
            .collect();
        let mut learned_installed = existing.iter().filter(|n| n.starts_with("learned_")).count();
        let mut seen: HashSet<String> = HashSet::new();
        let physical_refs: Vec<&str> = self.physical_tools.iter().map(|s| s.as_str()).collect();

        for ep in &episodes {
            let Some(candidate) = synthesize(ep) else {
                continue;
            };
            report.candidates += 1;

            if existing.contains(&candidate.name) || !seen.insert(candidate.name.clone()) {
                report.skipped_existing += 1;
                continue;
            }

            // Quarantine (never auto-verify/enable) if the skill is unsafe to
            // re-run: any actuator in the episode (Track 0 name list) OR the
            // delegate tool's declared RiskClass is not safe to replay
            // (irreversible / has blast radius / physical). Such skills are
            // installed disabled for staged operator promotion.
            let delegate_tool: Option<&str> = match &candidate.kind {
                SkillKind::Delegate { tool, .. } => Some(tool.as_str()),
                _ => None,
            };
            let replay_unsafe = match delegate_tool {
                Some(t) => physical_refs.contains(&t) || !safe_to_replay(executor.risk_of(t)),
                None => true,
            };
            if touches_actuator(ep, &physical_refs) || replay_unsafe {
                let supervised = tag_physical(candidate); // stays disabled
                if self.forge.install_skill(&supervised).is_ok() {
                    report.pending_supervised.push(supervised.name);
                }
                continue;
            }

            // Capacity guard for auto-enabled skills.
            if learned_installed >= self.max_learned {
                report.rejected.push(candidate.name);
                continue;
            }

            // Verify by replay, then approve + install.
            let name = candidate.name.clone();
            let verified = match &candidate.kind {
                SkillKind::Delegate { tool, fixed_args } => {
                    executor.replay(tool, fixed_args).await == Outcome::Success
                }
                // Non-delegate learned skills aren't produced yet; don't auto-trust.
                _ => false,
            };
            let installed_ok = if verified {
                let approved = approve(candidate);
                self.forge.install_skill(&approved).is_ok()
            } else {
                false
            };
            if installed_ok {
                report.installed.push(name);
                learned_installed += 1;
            } else {
                report.rejected.push(name);
            }
        }

        Ok(report)
    }

    /// Run the improvement loop forever on a fixed cadence. Spawn this on a
    /// background task; `executor` is typically the agent (`Arc<Agent>`). The
    /// first pass runs immediately, then every `interval`.
    pub async fn run_periodically(
        self,
        executor: Arc<dyn ReplayExecutor>,
        interval: std::time::Duration,
    ) {
        let mut since: u64 = 0;
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let pass_start = now_ms();
            match self.run_once(executor.as_ref(), since).await {
                Ok(rep) => {
                    if !rep.installed.is_empty() || !rep.pending_supervised.is_empty() {
                        tracing::info!(
                            installed = rep.installed.len(),
                            pending_supervised = rep.pending_supervised.len(),
                            rejected = rep.rejected.len(),
                            "Phase 16 self-improvement pass"
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "Phase 16 self-improvement pass failed"),
            }
            // Next pass scans episodes recorded from this pass onward.
            since = pass_start;
        }
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::trajectory::{Episode, EpisodeStep};
    use serde_json::json;

    struct MockExec(Outcome);
    #[async_trait::async_trait]
    impl ReplayExecutor for MockExec {
        async fn replay(&self, _tool: &str, _args: &Value) -> Outcome {
            self.0
        }
        // risk_of uses the trait default (safe) — these tools are replayable.
    }

    struct MockExecWithRisk {
        outcome: Outcome,
        risk: RiskClass,
    }
    #[async_trait::async_trait]
    impl ReplayExecutor for MockExecWithRisk {
        async fn replay(&self, _tool: &str, _args: &Value) -> Outcome {
            self.outcome
        }
        fn risk_of(&self, _tool: &str) -> RiskClass {
            self.risk
        }
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("obc-improve-{tag}-{nanos}"));
        p
    }

    fn episode(id: &str, objective: &str, tool: &str) -> Episode {
        Episode {
            id: id.to_string(),
            session_id: "s".to_string(),
            objective: objective.to_string(),
            steps: vec![EpisodeStep {
                tool: tool.to_string(),
                args: json!({"q": "weather"}),
                result: "ok".to_string(),
                ok: true,
            }],
            outcome: Outcome::Success,
            ts_ms: 10,
        }
    }

    #[tokio::test]
    async fn verifies_and_installs_nonphysical_skill() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "check the weather", "http")).unwrap();
        let dir = tmp_dir("install");
        let improver = SkillImprover::new(
            Arc::clone(&traj),
            SkillForge::new(&dir),
            vec!["gpio_write".to_string()],
            100,
        );

        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert_eq!(report.episodes_scanned, 1);
        assert_eq!(report.candidates, 1);
        assert_eq!(report.installed, vec!["learned_check_the_weather".to_string()]);
        assert!(report.pending_supervised.is_empty());
        // installed manifest is enabled
        let manifests = SkillForge::new(&dir).list_manifests().unwrap();
        let m = manifests.iter().find(|m| m.name == "learned_check_the_weather").unwrap();
        assert!(m.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_skill_that_fails_replay() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "do a thing", "http")).unwrap();
        let dir = tmp_dir("reject");
        let improver =
            SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);
        let report = improver.run_once(&MockExec(Outcome::Failure), 0).await.unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(report.rejected, vec!["learned_do_a_thing".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn physical_skill_is_quarantined_not_enabled() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "unlock the door", "gpio_write")).unwrap();
        let dir = tmp_dir("physical");
        let improver = SkillImprover::new(
            Arc::clone(&traj),
            SkillForge::new(&dir),
            vec!["gpio_write".to_string()],
            100,
        );
        // Even with a "success" executor, physical skills must NOT be replayed/enabled.
        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(
            report.pending_supervised,
            vec!["learned_unlock_the_door".to_string()]
        );
        let manifests = SkillForge::new(&dir).list_manifests().unwrap();
        let m = manifests
            .iter()
            .find(|m| m.name == "learned_unlock_the_door")
            .unwrap();
        assert!(!m.enabled, "physical learned skill must stay disabled");
        assert!(m.tags.iter().any(|t| t == "track0:supervised"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unsafe_risk_tool_quarantined_without_name_list() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "clean up files", "shell")).unwrap();
        let dir = tmp_dir("risk");
        // Empty actuator name list — gating relies purely on declared RiskClass.
        let improver = SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);
        let exec = MockExecWithRisk {
            outcome: Outcome::Success,
            // Non-physical but irreversible with a blast radius (e.g. `shell`).
            risk: RiskClass {
                reversible: false,
                blast: BlastRadius::Low,
                physical: false,
            },
        };
        let report = improver.run_once(&exec, 0).await.unwrap();
        assert!(report.installed.is_empty(), "side-effecting tool must not auto-install");
        assert_eq!(
            report.pending_supervised,
            vec!["learned_clean_up_files".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skips_already_installed_skill() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "check weather", "http")).unwrap();
        let dir = tmp_dir("dedup");
        let improver =
            SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);
        let first = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert_eq!(first.installed.len(), 1);
        let second = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert_eq!(second.installed.len(), 0);
        assert_eq!(second.skipped_existing, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

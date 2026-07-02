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

use super::synthesis::{
    approve, chain_signature, parameterize, synthesize, tag_physical, touches_actuator,
    VerificationCheck,
};
use super::{SkillForge, SkillManifest};
use crate::memory::trajectory::{Episode, Outcome, TrajectoryStore};
use crate::tools::traits::{BlastRadius, RiskClass};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
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

    /// Called after an improvement pass changed the installed skill set, so a
    /// live agent can hot-reload its tool registry. Default: no-op.
    fn on_skills_changed(&self, _forge: &SkillForge) {}

    /// Like [`ReplayExecutor::replay`], but also captures the tool's output
    /// text (needed for `SensorAssertion` verification). Default: replay with
    /// no captured output.
    async fn replay_capture(&self, tool: &str, args: &Value) -> (Outcome, String) {
        (self.replay(tool, args).await, String::new())
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

    fn on_skills_changed(&self, forge: &SkillForge) {
        let (added, removed, shadowed) = self.sync_skills(forge);
        if added + removed + shadowed > 0 {
            tracing::info!(
                added,
                removed,
                shadowed,
                "Agent tool registry synced with skill forge"
            );
        }
    }

    async fn replay_capture(&self, tool: &str, args: &Value) -> (Outcome, String) {
        match self.execute_tool_direct(tool, args.clone()).await {
            Ok(r) if r.success => (Outcome::Success, r.output),
            Ok(r) => (Outcome::Failure, r.error.unwrap_or_default()),
            Err(e) => (Outcome::Failure, e.to_string()),
        }
    }
}

/// A configured verification requirement applied to synthesized skills whose
/// name matches `skill_pattern` (exact, or prefix with a trailing `*`).
#[derive(Debug, Clone)]
pub struct VerificationRule {
    pub skill_pattern: String,
    pub check: VerificationCheck,
}

fn pattern_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

/// Run a shell command on the host and return its exit code (platform-aware).
pub(crate) async fn run_host_command(cmd: &str) -> i32 {
    #[cfg(windows)]
    let mut process = {
        let mut p = tokio::process::Command::new("cmd");
        p.arg("/C").arg(cmd);
        p
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut p = tokio::process::Command::new("sh");
        p.arg("-c").arg(cmd);
        p
    };
    match tokio::time::timeout(std::time::Duration::from_secs(60), process.output()).await {
        Ok(Ok(out)) => out.status.code().unwrap_or(-1),
        _ => -1,
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
    /// Optional metrics: `self_improve_*` counters per pass.
    obs: Option<Arc<crate::observability::ObsContext>>,
    /// Configured verification requirements (`[[self_improvement.verification]]`).
    rules: Vec<VerificationRule>,
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
            obs: None,
            rules: Vec::new(),
        }
    }

    /// Attach an observability context: each pass increments
    /// `self_improve_{scanned,candidates,installed,quarantined,rejected}_total`.
    pub fn with_obs(mut self, obs: Arc<crate::observability::ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    /// Attach configured verification rules. Matching non-physical candidates
    /// must pass **all** their rules (in addition to replay) to be enabled;
    /// matching physical candidates run their read-only `SensorAssertion`
    /// rules and, when all pass, are tagged `track0:sensor-verified` (still
    /// installed disabled — an operator promotes them).
    pub fn with_verification_rules(mut self, rules: Vec<VerificationRule>) -> Self {
        self.rules = rules;
        self
    }

    /// Evaluate one configured check. `replay_outcome` is the outcome of the
    /// candidate's replay (or the episode's recorded outcome for physical
    /// skills, which are never replayed).
    async fn check_passes(
        &self,
        executor: &dyn ReplayExecutor,
        check: &VerificationCheck,
        replay_outcome: Outcome,
    ) -> bool {
        match check {
            VerificationCheck::Replay { expect_outcome } => replay_outcome == *expect_outcome,
            VerificationCheck::TestCommand { cmd, expect_exit } => {
                run_host_command(cmd).await == *expect_exit
            }
            VerificationCheck::SensorAssertion { tool, contains } => {
                let (outcome, output) =
                    executor.replay_capture(tool, &serde_json::json!({})).await;
                outcome == Outcome::Success && output.contains(contains)
            }
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

        // Candidates: (manifest, exemplar episode). The exemplar provides the
        // *proven concrete* steps used for replay verification — never the
        // `{param}` templates. Parameterized group skills come first, so the
        // generalized recipe wins a name collision with a one-off recipe.
        let mut candidates: Vec<(SkillManifest, &Episode)> = Vec::new();

        let mut groups: BTreeMap<String, Vec<&Episode>> = BTreeMap::new();
        for ep in &episodes {
            let sig = chain_signature(ep);
            if !sig.is_empty() {
                groups.entry(sig).or_default().push(ep);
            }
        }
        for group in groups.values().filter(|g| g.len() >= 2) {
            if let Some(manifest) = parameterize(group) {
                let exemplar = group.iter().max_by_key(|e| e.ts_ms).unwrap();
                candidates.push((manifest, exemplar));
            }
        }
        for ep in &episodes {
            if let Some(manifest) = synthesize(ep) {
                candidates.push((manifest, ep));
            }
        }

        for (candidate, exemplar) in candidates {
            report.candidates += 1;

            if existing.contains(&candidate.name) || !seen.insert(candidate.name.clone()) {
                report.skipped_existing += 1;
                continue;
            }

            // Quarantine (never auto-verify/enable) if the recipe is unsafe to
            // re-run: any actuator among the exemplar's steps (Track 0 name
            // list) OR any step tool's declared RiskClass is not safe to
            // replay (irreversible / blast radius / physical). Such skills are
            // installed disabled for staged operator promotion.
            let replay_steps: Vec<(&str, &Value)> = exemplar
                .steps
                .iter()
                .filter(|s| s.ok)
                .map(|s| (s.tool.as_str(), &s.args))
                .collect();
            let replay_unsafe = replay_steps.is_empty()
                || replay_steps.iter().any(|(t, _)| {
                    physical_refs.contains(t) || !safe_to_replay(executor.risk_of(t))
                });
            if touches_actuator(exemplar, &physical_refs) || replay_unsafe {
                let mut supervised = tag_physical(candidate); // stays disabled
                // Read-only sensor checks can still strengthen the evidence:
                // all matching SensorAssertion rules passing earns a tag the
                // operator can trust when promoting (Track 0 staged rollout).
                let sensor_rules: Vec<_> = self
                    .rules
                    .iter()
                    .filter(|r| {
                        pattern_matches(&r.skill_pattern, &supervised.name)
                            && matches!(r.check, VerificationCheck::SensorAssertion { .. })
                    })
                    .collect();
                if !sensor_rules.is_empty() {
                    let mut all_ok = true;
                    for rule in &sensor_rules {
                        if !self.check_passes(executor, &rule.check, Outcome::Success).await {
                            all_ok = false;
                            break;
                        }
                    }
                    if all_ok {
                        supervised.tags.push("track0:sensor-verified".to_string());
                    }
                }
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

            // Verify: replay every proven step in order, then run all matching
            // configured checks. Everything must pass before the skill is
            // approved + installed enabled.
            let name = candidate.name.clone();
            let mut replay_outcome = Outcome::Success;
            for (tool, args) in &replay_steps {
                if executor.replay(tool, args).await != Outcome::Success {
                    replay_outcome = Outcome::Failure;
                    break;
                }
            }
            let mut verified = replay_outcome == Outcome::Success;
            if verified {
                for rule in self
                    .rules
                    .iter()
                    .filter(|r| pattern_matches(&r.skill_pattern, &name))
                {
                    if !self.check_passes(executor, &rule.check, replay_outcome).await {
                        tracing::info!(
                            skill = %name,
                            "learned skill failed a configured verification check"
                        );
                        verified = false;
                        break;
                    }
                }
            }
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

        // Metrics + hot reload: a changed skill set is pushed into the live
        // agent's tool registry (enabled skills only; quarantined ones stay
        // invisible until an operator promotes them).
        if let Some(obs) = &self.obs {
            let m = &obs.metrics;
            m.counter("self_improve_scanned_total").add(report.episodes_scanned as u64);
            m.counter("self_improve_candidates_total").add(report.candidates as u64);
            m.counter("self_improve_installed_total").add(report.installed.len() as u64);
            m.counter("self_improve_quarantined_total").add(report.pending_supervised.len() as u64);
            m.counter("self_improve_rejected_total").add(report.rejected.len() as u64);
        }
        if !report.installed.is_empty() || !report.pending_supervised.is_empty() {
            executor.on_skills_changed(&self.forge);
        }

        // Phase 16 metric: token/latency delta for runs that used a learned
        // skill vs. the rest (visible per pass in the logs).
        if let Ok(stats) = self.trajectory.efficiency_stats() {
            if stats.with_learned.runs > 0 && stats.without_learned.runs > 0 {
                tracing::info!(
                    learned_runs = stats.with_learned.runs,
                    learned_avg_ms = stats.with_learned.avg_ms(),
                    learned_avg_tokens = stats.with_learned.avg_tokens_est(),
                    other_runs = stats.without_learned.runs,
                    other_avg_ms = stats.without_learned.avg_ms(),
                    other_avg_tokens = stats.without_learned.avg_tokens_est(),
                    "Phase 16 efficiency: learned-skill runs vs rest"
                );
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
            // Pick up out-of-band operator changes (CLI promote/demote,
            // manual manifest edits) even when this pass installs nothing.
            executor.on_skills_changed(&self.forge);
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
            duration_ms: None,
            tokens_est: None,
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
        // Even with a "success" executor, physical skills must NOT be replayed
        // or promoted past the simulate stage.
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
        assert!(m.enabled, "loads so the model can invoke it — but only as a dry-run");
        assert_eq!(
            m.stage,
            crate::tools::traits::RolloutStage::Simulate,
            "physical learned skill starts at the simulate stage"
        );
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

    fn multistep_episode(id: &str, objective: &str, steps: Vec<(&str, Value)>) -> Episode {
        Episode {
            id: id.to_string(),
            session_id: "s".to_string(),
            objective: objective.to_string(),
            steps: steps
                .into_iter()
                .map(|(tool, args)| EpisodeStep {
                    tool: tool.to_string(),
                    args,
                    result: "ok".to_string(),
                    ok: true,
                })
                .collect(),
            outcome: Outcome::Success,
            ts_ms: 10,
            duration_ms: None,
            tokens_est: None,
        }
    }

    #[tokio::test]
    async fn multistep_episode_installs_sequence_skill() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&multistep_episode(
            "e1",
            "morning report",
            vec![("http", json!({"q": "weather"})), ("http", json!({"q": "news"}))],
        ))
        .unwrap();
        let dir = tmp_dir("sequence");
        let improver = SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);
        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert_eq!(report.installed, vec!["learned_morning_report".to_string()]);
        let manifests = SkillForge::new(&dir).list_manifests().unwrap();
        let m = manifests.iter().find(|m| m.name == "learned_morning_report").unwrap();
        assert!(m.enabled);
        assert!(matches!(&m.kind, crate::skill_forge::SkillKind::Sequence { steps } if steps.len() == 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn parameterized_group_skill_wins_and_installs() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&Episode {
            ts_ms: 1,
            ..multistep_episode("a", "check the weather in Oslo", vec![("http", json!({"q": "weather", "city": "Oslo"}))])
        })
        .unwrap();
        traj.record(&Episode {
            ts_ms: 2,
            ..multistep_episode("b", "check the weather", vec![("http", json!({"q": "weather", "city": "Bergen"}))])
        })
        .unwrap();
        let dir = tmp_dir("param");
        let improver = SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);
        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        // The generalized skill is synthesized ahead of the per-episode one and
        // wins the name collision with "check the weather"'s one-off recipe.
        assert!(report.installed.contains(&"learned_check_the_weather".to_string()));
        let manifests = SkillForge::new(&dir).list_manifests().unwrap();
        let m = manifests.iter().find(|m| m.name == "learned_check_the_weather").unwrap();
        assert!(m.tags.contains(&"parameterized".to_string()));
        assert!(m.parameters["properties"]["city"].is_object());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failing_test_command_rule_rejects_candidate() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "fetch data", "http")).unwrap();
        let dir = tmp_dir("rule-cmd");
        let improver = SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100)
            .with_verification_rules(vec![VerificationRule {
                skill_pattern: "learned_*".to_string(),
                check: super::VerificationCheck::TestCommand {
                    cmd: "exit 1".to_string(),
                    expect_exit: 0,
                },
            }]);
        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert!(report.installed.is_empty(), "replay passed but the check must fail it");
        assert_eq!(report.rejected, vec!["learned_fetch_data".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn passing_test_command_rule_allows_install() {
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "fetch data", "http")).unwrap();
        let dir = tmp_dir("rule-ok");
        let improver = SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100)
            .with_verification_rules(vec![VerificationRule {
                skill_pattern: "learned_*".to_string(),
                check: super::VerificationCheck::TestCommand {
                    cmd: "exit 0".to_string(),
                    expect_exit: 0,
                },
            }]);
        let report = improver.run_once(&MockExec(Outcome::Success), 0).await.unwrap();
        assert_eq!(report.installed, vec!["learned_fetch_data".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sensor_rule_tags_physical_skill_but_keeps_it_disabled() {
        struct SensorExec;
        #[async_trait::async_trait]
        impl ReplayExecutor for SensorExec {
            async fn replay(&self, _tool: &str, _args: &Value) -> Outcome {
                Outcome::Success
            }
            async fn replay_capture(&self, tool: &str, _args: &Value) -> (Outcome, String) {
                assert_eq!(tool, "sensor_read", "only the read-only sensor is consulted");
                (Outcome::Success, "door=open temp=21".to_string())
            }
        }

        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "unlock the door", "gpio_write")).unwrap();
        let dir = tmp_dir("sensor-tag");
        let improver = SkillImprover::new(
            Arc::clone(&traj),
            SkillForge::new(&dir),
            vec!["gpio_write".to_string()],
            100,
        )
        .with_verification_rules(vec![VerificationRule {
            skill_pattern: "learned_unlock_*".to_string(),
            check: super::VerificationCheck::SensorAssertion {
                tool: "sensor_read".to_string(),
                contains: "door=open".to_string(),
            },
        }]);
        let report = improver.run_once(&SensorExec, 0).await.unwrap();
        assert_eq!(report.pending_supervised, vec!["learned_unlock_the_door".to_string()]);
        let manifests = SkillForge::new(&dir).list_manifests().unwrap();
        let m = manifests.iter().find(|m| m.name == "learned_unlock_the_door").unwrap();
        assert_eq!(
            m.stage,
            crate::tools::traits::RolloutStage::Simulate,
            "sensor evidence never advances a physical skill past simulate"
        );
        assert!(m.tags.contains(&"track0:sensor-verified".to_string()));
        assert!(m.tags.contains(&"track0:supervised".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn notifies_executor_for_hot_reload_after_install() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct NotifyExec(AtomicUsize);
        #[async_trait::async_trait]
        impl ReplayExecutor for NotifyExec {
            async fn replay(&self, _tool: &str, _args: &Value) -> Outcome {
                Outcome::Success
            }
            fn on_skills_changed(&self, _forge: &SkillForge) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&episode("e1", "fetch news", "http")).unwrap();
        let dir = tmp_dir("notify");
        let improver =
            SkillImprover::new(Arc::clone(&traj), SkillForge::new(&dir), vec![], 100);

        let exec = NotifyExec(AtomicUsize::new(0));
        improver.run_once(&exec, 0).await.unwrap();
        assert_eq!(exec.0.load(Ordering::SeqCst), 1, "hot-reload hook fires after install");

        // A pass with no changes must not re-notify.
        improver.run_once(&exec, 0).await.unwrap();
        assert_eq!(exec.0.load(Ordering::SeqCst), 1);
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

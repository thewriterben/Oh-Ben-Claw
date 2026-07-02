//! Offline trace evolution (Phase 16 P4) — GEPA/DSPy-inspired.
//!
//! A scheduled, **config-gated** batch job that uses the LLM to rewrite
//! learned-skill *descriptions* from accumulated execution traces, improving
//! future tool selection. Strict invariants:
//!
//! - Only the `description` field is ever mutated — never `enabled`, `stage`,
//!   `kind`, or `parameters`. Evolution can make a skill easier to *pick*,
//!   never easier to *run*.
//! - Every change is appended to a JSONL diff log
//!   (`~/.oh-ben-claw/skill_evolution.jsonl`) and is revertible
//!   (`oh-ben-claw skill revert-description <name>`).
//! - LLM output is treated as a proposal: sanitized, length-bounded, and
//!   dropped when empty/unchanged.

use super::{SkillForge, SkillManifest};
use crate::config::ProviderConfig;
use crate::memory::trajectory::TrajectoryStore;
use crate::providers::{ChatMessage, ChatRole, Provider};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// One line of the evolution diff log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionEntry {
    pub ts_ms: u64,
    pub skill: String,
    pub old: String,
    pub new: String,
    /// `"evolve"` or `"revert"`.
    pub kind: String,
}

/// Summary of one evolution pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvolveReport {
    /// Learned skills considered.
    pub considered: usize,
    /// Skills whose description was rewritten.
    pub rewritten: Vec<String>,
    /// Skills skipped (no usage traces, or the proposal was rejected).
    pub skipped: usize,
}

/// The scheduled description evolver.
pub struct DescriptionEvolver {
    forge: SkillForge,
    trajectory: Arc<TrajectoryStore>,
    provider: Arc<dyn Provider>,
    provider_config: ProviderConfig,
    log_path: PathBuf,
    max_per_pass: usize,
}

/// The default evolution log path (`~/.oh-ben-claw/skill_evolution.jsonl`).
pub fn default_log_path() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().join(".oh-ben-claw"))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("skill_evolution.jsonl")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn append_log(path: &PathBuf, entry: &EvolutionEntry) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(line) = serde_json::to_string(entry) {
        use std::io::Write;
        match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut f) => {
                let _ = writeln!(f, "{line}");
            }
            Err(e) => tracing::warn!(error = %e, "failed to append skill evolution log"),
        }
    }
}

/// Sanitize an LLM description proposal. `None` = rejected.
fn sanitize_proposal(raw: &str, old: &str) -> Option<String> {
    let mut s = raw.trim();
    // Strip a fenced block if the model wrapped its answer.
    if s.starts_with("```") {
        s = s.trim_start_matches("```").trim_start_matches("text");
        s = s.trim_end_matches("```");
        s = s.trim();
    }
    let s = s.trim_matches('"').trim();
    // Single paragraph only.
    let s = s.lines().next().unwrap_or("").trim();
    if s.is_empty() || s.len() > 300 || s == old {
        return None;
    }
    Some(s.to_string())
}

impl DescriptionEvolver {
    /// Create an evolver.
    pub fn new(
        forge: SkillForge,
        trajectory: Arc<TrajectoryStore>,
        provider: Arc<dyn Provider>,
        provider_config: ProviderConfig,
        log_path: PathBuf,
        max_per_pass: usize,
    ) -> Self {
        Self {
            forge,
            trajectory,
            provider,
            provider_config,
            log_path,
            max_per_pass: max_per_pass.max(1),
        }
    }

    /// One evolution pass over learned skills with usage traces.
    pub async fn run_once(&self) -> anyhow::Result<EvolveReport> {
        let mut report = EvolveReport::default();
        let manifests: Vec<SkillManifest> = self
            .forge
            .list_manifests()?
            .into_iter()
            .filter(|m| m.name.starts_with("learned_"))
            .collect();
        let recent = self.trajectory.recent(500)?;

        for mut manifest in manifests.into_iter() {
            if report.rewritten.len() >= self.max_per_pass {
                break;
            }
            report.considered += 1;

            // Usage traces: objectives of runs that invoked this skill.
            let usages: Vec<String> = recent
                .iter()
                .filter(|ep| ep.steps.iter().any(|s| s.tool == manifest.name))
                .take(5)
                .map(|ep| {
                    format!(
                        "- objective: {:?}; outcome: {:?}",
                        ep.objective.trim(),
                        ep.outcome
                    )
                })
                .collect();
            if usages.is_empty() {
                report.skipped += 1;
                continue; // nothing observed — no evidence to evolve from
            }

            let prompt = format!(
                "You improve tool descriptions for an AI agent. A good description makes the \
                 agent select the tool exactly when appropriate — concrete, specific, one \
                 sentence, under 240 characters, no marketing language.\n\n\
                 Tool name: {}\nCurrent description: {}\nParameter schema: {}\n\n\
                 Observed real uses:\n{}\n\n\
                 Reply with ONLY the improved description text (no quotes, no preamble). If \
                 the current description is already ideal, reply with it unchanged.",
                manifest.name,
                manifest.description,
                manifest.parameters,
                usages.join("\n")
            );
            let messages = vec![ChatMessage {
                role: ChatRole::User,
                content: prompt,
            }];
            let completion = match self
                .provider
                .chat_completion(&messages, &[], &self.provider_config)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(skill = %manifest.name, error = %e, "evolution call failed");
                    report.skipped += 1;
                    continue;
                }
            };

            match sanitize_proposal(&completion.message, &manifest.description) {
                Some(new_desc) => {
                    let entry = EvolutionEntry {
                        ts_ms: now_ms(),
                        skill: manifest.name.clone(),
                        old: manifest.description.clone(),
                        new: new_desc.clone(),
                        kind: "evolve".to_string(),
                    };
                    // Only the description changes — stage/enabled/kind are
                    // whatever the manifest already had.
                    manifest.description = new_desc;
                    if self.forge.install_skill(&manifest).is_ok() {
                        append_log(&self.log_path, &entry);
                        tracing::info!(skill = %manifest.name, "skill description evolved");
                        report.rewritten.push(manifest.name);
                    } else {
                        report.skipped += 1;
                    }
                }
                None => report.skipped += 1,
            }
        }
        Ok(report)
    }

    /// Run forever on a fixed cadence (spawn on a background task).
    pub async fn run_periodically(self, interval: std::time::Duration) {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick: evolution wants accumulated traces.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match self.run_once().await {
                Ok(rep) if !rep.rewritten.is_empty() => {
                    tracing::info!(
                        rewritten = rep.rewritten.len(),
                        considered = rep.considered,
                        "Phase 16 offline evolution pass"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "Phase 16 offline evolution pass failed"),
            }
        }
    }
}

/// Revert `name`'s description to the previous value recorded in the log.
/// Appends a `revert` entry so the history stays append-only.
pub fn revert_description(
    forge: &SkillForge,
    log_path: &PathBuf,
    name: &str,
) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(log_path)
        .map_err(|_| anyhow::anyhow!("no evolution log at {}", log_path.display()))?;
    let last: EvolutionEntry = content
        .lines()
        .filter_map(|l| serde_json::from_str::<EvolutionEntry>(l).ok())
        .filter(|e| e.skill == name && e.kind == "evolve")
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("no evolution entry for '{name}'"))?;

    let mut manifest = forge
        .list_manifests()?
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| anyhow::anyhow!("no skill named '{name}'"))?;
    let entry = EvolutionEntry {
        ts_ms: now_ms(),
        skill: name.to_string(),
        old: manifest.description.clone(),
        new: last.old.clone(),
        kind: "revert".to_string(),
    };
    manifest.description = last.old.clone();
    forge.install_skill(&manifest)?;
    append_log(log_path, &entry);
    Ok(last.old)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::trajectory::{Episode, EpisodeStep, Outcome};
    use crate::providers::ChatCompletion;
    use crate::skill_forge::SkillKind;
    use crate::tools::traits::{RolloutStage, Tool};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct FixedProvider(Mutex<Vec<String>>);
    #[async_trait]
    impl Provider for FixedProvider {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn chat_completion(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Box<dyn Tool>],
            _config: &ProviderConfig,
        ) -> anyhow::Result<ChatCompletion> {
            let msg = self
                .0
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| "improved".to_string());
            Ok(ChatCompletion {
                message: msg,
                tool_calls: vec![],
                provider: "fixed".to_string(),
                model: "test".to_string(),
            })
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("obc-evolve-{tag}-{nanos}"))
    }

    fn learned_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: "Learned from a successful run: check the weather".to_string(),
            kind: SkillKind::Delegate {
                tool: "http".to_string(),
                fixed_args: json!({"q": "weather"}),
            },
            parameters: json!({ "type": "object", "properties": {} }),
            version: Some("0.1.0-learned".to_string()),
            stage: RolloutStage::Simulate,
            tags: vec!["learned".to_string()],
            enabled: true,
            timeout_secs: 30,
        }
    }

    fn usage_episode(skill: &str) -> Episode {
        Episode {
            id: "u1".to_string(),
            session_id: "s".to_string(),
            objective: "check the weather".to_string(),
            steps: vec![EpisodeStep {
                tool: skill.to_string(),
                args: json!({}),
                result: "ok".to_string(),
                ok: true,
            }],
            outcome: Outcome::Success,
            ts_ms: 1,
            duration_ms: Some(10),
            tokens_est: Some(5),
        }
    }

    fn evolver(dir: &PathBuf, traj: Arc<TrajectoryStore>, replies: Vec<&str>) -> DescriptionEvolver {
        DescriptionEvolver::new(
            SkillForge::new(dir),
            traj,
            Arc::new(FixedProvider(Mutex::new(
                replies.into_iter().rev().map(String::from).collect(),
            ))),
            ProviderConfig::default(),
            dir.join("skill_evolution.jsonl"),
            5,
        )
    }

    #[tokio::test]
    async fn evolves_description_only_and_logs_diff() {
        let dir = tmp_dir("evolve");
        let forge = SkillForge::new(&dir);
        forge.install_skill(&learned_manifest("learned_check")).unwrap();
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&usage_episode("learned_check")).unwrap();

        let ev = evolver(&dir, traj, vec!["Fetch the current weather for the configured location."]);
        let report = ev.run_once().await.unwrap();
        assert_eq!(report.rewritten, vec!["learned_check".to_string()]);

        let m = forge
            .list_manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "learned_check")
            .unwrap();
        assert_eq!(m.description, "Fetch the current weather for the configured location.");
        // Safety invariants: only the description changed.
        assert_eq!(m.stage, RolloutStage::Simulate);
        assert!(m.enabled);
        assert!(matches!(m.kind, SkillKind::Delegate { .. }));
        // Diff logged.
        let log = std::fs::read_to_string(dir.join("skill_evolution.jsonl")).unwrap();
        assert!(log.contains("\"kind\":\"evolve\""));
        assert!(log.contains("Learned from a successful run"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_bad_proposals_and_skips_unused_skills() {
        let dir = tmp_dir("reject");
        let forge = SkillForge::new(&dir);
        forge.install_skill(&learned_manifest("learned_used")).unwrap();
        forge.install_skill(&learned_manifest("learned_unused")).unwrap();
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&usage_episode("learned_used")).unwrap();

        // Identical proposal → rejected; unused skill → never asked.
        let ev = evolver(&dir, traj, vec!["Learned from a successful run: check the weather"]);
        let report = ev.run_once().await.unwrap();
        assert!(report.rewritten.is_empty());
        assert_eq!(report.skipped, 2);
        assert!(!dir.join("skill_evolution.jsonl").exists(), "no change → no log entry");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_restores_previous_description() {
        let dir = tmp_dir("revert");
        let forge = SkillForge::new(&dir);
        forge.install_skill(&learned_manifest("learned_check")).unwrap();
        let traj = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        traj.record(&usage_episode("learned_check")).unwrap();

        let ev = evolver(&dir, traj, vec!["A better description."]);
        ev.run_once().await.unwrap();

        let log = dir.join("skill_evolution.jsonl");
        let restored = revert_description(&forge, &log, "learned_check").unwrap();
        assert_eq!(restored, "Learned from a successful run: check the weather");
        let m = forge
            .list_manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "learned_check")
            .unwrap();
        assert_eq!(m.description, restored);
        let content = std::fs::read_to_string(&log).unwrap();
        assert!(content.contains("\"kind\":\"revert\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_proposal_rules() {
        let old = "old description";
        assert_eq!(
            sanitize_proposal("```text\nNew description.\n```", old).as_deref(),
            Some("New description.")
        );
        assert_eq!(sanitize_proposal("\"Quoted.\"", old).as_deref(), Some("Quoted."));
        assert!(sanitize_proposal("", old).is_none());
        assert!(sanitize_proposal(old, old).is_none(), "unchanged rejected");
        assert!(sanitize_proposal(&"x".repeat(400), old).is_none(), "too long rejected");
    }
}

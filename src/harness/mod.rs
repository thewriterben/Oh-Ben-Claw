//! Long-horizon embodied autonomy harness (Phase 17).
//!
//! Durable, resumable, self-verifying operation across hours/days and across
//! crashes, reboots, and context limits — the initializer+worker pattern with
//! the progress file externalized as structured JSON, and completion decided
//! by **physical evidence** (sensor/tool/world-memory checks), never by the
//! model's own say-so. Design: `docs/PHASE17-PLAN.md`.
//!
//! Safety invariants:
//! - An objective is checkpointed `InFlight` *before* the agent acts (the
//!   "non-persistable region" boundary). On resume, `InFlight` objectives are
//!   **never blindly re-run**: their verification checks decide whether the
//!   side effect already happened.
//! - An `InFlight` objective with no verification checks **fails closed** on
//!   resume — a crash inside an unverifiable action means manual review, not
//!   a possible double actuation.
//! - Every tool touch (verification reads included) goes through the agent's
//!   execution chokepoint: policy → Track 0 → trust → approval.

use crate::agent::Agent;
use crate::config::ProviderConfig;
use crate::memory::world::WorldMemory;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Verification checks ───────────────────────────────────────────────────────

/// Evidence an objective must produce before it may be `Done`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessCheck {
    /// Call a read tool (sensor, camera, …) through the agent chokepoint and
    /// require its output to contain a substring.
    ToolContains {
        tool: String,
        #[serde(default)]
        args: Value,
        contains: String,
    },
    /// Run a host command and require an exit code.
    Command {
        cmd: String,
        #[serde(default)]
        expect_exit: i32,
    },
    /// Require the current world-memory fact for `entity` to contain a
    /// substring (JSON-serialized value match).
    WorldFact { entity: String, contains: String },
}

// ── Progress record ───────────────────────────────────────────────────────────

/// Objective lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveStatus {
    /// Not yet attempted (or reopened after failed verification).
    Pending,
    /// The worker checkpointed it and is (or was, at crash time) acting on it.
    InFlight,
    /// Found `InFlight` on resume — verification must decide its fate.
    NeedsVerification,
    /// Verified complete.
    Done,
    /// Gave up (attempts exhausted, or unverifiable crash).
    Failed,
}

/// One objective in the externalized progress record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Objective {
    pub id: String,
    pub description: String,
    /// Evidence required before `Done`. Empty = completion on agent-run
    /// success (marked unverified; fails closed if a crash interrupts it).
    #[serde(default)]
    pub verify: Vec<HarnessCheck>,
    #[serde(default = "default_status")]
    pub status: ObjectiveStatus,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Human-readable trail (last transition reason).
    #[serde(default)]
    pub note: String,
}

fn default_status() -> ObjectiveStatus {
    ObjectiveStatus::Pending
}
fn default_max_attempts() -> u32 {
    3
}

/// The externalized world-state progress record — the durable "context" that
/// survives crashes and context compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressRecord {
    pub mission: String,
    pub objectives: Vec<Objective>,
    /// Environment snapshot taken by the initializer (world entities, …).
    #[serde(default)]
    pub environment: Value,
    /// How many times this mission has been (re)started.
    #[serde(default)]
    pub run_count: u32,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
}

impl ProgressRecord {
    /// Whether every objective has settled (`Done` or `Failed`).
    pub fn settled(&self) -> bool {
        self.objectives
            .iter()
            .all(|o| matches!(o.status, ObjectiveStatus::Done | ObjectiveStatus::Failed))
    }

    /// `(done, failed, outstanding)` counts.
    pub fn tally(&self) -> (usize, usize, usize) {
        let done = self
            .objectives
            .iter()
            .filter(|o| o.status == ObjectiveStatus::Done)
            .count();
        let failed = self
            .objectives
            .iter()
            .filter(|o| o.status == ObjectiveStatus::Failed)
            .count();
        (done, failed, self.objectives.len() - done - failed)
    }
}

/// File-backed store for progress records. Every write is atomic
/// (tmp + rename), so a crash never leaves a half-written checkpoint.
pub struct ProgressStore {
    dir: PathBuf,
}

impl ProgressStore {
    /// A store rooted at `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The default record directory (`~/.oh-ben-claw/harness`).
    pub fn default_dir() -> PathBuf {
        directories::UserDirs::new()
            .map(|d| d.home_dir().join(".oh-ben-claw"))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("harness")
    }

    fn path(&self, mission: &str) -> PathBuf {
        let safe: String = mission
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    /// Load a mission's record, if one exists.
    pub fn load(&self, mission: &str) -> Option<ProgressRecord> {
        let content = std::fs::read_to_string(self.path(mission)).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Atomically checkpoint a record (tmp + rename).
    pub fn save(&self, record: &ProgressRecord) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(&record.mission);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(record)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Delete a mission's record (operator reset).
    pub fn reset(&self, mission: &str) -> Result<()> {
        std::fs::remove_file(self.path(mission))?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Harness ───────────────────────────────────────────────────────────────────

/// Outcome of one worker pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassOutcome {
    /// An objective changed state (id included).
    Advanced(String),
    /// Every objective is settled.
    Settled,
}

/// The initializer+worker harness for one mission.
pub struct Harness {
    store: ProgressStore,
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    /// Conversation session used for worker prompts (must exist in memory).
    session_id: String,
    world: Option<Arc<WorldMemory>>,
}

impl Harness {
    pub fn new(
        store: ProgressStore,
        agent: Arc<Agent>,
        provider_config: ProviderConfig,
        session_id: String,
    ) -> Self {
        Self {
            store,
            agent,
            provider_config,
            session_id,
            world: None,
        }
    }

    /// Attach world memory (enables `WorldFact` checks + richer resume context).
    pub fn with_world(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// **Initializer**: load the existing record (resume) or create one from
    /// the given objectives. On resume, every `InFlight` objective becomes
    /// `NeedsVerification` — evidence, not assumption, decides its fate. The
    /// environment snapshot is refreshed either way.
    pub fn initialize(&self, mission: &str, objectives: Vec<Objective>) -> Result<ProgressRecord> {
        let mut record = match self.store.load(mission) {
            Some(mut existing) => {
                existing.run_count += 1;
                for o in existing.objectives.iter_mut() {
                    if o.status == ObjectiveStatus::InFlight {
                        if o.verify.is_empty() {
                            // Fail closed: crashed mid-flight, no way to verify.
                            o.status = ObjectiveStatus::Failed;
                            o.note = "crashed mid-flight with no verification checks — \
                                      manual review required (fail closed, no re-run)"
                                .to_string();
                        } else {
                            o.status = ObjectiveStatus::NeedsVerification;
                            o.note = "found in-flight on resume — verifying before any re-run"
                                .to_string();
                        }
                    }
                }
                tracing::info!(
                    mission = %mission,
                    run = existing.run_count,
                    "harness resuming from persisted record"
                );
                existing
            }
            None => ProgressRecord {
                mission: mission.to_string(),
                objectives,
                environment: Value::Null,
                run_count: 1,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        };
        record.environment = self.environment_snapshot();
        record.updated_ms = now_ms();
        self.store.save(&record)?;
        Ok(record)
    }

    /// Cheap environment snapshot for the record + resume context.
    fn environment_snapshot(&self) -> Value {
        let mut env = serde_json::Map::new();
        env.insert("snapshot_ms".to_string(), serde_json::json!(now_ms()));
        if let Some(world) = &self.world {
            if let Ok(entities) = world.entities() {
                let facts: serde_json::Map<String, Value> = entities
                    .iter()
                    .take(50)
                    .filter_map(|e| {
                        world
                            .current(e)
                            .ok()
                            .flatten()
                            .map(|f| (e.clone(), f.value))
                    })
                    .collect();
                env.insert("world".to_string(), Value::Object(facts));
            }
        }
        Value::Object(env)
    }

    /// The "resume smoke test": a compact context block re-establishing where
    /// the mission stands before the worker acts.
    fn resume_context(&self, record: &ProgressRecord) -> String {
        let (done, failed, outstanding) = record.tally();
        let mut ctx = format!(
            "[Mission '{}' — run {} | {} done, {} failed, {} outstanding]\n",
            record.mission, record.run_count, done, failed, outstanding
        );
        for o in &record.objectives {
            ctx.push_str(&format!("- [{:?}] {}: {}\n", o.status, o.id, o.description));
        }
        if let Some(world_facts) = record.environment.get("world") {
            ctx.push_str(&format!("Current device/world state: {world_facts}\n"));
        }
        ctx
    }

    /// Run one verification check through the safety chokepoint.
    async fn check_passes(&self, check: &HarnessCheck) -> bool {
        match check {
            HarnessCheck::ToolContains { tool, args, contains } => {
                match self.agent.execute_tool_direct(tool, args.clone()).await {
                    Ok(r) if r.success => r.output.contains(contains),
                    _ => false,
                }
            }
            HarnessCheck::Command { cmd, expect_exit } => {
                crate::skill_forge::improve::run_host_command(cmd).await == *expect_exit
            }
            HarnessCheck::WorldFact { entity, contains } => match &self.world {
                Some(world) => world
                    .current(entity)
                    .ok()
                    .flatten()
                    .map(|f| f.value.to_string().contains(contains))
                    .unwrap_or(false),
                None => false,
            },
        }
    }

    async fn all_checks_pass(&self, objective: &Objective) -> bool {
        for check in &objective.verify {
            if !self.check_passes(check).await {
                return false;
            }
        }
        true
    }

    /// **Worker**: advance the mission by at most one objective transition.
    /// Verification-pending objectives are settled first (no new side effects
    /// until the crash backlog is resolved); then the next `Pending` objective
    /// is executed inside an `InFlight` checkpoint and verified before `Done`.
    pub async fn run_once(&self, record: &mut ProgressRecord) -> Result<PassOutcome> {
        // 1. Resolve resume-time verification backlog first.
        if let Some(idx) = record
            .objectives
            .iter()
            .position(|o| o.status == ObjectiveStatus::NeedsVerification)
        {
            let passed = self.all_checks_pass(&record.objectives[idx]).await;
            let o = &mut record.objectives[idx];
            if passed {
                o.status = ObjectiveStatus::Done;
                o.note = "verified complete on resume — side effect had already landed; \
                          not re-run"
                    .to_string();
            } else {
                o.attempts += 1;
                if o.attempts >= o.max_attempts {
                    o.status = ObjectiveStatus::Failed;
                    o.note = "verification failed on resume; attempts exhausted".to_string();
                } else {
                    o.status = ObjectiveStatus::Pending;
                    o.note = "verification failed on resume — reopened".to_string();
                }
            }
            let id = record.objectives[idx].id.clone();
            record.updated_ms = now_ms();
            self.store.save(record)?;
            return Ok(PassOutcome::Advanced(id));
        }

        // 2. Next pending objective.
        let Some(idx) = record
            .objectives
            .iter()
            .position(|o| o.status == ObjectiveStatus::Pending)
        else {
            return Ok(PassOutcome::Settled);
        };

        // Non-persistable region boundary: checkpoint InFlight BEFORE acting.
        record.objectives[idx].status = ObjectiveStatus::InFlight;
        record.updated_ms = now_ms();
        self.store.save(record)?;

        let prompt = format!(
            "{}\nYour single task right now is objective '{}': {}\n\
             Complete ONLY this objective, then summarize what you did.",
            self.resume_context(record),
            record.objectives[idx].id,
            record.objectives[idx].description
        );
        let run = self
            .agent
            .process(&self.session_id, &prompt, &self.provider_config)
            .await;

        let run_ok = match &run {
            Ok(resp) => !resp.message.is_empty(),
            Err(e) => {
                tracing::warn!(objective = %record.objectives[idx].id, error = %e, "worker run failed");
                false
            }
        };

        // Mandatory verification before Done.
        let verified = run_ok && self.all_checks_pass(&record.objectives[idx]).await;
        let o = &mut record.objectives[idx];
        if verified {
            o.status = ObjectiveStatus::Done;
            o.note = if o.verify.is_empty() {
                "completed (UNVERIFIED — no checks configured)".to_string()
            } else {
                format!("completed; {} verification check(s) passed", o.verify.len())
            };
        } else {
            o.attempts += 1;
            if o.attempts >= o.max_attempts {
                o.status = ObjectiveStatus::Failed;
                o.note = "attempts exhausted".to_string();
            } else {
                o.status = ObjectiveStatus::Pending;
                o.note = if run_ok {
                    "verification failed after run — reopened".to_string()
                } else {
                    "agent run failed — reopened".to_string()
                };
            }
        }
        let id = record.objectives[idx].id.clone();
        record.updated_ms = now_ms();
        self.store.save(record)?;
        Ok(PassOutcome::Advanced(id))
    }

    /// Drive the mission until every objective settles (or `max_passes` is
    /// hit — a hard budget so a wedged mission cannot spin forever).
    pub async fn run_mission(
        &self,
        record: &mut ProgressRecord,
        max_passes: usize,
        pass_delay: std::time::Duration,
    ) -> Result<()> {
        for _ in 0..max_passes {
            match self.run_once(record).await? {
                PassOutcome::Settled => break,
                PassOutcome::Advanced(id) => {
                    let (done, failed, outstanding) = record.tally();
                    tracing::info!(
                        mission = %record.mission,
                        objective = %id,
                        done,
                        failed,
                        outstanding,
                        "harness pass complete"
                    );
                    if record.settled() {
                        break;
                    }
                    tokio::time::sleep(pass_delay).await;
                }
            }
        }
        let (done, failed, outstanding) = record.tally();
        tracing::info!(
            mission = %record.mission,
            done, failed, outstanding,
            "harness mission finished (or budget reached)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> ProgressStore {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        ProgressStore::new(std::env::temp_dir().join(format!("obc-harness-{tag}-{nanos}")))
    }

    fn objective(id: &str, verify: Vec<HarnessCheck>) -> Objective {
        Objective {
            id: id.to_string(),
            description: format!("do {id}"),
            verify,
            status: ObjectiveStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            note: String::new(),
        }
    }

    fn record(mission: &str, objectives: Vec<Objective>) -> ProgressRecord {
        ProgressRecord {
            mission: mission.to_string(),
            objectives,
            environment: Value::Null,
            run_count: 1,
            created_ms: 0,
            updated_ms: 0,
        }
    }

    #[test]
    fn store_roundtrip_is_atomic_and_reloadable() {
        let store = tmp_store("roundtrip");
        let rec = record("m1", vec![objective("o1", vec![])]);
        store.save(&rec).unwrap();
        let loaded = store.load("m1").unwrap();
        assert_eq!(loaded.mission, "m1");
        assert_eq!(loaded.objectives.len(), 1);
        // No stray tmp file left behind.
        assert!(!store.path("m1").with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&store.dir).ok();
    }

    #[test]
    fn store_sanitizes_mission_names() {
        let store = tmp_store("sanitize");
        let rec = record("../../etc/passwd", vec![]);
        store.save(&rec).unwrap();
        // The record landed inside the store dir, not outside it.
        let entries: Vec<_> = std::fs::read_dir(&store.dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        std::fs::remove_dir_all(&store.dir).ok();
    }

    #[test]
    fn tally_and_settled() {
        let mut rec = record(
            "m",
            vec![objective("a", vec![]), objective("b", vec![]), objective("c", vec![])],
        );
        rec.objectives[0].status = ObjectiveStatus::Done;
        rec.objectives[1].status = ObjectiveStatus::Failed;
        assert_eq!(rec.tally(), (1, 1, 1));
        assert!(!rec.settled());
        rec.objectives[2].status = ObjectiveStatus::Done;
        assert!(rec.settled());
    }
}

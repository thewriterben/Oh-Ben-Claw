//! The agent as a replay executor.
//!
//! `skill_forge` improves a skill by *replaying* its steps and watching what
//! happens. It declares [`ReplayExecutor`](obc_skill_forge::improve::ReplayExecutor)
//! for that — the abstraction is its own, and it must be, because the forge is
//! what calls it.
//!
//! The implementation belongs here. It lived in `skill_forge/improve.rs` until
//! 2026-08-13, next to the trait rather than next to the type, and it was that
//! module's *only* reference to the agent. `agent -> skill_forge` is nine
//! references the other way and every one of them is real, so this single
//! `impl` block was the entire `agent <-> skill_forge` cycle — the last of the
//! four two-module cycles that had a cheap answer.
//!
//! Trait where the abstraction is, implementation where the dependency is.
//! Fifth time this month: `SpineActuatorSink`, `SpineActionSink`,
//! `AgentExecutor` for `obc-a2a`, `Severity` out of `notify`, and now this.
//!
//! Nothing else changed. `replay` still goes through `execute_tool_direct`, so
//! a replayed tool takes the same chokepoint a live one does — policy and
//! Track 0 included. That property is why the agent is the executor rather
//! than the forge running tools itself, and moving the impl does not touch it.

use super::Agent;
use obc_memory::trajectory::Outcome;
use obc_skill_forge::improve::ReplayExecutor;
use obc_skill_forge::SkillForge;
use obc_tool_api::RiskClass;
use serde_json::Value;

/// The agent itself is a replay executor (runs the tool through its normal
/// chokepoint, including policy + Track 0).
#[async_trait::async_trait]
impl ReplayExecutor for Agent {
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

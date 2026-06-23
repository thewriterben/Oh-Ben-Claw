//! Skill synthesis from experience (Phase 16).
//!
//! Turns a successful [`Episode`](crate::memory::trajectory::Episode) into a
//! reusable [`SkillManifest`] so the agent can replay a proven recipe instead of
//! reasoning from scratch. Synthesized skills are **quarantined** (`enabled =
//! false`) until they pass a [`VerificationCheck`] — reflection without a real
//! signal is unreliable, so a learned skill is never offered to the LLM until it
//! is independently verified.
//!
//! This first increment synthesizes deterministic *single-action recipes* (a
//! `Delegate` to the proven tool with its arguments). LLM-driven reflective
//! synthesis of multi-step skills and GEPA-style prompt evolution are layered on
//! top later; the verification gate and Track 0 interlock here apply regardless.

use super::{SkillKind, SkillManifest};
use crate::memory::trajectory::{Episode, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A concrete check a synthesized skill must pass before it is trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum VerificationCheck {
    /// Re-run the recipe in a sandbox and require this outcome.
    Replay { expect_outcome: Outcome },
    /// Assert the replay output contains a substring (e.g. a sensor confirms the effect).
    SensorAssertion { tool: String, contains: String },
    /// Run a shell command and require this exit code.
    TestCommand { cmd: String, expect_exit: i32 },
}

/// Convert an objective into a safe snake_case skill-name fragment.
fn slugify(objective: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for ch in objective.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Synthesize a reusable, quarantined skill from a successful episode.
///
/// Returns `None` if the episode did not succeed or has no successful step.
/// The result has `enabled = false` and must be verified (see [`verify`]) and
/// approved (see [`approve`]) before it will run.
pub fn synthesize(ep: &Episode) -> Option<SkillManifest> {
    if ep.outcome != Outcome::Success {
        return None;
    }
    let step = ep.steps.iter().find(|s| s.ok)?;
    let name = format!("learned_{}", slugify(&ep.objective));
    Some(SkillManifest {
        name,
        description: format!("Learned from a successful run: {}", ep.objective.trim()),
        kind: SkillKind::Delegate {
            tool: step.tool.clone(),
            fixed_args: step.args.clone(),
        },
        parameters: json!({ "type": "object", "properties": {} }),
        version: Some("0.1.0-learned".to_string()),
        tags: vec!["learned".to_string()],
        // Quarantined: never trusted until verified + approved.
        enabled: false,
        timeout_secs: 30,
    })
}

/// Whether an episode invoked any physical/actuator tool (Track 0 interlock):
/// such learned skills must go through staged rollout, not run unattended.
pub fn touches_actuator(ep: &Episode, physical_tools: &[&str]) -> bool {
    ep.steps
        .iter()
        .any(|s| physical_tools.contains(&s.tool.as_str()))
}

/// Tag a synthesized skill that touches a physical tool so the approval/rollout
/// layer treats it as supervised.
pub fn tag_physical(mut manifest: SkillManifest) -> SkillManifest {
    if !manifest.tags.iter().any(|t| t == "track0:supervised") {
        manifest.tags.push("track0:supervised".to_string());
    }
    manifest
}

/// Evaluate a verification check against a replay's observed result.
pub fn verify(
    check: &VerificationCheck,
    replay_outcome: Outcome,
    replay_output: &str,
    replay_exit: i32,
) -> bool {
    match check {
        VerificationCheck::Replay { expect_outcome } => replay_outcome == *expect_outcome,
        VerificationCheck::SensorAssertion { contains, .. } => replay_output.contains(contains),
        VerificationCheck::TestCommand { expect_exit, .. } => replay_exit == *expect_exit,
    }
}

/// Approve a verified skill — flips it out of quarantine so it can be offered to
/// the agent. Call only after [`verify`] succeeds.
pub fn approve(mut manifest: SkillManifest) -> SkillManifest {
    manifest.enabled = true;
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::trajectory::EpisodeStep;
    use serde_json::json;

    fn success_episode(objective: &str, tool: &str, ok: bool) -> Episode {
        Episode {
            id: "e1".to_string(),
            session_id: "s".to_string(),
            objective: objective.to_string(),
            steps: vec![EpisodeStep {
                tool: tool.to_string(),
                args: json!({"pin": 18, "value": 1}),
                result: "done".to_string(),
                ok,
            }],
            outcome: Outcome::Success,
            ts_ms: 1,
        }
    }

    #[test]
    fn slugify_makes_safe_names() {
        assert_eq!(slugify("Run the morning routine!"), "run_the_morning_routine");
        assert_eq!(slugify("  turn ON — fan  "), "turn_on_fan");
        assert_eq!(slugify("???"), "skill");
    }

    #[test]
    fn synthesizes_quarantined_delegate_from_success() {
        let ep = success_episode("turn on the fan", "gpio_write", true);
        let m = synthesize(&ep).unwrap();
        assert_eq!(m.name, "learned_turn_on_the_fan");
        assert!(!m.enabled, "synthesized skills start quarantined");
        assert!(m.tags.contains(&"learned".to_string()));
        match m.kind {
            SkillKind::Delegate { tool, fixed_args } => {
                assert_eq!(tool, "gpio_write");
                assert_eq!(fixed_args["pin"], 18);
            }
            _ => panic!("expected a Delegate skill"),
        }
    }

    #[test]
    fn does_not_synthesize_from_failure_or_no_ok_step() {
        let mut ep = success_episode("x", "gpio_write", false); // step not ok
        assert!(synthesize(&ep).is_none());
        ep.steps[0].ok = true;
        ep.outcome = Outcome::Failure; // run failed overall
        assert!(synthesize(&ep).is_none());
    }

    #[test]
    fn track0_interlock_detects_and_tags_physical() {
        let ep = success_episode("unlock door", "gpio_write", true);
        assert!(touches_actuator(&ep, &["gpio_write", "relay"]));
        assert!(!touches_actuator(&ep, &["camera_capture"]));
        let m = tag_physical(synthesize(&ep).unwrap());
        assert!(m.tags.contains(&"track0:supervised".to_string()));
        assert!(!m.enabled);
    }

    #[test]
    fn verify_then_approve_enables() {
        let m = synthesize(&success_episode("y", "shell", true)).unwrap();
        let check = VerificationCheck::Replay {
            expect_outcome: Outcome::Success,
        };
        assert!(verify(&check, Outcome::Success, "", 0));
        assert!(!verify(&check, Outcome::Failure, "", 0));
        let approved = approve(m);
        assert!(approved.enabled);
    }

    #[test]
    fn verify_sensor_and_command() {
        let sensor = VerificationCheck::SensorAssertion {
            tool: "sensor_read".to_string(),
            contains: "21.5".to_string(),
        };
        assert!(verify(&sensor, Outcome::Success, "temp=21.5C", 0));
        assert!(!verify(&sensor, Outcome::Success, "temp=30C", 0));
        let cmd = VerificationCheck::TestCommand {
            cmd: "pytest".to_string(),
            expect_exit: 0,
        };
        assert!(verify(&cmd, Outcome::Success, "", 0));
        assert!(!verify(&cmd, Outcome::Success, "", 1));
    }
}

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

use super::{SkillKind, SkillManifest, SkillStep};
use crate::memory::trajectory::{Episode, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

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
/// A single successful step becomes a `Delegate` recipe; **multiple** all-ok
/// steps become a `Sequence` recipe replaying the proven tool chain in order.
/// Returns `None` if the episode did not succeed or has no successful step.
/// The result has `enabled = false` and must be verified (see [`verify`]) and
/// approved (see [`approve`]) before it will run.
pub fn synthesize(ep: &Episode) -> Option<SkillManifest> {
    if ep.outcome != Outcome::Success {
        return None;
    }
    let ok_steps: Vec<_> = ep.steps.iter().filter(|s| s.ok).collect();
    let kind = match ok_steps.as_slice() {
        [] => return None,
        [step] => SkillKind::Delegate {
            tool: step.tool.clone(),
            fixed_args: step.args.clone(),
        },
        // Multi-step recipe only when the *whole* episode succeeded cleanly —
        // a chain with failed steps interleaved is not a proven recipe.
        steps if steps.len() == ep.steps.len() => SkillKind::Sequence {
            steps: steps
                .iter()
                .map(|s| SkillStep {
                    tool: s.tool.clone(),
                    args: s.args.clone(),
                })
                .collect(),
        },
        // Mixed ok/failed: fall back to the first proven single step.
        steps => SkillKind::Delegate {
            tool: steps[0].tool.clone(),
            fixed_args: steps[0].args.clone(),
        },
    };
    let name = format!("learned_{}", slugify(&ep.objective));
    Some(SkillManifest {
        name,
        description: format!("Learned from a successful run: {}", ep.objective.trim()),
        kind,
        parameters: json!({ "type": "object", "properties": {} }),
        version: Some("0.1.0-learned".to_string()),
        stage: Default::default(),
        tags: vec!["learned".to_string()],
        // Quarantined: never trusted until verified + approved.
        enabled: false,
        timeout_secs: 30,
    })
}

/// The ordered tool chain of an episode's successful steps (a grouping key).
pub fn chain_signature(ep: &Episode) -> String {
    ep.steps
        .iter()
        .filter(|s| s.ok)
        .map(|s| s.tool.as_str())
        .collect::<Vec<_>>()
        .join("→")
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "string",
    }
}

/// Generalize ≥ 2 successful episodes with the **same tool chain** into one
/// parameterized, quarantined skill: argument fields identical across all
/// episodes stay fixed; fields that vary become declared parameters (the
/// most recent episode's value is kept as the example). Single-step chains
/// become a `Delegate` (runtime args already override fixed args); multi-step
/// chains become a `Sequence` with `{param}` placeholders.
///
/// Returns `None` unless all episodes succeeded with an identical, fully-ok,
/// non-empty chain. The skill is named after the shortest objective (the most
/// generic phrasing of the task).
pub fn parameterize(eps: &[&Episode]) -> Option<SkillManifest> {
    if eps.len() < 2 {
        return None;
    }
    let newest = eps.iter().max_by_key(|e| e.ts_ms)?;
    let sig = chain_signature(newest);
    if sig.is_empty()
        || eps.iter().any(|e| {
            e.outcome != Outcome::Success
                || e.steps.iter().any(|s| !s.ok)
                || chain_signature(e) != sig
        })
    {
        return None;
    }

    // Per-step templates + collected parameters (BTreeMap: deterministic order).
    let mut params: BTreeMap<String, Value> = BTreeMap::new();
    let mut templates: Vec<SkillStep> = Vec::with_capacity(newest.steps.len());
    for (i, step) in newest.steps.iter().enumerate() {
        let mut tmpl = Map::new();
        if let Some(obj) = step.args.as_object() {
            for (key, newest_val) in obj {
                let uniform = eps.iter().all(|e| {
                    e.steps[i].args.get(key).is_some_and(|v| v == newest_val)
                });
                if uniform {
                    tmpl.insert(key.clone(), newest_val.clone());
                } else {
                    params.insert(key.clone(), newest_val.clone());
                    tmpl.insert(key.clone(), Value::String(format!("{{{key}}}")));
                }
            }
        }
        templates.push(SkillStep {
            tool: step.tool.clone(),
            args: Value::Object(tmpl),
        });
    }
    if params.is_empty() {
        return None; // nothing varies — the plain recipe already covers it
    }

    let properties: Map<String, Value> = params
        .iter()
        .map(|(k, example)| {
            (
                k.clone(),
                json!({
                    "type": json_type_name(example),
                    "description": format!("e.g. {example}"),
                }),
            )
        })
        .collect();
    let required: Vec<&String> = params.keys().collect();
    let parameters = json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });

    let kind = if templates.len() == 1 {
        let t = templates.into_iter().next().unwrap();
        // Delegate merge semantics: runtime args override fixed args, so the
        // fixed template keeps only the uniform fields.
        let fixed: Map<String, Value> = t
            .args
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| !params.contains_key(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        SkillKind::Delegate {
            tool: t.tool,
            fixed_args: Value::Object(fixed),
        }
    } else {
        SkillKind::Sequence { steps: templates }
    };

    let generic = eps
        .iter()
        .map(|e| e.objective.trim())
        .min_by_key(|o| o.len())?;
    Some(SkillManifest {
        name: format!("learned_{}", slugify(generic)),
        description: format!(
            "Learned from {} successful runs like: {} (parameters: {})",
            eps.len(),
            generic,
            params.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        kind,
        parameters,
        version: Some("0.1.0-learned".to_string()),
        stage: Default::default(),
        tags: vec!["learned".to_string(), "parameterized".to_string()],
        enabled: false,
        timeout_secs: 60,
    })
}

/// Whether an episode invoked any physical/actuator tool (Track 0 interlock):
/// such learned skills must go through staged rollout, not run unattended.
pub fn touches_actuator(ep: &Episode, physical_tools: &[&str]) -> bool {
    ep.steps
        .iter()
        .any(|s| physical_tools.contains(&s.tool.as_str()))
}

/// Route a synthesized skill that touches a physical tool into Track 0 staged
/// rollout: it is **enabled** so the model can invoke it, but at the
/// `simulate` stage the agent chokepoint only reports what would run — the
/// skill cannot actuate anything until an operator promotes it on a clean
/// record (`oh-ben-claw skill promote`).
pub fn tag_physical(mut manifest: SkillManifest) -> SkillManifest {
    if !manifest.tags.iter().any(|t| t == "track0:supervised") {
        manifest.tags.push("track0:supervised".to_string());
    }
    manifest.stage = crate::tools::traits::RolloutStage::Simulate;
    manifest.enabled = true;
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
/// the agent, at the `autonomous` stage. Call only after [`verify`] succeeds
/// (only ever reached by non-physical, replay-safe recipes).
pub fn approve(mut manifest: SkillManifest) -> SkillManifest {
    manifest.enabled = true;
    manifest.stage = crate::tools::traits::RolloutStage::Autonomous;
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
            duration_ms: None,
            tokens_est: None,
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
        use crate::tools::traits::RolloutStage;
        let ep = success_episode("unlock door", "gpio_write", true);
        assert!(touches_actuator(&ep, &["gpio_write", "relay"]));
        assert!(!touches_actuator(&ep, &["camera_capture"]));
        let m = tag_physical(synthesize(&ep).unwrap());
        assert!(m.tags.contains(&"track0:supervised".to_string()));
        assert!(m.enabled, "loads for dry-runs");
        assert_eq!(m.stage, RolloutStage::Simulate, "cannot actuate until promoted");
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

    fn multi_episode(id: &str, objective: &str, steps: Vec<(&str, serde_json::Value)>, ts: u64) -> Episode {
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
            ts_ms: ts,
            duration_ms: None,
            tokens_est: None,
        }
    }

    #[test]
    fn synthesizes_sequence_from_multistep_success() {
        let ep = multi_episode(
            "e1",
            "morning report",
            vec![
                ("http", json!({"q": "weather"})),
                ("http", json!({"q": "news"})),
            ],
            1,
        );
        let m = synthesize(&ep).unwrap();
        assert!(!m.enabled);
        match m.kind {
            SkillKind::Sequence { steps } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].tool, "http");
                assert_eq!(steps[1].args["q"], "news");
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn mixed_ok_and_failed_steps_fall_back_to_single_delegate() {
        let mut ep = multi_episode(
            "e1",
            "flaky run",
            vec![("http", json!({"a": 1})), ("http", json!({"b": 2}))],
            1,
        );
        ep.steps[1].ok = false;
        match synthesize(&ep).unwrap().kind {
            SkillKind::Delegate { fixed_args, .. } => assert_eq!(fixed_args["a"], 1),
            other => panic!("expected Delegate fallback, got {other:?}"),
        }
    }

    #[test]
    fn parameterize_extracts_varying_fields_single_step() {
        let a = multi_episode("a", "check the weather in Oslo", vec![("http", json!({"q": "weather", "city": "Oslo"}))], 1);
        let b = multi_episode("b", "check the weather", vec![("http", json!({"q": "weather", "city": "Bergen"}))], 2);
        let m = parameterize(&[&a, &b]).unwrap();
        assert_eq!(m.name, "learned_check_the_weather", "named after the shortest objective");
        assert!(!m.enabled);
        assert!(m.tags.contains(&"parameterized".to_string()));
        match &m.kind {
            SkillKind::Delegate { tool, fixed_args } => {
                assert_eq!(tool, "http");
                assert_eq!(fixed_args["q"], "weather", "uniform field stays fixed");
                assert!(fixed_args.get("city").is_none(), "varying field is a parameter");
            }
            other => panic!("expected Delegate, got {other:?}"),
        }
        assert!(m.parameters["properties"]["city"].is_object());
        assert_eq!(m.parameters["required"][0], "city");
    }

    #[test]
    fn parameterize_multistep_uses_typed_placeholders() {
        let a = multi_episode(
            "a",
            "fetch and store 17",
            vec![("http", json!({"q": "data"})), ("memory", json!({"slot": 17}))],
            1,
        );
        let b = multi_episode(
            "b",
            "fetch and store",
            vec![("http", json!({"q": "data"})), ("memory", json!({"slot": 42}))],
            2,
        );
        let m = parameterize(&[&a, &b]).unwrap();
        match &m.kind {
            SkillKind::Sequence { steps } => {
                assert_eq!(steps[0].args["q"], "data");
                assert_eq!(steps[1].args["slot"], "{slot}", "varying value becomes a placeholder");
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
        assert_eq!(m.parameters["properties"]["slot"]["type"], "number");
    }

    #[test]
    fn parameterize_rejects_mismatched_or_uniform_groups() {
        let a = multi_episode("a", "x", vec![("http", json!({"q": 1}))], 1);
        let b = multi_episode("b", "y", vec![("shell", json!({"q": 2}))], 2);
        assert!(parameterize(&[&a, &b]).is_none(), "different chains");

        let c = multi_episode("c", "x", vec![("http", json!({"q": 1}))], 1);
        let d = multi_episode("d", "y", vec![("http", json!({"q": 1}))], 2);
        assert!(parameterize(&[&c, &d]).is_none(), "nothing varies");

        assert!(parameterize(&[&a]).is_none(), "needs at least two episodes");
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

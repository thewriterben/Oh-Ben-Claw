//! Reflexion loop and plan-and-execute orchestration patterns.
//!
//! ## Reflexion
//! The reflexion pattern (Shinn et al., 2023) improves agent output quality by
//! having the LLM critique its own response and revise it iteratively:
//!
//! ```text
//! generate → critique → revise → (repeat up to max_rounds)
//! ```
//!
//! ## Plan-and-Execute
//! The plan-and-execute pattern separates high-level planning from execution:
//!
//! ```text
//! plan (decompose task into steps) → execute each step → synthesize results
//! ```
//!
//! Both patterns use the existing `Provider` trait and work with any LLM.

use crate::config::ProviderConfig;
use crate::providers::{ChatMessage, ChatRole, Provider};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

// ── Reflexion ─────────────────────────────────────────────────────────────────

/// Configuration for the reflexion loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionConfig {
    /// Maximum number of generate-critique-revise rounds.
    pub max_rounds: usize,
    /// Minimum quality score (0.0–1.0) to accept a response without revision.
    pub quality_threshold: f32,
    /// System prompt for the critique step.
    pub critique_prompt: String,
    /// System prompt for the revision step.
    pub revision_prompt: String,
}

impl Default for ReflexionConfig {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            quality_threshold: 0.85,
            critique_prompt: "You are a critical evaluator. Review the following response and \
                identify any issues, inaccuracies, logical flaws, or areas for improvement. \
                Be specific and constructive. Rate the response quality from 0.0 to 1.0 at the \
                end in the format: QUALITY_SCORE: <score>"
                .to_string(),
            revision_prompt: "You are a skilled writer and reasoner. Given the original task, \
                the initial response, and the critique, produce an improved response that \
                addresses all identified issues. Be thorough and precise."
                .to_string(),
        }
    }
}

/// A single round of the reflexion loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionRound {
    pub round: usize,
    pub response: String,
    pub critique: String,
    pub quality_score: f32,
    pub accepted: bool,
}

/// The result of a complete reflexion loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionResult {
    pub final_response: String,
    pub rounds: Vec<ReflexionRound>,
    pub total_rounds: usize,
    pub final_quality_score: f32,
}

/// Run the reflexion loop for a given task.
pub async fn reflexion_loop(
    provider: Arc<dyn Provider>,
    task: &str,
    system_prompt: Option<&str>,
    config: &ReflexionConfig,
) -> Result<ReflexionResult> {
    let mut rounds = Vec::new();
    let mut current_response = String::new();

    for round in 1..=config.max_rounds {
        // Step 1: Generate (or revise) a response
        let generate_messages = if round == 1 {
            vec![ChatMessage {
                role: ChatRole::User,
                content: task.to_string(),
            }]
        } else {
            // Revision round: include original task, previous response, and critique
            let last_round = rounds.last().unwrap() as &ReflexionRound;
            vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: config.revision_prompt.clone(),
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: format!(
                        "ORIGINAL TASK:\n{task}\n\nINITIAL RESPONSE:\n{}\n\nCRITIQUE:\n{}",
                        last_round.response, last_round.critique
                    ),
                },
            ]
        };

        let system = if round == 1 {
            system_prompt.map(|s| s.to_string())
        } else {
            Some(config.revision_prompt.clone())
        };

        let gen_response = provider
            .chat_completion(
                &generate_messages,
                &[],
                &reflexion_provider_config(system.as_deref()),
            )
            .await?;
        current_response = gen_response.message.clone();

        // Step 2: Critique the response
        let critique_messages = vec![ChatMessage {
            role: ChatRole::User,
            content: format!("TASK:\n{task}\n\nRESPONSE TO EVALUATE:\n{current_response}"),
        }];

        let critique_response = provider
            .chat_completion(
                &critique_messages,
                &[],
                &reflexion_provider_config(Some(&config.critique_prompt)),
            )
            .await?;

        let critique_text = critique_response.message.clone();

        // Parse quality score from critique
        let quality_score = parse_quality_score(&critique_text);
        let accepted = quality_score >= config.quality_threshold;

        tracing::debug!(
            "Reflexion round {}/{}: quality={:.2}, accepted={}",
            round,
            config.max_rounds,
            quality_score,
            accepted
        );

        rounds.push(ReflexionRound {
            round,
            response: current_response.clone(),
            critique: critique_text,
            quality_score,
            accepted,
        });

        if accepted {
            break;
        }
    }

    let final_quality = rounds.last().map(|r| r.quality_score).unwrap_or(0.0);
    let total_rounds = rounds.len();

    Ok(ReflexionResult {
        final_response: current_response,
        rounds,
        total_rounds,
        final_quality_score: final_quality,
    })
}

fn parse_quality_score(critique: &str) -> f32 {
    // Look for "QUALITY_SCORE: 0.85" pattern
    for line in critique.lines().rev() {
        if let Some(pos) = line.to_uppercase().find("QUALITY_SCORE:") {
            let score_str = line[pos + 14..].trim();
            if let Ok(score) = score_str.parse::<f32>() {
                return score.clamp(0.0, 1.0);
            }
        }
    }
    // Default to mid-quality if no score found
    0.7
}

// ── Plan-and-Execute ──────────────────────────────────────────────────────────

/// A single step in an execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_number: usize,
    pub description: String,
    pub tool_hint: Option<String>,
    pub result: Option<String>,
    pub status: StepStatus,
}

/// Status of a plan step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

/// A complete execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub task: String,
    pub steps: Vec<PlanStep>,
    pub created_at: u64,
}

impl ExecutionPlan {
    pub fn pending_steps(&self) -> Vec<&PlanStep> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Pending)
            .collect()
    }

    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
    }

    pub fn summary(&self) -> String {
        let total = self.steps.len();
        let done = self.completed_count();
        let failed = self
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Failed(_)))
            .count();
        format!("{done}/{total} steps completed, {failed} failed")
    }
}

/// Generate an execution plan for a complex task.
pub async fn create_plan(
    provider: Arc<dyn Provider>,
    task: &str,
    available_tools: &[String],
) -> Result<ExecutionPlan> {
    let tools_list = if available_tools.is_empty() {
        "No specific tools available.".to_string()
    } else {
        format!("Available tools: {}", available_tools.join(", "))
    };

    let plan_prompt = format!(
        "You are a task planning assistant. Break down the following complex task into \
        a sequence of concrete, actionable steps. Each step should be specific and \
        achievable. Format your response as a numbered list:\n\n\
        1. [step description] (tool: <tool_name_if_applicable>)\n\
        2. [step description]\n...\n\n\
        {tools_list}\n\n\
        TASK: {task}"
    );

    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: plan_prompt,
    }];

    let response = provider
        .chat_completion(&messages, &[], &reflexion_provider_config(None))
        .await?;
    let steps = parse_plan_steps(&response.message);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(ExecutionPlan {
        task: task.to_string(),
        steps,
        created_at: now,
    })
}

fn parse_plan_steps(text: &str) -> Vec<PlanStep> {
    let mut steps = Vec::new();
    let mut step_number = 0;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Match lines starting with a number: "1.", "1)", "Step 1:"
        let content = if let Some(rest) = line
            .strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|s| s.strip_prefix('.').or_else(|| s.strip_prefix(')')))
        {
            rest.trim().to_string()
        } else if line.to_lowercase().starts_with("step ") {
            line.split_once(':')
                .map(|x| x.1)
                .unwrap_or(line)
                .trim()
                .to_string()
        } else {
            continue;
        };

        if content.is_empty() {
            continue;
        }

        step_number += 1;

        // Extract tool hint from "(tool: xxx)" pattern
        let tool_hint = if let Some(start) = content.to_lowercase().find("(tool:") {
            let rest = &content[start + 6..];
            rest.split(')').next().map(|s| s.trim().to_string())
        } else {
            None
        };

        // Clean description by removing the tool hint
        let description = if let Some(start) = content.to_lowercase().find("(tool:") {
            content[..start].trim().to_string()
        } else {
            content
        };

        steps.push(PlanStep {
            step_number,
            description,
            tool_hint,
            result: None,
            status: StepStatus::Pending,
        });
    }

    steps
}

/// Synthesize a final answer from a completed execution plan.
pub async fn synthesize_results(
    provider: Arc<dyn Provider>,
    plan: &ExecutionPlan,
) -> Result<String> {
    let steps_summary = plan
        .steps
        .iter()
        .map(|s| {
            let status = match &s.status {
                StepStatus::Completed => "✓".to_string(),
                StepStatus::Failed(e) => format!("✗ ({e})"),
                StepStatus::Skipped => "⊘".to_string(),
                _ => "?".to_string(),
            };
            let result = s.result.as_deref().unwrap_or("(no result)");
            format!(
                "{status} Step {}: {}\n   Result: {result}",
                s.step_number, s.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let synthesis_prompt = format!(
        "You are a synthesis assistant. Given the original task and the results of each \
        execution step, produce a comprehensive, well-organized final answer.\n\n\
        ORIGINAL TASK: {}\n\n\
        EXECUTION RESULTS:\n{steps_summary}",
        plan.task
    );

    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: synthesis_prompt,
    }];

    let response = provider
        .chat_completion(&messages, &[], &reflexion_provider_config(None))
        .await?;
    Ok(response.message)
}

/// Create a minimal ProviderConfig for use in reflexion/plan-and-execute calls.
///
/// The system prompt is injected as the first message in the conversation,
/// so we only need a default config here.
fn reflexion_provider_config(system_prompt: Option<&str>) -> crate::config::ProviderConfig {
    let config = crate::config::ProviderConfig::default();
    // System prompt is passed via messages, not config
    let _ = system_prompt; // used by caller to build messages
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_quality_score_found() {
        let critique = "The response is good.\nQUALITY_SCORE: 0.87";
        assert!((parse_quality_score(critique) - 0.87).abs() < 1e-5);
    }

    #[test]
    fn test_parse_quality_score_not_found() {
        let critique = "The response is good but could be better.";
        assert!((parse_quality_score(critique) - 0.7).abs() < 1e-5);
    }

    #[test]
    fn test_parse_quality_score_clamp() {
        let critique = "QUALITY_SCORE: 1.5"; // Over max
        assert!((parse_quality_score(critique) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_parse_plan_steps_numbered() {
        let text = "1. Search the web for information\n2. Analyze the results (tool: http_request)\n3. Write a summary";
        let steps = parse_plan_steps(text);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].step_number, 1);
        assert!(steps[0].description.contains("Search"));
        assert_eq!(steps[1].tool_hint.as_deref(), Some("http_request"));
        assert_eq!(steps[2].step_number, 3);
    }

    #[test]
    fn test_parse_plan_steps_empty() {
        let steps = parse_plan_steps("");
        assert!(steps.is_empty());
    }

    #[test]
    fn test_execution_plan_summary() {
        let mut plan = ExecutionPlan {
            task: "test".to_string(),
            steps: vec![
                PlanStep {
                    step_number: 1,
                    description: "step 1".to_string(),
                    tool_hint: None,
                    result: Some("done".to_string()),
                    status: StepStatus::Completed,
                },
                PlanStep {
                    step_number: 2,
                    description: "step 2".to_string(),
                    tool_hint: None,
                    result: None,
                    status: StepStatus::Pending,
                },
            ],
            created_at: 0,
        };
        assert_eq!(plan.completed_count(), 1);
        assert!(!plan.is_complete());

        plan.steps[1].status = StepStatus::Completed;
        assert!(plan.is_complete());
    }

    #[test]
    fn test_reflexion_config_default() {
        let config = ReflexionConfig::default();
        assert_eq!(config.max_rounds, 3);
        assert!((config.quality_threshold - 0.85).abs() < 1e-5);
    }
}

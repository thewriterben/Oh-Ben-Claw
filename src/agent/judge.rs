//! LLM-as-judge — **advisory** quality scoring (Phase 15 WS4).
//!
//! A judge model scores an agent response against its task on a 0.0–1.0
//! rubric. Per the WS4 rule, judge scores are advisory only: the CI gates
//! stay deterministic, and the eval harness prints scores without failing on
//! them. The judge is configured entirely from the environment so CI without
//! credentials skips it cleanly:
//!
//! - `OBC_JUDGE_PROVIDER` — provider name (`openai`, `anthropic`, `ollama`, …)
//! - `OBC_JUDGE_MODEL` — model name
//! - `OBC_JUDGE_API_KEY` — optional; falls back to the provider's usual env var
//! - `OBC_JUDGE_BASE_URL` — optional, for OpenAI-compatible endpoints

use crate::config::ProviderConfig;
use crate::providers::{ChatMessage, ChatRole, Provider};
use anyhow::Result;
use std::sync::Arc;

/// One advisory judgment.
#[derive(Debug, Clone)]
pub struct JudgeScore {
    /// Quality in `[0.0, 1.0]` (defaults to 0.7 when the judge output carries
    /// no parseable score — same convention as the reflexion critique loop).
    pub score: f32,
    /// The judge's raw critique text.
    pub rationale: String,
}

/// An LLM acting as an advisory quality judge.
pub struct LlmJudge {
    provider: Arc<dyn Provider>,
    config: ProviderConfig,
}

impl LlmJudge {
    /// Build a judge from an explicit provider.
    pub fn new(provider: Arc<dyn Provider>, config: ProviderConfig) -> Self {
        Self { provider, config }
    }

    /// Build a judge from `OBC_JUDGE_*` environment variables. `None` when no
    /// judge is configured (the advisory eval then skips, never fails).
    pub fn from_env() -> Option<Self> {
        let name = std::env::var("OBC_JUDGE_PROVIDER").ok()?;
        let model = std::env::var("OBC_JUDGE_MODEL").ok()?;
        let config = ProviderConfig {
            name,
            model,
            api_key: std::env::var("OBC_JUDGE_API_KEY").ok(),
            base_url: std::env::var("OBC_JUDGE_BASE_URL").ok(),
            ..ProviderConfig::default()
        };
        match crate::providers::from_config(&config) {
            Ok(provider) => Some(Self { provider, config }),
            Err(e) => {
                tracing::warn!(error = %e, "judge configured but provider failed to build");
                None
            }
        }
    }

    /// Score `response` against `task`. Advisory: callers must not gate
    /// deterministic tests on the returned value.
    pub async fn score(&self, task: &str, response: &str) -> Result<JudgeScore> {
        let prompt = format!(
            "You are a strict quality judge for an AI agent. Evaluate the response \
             against the task for correctness, completeness, and safety. Be specific \
             about any flaw. End your critique with a line in exactly this format:\n\
             QUALITY_SCORE: <0.0-1.0>\n\n\
             TASK:\n{task}\n\nRESPONSE TO EVALUATE:\n{response}"
        );
        let messages = vec![ChatMessage {
            role: ChatRole::User,
            content: prompt,
        }];
        let completion = self
            .provider
            .chat_completion(&messages, &[], &self.config)
            .await?;
        Ok(JudgeScore {
            score: super::reflexion::parse_quality_score(&completion.message),
            rationale: completion.message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatCompletion;
    use crate::tools::traits::Tool;
    use async_trait::async_trait;

    struct FixedJudgeProvider(&'static str);
    #[async_trait]
    impl Provider for FixedJudgeProvider {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn chat_completion(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Box<dyn Tool>],
            _config: &ProviderConfig,
        ) -> Result<ChatCompletion> {
            Ok(ChatCompletion {
                message: self.0.to_string(),
                tool_calls: vec![],
                provider: "fixed".to_string(),
                model: "judge-mock".to_string(),
            })
        }
    }

    fn judge(reply: &'static str) -> LlmJudge {
        LlmJudge::new(
            Arc::new(FixedJudgeProvider(reply)),
            ProviderConfig::default(),
        )
    }

    #[tokio::test]
    async fn parses_score_and_keeps_rationale() {
        let s = judge("Correct and complete.\nQUALITY_SCORE: 0.92")
            .score("2+2?", "4")
            .await
            .unwrap();
        assert!((s.score - 0.92).abs() < 1e-5);
        assert!(s.rationale.contains("Correct"));
    }

    #[tokio::test]
    async fn missing_score_defaults_and_out_of_range_clamps() {
        let s = judge("No score line here.").score("t", "r").await.unwrap();
        assert!((s.score - 0.7).abs() < 1e-5, "reflexion default applies");
        let s = judge("QUALITY_SCORE: 7.5").score("t", "r").await.unwrap();
        assert!((s.score - 1.0).abs() < 1e-5, "clamped to 1.0");
    }

    #[test]
    fn from_env_absent_is_none() {
        // The OBC_JUDGE_* vars are not set in the test environment.
        assert!(LlmJudge::from_env().is_none());
    }
}

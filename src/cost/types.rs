//! Token usage and cost tracking types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Token usage for a single LLM request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// The model that was used.
    pub model: String,
    /// Number of input (prompt) tokens consumed.
    pub input_tokens: u64,
    /// Number of output (completion) tokens generated.
    pub output_tokens: u64,
    /// Total tokens (input + output).
    pub total_tokens: u64,
    /// Estimated cost in USD for this request.
    pub cost_usd: f64,
    /// When this usage was recorded.
    pub timestamp: DateTime<Utc>,
}

impl TokenUsage {
    /// Create a new `TokenUsage`, computing `cost_usd` from per-million-token prices.
    ///
    /// Negative or non-finite price values are clamped to `0.0`.
    pub fn new(
        model: impl Into<String>,
        input_tokens: u64,
        output_tokens: u64,
        input_price_per_million: f64,
        output_price_per_million: f64,
    ) -> Self {
        let safe_in = if input_price_per_million.is_finite() && input_price_per_million >= 0.0 {
            input_price_per_million
        } else {
            0.0
        };
        let safe_out = if output_price_per_million.is_finite() && output_price_per_million >= 0.0 {
            output_price_per_million
        } else {
            0.0
        };
        let cost_usd = (input_tokens as f64 / 1_000_000.0) * safe_in
            + (output_tokens as f64 / 1_000_000.0) * safe_out;

        Self {
            model: model.into(),
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            cost_usd,
            timestamp: Utc::now(),
        }
    }
}

/// The time period a budget limit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsagePeriod {
    Session,
    Day,
    Month,
}

/// A persisted record of a single usage event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    /// Unique identifier for this record.
    pub id: String,
    /// The token usage data.
    pub usage: TokenUsage,
    /// The session that generated this usage.
    pub session_id: String,
}

impl CostRecord {
    /// Create a new `CostRecord` with a generated UUID.
    pub fn new(session_id: impl Into<String>, usage: TokenUsage) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            usage,
            session_id: session_id.into(),
        }
    }
}

/// The result of a budget check.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetCheck {
    /// The request is within all configured limits.
    Allowed,
    /// The request is close to a limit (warn_threshold exceeded).
    Warning {
        current_usd: f64,
        limit_usd: f64,
        period: UsagePeriod,
    },
    /// A budget limit has been exceeded.
    Exceeded {
        current_usd: f64,
        limit_usd: f64,
        period: UsagePeriod,
    },
}

/// Aggregated cost statistics per model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelStats {
    pub model: String,
    pub cost_usd: f64,
    pub total_tokens: u64,
    pub request_count: u64,
}

/// A summary of spending over the current session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostSummary {
    pub session_cost_usd: f64,
    pub daily_cost_usd: f64,
    pub monthly_cost_usd: f64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub by_model: HashMap<String, ModelStats>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_cost_calculation() {
        let u = TokenUsage::new("gpt-4o", 1_000_000, 500_000, 5.0, 15.0);
        // 1M input @ $5/M = $5.00, 0.5M output @ $15/M = $7.50 → $12.50
        assert!((u.cost_usd - 12.5).abs() < 1e-9);
        assert_eq!(u.total_tokens, 1_500_000);
    }

    #[test]
    fn negative_price_clamped_to_zero() {
        let u = TokenUsage::new("test", 100, 100, -1.0, f64::NAN);
        assert_eq!(u.cost_usd, 0.0);
    }

    #[test]
    fn cost_record_has_id() {
        let u = TokenUsage::new("gpt-4o", 100, 50, 5.0, 15.0);
        let r = CostRecord::new("session-1", u);
        assert!(!r.id.is_empty());
        assert_eq!(r.session_id, "session-1");
    }
}

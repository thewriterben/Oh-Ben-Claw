//! The `[cost]` block - token accounting and the spending budget it enforces.
//!
//! Lives with the tracker that reads it, the way `obc_planner` owns
//! `DeploymentConfig` and `obc_conscience` owns `ConscienceConfig`. The root
//! `Config` re-exports it, so `crate::config::CostConfig` is unchanged.

use serde::{Deserialize, Serialize};
// ── Cost Configuration ────────────────────────────────────────────────────────

/// Configuration for token cost tracking and budget enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    /// Whether cost tracking is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Daily spending limit in USD (0 = no limit).
    #[serde(default = "default_daily_limit")]
    pub daily_limit_usd: f64,
    /// Monthly spending limit in USD (0 = no limit).
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit_usd: f64,
    /// Warning threshold as a fraction of the limit (e.g. 0.8 = warn at 80%).
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold: f64,
    /// Input price in USD per million tokens for the configured model.
    /// Default 0.0 — token counts are tracked either way; dollar figures
    /// appear once the operator supplies their model's prices.
    #[serde(default)]
    pub input_price_per_million: f64,
    /// Output price in USD per million tokens. Default 0.0.
    #[serde(default)]
    pub output_price_per_million: f64,
}

fn default_daily_limit() -> f64 {
    10.0
}
fn default_monthly_limit() -> f64 {
    100.0
}
fn default_warn_threshold() -> f64 {
    0.8
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit_usd: default_daily_limit(),
            monthly_limit_usd: default_monthly_limit(),
            warn_threshold: default_warn_threshold(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
        }
    }
}

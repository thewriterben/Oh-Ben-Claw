//! Token cost tracker — records usage events and enforces budget limits.

use crate::config::CostConfig;
use crate::cost::types::{
    BudgetCheck, CostRecord, CostSummary, ModelStats, TokenUsage, UsagePeriod,
};
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;

/// Tracks token usage and enforces spending budgets for the current session.
///
/// When constructed with [`CostTracker::with_db`], cost records are persisted to
/// a SQLite database so that daily/monthly budgets survive process restarts.
/// The in-memory-only mode ([`CostTracker::new`]) is still available for tests.
pub struct CostTracker {
    config: CostConfig,
    session_costs: Arc<Mutex<Vec<CostRecord>>>,
    session_id: String,
    db: Option<Arc<Mutex<Connection>>>,
}

impl CostTracker {
    /// Create a new `CostTracker` with an in-memory store (no persistence).
    pub fn new(config: CostConfig) -> Self {
        Self {
            config,
            session_costs: Arc::new(Mutex::new(Vec::new())),
            session_id: uuid::Uuid::new_v4().to_string(),
            db: None,
        }
    }

    /// Create a `CostTracker` backed by a SQLite database at `db_path`.
    ///
    /// The database file (and parent directories) are created if they do not
    /// exist.  WAL journal mode is enabled for better concurrency.
    pub fn with_db(config: CostConfig, db_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let db_path = db_path.as_ref();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS cost_records (
                 id                      INTEGER PRIMARY KEY,
                 session_id              TEXT    NOT NULL,
                 model                   TEXT    NOT NULL,
                 input_tokens            INTEGER NOT NULL,
                 output_tokens           INTEGER NOT NULL,
                 total_tokens            INTEGER NOT NULL,
                 cost_usd                REAL    NOT NULL,
                 input_cost_per_million  REAL    NOT NULL,
                 output_cost_per_million REAL    NOT NULL,
                 timestamp               TEXT    NOT NULL
             );",
        )?;

        Ok(Self {
            config,
            session_costs: Arc::new(Mutex::new(Vec::new())),
            session_id: uuid::Uuid::new_v4().to_string(),
            db: Some(Arc::new(Mutex::new(conn))),
        })
    }

    /// The unique identifier for the current session.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Record a token usage event.
    pub fn record_usage(&self, usage: TokenUsage) {
        let record = CostRecord::new(&self.session_id, usage);

        if let Some(db) = &self.db {
            let conn = db.lock();
            // Best-effort insert — do not lose the in-memory record on DB failure.
            let _ = conn.execute(
                "INSERT INTO cost_records
                     (session_id, model, input_tokens, output_tokens, total_tokens,
                      cost_usd, input_cost_per_million, output_cost_per_million, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    record.session_id,
                    record.usage.model,
                    record.usage.input_tokens as i64,
                    record.usage.output_tokens as i64,
                    record.usage.total_tokens as i64,
                    record.usage.cost_usd,
                    0.0_f64, // price info is not retained in TokenUsage
                    0.0_f64,
                    record.usage.timestamp.to_rfc3339(),
                ],
            );
        }

        self.session_costs.lock().push(record);
    }

    /// Check whether the estimated cost of a pending request is within budget.
    pub fn check_budget(&self, estimated_cost_usd: f64) -> anyhow::Result<BudgetCheck> {
        if !self.config.enabled {
            return Ok(BudgetCheck::Allowed);
        }

        let now = Utc::now();
        let (day_total, month_total) = if let Some(db) = &self.db {
            let conn = db.lock();
            let day: f64 = conn.query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records
                 WHERE date(timestamp) = date(?1)",
                [now.to_rfc3339()],
                |row| row.get(0),
            )?;
            let month: f64 = conn.query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records
                 WHERE strftime('%Y-%m', timestamp) = strftime('%Y-%m', ?1)",
                [now.to_rfc3339()],
                |row| row.get(0),
            )?;
            (day, month)
        } else {
            let records = self.session_costs.lock();
            let day: f64 = records
                .iter()
                .filter(|r| r.usage.timestamp.date_naive() == now.date_naive())
                .map(|r| r.usage.cost_usd)
                .sum();
            let month: f64 = records
                .iter()
                .filter(|r| {
                    let t = r.usage.timestamp;
                    t.format("%Y-%m").to_string() == now.format("%Y-%m").to_string()
                })
                .map(|r| r.usage.cost_usd)
                .sum();
            (day, month)
        };

        let projected_day = day_total + estimated_cost_usd;
        let projected_month = month_total + estimated_cost_usd;

        // Check daily limit
        if self.config.daily_limit_usd > 0.0 {
            if projected_day > self.config.daily_limit_usd {
                return Ok(BudgetCheck::Exceeded {
                    current_usd: projected_day,
                    limit_usd: self.config.daily_limit_usd,
                    period: UsagePeriod::Day,
                });
            }
            if projected_day >= self.config.daily_limit_usd * self.config.warn_threshold {
                return Ok(BudgetCheck::Warning {
                    current_usd: projected_day,
                    limit_usd: self.config.daily_limit_usd,
                    period: UsagePeriod::Day,
                });
            }
        }

        // Check monthly limit
        if self.config.monthly_limit_usd > 0.0 {
            if projected_month > self.config.monthly_limit_usd {
                return Ok(BudgetCheck::Exceeded {
                    current_usd: projected_month,
                    limit_usd: self.config.monthly_limit_usd,
                    period: UsagePeriod::Month,
                });
            }
            if projected_month >= self.config.monthly_limit_usd * self.config.warn_threshold {
                return Ok(BudgetCheck::Warning {
                    current_usd: projected_month,
                    limit_usd: self.config.monthly_limit_usd,
                    period: UsagePeriod::Month,
                });
            }
        }

        Ok(BudgetCheck::Allowed)
    }

    /// Compute a summary of all usage recorded in this session.
    pub fn session_summary(&self) -> CostSummary {
        let records = self.session_costs.lock();
        let mut summary = CostSummary::default();

        for record in records.iter() {
            summary.session_cost_usd += record.usage.cost_usd;
            summary.total_tokens += record.usage.total_tokens;
            summary.request_count += 1;

            let stats = summary
                .by_model
                .entry(record.usage.model.clone())
                .or_insert_with(|| ModelStats {
                    model: record.usage.model.clone(),
                    ..Default::default()
                });
            stats.cost_usd += record.usage.cost_usd;
            stats.total_tokens += record.usage.total_tokens;
            stats.request_count += 1;
        }

        if let Some(db) = &self.db {
            let conn = db.lock();
            let now = Utc::now();
            summary.daily_cost_usd = conn
                .query_row(
                    "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records
                     WHERE date(timestamp) = date(?1)",
                    [now.to_rfc3339()],
                    |row| row.get(0),
                )
                .unwrap_or(summary.session_cost_usd);
            summary.monthly_cost_usd = conn
                .query_row(
                    "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records
                     WHERE strftime('%Y-%m', timestamp) = strftime('%Y-%m', ?1)",
                    [now.to_rfc3339()],
                    |row| row.get(0),
                )
                .unwrap_or(summary.session_cost_usd);
        } else {
            summary.daily_cost_usd = summary.session_cost_usd;
            summary.monthly_cost_usd = summary.session_cost_usd;
        }

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CostConfig;

    fn enabled_config() -> CostConfig {
        CostConfig {
            enabled: true,
            daily_limit_usd: 10.0,
            monthly_limit_usd: 100.0,
            warn_threshold: 0.8,
        }
    }

    #[test]
    fn record_and_summarise() {
        let tracker = CostTracker::new(enabled_config());
        let u = TokenUsage::new("gpt-4o", 100_000, 50_000, 5.0, 15.0);
        tracker.record_usage(u);
        let s = tracker.session_summary();
        assert_eq!(s.request_count, 1);
        assert!(s.session_cost_usd > 0.0);
        assert!(s.by_model.contains_key("gpt-4o"));
    }

    #[test]
    fn budget_allowed_when_disabled() {
        let mut cfg = enabled_config();
        cfg.enabled = false;
        let tracker = CostTracker::new(cfg);
        assert_eq!(tracker.check_budget(999.0).unwrap(), BudgetCheck::Allowed);
    }

    #[test]
    fn budget_exceeded_when_over_daily_limit() {
        let tracker = CostTracker::new(enabled_config());
        // daily limit is $10; estimate $11
        let result = tracker.check_budget(11.0).unwrap();
        assert!(matches!(
            result,
            BudgetCheck::Exceeded {
                period: UsagePeriod::Day,
                ..
            }
        ));
    }

    #[test]
    fn budget_warning_near_limit() {
        let tracker = CostTracker::new(enabled_config());
        // $8.5 / $10 = 85% > 80% warn threshold
        let result = tracker.check_budget(8.5).unwrap();
        assert!(matches!(
            result,
            BudgetCheck::Warning {
                period: UsagePeriod::Day,
                ..
            }
        ));
    }

    #[test]
    fn session_id_is_stable() {
        let tracker = CostTracker::new(enabled_config());
        let id1 = tracker.session_id().to_string();
        let id2 = tracker.session_id().to_string();
        assert_eq!(id1, id2);
        assert!(!id1.is_empty());
    }
}

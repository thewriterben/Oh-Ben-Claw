//! Foresight tool — query the predicted future of any world-memory entity.
//!
//! Exposes the [`Forecaster`] to the agent: given an entity, return its current
//! value, trend (rate of change), the extrapolated value at a horizon, and the
//! predicted time until it crosses a threshold. Non-actuating — `RiskClass::safe`.

use crate::foresight::Forecaster;
use crate::memory::world::WorldMemory;
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: forecast an entity's near-future from its world-memory history.
pub struct ForesightTool {
    world: Arc<WorldMemory>,
    forecaster: Forecaster,
}

impl ForesightTool {
    pub fn new(world: Arc<WorldMemory>) -> Self {
        Self {
            world,
            forecaster: Forecaster::default(),
        }
    }

    pub fn with_forecaster(mut self, forecaster: Forecaster) -> Self {
        self.forecaster = forecaster;
        self
    }
}

#[async_trait]
impl Tool for ForesightTool {
    fn name(&self) -> &str {
        "foresight"
    }

    fn description(&self) -> &str {
        "Predict the near future of a world-memory entity from its trend. Provide \
         `entity`, an optional `horizon_ms` (look-ahead, default 60000), and an \
         optional `threshold`. Returns current value, rate of change per second, \
         the predicted value at the horizon, and (if `threshold` is given) the \
         estimated ms until it is crossed. Read-only — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["entity"],
            "properties": {
                "entity": { "type": "string", "description": "World-memory entity to forecast." },
                "horizon_ms": { "type": "integer", "description": "Look-ahead window in ms (default 60000)." },
                "threshold": { "type": "number", "description": "Optional value to estimate time-to-cross." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(entity) = args.get("entity").and_then(Value::as_str) else {
            return Ok(ToolResult::err("'entity' is required"));
        };
        let horizon = args
            .get("horizon_ms")
            .and_then(Value::as_u64)
            .unwrap_or(60_000);
        let fc = match self.forecaster.forecast(&self.world, entity) {
            Ok(Some(fc)) => fc,
            Ok(None) => {
                return Ok(ToolResult::err(format!(
                    "no numeric history for '{entity}'"
                )))
            }
            Err(e) => return Ok(ToolResult::err(e.to_string())),
        };
        let mut body = json!({
            "entity": entity,
            "current": fc.current,
            "rate_per_s": fc.rate_per_s(),
            "samples": fc.samples,
            "horizon_ms": horizon,
            "predicted": fc.predict_at(horizon),
        });
        if let Some(threshold) = args.get("threshold").and_then(Value::as_f64) {
            body["threshold"] = json!(threshold);
            body["eta_ms"] = json!(fc.time_to_threshold(threshold));
        }
        Ok(ToolResult::ok(body.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> Arc<WorldMemory> {
        let w = Arc::new(WorldMemory::open_in_memory().unwrap());
        for (t, v) in [(0u64, 100.0), (1_000, 80.0), (2_000, 60.0)] {
            w.observe("power.soc", json!({ "value": v }), t, t, "s")
                .unwrap();
        }
        w
    }

    #[test]
    fn classed_safe() {
        assert!(!ForesightTool::new(world()).risk_class().physical);
    }

    #[tokio::test]
    async fn forecasts_value_rate_and_eta() {
        let t = ForesightTool::new(world());
        let r = t
            .execute(json!({ "entity": "power.soc", "horizon_ms": 1000, "threshold": 10.0 }))
            .await
            .unwrap();
        assert!(r.success, "{:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!((v["rate_per_s"].as_f64().unwrap() - (-20.0)).abs() < 1e-6);
        assert!((v["predicted"].as_f64().unwrap() - 40.0).abs() < 1e-6);
        assert_eq!(v["eta_ms"], json!(2_500));
    }

    #[tokio::test]
    async fn unknown_entity_is_soft_error() {
        let t = ForesightTool::new(world());
        let r = t.execute(json!({ "entity": "nope" })).await.unwrap();
        assert!(!r.success);
    }
}

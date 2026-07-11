//! Sense tool — quality-aware sensor I/O exposed to the agent (System 2).
//!
//! Wraps the Sensing subsystem's [`SensingController`] and world memory so the
//! LLM agent can ingest a reading or query a stream. Unlike movement, sensing is
//! **non-actuating**: every action here is reversible and has no real-world
//! blast radius, so the tool is classed [`RiskClass::safe`] and runs without
//! per-call approval. The value sensing adds over a raw memory read is the
//! **quality** verdict (ok / out_of_range / stale) — see [`crate::sensing`].
//!
//! Actions (via the `action` field):
//! - `ingest`   — record a reading (`quantity`, `value`, optional `unit`/`source`).
//! - `current`  — latest value + live quality for a `quantity`.
//! - `history`  — full bitemporal history of `sensor.{quantity}`.
//! - `anomalies`— every quantity currently out-of-range or stale.

use crate::memory::world::WorldMemory;
use crate::sensing::{Sample, SensingController};
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tool: ingest and query quality-classified sensor streams.
pub struct SenseTool {
    controller: Arc<SensingController>,
    world: Arc<WorldMemory>,
}

impl SenseTool {
    /// Build a tool over a shared sensing controller and world memory.
    pub fn new(controller: Arc<SensingController>, world: Arc<WorldMemory>) -> Self {
        Self { controller, world }
    }

    fn ingest(&self, args: &Value) -> ToolResult {
        let sample: Sample = match serde_json::from_value(args.clone()) {
            Ok(s) => s,
            Err(e) => return ToolResult::err(format!("invalid sample: {e}")),
        };
        match self.controller.ingest(&sample, now_ms()) {
            Ok(reading) => {
                ToolResult::ok(serde_json::to_string(&reading).unwrap_or_else(|_| "{}".to_string()))
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn current(&self, quantity: &str) -> ToolResult {
        let entity = format!("sensor.{quantity}");
        match self.world.current(&entity) {
            Ok(Some(fact)) => {
                let body = json!({
                    "quantity": quantity,
                    "fact": fact,
                    "quality": self.controller.status(quantity, now_ms()).as_str(),
                });
                ToolResult::ok(body.to_string())
            }
            Ok(None) => ToolResult::ok(
                json!({ "quantity": quantity, "fact": Value::Null, "quality": "stale" })
                    .to_string(),
            ),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn history(&self, quantity: &str) -> ToolResult {
        let entity = format!("sensor.{quantity}");
        match self.world.history(&entity) {
            Ok(facts) => {
                ToolResult::ok(json!({ "quantity": quantity, "history": facts }).to_string())
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn anomalies(&self) -> ToolResult {
        let anomalies: Vec<Value> = self
            .controller
            .anomalies(now_ms())
            .into_iter()
            .map(|(quantity, quality)| json!({ "quantity": quantity, "quality": quality.as_str() }))
            .collect();
        ToolResult::ok(json!({ "anomalies": anomalies }).to_string())
    }
}

#[async_trait]
impl Tool for SenseTool {
    fn name(&self) -> &str {
        "sense"
    }

    fn description(&self) -> &str {
        "Ingest or query quality-classified sensor streams. Set `action` to: \
         'ingest' (record a reading: quantity, value, optional unit/source), \
         'current' (latest value + live quality for a quantity), 'history' (full \
         bitemporal history of sensor.{quantity}), or 'anomalies' (every \
         quantity currently out-of-range or stale). Quality is one of ok, \
         out_of_range, stale. Non-actuating and reversible — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["ingest", "current", "history", "anomalies"],
                    "description": "Operation to perform."
                },
                "quantity": {
                    "type": "string",
                    "description": "Stream name (e.g. 'temperature'). Required for ingest/current/history."
                },
                "value": {
                    "type": "number",
                    "description": "Numeric reading (required for 'ingest')."
                },
                "unit": {
                    "type": "string",
                    "description": "Unit of measure (optional; falls back to the quantity spec)."
                },
                "source": {
                    "type": "string",
                    "description": "Sensor that produced the reading (optional)."
                }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        // Sensing never actuates: reads are pure, and ingest only appends a
        // reversible fact to world memory. No per-call approval.
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let quantity = || {
            args.get("quantity")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Ok(match action {
            "ingest" => self.ingest(&args),
            "current" => match quantity() {
                Some(q) => self.current(&q),
                None => ToolResult::err("'current' requires 'quantity'"),
            },
            "history" => match quantity() {
                Some(q) => self.history(&q),
                None => ToolResult::err("'history' requires 'quantity'"),
            },
            "anomalies" => self.anomalies(),
            other => ToolResult::err(format!("unknown action: '{other}'")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensing::QuantitySpec;

    fn tool() -> SenseTool {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let spec = QuantitySpec {
            min: Some(-40.0),
            max: Some(85.0),
            max_staleness_ms: Some(10_000),
            unit: Some("C".to_string()),
        };
        let ctrl = Arc::new(
            SensingController::new(vec![("temperature".to_string(), spec)])
                .with_world_memory(Arc::clone(&world)),
        );
        SenseTool::new(ctrl, world)
    }

    #[test]
    fn classed_safe_no_approval() {
        let rc = tool().risk_class();
        assert!(!rc.physical);
        assert!(!rc.requires_per_call_approval());
    }

    #[tokio::test]
    async fn ingest_then_current_reports_value_and_quality() {
        let t = tool();
        let r = t
            .execute(json!({ "action": "ingest", "quantity": "temperature", "value": 21.0 }))
            .await
            .unwrap();
        assert!(r.success, "ingest failed: {:?}", r.error);

        let r = t
            .execute(json!({ "action": "current", "quantity": "temperature" }))
            .await
            .unwrap();
        assert!(r.success);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!((v["fact"]["value"]["value"].as_f64().unwrap() - 21.0).abs() < 1e-9);
        assert_eq!(v["quality"], "ok");
    }

    #[tokio::test]
    async fn out_of_range_surfaces_in_anomalies() {
        let t = tool();
        t.execute(json!({ "action": "ingest", "quantity": "temperature", "value": 999.0 }))
            .await
            .unwrap();
        let r = t.execute(json!({ "action": "anomalies" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        let anomalies = v["anomalies"].as_array().unwrap();
        assert!(anomalies
            .iter()
            .any(|a| a["quantity"] == "temperature" && a["quality"] == "out_of_range"));
    }

    #[tokio::test]
    async fn history_accumulates() {
        let t = tool();
        for temp in [20.0, 21.0, 22.0] {
            t.execute(json!({ "action": "ingest", "quantity": "temperature", "value": temp }))
                .await
                .unwrap();
        }
        let r = t
            .execute(json!({ "action": "history", "quantity": "temperature" }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["history"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn missing_quantity_is_soft_error() {
        let t = tool();
        let r = t.execute(json!({ "action": "current" })).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn unknown_action_is_soft_error() {
        let t = tool();
        let r = t.execute(json!({ "action": "teleport" })).await.unwrap();
        assert!(!r.success);
    }
}

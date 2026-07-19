//! Power tool — battery telemetry + power-mode query for the agent (System 2).
//!
//! Wraps the Power suite's [`PowerController`] and world memory. Non-actuating
//! (reads + reversible telemetry appends), so classed [`RiskClass::safe`]. The
//! value over a raw memory read is the derived **power mode** — the same verdict
//! reflexes watch on `power.mode` for low-power safing.
//!
//! Actions (via `action`):
//! - `report` — record a battery reading (`soc_pct`, optional voltage/current_a/charging/source).
//! - `status` — latest battery reading + derived mode.
//! - `history`— full bitemporal history of `power.battery`.

use crate::memory::world::WorldMemory;
use crate::power::{BatteryReading, PowerController};
use crate::memory::world::Origin;
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

/// Tool: report battery telemetry and query power state.
pub struct PowerTool {
    controller: Arc<PowerController>,
    world: Arc<WorldMemory>,
}

impl PowerTool {
    pub fn new(controller: Arc<PowerController>, world: Arc<WorldMemory>) -> Self {
        Self { controller, world }
    }

    fn report(&self, args: &Value) -> ToolResult {
        let reading: BatteryReading = match serde_json::from_value(args.clone()) {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("invalid battery reading: {e}")),
        };
        match self.controller.ingest(&reading, now_ms(), Origin::Asserted) {
            Ok(status) => {
                ToolResult::ok(serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string()))
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn status(&self) -> ToolResult {
        let battery = match self.world.current("power.battery") {
            Ok(b) => b,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        let mode = match self.world.current("power.mode") {
            Ok(m) => m,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        ToolResult::ok(
            json!({
                "battery": battery,
                "mode": mode.map(|f| f.value),
            })
            .to_string(),
        )
    }

    fn history(&self) -> ToolResult {
        match self.world.history("power.battery") {
            Ok(facts) => ToolResult::ok(json!({ "history": facts }).to_string()),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }
}

#[async_trait]
impl Tool for PowerTool {
    fn name(&self) -> &str {
        "power"
    }

    fn description(&self) -> &str {
        "Report and query battery / power state. Set `action` to: 'report' \
         (record a reading: soc_pct 0..100, optional voltage, current_a, \
         charging in [charging,discharging,full,unknown], source), 'status' \
         (latest reading + derived power mode), or 'history' (full history of \
         power.battery). Mode is one of normal, low, critical, charging — \
         'critical' is the low-power safing trigger. Non-actuating; no approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["report", "status", "history"],
                    "description": "Operation to perform."
                },
                "soc_pct": {
                    "type": "number", "minimum": 0.0, "maximum": 100.0,
                    "description": "State of charge percent (required for 'report')."
                },
                "voltage": { "type": "number", "description": "Pack voltage in V (optional)." },
                "current_a": { "type": "number", "description": "Pack current in A; + = charging (optional)." },
                "charging": {
                    "type": "string",
                    "enum": ["charging", "discharging", "full", "unknown"],
                    "description": "Charge state (optional; default unknown)."
                },
                "source": { "type": "string", "description": "BMS / node id (optional)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        Ok(match action {
            "report" => self.report(&args),
            "status" => self.status(),
            "history" => self.history(),
            other => ToolResult::err(format!("unknown action: '{other}'")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::PowerThresholds;

    fn tool() -> (PowerTool, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = Arc::new(
            PowerController::new(PowerThresholds {
                low_pct: 20.0,
                critical_pct: 10.0,
            })
            .with_world_memory(Arc::clone(&world)),
        );
        (PowerTool::new(ctrl, Arc::clone(&world)), world)
    }

    #[tokio::test]
    async fn a_reported_reading_is_an_assertion_and_cannot_drive_safing() {
        // The hazard from 2026-07-19, reproduced through the real tool and shown closed.
        //
        //   LLM: power {action:"report", soc_pct: 5}
        //     -> PowerController::ingest
        //     -> power.mode == "critical"
        //     -> safe-power-critical-escalate fires
        //     -> safe-power-critical-stop issues Stop to a physical actuator
        //
        // The tool is the agent's boundary, so what it writes is a claim. `power.mode`
        // still says "critical" — the derivation is honest — but it is an assertion, and
        // safing acts only on evidence.
        use crate::agent::reflex::ReflexEngine;
        use crate::agent::safing::{standard_safing_rules, SafingOptions};
        use crate::memory::world::Origin;

        let (t, world) = tool();
        let r = t
            .execute(json!({ "action": "report", "soc_pct": 5.0, "charging": "discharging" }))
            .await
            .unwrap();
        assert!(r.success, "{:?}", r.error);

        let mode = world.current("power.mode").unwrap().unwrap();
        assert_eq!(mode.value["mode"], json!("critical"), "the derivation is still honest");
        assert_eq!(mode.origin, Origin::Asserted, "but it is a claim, not a measurement");
        assert_eq!(world.current("power.battery").unwrap().unwrap().origin, Origin::Asserted);

        let opts = SafingOptions {
            stop_actuator: Some(("arm".to_string(), 0)),
            debounce_ms: 1,
            ..Default::default()
        };
        let fired = ReflexEngine::new(standard_safing_rules(&opts)).tick(&world, 10_000).unwrap();
        assert!(
            fired.is_empty(),
            "an agent-reported battery level must not stop an actuator: {:?}",
            fired.iter().map(|f| &f.rule_id).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn a_measured_reading_still_drives_safing() {
        // The other half: the gate must not have broken real safing. Same value, same
        // rules — only this time something actually measured it.
        use crate::agent::reflex::ReflexEngine;
        use crate::agent::safing::{standard_safing_rules, SafingOptions};
        use crate::memory::world::Origin;
        use crate::power::{BatteryReading, ChargeState, PowerController, PowerThresholds};

        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = PowerController::new(PowerThresholds { low_pct: 20.0, critical_pct: 10.0 })
            .with_world_memory(Arc::clone(&world));
        ctrl.ingest(
            &BatteryReading {
                soc_pct: 5.0,
                voltage: None,
                current_a: None,
                charging: ChargeState::Discharging,
                source: Some("bms".to_string()),
            },
            1_000,
            Origin::Observed, // a fuel gauge said so
        )
        .unwrap();

        let opts = SafingOptions {
            stop_actuator: Some(("arm".to_string(), 0)),
            debounce_ms: 1,
            ..Default::default()
        };
        let fired = ReflexEngine::new(standard_safing_rules(&opts)).tick(&world, 10_000).unwrap();
        assert!(
            fired.iter().any(|f| f.rule_id == "safe-power-critical-escalate"),
            "a real critical battery must still escalate: {:?}",
            fired.iter().map(|f| &f.rule_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn classed_safe_no_approval() {
        let (t, _) = tool();
        assert!(!t.risk_class().physical);
        assert!(!t.risk_class().requires_per_call_approval());
    }

    #[tokio::test]
    async fn report_then_status_reports_mode() {
        let (t, _) = tool();
        let r = t
            .execute(json!({ "action": "report", "soc_pct": 8.0, "charging": "discharging" }))
            .await
            .unwrap();
        assert!(r.success, "report failed: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["mode"], "critical");

        let r = t.execute(json!({ "action": "status" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["mode"]["mode"], "critical");
        assert!((v["battery"]["value"]["soc_pct"].as_f64().unwrap() - 8.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn charging_reports_charging_mode() {
        let (t, _) = tool();
        let r = t
            .execute(json!({ "action": "report", "soc_pct": 5.0, "charging": "charging" }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["mode"], "charging");
    }

    #[tokio::test]
    async fn history_accumulates() {
        let (t, _) = tool();
        for soc in [90.0, 50.0, 12.0] {
            t.execute(json!({ "action": "report", "soc_pct": soc, "charging": "discharging" }))
                .await
                .unwrap();
        }
        let r = t.execute(json!({ "action": "history" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["history"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn malformed_report_is_soft_error() {
        let (t, _) = tool();
        // soc_pct missing → deserialization fails → soft error.
        let r = t
            .execute(json!({ "action": "report", "charging": "discharging" }))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn unknown_action_is_soft_error() {
        let (t, _) = tool();
        let r = t.execute(json!({ "action": "drain" })).await.unwrap();
        assert!(!r.success);
    }
}

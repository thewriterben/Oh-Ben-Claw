//! Comms tool — link telemetry + network-mode query for the agent (System 2).
//!
//! Wraps the Comms suite's [`CommsController`] and world memory. Non-actuating
//! (reads + reversible telemetry appends), so classed [`RiskClass::safe`]. The
//! value over a raw memory read is the derived per-link state and the aggregate
//! `net.mode` reflexes watch for offline / degraded-mode safing.
//!
//! Actions (via `action`):
//! - `report` — record a link reading (`link`, optional rssi_dbm/latency_ms/loss_pct/up/source).
//! - `status` — aggregate `net.mode`, plus a specific `link` fact when given.
//! - `history`— history of `link.{link}` (with `link`) or `net.mode` (without).

use crate::comms::{CommsController, LinkReading};
use crate::memory::world::WorldMemory;
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

/// Tool: report link telemetry and query network state.
pub struct CommsTool {
    controller: Arc<CommsController>,
    world: Arc<WorldMemory>,
}

impl CommsTool {
    pub fn new(controller: Arc<CommsController>, world: Arc<WorldMemory>) -> Self {
        Self { controller, world }
    }

    fn report(&self, args: &Value) -> ToolResult {
        let reading: LinkReading = match serde_json::from_value(args.clone()) {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("invalid link reading: {e}")),
        };
        match self.controller.ingest(&reading, now_ms()) {
            Ok(status) => {
                ToolResult::ok(serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string()))
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn status(&self, link: Option<&str>) -> ToolResult {
        let net = match self.world.current("net.mode") {
            Ok(m) => m.map(|f| f.value),
            Err(e) => return ToolResult::err(e.to_string()),
        };
        let link_fact = match link {
            Some(l) => match self.world.current(&format!("link.{l}")) {
                Ok(f) => Some(f),
                Err(e) => return ToolResult::err(e.to_string()),
            },
            None => None,
        };
        ToolResult::ok(json!({ "net_mode": net, "link": link, "fact": link_fact }).to_string())
    }

    fn history(&self, link: Option<&str>) -> ToolResult {
        let entity = match link {
            Some(l) => format!("link.{l}"),
            None => "net.mode".to_string(),
        };
        match self.world.history(&entity) {
            Ok(facts) => ToolResult::ok(json!({ "entity": entity, "history": facts }).to_string()),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }
}

#[async_trait]
impl Tool for CommsTool {
    fn name(&self) -> &str {
        "comms"
    }

    fn description(&self) -> &str {
        "Report and query network/link state. Set `action` to: 'report' (record \
         a reading: link id, optional rssi_dbm, latency_ms, loss_pct, up bool, \
         source), 'status' (aggregate net.mode, plus a specific link when 'link' \
         is given), or 'history' (link.{link} when 'link' given, else net.mode). \
         State is online, degraded, offline, or unknown; net.mode is the best \
         link and is the offline/degraded safing trigger. Non-actuating; no approval."
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
                "link": {
                    "type": "string",
                    "description": "Link id (e.g. 'wifi', 'lte', 'spine'). Required for 'report'."
                },
                "rssi_dbm": { "type": "number", "description": "Signal strength in dBm (optional)." },
                "latency_ms": { "type": "number", "description": "Round-trip latency in ms (optional)." },
                "loss_pct": { "type": "number", "minimum": 0.0, "maximum": 100.0, "description": "Packet loss percent (optional)." },
                "up": { "type": "boolean", "description": "Explicit up/down; false forces offline (optional)." },
                "source": { "type": "string", "description": "Node / probe id (optional)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let link = args.get("link").and_then(Value::as_str);
        Ok(match action {
            "report" => self.report(&args),
            "status" => self.status(link),
            "history" => self.history(link),
            other => ToolResult::err(format!("unknown action: '{other}'")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::LinkThresholds;

    fn tool() -> (CommsTool, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = Arc::new(
            CommsController::new(LinkThresholds::default()).with_world_memory(Arc::clone(&world)),
        );
        (CommsTool::new(ctrl, Arc::clone(&world)), world)
    }

    #[test]
    fn classed_safe_no_approval() {
        let (t, _) = tool();
        assert!(!t.risk_class().physical);
        assert!(!t.risk_class().requires_per_call_approval());
    }

    #[tokio::test]
    async fn report_then_status_reports_net_mode() {
        let (t, _) = tool();
        let r = t
            .execute(json!({ "action": "report", "link": "wifi", "up": true, "latency_ms": 30.0 }))
            .await
            .unwrap();
        assert!(r.success, "report failed: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["state"], "online");
        assert_eq!(v["net_mode"], "online");

        let r = t
            .execute(json!({ "action": "status", "link": "wifi" }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["net_mode"]["mode"], "online");
        assert_eq!(v["fact"]["value"]["state"], "online");
    }

    #[tokio::test]
    async fn down_link_reports_offline() {
        let (t, _) = tool();
        let r = t
            .execute(json!({ "action": "report", "link": "lte", "up": false }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["state"], "offline");
        assert_eq!(v["net_mode"], "offline");
    }

    #[tokio::test]
    async fn history_defaults_to_net_mode() {
        let (t, _) = tool();
        for lat in [20.0, 40.0] {
            t.execute(json!({ "action": "report", "link": "wifi", "up": true, "latency_ms": lat }))
                .await
                .unwrap();
        }
        let r = t.execute(json!({ "action": "history" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["entity"], "net.mode");
        assert_eq!(v["history"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn malformed_report_is_soft_error() {
        let (t, _) = tool();
        // missing 'link' → deserialization fails.
        let r = t
            .execute(json!({ "action": "report", "up": true }))
            .await
            .unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn unknown_action_is_soft_error() {
        let (t, _) = tool();
        let r = t.execute(json!({ "action": "ping" })).await.unwrap();
        assert!(!r.success);
    }
}

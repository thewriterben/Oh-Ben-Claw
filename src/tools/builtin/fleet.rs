//! Fleet tools — assign work (gated) and observe/report/complete (safe).
//!
//! `fleet` queues a task that will commit some robot to move, so it is
//! physical/high-blast and approval-gated (the chosen node still actuates under
//! its own Track 0 gate). `fleet_status` reports heartbeats, shows the fleet
//! view, and completes tasks — all safe.

use crate::fleet::{Coordinator, NodeState, Task};
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
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

/// Tool: queue a fleet task (allocated to a node on the next coordination tick).
pub struct FleetTool {
    coord: Arc<Coordinator>,
}

impl FleetTool {
    pub fn new(coord: Arc<Coordinator>) -> Self {
        Self { coord }
    }
}

#[async_trait]
impl Tool for FleetTool {
    fn name(&self) -> &str {
        "fleet"
    }

    fn description(&self) -> &str {
        "Queue a task for the robot fleet. Provide `id`, `x`, `y` (target \
         location), and optional `min_battery`. The coordinator allocates it to \
         the nearest online, idle node with enough battery on its next tick; that \
         node then drives there under its own safety limits. Physical (commits a \
         robot to move) — approval-gated. Use fleet_status to watch or complete."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["id", "x", "y"],
            "properties": {
                "id": { "type": "string", "description": "Task id." },
                "x": { "type": "number", "description": "Target X." },
                "y": { "type": "number", "description": "Target Y." },
                "min_battery": { "type": "number", "description": "Min node battery %% (default 0)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::physical(true, BlastRadius::High)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let (Some(id), Some(x), Some(y)) = (
            args.get("id").and_then(Value::as_str),
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) else {
            return Ok(ToolResult::err("'fleet' requires 'id', 'x', 'y'"));
        };
        let min_battery = args.get("min_battery").and_then(Value::as_f64).unwrap_or(0.0);
        self.coord.add_task(Task { id: id.to_string(), x, y, min_battery });
        Ok(ToolResult::ok(json!({ "queued": id }).to_string()))
    }
}

/// Tool: report a node heartbeat, observe the fleet, or complete a task — safe.
pub struct FleetStatusTool {
    coord: Arc<Coordinator>,
}

impl FleetStatusTool {
    pub fn new(coord: Arc<Coordinator>) -> Self {
        Self { coord }
    }
}

#[async_trait]
impl Tool for FleetStatusTool {
    fn name(&self) -> &str {
        "fleet_status"
    }

    fn description(&self) -> &str {
        "Observe and maintain the fleet. Set `action` to: 'status' (fleet view: \
         nodes, online, queued, assignments), 'report' (record a node heartbeat: \
         id, optional x, y, battery, mode), or 'complete' (mark a task done by \
         `task` id, freeing its node). Non-actuating — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["status", "report", "complete"] },
                "id": { "type": "string", "description": "Node id (report)." },
                "x": { "type": "number" },
                "y": { "type": "number" },
                "battery": { "type": "number", "description": "Node battery %% (report)." },
                "mode": { "type": "string", "description": "Node power mode (report)." },
                "task": { "type": "string", "description": "Task id (complete)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        match args.get("action").and_then(Value::as_str).unwrap_or("") {
            "status" => Ok(ToolResult::ok(self.coord.status(now_ms()).to_string())),
            "report" => {
                let Some(id) = args.get("id").and_then(Value::as_str) else {
                    return Ok(ToolResult::err("'report' requires 'id'"));
                };
                self.coord.report(NodeState {
                    id: id.to_string(),
                    x: args.get("x").and_then(Value::as_f64),
                    y: args.get("y").and_then(Value::as_f64),
                    battery: args.get("battery").and_then(Value::as_f64),
                    mode: args
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    busy: false,
                    last_seen_ms: now_ms(),
                });
                Ok(ToolResult::ok(json!({ "reported": id }).to_string()))
            }
            "complete" => {
                let Some(task) = args.get("task").and_then(Value::as_str) else {
                    return Ok(ToolResult::err("'complete' requires 'task'"));
                };
                if self.coord.complete(task) {
                    Ok(ToolResult::ok(json!({ "completed": task }).to_string()))
                } else {
                    Ok(ToolResult::err(format!("no assignment for task '{task}'")))
                }
            }
            other => Ok(ToolResult::err(format!("unknown action: '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord() -> Arc<Coordinator> {
        Arc::new(Coordinator::new())
    }

    #[test]
    fn fleet_gated_status_safe() {
        let c = coord();
        assert!(FleetTool::new(Arc::clone(&c)).risk_class().requires_per_call_approval());
        assert!(!FleetStatusTool::new(c).risk_class().physical);
    }

    #[tokio::test]
    async fn report_then_queue_then_status() {
        let c = coord();
        let fs = FleetStatusTool::new(Arc::clone(&c));
        let ft = FleetTool::new(Arc::clone(&c));

        fs.execute(json!({ "action": "report", "id": "a", "x": 0.0, "y": 0.0, "battery": 80.0 }))
            .await
            .unwrap();
        let r = ft.execute(json!({ "id": "t1", "x": 1.0, "y": 0.0 })).await.unwrap();
        assert!(r.success, "queue failed: {:?}", r.error);

        // a coordination tick assigns it
        let made = c.tick(now_ms());
        assert_eq!(made.len(), 1);

        let r = fs.execute(json!({ "action": "status" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["online"], 1);
    }

    #[tokio::test]
    async fn queue_missing_fields_is_soft_error() {
        let r = FleetTool::new(coord()).execute(json!({ "id": "t" })).await.unwrap();
        assert!(!r.success);
    }
}

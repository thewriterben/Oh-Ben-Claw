//! Mission tools — start a mission (gated) and observe/abort it (safe).
//!
//! `mission` commits the platform to a deliberative sequence (it will drive,
//! speak, act), so it is physical/high-blast and approval-gated. `mission_status`
//! only observes or **aborts** (always-safe stop), so it is `safe` — and abort
//! halts navigation immediately.

use crate::mission::{Mission, MissionRunner};
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Named library of missions (from config).
pub type MissionLibrary = HashMap<String, Mission>;

/// Tool: start a named mission (begins autonomous, guarded execution).
pub struct MissionStartTool {
    runner: Arc<MissionRunner>,
    library: Arc<MissionLibrary>,
}

impl MissionStartTool {
    pub fn new(runner: Arc<MissionRunner>, library: Arc<MissionLibrary>) -> Self {
        Self { runner, library }
    }
}

#[async_trait]
impl Tool for MissionStartTool {
    fn name(&self) -> &str {
        "mission"
    }

    fn description(&self) -> &str {
        "Start a named, pre-defined mission — a guarded sequence of steps the \
         platform executes autonomously (navigate, speak, wait, await state). \
         Provide `name`. The mission drives/acts and is preempted by its guards \
         (e.g. abort on battery critical). Physical action — approval-gated. Use \
         mission_status to watch or stop it."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name"],
            "properties": { "name": { "type": "string", "description": "Mission id to run." } }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::physical(true, BlastRadius::High)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");
        match self.library.get(name) {
            Some(mission) => {
                self.runner.start(mission.clone());
                Ok(ToolResult::ok(
                    json!({ "started": name, "steps": mission.steps.len() }).to_string(),
                ))
            }
            None => {
                let mut available: Vec<&str> = self.library.keys().map(String::as_str).collect();
                available.sort();
                Ok(ToolResult::err(format!(
                    "unknown mission '{name}'; available: {}",
                    available.join(", ")
                )))
            }
        }
    }
}

/// Tool: observe or abort the active mission — always safe.
pub struct MissionStatusTool {
    runner: Arc<MissionRunner>,
    library: Arc<MissionLibrary>,
}

impl MissionStatusTool {
    pub fn new(runner: Arc<MissionRunner>, library: Arc<MissionLibrary>) -> Self {
        Self { runner, library }
    }
}

#[async_trait]
impl Tool for MissionStatusTool {
    fn name(&self) -> &str {
        "mission_status"
    }

    fn description(&self) -> &str {
        "Observe or stop missions. Set `action` to 'status' (the active mission's \
         state), 'abort' (stop the mission and halt the platform), or 'list' (the \
         available mission names). Non-actuating except the always-safe abort — \
         no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["status", "abort", "list"] }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        match args.get("action").and_then(Value::as_str).unwrap_or("") {
            "status" => Ok(ToolResult::ok(
                serde_json::to_string(&self.runner.status()).unwrap_or_else(|_| "{}".to_string()),
            )),
            "abort" => {
                self.runner.abort("operator abort via mission_status").await;
                Ok(ToolResult::ok(json!({ "status": "aborted" }).to_string()))
            }
            "list" => {
                let mut names: Vec<&str> = self.library.keys().map(String::as_str).collect();
                names.sort();
                Ok(ToolResult::ok(json!({ "missions": names }).to_string()))
            }
            other => Ok(ToolResult::err(format!("unknown action: '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::world::WorldMemory;
    use crate::mission::{Mission, MissionStep};

    fn setup() -> (Arc<MissionRunner>, Arc<MissionLibrary>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let runner = Arc::new(MissionRunner::new(world));
        let mut lib = MissionLibrary::new();
        lib.insert(
            "greet".to_string(),
            Mission {
                id: "greet".into(),
                steps: vec![MissionStep::Record { entity: "m".into(), value: json!(1) }],
                guards: vec![],
            },
        );
        (runner, Arc::new(lib))
    }

    #[test]
    fn start_gated_status_safe() {
        let (r, l) = setup();
        assert!(MissionStartTool::new(Arc::clone(&r), Arc::clone(&l))
            .risk_class()
            .requires_per_call_approval());
        assert!(!MissionStatusTool::new(r, l).risk_class().physical);
    }

    #[tokio::test]
    async fn start_known_mission_then_status() {
        let (r, l) = setup();
        let start = MissionStartTool::new(Arc::clone(&r), Arc::clone(&l));
        let st = MissionStatusTool::new(Arc::clone(&r), Arc::clone(&l));

        let res = start.execute(json!({ "name": "greet" })).await.unwrap();
        assert!(res.success, "start failed: {:?}", res.error);
        assert!(r.is_running());

        let res = st.execute(json!({ "action": "status" })).await.unwrap();
        assert!(res.output.contains("running") || res.output.contains("completed"));
    }

    #[tokio::test]
    async fn unknown_mission_is_soft_error_listing_available() {
        let (r, l) = setup();
        let start = MissionStartTool::new(r, l);
        let res = start.execute(json!({ "name": "nope" })).await.unwrap();
        assert!(!res.success);
        assert!(res.error.unwrap().contains("greet"));
    }

    #[tokio::test]
    async fn list_and_abort() {
        let (r, l) = setup();
        let st = MissionStatusTool::new(r, l);
        let res = st.execute(json!({ "action": "list" })).await.unwrap();
        assert!(res.output.contains("greet"));
        let res = st.execute(json!({ "action": "abort" })).await.unwrap();
        assert!(res.success);
    }
}

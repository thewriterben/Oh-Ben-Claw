//! Mission tools — start a mission (gated) and observe/abort it (safe).
//!
//! `mission` commits the platform to a deliberative sequence (it will drive,
//! speak, act), so it is physical/high-blast and approval-gated. `mission_status`
//! only observes or **aborts** (always-safe stop), so it is `safe` — and abort
//! halts navigation immediately.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_memory::world::WorldMemory;
use obc_mission::teach::{draft_from_world, valid_id, DraftStore, TeachSettings};
use obc_mission::{Mission, MissionRunner};
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

/// World-memory entity holding the teach recorder's state, so a GUI or T-Deck
/// can show "recording" and a restart mid-demonstration loses nothing.
pub const TEACH_ENTITY: &str = "mission.teach";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tool: record a teleoperated run and turn it into a quarantined draft mission.
///
/// Safe: it reads the pose trail and writes a draft file, nothing else. It has
/// no approve action — a draft only reaches the library through the operator's
/// `mission approve`, so the agent cannot teach itself a mission and run it.
pub struct MissionTeachTool {
    world: Arc<WorldMemory>,
    drafts: Arc<DraftStore>,
    settings: TeachSettings,
}

impl MissionTeachTool {
    pub fn new(world: Arc<WorldMemory>, drafts: Arc<DraftStore>, settings: TeachSettings) -> Self {
        Self {
            world,
            drafts,
            settings,
        }
    }

    fn draft(&self, id: &str, from_ms: u64, to_ms: u64) -> ToolResult {
        let draft =
            match draft_from_world(&self.world, id, from_ms, to_ms, &self.settings, now_ms()) {
                Ok(d) => d,
                Err(e) => return ToolResult::err(e.to_string()),
            };
        match self.drafts.save(&draft) {
            Ok(_) => ToolResult::ok(
                json!({
                    "draft": id,
                    "status": "pending",
                    "steps": draft.mission.steps,
                    "path_m": draft.taught.path_m,
                    "next": format!("an operator reviews it and runs `oh-ben-claw mission approve {id}`; it is not startable until then"),
                })
                .to_string(),
            ),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }
}

#[async_trait]
impl Tool for MissionTeachTool {
    fn name(&self) -> &str {
        "mission_teach"
    }

    fn description(&self) -> &str {
        "Teach a mission by demonstration. 'start' (with `id`) begins recording \
         while the operator drives the platform by hand; 'stop' turns the drive \
         into a draft mission of waypoints and waits. 'draft' (with `id`, \
         `from_ms`, optional `to_ms`) does the same for a past run already in \
         world memory. 'list' shows drafts. Drafts are quarantined: they cannot \
         be started until an operator approves them. Does not move the platform."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["start", "stop", "draft", "list"] },
                "id": { "type": "string", "description": "Mission id: a-z, 0-9, _ or -." },
                "from_ms": { "type": "integer", "description": "Window start (ms since epoch), for 'draft'." },
                "to_ms": { "type": "integer", "description": "Window end (ms since epoch), for 'draft'; default now." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let id = args.get("id").and_then(Value::as_str).unwrap_or("");
        let recording = self
            .world
            .current(TEACH_ENTITY)?
            .filter(|f| f.value["state"] == "recording");
        match args.get("action").and_then(Value::as_str).unwrap_or("") {
            "start" => {
                if let Some(f) = recording {
                    return Ok(ToolResult::err(format!(
                        "already recording '{}'; stop it first",
                        f.value["id"].as_str().unwrap_or("")
                    )));
                }
                if !valid_id(id) {
                    return Ok(ToolResult::err(format!(
                        "mission id '{id}' must be 1-64 chars of a-z, 0-9, _ or -"
                    )));
                }
                let now = now_ms();
                self.world.observe(
                    TEACH_ENTITY,
                    json!({ "state": "recording", "id": id, "from_ms": now }),
                    now,
                    now,
                    "mission_teach",
                )?;
                Ok(ToolResult::ok(
                    json!({ "recording": id, "from_ms": now }).to_string(),
                ))
            }
            "stop" => {
                let Some(f) = recording else {
                    return Ok(ToolResult::err("not recording; use 'start' first"));
                };
                let id = f.value["id"].as_str().unwrap_or("").to_string();
                let from = f.value["from_ms"].as_u64().unwrap_or(f.valid_from);
                let now = now_ms();
                self.world.observe(
                    TEACH_ENTITY,
                    json!({ "state": "idle", "last": id }),
                    now,
                    now,
                    "mission_teach",
                )?;
                Ok(self.draft(&id, from, now))
            }
            "draft" => {
                let Some(from) = args.get("from_ms").and_then(Value::as_u64) else {
                    return Ok(ToolResult::err("'draft' needs `from_ms`"));
                };
                let to = args
                    .get("to_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(now_ms);
                Ok(self.draft(id, from, to))
            }
            "list" => {
                let drafts: Vec<Value> = self
                    .drafts
                    .list()?
                    .into_iter()
                    .map(|d| {
                        json!({
                            "id": d.mission.id,
                            "status": d.status,
                            "steps": d.mission.steps.len(),
                            "path_m": d.taught.path_m,
                        })
                    })
                    .collect();
                Ok(ToolResult::ok(json!({ "drafts": drafts }).to_string()))
            }
            other => Ok(ToolResult::err(format!("unknown action: '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_mission::{Mission, MissionStep};

    fn setup() -> (Arc<MissionRunner>, Arc<MissionLibrary>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let runner = Arc::new(MissionRunner::new(world));
        let mut lib = MissionLibrary::new();
        lib.insert(
            "greet".to_string(),
            Mission {
                id: "greet".into(),
                steps: vec![MissionStep::Record {
                    entity: "m".into(),
                    value: json!(1),
                }],
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

    fn teach_tool(tag: &str) -> (MissionTeachTool, Arc<WorldMemory>, Arc<DraftStore>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let dir = std::env::temp_dir().join(format!("obc-teach-tool-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let drafts = Arc::new(DraftStore::new(dir));
        let tool = MissionTeachTool::new(
            Arc::clone(&world),
            Arc::clone(&drafts),
            TeachSettings::default(),
        );
        (tool, world, drafts)
    }

    #[tokio::test]
    async fn teach_drafts_a_past_run_without_releasing_it() {
        let (tool, world, drafts) = teach_tool("draft");
        assert!(!tool.risk_class().physical);
        for (t, x) in [(1_000u64, 0.0), (2_000, 1.0), (3_000, 2.0)] {
            world
                .observe("sensor.pos_x", json!({ "value": x }), t, t, "odom")
                .unwrap();
            world
                .observe("sensor.pos_y", json!({ "value": 0.0 }), t, t, "odom")
                .unwrap();
        }
        let res = tool
            .execute(json!({ "action": "draft", "id": "hall", "from_ms": 1_000, "to_ms": 3_000 }))
            .await
            .unwrap();
        assert!(res.success, "draft failed: {:?}", res.error);
        assert!(res.output.contains("pending"));
        assert!(drafts.approved().unwrap().is_empty());

        // The agent has no way to approve its own draft.
        let res = tool
            .execute(json!({ "action": "approve", "id": "hall" }))
            .await
            .unwrap();
        assert!(!res.success);
        let res = tool.execute(json!({ "action": "list" })).await.unwrap();
        assert!(res.output.contains("hall"));
        let _ = std::fs::remove_dir_all(drafts.dir());
    }

    #[tokio::test]
    async fn teach_start_stop_tracks_recording_in_world_memory() {
        let (tool, world, drafts) = teach_tool("record");
        assert!(
            !tool
                .execute(json!({ "action": "stop" }))
                .await
                .unwrap()
                .success
        );
        assert!(
            !tool
                .execute(json!({ "action": "start", "id": "Bad Id" }))
                .await
                .unwrap()
                .success
        );

        let res = tool
            .execute(json!({ "action": "start", "id": "lap" }))
            .await
            .unwrap();
        assert!(res.success, "{:?}", res.error);
        assert_eq!(
            world.current(TEACH_ENTITY).unwrap().unwrap().value["state"],
            "recording"
        );
        assert!(
            !tool
                .execute(json!({ "action": "start", "id": "lap2" }))
                .await
                .unwrap()
                .success
        );

        // Nothing drove, so no draft — but recording still ends.
        let res = tool.execute(json!({ "action": "stop" })).await.unwrap();
        assert!(!res.success);
        assert_eq!(
            world.current(TEACH_ENTITY).unwrap().unwrap().value["state"],
            "idle"
        );
        let _ = std::fs::remove_dir_all(drafts.dir());
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

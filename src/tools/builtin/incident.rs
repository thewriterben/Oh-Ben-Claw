//! Incident-recording tool — the typed way for an agent to write down what it
//! concluded (Phase 18).
//!
//! Escalation playbooks tell System 2 to record what it found. Before this tool, the
//! only way to do that was raw `world_memory.observe`, which meant improvising both the
//! entity name and the payload shape at runtime. A bench audit (2026-07-17) caught what
//! that produces: two recordings across 23 wakes, with two different naming conventions
//! (`mesh.gw-40.failure` and `mesh.escalation_status`), two different schemas, both
//! double-encoded as JSON strings rather than objects, one carrying a fabricated 2023
//! timestamp in a system that uses epoch millis — and one of them landing in the mesh
//! namespace, where the supervisor read it back as a live node and alarmed on it for 100
//! minutes.
//!
//! Nothing was wrong with the model's judgement; the note it wrote was accurate. The
//! interface was wrong: it asked for a decision about *storage* from something that only
//! had an opinion about *the world*. So this tool owns the entity name, the schema, and
//! the provenance, and the agent supplies only semantics.
//!
//! Facts land at `incident.<subject>`, which is deliberately outside `mesh.*` and any
//! other perception namespace — an agent's conclusion is never mistaken for a reading.

use crate::memory::world::{Origin, WorldMemory};
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Provenance stamped on incidents. An incident is something the agent *concluded*, so
/// it is recorded as an assertion regardless of how confident the agent is.
pub const INCIDENT_SOURCE: &str = "agent";

/// Entity namespace for agent-recorded incidents.
pub const INCIDENT_PREFIX: &str = "incident";

/// Recognised incident statuses. A closed set so the record stays queryable — an agent
/// inventing `"critical"`, `"presumed_lost"`, and `"degraded_maybe"` across three wakes
/// is how a log becomes unreadable.
pub(crate) const STATUSES: [&str; 5] =
    ["investigating", "confirmed", "resolved", "unresolved", "false_alarm"];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tool: record an incident the agent has concluded something about.
pub struct RecordIncidentTool {
    mem: Arc<WorldMemory>,
}

impl RecordIncidentTool {
    /// Build a tool over a shared world-memory store.
    pub fn new(mem: Arc<WorldMemory>) -> Self {
        Self { mem }
    }
}

#[async_trait]
impl Tool for RecordIncidentTool {
    fn name(&self) -> &str {
        "record_incident"
    }

    fn description(&self) -> &str {
        "Record what you concluded about a problem — use this whenever a playbook says \
         to note, record, or write down a finding. Give the subject (what the incident \
         is about, e.g. a node id), a status, and the evidence you based it on. The \
         entity name, schema, timestamp and provenance are handled for you; do not use \
         world_memory to record findings by hand."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "What the incident is about — a node id, device, or subsystem (e.g. 'obc-esp32-s3-001')."
                },
                "status": {
                    "type": "string",
                    "enum": STATUSES,
                    "description": "investigating = looking into it; confirmed = the problem is real; resolved = it recovered or was fixed; unresolved = real and not fixed, needs a human; false_alarm = the signal was wrong."
                },
                "detail": {
                    "type": "string",
                    "description": "One or two sentences on what you concluded and why."
                },
                "evidence": {
                    "description": "What you based it on — tool results, readings, or observations. Any JSON. Record only what you actually saw; do not fill in values you did not observe."
                }
            },
            "required": ["subject", "status"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(subject) = args.get("subject").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("'record_incident' requires 'subject'"));
        };
        if subject.is_empty() {
            return Ok(ToolResult::err("'subject' must not be empty"));
        }
        // The subject becomes part of the entity key, so a dotted subject would forge
        // extra namespace levels (`incident.a.b` reads as a sub-fact of `incident.a`).
        if subject.contains('.') {
            return Ok(ToolResult::err(
                "'subject' must not contain '.' — it is one name (e.g. a node id), not a path",
            ));
        }

        let Some(status) = args.get("status").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("'record_incident' requires 'status'"));
        };
        if !STATUSES.contains(&status) {
            return Ok(ToolResult::err(format!(
                "unknown status '{status}' — use one of: {}",
                STATUSES.join(", ")
            )));
        }

        let now = now_ms();
        // Framework-owned schema. `observed_at` is stamped from the clock rather than
        // accepted, so a recorded time is always a real one.
        let mut value = json!({
            "subject": subject,
            "status": status,
            "observed_at": now,
        });
        if let Some(detail) = args.get("detail").and_then(|v| v.as_str()) {
            value["detail"] = json!(detail);
        }
        if let Some(evidence) = args.get("evidence") {
            if !evidence.is_null() {
                value["evidence"] = evidence.clone();
            }
        }

        let entity = format!("{INCIDENT_PREFIX}.{subject}");
        // An incident is something the agent *concluded*. Asserted by construction,
        // however confident it is — that is what keeps it out of the evidence pool.
        let fact = self
            .mem
            .observe_as(&entity, value, now, now, INCIDENT_SOURCE, Origin::Asserted)?;
        Ok(ToolResult::ok(serde_json::to_string(&fact)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> RecordIncidentTool {
        RecordIncidentTool::new(Arc::new(WorldMemory::open_in_memory().unwrap()))
    }

    #[tokio::test]
    async fn records_under_a_framework_owned_entity_and_schema() {
        let t = tool();
        let r = t
            .execute(json!({
                "subject": "obc-esp32-s3-001",
                "status": "unresolved",
                "detail": "No reply to two capability pings.",
                "evidence": { "rssi_dbm": -57, "age_s": 258 }
            }))
            .await
            .unwrap();
        assert!(r.success, "{:?}", r.error);
        // The agent chose none of these.
        assert!(r.output.contains(r#""entity":"incident.obc-esp32-s3-001""#), "{}", r.output);
        assert!(r.output.contains(r#""source":"agent""#));
        assert!(r.output.contains(r#""observed_at""#));
        assert!(r.output.contains(r#""status\":\"unresolved"#) || r.output.contains("unresolved"));
    }

    #[tokio::test]
    async fn the_value_is_an_object_not_a_json_string() {
        // The audited freehand writes were double-encoded — a JSON string containing
        // JSON — so `value.get("status")` returned None. Building the value here means
        // that cannot happen.
        let t = tool();
        t.execute(json!({"subject": "n1", "status": "confirmed"}))
            .await
            .unwrap();
        let fact = t.mem.current("incident.n1").unwrap().unwrap();
        assert!(fact.value.is_object(), "value must be an object: {}", fact.value);
        assert_eq!(fact.value.get("status").and_then(|v| v.as_str()), Some("confirmed"));
        assert_eq!(fact.value.get("subject").and_then(|v| v.as_str()), Some("n1"));
    }

    #[tokio::test]
    async fn an_unknown_status_is_refused_with_the_allowed_set() {
        let t = tool();
        let r = t
            .execute(json!({"subject": "n1", "status": "critical"}))
            .await
            .unwrap();
        assert!(!r.success, "a free-form status would make the log unqueryable");
        let why = r.error.unwrap_or_default();
        assert!(why.contains("investigating"), "names the allowed set: {why}");
    }

    #[tokio::test]
    async fn a_dotted_subject_cannot_forge_namespace_levels() {
        let t = tool();
        let r = t
            .execute(json!({"subject": "gw-40.failure", "status": "confirmed"}))
            .await
            .unwrap();
        assert!(!r.success, "a dotted subject would forge a sub-fact level");
    }

    #[tokio::test]
    async fn subject_and_status_are_required() {
        let t = tool();
        assert!(!t.execute(json!({"status": "confirmed"})).await.unwrap().success);
        assert!(!t.execute(json!({"subject": "n1"})).await.unwrap().success);
    }

    #[tokio::test]
    async fn successive_records_keep_a_queryable_history() {
        // The point of a fixed schema: the trail is readable afterwards.
        let t = tool();
        t.execute(json!({"subject": "n1", "status": "investigating"})).await.unwrap();
        t.execute(json!({"subject": "n1", "status": "confirmed"})).await.unwrap();
        t.execute(json!({"subject": "n1", "status": "resolved"})).await.unwrap();
        let trail: Vec<String> = t
            .mem
            .history("incident.n1")
            .unwrap()
            .into_iter()
            .filter_map(|f| f.value.get("status").and_then(|s| s.as_str()).map(str::to_string))
            .collect();
        assert_eq!(trail, vec!["investigating", "confirmed", "resolved"]);
    }
}

//! Learn tool — mine, review, and approve self-authored reflex rules.
//!
//! Surfaces the experiential rule-synthesis loop to the agent/operator: `mine`
//! runs the miner over world-memory history; `list` shows proposals with support
//! and confidence; `approve`/`reject` are the **gate** — approving activates a
//! conservative (escalate-only) foresight rule into the live engine. Read/review
//! only — `RiskClass::safe`. (The activated rule still only escalates, so nothing
//! physical happens without separate approval.)

use crate::learning::{OutcomeSpec, ProposalStore, RuleMiner};
use crate::memory::world::WorldMemory;
use crate::tools::traits::{RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: mine and review learned rule proposals.
pub struct LearnTool {
    world: Arc<WorldMemory>,
    store: Arc<ProposalStore>,
    miner: RuleMiner,
    outcome: OutcomeSpec,
}

impl LearnTool {
    pub fn new(
        world: Arc<WorldMemory>,
        store: Arc<ProposalStore>,
        miner: RuleMiner,
        outcome: OutcomeSpec,
    ) -> Self {
        Self { world, store, miner, outcome }
    }
}

#[async_trait]
impl Tool for LearnTool {
    fn name(&self) -> &str {
        "learn"
    }

    fn description(&self) -> &str {
        "Mine and review self-authored predictive rules. Set `action` to: 'mine' \
         (scan world-memory history for conditions that preceded the configured \
         bad outcome and add proposals), 'list' (proposals with support + \
         confidence), 'approve' (activate a proposal into the live foresight \
         engine — by `id`), or 'reject' (by `id`). Approved rules only escalate, \
         so review is safe. No approval needed to use this tool."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["mine", "list", "approve", "reject"] },
                "id": { "type": "string", "description": "Proposal id (approve/reject)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "mine" => match self.miner.mine(&self.world, &self.outcome) {
                Ok(proposals) => {
                    let added = self.store.ingest(proposals);
                    Ok(ToolResult::ok(
                        json!({ "mined": added, "pending": self.store.list().len() }).to_string(),
                    ))
                }
                Err(e) => Ok(ToolResult::err(e.to_string())),
            },
            "list" => Ok(ToolResult::ok(
                json!({ "proposals": self.store.list(), "active": self.store.active_count() })
                    .to_string(),
            )),
            "approve" => {
                let Some(id) = args.get("id").and_then(Value::as_str) else {
                    return Ok(ToolResult::err("'approve' requires 'id'"));
                };
                if self.store.approve(id) {
                    Ok(ToolResult::ok(
                        json!({ "approved": id, "active": self.store.active_count() }).to_string(),
                    ))
                } else {
                    Ok(ToolResult::err(format!("no pending proposal '{id}'")))
                }
            }
            "reject" => {
                let Some(id) = args.get("id").and_then(Value::as_str) else {
                    return Ok(ToolResult::err("'reject' requires 'id'"));
                };
                if self.store.reject(id) {
                    Ok(ToolResult::ok(json!({ "rejected": id }).to_string()))
                } else {
                    Ok(ToolResult::err(format!("no pending proposal '{id}'")))
                }
            }
            other => Ok(ToolResult::err(format!("unknown action: '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::reflex::Cmp;
    use std::sync::Mutex;

    fn obs(world: &WorldMemory, entity: &str, t: u64, v: f64) {
        world.observe(entity, json!({ "value": v }), t, t, "test").unwrap();
    }

    fn setup() -> LearnTool {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        for &(t, v) in &[(0, 0.0), (1_000, 1.0), (1_100, 0.0), (2_000, 1.0), (2_100, 0.0), (3_000, 1.0)] {
            obs(&world, "alarm", t, v);
        }
        for &(t, v) in &[
            (950, 80.0), (1_050, 10.0), (1_950, 80.0), (2_050, 10.0), (2_950, 80.0), (3_050, 10.0),
            (500, 10.0), (1_500, 10.0), (2_500, 10.0),
        ] {
            obs(&world, "x", t, v);
        }
        let active = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(ProposalStore::new(active));
        let miner = RuleMiner {
            lookback_ms: 200,
            min_support: 2,
            min_confidence: 0.6,
            candidates: vec!["x".into()],
        };
        let outcome = OutcomeSpec { entity: "alarm".into(), op: Cmp::Ge, threshold: 1.0 };
        LearnTool::new(world, store, miner, outcome)
    }

    #[test]
    fn classed_safe() {
        assert!(!setup().risk_class().physical);
    }

    #[tokio::test]
    async fn mine_list_then_approve_activates() {
        let t = setup();
        let r = t.execute(json!({ "action": "mine" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert!(v["mined"].as_u64().unwrap() >= 1);

        let r = t.execute(json!({ "action": "list" })).await.unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        let id = v["proposals"][0]["rule"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["active"], 0);

        let r = t.execute(json!({ "action": "approve", "id": id })).await.unwrap();
        assert!(r.success, "approve failed: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["active"], 1);
    }

    #[tokio::test]
    async fn approve_unknown_is_soft_error() {
        let t = setup();
        let r = t.execute(json!({ "action": "approve", "id": "nope" })).await.unwrap();
        assert!(!r.success);
    }
}

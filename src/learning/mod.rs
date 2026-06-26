//! Self-authored reflexes — experiential rule synthesis from world memory.
//!
//! The reflex and foresight layers are hand-authored: an operator decides what to
//! watch. This module lets the system **author its own** rules from experience.
//! It mines the bitemporal history for *antecedents* — conditions that
//! repeatedly preceded a "bad outcome" — and proposes anticipatory rules with a
//! support count and a confidence (specificity vs. the background). Proposals are
//! never activated automatically: a human (or policy) **approves** them, and only
//! then are they pushed into the live foresight engine's shared rule buffer.
//!
//! This closes the loop with Foresight: the system not only acts on predictions,
//! it learns *what to predict* — gated by approval so a wrong correlation can
//! never silently start driving behavior.

use crate::agent::reflex::{Action, Cmp};
use crate::foresight::ForesightRule;
use crate::memory::world::WorldMemory;
use serde::Serialize;
use std::sync::{Arc, Mutex};

fn value_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Object(o) => o.get("value").and_then(value_to_f64),
        _ => None,
    }
}

fn op_str(op: Cmp) -> &'static str {
    match op {
        Cmp::Gt => ">",
        Cmp::Ge => ">=",
        Cmp::Lt => "<",
        Cmp::Le => "<=",
        Cmp::Eq => "==",
        Cmp::Ne => "!=",
    }
}

/// Defines a "bad outcome" event: when `entity` (numeric) becomes `op threshold`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeSpec {
    pub entity: String,
    pub op: Cmp,
    pub threshold: f64,
}

/// A mined rule proposal: an antecedent that preceded the outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedRule {
    /// Stable id (for approve/reject).
    pub id: String,
    pub entity: String,
    pub op: Cmp,
    pub threshold: f64,
    /// How many outcome events this antecedent preceded.
    pub support: usize,
    /// support / (support + background false-positives) — specificity.
    pub confidence: f64,
    /// Human-readable rationale.
    pub reason: String,
}

impl ProposedRule {
    /// Turn the proposal into a (conservative, escalate-only) foresight rule.
    pub fn to_foresight_rule(&self, horizon_ms: u64, debounce_ms: u64) -> ForesightRule {
        ForesightRule {
            id: self.id.clone(),
            entity: self.entity.clone(),
            op: self.op,
            threshold: self.threshold,
            horizon_ms,
            then: Action::Escalate {
                reason: format!("learned: {}", self.reason),
            },
            debounce_ms,
        }
    }
}

/// Mines antecedent rules from world-memory history.
#[derive(Debug, Clone)]
pub struct RuleMiner {
    /// How far before an outcome event to look for the antecedent value (ms).
    pub lookback_ms: u64,
    /// Minimum outcome events an antecedent must precede to be proposed.
    pub min_support: usize,
    /// Minimum specificity to propose.
    pub min_confidence: f64,
    /// Candidate entities to test as antecedents.
    pub candidates: Vec<String>,
}

impl Default for RuleMiner {
    fn default() -> Self {
        Self { lookback_ms: 5_000, min_support: 2, min_confidence: 0.6, candidates: Vec::new() }
    }
}

impl RuleMiner {
    /// Find the timestamps where `outcome` *becomes* true (false→true transitions).
    fn outcome_events(&self, world: &WorldMemory, outcome: &OutcomeSpec) -> anyhow::Result<Vec<u64>> {
        let hist = world.history(&outcome.entity)?;
        let mut events = Vec::new();
        let mut prev_bad = false;
        for f in &hist {
            let bad = value_to_f64(&f.value).is_some_and(|x| outcome.op.test(x, outcome.threshold));
            if bad && !prev_bad {
                events.push(f.valid_from);
            }
            prev_bad = bad;
        }
        Ok(events)
    }

    /// Mine antecedent rule proposals for the given outcome.
    pub fn mine(&self, world: &WorldMemory, outcome: &OutcomeSpec) -> anyhow::Result<Vec<ProposedRule>> {
        let events = self.outcome_events(world, outcome)?;
        if events.len() < self.min_support {
            return Ok(Vec::new());
        }
        let mut proposals = Vec::new();
        for c in &self.candidates {
            let hist = world.history(c)?;
            let pts: Vec<(u64, f64)> = hist
                .iter()
                .filter_map(|f| value_to_f64(&f.value).map(|v| (f.valid_from, v)))
                .collect();
            if pts.is_empty() {
                continue;
            }
            // The candidate's value just before each outcome event.
            let mut pre = Vec::new();
            for &te in &events {
                if let Some((_, v)) = pts
                    .iter()
                    .rev()
                    .find(|(t, _)| *t <= te && te.saturating_sub(*t) <= self.lookback_ms)
                {
                    pre.push(*v);
                }
            }
            if pre.len() < self.min_support {
                continue;
            }
            let gmean = pts.iter().map(|(_, v)| v).sum::<f64>() / pts.len() as f64;
            let pmean = pre.iter().sum::<f64>() / pre.len() as f64;
            // Direction: high-before ⇒ "≥ min(pre)"; low-before ⇒ "≤ max(pre)".
            let (op, threshold) = if pmean >= gmean {
                (Cmp::Ge, pre.iter().cloned().fold(f64::INFINITY, f64::min))
            } else {
                (Cmp::Le, pre.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
            };
            // Specificity: how often the condition holds away from any event.
            let mut bg_pos = 0usize;
            for (t, v) in &pts {
                let near = events
                    .iter()
                    .any(|te| *t <= *te && te.saturating_sub(*t) <= self.lookback_ms);
                if near {
                    continue;
                }
                if op.test(*v, threshold) {
                    bg_pos += 1;
                }
            }
            let support = pre.len();
            let confidence = support as f64 / (support + bg_pos) as f64;
            if confidence < self.min_confidence {
                continue;
            }
            let reason = format!(
                "{c} {} {threshold:.2} preceded {} {} {} in {support}/{} events (confidence {confidence:.2})",
                op_str(op),
                outcome.entity,
                op_str(outcome.op),
                outcome.threshold,
                events.len()
            );
            proposals.push(ProposedRule {
                id: format!("learned:{c}:{}:{threshold:.2}", op_str(op)),
                entity: c.clone(),
                op,
                threshold,
                support,
                confidence,
                reason,
            });
        }
        Ok(proposals)
    }
}

/// Review status of a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
}

/// A proposal with its review status.
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub rule: ProposedRule,
    pub status: ProposalStatus,
}

/// Holds mined proposals and the **approval gate**. Approving a proposal pushes a
/// (conservative, escalate-only) foresight rule into the shared live buffer.
pub struct ProposalStore {
    pending: Mutex<Vec<Proposal>>,
    active: Arc<Mutex<Vec<ForesightRule>>>,
    horizon_ms: u64,
    debounce_ms: u64,
}

impl ProposalStore {
    /// A store writing approved rules into `active` (the foresight engine's buffer).
    pub fn new(active: Arc<Mutex<Vec<ForesightRule>>>) -> Self {
        Self { pending: Mutex::new(Vec::new()), active, horizon_ms: 60_000, debounce_ms: 30_000 }
    }

    /// Set the horizon/debounce applied to approved learned rules.
    pub fn with_params(mut self, horizon_ms: u64, debounce_ms: u64) -> Self {
        self.horizon_ms = horizon_ms;
        self.debounce_ms = debounce_ms;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Proposal>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Add newly mined proposals (deduped by id; existing reviews are kept).
    pub fn ingest(&self, mined: Vec<ProposedRule>) -> usize {
        let mut guard = self.lock();
        let mut added = 0;
        for rule in mined {
            if !guard.iter().any(|p| p.rule.id == rule.id) {
                guard.push(Proposal { rule, status: ProposalStatus::Pending });
                added += 1;
            }
        }
        added
    }

    /// All proposals (clone).
    pub fn list(&self) -> Vec<Proposal> {
        self.lock().clone()
    }

    /// Approve a proposal by id: mark approved and activate it into the live
    /// foresight buffer. Returns `false` if not found or not pending.
    pub fn approve(&self, id: &str) -> bool {
        let mut guard = self.lock();
        let Some(p) = guard.iter_mut().find(|p| p.rule.id == id) else {
            return false;
        };
        if p.status != ProposalStatus::Pending {
            return false;
        }
        p.status = ProposalStatus::Approved;
        let rule = p.rule.to_foresight_rule(self.horizon_ms, self.debounce_ms);
        let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
        if !active.iter().any(|r| r.id == rule.id) {
            active.push(rule);
        }
        true
    }

    /// Reject a proposal by id.
    pub fn reject(&self, id: &str) -> bool {
        let mut guard = self.lock();
        match guard.iter_mut().find(|p| p.rule.id == id) {
            Some(p) if p.status == ProposalStatus::Pending => {
                p.status = ProposalStatus::Rejected;
                true
            }
            _ => false,
        }
    }

    /// Number of active (approved) learned rules.
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obs(world: &WorldMemory, entity: &str, t: u64, v: f64) {
        world.observe(entity, json!({ "value": v }), t, t, "test").unwrap();
    }

    /// History where `x` spikes high just before each `alarm` event, and `y` is
    /// flat (a non-antecedent that must be filtered out).
    fn scenario() -> Arc<WorldMemory> {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        // alarm: 0→1 at t=1000, 2000, 3000
        for &(t, v) in &[(0, 0.0), (1_000, 1.0), (1_100, 0.0), (2_000, 1.0), (2_100, 0.0), (3_000, 1.0)] {
            obs(&world, "alarm", t, v);
        }
        // x: high (80) just before each event, low (10) otherwise
        for &(t, v) in &[
            (500, 10.0), (950, 80.0), (1_050, 10.0),
            (1_500, 10.0), (1_950, 80.0), (2_050, 10.0),
            (2_500, 10.0), (2_950, 80.0), (3_050, 10.0),
        ] {
            obs(&world, "x", t, v);
        }
        // y: constant — no correlation
        for t in [500, 1_000, 1_500, 2_000, 2_500, 3_000] {
            obs(&world, "y", t, 10.0);
        }
        world
    }

    fn miner() -> RuleMiner {
        RuleMiner {
            lookback_ms: 200,
            min_support: 2,
            min_confidence: 0.6,
            candidates: vec!["x".into(), "y".into()],
        }
    }

    fn outcome() -> OutcomeSpec {
        OutcomeSpec { entity: "alarm".into(), op: Cmp::Ge, threshold: 1.0 }
    }

    #[test]
    fn mines_the_antecedent_and_filters_noise() {
        let world = scenario();
        let proposals = miner().mine(&world, &outcome()).unwrap();
        // x is proposed (high before the alarm); y is not (flat → low specificity)
        assert!(proposals.iter().any(|p| p.entity == "x" && p.op == Cmp::Ge && (p.threshold - 80.0).abs() < 1e-6));
        assert!(!proposals.iter().any(|p| p.entity == "y"));
        let xp = proposals.iter().find(|p| p.entity == "x").unwrap();
        assert_eq!(xp.support, 3);
        assert!(xp.confidence >= 0.6);
    }

    #[test]
    fn too_few_events_proposes_nothing() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        obs(&world, "alarm", 1_000, 1.0); // a single event
        obs(&world, "x", 950, 80.0);
        assert!(miner().mine(&world, &outcome()).unwrap().is_empty());
    }

    #[test]
    fn approval_gate_activates_rule_into_buffer() {
        let world = scenario();
        let active = Arc::new(Mutex::new(Vec::new()));
        let store = ProposalStore::new(Arc::clone(&active));
        let mined = miner().mine(&world, &outcome()).unwrap();
        assert!(store.ingest(mined) >= 1);

        let id = store.list().into_iter().find(|p| p.rule.entity == "x").unwrap().rule.id;
        // pending until approved — nothing active yet
        assert_eq!(store.active_count(), 0);
        assert!(store.approve(&id));
        // now a live foresight rule exists
        assert_eq!(store.active_count(), 1);
        assert_eq!(active.lock().unwrap()[0].entity, "x");
        // re-approve is a no-op; reject of an approved one fails
        assert!(!store.approve(&id));
        assert!(!store.reject(&id));
    }

    #[test]
    fn reject_keeps_rule_inactive() {
        let world = scenario();
        let active = Arc::new(Mutex::new(Vec::new()));
        let store = ProposalStore::new(Arc::clone(&active));
        store.ingest(miner().mine(&world, &outcome()).unwrap());
        let id = store.list().into_iter().find(|p| p.rule.entity == "x").unwrap().rule.id;
        assert!(store.reject(&id));
        assert_eq!(store.active_count(), 0);
        assert!(matches!(
            store.list().into_iter().find(|p| p.rule.id == id).unwrap().status,
            ProposalStatus::Rejected
        ));
    }
}

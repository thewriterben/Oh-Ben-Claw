//! Human-in-the-loop approval workflow.
//!
//! The `ApprovalManager` checks whether a tool call needs explicit user approval
//! before execution, prompts the user interactively via stdin, and records all
//! decisions in an in-memory audit log.
//!
//! ## Phase 15, WS6 — scoped approvals
//!
//! Approvals now carry a [`ApprovalScope`]:
//!
//! - **Call** — this single invocation (`y`)
//! - **Session** — this tool for the rest of the session (`a`, formerly "always")
//! - **Forever** — persisted across sessions to `~/.oh-ben-claw/approval_grants.json` (`f`)
//!
//! **Plan-mode approval**: a multi-step [`ApprovedPlan`] is approved once with
//! per-step [`ArgumentBound`]s; execution then checks each call against the
//! plan and **halts on drift** (the plan is revoked on the first violation).
//!
//! **Funnel analytics**: every ask/approve/deny is counted per tool in the
//! [`ApprovalFunnel`], so policy can be tuned (which tools ask too often?).

use crate::config::{AutonomyConfig, AutonomyLevel};
use crate::security::trust::{self, TrustGate, TrustScorer};
use crate::tools::traits::RiskClass;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// A pending tool approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// The name of the tool being requested.
    pub tool_name: String,
    /// The arguments that will be passed to the tool.
    pub arguments: serde_json::Value,
}

/// How long an approval lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    /// This single invocation only.
    Call,
    /// This tool for the rest of the current session.
    Session,
    /// Persisted across sessions.
    Forever,
}

/// The user's response to an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalResponse {
    /// Allow this single invocation (scope: call).
    Yes,
    /// Deny this invocation.
    No,
    /// Allow all future invocations of this tool in the current session.
    Always,
    /// Allow this tool permanently (persisted across sessions).
    Forever,
}

impl ApprovalResponse {
    /// The scope granted by this response, if it is an approval.
    pub fn granted_scope(&self) -> Option<ApprovalScope> {
        match self {
            Self::Yes => Some(ApprovalScope::Call),
            Self::Always => Some(ApprovalScope::Session),
            Self::Forever => Some(ApprovalScope::Forever),
            Self::No => None,
        }
    }
}

/// A single entry in the approval audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalLogEntry {
    /// ISO-8601 timestamp of the decision.
    pub timestamp: String,
    /// Name of the tool that was reviewed.
    pub tool_name: String,
    /// Truncated summary of the arguments.
    pub arguments_summary: String,
    /// The decision that was made.
    pub decision: ApprovalResponse,
    /// The channel or context in which the approval occurred.
    pub channel: String,
}

// ── Forever grants (persisted) ────────────────────────────────────────────────

/// One persisted forever-grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeverGrant {
    pub tool_name: String,
    /// ISO-8601 timestamp the grant was made.
    pub granted_at: String,
}

/// Cross-session approval grants, persisted as JSON.
#[derive(Debug)]
pub struct ForeverGrants {
    path: PathBuf,
    grants: Mutex<HashMap<String, ForeverGrant>>,
}

impl ForeverGrants {
    /// Default location: `~/.oh-ben-claw/approval_grants.json`.
    pub fn default_path() -> PathBuf {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".oh-ben-claw")
            .join("approval_grants.json")
    }

    /// Load grants from `path` (missing file ⇒ empty store).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let grants = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<ForeverGrant>>(&s).ok())
            .map(|list| {
                list.into_iter()
                    .map(|g| (g.tool_name.clone(), g))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        Self {
            path,
            grants: Mutex::new(grants),
        }
    }

    pub fn contains(&self, tool_name: &str) -> bool {
        self.grants.lock().contains_key(tool_name)
    }

    /// Grant `tool_name` forever and persist.
    pub fn grant(&self, tool_name: &str) {
        self.grants.lock().insert(
            tool_name.to_string(),
            ForeverGrant {
                tool_name: tool_name.to_string(),
                granted_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        self.save();
    }

    /// Revoke a grant and persist. Returns true if a grant existed.
    pub fn revoke(&self, tool_name: &str) -> bool {
        let removed = self.grants.lock().remove(tool_name).is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// All current grants (sorted by tool name).
    pub fn list(&self) -> Vec<ForeverGrant> {
        let mut grants: Vec<_> = self.grants.lock().values().cloned().collect();
        grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        grants
    }

    fn save(&self) {
        let list = self.list();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            if let Err(e) = std::fs::write(&self.path, json) {
                tracing::warn!(error = %e, path = ?self.path, "Failed to persist approval grants");
            }
        }
    }
}

// ── Plan-mode approval ────────────────────────────────────────────────────────

/// A constraint on one argument of a planned tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArgumentBound {
    /// The argument must equal this value exactly.
    Exact { value: serde_json::Value },
    /// The argument must be one of these values.
    OneOf { values: Vec<serde_json::Value> },
    /// The argument must be a number within [min, max] inclusive.
    Range { min: f64, max: f64 },
    /// Any value is acceptable (documents intent explicitly).
    Any,
}

impl ArgumentBound {
    /// Check a concrete value against this bound.
    pub fn allows(&self, value: &serde_json::Value) -> bool {
        match self {
            Self::Exact { value: expected } => value == expected,
            Self::OneOf { values } => values.contains(value),
            Self::Range { min, max } => value
                .as_f64()
                .map(|n| n >= *min && n <= *max)
                .unwrap_or(false),
            Self::Any => true,
        }
    }
}

/// One step of an approved plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub tool_name: String,
    /// Bounds on specific argument keys. Keys not listed here are
    /// unconstrained unless `deny_unlisted_args` is set.
    #[serde(default)]
    pub bounds: HashMap<String, ArgumentBound>,
    /// When true, the call may only contain argument keys listed in `bounds`.
    #[serde(default)]
    pub deny_unlisted_args: bool,
}

/// Why a plan-checked call was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanViolation {
    /// The call's tool does not match the next plan step.
    WrongTool { expected: String, got: String },
    /// An argument fell outside its approved bound.
    ArgOutOfBounds { key: String },
    /// The call contains an argument key the step does not allow.
    UnlistedArg { key: String },
    /// All plan steps have already been consumed.
    PlanExhausted,
    /// The plan id is unknown (never approved, or revoked after a violation).
    UnknownPlan,
}

/// A multi-step plan approved once; execution is checked step by step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedPlan {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub created_at: String,
    /// Index of the next expected step.
    pub cursor: usize,
}

impl ApprovedPlan {
    /// Check the next call against the plan, advancing the cursor on success.
    fn check_next(
        &mut self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<(), PlanViolation> {
        let step = self.steps.get(self.cursor).ok_or(PlanViolation::PlanExhausted)?;

        if step.tool_name != tool_name {
            return Err(PlanViolation::WrongTool {
                expected: step.tool_name.clone(),
                got: tool_name.to_string(),
            });
        }

        let empty = serde_json::Map::new();
        let args = arguments.as_object().unwrap_or(&empty);

        if step.deny_unlisted_args {
            for key in args.keys() {
                if !step.bounds.contains_key(key) {
                    return Err(PlanViolation::UnlistedArg { key: key.clone() });
                }
            }
        }
        for (key, bound) in &step.bounds {
            // A bounded key that is absent counts as out of bounds unless Any.
            match args.get(key) {
                Some(value) if bound.allows(value) => {}
                None if matches!(bound, ArgumentBound::Any) => {}
                _ => return Err(PlanViolation::ArgOutOfBounds { key: key.clone() }),
            }
        }

        self.cursor += 1;
        Ok(())
    }

    /// True when every step has been consumed.
    pub fn is_complete(&self) -> bool {
        self.cursor >= self.steps.len()
    }
}

// ── Funnel analytics ──────────────────────────────────────────────────────────

/// Per-tool approval funnel counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunnelCounters {
    pub asked: u64,
    pub approved_call: u64,
    pub approved_session: u64,
    pub approved_forever: u64,
    pub denied: u64,
    pub plan_violations: u64,
}

/// Aggregated ask/approve/deny statistics per tool.
#[derive(Debug, Default)]
pub struct ApprovalFunnel {
    counters: Mutex<HashMap<String, FunnelCounters>>,
}

impl ApprovalFunnel {
    fn record_decision(&self, tool_name: &str, decision: &ApprovalResponse) {
        let mut map = self.counters.lock();
        let c = map.entry(tool_name.to_string()).or_default();
        c.asked += 1;
        match decision {
            ApprovalResponse::Yes => c.approved_call += 1,
            ApprovalResponse::Always => c.approved_session += 1,
            ApprovalResponse::Forever => c.approved_forever += 1,
            ApprovalResponse::No => c.denied += 1,
        }
    }

    fn record_plan_violation(&self, tool_name: &str) {
        let mut map = self.counters.lock();
        map.entry(tool_name.to_string()).or_default().plan_violations += 1;
    }

    /// Snapshot of all counters, sorted by ask count descending.
    pub fn summary(&self) -> Vec<(String, FunnelCounters)> {
        let mut rows: Vec<_> = self
            .counters
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1.asked));
        rows
    }
}

// ── Approval Manager ──────────────────────────────────────────────────────────

/// Manages the human-in-the-loop approval flow for tool execution.
/// The outcome of a trust-aware approval check ([`ApprovalManager::decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Proceed without prompting.
    Allow,
    /// Prompt the operator before executing.
    NeedsApproval,
    /// Refuse outright — the node is too untrusted for this physical action.
    Deny,
}

pub struct ApprovalManager {
    config: AutonomyConfig,
    /// Tools approved with `Always` during this session.
    session_allowlist: Arc<Mutex<HashSet<String>>>,
    /// Cross-session forever grants (persisted).
    forever_grants: Arc<ForeverGrants>,
    /// Approved multi-step plans, by plan id. Revoked on first violation.
    plans: Arc<Mutex<HashMap<String, ApprovedPlan>>>,
    /// Audit log of all approval decisions this session.
    audit_log: Arc<Mutex<Vec<ApprovalLogEntry>>>,
    /// Ask/approve/deny statistics per tool.
    funnel: Arc<ApprovalFunnel>,
    /// Optional observability context (WS5): counts approval asks centrally.
    obs: Option<Arc<crate::observability::ObsContext>>,
    /// When true, all approval requests are auto-denied (non-interactive mode).
    non_interactive: bool,
    /// Optional behavioral trust scorer: when set, an anomalous node loses its
    /// auto/forever shortcut for physical actions (or is denied them entirely).
    trust: Option<Arc<TrustScorer>>,
}

impl ApprovalManager {
    /// Create an `ApprovalManager` from the given autonomy configuration.
    pub fn from_config(config: &AutonomyConfig) -> Self {
        Self::with_grants(config, ForeverGrants::load(ForeverGrants::default_path()), false)
    }

    /// Create an `ApprovalManager` for non-interactive contexts (bots, API, etc.).
    ///
    /// In non-interactive mode every request that would normally prompt the user
    /// is automatically denied so that the system never blocks waiting for input.
    pub fn for_non_interactive(config: &AutonomyConfig) -> Self {
        Self::with_grants(config, ForeverGrants::load(ForeverGrants::default_path()), true)
    }

    /// Construct with an explicit grants store (used by tests).
    pub fn with_grants(
        config: &AutonomyConfig,
        forever_grants: ForeverGrants,
        non_interactive: bool,
    ) -> Self {
        Self {
            config: config.clone(),
            session_allowlist: Arc::new(Mutex::new(HashSet::new())),
            forever_grants: Arc::new(forever_grants),
            plans: Arc::new(Mutex::new(HashMap::new())),
            audit_log: Arc::new(Mutex::new(Vec::new())),
            funnel: Arc::new(ApprovalFunnel::default()),
            obs: None,
            non_interactive,
            trust: None,
        }
    }

    /// Attach an observability context so approval asks count centrally (WS5).
    pub fn with_obs(mut self, obs: Arc<crate::observability::ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    /// Attach a behavioral [`TrustScorer`] (Track 0 dynamic trust). Once set,
    /// [`decide`](Self::decide) consults the acting node's trust level: a node on
    /// probation is forced back through approval for physical actions it could
    /// otherwise auto-run, and an untrusted node is denied them.
    pub fn with_trust(mut self, trust: Arc<TrustScorer>) -> Self {
        self.trust = Some(trust);
        self
    }

    /// Returns `true` if this tool requires explicit user approval before execution.
    pub fn needs_approval(&self, tool_name: &str) -> bool {
        // always_ask overrides everything — including forever grants.
        if self.config.always_ask.iter().any(|t| t == tool_name) {
            return true;
        }
        // auto_approve short-circuits any level check
        if self.config.auto_approve.iter().any(|t| t == tool_name) {
            return false;
        }
        // session allowlist from previous Always decisions
        if self.session_allowlist.lock().contains(tool_name) {
            return false;
        }
        // persisted forever grants
        if self.forever_grants.contains(tool_name) {
            return false;
        }
        match self.config.level {
            AutonomyLevel::Full => false,
            AutonomyLevel::Supervised => true,
            AutonomyLevel::Manual => true,
        }
    }

    /// Returns `true` only when an operator has **explicitly** granted this
    /// tool — via the `auto_approve` config list, a session "Always" grant, or
    /// a persisted forever grant. Unlike [`needs_approval`](Self::needs_approval),
    /// `Full` autonomy does NOT count as a grant: this is the gate for
    /// supervised-rollout-stage skills (Track 0), which must never run
    /// unattended merely because the autonomy level is permissive.
    pub fn explicitly_granted(&self, tool_name: &str) -> bool {
        if self.config.always_ask.iter().any(|t| t == tool_name) {
            return false;
        }
        self.config.auto_approve.iter().any(|t| t == tool_name)
            || self.session_allowlist.lock().contains(tool_name)
            || self.forever_grants.contains(tool_name)
    }

    /// Trust-aware approval check for a tool call by `node_id` with risk profile
    /// `risk`. Behavioral trust can only **tighten** the normal decision, never
    /// relax it: for a physical action, an untrusted node is `Deny`ed and a node on
    /// probation `NeedsApproval` even if the tool is auto-approved or forever-
    /// granted. Non-physical actions, a trusted node, or no scorer attached fall
    /// through to the ordinary [`needs_approval`](Self::needs_approval) rules.
    pub fn decide(&self, tool_name: &str, node_id: Option<&str>, risk: RiskClass) -> Decision {
        if risk.physical {
            if let (Some(scorer), Some(node)) = (&self.trust, node_id) {
                match trust::gate(scorer.level(node), risk) {
                    TrustGate::Deny => return Decision::Deny,
                    TrustGate::RequireApproval => return Decision::NeedsApproval,
                    TrustGate::Allow => {}
                }
            }
        }
        if self.needs_approval(tool_name) {
            Decision::NeedsApproval
        } else {
            Decision::Allow
        }
    }

    /// Prompt the user for approval of a tool call.
    ///
    /// In non-interactive mode this always returns [`ApprovalResponse::No`].
    pub fn request_approval(&self, req: &ApprovalRequest) -> ApprovalResponse {
        let args_summary = {
            let s = req.arguments.to_string();
            if s.len() > 120 {
                format!("{}…", &s[..120])
            } else {
                s
            }
        };

        let decision = if self.non_interactive {
            ApprovalResponse::No
        } else {
            println!(
                "\n⚠️  Tool approval required\n   Tool : {}\n   Args : {}\n",
                req.tool_name, args_summary
            );
            println!("Allow? [y]es (this call) / [n]o / [a] session / [f]orever: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                ApprovalResponse::No
            } else {
                match input.trim().to_lowercase().as_str() {
                    "y" | "yes" => ApprovalResponse::Yes,
                    "a" | "always" | "session" => ApprovalResponse::Always,
                    "f" | "forever" => ApprovalResponse::Forever,
                    _ => ApprovalResponse::No,
                }
            }
        };

        self.apply_decision(&req.tool_name, &decision);

        let entry = ApprovalLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_name: req.tool_name.clone(),
            arguments_summary: args_summary,
            decision: decision.clone(),
            channel: if self.non_interactive {
                "non-interactive".to_string()
            } else {
                "cli".to_string()
            },
        };
        self.audit_log.lock().push(entry);
        self.funnel.record_decision(&req.tool_name, &decision);
        if let Some(obs) = &self.obs {
            obs.record_approval_ask(&req.tool_name);
        }

        decision
    }

    /// Record an approval decision made out-of-band (e.g. via a chat channel
    /// or dashboard) so scope grants, audit, and funnel stay consistent.
    pub fn record_external_decision(&self, req: &ApprovalRequest, decision: ApprovalResponse) {
        self.apply_decision(&req.tool_name, &decision);
        let entry = ApprovalLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_name: req.tool_name.clone(),
            arguments_summary: req.arguments.to_string(),
            decision: decision.clone(),
            channel: "external".to_string(),
        };
        self.audit_log.lock().push(entry);
        self.funnel.record_decision(&req.tool_name, &decision);
    }

    fn apply_decision(&self, tool_name: &str, decision: &ApprovalResponse) {
        match decision {
            ApprovalResponse::Always => {
                self.session_allowlist.lock().insert(tool_name.to_string());
            }
            ApprovalResponse::Forever => {
                self.forever_grants.grant(tool_name);
            }
            _ => {}
        }
    }

    // ── Plan mode ─────────────────────────────────────────────────────────

    /// Register an approved plan and return its id. The plan should be shown
    /// to the operator (tools + bounds) before calling this.
    pub fn approve_plan(&self, steps: Vec<PlanStep>) -> String {
        let plan = ApprovedPlan {
            plan_id: uuid::Uuid::new_v4().to_string(),
            steps,
            created_at: chrono::Utc::now().to_rfc3339(),
            cursor: 0,
        };
        let id = plan.plan_id.clone();
        self.plans.lock().insert(id.clone(), plan);
        id
    }

    /// Check a tool call against an approved plan.
    ///
    /// On success the plan cursor advances. **On any violation the plan is
    /// revoked** (halt on drift) — subsequent calls return `UnknownPlan` and
    /// must go through a fresh approval.
    pub fn check_plan_call(
        &self,
        plan_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<(), PlanViolation> {
        let mut plans = self.plans.lock();
        let plan = plans.get_mut(plan_id).ok_or(PlanViolation::UnknownPlan)?;
        match plan.check_next(tool_name, arguments) {
            Ok(()) => {
                if plan.is_complete() {
                    plans.remove(plan_id);
                }
                Ok(())
            }
            Err(violation) => {
                plans.remove(plan_id); // halt on drift
                drop(plans);
                self.funnel.record_plan_violation(tool_name);
                self.audit_log.lock().push(ApprovalLogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    tool_name: tool_name.to_string(),
                    arguments_summary: format!("plan {plan_id} violation: {violation:?}"),
                    decision: ApprovalResponse::No,
                    channel: "plan-mode".to_string(),
                });
                Err(violation)
            }
        }
    }

    /// Number of currently active (not yet completed/revoked) plans.
    pub fn active_plan_count(&self) -> usize {
        self.plans.lock().len()
    }

    // ── Introspection ─────────────────────────────────────────────────────

    /// Returns a snapshot of the full approval audit log for this session.
    pub fn audit_log(&self) -> Vec<ApprovalLogEntry> {
        self.audit_log.lock().clone()
    }

    /// Per-tool ask/approve/deny statistics, sorted by ask count.
    pub fn funnel_summary(&self) -> Vec<(String, FunnelCounters)> {
        self.funnel.summary()
    }

    /// The persisted forever-grant store.
    pub fn forever_grants(&self) -> &ForeverGrants {
        &self.forever_grants
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyLevel;
    use serde_json::json;

    fn config_full() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec![],
        }
    }
    fn config_supervised() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec![],
            always_ask: vec![],
        }
    }
    fn config_manual() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Manual,
            auto_approve: vec![],
            always_ask: vec![],
        }
    }

    fn temp_grants() -> ForeverGrants {
        let path = std::env::temp_dir()
            .join(format!("obc_grants_{}_{}.json", std::process::id(), rand_suffix()));
        ForeverGrants::load(path)
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    fn manager(config: &AutonomyConfig, non_interactive: bool) -> ApprovalManager {
        ApprovalManager::with_grants(config, temp_grants(), non_interactive)
    }

    fn scorer_at(failures: usize) -> Arc<TrustScorer> {
        let s = TrustScorer::default();
        for _ in 0..failures {
            s.record("rover", 50.0, false);
        }
        Arc::new(s)
    }
    fn phys_low() -> RiskClass {
        RiskClass::physical(true, crate::tools::traits::BlastRadius::Low)
    }

    #[test]
    fn decide_without_trust_falls_through_to_needs_approval() {
        // No scorer attached ⇒ trust never tightens; decide mirrors needs_approval.
        assert_eq!(
            manager(&config_full(), false).decide("move", Some("rover"), phys_low()),
            Decision::Allow
        );
        assert_eq!(
            manager(&config_supervised(), false).decide("move", Some("rover"), phys_low()),
            Decision::NeedsApproval
        );
    }

    #[test]
    fn trusted_node_keeps_auto_approval_for_physical() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["move".into()],
            always_ask: vec![],
        };
        let mgr = manager(&cfg, false).with_trust(scorer_at(0)); // trusted
        assert_eq!(mgr.decide("move", Some("rover"), phys_low()), Decision::Allow);
    }

    #[test]
    fn probation_node_loses_auto_approval_for_physical() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["move".into()],
            always_ask: vec![],
        };
        // Two failures → probation; the auto-approve shortcut is overridden.
        let mgr = manager(&cfg, false).with_trust(scorer_at(2));
        assert_eq!(mgr.decide("move", Some("rover"), phys_low()), Decision::NeedsApproval);
    }

    #[test]
    fn untrusted_node_denied_physical_but_not_reads() {
        let mgr = manager(&config_full(), false).with_trust(scorer_at(3)); // untrusted
        assert_eq!(mgr.decide("move", Some("rover"), phys_low()), Decision::Deny);
        // non-physical reads are never blocked by trust
        assert_eq!(
            mgr.decide("read_file", Some("rover"), RiskClass::safe()),
            Decision::Allow
        );
    }

    #[test]
    fn full_autonomy_no_approval_needed() {
        let mgr = manager(&config_full(), false);
        assert!(!mgr.needs_approval("shell"));
    }

    #[test]
    fn supervised_needs_approval() {
        let mgr = manager(&config_supervised(), false);
        assert!(mgr.needs_approval("shell"));
    }

    #[test]
    fn auto_approve_bypasses_supervised() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["read_file".to_string()],
            always_ask: vec![],
        };
        let mgr = manager(&cfg, false);
        assert!(!mgr.needs_approval("read_file"));
        assert!(mgr.needs_approval("shell"));
    }

    #[test]
    fn always_ask_overrides_full() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec!["delete_file".to_string()],
        };
        let mgr = manager(&cfg, false);
        assert!(mgr.needs_approval("delete_file"));
        assert!(!mgr.needs_approval("shell"));
    }

    #[test]
    fn non_interactive_auto_denies() {
        let mgr = manager(&config_supervised(), true);
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: json!({"cmd": "ls"}),
        };
        assert_eq!(mgr.request_approval(&req), ApprovalResponse::No);
    }

    #[test]
    fn session_allowlist_populated_by_always() {
        let mgr = manager(&config_supervised(), true);
        mgr.session_allowlist.lock().insert("read_file".to_string());
        assert!(!mgr.needs_approval("read_file"));
    }

    #[test]
    fn audit_log_records_decisions() {
        let mgr = manager(&config_supervised(), true);
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: json!({}),
        };
        mgr.request_approval(&req);
        let log = mgr.audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tool_name, "shell");
        assert_eq!(log[0].decision, ApprovalResponse::No);
    }

    #[test]
    fn manual_needs_approval() {
        let mgr = manager(&config_manual(), false);
        assert!(mgr.needs_approval("shell"));
    }

    // ── WS6: scopes ───────────────────────────────────────────────────────

    #[test]
    fn response_scope_mapping() {
        assert_eq!(ApprovalResponse::Yes.granted_scope(), Some(ApprovalScope::Call));
        assert_eq!(
            ApprovalResponse::Always.granted_scope(),
            Some(ApprovalScope::Session)
        );
        assert_eq!(
            ApprovalResponse::Forever.granted_scope(),
            Some(ApprovalScope::Forever)
        );
        assert_eq!(ApprovalResponse::No.granted_scope(), None);
    }

    #[test]
    fn forever_grant_skips_future_approval() {
        let mgr = manager(&config_supervised(), true);
        assert!(mgr.needs_approval("backup_tool"));
        let req = ApprovalRequest {
            tool_name: "backup_tool".to_string(),
            arguments: json!({}),
        };
        mgr.record_external_decision(&req, ApprovalResponse::Forever);
        assert!(!mgr.needs_approval("backup_tool"));
    }

    #[test]
    fn forever_grants_persist_across_instances() {
        let path = std::env::temp_dir()
            .join(format!("obc_grants_persist_{}.json", rand_suffix()));
        {
            let grants = ForeverGrants::load(&path);
            grants.grant("backup_tool");
        }
        let reloaded = ForeverGrants::load(&path);
        assert!(reloaded.contains("backup_tool"));
        assert_eq!(reloaded.list().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn forever_grant_revocable() {
        let grants = temp_grants();
        grants.grant("x");
        assert!(grants.contains("x"));
        assert!(grants.revoke("x"));
        assert!(!grants.contains("x"));
        assert!(!grants.revoke("x"));
    }

    #[test]
    fn always_ask_overrides_forever_grant() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec!["delete_file".to_string()],
        };
        let mgr = manager(&cfg, true);
        let req = ApprovalRequest {
            tool_name: "delete_file".to_string(),
            arguments: json!({}),
        };
        mgr.record_external_decision(&req, ApprovalResponse::Forever);
        // Even with a forever grant, always_ask wins.
        assert!(mgr.needs_approval("delete_file"));
    }

    // ── WS6: plan mode ────────────────────────────────────────────────────

    fn two_step_plan() -> Vec<PlanStep> {
        vec![
            PlanStep {
                tool_name: "camera_capture".to_string(),
                bounds: HashMap::from([(
                    "device_id".to_string(),
                    ArgumentBound::Exact { value: json!("cam-01") },
                )]),
                deny_unlisted_args: false,
            },
            PlanStep {
                tool_name: "send_message".to_string(),
                bounds: HashMap::from([(
                    "channel".to_string(),
                    ArgumentBound::OneOf {
                        values: vec![json!("telegram"), json!("discord")],
                    },
                )]),
                deny_unlisted_args: false,
            },
        ]
    }

    #[test]
    fn plan_happy_path_consumes_steps() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(two_step_plan());
        assert_eq!(mgr.active_plan_count(), 1);

        mgr.check_plan_call(&id, "camera_capture", &json!({"device_id": "cam-01"}))
            .unwrap();
        mgr.check_plan_call(&id, "send_message", &json!({"channel": "telegram"}))
            .unwrap();
        // Plan complete → removed.
        assert_eq!(mgr.active_plan_count(), 0);
        assert_eq!(
            mgr.check_plan_call(&id, "send_message", &json!({})),
            Err(PlanViolation::UnknownPlan)
        );
    }

    #[test]
    fn plan_halts_on_wrong_tool() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(two_step_plan());
        let err = mgr
            .check_plan_call(&id, "delete_file", &json!({}))
            .unwrap_err();
        assert!(matches!(err, PlanViolation::WrongTool { .. }));
        // Revoked: even a correct call now fails.
        assert_eq!(
            mgr.check_plan_call(&id, "camera_capture", &json!({"device_id": "cam-01"})),
            Err(PlanViolation::UnknownPlan)
        );
        // Violation recorded in funnel + audit.
        let funnel = mgr.funnel_summary();
        assert!(funnel.iter().any(|(t, c)| t == "delete_file" && c.plan_violations == 1));
        assert_eq!(mgr.audit_log().len(), 1);
    }

    #[test]
    fn plan_halts_on_argument_drift() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(two_step_plan());
        let err = mgr
            .check_plan_call(&id, "camera_capture", &json!({"device_id": "cam-99"}))
            .unwrap_err();
        assert_eq!(err, PlanViolation::ArgOutOfBounds { key: "device_id".to_string() });
    }

    #[test]
    fn plan_range_and_unlisted_args() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(vec![PlanStep {
            tool_name: "set_brightness".to_string(),
            bounds: HashMap::from([(
                "level".to_string(),
                ArgumentBound::Range { min: 0.0, max: 100.0 },
            )]),
            deny_unlisted_args: true,
        }]);
        let err = mgr
            .check_plan_call(&id, "set_brightness", &json!({"level": 50, "extra": true}))
            .unwrap_err();
        assert_eq!(err, PlanViolation::UnlistedArg { key: "extra".to_string() });

        // Fresh plan; in-range value with only listed keys passes.
        let id2 = mgr.approve_plan(vec![PlanStep {
            tool_name: "set_brightness".to_string(),
            bounds: HashMap::from([(
                "level".to_string(),
                ArgumentBound::Range { min: 0.0, max: 100.0 },
            )]),
            deny_unlisted_args: true,
        }]);
        mgr.check_plan_call(&id2, "set_brightness", &json!({"level": 50}))
            .unwrap();
    }

    #[test]
    fn argument_bounds_allow() {
        assert!(ArgumentBound::Exact { value: json!("a") }.allows(&json!("a")));
        assert!(!ArgumentBound::Exact { value: json!("a") }.allows(&json!("b")));
        assert!(ArgumentBound::Range { min: 1.0, max: 5.0 }.allows(&json!(3)));
        assert!(!ArgumentBound::Range { min: 1.0, max: 5.0 }.allows(&json!(9)));
        assert!(!ArgumentBound::Range { min: 1.0, max: 5.0 }.allows(&json!("3")));
        assert!(ArgumentBound::Any.allows(&json!(null)));
    }

    // ── WS6: funnel ───────────────────────────────────────────────────────

    #[test]
    fn funnel_counts_decisions() {
        let mgr = manager(&config_supervised(), true);
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: json!({}),
        };
        mgr.request_approval(&req); // non-interactive → denied
        mgr.record_external_decision(&req, ApprovalResponse::Yes);
        mgr.record_external_decision(&req, ApprovalResponse::Always);
        mgr.record_external_decision(&req, ApprovalResponse::Forever);

        let summary = mgr.funnel_summary();
        let (tool, c) = &summary[0];
        assert_eq!(tool, "shell");
        assert_eq!(c.asked, 4);
        assert_eq!(c.denied, 1);
        assert_eq!(c.approved_call, 1);
        assert_eq!(c.approved_session, 1);
        assert_eq!(c.approved_forever, 1);
    }
}

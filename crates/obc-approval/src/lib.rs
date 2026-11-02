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
//! - **Forever** — persisted across sessions to `approval_grants.json` in the data root (`f`)
//!
//! **Plan-mode approval**: a multi-step [`ApprovedPlan`] is approved once with
//! per-step [`ArgumentBound`]s; execution then checks each call against the
//! plan and **halts on drift** (the plan is revoked on the first violation).
//!
//! **Funnel analytics**: every ask/approve/deny is counted per tool in the
//! [`ApprovalFunnel`], so policy can be tuned (which tools ask too often?).

use obc_safety::trust::{self, TrustGate, TrustScorer};
use obc_tool_api::RiskClass;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A pending tool approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// The name of the tool being requested.
    pub tool_name: String,
    /// The arguments that will be passed to the tool.
    pub arguments: serde_json::Value,
    /// The tool's declared risk class, when known. Physical-aware prompts
    /// (Track 0) surface it so the operator sees *what kind* of action they
    /// are approving, not just its name.
    pub risk: Option<RiskClass>,
}

/// Render the operator-facing approval prompt (Track 0 physical-aware: risk
/// class + concrete effect line when derivable). Extracted from the stdin
/// flow so its content is testable.
pub fn format_approval_prompt(req: &ApprovalRequest, args_summary: &str) -> String {
    let mut out = format!(
        "\n\u{26a0}\u{fe0f}  Tool approval required\n   Tool  : {}\n",
        req.tool_name
    );
    if let Some(risk) = &req.risk {
        out.push_str(&format!("   Risk  : {}\n", risk_summary(risk)));
    }
    if let Some(effect) = describe_effect(&req.tool_name, &req.arguments) {
        out.push_str(&format!("   Effect: {effect}\n"));
    }
    out.push_str(&format!("   Args  : {args_summary}\n"));
    out
}

/// One-line human summary of a risk class for approval prompts
/// (e.g. `"PHYSICAL / irreversible / high blast - defaults to per-call approval"`).
pub fn risk_summary(risk: &RiskClass) -> String {
    if !risk.physical {
        return "software-only".to_string();
    }
    let rev = if risk.reversible {
        "reversible"
    } else {
        "irreversible"
    };
    let blast = match risk.blast {
        obc_tool_api::BlastRadius::None => "no blast radius",
        obc_tool_api::BlastRadius::Low => "low blast radius",
        obc_tool_api::BlastRadius::High => "high blast radius",
    };
    let tail = if risk.requires_per_call_approval() {
        " - defaults to per-call approval"
    } else {
        ""
    };
    format!("PHYSICAL / {rev} / {blast}{tail}")
}

/// Best-effort concrete-effect description for a physical tool call, for
/// approval prompts (Track 0: "open GPIO 17 -> unlock front door" class of
/// clarity). Returns `None` when no meaningful description can be derived -
/// callers fall back to showing the raw arguments.
pub fn describe_effect(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let n = |k: &str| args.get(k).and_then(serde_json::Value::as_i64);
    match tool_name {
        "gpio_write" => {
            let pin = n("pin")?;
            let value = n("value")?;
            let device = s("node_id").unwrap_or_else(|| "local".to_string());
            Some(format!(
                "drive {device} GPIO {pin} -> {} ({})",
                value,
                if value == 0 { "LOW" } else { "HIGH" }
            ))
        }
        "move_actuator" => {
            let name = s("actuator").or_else(|| s("name"))?;
            Some(format!("actuate '{name}' with {args}"))
        }
        "mesh_command" => {
            let node = s("node").or_else(|| s("node_id"))?;
            let cmd = s("command").unwrap_or_else(|| "command".to_string());
            Some(format!("send '{cmd}' to mesh node {node} over LoRa"))
        }
        _ => None,
    }
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
    /// Set when the decision was *not* applied, saying why: a tap from a
    /// chat user who is not on the channel's approver list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
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
    /// `approval_grants.json` in the data root — see [`obc_paths`].
    ///
    /// This used to read `$HOME`/`%USERPROFILE%` directly and append `.oh-ben-claw`,
    /// which put standing permission-to-act grants somewhere no other subsystem
    /// wrote and no environment variable could move. Two instances on one machine
    /// shared one grant file; a grant given to the kitchen agent applied to the
    /// workshop one.
    pub fn default_path() -> PathBuf {
        obc_paths::in_data_dir("approval_grants.json")
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
        let step = self
            .steps
            .get(self.cursor)
            .ok_or(PlanViolation::PlanExhausted)?;

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
        map.entry(tool_name.to_string())
            .or_default()
            .plan_violations += 1;
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

/// Whether someone is there to see a call happen.
///
/// A CLI, channel, GUI or gateway request is `Attended`: someone asked and is
/// reading the reply. A scheduled task or an A2A call is `Unattended`: it runs
/// because a timer fired or another agent asked. When unattended, `Full`
/// autonomy no longer covers a gated tool (physical, irreversible or with a
/// blast radius, [`obc_safety::taint::gated`]); only an explicit grant does,
/// and a session grant counts only for
/// [`AutonomyConfig::unattended_grant_ttl_secs`] after it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    /// Someone asked and is watching.
    #[default]
    Attended,
    /// Nobody is watching: a scheduled task or an A2A call.
    Unattended,
}

// ── Chat approvals: an unattended call that waits for an answer ─────────────

/// A gated call from an unattended turn, waiting for an operator to answer it
/// from a chat channel. Each [`ApprovalPrompter`] shows it with buttons; the
/// tap comes back through [`ApprovalManager::resolve_pending`]. Unanswered
/// after [`AutonomyConfig::approval_wait_secs`], the call is refused and the
/// expiry is audited as a denial.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Opaque id the buttons carry back: 32 hex characters, so a Telegram
    /// `callback_data` (64 bytes at most) holds it with room to spare.
    pub id: String,
    /// The tool waiting to run.
    pub tool_name: String,
    /// The arguments it will run with.
    pub arguments: serde_json::Value,
    /// The tool's declared risk class.
    pub risk: Option<RiskClass>,
    /// What asked: `"scheduled task 'open-gate'"`, `"an A2A call"`, ….
    pub origin: String,
    /// Why it needs approval: the refusal it gets if nobody answers.
    pub reason: String,
    /// How long the call waits before it is refused.
    pub expires_in_secs: u64,
    /// How long an "allow" answer keeps covering unattended calls of this
    /// tool ([`AutonomyConfig::unattended_grant_ttl_secs`]); 0 means it
    /// covers none, so channels do not offer it.
    pub session_grant_secs: u64,
}

impl PendingApproval {
    /// The same call as an [`ApprovalRequest`].
    pub fn request(&self) -> ApprovalRequest {
        ApprovalRequest {
            tool_name: self.tool_name.clone(),
            arguments: self.arguments.clone(),
            risk: self.risk,
        }
    }

    /// The text a channel shows above its buttons.
    pub fn prompt_text(&self) -> String {
        let args = self.arguments.to_string();
        let args = if args.chars().count() > 300 {
            format!("{}…", args.chars().take(300).collect::<String>())
        } else {
            args
        };
        let mut out = format_approval_prompt(&self.request(), &args)
            .trim()
            .to_string();
        out.push_str(&format!(
            "\n   From  : {}\n   Why   : {}\n\nRefused in {} unless approved.",
            self.origin,
            self.reason,
            human_secs(self.expires_in_secs)
        ));
        out
    }

    /// The answers a chat offers, with their button labels: run this call,
    /// allow the tool for the unattended window (when that is above zero),
    /// deny. Never "forever": a standing grant is made at the desk.
    pub fn chat_choices(&self) -> Vec<(ApprovalResponse, String)> {
        let mut choices = vec![(ApprovalResponse::Yes, "Approve".to_string())];
        if self.session_grant_secs > 0 {
            choices.push((
                ApprovalResponse::Always,
                format!("Allow {}", human_secs(self.session_grant_secs)),
            ));
        }
        choices.push((ApprovalResponse::No, "Deny".to_string()));
        choices
    }
}

/// `1800` reads "30 min", `90` reads "90s".
fn human_secs(secs: u64) -> String {
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// The one-letter code a chat button carries for `decision`.
pub fn chat_code(decision: &ApprovalResponse) -> &'static str {
    match decision {
        ApprovalResponse::Yes => "y",
        ApprovalResponse::Always => "a",
        ApprovalResponse::No => "n",
        ApprovalResponse::Forever => "f",
    }
}

/// The answer a chat button's code stands for. `f` is not accepted: chats
/// do not offer a forever grant, so a button claiming one was not ours.
pub fn parse_chat_code(code: &str) -> Option<ApprovalResponse> {
    match code {
        "y" => Some(ApprovalResponse::Yes),
        "a" => Some(ApprovalResponse::Always),
        "n" => Some(ApprovalResponse::No),
        _ => None,
    }
}

/// A chat channel that can show a [`PendingApproval`] with buttons.
///
/// `prompt` must not block the caller: send from a spawned task. The answer
/// comes back through [`ApprovalManager::resolve_pending`], from the same
/// channel's inbound loop.
pub trait ApprovalPrompter: Send + Sync {
    /// Short name for logs: `"telegram"`, `"slack"`.
    fn name(&self) -> &str;
    /// Show `pending` to this channel's approvers.
    fn prompt(&self, pending: &PendingApproval);
}

/// How a wait for a chat answer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// An approver let it run; `by` is `"<channel>:<user id>"`.
    Approved {
        decision: ApprovalResponse,
        by: String,
    },
    /// An approver said no.
    Denied { by: String },
    /// Nobody answered in time.
    Expired,
}

/// Why [`ApprovalManager::resolve_pending`] did not apply an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// The user who tapped is not on the channel's approver list. The
    /// attempt is audited and the call stays pending.
    NotAnApprover,
    /// No call is waiting under that id: answered already, or expired.
    NotPending,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotAnApprover => "you are not on this channel's approver list",
            Self::NotPending => "this request is no longer pending: answered already, or expired",
        })
    }
}

impl std::error::Error for ResolveError {}

struct Waiting {
    pending: PendingApproval,
    since: Instant,
    answer: tokio::sync::oneshot::Sender<(ApprovalResponse, String)>,
}

/// Takes a pending call out of the queue when its wait ends however it ends,
/// so a cancelled turn leaves nothing behind for a tap to answer.
struct PendingGuard<'a> {
    pending: &'a Mutex<HashMap<String, Waiting>>,
    id: &'a str,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.pending.lock().remove(self.id);
    }
}

pub struct ApprovalManager {
    config: AutonomyConfig,
    /// Tools approved with `Always` during this session, with when each was
    /// last granted (unattended turns honour a grant only for a while).
    session_allowlist: Arc<Mutex<HashMap<String, Instant>>>,
    /// Cross-session forever grants (persisted).
    forever_grants: Arc<ForeverGrants>,
    /// Approved multi-step plans, by plan id. Revoked on first violation.
    plans: Arc<Mutex<HashMap<String, ApprovedPlan>>>,
    /// Audit log of all approval decisions this session.
    audit_log: Arc<Mutex<Vec<ApprovalLogEntry>>>,
    /// Ask/approve/deny statistics per tool.
    funnel: Arc<ApprovalFunnel>,
    /// Optional observability context (WS5): counts approval asks centrally.
    obs: Option<Arc<obc_observability::ObsContext>>,
    /// When true, all approval requests are auto-denied (non-interactive mode).
    non_interactive: bool,
    /// Optional behavioral trust scorer: when set, an anomalous node loses its
    /// auto/forever shortcut for physical actions (or is denied them entirely).
    trust: Option<Arc<TrustScorer>>,
    /// Chat channels that show a pending call with buttons.
    prompters: Mutex<Vec<Arc<dyn ApprovalPrompter>>>,
    /// Unattended calls waiting for an answer, by pending id.
    pending: Mutex<HashMap<String, Waiting>>,
}

impl ApprovalManager {
    /// Create an `ApprovalManager` from the given autonomy configuration.
    pub fn from_config(config: &AutonomyConfig) -> Self {
        Self::with_grants(
            config,
            ForeverGrants::load(ForeverGrants::default_path()),
            false,
        )
    }

    /// Create an `ApprovalManager` for non-interactive contexts (bots, API, etc.).
    ///
    /// In non-interactive mode every request that would normally prompt the user
    /// is automatically denied so that the system never blocks waiting for input.
    pub fn for_non_interactive(config: &AutonomyConfig) -> Self {
        Self::with_grants(
            config,
            ForeverGrants::load(ForeverGrants::default_path()),
            true,
        )
    }

    /// Construct with an explicit grants store (used by tests).
    pub fn with_grants(
        config: &AutonomyConfig,
        forever_grants: ForeverGrants,
        non_interactive: bool,
    ) -> Self {
        Self {
            config: config.clone(),
            session_allowlist: Arc::new(Mutex::new(HashMap::new())),
            forever_grants: Arc::new(forever_grants),
            plans: Arc::new(Mutex::new(HashMap::new())),
            audit_log: Arc::new(Mutex::new(Vec::new())),
            funnel: Arc::new(ApprovalFunnel::default()),
            obs: None,
            non_interactive,
            trust: None,
            prompters: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Attach an observability context so approval asks count centrally (WS5).
    pub fn with_obs(mut self, obs: Arc<obc_observability::ObsContext>) -> Self {
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
        if self.session_allowlist.lock().contains_key(tool_name) {
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
            || self.session_allowlist.lock().contains_key(tool_name)
            || self.forever_grants.contains(tool_name)
    }

    /// Whether a session grant for `tool_name` covers a call made with
    /// `presence`: any grant for an attended call, and for an unattended one
    /// only a grant made within `unattended_grant_ttl_secs`.
    fn session_grant_covers(&self, tool_name: &str, presence: Presence) -> bool {
        let Some(granted_at) = self.session_allowlist.lock().get(tool_name).copied() else {
            return false;
        };
        match presence {
            Presence::Attended => true,
            Presence::Unattended => {
                granted_at.elapsed() < Duration::from_secs(self.config.unattended_grant_ttl_secs)
            }
        }
    }

    /// Trust-aware approval check for a tool call by `node_id` with risk profile
    /// `risk`. Behavioral trust can only **tighten** the normal decision, never
    /// relax it: for a physical action, an untrusted node is `Deny`ed and a node on
    /// probation `NeedsApproval` even if the tool is auto-approved or forever-
    /// granted. Non-physical actions, a trusted node, or no scorer attached fall
    /// through to the ordinary [`needs_approval`](Self::needs_approval) rules.
    pub fn decide(&self, tool_name: &str, node_id: Option<&str>, risk: RiskClass) -> Decision {
        self.decide_as(tool_name, node_id, risk, Presence::Attended)
    }

    /// [`decide`](Self::decide) for a call made with `presence`. Unattended,
    /// a gated tool runs only on an explicit grant: `auto_approve`, a forever
    /// grant, or a session grant still inside `unattended_grant_ttl_secs`.
    /// `Full` autonomy is not a grant. Like trust, presence only tightens.
    pub fn decide_as(
        &self,
        tool_name: &str,
        node_id: Option<&str>,
        risk: RiskClass,
        presence: Presence,
    ) -> Decision {
        self.decide_gated(tool_name, self.trust_gate(node_id, risk), risk, presence)
    }

    /// [`decide_as`](Self::decide_as) with the trust gate already read, so a
    /// caller that also explains the decision reads the scorer once and
    /// explains the same verdict it got.
    fn decide_gated(
        &self,
        tool_name: &str,
        gate: Option<TrustGate>,
        risk: RiskClass,
        presence: Presence,
    ) -> Decision {
        match gate {
            Some(TrustGate::Deny) => return Decision::Deny,
            Some(TrustGate::RequireApproval) => return Decision::NeedsApproval,
            Some(TrustGate::Allow) | None => {}
        }
        let auto_approved = self.config.auto_approve.iter().any(|t| t == tool_name);
        let session = self.session_grant_covers(tool_name, presence);
        // Track 0: irreversible / high-blast physical actions default to
        // **per-call approval**. Only an explicit `auto_approve` listing or a
        // session grant the operator made *this session* exempts them —
        // persisted forever grants and `Full` autonomy deliberately do not
        // (such tools are never auto-grantable across sessions).
        if risk.requires_per_call_approval() && !auto_approved && !session {
            return Decision::NeedsApproval;
        }
        // Nobody is watching: the autonomy level stops counting as a grant
        // for anything gated, and a session grant has to be recent.
        if presence == Presence::Unattended && obc_safety::taint::gated(risk) {
            let always_ask = self.config.always_ask.iter().any(|t| t == tool_name);
            let granted = auto_approved || session || self.forever_grants.contains(tool_name);
            return if granted && !always_ask {
                Decision::Allow
            } else {
                Decision::NeedsApproval
            };
        }
        if self.needs_approval(tool_name) {
            Decision::NeedsApproval
        } else {
            Decision::Allow
        }
    }

    /// What behavioral trust alone says about this call: `None` when it has no
    /// say (no scorer attached, no node, or a non-physical action).
    fn trust_gate(&self, node_id: Option<&str>, risk: RiskClass) -> Option<TrustGate> {
        if !risk.physical {
            return None;
        }
        let (scorer, node) = (self.trust.as_ref()?, node_id?);
        Some(trust::gate(scorer.level(node), risk))
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
            println!("{}", format_approval_prompt(req, &args_summary));
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
            refused: None,
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
        self.record_decision_via(req, decision, "external");
    }

    /// [`record_external_decision`](Self::record_external_decision), naming
    /// where the decision came from: `"telegram:<user id>"`, `"expired"`, ….
    fn record_decision_via(
        &self,
        req: &ApprovalRequest,
        decision: ApprovalResponse,
        channel: &str,
    ) {
        self.apply_decision(&req.tool_name, &decision);
        let entry = ApprovalLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_name: req.tool_name.clone(),
            arguments_summary: req.arguments.to_string(),
            decision: decision.clone(),
            channel: channel.to_string(),
            refused: None,
        };
        self.audit_log.lock().push(entry);
        self.funnel.record_decision(&req.tool_name, &decision);
    }

    fn apply_decision(&self, tool_name: &str, decision: &ApprovalResponse) {
        match decision {
            // Re-granting restarts the unattended window.
            ApprovalResponse::Always => {
                self.session_allowlist
                    .lock()
                    .insert(tool_name.to_string(), Instant::now());
            }
            ApprovalResponse::Forever => {
                self.forever_grants.grant(tool_name);
            }
            _ => {}
        }
    }

    // ── Chat approvals ────────────────────────────────────────────────────

    /// Attach a chat channel that shows pending calls with buttons. With one
    /// attached and [`AutonomyConfig::approval_wait_secs`] above zero, an
    /// unattended call that needs approval waits for an answer
    /// ([`approval_authorize_waiting`]) instead of being refused at once.
    pub fn add_prompter(&self, prompter: Arc<dyn ApprovalPrompter>) {
        tracing::info!(
            channel = prompter.name(),
            wait_secs = self.config.approval_wait_secs,
            "unattended approvals will be asked in chat"
        );
        self.prompters.lock().push(prompter);
    }

    /// Names of the attached prompters, in the order they were added.
    pub fn prompter_names(&self) -> Vec<String> {
        self.prompters
            .lock()
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    /// Whether an unattended call that needs approval can wait for one.
    pub fn can_wait(&self) -> bool {
        self.config.approval_wait_secs > 0 && !self.prompters.lock().is_empty()
    }

    /// Calls waiting for an answer now, oldest first.
    pub fn pending(&self) -> Vec<PendingApproval> {
        let map = self.pending.lock();
        let mut waiting: Vec<&Waiting> = map.values().collect();
        waiting.sort_by_key(|w| w.since);
        waiting.into_iter().map(|w| w.pending.clone()).collect()
    }

    /// Show `req` on every attached prompter and wait for an answer, at most
    /// [`AutonomyConfig::approval_wait_secs`]. An answer has already been
    /// applied and audited when this returns (by
    /// [`resolve_pending`](Self::resolve_pending)); an expiry is audited
    /// here, as a `No` from channel `"expired"`.
    pub async fn wait_for_decision(
        &self,
        req: &ApprovalRequest,
        origin: &str,
        reason: &str,
    ) -> WaitOutcome {
        let secs = self.config.approval_wait_secs;
        let pending = PendingApproval {
            id: uuid::Uuid::new_v4().simple().to_string(),
            tool_name: req.tool_name.clone(),
            arguments: req.arguments.clone(),
            risk: req.risk,
            origin: origin.to_string(),
            reason: reason.to_string(),
            expires_in_secs: secs,
            session_grant_secs: self.config.unattended_grant_ttl_secs,
        };
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        self.pending.lock().insert(
            pending.id.clone(),
            Waiting {
                pending: pending.clone(),
                since: Instant::now(),
                answer: tx,
            },
        );
        let _guard = PendingGuard {
            pending: &self.pending,
            id: &pending.id,
        };
        if let Some(obs) = &self.obs {
            obs.record_approval_ask(&req.tool_name);
        }
        tracing::info!(
            tool = %req.tool_name,
            id = %pending.id,
            origin,
            wait_secs = secs,
            "unattended call waiting for approval in chat"
        );
        let prompters = self.prompters.lock().clone();
        for prompter in &prompters {
            prompter.prompt(&pending);
        }

        let answer = match tokio::time::timeout(Duration::from_secs(secs), &mut rx).await {
            Ok(answer) => answer.ok(),
            Err(_) => {
                // Still queued: nobody answered. Gone: a tap took it just as
                // time ran out, and its answer is already on the way.
                let unanswered = self.pending.lock().remove(&pending.id).is_some();
                if unanswered {
                    None
                } else {
                    rx.await.ok()
                }
            }
        };
        match answer {
            Some((ApprovalResponse::No, by)) => WaitOutcome::Denied { by },
            Some((decision, by)) => WaitOutcome::Approved { decision, by },
            None => {
                tracing::warn!(
                    tool = %req.tool_name,
                    id = %pending.id,
                    "approval request expired unanswered; the call is refused"
                );
                self.record_decision_via(req, ApprovalResponse::No, "expired");
                WaitOutcome::Expired
            }
        }
    }

    /// Answer a pending call from a chat channel: `user` tapped a button in
    /// `channel` (`"telegram"`, `"slack"`). Only a `user` listed in
    /// `approvers` may answer; anyone else is refused, the attempt is
    /// audited with [`ApprovalLogEntry::refused`] set, and the call stays
    /// pending. An accepted answer is applied and audited the way
    /// [`record_external_decision`](Self::record_external_decision) does,
    /// with channel `"<channel>:<user>"`, and wakes the waiting call.
    pub fn resolve_pending(
        &self,
        id: &str,
        decision: ApprovalResponse,
        channel: &str,
        user: &str,
        approvers: &[String],
    ) -> Result<PendingApproval, ResolveError> {
        let by = format!("{channel}:{user}");
        if !approvers.iter().any(|a| a == user) {
            let tool_name = self
                .pending
                .lock()
                .get(id)
                .map(|w| w.pending.tool_name.clone())
                .unwrap_or_default();
            tracing::warn!(
                %by,
                tool = %tool_name,
                id,
                "approval answer refused: not on the channel's approver list"
            );
            self.audit_log.lock().push(ApprovalLogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_name,
                arguments_summary: format!("pending {id}"),
                decision,
                channel: by,
                refused: Some("not an approver".to_string()),
            });
            return Err(ResolveError::NotAnApprover);
        }
        let waiting = self
            .pending
            .lock()
            .remove(id)
            .ok_or(ResolveError::NotPending)?;
        self.record_decision_via(&waiting.pending.request(), decision.clone(), &by);
        tracing::info!(
            %by,
            tool = %waiting.pending.tool_name,
            id,
            ?decision,
            "approval answered in chat"
        );
        // The wait may have been cancelled with its turn; the decision stands.
        let _ = waiting.answer.send((decision, by));
        Ok(waiting.pending)
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
                    refused: None,
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

    /// Tools granted `Always` (session scope) so far this session, sorted.
    pub fn session_grants(&self) -> Vec<String> {
        let mut v: Vec<String> = self.session_allowlist.lock().keys().cloned().collect();
        v.sort();
        v
    }

    /// Revoke a session-scope grant. Returns whether it was present.
    pub fn revoke_session(&self, tool_name: &str) -> bool {
        self.session_allowlist.lock().remove(tool_name).is_some()
    }
}

// ── The join: a decision, turned into an outcome ────────────────────────────

/// Consult the approval policy for a tool call. `Ok(())` to proceed;
/// `Err(reason)` to refuse — either denied outright by dynamic trust, or (in an
/// autonomous loop) needing operator approval that has not been granted. With
/// no manager attached, or under `Full` autonomy, everything is permitted.
///
/// # Why it lives here
///
/// It lived in `obc-agent` until 2026-08-20, which is the same shape of problem
/// `track0_authorize` had the day before: this crate held [`ApprovalManager`]
/// and [`Decision`], and the one function that turns a `Decision` into a refusal
/// a caller can act on lived somewhere that a repository vendoring this crate
/// does not have. Every call to [`ApprovalManager::decide`] outside the agent
/// was in this crate's own test module — the logic was present and the entry
/// point was not.
///
/// It touches [`ApprovalManager`], [`Decision`] and `RiskClass`, all three
/// already here. Moving it added no dependency in either direction.
///
/// # What it does not do
///
/// It does not prompt. [`ApprovalManager::request_approval`] is the interactive
/// path; this is the autonomous one, where "needs approval" is a refusal with a
/// reason rather than a question. That distinction is the reason the string
/// says *why* rather than just failing.
pub fn approval_authorize(
    approval: Option<&ApprovalManager>,
    tool: &str,
    node_id: &str,
    risk: RiskClass,
) -> Result<(), String> {
    approval_authorize_as(approval, tool, node_id, risk, Presence::Attended)
}

/// [`approval_authorize`] for a call made with `presence`. Scheduled tasks and
/// A2A calls pass [`Presence::Unattended`]; see [`ApprovalManager::decide_as`].
pub fn approval_authorize_as(
    approval: Option<&ApprovalManager>,
    tool: &str,
    node_id: &str,
    risk: RiskClass,
    presence: Presence,
) -> Result<(), String> {
    let Some(approval) = approval else {
        return Ok(());
    };
    // One read of the scorer: a second one could see the node's level change
    // and explain a different verdict from the one that was made.
    let gate = approval.trust_gate(Some(node_id), risk);
    let decision = approval.decide_gated(tool, gate, risk, presence);
    explain_decision(approval, decision, gate, tool, node_id, risk, presence)
}

/// [`approval_authorize_as`], except that an unattended call that needs
/// approval waits for an answer from a chat channel when one is attached
/// ([`ApprovalManager::can_wait`]), for at most
/// [`AutonomyConfig::approval_wait_secs`], instead of being refused at once.
/// `origin` says what asked, for the prompt: `"scheduled task 'gate'"`.
///
/// Attended calls never wait. Whoever asked is reading the reply, and a
/// channel that handles its messages one at a time would be waiting on the
/// very tap it has not read yet. A `Deny` never waits either: no answer
/// lifts what dynamic trust refused.
pub async fn approval_authorize_waiting(
    approval: Option<&ApprovalManager>,
    tool: &str,
    args: &serde_json::Value,
    node_id: &str,
    risk: RiskClass,
    presence: Presence,
    origin: &str,
) -> Result<(), String> {
    let Some(approval) = approval else {
        return Ok(());
    };
    let gate = approval.trust_gate(Some(node_id), risk);
    let decision = approval.decide_gated(tool, gate, risk, presence);
    let reason = match explain_decision(approval, decision, gate, tool, node_id, risk, presence) {
        Ok(()) => return Ok(()),
        Err(reason) => reason,
    };
    if decision != Decision::NeedsApproval
        || presence != Presence::Unattended
        || !approval.can_wait()
    {
        return Err(reason);
    }
    let req = ApprovalRequest {
        tool_name: tool.to_string(),
        arguments: args.clone(),
        risk: Some(risk),
    };
    match approval.wait_for_decision(&req, origin, &reason).await {
        WaitOutcome::Approved { .. } => Ok(()),
        WaitOutcome::Denied { by } => Err(format!("denied by the operator in chat ({by})")),
        WaitOutcome::Expired => Err(format!(
            "{reason}; the approval request sent to {} got no answer within {}s \
             ([autonomy].approval_wait_secs)",
            approval.prompter_names().join(" and "),
            approval.config.approval_wait_secs
        )),
    }
}

/// Turn a decision into the refusal a caller can act on, naming why.
fn explain_decision(
    approval: &ApprovalManager,
    decision: Decision,
    gate: Option<TrustGate>,
    tool: &str,
    node_id: &str,
    risk: RiskClass,
    presence: Presence,
) -> Result<(), String> {
    match decision {
        Decision::Allow => Ok(()),
        Decision::Deny => Err("denied by approval policy".to_string()),
        // Named apart, because under `Full` autonomy the other reason cannot
        // apply and an operator reading it would look in the wrong place.
        Decision::NeedsApproval if gate == Some(TrustGate::RequireApproval) => {
            Err(format!(
                "requires operator approval (node '{node_id}' is on trust probation after \
                 recent failures, so its physical actions lose auto-approval)"
            ))
        }
        // Named apart for the same reason: under `Full` the level is not why.
        Decision::NeedsApproval
            if presence == Presence::Unattended && obc_safety::taint::gated(risk) =>
        {
            Err(format!(
                "requires operator approval (unattended turn: a scheduled task or A2A call \
                 does not get autonomy-level approval for a physical or irreversible tool; \
                 list '{tool}' in [autonomy].auto_approve, or grant it for the session \
                 within the last {}s ([autonomy].unattended_grant_ttl_secs))",
                approval.config.unattended_grant_ttl_secs
            ))
        }
        Decision::NeedsApproval => Err(
            "requires operator approval (autonomy is supervised/manual and it is not auto-approved or granted)"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config_full() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec![],
            ..Default::default()
        }
    }
    fn config_supervised() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec![],
            always_ask: vec![],
            ..Default::default()
        }
    }
    fn config_manual() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Manual,
            auto_approve: vec![],
            always_ask: vec![],
            ..Default::default()
        }
    }

    fn temp_grants() -> ForeverGrants {
        let path = std::env::temp_dir().join(format!(
            "obc_grants_{}_{}.json",
            std::process::id(),
            rand_suffix()
        ));
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
        RiskClass::physical(true, obc_tool_api::BlastRadius::Low)
    }

    fn phys_critical() -> RiskClass {
        // Irreversible + high blast: the "unlock the front door" class.
        RiskClass::physical(false, obc_tool_api::BlastRadius::High)
    }

    // ── Track 0: risk-driven approval defaults ────────────────────────────────

    #[test]
    fn critical_physical_defaults_to_per_call_even_under_full_autonomy() {
        let mgr = manager(&config_full(), false);
        assert_eq!(
            mgr.decide("door_lock", Some("rover"), phys_critical()),
            Decision::NeedsApproval,
            "irreversible/high-blast must ask, Full autonomy notwithstanding"
        );
        // Reversible low-blast physical is unchanged under Full.
        assert_eq!(
            mgr.decide("move", Some("rover"), phys_low()),
            Decision::Allow
        );
    }

    #[test]
    fn critical_physical_is_never_exempted_by_forever_grants() {
        let dir = tempfile::tempdir().unwrap();
        let grants = ForeverGrants::load(dir.path().join("grants.json"));
        grants.grant("door_lock");
        let mgr = ApprovalManager::with_grants(&config_full(), grants, false);
        assert_eq!(
            mgr.decide("door_lock", Some("rover"), phys_critical()),
            Decision::NeedsApproval,
            "a persisted forever grant must not auto-run an irreversible/high-blast tool"
        );
    }

    #[test]
    fn critical_physical_session_grant_and_auto_approve_do_exempt() {
        // Session grant: the operator explicitly allowed it *this session*.
        let mgr = manager(&config_full(), false);
        mgr.record_external_decision(
            &ApprovalRequest {
                tool_name: "door_lock".to_string(),
                arguments: json!({}),
                risk: Some(phys_critical()),
            },
            ApprovalResponse::Always,
        );
        assert_eq!(
            mgr.decide("door_lock", Some("rover"), phys_critical()),
            Decision::Allow
        );

        // Explicit auto_approve config listing is likewise an operator decision.
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec!["door_lock".into()],
            always_ask: vec![],
            ..Default::default()
        };
        assert_eq!(
            manager(&cfg, false).decide("door_lock", Some("rover"), phys_critical()),
            Decision::Allow
        );
    }

    // ── Track 0: physical-aware prompts ───────────────────────────────────────

    #[test]
    fn prompt_surfaces_risk_class_and_concrete_effect() {
        let req = ApprovalRequest {
            tool_name: "gpio_write".to_string(),
            arguments: json!({"node_id": "node-3", "pin": 17, "value": 1}),
            risk: Some(phys_critical()),
        };
        let prompt = format_approval_prompt(&req, "{...}");
        assert!(prompt.contains("gpio_write"));
        assert!(prompt.contains("PHYSICAL / irreversible / high blast"));
        assert!(prompt.contains("defaults to per-call approval"));
        assert!(prompt.contains("drive node-3 GPIO 17 -> 1 (HIGH)"));
    }

    #[test]
    fn prompt_without_risk_or_effect_still_shows_tool_and_args() {
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: json!({"cmd": "ls"}),
            risk: None,
        };
        let prompt = format_approval_prompt(&req, "{\"cmd\":\"ls\"}");
        assert!(prompt.contains("shell"));
        assert!(prompt.contains("{\"cmd\":\"ls\"}"));
        assert!(!prompt.contains("Risk"));
        assert!(!prompt.contains("Effect"));
    }

    #[test]
    fn describe_effect_covers_known_actuators_and_falls_back_to_none() {
        assert_eq!(
            describe_effect("gpio_write", &json!({"pin": 4, "value": 0})).unwrap(),
            "drive local GPIO 4 -> 0 (LOW)"
        );
        assert!(
            describe_effect("mesh_command", &json!({"node": "n2", "command": "reboot"}))
                .unwrap()
                .contains("send 'reboot' to mesh node n2")
        );
        assert!(describe_effect("http_get", &json!({"url": "x"})).is_none());
    }

    #[test]
    fn risk_summary_reads_naturally() {
        assert_eq!(risk_summary(&RiskClass::safe()), "software-only");
        assert!(risk_summary(&phys_low()).contains("PHYSICAL / reversible / low blast"));
        assert!(!risk_summary(&phys_low()).contains("per-call"));
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
            ..Default::default()
        };
        let mgr = manager(&cfg, false).with_trust(scorer_at(0)); // trusted
        assert_eq!(
            mgr.decide("move", Some("rover"), phys_low()),
            Decision::Allow
        );
    }

    #[test]
    fn probation_node_loses_auto_approval_for_physical() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["move".into()],
            always_ask: vec![],
            ..Default::default()
        };
        // Two failures → probation; the auto-approve shortcut is overridden.
        let mgr = manager(&cfg, false).with_trust(scorer_at(2));
        assert_eq!(
            mgr.decide("move", Some("rover"), phys_low()),
            Decision::NeedsApproval
        );
    }

    #[test]
    fn probation_refuses_physical_under_full_autonomy_and_says_why() {
        // The case main.rs could not reach before the scorer was attached:
        // Full autonomy passes everything, so only trust can stop this call.
        let mgr = manager(&config_full(), false).with_trust(scorer_at(2));
        let err = approval_authorize(Some(&mgr), "move", "rover", phys_low()).unwrap_err();
        assert!(err.contains("trust probation"), "{err}");
        // A healthy node under the same manager is untouched.
        assert!(approval_authorize(Some(&mgr), "move", "other", phys_low()).is_ok());
    }

    #[test]
    fn untrusted_node_denied_physical_but_not_reads() {
        let mgr = manager(&config_full(), false).with_trust(scorer_at(3)); // untrusted
        assert_eq!(
            mgr.decide("move", Some("rover"), phys_low()),
            Decision::Deny
        );
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
            ..Default::default()
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
            ..Default::default()
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
            risk: None,
        };
        assert_eq!(mgr.request_approval(&req), ApprovalResponse::No);
    }

    #[test]
    fn session_allowlist_populated_by_always() {
        let mgr = manager(&config_supervised(), true);
        mgr.session_allowlist
            .lock()
            .insert("read_file".to_string(), Instant::now());
        assert!(!mgr.needs_approval("read_file"));
    }

    #[test]
    fn audit_log_records_decisions() {
        let mgr = manager(&config_supervised(), true);
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: json!({}),
            risk: None,
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
        assert_eq!(
            ApprovalResponse::Yes.granted_scope(),
            Some(ApprovalScope::Call)
        );
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
            risk: None,
        };
        mgr.record_external_decision(&req, ApprovalResponse::Forever);
        assert!(!mgr.needs_approval("backup_tool"));
    }

    #[test]
    fn forever_grants_persist_across_instances() {
        let path = std::env::temp_dir().join(format!("obc_grants_persist_{}.json", rand_suffix()));
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
            ..Default::default()
        };
        let mgr = manager(&cfg, true);
        let req = ApprovalRequest {
            tool_name: "delete_file".to_string(),
            arguments: json!({}),
            risk: None,
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
                    ArgumentBound::Exact {
                        value: json!("cam-01"),
                    },
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
        assert!(funnel
            .iter()
            .any(|(t, c)| t == "delete_file" && c.plan_violations == 1));
        assert_eq!(mgr.audit_log().len(), 1);
    }

    #[test]
    fn plan_halts_on_argument_drift() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(two_step_plan());
        let err = mgr
            .check_plan_call(&id, "camera_capture", &json!({"device_id": "cam-99"}))
            .unwrap_err();
        assert_eq!(
            err,
            PlanViolation::ArgOutOfBounds {
                key: "device_id".to_string()
            }
        );
    }

    #[test]
    fn plan_range_and_unlisted_args() {
        let mgr = manager(&config_supervised(), true);
        let id = mgr.approve_plan(vec![PlanStep {
            tool_name: "set_brightness".to_string(),
            bounds: HashMap::from([(
                "level".to_string(),
                ArgumentBound::Range {
                    min: 0.0,
                    max: 100.0,
                },
            )]),
            deny_unlisted_args: true,
        }]);
        let err = mgr
            .check_plan_call(&id, "set_brightness", &json!({"level": 50, "extra": true}))
            .unwrap_err();
        assert_eq!(
            err,
            PlanViolation::UnlistedArg {
                key: "extra".to_string()
            }
        );

        // Fresh plan; in-range value with only listed keys passes.
        let id2 = mgr.approve_plan(vec![PlanStep {
            tool_name: "set_brightness".to_string(),
            bounds: HashMap::from([(
                "level".to_string(),
                ArgumentBound::Range {
                    min: 0.0,
                    max: 100.0,
                },
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
            risk: None,
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

    // ── approval_authorize ───────────────────────────────────────────────
    //
    // These four moved here from `obc-agent` with the function on 2026-08-20,
    // plus a fifth that is new. They never needed the agent: a manager, a tool
    // name and a risk class.

    fn authorize_mgr(level: AutonomyLevel, auto_approve: Vec<String>) -> ApprovalManager {
        let cfg = AutonomyConfig {
            level,
            auto_approve,
            always_ask: vec![],
            ..Default::default()
        };
        manager(&cfg, false)
    }

    // ── Unattended posture ───────────────────────────────────────────────────

    fn irreversible_software() -> RiskClass {
        RiskClass {
            reversible: false,
            blast: obc_tool_api::BlastRadius::None,
            physical: false,
        }
    }

    fn always(mgr: &ApprovalManager, tool: &str) {
        let req = ApprovalRequest {
            tool_name: tool.to_string(),
            arguments: json!({}),
            risk: None,
        };
        mgr.record_external_decision(&req, ApprovalResponse::Always);
    }

    /// The gap this closes: under `Full`, a scheduled task could run a gated
    /// tool nobody had granted, because the level itself was the grant.
    #[test]
    fn unattended_full_autonomy_is_not_a_grant_for_gated_tools() {
        let mgr = manager(&config_full(), false);
        for risk in [irreversible_software(), phys_low()] {
            assert_eq!(mgr.decide("browser_click", None, risk), Decision::Allow);
            assert_eq!(
                mgr.decide_as("browser_click", None, risk, Presence::Unattended),
                Decision::NeedsApproval
            );
        }
        let err = approval_authorize_as(
            Some(&mgr),
            "browser_click",
            "local",
            irreversible_software(),
            Presence::Unattended,
        )
        .unwrap_err();
        assert!(err.contains("unattended"), "{err}");
        assert!(err.contains("auto_approve"), "{err}");
    }

    #[test]
    fn unattended_safe_tools_are_unaffected() {
        let mgr = manager(&config_full(), false);
        assert_eq!(
            mgr.decide_as("read_file", None, RiskClass::safe(), Presence::Unattended),
            Decision::Allow
        );
        // Supervised still refuses an ungranted safe tool, with the old reason.
        let mgr = manager(&config_supervised(), false);
        let err = approval_authorize_as(
            Some(&mgr),
            "read_file",
            "local",
            RiskClass::safe(),
            Presence::Unattended,
        )
        .unwrap_err();
        assert!(err.contains("supervised/manual"), "{err}");
    }

    #[test]
    fn unattended_explicit_grants_still_allow() {
        let cfg = AutonomyConfig {
            auto_approve: vec!["browser_click".to_string()],
            ..Default::default()
        };
        let mgr = manager(&cfg, false);
        assert_eq!(
            mgr.decide_as(
                "browser_click",
                None,
                irreversible_software(),
                Presence::Unattended
            ),
            Decision::Allow
        );

        let mgr = manager(&config_full(), false);
        mgr.forever_grants().grant("file_delete");
        assert_eq!(
            mgr.decide_as(
                "file_delete",
                None,
                irreversible_software(),
                Presence::Unattended
            ),
            Decision::Allow
        );
    }

    #[test]
    fn unattended_always_ask_overrides_every_grant() {
        let cfg = AutonomyConfig {
            auto_approve: vec!["browser_click".to_string()],
            always_ask: vec!["browser_click".to_string()],
            ..Default::default()
        };
        let mgr = manager(&cfg, false);
        assert_eq!(
            mgr.decide_as(
                "browser_click",
                None,
                irreversible_software(),
                Presence::Unattended
            ),
            Decision::NeedsApproval
        );
    }

    #[test]
    fn a_fresh_session_grant_covers_unattended_work() {
        let mgr = manager(&config_full(), false);
        always(&mgr, "browser_click");
        assert_eq!(
            mgr.decide_as(
                "browser_click",
                None,
                irreversible_software(),
                Presence::Unattended
            ),
            Decision::Allow
        );
    }

    /// A session grant expires for unattended work and only for unattended
    /// work: ttl 0 makes every grant stale on arrival.
    #[test]
    fn a_stale_session_grant_does_not_cover_unattended_work() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            unattended_grant_ttl_secs: 0,
            ..Default::default()
        };
        let mgr = manager(&cfg, false);
        always(&mgr, "browser_click");
        assert_eq!(
            mgr.decide_as(
                "browser_click",
                None,
                irreversible_software(),
                Presence::Unattended
            ),
            Decision::NeedsApproval
        );
        assert_eq!(
            mgr.decide("browser_click", None, irreversible_software()),
            Decision::Allow,
            "attended calls keep the session grant"
        );
        // The per-call rule for critical physical tools honours the same window.
        always(&mgr, "door_unlock");
        assert_eq!(
            mgr.decide_as("door_unlock", None, phys_critical(), Presence::Unattended),
            Decision::NeedsApproval
        );
        assert_eq!(
            mgr.decide("door_unlock", None, phys_critical()),
            Decision::Allow
        );
    }

    #[test]
    fn unattended_grant_ttl_defaults_to_thirty_minutes() {
        assert_eq!(AutonomyConfig::default().unattended_grant_ttl_secs, 1800);
        let parsed: AutonomyConfig = serde_json::from_str(r#"{"level": "full"}"#).unwrap();
        assert_eq!(parsed.unattended_grant_ttl_secs, 1800);
    }

    #[test]
    fn approval_no_manager_permits_everything() {
        assert!(approval_authorize(None, "shell", "local", RiskClass::safe()).is_ok());
    }

    #[test]
    fn approval_full_autonomy_permits() {
        let mgr = authorize_mgr(AutonomyLevel::Full, vec![]);
        assert!(approval_authorize(Some(&mgr), "shell", "local", RiskClass::safe()).is_ok());
    }

    #[test]
    fn approval_supervised_refuses_ungranted_tool() {
        let mgr = authorize_mgr(AutonomyLevel::Supervised, vec![]);
        let err = approval_authorize(Some(&mgr), "shell", "local", RiskClass::safe());
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("approval"));
    }

    #[test]
    fn approval_supervised_permits_auto_approved_tool() {
        let mgr = authorize_mgr(AutonomyLevel::Supervised, vec!["sensor_read".to_string()]);
        assert!(approval_authorize(Some(&mgr), "sensor_read", "local", RiskClass::safe()).is_ok());
    }

    /// The refusal says *why*, and that is load-bearing rather than decorative.
    ///
    /// An autonomous loop cannot ask, so "needs approval" reaches it as an
    /// error string. A caller that logs it must be able to tell a policy denial
    /// from a missing grant — they call for different operator actions — and
    /// the only carrier is the text.
    #[test]
    fn a_refusal_distinguishes_denied_from_merely_ungranted() {
        let mgr = authorize_mgr(AutonomyLevel::Supervised, vec![]);
        let ungranted =
            approval_authorize(Some(&mgr), "shell", "local", RiskClass::safe()).unwrap_err();
        assert!(ungranted.contains("requires operator approval"));
        assert!(!ungranted.contains("denied by approval policy"));
    }

    // ── Chat approvals ───────────────────────────────────────────────────────

    /// Keeps what a chat channel would have shown.
    struct Captured(Mutex<Vec<PendingApproval>>);

    impl ApprovalPrompter for Captured {
        fn name(&self) -> &str {
            "test-chat"
        }
        fn prompt(&self, pending: &PendingApproval) {
            self.0.lock().push(pending.clone());
        }
    }

    fn chat_mgr(wait_secs: u64) -> (Arc<ApprovalManager>, Arc<Captured>) {
        let cfg = AutonomyConfig {
            approval_wait_secs: wait_secs,
            ..config_full()
        };
        let mgr = Arc::new(manager(&cfg, false));
        let chat = Arc::new(Captured(Mutex::new(Vec::new())));
        mgr.add_prompter(chat.clone());
        (mgr, chat)
    }

    async fn shown(chat: &Captured) -> PendingApproval {
        for _ in 0..400 {
            if let Some(p) = chat.0.lock().first().cloned() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("nothing was shown in chat");
    }

    fn waiting_call(
        mgr: &Arc<ApprovalManager>,
        presence: Presence,
    ) -> tokio::task::JoinHandle<Result<(), String>> {
        let mgr = Arc::clone(mgr);
        tokio::spawn(async move {
            approval_authorize_waiting(
                Some(&mgr),
                "browser_click",
                &json!({"selector": "#send"}),
                "local",
                irreversible_software(),
                presence,
                "scheduled task 'demo'",
            )
            .await
        })
    }

    fn approvers() -> Vec<String> {
        vec!["42".to_string()]
    }

    #[tokio::test]
    async fn an_unattended_call_waits_for_a_chat_approval_and_then_runs() {
        let (mgr, chat) = chat_mgr(30);
        let call = waiting_call(&mgr, Presence::Unattended);
        let pending = shown(&chat).await;
        assert_eq!(pending.tool_name, "browser_click");
        assert_eq!(pending.id.len(), 32);
        assert!(pending.prompt_text().contains("scheduled task 'demo'"));
        assert!(pending
            .prompt_text()
            .contains("Refused in 30s unless approved"));

        mgr.resolve_pending(
            &pending.id,
            ApprovalResponse::Yes,
            "telegram",
            "42",
            &approvers(),
        )
        .unwrap();
        assert_eq!(call.await.unwrap(), Ok(()));
        let last = mgr.audit_log().pop().unwrap();
        assert_eq!(last.channel, "telegram:42");
        assert_eq!(last.decision, ApprovalResponse::Yes);
        // "This call" grants nothing beyond it.
        assert!(mgr.session_grants().is_empty());
        assert!(mgr.pending().is_empty());
    }

    #[tokio::test]
    async fn a_tap_from_a_non_approver_is_refused_audited_and_the_call_keeps_waiting() {
        let (mgr, chat) = chat_mgr(30);
        let call = waiting_call(&mgr, Presence::Unattended);
        let pending = shown(&chat).await;

        let refused = mgr.resolve_pending(
            &pending.id,
            ApprovalResponse::Yes,
            "telegram",
            "99",
            &approvers(),
        );
        assert_eq!(refused.unwrap_err(), ResolveError::NotAnApprover);
        let attempt = mgr.audit_log().pop().unwrap();
        assert_eq!(attempt.channel, "telegram:99");
        assert_eq!(attempt.tool_name, "browser_click");
        assert_eq!(attempt.refused.as_deref(), Some("not an approver"));
        assert_eq!(
            mgr.pending().len(),
            1,
            "a stranger's tap must not answer it"
        );
        assert!(!call.is_finished());

        // The same call, answered by the approver, runs and grants the window.
        mgr.resolve_pending(
            &pending.id,
            ApprovalResponse::Always,
            "telegram",
            "42",
            &approvers(),
        )
        .unwrap();
        assert_eq!(call.await.unwrap(), Ok(()));
        assert_eq!(mgr.session_grants(), vec!["browser_click".to_string()]);
    }

    #[tokio::test]
    async fn a_chat_deny_refuses_and_says_who() {
        let (mgr, chat) = chat_mgr(30);
        let call = waiting_call(&mgr, Presence::Unattended);
        let pending = shown(&chat).await;
        mgr.resolve_pending(
            &pending.id,
            ApprovalResponse::No,
            "slack",
            "42",
            &approvers(),
        )
        .unwrap();
        let err = call.await.unwrap().unwrap_err();
        assert!(
            err.contains("denied by the operator in chat (slack:42)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn an_unanswered_call_expires_into_an_audited_denial() {
        let (mgr, chat) = chat_mgr(1);
        let err = waiting_call(&mgr, Presence::Unattended)
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.contains("unattended turn"), "{err}");
        assert!(err.contains("test-chat got no answer within 1s"), "{err}");
        let last = mgr.audit_log().pop().unwrap();
        assert_eq!(last.channel, "expired");
        assert_eq!(last.decision, ApprovalResponse::No);
        assert!(mgr.pending().is_empty());

        // A tap that arrives after the expiry finds nothing to answer.
        let late = chat.0.lock()[0].id.clone();
        assert_eq!(
            mgr.resolve_pending(&late, ApprovalResponse::Yes, "telegram", "42", &approvers())
                .unwrap_err(),
            ResolveError::NotPending
        );
    }

    #[tokio::test]
    async fn attended_calls_and_managers_without_a_chat_refuse_at_once() {
        let (mgr, chat) = chat_mgr(30);
        // Full autonomy, attended: allowed outright, nothing asked.
        assert_eq!(
            waiting_call(&mgr, Presence::Attended).await.unwrap(),
            Ok(())
        );
        let supervised = Arc::new(manager(
            &AutonomyConfig {
                level: AutonomyLevel::Supervised,
                ..Default::default()
            },
            false,
        ));
        supervised.add_prompter(chat.clone());
        assert!(waiting_call(&supervised, Presence::Attended)
            .await
            .unwrap()
            .is_err());
        assert!(chat.0.lock().is_empty(), "attended calls are never queued");

        // No prompter attached: the unattended refusal is immediate, as before.
        let bare = Arc::new(manager(&config_full(), false));
        assert!(!bare.can_wait());
        let err = waiting_call(&bare, Presence::Unattended)
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.contains("unattended turn") && err.contains("auto_approve"));
    }

    #[test]
    fn chat_codes_round_trip_and_forever_is_never_offered() {
        for d in [
            ApprovalResponse::Yes,
            ApprovalResponse::Always,
            ApprovalResponse::No,
        ] {
            assert_eq!(parse_chat_code(chat_code(&d)), Some(d));
        }
        assert_eq!(parse_chat_code("f"), None);
        let mut pending = PendingApproval {
            id: "0".repeat(32),
            tool_name: "gpio_write".into(),
            arguments: json!({}),
            risk: None,
            origin: "an A2A call".into(),
            reason: "r".into(),
            expires_in_secs: 300,
            session_grant_secs: 1800,
        };
        let labels: Vec<String> = pending.chat_choices().into_iter().map(|c| c.1).collect();
        assert_eq!(labels, ["Approve", "Allow 30 min", "Deny"]);
        pending.session_grant_secs = 0;
        assert_eq!(pending.chat_choices().len(), 2);
        assert!(pending.prompt_text().contains("Refused in 5 min"));
    }
}

// ── Autonomy: this module's own configuration block ─────────────────────────
// Moved here from the root config module on 2026-08-13, via a stop in `agent`
// that lasted about an hour and was wrong.
//
// The rule being applied all day is "the module owns its config block", and the
// first reading put these in `agent` because the agent reads them. But autonomy
// *level* is the approval policy -- how much a human has to confirm -- and this
// is the module that turns it into an `ApprovalManager`. The graph said so
// before I did: with them in `agent`, `approval -> config -> agent -> approval`
// stayed a cycle, and moving them one module further made it disappear.
//
// The root config module re-exports both names, and `agent` reads them from
// which is free, because `agent -> approval` already existed.

/// Autonomy level for tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Full autonomy: all tools execute without approval.
    #[default]
    Full,
    /// Supervised: most tools require approval unless auto_approve'd.
    Supervised,
    /// Manual: all tools require explicit approval.
    Manual,
}

/// Configuration for the human-in-the-loop approval system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyConfig {
    /// The autonomy level.
    #[serde(default)]
    pub level: AutonomyLevel,
    /// Tools that never need approval regardless of level.
    #[serde(default)]
    pub auto_approve: Vec<String>,
    /// Tools that always need approval regardless of level or session allowlist.
    #[serde(default)]
    pub always_ask: Vec<String>,
    /// How long a session ("Always") grant keeps covering a gated tool in an
    /// unattended turn (scheduled task, A2A call), counted from when it was
    /// granted. Attended turns are not affected. Default 1800 (30 minutes);
    /// 0 means a session grant never covers unattended work.
    #[serde(default = "default_unattended_grant_ttl_secs")]
    pub unattended_grant_ttl_secs: u64,
    /// How long an unattended call that needs approval waits for an answer
    /// from a chat channel with approvers (`[channels.telegram]
    /// approver_user_ids`, `[channels.slack] approver_users`) before it is
    /// refused and the expiry audited as a denial. Default 300 (5 minutes);
    /// 0 refuses at once, as happens anyway when no channel has approvers.
    /// The scheduler runs one task at a time, so a waiting call delays the
    /// tasks due after it by up to this long.
    #[serde(default = "default_approval_wait_secs")]
    pub approval_wait_secs: u64,
}

fn default_unattended_grant_ttl_secs() -> u64 {
    1800
}

fn default_approval_wait_secs() -> u64 {
    300
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec![],
            unattended_grant_ttl_secs: default_unattended_grant_ttl_secs(),
            approval_wait_secs: default_approval_wait_secs(),
        }
    }
}

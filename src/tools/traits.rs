//! Core tool trait — the interface all agent tools must implement.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How large a real-world effect a tool can have if it goes wrong.
///
/// Drives Track 0 approval defaults: higher blast radius ⇒ stricter gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlastRadius {
    /// No physical/real-world effect (pure compute, reads).
    None,
    /// A small, contained physical effect (e.g. toggle an LED).
    Low,
    /// A large or hazardous physical effect (e.g. unlock a door, drive a motor).
    High,
}

/// The physical risk profile of a tool, used by the Track 0 safety layer.
///
/// Non-physical, reversible tools (the default) are unaffected. Tools that
/// actuate the real world override [`Tool::risk_class`] to declare their risk;
/// the approval layer uses this to set default scopes (irreversible/high-blast
/// actions default to per-call approval and are never auto-grantable to
/// `forever`), and the safety gate uses it to require deterministic limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskClass {
    /// Whether the action can be cleanly undone.
    pub reversible: bool,
    /// The real-world blast radius.
    pub blast: BlastRadius,
    /// Whether the tool drives a physical actuator / real-world effect.
    pub physical: bool,
}

impl RiskClass {
    /// A non-physical, reversible action (the default for ordinary tools).
    pub const fn safe() -> Self {
        Self {
            reversible: true,
            blast: BlastRadius::None,
            physical: false,
        }
    }

    /// A physical actuator action with the given reversibility and blast radius.
    pub const fn physical(reversible: bool, blast: BlastRadius) -> Self {
        Self {
            reversible,
            blast,
            physical: true,
        }
    }

    /// Whether this action must default to per-call approval (irreversible or
    /// high-blast physical actions).
    pub const fn requires_per_call_approval(&self) -> bool {
        self.physical && (!self.reversible || matches!(self.blast, BlastRadius::High))
    }
}

impl Default for RiskClass {
    fn default() -> Self {
        Self::safe()
    }
}

/// Track 0 staged-rollout stage of a tool (relevant to learned/installed
/// skills; ordinary built-in tools are always `Autonomous`).
///
/// A skill climbs `Simulate → Supervised → Autonomous`, each promotion gated
/// on a clean run record and performed by an operator:
/// - `Simulate` — the agent may *invoke* the skill, but the execution
///   chokepoint only reports what **would** run; nothing executes.
/// - `Supervised` — executes only with an explicit operator grant
///   (auto-approve list, session, or forever grant); a failure demotes.
/// - `Autonomous` — runs like any other tool (still subject to policy,
///   Track 0 limits, trust, and approval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStage {
    Simulate,
    Supervised,
    #[default]
    Autonomous,
}

impl RolloutStage {
    /// The next stage up, if any.
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Simulate => Some(Self::Supervised),
            Self::Supervised => Some(Self::Autonomous),
            Self::Autonomous => None,
        }
    }

    /// The next stage down, if any.
    pub fn prev(self) -> Option<Self> {
        match self {
            Self::Autonomous => Some(Self::Supervised),
            Self::Supervised => Some(Self::Simulate),
            Self::Simulate => None,
        }
    }

    /// Stable string form (matches the serde wire format).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulate => "simulate",
            Self::Supervised => "supervised",
            Self::Autonomous => "autonomous",
        }
    }
}

/// The result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Whether the tool execution succeeded.
    pub success: bool,
    /// The output of the tool (if successful).
    pub output: String,
    /// The error message (if the execution failed).
    pub error: Option<String>,
}

impl ToolResult {
    /// Create a successful result.
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }

    /// Create a failed result.
    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
        }
    }

    /// Returns `true` if the tool execution succeeded.
    pub fn is_ok(&self) -> bool {
        self.success
    }

    /// Returns the output string of the tool execution.
    pub fn output(&self) -> &str {
        &self.output
    }
}

/// A tool that the agent can invoke.
///
/// Tools are the primary mechanism through which the agent interacts with the
/// world. Each tool has a name, a description, and a JSON Schema for its
/// parameters. The agent uses the description and schema to decide when and
/// how to invoke the tool.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The unique name of this tool (e.g., "camera_capture", "gpio_write").
    fn name(&self) -> &str;

    /// A human-readable description of what this tool does.
    ///
    /// This description is included in the LLM's system prompt, so it should
    /// be clear, concise, and accurate.
    fn description(&self) -> &str;

    /// The JSON Schema for this tool's parameters.
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    /// The physical risk profile of this tool (Track 0).
    ///
    /// Defaults to a non-physical, reversible action. Tools that actuate the
    /// real world (GPIO writes, relays, motors, locks, `capture_now`, OTA, …)
    /// MUST override this so the approval layer and safety gate treat them
    /// accordingly.
    fn risk_class(&self) -> RiskClass {
        RiskClass::default()
    }

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult>;

    /// If this tool is a pure delegation to another registered tool (a
    /// skill-forge `Delegate` skill), the target tool name and fixed args.
    ///
    /// The agent resolves delegate chains **inside its execution chokepoint**,
    /// so policy, Track 0, trust, and approval all evaluate the *real*
    /// underlying call, not just the skill wrapper. Default: not a delegate.
    fn as_delegate(&self) -> Option<(String, Value)> {
        None
    }

    /// If this tool is an ordered multi-step recipe (a skill-forge `Sequence`
    /// skill), its steps as `(tool, arg-template)` pairs. Like `as_delegate`,
    /// the agent executes each step through its chokepoint so every real call
    /// is policy/Track 0/trust/approval-gated. Default: not a sequence.
    fn as_sequence(&self) -> Option<Vec<(String, Value)>> {
        None
    }

    /// Track 0 staged-rollout stage. Built-in tools are `Autonomous`; skills
    /// carry their manifest's stage and are simulated / operator-gated by the
    /// agent chokepoint until promoted.
    fn rollout_stage(&self) -> RolloutStage {
        RolloutStage::Autonomous
    }
}

/// A shared handle to a tool is itself a tool (pure delegation).
///
/// This lets the agent keep its registry as `Arc<dyn Tool>` (so tools can be
/// hot-added while calls are in flight — Phase 16 skill reload) while still
/// producing the `Box<dyn Tool>` slices the provider trait expects:
/// `Box::new(Arc::clone(&tool))` is a cheap per-call snapshot.
#[async_trait]
impl Tool for std::sync::Arc<dyn Tool> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> &str {
        (**self).description()
    }

    fn parameters_schema(&self) -> Value {
        (**self).parameters_schema()
    }

    fn risk_class(&self) -> RiskClass {
        (**self).risk_class()
    }

    fn as_delegate(&self) -> Option<(String, Value)> {
        (**self).as_delegate()
    }

    fn as_sequence(&self) -> Option<Vec<(String, Value)>> {
        (**self).as_sequence()
    }

    fn rollout_stage(&self) -> RolloutStage {
        (**self).rollout_stage()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        (**self).execute(args).await
    }
}

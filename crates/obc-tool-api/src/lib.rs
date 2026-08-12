//! Core tool trait — the interface all agent tools must implement.

use async_trait::async_trait;
use serde_json::Value;

// ── Track 0 vocabulary ────────────────────────────────────────────────────────
//
// `RiskClass`, `BlastRadius`, `RolloutStage` and `OutputTrust` moved to
// `obc_safety::risk` on 2026-07-30. They are the contract between this layer,
// the approval layer and the safety gate — used by six modules — and keeping
// them here forced the security subsystem to depend on `crate::tools`, which
// was its only outward edge and pointed at the largest, least self-contained
// module in the tree.
//
// Re-exported so that `crate::tools::traits::RiskClass` still resolves; nothing
// downstream of this file changed.
pub use obc_safety::risk::{BlastRadius, OutputTrust, RiskClass, RolloutStage};

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

    /// Trust level of this tool's **output** as a data source (Track 0 taint
    /// tracking). Tools that surface content from outside the trust boundary
    /// (web fetch, remote MCP, untrusted inbound text) MUST override this to
    /// [`OutputTrust::External`]. Default: `Trusted`.
    fn output_trust(&self) -> OutputTrust {
        OutputTrust::Trusted
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

    fn output_trust(&self) -> OutputTrust {
        (**self).output_trust()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        (**self).execute(args).await
    }
}

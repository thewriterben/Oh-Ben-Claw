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
}

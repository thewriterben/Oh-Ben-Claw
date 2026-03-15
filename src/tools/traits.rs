//! Core tool trait — the interface all agent tools must implement.

use async_trait::async_trait;
use serde_json::Value;

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

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult>;
}

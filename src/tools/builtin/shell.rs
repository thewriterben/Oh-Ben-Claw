//! Shell execution tool — run shell commands and return their output.

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

/// Tool: run a shell command and return stdout + stderr.
pub struct ShellTool;

impl ShellTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its stdout and stderr. \
         Use this to run programs, scripts, system commands, or pipelines. \
         The command is run with /bin/sh -c on Linux/macOS. \
         Use this sparingly and only for safe, non-destructive operations unless explicitly asked."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 30, max: 120).",
                    "default": 30,
                    "minimum": 1,
                    "maximum": 120
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?
            .to_string();

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .clamp(1, 120);

        tracing::debug!(command = %command, timeout_secs = timeout_secs, "Executing shell command");

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("/bin/sh")
                .arg("-c")
                .arg(&command)
                .output(),
        )
        .await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let exit_code = out.status.code().unwrap_or(-1);

                let combined = if stderr.is_empty() {
                    stdout.clone()
                } else if stdout.is_empty() {
                    format!("stderr: {}", stderr)
                } else {
                    format!("{}\nstderr: {}", stdout, stderr)
                };

                if out.status.success() {
                    Ok(ToolResult::ok(combined))
                } else {
                    Ok(ToolResult {
                        success: false,
                        output: combined.clone(),
                        error: Some(format!("Exit code: {}", exit_code)),
                    })
                }
            }
            Ok(Err(e)) => Ok(ToolResult::err(format!("Failed to spawn process: {}", e))),
            Err(_) => Ok(ToolResult::err(format!(
                "Command timed out after {}s",
                timeout_secs
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shell_echo() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn shell_missing_command_param() {
        let tool = ShellTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn shell_exit_code_non_zero() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "exit 1"}))
            .await
            .unwrap();
        assert!(!result.success);
    }
}

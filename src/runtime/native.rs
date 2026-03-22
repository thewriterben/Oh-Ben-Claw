//! Native OS runtime adapter.

use crate::runtime::traits::RuntimeAdapter;
use anyhow::Context;
use std::time::Duration;
use tokio::process::Command;

/// Executes commands directly on the host operating system.
pub struct NativeRuntime;

#[async_trait::async_trait]
impl RuntimeAdapter for NativeRuntime {
    fn name(&self) -> &str {
        "native"
    }

    fn has_shell_access(&self) -> bool {
        true
    }

    async fn run_shell(
        &self,
        cmd: &str,
        args: &[&str],
        timeout_secs: u64,
    ) -> anyhow::Result<String> {
        let mut command = Command::new(cmd);
        command.args(args);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let child = command.spawn().context("failed to spawn process")?;

        let output =
            tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
                .await
                .context("command timed out")?
                .context("failed to wait for process")?;

        let mut result = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&stderr);
        }

        Ok(result)
    }
}

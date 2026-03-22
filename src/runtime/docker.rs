//! Docker container runtime adapter.

use crate::config::DockerConfig;
use crate::runtime::traits::RuntimeAdapter;
use anyhow::Context;
use std::time::Duration;
use tokio::process::Command;

/// Executes commands inside a Docker container for sandboxed execution.
pub struct DockerRuntime {
    config: DockerConfig,
}

impl DockerRuntime {
    /// Create a new `DockerRuntime` with the given configuration.
    pub fn new(config: DockerConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl RuntimeAdapter for DockerRuntime {
    fn name(&self) -> &str {
        "docker"
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
        let memory_flag = format!("{}m", self.config.memory_mb); // Docker accepts "m" for megabytes

        let mut docker_args = vec![
            "run",
            "--rm",
            "--network",
            self.config.network.as_str(),
            "--memory",
            memory_flag.as_str(),
            self.config.image.as_str(),
            cmd,
        ];
        docker_args.extend_from_slice(args);

        let child = Command::new("docker")
            .args(&docker_args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to spawn docker process")?;

        let output =
            tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
                .await
                .context("docker command timed out")?
                .context("failed to wait for docker process")?;

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

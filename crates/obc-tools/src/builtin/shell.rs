//! Shell execution tool — run shell commands and return their output.
//!
//! Two backends since 2026-09-11 (parity Stage 3, item 10):
//!
//! - [`ShellBackend::Local`]: the host shell, `cmd /C` on Windows and
//!   `/bin/sh -c` elsewhere — what this tool always did.
//! - [`ShellBackend::Docker`]: one long-lived Linux container (`docker exec`
//!   into it per command), no network unless configured, only the listed
//!   bind mounts. The container is created on first use and kept; a command
//!   runs under busybox `timeout` inside it so a runaway is killed there, not
//!   just abandoned here.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

/// Where the commands run.
#[derive(Debug, Clone)]
pub enum ShellBackend {
    /// The host shell.
    Local,
    /// A long-lived Docker container.
    Docker(DockerSandbox),
}

/// The container the docker backend execs into.
#[derive(Debug, Clone)]
pub struct DockerSandbox {
    pub image: String,
    pub container: String,
    /// `none`, `bridge`, or a named network.
    pub network: String,
    /// `(host_path, guest_path, read_only)`.
    pub mounts: Vec<(String, String, bool)>,
    pub memory: String,
    pub cpus: f64,
    /// `(name, value)` pairs passed as `-e` at creation (e.g. `TZ`).
    pub env: Vec<(String, String)>,
}

impl DockerSandbox {
    /// Working directory inside the container: the first mount's guest path
    /// (the workspace, by convention) or `/`.
    pub fn workdir(&self) -> &str {
        self.mounts.first().map(|m| m.1.as_str()).unwrap_or("/")
    }

    /// `docker run` arguments that create the long-lived container.
    pub fn run_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            "--name".into(),
            self.container.clone(),
            "--network".into(),
            self.network.clone(),
            "--memory".into(),
            self.memory.clone(),
            "--cpus".into(),
            format!("{}", self.cpus),
            "--pids-limit".into(),
            "256".into(),
            "-w".into(),
            self.workdir().to_string(),
        ];
        for (k, v) in &self.env {
            a.push("-e".into());
            a.push(format!("{k}={v}"));
        }
        for (host, guest, ro) in &self.mounts {
            a.push("-v".into());
            a.push(if *ro {
                format!("{host}:{guest}:ro")
            } else {
                format!("{host}:{guest}")
            });
        }
        a.push(self.image.clone());
        a.extend(["sleep".into(), "infinity".into()]);
        a
    }

    /// `docker exec` arguments for one command with an in-container timeout.
    pub fn exec_args(&self, command: &str, timeout_secs: u64) -> Vec<String> {
        vec![
            "exec".into(),
            self.container.clone(),
            "timeout".into(),
            timeout_secs.to_string(),
            "sh".into(),
            "-c".into(),
            command.to_string(),
        ]
    }

    /// Make sure the container exists and runs. Creates it on first use,
    /// restarts it if it was stopped (a Docker Desktop restart stops it).
    async fn ensure_running(&self) -> anyhow::Result<()> {
        let state = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", &self.container])
            .stdin(std::process::Stdio::null())
            .output()
            .await?;
        if state.status.success() {
            if String::from_utf8_lossy(&state.stdout).trim() == "true" {
                return Ok(());
            }
            let started = Command::new("docker")
                .args(["start", &self.container])
                .stdin(std::process::Stdio::null())
                .output()
                .await?;
            if started.status.success() {
                return Ok(());
            }
            anyhow::bail!(
                "docker start {} failed: {}",
                self.container,
                String::from_utf8_lossy(&started.stderr).trim()
            );
        }
        let created = Command::new("docker")
            .args(self.run_args())
            .stdin(std::process::Stdio::null())
            .output()
            .await?;
        if !created.status.success() {
            anyhow::bail!(
                "docker run failed (is Docker running? is the image pulled?): {}",
                String::from_utf8_lossy(&created.stderr).trim()
            );
        }
        tracing::info!(
            container = %self.container,
            image = %self.image,
            network = %self.network,
            mounts = self.mounts.len(),
            "shell sandbox container created"
        );
        Ok(())
    }
}

/// Tool: run a shell command and return stdout + stderr.
pub struct ShellTool {
    backend: ShellBackend,
    description: String,
}

impl ShellTool {
    /// The host shell.
    pub fn new() -> Self {
        Self::with_backend(ShellBackend::Local)
    }

    pub fn with_backend(backend: ShellBackend) -> Self {
        let description = match &backend {
            ShellBackend::Local => {
                "Execute a shell command and return its stdout and stderr. \
                 Use this to run programs, scripts, system commands, or pipelines. \
                 The command is run with /bin/sh -c on Linux/macOS. \
                 Use this sparingly and only for safe, non-destructive operations unless explicitly asked."
                    .to_string()
            }
            ShellBackend::Docker(d) => {
                let mounts = if d.mounts.is_empty() {
                    "no host directories are mounted".to_string()
                } else {
                    format!(
                        "host directories visible: {}",
                        d.mounts
                            .iter()
                            .map(|(h, g, ro)| format!("{g} (={h}{})", if *ro { ", read-only" } else { "" }))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                format!(
                    "Execute a shell command in a Linux sandbox container ({} image, busybox `sh -c`, \
                     working directory {}) and return its stdout and stderr. Network: {}. {}. \
                     This is NOT the host machine: Windows commands, host services and host files \
                     outside the mounted directories are unreachable. Use it for text processing, \
                     scripting, and files under the mounted directories.",
                    d.image,
                    d.workdir(),
                    if d.network == "none" { "off" } else { d.network.as_str() },
                    mounts
                )
            }
        };
        Self {
            backend,
            description,
        }
    }

    pub fn backend(&self) -> &ShellBackend {
        &self.backend
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
        &self.description
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

    fn risk_class(&self) -> RiskClass {
        // Shell commands are side-effecting and not safely re-runnable, so the
        // self-improvement loop must never auto-verify a learned skill by
        // replaying them. Not `physical` (no actuator), so the Track 0 gate is
        // unaffected; the non-`None` blast radius + irreversibility signal the
        // improver to quarantine rather than replay.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
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

        // Where it runs: the host shell (`cmd /C` on Windows, `/bin/sh -c`
        // elsewhere) or `docker exec` into the sandbox container.
        let mut process = match &self.backend {
            ShellBackend::Local => {
                #[cfg(windows)]
                {
                    let mut p = Command::new("cmd");
                    p.arg("/C").arg(&command);
                    p
                }
                #[cfg(not(windows))]
                {
                    let mut p = Command::new("/bin/sh");
                    p.arg("-c").arg(&command);
                    p
                }
            }
            ShellBackend::Docker(d) => {
                if let Err(e) = d.ensure_running().await {
                    return Ok(ToolResult::err(format!("shell sandbox unavailable: {e}")));
                }
                let mut p = Command::new("docker");
                p.args(d.exec_args(&command, timeout_secs));
                p
            }
        };
        // The container kills the command itself at `timeout_secs` (busybox
        // `timeout` exits 143 on the kill, GNU's 124);
        // give the docker CLI a few seconds on top before we give up here.
        let outer_timeout = match &self.backend {
            ShellBackend::Local => timeout_secs,
            ShellBackend::Docker(_) => timeout_secs + 10,
        };

        // No stdin. A model that runs a bare `date` under `cmd /C` (or
        // anything else that prompts) would otherwise sit waiting on an
        // inherited handle nobody writes to until the timeout — 30 s of
        // silence that looks like a slow command. With a closed stdin the
        // prompt reads EOF and the command returns at once.
        process.stdin(std::process::Stdio::null());

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(outer_timeout),
            process.output(),
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
                } else if matches!(exit_code, 124 | 143)
                    && matches!(self.backend, ShellBackend::Docker(_))
                {
                    Ok(ToolResult::err(format!(
                        "Command timed out after {timeout_secs}s (killed in the sandbox){}",
                        if combined.trim().is_empty() {
                            String::new()
                        } else {
                            format!("; output so far: {}", combined.trim())
                        }
                    )))
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
        let result = tool.execute(json!({"command": "exit 1"})).await.unwrap();
        assert!(!result.success);
    }

    /// A command that reads stdin must return immediately on EOF, not wait
    /// for input that is never coming. Before stdin was closed, a bare
    /// `date` on Windows hung for the full timeout (found 2026-09-05).
    #[tokio::test]
    async fn shell_command_reading_stdin_returns_at_once() {
        let tool = ShellTool::new();
        // `set /p` on cmd and `read` on sh both block on stdin when it is open.
        #[cfg(windows)]
        let command = "set /p x=prompt";
        #[cfg(not(windows))]
        let command = "read x; echo done";

        let started = std::time::Instant::now();
        let result = tool
            .execute(json!({"command": command, "timeout_secs": 10}))
            .await
            .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "command blocked on stdin for {:?}: {:?}",
            started.elapsed(),
            result
        );
        assert!(result.error.as_deref() != Some("Command timed out after 10s"));
    }
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;

    fn sandbox() -> DockerSandbox {
        DockerSandbox {
            image: "alpine:3.20".into(),
            container: "obc-shell".into(),
            network: "none".into(),
            mounts: vec![(
                "C:/Users/b/.oh-ben-claw/workspace".into(),
                "/workspace".into(),
                false,
            )],
            memory: "512m".into(),
            cpus: 1.0,
            env: vec![("TZ".into(), "MST7MDT,M3.2.0,M11.1.0".into())],
        }
    }

    #[test]
    fn env_rides_along_as_dash_e() {
        let a = sandbox().run_args().join(" ");
        assert!(a.contains(" -e TZ=MST7MDT,M3.2.0,M11.1.0 -v "), "{a}");
    }

    #[test]
    fn run_args_pin_network_limits_mounts_and_a_sleeping_entrypoint() {
        let a = sandbox().run_args();
        let s = a.join(" ");
        assert!(s.starts_with("run -d --name obc-shell --network none --memory 512m --cpus 1 --pids-limit 256 -w /workspace"));
        assert!(s.contains("-v C:/Users/b/.oh-ben-claw/workspace:/workspace "));
        assert!(s.ends_with("alpine:3.20 sleep infinity"));
        let mut ro = sandbox();
        ro.mounts[0].2 = true;
        assert!(ro.run_args().join(" ").contains("/workspace:ro "));
        let mut none = sandbox();
        none.mounts.clear();
        assert_eq!(none.workdir(), "/");
    }

    #[test]
    fn exec_args_run_the_command_under_an_in_container_timeout() {
        assert_eq!(
            sandbox().exec_args("uname -a | head -1", 30),
            [
                "exec",
                "obc-shell",
                "timeout",
                "30",
                "sh",
                "-c",
                "uname -a | head -1"
            ]
        );
    }

    #[test]
    fn the_description_tells_the_model_where_it_is() {
        let local = ShellTool::new();
        assert!(local.description().contains("/bin/sh -c"));
        let boxed = ShellTool::with_backend(ShellBackend::Docker(sandbox()));
        let d = boxed.description();
        assert!(d.contains("Linux sandbox container"), "{d}");
        assert!(d.contains("NOT the host machine"));
        assert!(d.contains("/workspace"));
        assert!(d.contains("Network: off"));
    }

    /// Needs a running Docker with `alpine:3.20` pulled; run with
    /// `OBC_TEST_DOCKER=1 cargo test -p obc-tools -- --ignored sandbox`.
    #[tokio::test]
    #[ignore]
    async fn a_command_runs_inside_the_container_without_network() {
        if std::env::var_os("OBC_TEST_DOCKER").is_none() {
            return;
        }
        let mut sb = sandbox();
        sb.container = "obc-shell-test".into();
        sb.mounts.clear();
        let tool = ShellTool::with_backend(ShellBackend::Docker(sb.clone()));
        let r = tool.execute(json!({"command": "uname -s"})).await.unwrap();
        assert!(r.success, "{r:?}");
        assert!(r.output.contains("Linux"));
        let r = tool
            .execute(json!({"command": "wget -q -T 3 -O - http://example.com", "timeout_secs": 10}))
            .await
            .unwrap();
        assert!(!r.success, "network should be off: {r:?}");
        let r = tool
            .execute(json!({"command": "sleep 5", "timeout_secs": 1}))
            .await
            .unwrap();
        assert!(
            !r.success && r.error.as_deref().unwrap_or("").contains("timed out"),
            "{r:?}"
        );
        let _ = Command::new("docker")
            .args(["rm", "-f", &sb.container])
            .output()
            .await;
    }
}

//! Runtime adapter trait for sandboxed tool execution.

/// A runtime adapter provides an abstraction over different execution
/// environments (native OS, Docker container, etc.).
#[async_trait::async_trait]
pub trait RuntimeAdapter: Send + Sync {
    /// The unique name of this runtime (e.g., `"native"`, `"docker"`).
    fn name(&self) -> &str;

    /// Whether this runtime provides shell access.
    fn has_shell_access(&self) -> bool;

    /// Execute a shell command with the given arguments, enforcing a timeout.
    ///
    /// Returns the combined stdout+stderr output as a `String`.
    async fn run_shell(
        &self,
        cmd: &str,
        args: &[&str],
        timeout_secs: u64,
    ) -> anyhow::Result<String>;
}

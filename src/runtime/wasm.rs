//! WebAssembly sandbox runtime adapter.
//!
//! This module provides a sandboxed execution environment using WebAssembly.
//! WASM modules run in a memory-safe sandbox with configurable resource limits
//! and restricted filesystem access via WASI.
//!
//! **Note:** The actual wasmtime integration is not yet included. The current
//! implementation is a framework stub that will be filled in when the
//! `wasmtime` crate is added as a dependency.

use crate::runtime::traits::RuntimeAdapter;
use std::path::PathBuf;

/// Executes code inside a WebAssembly sandbox with WASI capabilities.
///
/// The WASM runtime enforces strict resource limits (memory pages and
/// execution fuel) and only exposes explicitly allowed host directories
/// to the guest module.
pub struct WasmRuntime {
    /// Maximum number of WASM linear-memory pages (1 page = 64 KiB).
    /// Default: 256 (= 16 MiB).
    pub max_memory_pages: u32,
    /// Execution fuel limit — a coarse measure of how many instructions the
    /// guest is allowed to execute before being terminated.
    /// Default: 1_000_000.
    pub max_fuel: u64,
    /// Host directories that the WASI layer may expose to the guest module.
    pub allowed_dirs: Vec<PathBuf>,
}

impl WasmRuntime {
    /// Create a new `WasmRuntime` with explicit resource limits.
    pub fn new(max_memory_pages: u32, max_fuel: u64, allowed_dirs: Vec<PathBuf>) -> Self {
        Self {
            max_memory_pages,
            max_fuel,
            allowed_dirs,
        }
    }

    /// Execute a pre-compiled WASM module with the given arguments.
    ///
    /// The execution model:
    /// 1. Validate and compile `wasm_bytes` (AOT or lazy).
    /// 2. Configure a WASI context with `allowed_dirs` mapped into the guest.
    /// 3. Instantiate the module with memory capped at `max_memory_pages` and
    ///    fuel capped at `max_fuel`.
    /// 4. Invoke the module's `_start` (CLI) or `main` export, passing `args`.
    /// 5. Capture stdout and return it as a `String`.
    ///
    /// # Errors
    ///
    /// Currently always returns an error because the wasmtime engine is not
    /// yet linked. Once `wasmtime` is added as a dependency this method will
    /// perform real execution.
    pub fn run_wasm(
        &self,
        _wasm_bytes: &[u8],
        _args: &[&str],
        _timeout_secs: u64,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "WASM engine not available: wasmtime is not yet linked. \
             Add the `wasmtime` and `wasi-common` crates to enable \
             WebAssembly execution (max_memory_pages={}, max_fuel={}, \
             allowed_dirs={}).",
            self.max_memory_pages,
            self.max_fuel,
            self.allowed_dirs.len(),
        )
    }
}

impl Default for WasmRuntime {
    fn default() -> Self {
        Self {
            max_memory_pages: 256,  // 16 MiB
            max_fuel: 1_000_000,
            allowed_dirs: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl RuntimeAdapter for WasmRuntime {
    fn name(&self) -> &str {
        "wasm"
    }

    fn has_shell_access(&self) -> bool {
        false
    }

    async fn run_shell(
        &self,
        _cmd: &str,
        _args: &[&str],
        _timeout_secs: u64,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "WASM runtime does not support shell execution. \
             WebAssembly modules run in a sandboxed environment without \
             access to the host shell. Use `WasmRuntime::run_wasm()` to \
             execute a WASM module instead."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_wasm() {
        let rt = WasmRuntime::default();
        assert_eq!(rt.name(), "wasm");
    }

    #[test]
    fn has_no_shell_access() {
        let rt = WasmRuntime::default();
        assert!(!rt.has_shell_access());
    }

    #[test]
    fn default_values() {
        let rt = WasmRuntime::default();
        assert_eq!(rt.max_memory_pages, 256);
        assert_eq!(rt.max_fuel, 1_000_000);
        assert!(rt.allowed_dirs.is_empty());
    }

    #[test]
    fn custom_construction() {
        let dirs = vec![PathBuf::from("/data"), PathBuf::from("/config")];
        let rt = WasmRuntime::new(512, 2_000_000, dirs.clone());
        assert_eq!(rt.max_memory_pages, 512);
        assert_eq!(rt.max_fuel, 2_000_000);
        assert_eq!(rt.allowed_dirs, dirs);
    }

    #[test]
    fn run_wasm_returns_engine_not_available() {
        let rt = WasmRuntime::default();
        let err = rt.run_wasm(b"\0asm", &["--help"], 30).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("WASM engine not available"), "got: {msg}");
        assert!(msg.contains("wasmtime"), "got: {msg}");
    }

    #[tokio::test]
    async fn run_shell_returns_error() {
        let rt = WasmRuntime::default();
        let err = rt.run_shell("echo", &["hello"], 10).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not support shell execution"), "got: {msg}");
        assert!(msg.contains("run_wasm"), "got: {msg}");
    }
}

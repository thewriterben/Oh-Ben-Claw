//! Sandboxed tool execution runtime.
//!
//! This module provides an abstraction layer over different execution
//! environments. The runtime is selected via [`RuntimeConfig`].
//!
//! # Supported Runtimes
//!
//! - `"native"` — executes commands directly on the host OS
//! - `"docker"` — executes commands inside a Docker container
//! - `"wasm"`   — executes WebAssembly modules in a sandboxed environment

pub mod docker;
pub mod native;
pub mod traits;
pub mod wasm;

pub use docker::DockerRuntime;
pub use native::NativeRuntime;
pub use traits::RuntimeAdapter;
pub use wasm::WasmRuntime;

use crate::config::RuntimeConfig;

/// Create a runtime adapter from the given configuration.
///
/// # Errors
///
/// Returns an error if `config.kind` is not `"native"`, `"docker"`, or `"wasm"`.
pub fn create_runtime(config: &RuntimeConfig) -> anyhow::Result<Box<dyn RuntimeAdapter>> {
    match config.kind.as_str() {
        "native" => Ok(Box::new(NativeRuntime)),
        "docker" => Ok(Box::new(DockerRuntime::new(config.docker.clone()))),
        "wasm" => {
            let wasm_cfg = &config.wasm;
            Ok(Box::new(WasmRuntime::new(
                wasm_cfg.max_memory_pages,
                wasm_cfg.max_fuel,
                wasm_cfg.allowed_dirs.iter().map(Into::into).collect(),
            )))
        }
        other => anyhow::bail!(
            "Unknown runtime kind: '{}'. Valid values are 'native', 'docker', and 'wasm'.",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DockerConfig, RuntimeConfig, WasmConfig};

    #[test]
    fn create_native_runtime() {
        let config = RuntimeConfig {
            kind: "native".to_string(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        };
        let runtime = create_runtime(&config).unwrap();
        assert_eq!(runtime.name(), "native");
        assert!(runtime.has_shell_access());
    }

    #[test]
    fn create_docker_runtime() {
        let config = RuntimeConfig {
            kind: "docker".to_string(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        };
        let runtime = create_runtime(&config).unwrap();
        assert_eq!(runtime.name(), "docker");
    }

    #[test]
    fn create_wasm_runtime() {
        let config = RuntimeConfig {
            kind: "wasm".to_string(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        };
        let runtime = create_runtime(&config).unwrap();
        assert_eq!(runtime.name(), "wasm");
        assert!(!runtime.has_shell_access());
    }

    #[test]
    fn unknown_runtime_returns_error() {
        let config = RuntimeConfig {
            kind: "kubernetes".to_string(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        };
        assert!(create_runtime(&config).is_err());
    }

    #[test]
    fn empty_kind_returns_error() {
        let config = RuntimeConfig {
            kind: String::new(),
            docker: DockerConfig::default(),
            wasm: WasmConfig::default(),
        };
        assert!(create_runtime(&config).is_err());
    }
}

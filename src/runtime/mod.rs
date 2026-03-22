//! Sandboxed tool execution runtime.
//!
//! This module provides an abstraction layer over different execution
//! environments. The runtime is selected via [`RuntimeConfig`].
//!
//! # Supported Runtimes
//!
//! - `"native"` — executes commands directly on the host OS
//! - `"docker"` — executes commands inside a Docker container

pub mod docker;
pub mod native;
pub mod traits;

pub use docker::DockerRuntime;
pub use native::NativeRuntime;
pub use traits::RuntimeAdapter;

use crate::config::RuntimeConfig;

/// Create a runtime adapter from the given configuration.
///
/// # Errors
///
/// Returns an error if `config.kind` is not `"native"` or `"docker"`.
pub fn create_runtime(config: &RuntimeConfig) -> anyhow::Result<Box<dyn RuntimeAdapter>> {
    match config.kind.as_str() {
        "native" => Ok(Box::new(NativeRuntime)),
        "docker" => Ok(Box::new(DockerRuntime::new(config.docker.clone()))),
        other => anyhow::bail!(
            "Unknown runtime kind: '{}'. Valid values are 'native' and 'docker'.",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DockerConfig, RuntimeConfig};

    #[test]
    fn create_native_runtime() {
        let config = RuntimeConfig {
            kind: "native".to_string(),
            docker: DockerConfig::default(),
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
        };
        let runtime = create_runtime(&config).unwrap();
        assert_eq!(runtime.name(), "docker");
    }

    #[test]
    fn unknown_runtime_returns_error() {
        let config = RuntimeConfig {
            kind: "kubernetes".to_string(),
            docker: DockerConfig::default(),
        };
        assert!(create_runtime(&config).is_err());
    }

    #[test]
    fn empty_kind_returns_error() {
        let config = RuntimeConfig {
            kind: String::new(),
            docker: DockerConfig::default(),
        };
        assert!(create_runtime(&config).is_err());
    }
}

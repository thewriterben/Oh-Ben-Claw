//! Oh-Ben-Claw tool registry.
//!
//! This module provides the core tool infrastructure and re-exports all
//! available tools. The agent's tool registry is built by combining the
//! default tools with any tools discovered from connected peripheral nodes.

pub mod traits;

pub use traits::{Tool, ToolResult};

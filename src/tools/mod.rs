//! Oh-Ben-Claw tool registry.
//!
//! This module provides the core tool infrastructure and all built-in tools.
//! The agent's tool registry is assembled by combining the built-in tools
//! with any tools discovered from connected peripheral nodes.

pub mod traits;
pub mod builtin;

pub use traits::{Tool, ToolResult};
pub use builtin::{shell::ShellTool, file::FileTool, http::HttpTool, memory::MemoryTool};

/// Build the default set of built-in tools.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ShellTool::new()),
        Box::new(FileTool::new()),
        Box::new(HttpTool::new()),
        Box::new(MemoryTool::new()),
    ]
}

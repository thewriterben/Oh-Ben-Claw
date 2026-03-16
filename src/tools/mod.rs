//! Oh-Ben-Claw tool registry.
//!
//! This module provides the core tool infrastructure and all built-in tools.
//! The agent's tool registry is assembled by combining the built-in tools
//! with any tools discovered from connected peripheral nodes.

pub mod builtin;
pub mod traits;

pub use traits::{Tool, ToolResult};
pub use builtin::{
    shell::ShellTool,
    file::FileTool,
    http::HttpTool,
    memory::MemoryTool,
    vision::VisionTool,
    audio::{AudioTranscribeTool, TextToSpeechTool},
    ota::{OtaUpdateTool, DeviceHealthTool},
};
/// Build the default set of built-in tools.
///
/// Vision and audio tools read their API keys from environment variables
/// (`OPENAI_API_KEY`, `OPENAI_API_BASE`) at construction time.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ShellTool::new()),
        Box::new(FileTool::new()),
        Box::new(HttpTool::new()),
        Box::new(MemoryTool::new()),
        Box::new(AudioTranscribeTool::default()),
        Box::new(TextToSpeechTool::default()),
        Box::new(OtaUpdateTool),
        Box::new(DeviceHealthTool),
    ];
    // Vision tool requires an API key; only add if one is available
    if !api_key.is_empty() {
        tools.push(Box::new(VisionTool::new(api_key)));
    }
    tools
}

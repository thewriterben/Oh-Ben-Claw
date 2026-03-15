//! Oh-Ben-Claw core agent — the central reasoning and orchestration engine.
//!
//! The agent loop receives messages from channels, invokes the LLM with the
//! current conversation history and tool registry, executes any tool calls,
//! and returns the final response.
//!
//! # Multi-Agent Delegation
//!
//! The agent supports delegation to sub-agents, allowing complex tasks to be
//! broken down and distributed across multiple specialized agents. Each
//! sub-agent can have its own provider, model, and system prompt.

/// Maximum tool-use iterations per user message to prevent runaway loops.
pub const MAX_TOOL_ITERATIONS: usize = 10;

/// Maximum conversation history messages before compaction.
pub const MAX_HISTORY_MESSAGES: usize = 50;

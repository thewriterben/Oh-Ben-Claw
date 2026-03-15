//! Oh-Ben-Claw core agent — the central reasoning and orchestration engine.
//!
//! The agent loop receives a user message, builds the conversation context,
//! calls the LLM, executes any requested tool calls, feeds results back to the
//! LLM, and repeats until the model produces a final text response.

use crate::config::AgentConfig;
use crate::memory::MemoryStore;
use crate::providers::{ChatMessage, ChatRole, Provider};
use crate::tools::traits::Tool;
use anyhow::Result;
use std::sync::Arc;

/// Maximum tool-use iterations per user message to prevent runaway loops.
pub const MAX_TOOL_ITERATIONS: usize = 10;

/// Maximum conversation history messages to include in each LLM call.
pub const MAX_HISTORY_MESSAGES: usize = 50;

// ── Agent ─────────────────────────────────────────────────────────────────────

/// The core Oh-Ben-Claw agent.
pub struct Agent {
    config: AgentConfig,
    provider: Arc<dyn Provider>,
    memory: Arc<MemoryStore>,
    tools: Vec<Box<dyn Tool>>,
}

impl Agent {
    /// Create a new agent.
    pub fn new(
        config: AgentConfig,
        provider: Arc<dyn Provider>,
        memory: Arc<MemoryStore>,
        tools: Vec<Box<dyn Tool>>,
    ) -> Self {
        Self {
            config,
            provider,
            memory,
            tools,
        }
    }

    /// Add tools to the agent's registry.
    pub fn add_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools.extend(tools);
    }

    /// Process a user message and return the assistant's final response.
    ///
    /// This method:
    /// 1. Appends the user message to memory.
    /// 2. Builds the conversation context (system prompt + recent history).
    /// 3. Calls the LLM with the current tool registry.
    /// 4. Executes any tool calls requested by the LLM.
    /// 5. Feeds tool results back to the LLM.
    /// 6. Repeats steps 3–5 until the LLM produces a final text response.
    /// 7. Appends the final response to memory and returns it.
    pub async fn process(
        &self,
        session_id: &str,
        user_message: &str,
        provider_config: &crate::config::ProviderConfig,
    ) -> Result<AgentResponse> {
        // 1. Store the user message
        self.memory
            .append_message(session_id, ChatRole::User, user_message)?;

        // 2. Build conversation context
        let mut messages = self.build_context(session_id)?;

        let max_iterations = self.config.max_tool_iterations.min(MAX_TOOL_ITERATIONS);
        let mut tool_calls_made = Vec::new();
        let mut final_response = String::new();

        // 3–6. Agent loop
        for iteration in 0..max_iterations {
            tracing::debug!(
                session_id = %session_id,
                iteration = iteration,
                message_count = messages.len(),
                "Agent loop iteration"
            );

            let completion = self
                .provider
                .chat_completion(&messages, &self.tools, provider_config)
                .await?;

            if completion.tool_calls.is_empty() {
                // Final text response — we're done
                final_response = completion.message.clone();
                break;
            }

            // Execute tool calls
            let mut tool_results = Vec::new();
            for call in &completion.tool_calls {
                tracing::info!(
                    tool = %call.name,
                    call_id = %call.id,
                    "Executing tool call"
                );

                let result = self.execute_tool(&call.name, &call.args).await;
                let result_str = match &result {
                    Ok(r) => {
                        if r.success {
                            r.output.clone()
                        } else {
                            format!(
                                "Tool error: {}",
                                r.error.as_deref().unwrap_or("unknown error")
                            )
                        }
                    }
                    Err(e) => format!("Tool execution failed: {}", e),
                };

                tool_calls_made.push(ToolCallRecord {
                    name: call.name.clone(),
                    args: call.args.clone(),
                    result: result_str.clone(),
                });

                tool_results.push((call.id.clone(), call.name.clone(), result_str));
            }

            // Add assistant's tool-call message and tool results to context
            // (OpenAI-style: assistant message with tool_calls, then tool result messages)
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: format!(
                    "[Tool calls: {}]",
                    completion
                        .tool_calls
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });

            for (call_id, tool_name, result) in tool_results {
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: format!("[Tool result for {} (id={})]: {}", tool_name, call_id, result),
                });
            }

            // If this was the last iteration, force a final response
            if iteration == max_iterations - 1 {
                tracing::warn!(
                    session_id = %session_id,
                    "Max tool iterations reached; requesting final response"
                );
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: "Please provide your final response based on the tool results above."
                        .to_string(),
                });
                let final_completion = self
                    .provider
                    .chat_completion(&messages, &[], provider_config)
                    .await?;
                final_response = final_completion.message;
            }
        }

        // 7. Store the final response
        if !final_response.is_empty() {
            self.memory
                .append_message(session_id, ChatRole::Assistant, &final_response)?;
        }

        Ok(AgentResponse {
            message: final_response,
            tool_calls: tool_calls_made,
        })
    }

    /// Build the conversation context for an LLM call.
    fn build_context(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let mut messages = Vec::new();

        // System prompt
        messages.push(ChatMessage {
            role: ChatRole::System,
            content: self.config.system_prompt.clone(),
        });

        // Recent conversation history
        let history = self
            .memory
            .load_recent_messages(session_id, MAX_HISTORY_MESSAGES)?;

        // Skip the last user message since we'll add it fresh from memory
        // (it was already appended before this call)
        messages.extend(history);

        Ok(messages)
    }

    /// Execute a tool by name with the given JSON arguments string.
    async fn execute_tool(
        &self,
        name: &str,
        args_str: &str,
    ) -> Result<crate::tools::traits::ToolResult> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;

        let args: serde_json::Value = serde_json::from_str(args_str)
            .unwrap_or_else(|_| serde_json::json!({}));

        tool.execute(args).await
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

// ── Response Types ────────────────────────────────────────────────────────────

/// A record of a single tool call made during an agent loop iteration.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub args: String,
    pub result: String,
}

/// The final response from the agent after processing a user message.
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// The assistant's final text response.
    pub message: String,
    /// All tool calls made during the agent loop.
    pub tool_calls: Vec<ToolCallRecord>,
}

impl AgentResponse {
    /// Whether any tools were called during this response.
    pub fn used_tools(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryStore;

    #[test]
    fn agent_response_used_tools() {
        let response = AgentResponse {
            message: "Done".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "shell".to_string(),
                args: "{}".to_string(),
                result: "ok".to_string(),
            }],
        };
        assert!(response.used_tools());

        let empty = AgentResponse {
            message: "Hello".to_string(),
            tool_calls: vec![],
        };
        assert!(!empty.used_tools());
    }
}

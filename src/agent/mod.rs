//! Oh-Ben-Claw core agent — the central reasoning and orchestration engine.
//!
//! The agent loop receives a user message, builds the conversation context,
//! calls the LLM, executes any requested tool calls, feeds results back to the
//! LLM, and repeats until the model produces a final text response.

pub mod delegation_tools;
pub mod edge;
pub mod handle;
pub mod orchestrator;
pub mod pool;
pub mod reflex;
pub mod reflexion;
pub mod streaming;
pub use edge::{EdgeAgent, EdgeAgentBuilder};
pub use handle::{AgentEvent, AgentHandle};
#[allow(unused_imports)]
pub use orchestrator::{OrchestratorAgent, OrchestratorConfig, RoutingStrategy};
#[allow(unused_imports)]
pub use pool::{AgentPool, SubAgentInfo, SubAgentSpec, SubAgentStatus};

use crate::config::AgentConfig;
use crate::memory::trajectory::{Episode, EpisodeStep, Outcome, TrajectoryStore};
use crate::memory::MemoryStore;
use crate::providers::{ChatMessage, ChatRole, Provider};
use crate::security::audit::{ActionAuditor, Decision};
use crate::security::limits::SafetyGate;
use crate::security::PolicyEngine;
use crate::tools::traits::{RiskClass, Tool};
use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};

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
    /// Optional policy engine for tool execution enforcement.
    policy: Option<PolicyEngine>,
    /// Optional observability context (Phase 15 WS5): when attached, every
    /// run records an `agent.process` span, each tool call an `agent.tool`
    /// span, and the turn/tool/error counters are incremented.
    obs: Option<Arc<crate::observability::ObsContext>>,
    /// Track 0: deterministic, model-independent safety limits applied to
    /// physical tool calls before execution.
    safety: Option<Arc<SafetyGate>>,
    /// Track 0: tamper-evident audit log of physical-action decisions.
    auditor: Option<Arc<Mutex<ActionAuditor>>>,
    /// Phase 16: when attached, each run is captured as an `Episode` for
    /// experiential self-improvement.
    trajectory: Option<Arc<TrajectoryStore>>,
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
            policy: None,
            obs: None,
            safety: None,
            auditor: None,
            trajectory: None,
        }
    }

    /// Attach a policy engine to enforce tool execution policies.
    pub fn with_policy(mut self, policy: PolicyEngine) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Attach an observability context (spans + counters per run).
    pub fn with_obs(mut self, obs: Arc<crate::observability::ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    /// Attach a Track 0 safety gate enforcing deterministic limits on physical
    /// tool calls (pin allow-list, value range, rate).
    pub fn with_safety_gate(mut self, gate: Arc<SafetyGate>) -> Self {
        self.safety = Some(gate);
        self
    }

    /// Attach a Track 0 action auditor that records every physical-action
    /// decision to a tamper-evident log.
    pub fn with_action_auditor(mut self, auditor: Arc<Mutex<ActionAuditor>>) -> Self {
        self.auditor = Some(auditor);
        self
    }

    /// Attach a trajectory store so each run is captured as an `Episode`
    /// (Phase 16 experiential self-improvement).
    pub fn with_trajectory_store(mut self, store: Arc<TrajectoryStore>) -> Self {
        self.trajectory = Some(store);
        self
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
        // WS5: outer span for the whole run (finished before returning).
        let mut run_span = self.obs.as_ref().map(|obs| {
            let mut span = obs.span("agent.process");
            span.set_attr("session_id", session_id);
            span
        });

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

                // WS5: per-tool-call span + counters.
                let mut tool_span = self.obs.as_ref().map(|obs| {
                    obs.record_tool_call(&call.name);
                    let mut span = obs.span("agent.tool");
                    span.set_attr("tool", &call.name);
                    span.set_attr("session_id", session_id);
                    span
                });

                let t0 = std::time::Instant::now();
                let result = self.execute_tool(&call.name, &call.args).await;
                let duration_ms = t0.elapsed().as_millis() as u64;

                if let Some(span) = tool_span.take() {
                    match &result {
                        Ok(r) if r.success => {
                            span.finish_ok();
                        }
                        Ok(r) => {
                            if let Some(obs) = &self.obs {
                                obs.record_tool_error(&call.name);
                            }
                            span.finish_err(
                                r.error.clone().unwrap_or_else(|| "tool error".to_string()),
                            );
                        }
                        Err(e) => {
                            if let Some(obs) = &self.obs {
                                obs.record_tool_error(&call.name);
                            }
                            span.finish_err(e.to_string());
                        }
                    }
                }
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
                    duration_ms,
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
                    content: format!(
                        "[Tool result for {} (id={})]: {}",
                        tool_name, call_id, result
                    ),
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

        // WS5: close the run span and record the completed turn.
        if let Some(mut span) = run_span.take() {
            span.set_attr("tool_calls", tool_calls_made.len().to_string());
            span.finish_ok();
        }
        if let Some(obs) = &self.obs {
            // Tool calls were already counted per-call above; only the turn
            // itself is recorded here (avoids double-counting tool_calls_total).
            obs.metrics.counter("agent_turns_total").inc();
        }

        // Phase 16: capture this run as an episode for experiential self-improvement.
        if let Some(traj) = &self.trajectory {
            let steps: Vec<EpisodeStep> = tool_calls_made
                .iter()
                .map(|tc| EpisodeStep {
                    tool: tc.name.clone(),
                    args: serde_json::from_str(&tc.args)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    result: tc.result.clone(),
                    ok: !tc.result.starts_with("Tool error:")
                        && !tc.result.starts_with("Tool execution failed:")
                        && !tc.result.contains("refused by safety gate"),
                })
                .collect();
            let episode = Episode {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
                objective: user_message.to_string(),
                steps,
                outcome: if final_response.is_empty() {
                    Outcome::Failure
                } else {
                    Outcome::Success
                },
                ts_ms: now_ms(),
            };
            if let Err(e) = traj.record(&episode) {
                tracing::warn!(error = %e, "Failed to record trajectory episode");
            }
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
    ///
    /// Evaluates the security policy before executing. Denied tool calls
    /// return an error immediately without invoking the tool.
    async fn execute_tool(
        &self,
        name: &str,
        args_str: &str,
    ) -> Result<crate::tools::traits::ToolResult> {
        // Policy check
        if let Some(ref policy) = self.policy {
            let verdict = policy.evaluate(name, args_str);
            if !verdict.is_allowed() {
                let reason = verdict
                    .reason
                    .as_deref()
                    .unwrap_or("blocked by security policy");
                let policy_name = verdict.policy_name.as_deref().unwrap_or("unknown");
                tracing::warn!(
                    tool = %name,
                    policy = %policy_name,
                    reason = %reason,
                    "Tool call blocked by policy"
                );
                return Ok(crate::tools::traits::ToolResult::err(format!(
                    "Tool '{}' blocked by security policy '{}': {}",
                    name, policy_name, reason
                )));
            }
        }

        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;

        let args: serde_json::Value =
            serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));

        // Track 0: for physical actions, enforce deterministic safety limits and
        // record a tamper-evident audit entry BEFORE the tool runs. Refused
        // actions never reach the hardware.
        if let Err(reason) = track0_authorize(
            self.safety.as_deref(),
            self.auditor.as_deref(),
            name,
            tool.risk_class(),
            &args,
        ) {
            tracing::warn!(
                tool = %name,
                reason = %reason,
                "Physical action refused by Track 0 safety gate"
            );
            return Ok(crate::tools::traits::ToolResult::err(format!(
                "Tool '{}' refused by safety gate: {}",
                name, reason
            )));
        }

        tool.execute(args).await
    }

    /// Execute a tool directly by name with a JSON `Value` argument.
    ///
    /// Bypasses the agent loop — useful for direct tool invocation via the
    /// gateway's `POST /api/v1/tools/{name}` endpoint.
    /// Security policy is still evaluated.
    pub async fn execute_tool_direct(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<crate::tools::traits::ToolResult> {
        let args_str = args.to_string();
        self.execute_tool(name, &args_str).await
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// The declared physical-risk of a registered tool (default-safe if unknown).
    pub fn tool_risk(&self, name: &str) -> RiskClass {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.risk_class())
            .unwrap_or_default()
    }

    /// Clear all conversation history for the given session.
    pub fn clear_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.memory.clear_session(session_id)
    }
}

// ── Track 0: physical-action authorization ──────────────────────────────────────

/// Authorize a single tool call against the Track 0 safety layer.
///
/// Non-physical tools pass through untouched (returns `Ok`). For physical tools,
/// the deterministic [`SafetyGate`] (when configured) is consulted using the
/// action's `node_id`/`pin`/`value`, and the resulting decision is appended to
/// the tamper-evident audit log (when configured). Auditing never blocks the
/// action path. Returns `Err(reason)` only when the gate refuses the action.
fn track0_authorize(
    safety: Option<&SafetyGate>,
    auditor: Option<&Mutex<ActionAuditor>>,
    tool: &str,
    risk: RiskClass,
    args: &Value,
) -> std::result::Result<(), String> {
    if !risk.physical {
        return Ok(());
    }

    let node_id = args
        .get("node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("local");
    let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0);
    let value = args.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
    let now = now_ms();

    let decision = match safety {
        Some(gate) => match gate.check(node_id, tool, pin, value, now) {
            Ok(()) => Decision::Allowed,
            Err(violation) => Decision::Denied(violation.to_string()),
        },
        // No deterministic gate configured: the approval layer governs; we still
        // audit the action as allowed-through-here.
        None => Decision::Allowed,
    };

    if let Some(auditor) = auditor {
        let mut a = auditor.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = a.record(now, node_id, tool, args, risk, decision.clone()) {
            tracing::warn!(error = %e, "Track 0 action audit write failed");
        }
    }

    match decision {
        Decision::Denied(reason) => Err(reason),
        _ => Ok(()),
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Response Types ────────────────────────────────────────────────────────────

/// A record of a single tool call made during an agent loop iteration.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub args: String,
    pub result: String,
    /// Wall-clock time the tool call took, in milliseconds.
    pub duration_ms: u64,
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
    use crate::security::limits::{SafetyGate, SafetyLimit};
    use crate::tools::traits::BlastRadius;
    use serde_json::json;

    #[test]
    fn track0_passes_nonphysical_tools() {
        // A normal (non-physical) tool is never gated.
        let r = track0_authorize(None, None, "shell", RiskClass::safe(), &json!({}));
        assert!(r.is_ok());
    }

    #[test]
    fn track0_gate_allows_in_policy_and_denies_out_of_policy() {
        let gate = SafetyGate::new(vec![SafetyLimit {
            node_id: "local".into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![17]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: None,
        }]);
        let risk = RiskClass::physical(false, BlastRadius::High);

        // In-policy pin/value is allowed.
        assert!(track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            risk,
            &json!({"pin": 17, "value": 1})
        )
        .is_ok());

        // Out-of-policy pin is refused (and the reason is surfaced).
        let denied = track0_authorize(
            Some(&gate),
            None,
            "gpio_write",
            risk,
            &json!({"pin": 99, "value": 1}),
        );
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("pin"));
    }

    #[test]
    fn track0_without_gate_allows_physical() {
        // No gate configured ⇒ deterministic layer is permissive (approval governs).
        let r = track0_authorize(
            None,
            None,
            "gpio_write",
            RiskClass::physical(false, BlastRadius::High),
            &json!({"pin": 99, "value": 1}),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn agent_response_used_tools() {
        let response = AgentResponse {
            message: "Done".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "shell".to_string(),
                args: "{}".to_string(),
                result: "ok".to_string(),
                duration_ms: 0,
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

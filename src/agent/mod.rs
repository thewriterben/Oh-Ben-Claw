//! Oh-Ben-Claw core agent — the central reasoning and orchestration engine.
//!
//! The agent loop receives a user message, builds the conversation context,
//! calls the LLM, executes any requested tool calls, feeds results back to the
//! LLM, and repeats until the model produces a final text response.

pub mod delegation_tools;
pub mod edge;
pub mod handle;
pub mod judge;
pub mod orchestrator;
pub mod pool;
pub mod reflex;
pub mod reflexion;
pub mod safing;
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
use crate::approval::ApprovalManager;
use crate::security::trust::{self, TrustGate, TrustScorer};
use crate::skill_forge::rollout::RolloutTracker;
use crate::skill_forge::{SkillForge, SkillTool};
use crate::tools::traits::{RiskClass, RolloutStage, Tool};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

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
    /// Tool registry. `RwLock` + `Arc` elements so skills learned at runtime
    /// (Phase 16) can be hot-added/removed while calls are in flight; every
    /// LLM call takes a cheap snapshot.
    tools: RwLock<Vec<Arc<dyn Tool>>>,
    /// Names of tools that came from the skill forge (managed by
    /// [`Agent::sync_skills`]); disjoint from built-in tool names.
    skill_names: Mutex<HashSet<String>>,
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
    /// Track 0 dynamic trust: when attached, physical tool calls from an
    /// untrusted node are refused, and every tool round-trip (latency + success)
    /// feeds the per-node behavioral score.
    trust: Option<Arc<TrustScorer>>,
    /// Approval policy: when attached, every tool call is gated by the autonomy
    /// level, auto-approve list, and session/forever grants (composing with trust).
    approval: Option<Arc<ApprovalManager>>,
    /// Phase 16 P1: when `Some(k)`, each run injects a compact "learned
    /// experience" system block — up to `k` relevant learned skills and `k`
    /// similar past successful episodes — so the model prefers a verified
    /// recipe over reasoning from scratch.
    experience_k: Option<usize>,
    /// Phase 15/9: token cost tracking — `(tracker, in_price/M, out_price/M)`.
    /// Each run records an estimated `TokenUsage` (chars/4 heuristic, same as
    /// episode metrics) so the gateway can show a live cost summary.
    cost: Option<(Arc<crate::cost::CostTracker>, f64, f64)>,
    /// Track 0 staged rollout (Phase 16 P3): clean-run/failure record for
    /// staged skills. Without it, simulate/supervised gating still applies —
    /// runs just aren't counted toward promotion.
    rollout: Option<Arc<RolloutTracker>>,
    /// Skill-forge directory, enabling auto-demotion of a failing supervised
    /// skill (manifest rewrite + hot resync).
    forge_dir: Option<std::path::PathBuf>,
    /// Track 0 taint tracking: how privileged calls whose arguments echo
    /// untrusted (external-origin) tool output are handled. `Off` disables
    /// scanning; `Warn` (default) logs + counts; `Enforce` refuses unless the
    /// tool is explicitly operator-granted.
    taint_mode: crate::security::taint::TaintMode,
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
            tools: RwLock::new(tools.into_iter().map(Arc::from).collect()),
            skill_names: Mutex::new(HashSet::new()),
            policy: None,
            obs: None,
            safety: None,
            auditor: None,
            trajectory: None,
            trust: None,
            approval: None,
            experience_k: None,
            cost: None,
            rollout: None,
            forge_dir: None,
            taint_mode: crate::security::taint::TaintMode::Off,
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

    /// Enable experience retrieval (Phase 16 P1): each run injects up to `k`
    /// relevant learned skills and `k` similar past successes into the prompt.
    pub fn with_experience_retrieval(mut self, k: usize) -> Self {
        self.experience_k = Some(k.max(1));
        self
    }

    /// Attach a cost tracker (Phase 15/9): each run records an estimated
    /// `TokenUsage` priced at the given USD-per-million-token rates.
    pub fn with_cost(
        mut self,
        tracker: Arc<crate::cost::CostTracker>,
        input_price_per_million: f64,
        output_price_per_million: f64,
    ) -> Self {
        self.cost = Some((tracker, input_price_per_million, output_price_per_million));
        self
    }

    /// Attach the Track 0 staged-rollout tracker: simulated and supervised
    /// skill runs are recorded toward (or against) promotion.
    pub fn with_rollout(mut self, tracker: Arc<RolloutTracker>) -> Self {
        self.rollout = Some(tracker);
        self
    }

    /// Tell the agent where the skill forge lives, enabling auto-demotion of
    /// a failing supervised-stage skill (manifest rewrite + hot resync).
    pub fn with_forge_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.forge_dir = Some(dir.into());
        self
    }

    /// Set the Track 0 taint-tracking mode (default `Off`). In `Warn`/`Enforce`,
    /// each run pools output from `External`-trust tools and gated calls whose
    /// argument values echo that content are flagged (`Warn`) or refused
    /// (`Enforce`).
    pub fn with_taint_mode(mut self, mode: crate::security::taint::TaintMode) -> Self {
        self.taint_mode = mode;
        self
    }

    /// Attach a Track 0 dynamic trust scorer. Physical tool calls from an
    /// untrusted node are then refused, and every tool round-trip feeds the score.
    pub fn with_trust(mut self, trust: Arc<TrustScorer>) -> Self {
        self.trust = Some(trust);
        self
    }

    /// Attach an approval manager. Every tool call is then gated by the autonomy
    /// level, auto-approve list, and grants; in this autonomous loop a tool that
    /// needs operator approval is refused (no blocking prompt), and a tool denied
    /// by dynamic trust is refused outright.
    pub fn with_approval(mut self, approval: Arc<ApprovalManager>) -> Self {
        self.approval = Some(approval);
        self
    }

    /// Add tools to the agent's registry.
    pub fn add_tools(&self, tools: Vec<Box<dyn Tool>>) {
        let mut reg = self.tools.write().unwrap_or_else(|p| p.into_inner());
        reg.extend(tools.into_iter().map(Arc::<dyn Tool>::from));
    }

    /// A point-in-time snapshot of the tool registry, boxed for the provider
    /// call. Each element is an `Arc` clone — cheap, and keeps the tool alive
    /// even if the registry changes mid-run.
    fn tools_snapshot(&self) -> Vec<Box<dyn Tool>> {
        let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
        reg.iter()
            .map(|t| Box::new(Arc::clone(t)) as Box<dyn Tool>)
            .collect()
    }

    /// Look up a registered tool by name (shared handle).
    fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
        reg.iter().find(|t| t.name() == name).cloned()
    }

    /// Synchronize the tool registry with the skill forge (Phase 16).
    ///
    /// Rebuilds the forge-managed slice of the registry from the **enabled**
    /// manifests on disk, so both membership changes *and* manifest edits
    /// (e.g. a rollout-stage promotion) take effect hot:
    /// - newly enabled skills are added (no restart),
    /// - skills that were disabled/removed on disk are unregistered,
    /// - changed manifests are swapped in,
    /// - a skill whose name would shadow a built-in tool is skipped with a
    ///   warning (skills may never replace built-ins).
    ///
    /// Returns `(added, removed, shadowed)` — net membership change.
    pub fn sync_skills(&self, forge: &SkillForge) -> (usize, usize, usize) {
        let manifests = match forge.list_manifests() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "sync_skills: failed to list skill manifests");
                return (0, 0, 0);
            }
        };

        let mut skill_names = self.skill_names.lock().unwrap_or_else(|p| p.into_inner());
        let mut reg = self.tools.write().unwrap_or_else(|p| p.into_inner());
        let before: HashSet<String> = skill_names.clone();

        // Drop every forge-managed tool; re-add from the manifests on disk.
        reg.retain(|t| !skill_names.contains(t.name()));
        skill_names.clear();

        let mut shadowed = 0;
        for manifest in manifests.into_iter().filter(|m| m.enabled) {
            if reg.iter().any(|t| t.name() == manifest.name) {
                tracing::warn!(
                    skill = %manifest.name,
                    "sync_skills: skill would shadow a built-in tool; skipped"
                );
                shadowed += 1;
                continue;
            }
            match SkillTool::new(manifest) {
                Ok(tool) => {
                    if !before.contains(tool.name()) {
                        tracing::info!(skill = %tool.name(), "sync_skills: skill registered");
                    }
                    skill_names.insert(tool.name().to_string());
                    reg.push(Arc::new(tool));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "sync_skills: invalid skill manifest; skipped");
                }
            }
        }

        let added = skill_names.difference(&before).count();
        let removed = before.difference(&skill_names).count();
        (added, removed, shadowed)
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
        // Phase 16 P4: wall-clock + rough token measurement for the episode.
        let run_started = std::time::Instant::now();

        // Track 0 taint tracking: a fresh per-run pool of untrusted (external-
        // origin) tool output. `None` when scanning is off — no allocation, no
        // work in the chokepoint. Never shared across runs (no cross-turn taint).
        let taint_pool = (self.taint_mode != crate::security::taint::TaintMode::Off)
            .then(crate::security::taint::TaintPool::new);

        // 1. Store the user message
        self.memory
            .append_message(session_id, ChatRole::User, user_message)?;

        // 2. Build conversation context
        let mut messages = self.build_context(session_id)?;

        // Phase 16 P1: surface verified experience (learned skills + similar
        // past successes) as a system block right after the system prompt.
        if let Some(k) = self.experience_k {
            if let Some(block) = self.experience_block(user_message, k) {
                if let Some(obs) = &self.obs {
                    obs.metrics.counter("experience_blocks_injected_total").inc();
                }
                messages.insert(
                    1.min(messages.len()),
                    ChatMessage {
                        role: ChatRole::System,
                        content: block,
                    },
                );
            }
        }

        let max_iterations = self.config.max_tool_iterations.min(MAX_TOOL_ITERATIONS);
        let mut tool_calls_made = Vec::new();
        let mut final_response = String::new();

        // Stable tool set for this run (hot-added skills apply from the next run).
        let tool_list = self.tools_snapshot();

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
                .chat_completion(&messages, &tool_list, provider_config)
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
                    // Phase 16 reuse metric: invocations of learned skills.
                    if call.name.starts_with("learned_") {
                        obs.metrics.counter("learned_skill_invocations_total").inc();
                    }
                    let mut span = obs.span("agent.tool");
                    span.set_attr("tool", &call.name);
                    span.set_attr("session_id", session_id);
                    span
                });

                let t0 = std::time::Instant::now();
                let result = self
                    .execute_tool(&call.name, &call.args, taint_pool.as_ref())
                    .await;
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

        // Rough token split for cost + episode metrics (chars/4 heuristic —
        // a relative signal, not billing-grade accounting): the model *reads*
        // the user message and tool results, and *writes* tool args and the
        // final response.
        let input_est = {
            let chars = user_message.len()
                + tool_calls_made.iter().map(|tc| tc.result.len()).sum::<usize>();
            (chars / 4) as u64
        };
        let output_est = {
            let chars = final_response.len()
                + tool_calls_made.iter().map(|tc| tc.args.len()).sum::<usize>();
            (chars / 4) as u64
        };

        // Phase 15/9: record estimated usage against the cost budget.
        if let Some((tracker, in_price, out_price)) = &self.cost {
            tracker.record_usage(crate::cost::TokenUsage::new(
                provider_config.model.clone(),
                input_est,
                output_est,
                *in_price,
                *out_price,
            ));
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
                duration_ms: Some(run_started.elapsed().as_millis() as u64),
                tokens_est: Some(input_est + output_est),
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

    /// Build the "learned experience" system block for an objective: up to `k`
    /// relevant registered learned skills and `k` similar past successful
    /// episodes, both ranked by deterministic token overlap. `None` when
    /// nothing relevant is known — no prompt noise on novel tasks.
    fn experience_block(&self, objective: &str, k: usize) -> Option<String> {
        use crate::memory::trajectory::lexical_score;
        const MIN_SCORE: f32 = 0.2;

        // Relevant learned skills currently registered as tools.
        let mut skills: Vec<(f32, String, String)> = {
            let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
            reg.iter()
                .filter(|t| t.name().starts_with("learned_"))
                .filter_map(|t| {
                    // Match on the skill name (de-slugged) + description.
                    let haystack =
                        format!("{} {}", t.name().replace('_', " "), t.description());
                    let s = lexical_score(objective, &haystack);
                    (s >= MIN_SCORE).then(|| {
                        (s, t.name().to_string(), t.description().to_string())
                    })
                })
                .collect()
        };
        skills.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        skills.truncate(k);

        // Similar past successful episodes (proven recipes).
        let episodes = self
            .trajectory
            .as_ref()
            .and_then(|t| t.similar(objective, k).ok())
            .unwrap_or_default();

        if skills.is_empty() && episodes.is_empty() {
            return None;
        }

        let mut block = String::from(
            "[Learned experience — verified results from this agent's past successful runs]\n",
        );
        if !skills.is_empty() {
            block.push_str(
                "Learned skills relevant to this task (prefer them over re-deriving the steps):\n",
            );
            for (_, name, desc) in &skills {
                block.push_str(&format!("- {name}: {desc}\n"));
            }
        }
        if !episodes.is_empty() {
            block.push_str("Similar past successes (proven tool recipes):\n");
            for ep in &episodes {
                let recipe = ep
                    .steps
                    .iter()
                    .filter(|s| s.ok)
                    .take(3)
                    .map(|s| {
                        let mut args = s.args.to_string();
                        if args.len() > 80 {
                            args.truncate(77);
                            args.push_str("...");
                        }
                        format!("{}({})", s.tool, args)
                    })
                    .collect::<Vec<_>>()
                    .join(" → ");
                let recipe = if recipe.is_empty() {
                    "(no tool calls)".to_string()
                } else {
                    recipe
                };
                block.push_str(&format!("- \"{}\" → {}\n", ep.objective.trim(), recipe));
            }
        }
        Some(block)
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
        taint: Option<&crate::security::taint::TaintPool>,
    ) -> Result<crate::tools::traits::ToolResult> {
        self.execute_tool_inner(name, args_str, false, taint).await
    }

    /// The execution chokepoint. `in_sequence` marks a call made on behalf of
    /// a Sequence-skill step, so nested sequences are refused (bounded depth).
    /// `taint` is the per-run untrusted-content pool (Track 0 taint tracking);
    /// `None` disables pooling/scanning for this call.
    async fn execute_tool_inner(
        &self,
        name: &str,
        args_str: &str,
        in_sequence: bool,
        taint: Option<&crate::security::taint::TaintPool>,
    ) -> Result<crate::tools::traits::ToolResult> {
        let mut name = name.to_string();
        let mut args_str = args_str.to_string();

        // Resolve delegate skills (Phase 16 learned recipes) to the underlying
        // tool *before* the safety layers run, so policy (per hop), Track 0,
        // trust, and approval all evaluate the real call. A bounded hop count
        // prevents delegate cycles.
        const MAX_DELEGATE_HOPS: usize = 3;
        let mut hops = 0;
        // Set when a supervised-rollout-stage skill passed its grant gate; the
        // run's outcome is then recorded toward (or against) promotion.
        let mut staged_skill: Option<String> = None;
        let (tool, args) = loop {
            // Policy check — evaluated at every hop (skill name and target).
            if let Some(ref policy) = self.policy {
                let verdict = policy.evaluate(&name, &args_str);
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
                .find_tool(&name)
                .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;

            let args: serde_json::Value =
                serde_json::from_str(&args_str).unwrap_or_else(|_| serde_json::json!({}));

            // Track 0 staged rollout (Phase 16 P3) — checked on the *wrapper*
            // before delegate resolution, and on every hop target.
            match tool.rollout_stage() {
                RolloutStage::Simulate => {
                    // Dry-run: report what would execute; nothing runs.
                    let description = describe_simulation(&tool, &args);
                    if let Some(tracker) = &self.rollout {
                        tracker.record_clean(&name, RolloutStage::Simulate);
                    }
                    if let Some(obs) = &self.obs {
                        obs.metrics.counter("skill_simulations_total").inc();
                    }
                    tracing::info!(skill = %name, "staged skill simulated (stage=simulate)");
                    return Ok(crate::tools::traits::ToolResult::ok(format!(
                        "SIMULATION — Track 0 staged rollout (stage=simulate): skill '{}' did \
                         NOT execute. It {}. Clean simulated runs count toward promotion; an \
                         operator can promote with `oh-ben-claw skill promote {}`.",
                        name, description, name
                    )));
                }
                RolloutStage::Supervised => {
                    // Fail closed: an explicit operator grant is required; a
                    // permissive autonomy level is NOT a grant.
                    let granted = self
                        .approval
                        .as_ref()
                        .is_some_and(|a| a.explicitly_granted(&name));
                    if !granted {
                        tracing::warn!(
                            skill = %name,
                            "supervised-stage skill refused: no explicit operator grant"
                        );
                        return Ok(crate::tools::traits::ToolResult::err(format!(
                            "Skill '{}' is at rollout stage 'supervised' and requires an \
                             explicit operator grant (auto_approve list, session, or forever \
                             grant) before it may execute",
                            name
                        )));
                    }
                    staged_skill = Some(name.clone());
                }
                RolloutStage::Autonomous => {}
            }

            match tool.as_delegate() {
                Some((target, fixed_args)) => {
                    hops += 1;
                    if hops > MAX_DELEGATE_HOPS {
                        return Ok(crate::tools::traits::ToolResult::err(format!(
                            "Delegate chain exceeded {} hops at '{}' (cycle?)",
                            MAX_DELEGATE_HOPS, name
                        )));
                    }
                    let merged = merge_delegate_args(fixed_args, &args);
                    tracing::debug!(skill = %name, target = %target, "Resolving delegate skill");
                    args_str = merged.to_string();
                    name = target;
                }
                None => break (tool, args),
            }
        };
        let name = name.as_str();

        // Sequence skills (Phase 16 P2): run each step through this same
        // chokepoint, so every real call is policy/Track 0/trust/approval-
        // gated individually. Nested sequences are refused (bounded depth);
        // the first failing step aborts the recipe.
        if let Some(steps) = tool.as_sequence() {
            if in_sequence {
                return Ok(crate::tools::traits::ToolResult::err(format!(
                    "Sequence skill '{}' cannot run inside another sequence",
                    name
                )));
            }
            let mut outputs = Vec::with_capacity(steps.len());
            for (i, (step_tool, template)) in steps.iter().enumerate() {
                let step_args =
                    crate::skill_forge::substitute_args(template, &args).to_string();
                let result =
                    Box::pin(self.execute_tool_inner(step_tool, &step_args, true, taint))
                        .await;
                match result {
                    Ok(r) if r.success => {
                        outputs.push(format!("[step {} {}] {}", i + 1, step_tool, r.output));
                    }
                    Ok(r) => {
                        self.record_staged_run(&staged_skill, false);
                        return Ok(crate::tools::traits::ToolResult::err(format!(
                            "Sequence '{}' failed at step {} ({}): {}",
                            name,
                            i + 1,
                            step_tool,
                            r.error.as_deref().unwrap_or("tool error")
                        )));
                    }
                    Err(e) => {
                        self.record_staged_run(&staged_skill, false);
                        return Ok(crate::tools::traits::ToolResult::err(format!(
                            "Sequence '{}' failed at step {} ({}): {}",
                            name,
                            i + 1,
                            step_tool,
                            e
                        )));
                    }
                }
            }
            self.record_staged_run(&staged_skill, true);
            return Ok(crate::tools::traits::ToolResult::ok(outputs.join("\n")));
        }

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

        // Track 0 dynamic trust: quarantine physical actions from an untrusted
        // node, then feed the per-node behavioral score from this round-trip.
        let risk = tool.risk_class();
        let node_id = args
            .get("node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("local")
            .to_string();
        if risk.physical {
            if let Some(scorer) = &self.trust {
                if matches!(trust::gate(scorer.level(&node_id), risk), TrustGate::Deny) {
                    tracing::warn!(
                        tool = %name,
                        node = %node_id,
                        "Physical action denied: node is untrusted (Track 0 dynamic trust)"
                    );
                    return Ok(crate::tools::traits::ToolResult::err(format!(
                        "Tool '{}' denied: node '{}' is untrusted",
                        name, node_id
                    )));
                }
            }
        }

        // Approval policy: gate the call by the autonomy level + auto-approve list +
        // session/forever grants (and dynamic trust, via decide()). In this
        // autonomous loop a tool that needs operator approval is refused rather than
        // blocking on a prompt; Full autonomy and granted/auto-approved tools pass.
        if let Err(reason) = approval_authorize(self.approval.as_deref(), name, &node_id, risk) {
            tracing::warn!(tool = %name, node = %node_id, reason = %reason, "tool call refused by approval policy");
            return Ok(crate::tools::traits::ToolResult::err(format!(
                "Tool '{}' refused: {}",
                name, reason
            )));
        }

        // Track 0 taint tracking: before a *privileged* call runs, check whether
        // its argument values echo untrusted (external-origin) content pooled
        // earlier this run. This is the CaMeL data-flow guard: fetched web/MCP
        // text must not steer a physical/irreversible action.
        use crate::security::taint::{self, TaintMode};
        if self.taint_mode != TaintMode::Off && taint::gated(risk) {
            if let Some(pool) = taint {
                if let Some(hit) = taint::scan_args(&args, pool) {
                    let granted = self
                        .approval
                        .as_ref()
                        .is_some_and(|a| a.explicitly_granted(name));
                    if let Some(obs) = &self.obs {
                        obs.metrics.counter("taint_hits_total").inc();
                    }
                    if self.taint_mode == TaintMode::Enforce && !granted {
                        if let Some(obs) = &self.obs {
                            obs.metrics.counter("taint_refusals_total").inc();
                        }
                        tracing::warn!(
                            tool = %name, arg = %hit.arg_path, source = %hit.source,
                            "privileged call refused: argument derives from untrusted content (Track 0 taint)"
                        );
                        return Ok(crate::tools::traits::ToolResult::err(format!(
                            "Tool '{}' refused (Track 0 taint): argument '{}' (={:?}) echoes \
                             untrusted content from '{}'. A value derived from external content \
                             may not parameterize a privileged action without an explicit \
                             operator grant.",
                            name, hit.arg_path, hit.value, hit.source
                        )));
                    }
                    tracing::warn!(
                        tool = %name, arg = %hit.arg_path, source = %hit.source, granted,
                        "privileged call has an argument derived from untrusted content (Track 0 taint, advisory)"
                    );
                }
            }
        }

        let started = std::time::Instant::now();
        let result = tool.execute(args).await;
        let success = result.as_ref().map(|r| r.success).unwrap_or(false);
        if let Some(scorer) = &self.trust {
            let latency_ms = started.elapsed().as_millis() as f64;
            scorer.record(&node_id, latency_ms, success);
        }
        self.record_staged_run(&staged_skill, success);

        // Pool successful output from External-trust tools (web, remote MCP, …)
        // so later privileged calls this run can be checked against it.
        if self.taint_mode != TaintMode::Off {
            if let (Some(pool), Ok(r)) = (taint, &result) {
                if r.success
                    && tool.output_trust() == crate::tools::traits::OutputTrust::External
                {
                    pool.add(name, &r.output);
                }
            }
        }
        result
    }

    /// Record the outcome of a supervised-rollout-stage skill run. A failure
    /// auto-demotes the skill back to `simulate` (Track 0: halt on drift) when
    /// the forge directory is attached.
    fn record_staged_run(&self, staged_skill: &Option<String>, success: bool) {
        let Some(skill) = staged_skill else { return };
        if let Some(tracker) = &self.rollout {
            if success {
                tracker.record_clean(skill, RolloutStage::Supervised);
                return;
            }
            tracker.record_failure(skill, RolloutStage::Supervised);
            if let Some(dir) = &self.forge_dir {
                let forge = SkillForge::new(dir.clone());
                match crate::skill_forge::rollout::demote(&forge, tracker, skill) {
                    Ok(stage) => {
                        tracing::warn!(
                            skill = %skill,
                            demoted_to = stage.as_str(),
                            "supervised skill failed a real run — auto-demoted"
                        );
                        self.sync_skills(&forge);
                    }
                    Err(e) => {
                        tracing::warn!(skill = %skill, error = %e, "auto-demotion failed");
                    }
                }
            }
        } else if !success {
            tracing::warn!(
                skill = %skill,
                "supervised skill failed but no rollout tracker is attached"
            );
        }
    }

    /// Execute a tool directly by name with a JSON `Value` argument.
    ///
    /// Bypasses the agent loop — useful for direct tool invocation via the
    /// gateway's `POST /api/v1/tools/{name}` endpoint.
    /// Security policy is still evaluated. No taint pool: a standalone call has
    /// no prior in-run external content to be tainted by.
    pub async fn execute_tool_direct(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<crate::tools::traits::ToolResult> {
        let args_str = args.to_string();
        self.execute_tool(name, &args_str, None).await
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
        reg.iter().map(|t| t.name().to_string()).collect()
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The declared physical-risk of a registered tool (default-safe if unknown).
    pub fn tool_risk(&self, name: &str) -> RiskClass {
        self.find_tool(name)
            .map(|t| t.risk_class())
            .unwrap_or_default()
    }

    /// Clear all conversation history for the given session.
    pub fn clear_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.memory.clear_session(session_id)
    }
}

/// Consult the approval policy for a tool call. `Ok(())` to proceed; `Err(reason)`
/// to refuse — either denied outright by dynamic trust, or (in this autonomous
/// loop) needing operator approval that hasn't been granted. With no manager
/// attached, or under Full autonomy, everything is permitted.
fn approval_authorize(
    approval: Option<&ApprovalManager>,
    tool: &str,
    node_id: &str,
    risk: RiskClass,
) -> std::result::Result<(), String> {
    let Some(approval) = approval else {
        return Ok(());
    };
    match approval.decide(tool, Some(node_id), risk) {
        crate::approval::Decision::Allow => Ok(()),
        crate::approval::Decision::Deny => Err("denied by approval policy".to_string()),
        crate::approval::Decision::NeedsApproval => Err(
            "requires operator approval (autonomy is supervised/manual and it is not auto-approved or granted)"
                .to_string(),
        ),
    }
}

/// Merge a delegate skill's fixed args with the runtime args (runtime wins).
fn merge_delegate_args(fixed: Value, runtime: &Value) -> Value {
    let mut merged = fixed;
    if let (Some(m), Some(a)) = (merged.as_object_mut(), runtime.as_object()) {
        for (k, v) in a {
            m.insert(k.clone(), v.clone());
        }
    }
    merged
}

/// Human/model-readable description of what a simulate-stage skill *would*
/// execute — used in the dry-run result so the trace is auditable.
fn describe_simulation(tool: &Arc<dyn Tool>, args: &Value) -> String {
    if let Some((target, fixed)) = tool.as_delegate() {
        let merged = merge_delegate_args(fixed, args);
        format!("would call tool '{}' with args {}", target, merged)
    } else if let Some(steps) = tool.as_sequence() {
        let rendered: Vec<String> = steps
            .iter()
            .enumerate()
            .map(|(i, (t, tmpl))| {
                let concrete = crate::skill_forge::substitute_args(tmpl, args);
                format!("step {} → {}({})", i + 1, t, concrete)
            })
            .collect();
        format!("would run {} steps: {}", rendered.len(), rendered.join("; "))
    } else {
        format!("would execute with args {}", args)
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

    fn autonomy(level: crate::config::AutonomyLevel, auto_approve: Vec<String>) -> crate::config::AutonomyConfig {
        crate::config::AutonomyConfig { level, auto_approve, always_ask: vec![] }
    }
    fn approval_mgr(cfg: &crate::config::AutonomyConfig) -> ApprovalManager {
        let path = std::env::temp_dir()
            .join(format!("obc_agent_grants_{}.json", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        ApprovalManager::with_grants(cfg, crate::approval::ForeverGrants::load(path), false)
    }

    #[test]
    fn approval_no_manager_permits_everything() {
        assert!(approval_authorize(None, "shell", "local", RiskClass::safe()).is_ok());
    }

    #[test]
    fn approval_full_autonomy_permits() {
        let mgr = approval_mgr(&autonomy(crate::config::AutonomyLevel::Full, vec![]));
        assert!(approval_authorize(Some(&mgr), "shell", "local", RiskClass::safe()).is_ok());
    }

    #[test]
    fn approval_supervised_refuses_ungranted_tool() {
        let mgr = approval_mgr(&autonomy(crate::config::AutonomyLevel::Supervised, vec![]));
        let err = approval_authorize(Some(&mgr), "shell", "local", RiskClass::safe());
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("approval"));
    }

    #[test]
    fn approval_supervised_permits_auto_approved_tool() {
        let mgr = approval_mgr(&autonomy(
            crate::config::AutonomyLevel::Supervised,
            vec!["sensor_read".to_string()],
        ));
        assert!(approval_authorize(Some(&mgr), "sensor_read", "local", RiskClass::safe()).is_ok());
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

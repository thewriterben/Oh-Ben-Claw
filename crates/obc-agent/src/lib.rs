//! Oh-Ben-Claw core agent — the central reasoning and orchestration engine.
//!
//! The agent loop receives a user message, builds the conversation context,
//! calls the LLM, executes any requested tool calls, feeds results back to the
//! LLM, and repeats until the model produces a final text response.

pub mod a2a_executor;
pub mod delegation_tools;
pub mod edge;
pub mod handle;
pub mod judge;
pub mod notify;
pub mod orchestrator;
pub mod pool;
/// System 1 — the reflex rule language, engine, escalation budget and action
/// sink — extracted to [`obc_reflex`] on 2026-08-12.
///
/// It named four things outside itself and three had been crates for days. The
/// fourth was `obc_spine`, entirely inside `SpineActionSink`, which moved to
/// [`obc_spine::action`] the same day and implements this crate's trait from
/// there.
///
/// This alias is *not* the compatibility shim it looks like. Thirteen import
/// sites across seven modules were repointed at `obc_reflex` directly in the
/// same commit, because a re-export keeps a build green while leaving the
/// dependency graph identical — see `docs/ENDGAME.md`, step 1. What is left
/// here is the four sites inside `agent` itself, where reaching for a crate to
/// find this module's own vocabulary would be the wrong shape, and `src/main.rs`.
pub use obc_reflex as reflex;
// The reflexion loop and plan-and-execute orchestration lived here until
// 2026-08-21, when they were removed for never having been called.
//
// `reflexion_loop`, `create_plan` and `synthesize_results` had no caller
// anywhere in the workspace — not `main.rs`, not a test, not a bench. The
// patterns were implemented, documented, and never wired to anything, and
// `docs/architecture/ARCHITECTURE.md` carried a ✅ for "Reflexion /
// Plan-and-Execute" in the table comparing this project to ZeroClaw. A tick in
// a comparison table is a claim in its strongest form: it is read as settled,
// it is read fast, and it is trusted *because* it does not argue.
//
// `scripts/file_reachability.py` had been reporting all three as claimed by
// that document and reachable from nothing, in its own output, for as long as
// ARCHITECTURE.md has been in its document set. The instrument was right and
// nobody acted on it, which is the more useful half of this note.
//
// One function survived, because it was never dead: `parse_quality_score` is
// the judge's score parser and always was — `judge.rs` called it across the
// module boundary. It is `pub(crate)`, which is why a *public*-API survey
// reported this file as entirely unreferenced, and why a disclosure saying so
// was written and was wrong. It now sits in `judge`, next to its only caller.

/// The standard safing rule set, moved to [`obc_reflex::safing`] on 2026-08-20.
///
/// The alias above is why this took eight days longer than it should have.
/// `safing.rs` imported `crate::reflex::{Action, ActionSink, Cmp, Condition,
/// ReflexRule}` — five names that read like this crate's and are all declared
/// in `obc-reflex`. One line, and the module looked agent-owned to every
/// instrument that reads imports. It named nothing else here.
///
/// Its fourteen tests came with it. The two suites they drive — telemetry and
/// audio — are `obc-reflex` dev-dependencies now, which is legal because
/// neither depends on it: `perceive → remember → reflex` runs one way.
///
/// Re-exported rather than repointed, unlike the thirteen sites in the note
/// above, because this one has a caller outside the workspace's control: the
/// public path is `oh_ben_claw::agent::safing`, and `src/main.rs`, four
/// integration tests and OBC-Prime's playbook all spell it that way.
pub use obc_reflex::safing;
/// The `ReplayExecutor` impl for [`Agent`], moved here from `skill_forge` on
/// 2026-08-13. It was that module's only reference to this one, and therefore
/// the whole `agent <-> skill_forge` cycle.
///
/// Private, and correctly so: a trait impl is visible wherever both the trait
/// and the type are, regardless of the module it is written in. Nothing needs
/// to name this module — which is also why forgetting to declare it failed at
/// the *use* site in main.rs rather than here.
mod skill_replay;
// Streaming tool-call accumulation and response building lived here until
// 2026-08-21, when both halves of the feature were removed for never having
// been connected to anything.
//
// `StreamingToolCallAccumulator` and `StreamingResponseBuilder` were public,
// compiled, and referenced by nothing outside this file. The provider half was
// worse: `crates/obc-providers/src/streaming.rs` -- the SSE parsers those
// types existed to consume -- was never listed in a `mod` declaration, so it
// was never compiled at all, and its six tests have never run. Neither
// `cargo test` nor `cargo clippy -D warnings` had anything to say about 313
// lines sitting in the source tree.
//
// `docs/architecture/ARCHITECTURE.md` ticked "Streaming tool calls" as shipped
// until 2026-08-21. No provider streamed until 2026-09-11; since then the loop
// calls `Provider::chat_completion_streaming` and broadcasts each text delta
// as `AgentEvent::Token`, with `Thinking`, `ToolCall` and `ToolResult` emitted
// from inside the loop as they happen rather than reconstructed afterwards.
pub mod context;
pub mod routing;
pub mod scheduled;
pub mod system2;
pub mod world_context;
pub use edge::{EdgeAgent, EdgeAgentBuilder};
pub use handle::{AgentEvent, AgentHandle};
#[allow(unused_imports)]
pub use orchestrator::{OrchestratorAgent, OrchestratorConfig, RoutingStrategy};
#[allow(unused_imports)]
pub use pool::{AgentPool, SubAgentInfo, SubAgentSpec, SubAgentStatus};

use anyhow::Result;
use obc_approval::ApprovalManager;
use obc_memory::trajectory::{Episode, EpisodeStep, Outcome, TrajectoryStore};
use obc_memory::MemoryStore;
use obc_providers::{ChatMessage, ChatRole, Provider};
use obc_safety::audit::ActionAuditor;
use obc_safety::limits::SafetyGate;
use obc_safety::trust::{self, TrustGate, TrustScorer};
use obc_safety::PolicyEngine;
use obc_skill_forge::rollout::RolloutTracker;
use obc_skill_forge::{SkillForge, SkillTool};
use obc_tool_api::{RiskClass, RolloutStage, Tool};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

/// Maximum tool-use iterations per user message to prevent runaway loops.
pub const MAX_TOOL_ITERATIONS: usize = 10;

/// Maximum conversation history messages to include in each LLM call.
pub const MAX_HISTORY_MESSAGES: usize = 50;

// ── Agent ─────────────────────────────────────────────────────────────────────

/// The core Oh-Ben-Claw agent.
/// Capacity of the agent event broadcast channel. A slow subscriber lags
/// (and is told so) rather than stalling the loop.
pub const EVENT_CHANNEL_CAPACITY: usize = 512;

pub struct Agent {
    /// Live events from inside `process()`: tokens as they stream, tool calls
    /// as they dispatch. `AgentHandle` and the gateway subscribe here.
    events: tokio::sync::broadcast::Sender<AgentEvent>,
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
    obs: Option<Arc<obc_observability::ObsContext>>,
    /// Track 0: deterministic, model-independent safety limits applied to
    /// physical tool calls before execution.
    safety: Option<Arc<SafetyGate>>,
    /// Track 0: tamper-evident audit log of physical-action decisions.
    auditor: Option<Arc<Mutex<ActionAuditor>>>,
    /// World memory, for the state preamble. Read-only from here: the agent's own
    /// writes go through the `world_memory` tool, which stamps provenance the agent
    /// cannot forge.
    world: Option<Arc<obc_memory::world::WorldMemory>>,
    /// How much of that state to render, and which.
    world_context: world_context::WorldContextConfig,
    /// Phase 16: when attached, each run is captured as an `Episode` for
    /// experiential self-improvement.
    trajectory: Option<Arc<TrajectoryStore>>,
    /// Session-id prefixes whose turns are not captured as trajectories.
    trajectory_skip: Vec<String>,
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
    /// Parity item 3: a second, cloud brain chosen per turn. `None` means
    /// every turn uses `provider`, as before 2026-09-11.
    routing: Option<Routing>,
    /// Parity item 4: the agent's two bounded note files, appended to the
    /// system prompt (so they sit in the cached prefix and change rarely).
    notes: Option<Arc<obc_memory::notes::Notes>>,
    /// Parity item 4: every invocation of a forge-managed skill is recorded
    /// here so the curator knows which skills earn their context rent.
    skill_usage: Option<Arc<obc_skill_forge::usage::UsageLedger>>,
    /// Phase 15/9: token cost tracking — `(tracker, in_price/M, out_price/M)`.
    /// Each run records an estimated `TokenUsage` (chars/4 heuristic, same as
    /// episode metrics) so the gateway can show a live cost summary.
    cost: Option<(Arc<obc_cost::CostTracker>, f64, f64)>,
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
    taint_mode: obc_safety::taint::TaintMode,
    /// How many recent messages to replay into context. Defaults to
    /// [`MAX_HISTORY_MESSAGES`]; an edge device lowers it to bound RAM. This used to
    /// be the constant, read directly, which is why `edge.max_history_messages` could
    /// be documented, generated into every NanoPi config by the deployment planner,
    /// and have no effect whatsoever.
    max_history: usize,
}

/// The router's runtime state: the cloud brain, its back-off, today's spend.
struct Routing {
    cfg: obc_providers::RoutingConfig,
    cloud: Arc<dyn Provider>,
    cloud_down_until: Mutex<Option<std::time::Instant>>,
    /// `(day number since the epoch, estimated USD spent on cloud turns that day)`.
    cloud_spend: Mutex<(u64, f64)>,
}

impl Routing {
    fn cooling(&self) -> bool {
        self.cloud_down_until
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some_and(|until| until > std::time::Instant::now())
    }

    fn mark_down(&self) {
        *self
            .cloud_down_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(
            std::time::Instant::now()
                + std::time::Duration::from_secs(self.cfg.offline_backoff_secs),
        );
    }

    fn today() -> u64 {
        now_ms() / 86_400_000
    }

    fn spent_today(&self) -> f64 {
        let g = self.cloud_spend.lock().unwrap_or_else(|p| p.into_inner());
        if g.0 == Self::today() {
            g.1
        } else {
            0.0
        }
    }

    fn add_spend(&self, usd: f64) {
        let mut g = self.cloud_spend.lock().unwrap_or_else(|p| p.into_inner());
        let today = Self::today();
        if g.0 != today {
            *g = (today, 0.0);
        }
        g.1 += usd;
    }

    fn budget_exceeded(&self) -> bool {
        self.cfg.daily_budget_usd > 0.0 && self.spent_today() >= self.cfg.daily_budget_usd
    }
}

impl Agent {
    /// Create a new agent.
    pub fn new(
        config: AgentConfig,
        provider: Arc<dyn Provider>,
        memory: Arc<MemoryStore>,
        tools: Vec<Box<dyn Tool>>,
    ) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            events,
            config,
            provider,
            memory,
            tools: RwLock::new(tools.into_iter().map(Arc::from).collect()),
            skill_names: Mutex::new(HashSet::new()),
            policy: None,
            obs: None,
            safety: None,
            auditor: None,
            // Off until wired: an agent built without world memory sees exactly what it
            // saw before, so this cannot change behaviour for a caller that has not
            // asked for it.
            world: None,
            world_context: world_context::WorldContextConfig::default(),
            trajectory: None,
            trajectory_skip: vec!["scheduled-".to_string()],
            trust: None,
            approval: None,
            experience_k: None,
            routing: None,
            notes: None,
            skill_usage: None,
            cost: None,
            rollout: None,
            forge_dir: None,
            taint_mode: obc_safety::taint::TaintMode::Off,
            max_history: MAX_HISTORY_MESSAGES,
        }
    }

    /// Bound how much conversation history is replayed into context.
    ///
    /// Zero is rejected rather than honoured: an agent that cannot see its own last
    /// turn is broken in a way that looks like the model being stupid, and the caller
    /// almost certainly meant "default".
    pub fn with_max_history(mut self, n: usize) -> Self {
        if n > 0 {
            self.max_history = n;
        }
        self
    }

    /// Attach a policy engine to enforce tool execution policies.
    pub fn with_policy(mut self, policy: PolicyEngine) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Attach an observability context (spans + counters per run).
    pub fn with_obs(mut self, obs: Arc<obc_observability::ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    /// Attach a Track 0 safety gate enforcing deterministic limits on physical
    /// tool calls (pin allow-list, value range, rate).
    pub fn with_safety_gate(mut self, gate: Arc<SafetyGate>) -> Self {
        self.safety = Some(gate);
        self
    }

    /// Let the agent see what it currently believes about the physical world.
    ///
    /// Without this, [`Agent::build_context`] is the system prompt and the last 50
    /// messages and nothing else — the world model exists, and the thing reasoning on
    /// top of it cannot see any of it. It can query `world_memory`, but only if
    /// something prompts it to, and nothing does.
    pub fn with_world_context(
        mut self,
        world: Arc<obc_memory::world::WorldMemory>,
        cfg: world_context::WorldContextConfig,
    ) -> Self {
        self.world = Some(world);
        self.world_context = cfg;
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

    /// Do not capture turns from sessions whose id starts with any of these
    /// (`[self_improvement] skip_session_prefixes`).
    pub fn with_trajectory_skip(mut self, prefixes: Vec<String>) -> Self {
        self.trajectory_skip = prefixes;
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
        tracker: Arc<obc_cost::CostTracker>,
        input_price_per_million: f64,
        output_price_per_million: f64,
    ) -> Self {
        self.cost = Some((tracker, input_price_per_million, output_price_per_million));
        self
    }

    /// Attach the skill usage ledger (see `obc_skill_forge::curator`).
    pub fn with_skill_usage(mut self, ledger: Arc<obc_skill_forge::usage::UsageLedger>) -> Self {
        self.skill_usage = Some(ledger);
        self
    }

    /// Attach the note files. Their text follows the system prompt in the same
    /// system message: part of the cached prefix, invalidated only when a note
    /// changes.
    pub fn with_notes(mut self, notes: Arc<obc_memory::notes::Notes>) -> Self {
        self.notes = Some(notes);
        self
    }

    /// Attach the per-turn router (`[provider.routing]`). Builds the cloud
    /// provider from its config. Without a key for it, nothing is attached
    /// and one warning says so: every turn stays local until the key exists
    /// and the agent restarts.
    pub fn with_routing(self, cfg: obc_providers::RoutingConfig) -> Result<Self> {
        if !cfg.enabled {
            tracing::info!("routing: disabled in config; every turn stays local");
            return Ok(self);
        }
        if !obc_providers::key_present(&cfg.cloud) {
            tracing::warn!(
                provider = %cfg.cloud.name,
                model = %cfg.cloud.model,
                var = obc_providers::key_env_var(&cfg.cloud.name).unwrap_or("its API key"),
                "routing: the cloud brain has no API key; every turn stays local until it is set and OBC restarts"
            );
            return Ok(self);
        }
        let cloud = obc_providers::from_config(&cfg.cloud)?;
        Ok(self.with_routing_provider(cfg, cloud))
    }

    /// [`with_routing`](Self::with_routing) with the cloud provider supplied.
    pub fn with_routing_provider(
        mut self,
        cfg: obc_providers::RoutingConfig,
        cloud: Arc<dyn Provider>,
    ) -> Self {
        tracing::info!(
            cloud = %format!("{}/{}", cfg.cloud.name, cfg.cloud.model),
            local = "[provider]",
            console_to_cloud = cfg.console_to_cloud,
            daily_budget_usd = cfg.daily_budget_usd,
            "routing: two brains, chosen per turn"
        );
        self.routing = Some(Routing {
            cfg,
            cloud,
            cloud_down_until: Mutex::new(None),
            cloud_spend: Mutex::new((0, 0.0)),
        });
        self
    }

    /// Decide which brain answers this turn. See [`routing::decide`].
    pub fn route_turn(
        &self,
        session_id: &str,
        tool_count: usize,
    ) -> (routing::Route, &'static str) {
        let Some(r) = &self.routing else {
            return (routing::Route::Local, "no router");
        };
        let private = self
            .world
            .as_ref()
            .and_then(|w| world_context::context_facts(w, &self.world_context, now_ms()))
            .map(|(facts, withdrawn)| {
                routing::private_facts(
                    facts
                        .iter()
                        .take(self.world_context.max_facts)
                        .chain(withdrawn.iter().take(self.world_context.max_withdrawals)),
                    &r.cfg,
                )
            })
            .unwrap_or(false);
        routing::decide(
            &r.cfg,
            &routing::TurnFacts {
                session_id,
                tool_count,
                private_facts: private,
                cloud_cooling: r.cooling(),
                budget_exceeded: r.budget_exceeded(),
            },
        )
    }

    /// One model call on the chosen brain. A cloud failure is answered locally
    /// in the same call — the client is told to discard what it rendered — and
    /// starts the back-off, so the rest of the turn and the next
    /// `offline_backoff_secs` stay local. Returns the completion and whether
    /// the cloud produced it.
    async fn complete_routed(
        &self,
        route: routing::Route,
        messages: &[ChatMessage],
        tools: &[Box<dyn Tool>],
        local_config: &obc_providers::ProviderConfig,
        sink: obc_providers::DeltaSink<'_>,
    ) -> Result<(obc_providers::ChatCompletion, bool)> {
        if let (routing::Route::Cloud, Some(r)) = (route, &self.routing) {
            if !r.cooling() {
                match r
                    .cloud
                    .chat_completion_streaming(messages, tools, &r.cfg.cloud, sink)
                    .await
                {
                    Ok(c) => return Ok((c, true)),
                    Err(e) => {
                        r.mark_down();
                        tracing::warn!(
                            error = %e,
                            backoff_secs = r.cfg.offline_backoff_secs,
                            "routing: cloud turn failed; answering locally and backing off"
                        );
                        if let Some(obs) = &self.obs {
                            obs.metrics.counter("routing_cloud_failures_total").inc();
                        }
                        sink(obc_providers::StreamDelta::Restart);
                    }
                }
            }
        }
        let c = self
            .provider
            .chat_completion_streaming(messages, tools, local_config, sink)
            .await?;
        Ok((c, false))
    }

    /// Write the decision to world memory as `agent.brain` when it changed, so
    /// the world-state block says which brain is answering and why.
    fn record_brain(&self, route: routing::Route, reason: &str, provider: &str, model: &str) {
        let Some(world) = &self.world else { return };
        let value = serde_json::json!({
            "route": route.as_str(), "provider": provider, "model": model, "reason": reason,
        });
        let unchanged = world
            .current("agent.brain")
            .ok()
            .flatten()
            .is_some_and(|f| f.value == value);
        if unchanged {
            return;
        }
        let now = now_ms();
        if let Err(e) = world.observe_as(
            "agent.brain",
            value,
            now,
            now,
            "router",
            obc_memory::world::Origin::Derived,
        ) {
            tracing::debug!(error = %e, "routing: could not record agent.brain");
        }
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
    pub fn with_taint_mode(mut self, mode: obc_safety::taint::TaintMode) -> Self {
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

    /// Subscribe to live events (`Token`, `Thinking`, `ToolCall`, `ToolResult`).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    /// The sender behind [`Agent::subscribe`], for a handle that wants to emit
    /// `Started` / `Response` / `Error` on the same bus.
    pub fn event_sender(&self) -> tokio::sync::broadcast::Sender<AgentEvent> {
        self.events.clone()
    }

    fn emit(&self, event: AgentEvent) {
        // No subscribers is the normal state for a CLI-only agent; not an error.
        let _ = self.events.send(event);
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
        provider_config: &obc_providers::ProviderConfig,
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
        let taint_pool = (self.taint_mode != obc_safety::taint::TaintMode::Off)
            .then(obc_safety::taint::TaintPool::new);

        // 1. Store the user message. The session row must exist first: since
        //    `PRAGMA foreign_keys` went on (2026-09-11, #146) a message for an
        //    unknown session is refused, and callers hand this method fresh ids
        //    all the time — a channel's chat id, a scheduled task's own
        //    session, a Command Center tab. Idempotent, one cheap statement.
        self.memory.create_session_with_id(session_id)?;
        self.memory
            .append_message(session_id, ChatRole::User, user_message)?;

        // 2. Fold old history if the window is filling, then build the context
        //    in cache-stable order (system, history, ephemeral blocks, the ask).
        if let Err(e) = self.compact_if_needed(session_id, provider_config).await {
            // A failed summary must not cost the turn; the raw window still works.
            tracing::warn!(session_id = %session_id, error = %e, "compaction failed; using raw history");
        }
        let mut messages = self.build_context_for(session_id, Some(user_message))?;

        let max_iterations = self.config.max_tool_iterations.min(MAX_TOOL_ITERATIONS);
        let mut tool_calls_made = Vec::new();
        let mut final_response = String::new();

        // Stable tool set for this run (hot-added skills apply from the next run).
        let tool_list = self.tools_snapshot();

        // Which brain answers this turn (parity item 3). Decided once per turn;
        // a cloud failure mid-turn falls back to local inside `complete_routed`.
        let (route, route_reason) = self.route_turn(session_id, tool_list.len());
        if self.routing.is_some() {
            tracing::info!(session_id = %session_id, route = route.as_str(), reason = route_reason, "routing");
        }
        let mut used_cloud = false;
        // Real token numbers when the provider reports them (Anthropic, Ollama);
        // summed over the turn's iterations. Otherwise the chars/4 guess below.
        let mut turn_usage = obc_providers::Usage::default();
        let mut usage_known = false;
        let mut brain: Option<(String, String)> = None;

        // 3–6. Agent loop
        for iteration in 0..max_iterations {
            tracing::debug!(
                session_id = %session_id,
                iteration = iteration,
                message_count = messages.len(),
                "Agent loop iteration"
            );

            if iteration > 0 {
                // The model has acted on older tool results; what it needs now
                // is that they happened, not their bytes.
                let stubbed = context::stub_old_tool_outputs(&mut messages, 2);
                if stubbed > 0 {
                    tracing::debug!(session_id = %session_id, stubbed, "old tool outputs stubbed");
                }
            }
            self.emit(AgentEvent::Thinking {
                session_id: session_id.to_string(),
                iteration: iteration as u32,
            });
            let sink = |delta: obc_providers::StreamDelta| {
                let (delta, reset) = match delta {
                    obc_providers::StreamDelta::Text(t) => (t, false),
                    obc_providers::StreamDelta::Restart => (String::new(), true),
                };
                self.emit(AgentEvent::Token {
                    session_id: session_id.to_string(),
                    iteration: iteration as u32,
                    delta,
                    reset,
                });
            };
            let (completion, from_cloud) = self
                .complete_routed(route, &messages, &tool_list, provider_config, &sink)
                .await?;
            used_cloud |= from_cloud;
            brain = Some((completion.provider.clone(), completion.model.clone()));
            if let Some(u) = &completion.usage {
                turn_usage.add(u);
                usage_known = true;
            }

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
                if let Some(ledger) = &self.skill_usage {
                    let is_skill = self
                        .skill_names
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .contains(&call.name);
                    if is_skill {
                        ledger.record(&call.name);
                    }
                }

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

                self.emit(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    args: serde_json::from_str(&call.args).unwrap_or(serde_json::Value::Null),
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

                self.emit(AgentEvent::ToolResult {
                    session_id: session_id.to_string(),
                    call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: matches!(&result, Ok(r) if r.success),
                    output: result_str.clone(),
                    duration_ms,
                });
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
                let sink = |delta: obc_providers::StreamDelta| {
                    let (delta, reset) = match delta {
                        obc_providers::StreamDelta::Text(t) => (t, false),
                        obc_providers::StreamDelta::Restart => (String::new(), true),
                    };
                    self.emit(AgentEvent::Token {
                        session_id: session_id.to_string(),
                        iteration: max_iterations as u32,
                        delta,
                        reset,
                    });
                };
                let (final_completion, from_cloud) = self
                    .complete_routed(route, &messages, &[], provider_config, &sink)
                    .await?;
                used_cloud |= from_cloud;
                brain = Some((
                    final_completion.provider.clone(),
                    final_completion.model.clone(),
                ));
                if let Some(u) = &final_completion.usage {
                    turn_usage.add(u);
                    usage_known = true;
                }
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
                + tool_calls_made
                    .iter()
                    .map(|tc| tc.result.len())
                    .sum::<usize>();
            (chars / 4) as u64
        };
        let output_est = {
            let chars = final_response.len()
                + tool_calls_made
                    .iter()
                    .map(|tc| tc.args.len())
                    .sum::<usize>();
            (chars / 4) as u64
        };

        // Parity item 3: say which brain answered, and charge the cloud budget.
        if let Some((provider_name, model)) = &brain {
            if self.routing.is_some() {
                // What actually answered, not what was decided: a cloud turn
                // that failed over mid-way was answered locally.
                let (answered, why) = match (route, used_cloud) {
                    (routing::Route::Cloud, false) => {
                        (routing::Route::Local, "cloud unreachable, backing off")
                    }
                    _ => (route, route_reason),
                };
                self.record_brain(answered, why, provider_name, model);
            }
        }
        let (model_used, in_price, out_price) = match (&self.routing, used_cloud) {
            (Some(r), true) => (
                r.cfg.cloud.model.clone(),
                r.cfg.cloud_input_price_per_million,
                r.cfg.cloud_output_price_per_million,
            ),
            _ => (
                provider_config.model.clone(),
                self.cost.as_ref().map(|c| c.1).unwrap_or(0.0),
                self.cost.as_ref().map(|c| c.2).unwrap_or(0.0),
            ),
        };
        // Bill from the provider's numbers when it gave them: cache reads at
        // 10%, cache writes at 125%, the rest at list. The estimate is the
        // fallback, not the default.
        let (input_billed, output_billed) = if usage_known {
            (
                turn_usage.billable_input().round() as u64,
                turn_usage.output_tokens,
            )
        } else {
            (input_est, output_est)
        };
        let turn_cost =
            input_billed as f64 * in_price / 1e6 + output_billed as f64 * out_price / 1e6;
        if usage_known {
            tracing::info!(
                session_id = %session_id,
                provider = brain.as_ref().map(|b| b.0.as_str()).unwrap_or(""),
                model = %model_used,
                prompt = turn_usage.prompt_tokens(),
                uncached = turn_usage.input_tokens,
                cache_read = turn_usage.cache_read_input_tokens,
                cache_write = turn_usage.cache_creation_input_tokens,
                output = turn_usage.output_tokens,
                cache_hit = format!("{:.0}%", turn_usage.cache_hit_ratio().unwrap_or(0.0) * 100.0),
                cost_usd = format!("{turn_cost:.5}"),
                "brain usage"
            );
        }
        if used_cloud {
            if let Some(r) = &self.routing {
                r.add_spend(turn_cost);
            }
        }

        // Phase 15/9: record usage against the cost budget.
        if let Some((tracker, _, _)) = &self.cost {
            tracker.record_usage(obc_cost::TokenUsage::new(
                model_used.clone(),
                input_billed,
                output_billed,
                in_price,
                out_price,
            ));
        }

        // Phase 16: capture this run as an episode for experiential self-improvement.
        // Not for sessions the operator did not speak in (timers by default): the
        // bench learned a skill from a scheduled turn and then replayed the timer.
        let capture = self.trajectory.as_ref().filter(|_| {
            !self
                .trajectory_skip
                .iter()
                .any(|p| session_id.starts_with(p.as_str()))
        });
        if let Some(traj) = capture {
            let steps: Vec<EpisodeStep> = tool_calls_made
                .iter()
                .map(|tc| EpisodeStep {
                    tool: tc.name.clone(),
                    args: serde_json::from_str(&tc.args).unwrap_or_else(|_| serde_json::json!({})),
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
            provider: brain.as_ref().map(|b| b.0.clone()).unwrap_or_default(),
            model: brain.as_ref().map(|b| b.1.clone()).unwrap_or_default(),
            usage: usage_known.then_some(turn_usage),
        })
    }

    /// Build the "learned experience" system block for an objective: up to `k`
    /// relevant registered learned skills and `k` similar past successful
    /// episodes, both ranked by deterministic token overlap. `None` when
    /// nothing relevant is known — no prompt noise on novel tasks.
    fn experience_block(&self, objective: &str, k: usize) -> Option<String> {
        use obc_memory::trajectory::lexical_score;
        const MIN_SCORE: f32 = 0.2;

        // Relevant learned skills currently registered as tools.
        let mut skills: Vec<(f32, String, String)> = {
            let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
            reg.iter()
                .filter(|t| t.name().starts_with("learned_"))
                .filter_map(|t| {
                    // Match on the skill name (de-slugged) + description.
                    let haystack = format!("{} {}", t.name().replace('_', " "), t.description());
                    let s = lexical_score(objective, &haystack);
                    (s >= MIN_SCORE).then(|| (s, t.name().to_string(), t.description().to_string()))
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
    /// The context with no objective (no experience block); what the tests
    /// exercise directly.
    #[cfg(test)]
    fn build_context(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        self.build_context_for(session_id, None)
    }

    /// The prompt for one turn, in cache-stable order:
    ///
    /// 1. the system prompt (never changes);
    /// 2. the history — the latest persisted summary, if any, then every message
    ///    after it, up to `max_history` rows (only grows at the end);
    /// 3. the ephemeral blocks — experience for this objective, then what the
    ///    agent currently believes about the world — regenerated every turn;
    /// 4. the user's latest message.
    ///
    /// Until 2026-09-11 the world state came second, so every turn's prompt
    /// differed from the last one two messages in, and neither Ollama's prompt
    /// cache nor Anthropic's prompt caching could reuse anything past the
    /// system prompt. The world state is still its own system message, not
    /// spliced into the prompt: the prompt is who the agent is; this is what is
    /// true right now, and the boundary stays visible in a transcript.
    fn build_context_for(
        &self,
        session_id: &str,
        objective: Option<&str>,
    ) -> Result<Vec<ChatMessage>> {
        let system = match self.notes.as_ref().and_then(|n| n.render()) {
            Some(notes) => format!("{}\n\n{notes}", self.config.system_prompt.trim_end()),
            None => self.config.system_prompt.clone(),
        };
        let mut messages = vec![ChatMessage {
            role: ChatRole::System,
            content: system,
        }];

        let stored = self
            .memory
            .load_recent_stored(session_id, self.max_history)?;
        messages.extend(context::assemble_history(&stored));

        let mut ephemeral = Vec::new();
        // Phase 16 P1: verified experience (learned skills + similar past
        // successes) for this objective.
        if let (Some(k), Some(objective)) = (self.experience_k, objective) {
            if let Some(block) = self.experience_block(objective, k) {
                if let Some(obs) = &self.obs {
                    obs.metrics
                        .counter("experience_blocks_injected_total")
                        .inc();
                }
                ephemeral.push(ChatMessage {
                    role: ChatRole::System,
                    content: block,
                });
            }
        }
        if let Some(world) = &self.world {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if let Some(state) = world_context::render(world, &self.world_context, now) {
                ephemeral.push(ChatMessage {
                    role: ChatRole::System,
                    content: state,
                });
            }
        }

        Ok(context::with_ephemeral_tail(messages, ephemeral))
    }

    /// Fold older history into a persisted summary when the history has grown
    /// past `compaction_threshold × context_tokens`. Returns whether it did.
    ///
    /// One model call (no tools, non-streaming) per compaction, so at most one
    /// every `keep_tail`-plus messages; the summary is appended to the session
    /// as a system message carrying the id it covers through, and
    /// [`context::assemble_history`] puts it first. Nothing is deleted: the
    /// rows it summarises stay in `memory.db` for search and export.
    pub async fn compact_if_needed(
        &self,
        session_id: &str,
        provider_config: &obc_providers::ProviderConfig,
    ) -> Result<bool> {
        if !self.config.compaction {
            return Ok(false);
        }
        let stored = self
            .memory
            .load_recent_stored(session_id, self.max_history)?;
        let history = context::assemble_history(&stored);
        let budget =
            (self.config.context_tokens as f64 * self.config.compaction_threshold) as usize;
        let used = context::estimate_tokens(&history) + self.config.system_prompt.len() / 4;
        if used <= budget {
            return Ok(false);
        }
        let Some((a, b)) = context::compaction_range(&stored, self.config.compaction_keep_tail)
        else {
            return Ok(false);
        };
        let previous = stored
            .iter()
            .rev()
            .find(|m| m.role == "system" && context::summary_cursor(&m.content).is_some())
            .and_then(|s| s.content.split_once('\n').map(|(_, rest)| rest.to_string()));
        let span: Vec<&obc_memory::StoredMessage> = stored[a..=b]
            .iter()
            .filter(|m| !(m.role == "system" && context::summary_cursor(&m.content).is_some()))
            .collect();
        let through = stored[b].id;
        let prompt = context::summarizer_input(previous.as_deref(), &span);
        let completion = self
            .provider
            .chat_completion(&prompt, &[], provider_config)
            .await?;
        if completion.message.trim().is_empty() {
            tracing::warn!(session_id = %session_id, "compaction: summariser returned nothing; keeping raw history");
            return Ok(false);
        }
        self.memory.append_message(
            session_id,
            ChatRole::System,
            &context::summary_message(through, &completion.message),
        )?;
        if let Some(obs) = &self.obs {
            obs.metrics.counter("context_compactions_total").inc();
        }
        tracing::info!(
            session_id = %session_id,
            folded = span.len(),
            through_id = through,
            used_tokens = used,
            budget_tokens = budget,
            "context compacted into a summary"
        );
        Ok(true)
    }

    /// Execute a tool by name with the given JSON arguments string.
    ///
    /// Evaluates the security policy before executing. Denied tool calls
    /// return an error immediately without invoking the tool.
    async fn execute_tool(
        &self,
        name: &str,
        args_str: &str,
        taint: Option<&obc_safety::taint::TaintPool>,
    ) -> Result<obc_tool_api::ToolResult> {
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
        taint: Option<&obc_safety::taint::TaintPool>,
    ) -> Result<obc_tool_api::ToolResult> {
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
                    return Ok(obc_tool_api::ToolResult::err(format!(
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
                    return Ok(obc_tool_api::ToolResult::ok(format!(
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
                        return Ok(obc_tool_api::ToolResult::err(format!(
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
                        return Ok(obc_tool_api::ToolResult::err(format!(
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
                return Ok(obc_tool_api::ToolResult::err(format!(
                    "Sequence skill '{}' cannot run inside another sequence",
                    name
                )));
            }
            let mut outputs = Vec::with_capacity(steps.len());
            for (i, (step_tool, template)) in steps.iter().enumerate() {
                let step_args = obc_skill_forge::substitute_args(template, &args).to_string();
                let result =
                    Box::pin(self.execute_tool_inner(step_tool, &step_args, true, taint)).await;
                match result {
                    Ok(r) if r.success => {
                        outputs.push(format!("[step {} {}] {}", i + 1, step_tool, r.output));
                    }
                    Ok(r) => {
                        self.record_staged_run(&staged_skill, false);
                        return Ok(obc_tool_api::ToolResult::err(format!(
                            "Sequence '{}' failed at step {} ({}): {}",
                            name,
                            i + 1,
                            step_tool,
                            r.error.as_deref().unwrap_or("tool error")
                        )));
                    }
                    Err(e) => {
                        self.record_staged_run(&staged_skill, false);
                        return Ok(obc_tool_api::ToolResult::err(format!(
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
            return Ok(obc_tool_api::ToolResult::ok(outputs.join("\n")));
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
            return Ok(obc_tool_api::ToolResult::err(format!(
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
                    return Ok(obc_tool_api::ToolResult::err(format!(
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
            return Ok(obc_tool_api::ToolResult::err(format!(
                "Tool '{}' refused: {}",
                name, reason
            )));
        }

        // Track 0 taint tracking: before a *privileged* call runs, check whether
        // its argument values echo untrusted (external-origin) content pooled
        // earlier this run. This is the CaMeL data-flow guard: fetched web/MCP
        // text must not steer a physical/irreversible action.
        use obc_safety::taint::{self, TaintMode};
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
                        return Ok(obc_tool_api::ToolResult::err(format!(
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
                if r.success && tool.output_trust() == obc_tool_api::OutputTrust::External {
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
                match obc_skill_forge::rollout::demote(&forge, tracker, skill) {
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
    ) -> Result<obc_tool_api::ToolResult> {
        let args_str = args.to_string();
        self.execute_tool(name, &args_str, None).await
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
        reg.iter().map(|t| t.name().to_string()).collect()
    }

    /// Name, description and parameter schema for every registered tool.
    ///
    /// Enough to announce this agent's capabilities to a peer without the caller
    /// reaching into the registry. `tool_names` alone produced announcements a peer
    /// could see but not call, because it had no schema to build arguments from.
    pub fn tool_specs(&self) -> Vec<(String, String, serde_json::Value)> {
        let reg = self.tools.read().unwrap_or_else(|p| p.into_inner());
        reg.iter()
            .map(|t| {
                (
                    t.name().to_string(),
                    t.description().to_string(),
                    t.parameters_schema(),
                )
            })
            .collect()
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

// `approval_authorize` moved to `obc_approval` on 2026-08-20, for the same
// reason `track0_authorize` moved to `obc_safety` the day before: it only
// touched ApprovalManager, Decision and RiskClass, and keeping it here left
// every repository without the agent holding the approval logic and none of
// the entry point to it.
//
// Re-exported so this crate's callers are unchanged.
pub use obc_approval::approval_authorize;

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
                let concrete = obc_skill_forge::substitute_args(tmpl, args);
                format!("step {} → {}({})", i + 1, t, concrete)
            })
            .collect();
        format!(
            "would run {} steps: {}",
            rendered.len(),
            rendered.join("; ")
        )
    } else {
        format!("would execute with args {}", args)
    }
}

// ── Track 0: physical-action authorization ────────────────────────────────
//
// `track0_authorize` moved to `obc_safety::authorize` on 2026-08-19. It only
// ever touched SafetyGate, ActionAuditor, Decision and RiskClass -- all four
// defined there -- so keeping it here meant the one function that reads a
// tool's declared risk and acts on it was unavailable to anything without the
// agent, including the public repository that vendors the rest of Track 0.
//
// Re-exported below so this crate's callers are unchanged.
pub use obc_safety::authorize::track0_authorize;

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
#[derive(Debug, Clone, Default)]
pub struct AgentResponse {
    /// The assistant's final text response.
    pub message: String,
    /// All tool calls made during the agent loop.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Which brain answered (the router's choice, or the failover's).
    pub provider: String,
    pub model: String,
    /// The turn's token numbers when the provider reported them.
    pub usage: Option<obc_providers::Usage>,
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

    // ── History window ────────────────────────────────────────────────────────

    struct SilentProvider;

    #[async_trait::async_trait]
    impl obc_providers::Provider for SilentProvider {
        fn name(&self) -> &str {
            "silent"
        }
        async fn chat_completion(
            &self,
            _messages: &[obc_providers::ChatMessage],
            _tools: &[Box<dyn Tool>],
            _config: &obc_providers::ProviderConfig,
        ) -> Result<obc_providers::ChatCompletion> {
            anyhow::bail!("SilentProvider never answers; these tests only build context")
        }
    }

    fn agent_with_history(n: Option<usize>) -> (Agent, String) {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("history").unwrap();
        for i in 0..40 {
            memory
                .append_message(&session, ChatRole::User, &format!("msg {i}"))
                .unwrap();
        }
        let mut agent = Agent::new(
            AgentConfig::default(),
            Arc::new(SilentProvider),
            memory,
            vec![],
        );
        if let Some(n) = n {
            agent = agent.with_max_history(n);
        }
        (agent, session)
    }

    /// The regression this exists for: `edge.max_history_messages` was documented,
    /// emitted into every generated NanoPi config by the deployment planner, and
    /// applied to nothing at all — `build_context` read a crate constant.
    #[test]
    fn the_history_window_is_the_one_it_was_told_about() {
        let (agent, session) = agent_with_history(Some(6));
        let ctx = agent.build_context(&session).unwrap();
        // One system message for the prompt; no world memory attached here.
        let history = ctx.len() - 1;
        assert_eq!(history, 6, "asked for 6 turns of history, got {history}");
    }

    #[test]
    fn an_unbounded_agent_still_gets_the_default_window() {
        let (agent, session) = agent_with_history(None);
        let ctx = agent.build_context(&session).unwrap();
        assert_eq!(ctx.len() - 1, MAX_HISTORY_MESSAGES.min(40));
    }

    #[test]
    fn a_zero_window_is_refused_rather_than_honoured() {
        // An agent that cannot see its own last turn looks like a stupid model, not
        // like a misconfiguration, so zero is treated as "no opinion".
        let (agent, session) = agent_with_history(Some(0));
        let ctx = agent.build_context(&session).unwrap();
        assert_eq!(ctx.len() - 1, MAX_HISTORY_MESSAGES.min(40));
    }
    use obc_safety::limits::{SafetyGate, SafetyLimit};
    use obc_tool_api::BlastRadius;
    use serde_json::json;

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
    fn agent_response_used_tools() {
        let response = AgentResponse {
            message: "Done".to_string(),
            tool_calls: vec![ToolCallRecord {
                name: "shell".to_string(),
                args: "{}".to_string(),
                result: "ok".to_string(),
                duration_ms: 0,
            }],
            ..Default::default()
        };
        assert!(response.used_tools());

        let empty = AgentResponse {
            message: "Hello".to_string(),
            tool_calls: vec![],
            ..Default::default()
        };
        assert!(!empty.used_tools());
    }
}

// ── The agent's own configuration blocks ────────────────────────────────────
// Moved here from the root config module on 2026-08-13, with the serde default
// helpers only they use. The root config module re-exports all four names, so
// every call site outside this directory is unchanged -- including `approval`,
// which reads AutonomyConfig through there on purpose: pointing it here instead
// would create `approval -> agent` while `agent -> approval` already exists,
// trading one mutual pair for another.
//
// These were the last cycle in the core. The rule is the one every extracted
// crate here follows and the one the last three commits applied: the module
// owns its config block, and the root `Config` composes it.

use serde::{Deserialize, Serialize};

fn default_agent_name() -> String {
    "Oh-Ben-Claw".to_string()
}
fn default_max_tool_iterations() -> usize {
    10
}
fn default_system_prompt() -> String {
    "You are Oh-Ben-Claw, an advanced multi-device AI assistant. \
     You can see, hear, sense, and act in the physical world through \
     a fleet of connected hardware devices. Be helpful, precise, and proactive."
        .to_string()
}
fn default_edge_max_history() -> usize {
    20
}
fn default_edge_max_tool_iterations() -> usize {
    5
}

/// Configuration for the core agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The name of the agent (used in system prompts and UI).
    #[serde(default = "default_agent_name")]
    pub name: String,
    /// The system prompt for the agent.
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// Maximum number of tool-use iterations per user message.
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// The model's context window in tokens, as far as this agent should
    /// assume. OBC's Ollama adapter sends no `num_ctx`, so the number the
    /// model actually has is whatever its Modelfile says; tell the agent the
    /// same number here. Default 8192, Ollama's global default on the bench.
    #[serde(default = "default_context_tokens")]
    pub context_tokens: usize,
    /// Fold older history into a summary once the estimated history exceeds
    /// this fraction of `context_tokens`. Default 0.5 (Hermes's in-loop figure).
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f64,
    /// How many of the most recent messages are never folded. Default 8.
    #[serde(default = "default_compaction_keep_tail")]
    pub compaction_keep_tail: usize,
    /// Master switch for compaction. Off means the history is the raw last
    /// `max_history` messages, as before 2026-09-11.
    #[serde(default = "default_true")]
    pub compaction: bool,
}

fn default_context_tokens() -> usize {
    8192
}
fn default_compaction_threshold() -> f64 {
    0.5
}
fn default_compaction_keep_tail() -> usize {
    8
}
fn default_true() -> bool {
    true
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: default_agent_name(),
            system_prompt: default_system_prompt(),
            max_tool_iterations: default_max_tool_iterations(),
            context_tokens: default_context_tokens(),
            compaction_threshold: default_compaction_threshold(),
            compaction_keep_tail: default_compaction_keep_tail(),
            compaction: default_true(),
        }
    }
}

/// Configuration for the edge-native agent mode (NanoPi Neo3 and similar
/// Linux single-board computers running Oh-Ben-Claw without a central host).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    /// Whether edge-native mode is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of messages retained in the rolling conversation history.
    /// Kept small to reduce RAM pressure on resource-constrained devices.
    #[serde(default = "default_edge_max_history")]
    pub max_history_messages: usize,
    /// Maximum tool-use iterations per user message.
    #[serde(default = "default_edge_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// Whether to start the P2P spine and join the local mesh.
    #[serde(default)]
    pub p2p_enabled: bool,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_history_messages: default_edge_max_history(),
            max_tool_iterations: default_edge_max_tool_iterations(),
            p2p_enabled: false,
        }
    }
}

#[cfg(test)]
mod streaming_events_tests {
    use super::*;

    /// Streams two deltas and finishes with no tool calls.
    struct TwoDeltas;

    #[async_trait::async_trait]
    impl obc_providers::Provider for TwoDeltas {
        fn name(&self) -> &str {
            "two"
        }
        async fn chat_completion(
            &self,
            _m: &[obc_providers::ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &obc_providers::ProviderConfig,
        ) -> Result<obc_providers::ChatCompletion> {
            Ok(obc_providers::ChatCompletion {
                message: "hello world".into(),
                tool_calls: vec![],
                provider: "two".into(),
                model: c.model.clone(),
                usage: None,
            })
        }
        async fn chat_completion_streaming(
            &self,
            _m: &[obc_providers::ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &obc_providers::ProviderConfig,
            sink: obc_providers::DeltaSink<'_>,
        ) -> Result<obc_providers::ChatCompletion> {
            sink(obc_providers::StreamDelta::Text("hello ".into()));
            sink(obc_providers::StreamDelta::Text("world".into()));
            Ok(obc_providers::ChatCompletion {
                message: "hello world".into(),
                tool_calls: vec![],
                provider: "two".into(),
                model: c.model.clone(),
                usage: Some(obc_providers::Usage {
                    input_tokens: 100,
                    output_tokens: 2,
                    cache_read_input_tokens: 50,
                    cache_creation_input_tokens: 0,
                }),
            })
        }
    }

    #[tokio::test]
    async fn the_response_says_which_brain_answered_and_what_it_read() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let agent = Agent::new(AgentConfig::default(), Arc::new(TwoDeltas), memory, vec![]);
        let cfg = obc_providers::ProviderConfig {
            model: "m-two".into(),
            ..Default::default()
        };
        let response = agent.process("s", "hi", &cfg).await.unwrap();
        assert_eq!(response.provider, "two");
        assert_eq!(response.model, "m-two");
        let u = response.usage.expect("usage from the provider");
        assert_eq!(
            (u.input_tokens, u.output_tokens, u.cache_read_input_tokens),
            (100, 2, 50)
        );
        assert_eq!(u.prompt_tokens(), 150);
    }

    #[tokio::test]
    async fn scheduled_sessions_leave_no_trajectory_but_console_sessions_do() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let store = Arc::new(TrajectoryStore::open_in_memory().unwrap());
        let agent = Agent::new(AgentConfig::default(), Arc::new(TwoDeltas), memory, vec![])
            .with_trajectory_store(Arc::clone(&store));
        let cfg = obc_providers::ProviderConfig::default();
        agent
            .process("scheduled-printer", "hi", &cfg)
            .await
            .unwrap();
        assert_eq!(store.count().unwrap(), 0, "a timer's turn was captured");
        agent.process("console", "hi", &cfg).await.unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let quiet = Agent::new(
            AgentConfig::default(),
            Arc::new(TwoDeltas),
            Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap()),
            vec![],
        )
        .with_trajectory_store(Arc::clone(&store))
        .with_trajectory_skip(vec!["tg-".into()]);
        quiet.process("tg-42", "hi", &cfg).await.unwrap();
        assert_eq!(store.count().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_turn_in_a_session_nobody_created_still_works() {
        // Channels, scheduled tasks and Command Center tabs hand `process` ids
        // that have no row yet; with foreign keys on, that used to fail with
        // "FOREIGN KEY constraint failed" before a single token was produced.
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(TwoDeltas),
            Arc::clone(&memory),
            vec![],
        );
        let cfg = obc_providers::ProviderConfig::default();
        let response = agent
            .process("scheduled-printer-check", "hi", &cfg)
            .await
            .unwrap();
        assert_eq!(response.message, "hello world");
        assert!(memory
            .list_sessions()
            .unwrap()
            .iter()
            .any(|s| s.id == "scheduled-printer-check"));
    }

    #[tokio::test]
    async fn a_turn_broadcasts_thinking_then_each_token_as_it_arrives() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("stream").unwrap();
        let agent = Agent::new(AgentConfig::default(), Arc::new(TwoDeltas), memory, vec![]);
        let mut rx = agent.subscribe();
        let cfg = obc_providers::ProviderConfig::default();
        let response = agent.process(&session, "hi", &cfg).await.unwrap();
        assert_eq!(response.message, "hello world");

        let mut kinds = Vec::new();
        let mut text = String::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AgentEvent::Thinking { iteration, .. } => {
                    kinds.push(format!("thinking{iteration}"))
                }
                AgentEvent::Token { delta, reset, .. } => {
                    assert!(!reset);
                    text.push_str(&delta);
                    kinds.push("token".into());
                }
                other => kinds.push(format!("{other:?}")),
            }
        }
        assert_eq!(kinds, vec!["thinking0", "token", "token"]);
        assert_eq!(
            text, "hello world",
            "the deltas concatenate to the final message"
        );
    }
}

#[cfg(test)]
mod context_order_and_compaction_tests {
    use super::*;

    /// Answers every call with a fixed string: as the brain it is the "final
    /// response", as the summariser it is the summary.
    struct Fixed(&'static str);

    #[async_trait::async_trait]
    impl obc_providers::Provider for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn chat_completion(
            &self,
            _m: &[obc_providers::ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &obc_providers::ProviderConfig,
        ) -> Result<obc_providers::ChatCompletion> {
            Ok(obc_providers::ChatCompletion {
                message: self.0.to_string(),
                tool_calls: vec![],
                provider: "fixed".into(),
                model: c.model.clone(),
                usage: None,
            })
        }
    }

    #[test]
    fn the_world_state_is_after_the_history_and_before_the_ask() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("order").unwrap();
        memory
            .append_message(&session, ChatRole::User, "earlier")
            .unwrap();
        memory
            .append_message(&session, ChatRole::Assistant, "reply")
            .unwrap();
        memory
            .append_message(&session, ChatRole::User, "the ask")
            .unwrap();
        let world = Arc::new(obc_memory::world::WorldMemory::open_in_memory().unwrap());
        world
            .observe(
                "printer.state",
                serde_json::json!("idle"),
                1_000,
                1_000,
                "test",
            )
            .unwrap();
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(Fixed("ok")),
            memory,
            vec![],
        )
        .with_world_context(world, world_context::WorldContextConfig::default());
        let ctx = agent.build_context_for(&session, Some("the ask")).unwrap();
        let contents: Vec<&str> = ctx.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents[0], AgentConfig::default().system_prompt);
        assert_eq!(&contents[1..3], &["earlier", "reply"]);
        assert!(
            contents[3].starts_with("## World state"),
            "the ephemeral block sits after the stable history: {:?}",
            contents[3]
        );
        assert_eq!(contents[4], "the ask", "the user's message is last");
    }

    #[tokio::test]
    async fn a_long_history_is_folded_into_a_summary_that_leads_the_context() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("compact").unwrap();
        for i in 0..20 {
            let role = if i % 2 == 0 {
                ChatRole::User
            } else {
                ChatRole::Assistant
            };
            memory
                .append_message(&session, role, &format!("message {i} {}", "x".repeat(200)))
                .unwrap();
        }
        let cfg = AgentConfig {
            context_tokens: 800, // 20 × ~54 tokens ≈ 1,080 > 400 budget
            compaction_keep_tail: 4,
            ..AgentConfig::default()
        };
        let agent = Agent::new(
            cfg,
            Arc::new(Fixed("SUMMARY of the first sixteen")),
            memory.clone(),
            vec![],
        );
        let pc = obc_providers::ProviderConfig::default();

        assert!(agent.compact_if_needed(&session, &pc).await.unwrap());
        let ctx = agent.build_context(&session).unwrap();
        // system prompt, the summary, then the kept tail of 4
        assert_eq!(
            ctx.len(),
            1 + 1 + 4,
            "{:?}",
            ctx.iter()
                .map(|m| &m.content[..20.min(m.content.len())])
                .collect::<Vec<_>>()
        );
        assert!(ctx[1].content.starts_with(context::SUMMARY_MARKER));
        assert!(ctx[1].content.contains("SUMMARY of the first sixteen"));
        assert!(ctx[2].content.starts_with("message 16"));
        assert!(ctx[5].content.starts_with("message 19"));

        // Under budget now: a second call is a no-op, and the rows still exist.
        assert!(!agent.compact_if_needed(&session, &pc).await.unwrap());
        assert_eq!(
            memory.message_count(&session).unwrap(),
            21,
            "nothing was deleted"
        );
    }

    #[tokio::test]
    async fn compaction_can_be_switched_off() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("raw").unwrap();
        for i in 0..20 {
            memory
                .append_message(
                    &session,
                    ChatRole::User,
                    &format!("m{i} {}", "x".repeat(200)),
                )
                .unwrap();
        }
        let cfg = AgentConfig {
            context_tokens: 800,
            compaction: false,
            ..AgentConfig::default()
        };
        let agent = Agent::new(cfg, Arc::new(Fixed("never")), memory, vec![]);
        let pc = obc_providers::ProviderConfig::default();
        assert!(!agent.compact_if_needed(&session, &pc).await.unwrap());
    }
}

#[cfg(test)]
mod routing_agent_tests {
    use super::*;

    struct Named(&'static str, &'static str);

    #[async_trait::async_trait]
    impl obc_providers::Provider for Named {
        fn name(&self) -> &str {
            self.0
        }
        async fn chat_completion(
            &self,
            _m: &[obc_providers::ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &obc_providers::ProviderConfig,
        ) -> Result<obc_providers::ChatCompletion> {
            if self.1 == "FAIL" {
                anyhow::bail!("503 upstream unavailable");
            }
            Ok(obc_providers::ChatCompletion {
                message: self.1.to_string(),
                tool_calls: vec![],
                provider: self.0.into(),
                model: c.model.clone(),
                usage: None,
            })
        }
    }

    fn routing_cfg() -> obc_providers::RoutingConfig {
        obc_providers::RoutingConfig::default_with_cloud(obc_providers::ProviderConfig {
            name: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            ..Default::default()
        })
    }

    fn routed_agent(
        cloud: Named,
    ) -> (
        Agent,
        Arc<obc_memory::MemoryStore>,
        Arc<obc_memory::world::WorldMemory>,
    ) {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let world = Arc::new(obc_memory::world::WorldMemory::open_in_memory().unwrap());
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(Named("ollama", "from local")),
            Arc::clone(&memory),
            vec![],
        )
        .with_world_context(
            Arc::clone(&world),
            world_context::WorldContextConfig::default(),
        )
        .with_routing_provider(routing_cfg(), Arc::new(cloud));
        (agent, memory, world)
    }

    #[tokio::test]
    async fn an_operator_turn_is_answered_by_the_cloud_and_recorded() {
        let (agent, memory, world) = routed_agent(Named("anthropic", "from cloud"));
        let session = memory.create_session("console").unwrap();
        let local_cfg = obc_providers::ProviderConfig::default();
        let r = agent.process(&session, "hello", &local_cfg).await.unwrap();
        assert_eq!(r.message, "from cloud");
        let brain = world
            .current("agent.brain")
            .unwrap()
            .expect("agent.brain fact");
        assert_eq!(brain.value["route"], "cloud");
        assert_eq!(brain.value["model"], "claude-sonnet-5");
        assert_eq!(brain.value["reason"], "operator turn");
        assert_eq!(brain.source, "router");
    }

    #[tokio::test]
    async fn a_background_session_stays_local() {
        let (agent, memory, world) = routed_agent(Named("anthropic", "from cloud"));
        memory.create_session_with_id("system2").unwrap();
        let r = agent
            .process("system2", "wake", &obc_providers::ProviderConfig::default())
            .await
            .unwrap();
        assert_eq!(r.message, "from local");
        assert_eq!(
            world.current("agent.brain").unwrap().unwrap().value["reason"],
            "background session"
        );
    }

    #[tokio::test]
    async fn a_failed_cloud_turn_is_answered_locally_and_starts_the_backoff() {
        let (agent, memory, world) = routed_agent(Named("anthropic", "FAIL"));
        let session = memory.create_session("console").unwrap();
        let mut events = agent.subscribe();
        let r = agent
            .process(&session, "hello", &obc_providers::ProviderConfig::default())
            .await
            .unwrap();
        assert_eq!(r.message, "from local");
        // The client was told to discard the (empty) cloud attempt.
        let mut saw_reset = false;
        while let Ok(ev) = events.try_recv() {
            if let AgentEvent::Token { reset: true, .. } = ev {
                saw_reset = true;
            }
        }
        assert!(saw_reset, "a Restart token precedes the local answer");
        assert_eq!(
            agent.route_turn(&session, 0),
            (routing::Route::Local, "cloud unreachable, backing off")
        );
        assert_eq!(
            world.current("agent.brain").unwrap().unwrap().value["route"],
            "local"
        );
    }

    #[tokio::test]
    async fn private_facts_in_the_context_keep_the_turn_local() {
        let (agent, memory, world) = routed_agent(Named("anthropic", "from cloud"));
        world
            .observe_as(
                "vision.subject.person-1",
                serde_json::json!({"label": "person"}),
                now_ms(),
                now_ms(),
                "clawcam",
                obc_memory::world::Origin::Derived,
            )
            .unwrap();
        let session = memory.create_session("console").unwrap();
        assert_eq!(
            agent.route_turn(&session, 5),
            (routing::Route::Local, "private facts in context")
        );
    }

    #[test]
    fn the_daily_budget_stops_cloud_turns() {
        let (agent, memory, _) = routed_agent(Named("anthropic", "from cloud"));
        let session = memory.create_session("console").unwrap();
        let r = agent.routing.as_ref().unwrap();
        assert_eq!(agent.route_turn(&session, 0).0, routing::Route::Cloud);
        r.add_spend(0.5);
        assert_eq!(
            agent.route_turn(&session, 0).0,
            routing::Route::Cloud,
            "no cap by default"
        );
        let (mut agent2, _, _) = routed_agent(Named("anthropic", "from cloud"));
        agent2.routing.as_mut().unwrap().cfg.daily_budget_usd = 0.25;
        agent2.routing.as_ref().unwrap().add_spend(0.5);
        assert_eq!(
            agent2.route_turn(&session, 0),
            (routing::Route::Local, "daily cloud budget spent")
        );
    }
}

#[cfg(test)]
mod notes_context_tests {
    use super::*;

    struct Silent;
    #[async_trait::async_trait]
    impl obc_providers::Provider for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        async fn chat_completion(
            &self,
            _m: &[obc_providers::ChatMessage],
            _t: &[Box<dyn Tool>],
            c: &obc_providers::ProviderConfig,
        ) -> Result<obc_providers::ChatCompletion> {
            Ok(obc_providers::ChatCompletion {
                message: String::new(),
                tool_calls: vec![],
                provider: "silent".into(),
                model: c.model.clone(),
                usage: None,
            })
        }
    }

    #[test]
    fn notes_follow_the_system_prompt_in_the_first_message() {
        let memory = Arc::new(obc_memory::MemoryStore::open_in_memory().unwrap());
        let session = memory.create_session("n").unwrap();
        memory
            .append_message(&session, ChatRole::User, "hi")
            .unwrap();
        let dir = std::env::temp_dir().join(format!("obc-agent-notes-{}", uuid::Uuid::new_v4()));
        let notes = Arc::new(obc_memory::notes::Notes::open(dir).unwrap());
        let agent = Agent::new(AgentConfig::default(), Arc::new(Silent), memory, vec![])
            .with_notes(Arc::clone(&notes));

        let ctx = agent.build_context(&session).unwrap();
        assert_eq!(
            ctx[0].content,
            AgentConfig::default().system_prompt,
            "empty notes add nothing"
        );

        notes
            .add(
                obc_memory::notes::Target::User,
                "Prefers evidence before conclusions.",
            )
            .unwrap();
        let ctx = agent.build_context(&session).unwrap();
        assert!(ctx[0]
            .content
            .starts_with(AgentConfig::default().system_prompt.trim_end()));
        assert!(ctx[0]
            .content
            .contains("### About the operator\n- Prefers evidence"));
        assert_eq!(
            ctx[1].content, "hi",
            "still one system message before the history"
        );
    }
}

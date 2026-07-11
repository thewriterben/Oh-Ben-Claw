//! System 2 — the slow reasoner (Phase 18).
//!
//! System 1 ([`ReflexEngine`](super::reflex::ReflexEngine)) handles known
//! situations deterministically at reflex speed. When it meets something it
//! can't resolve it emits [`Action::Escalate`] — and *this* module is what
//! that escalation wakes: the cloud/host LLM, invoked **for planning and
//! novelty only, never per event**.
//!
//! Three layers keep the LLM out of the hot path:
//! 1. [`System2Sink`] — an `ActionSink` decorator that forwards every action
//!    untouched and, on escalation, `try_send`s a wake onto a bounded channel.
//!    A full channel drops the wake (counted) — System 1 never blocks on
//!    System 2.
//! 2. [`NoveltyGate`] — wakes only pass for *novel* escalations: reasons are
//!    fingerprinted (numeric tokens stripped, so `"offline 120s"` and
//!    `"offline 240s"` are the same situation) and repeats within the novelty
//!    window are suppressed. An hourly wake budget caps LLM spend outright.
//! 3. [`System2Reasoner`] — the consumer task: builds a compact context from
//!    world memory (recent escalations + fleet state), invokes the
//!    [`Reasoner`] (the real agent in production, a stub in tests), and
//!    records the wake + outcome back into world memory
//!    (`system2.last_wake`), closing the perceive → reflex → reason loop.

use crate::agent::reflex::{Action, ActionSink};
use crate::memory::world::WorldMemory;
use crate::movement::MovementCommand;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Wake events ───────────────────────────────────────────────────────────────

/// One escalation handed up from System 1.
#[derive(Debug, Clone)]
pub struct WakeEvent {
    /// The escalation reason (from the reflex rule).
    pub reason: String,
    /// When System 1 escalated (ms since epoch).
    pub at_ms: u64,
}

// ── Novelty gate ──────────────────────────────────────────────────────────────

/// Decides whether an escalation is *novel* enough to spend an LLM call on.
///
/// Reasons are normalized into a fingerprint: lowercase, with purely numeric
/// tokens (and number+unit tokens like `120s` / `85%`) dropped, because the
/// varying quantity in an otherwise identical alarm does not make it a new
/// situation. Distinct entities (e.g. `node-3` vs `node-4`) keep distinct
/// fingerprints. A repeat within `window_ms` is suppressed; the hourly budget
/// bounds total wakes regardless of variety.
pub struct NoveltyGate {
    window_ms: u64,
    max_wakes_per_hour: u32,
    seen: Mutex<HashMap<String, u64>>,
    wake_times: Mutex<Vec<u64>>,
}

/// Why the gate suppressed a wake (or didn't).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Novel — spend the LLM call.
    Wake,
    /// Same situation seen within the novelty window.
    Repeat,
    /// The hourly wake budget is exhausted.
    OverBudget,
}

impl NoveltyGate {
    pub fn new(window_ms: u64, max_wakes_per_hour: u32) -> Self {
        Self {
            window_ms,
            max_wakes_per_hour,
            seen: Mutex::new(HashMap::new()),
            wake_times: Mutex::new(Vec::new()),
        }
    }

    /// Normalize a reason into its situation fingerprint.
    pub fn fingerprint(reason: &str) -> String {
        reason
            .to_lowercase()
            .split_whitespace()
            .filter(|tok| {
                // Drop tokens that are numbers or number+short-unit (120s, 85%, 3.3v):
                // strip trailing non-digits, then require some leading digit content.
                let trimmed = tok.trim_end_matches(|c: char| !c.is_ascii_digit());
                let numeric_head = !trimmed.is_empty()
                    && trimmed
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
                // Keep hyphenated identifiers like "node-3" (they contain letters
                // before the digits); drop "120", "120s", "85%", "3.3v".
                !(numeric_head && tok.len() - trimmed.len() <= 2)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Gate an escalation at time `now_ms`. Records the wake when admitted.
    pub fn admit(&self, reason: &str, now_ms: u64) -> GateDecision {
        // Budget first: prune the sliding hour window.
        {
            let mut times = self.wake_times.lock();
            times.retain(|t| now_ms.saturating_sub(*t) < 3_600_000);
            if self.max_wakes_per_hour > 0 && times.len() >= self.max_wakes_per_hour as usize {
                return GateDecision::OverBudget;
            }
        }

        let fp = Self::fingerprint(reason);
        {
            let mut seen = self.seen.lock();
            if let Some(last) = seen.get(&fp) {
                if now_ms.saturating_sub(*last) < self.window_ms {
                    return GateDecision::Repeat;
                }
            }
            seen.insert(fp, now_ms);
        }
        self.wake_times.lock().push(now_ms);
        GateDecision::Wake
    }
}

// ── Reasoner abstraction ──────────────────────────────────────────────────────

/// The slow reasoner itself: in production the live agent
/// (`AgentHandle::process` on a dedicated session); in tests a scripted stub.
#[async_trait]
pub trait Reasoner: Send + Sync {
    async fn reason(&self, objective: &str) -> anyhow::Result<String>;
}

// ── System 2 task ─────────────────────────────────────────────────────────────

/// Consumer of escalation wakes: novelty-gates, builds context, invokes the
/// [`Reasoner`], and records the outcome into world memory.
pub struct System2Reasoner {
    gate: NoveltyGate,
    reasoner: Arc<dyn Reasoner>,
    world: Option<Arc<WorldMemory>>,
    obs: Option<Arc<crate::observability::ObsContext>>,
}

impl System2Reasoner {
    pub fn new(gate: NoveltyGate, reasoner: Arc<dyn Reasoner>) -> Self {
        Self {
            gate,
            reasoner,
            world: None,
            obs: None,
        }
    }

    pub fn with_world(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    pub fn with_obs(mut self, obs: Arc<crate::observability::ObsContext>) -> Self {
        self.obs = Some(obs);
        self
    }

    fn count(&self, name: &str) {
        if let Some(obs) = &self.obs {
            obs.metrics.counter(name).inc();
        }
    }

    /// Compact situation context from world memory: the escalation plus a
    /// bounded snapshot of fleet-relevant current facts. Small on purpose —
    /// System 2 can pull more through its tools (`world_memory`,
    /// `mesh_status`) once awake.
    fn build_objective(&self, event: &WakeEvent) -> String {
        let mut ctx = String::new();
        if let Some(world) = &self.world {
            if let Ok(entities) = world.entities() {
                let interesting: Vec<&String> = entities
                    .iter()
                    .filter(|e| {
                        e.starts_with("mesh.")
                            || e.starts_with("safing")
                            || e.starts_with("power")
                            || e.starts_with("escalation")
                    })
                    .take(12)
                    .collect();
                for entity in interesting {
                    if let Ok(Some(fact)) = world.current(entity) {
                        let val = fact.value.to_string();
                        let val = if val.len() > 160 {
                            format!("{}…", &val[..160])
                        } else {
                            val
                        };
                        ctx.push_str(&format!("- {entity} = {val}\n"));
                    }
                }
            }
        }
        format!(
            "SYSTEM 1 ESCALATION (reflex layer could not resolve this — you are \
             System 2, the planning layer).\n\
             Reason: {}\n\
             Escalated at (ms): {}\n\
             {}{}\
             Diagnose the situation using your tools (world_memory, mesh_status, \
             sensors) and take or recommend the safest corrective action. Physical \
             actions remain subject to Track 0 approval and safety gates.",
            event.reason,
            event.at_ms,
            if ctx.is_empty() {
                String::new()
            } else {
                format!("Current world state (snapshot):\n{ctx}")
            },
            ""
        )
    }

    /// Handle one wake event end-to-end. Returns what the gate decided (and,
    /// on a wake, whether the reasoner succeeded).
    pub async fn handle(&self, event: WakeEvent) -> GateDecision {
        let decision = self.gate.admit(&event.reason, event.at_ms);
        match &decision {
            GateDecision::Repeat => {
                self.count("system2_suppressed_repeat_total");
                tracing::debug!(reason = %event.reason, "System 2: suppressed (repeat)");
            }
            GateDecision::OverBudget => {
                self.count("system2_suppressed_budget_total");
                tracing::warn!(reason = %event.reason, "System 2: suppressed (wake budget)");
            }
            GateDecision::Wake => {
                self.count("system2_wakes_total");
                let objective = self.build_objective(&event);
                tracing::info!(reason = %event.reason, "System 2: waking the slow reasoner");
                let outcome = match self.reasoner.reason(&objective).await {
                    Ok(response) => {
                        let head: String = response.chars().take(240).collect();
                        json!({ "ok": true, "reason": event.reason, "response_head": head })
                    }
                    Err(e) => {
                        self.count("system2_errors_total");
                        tracing::warn!(error = %e, "System 2 reasoning failed");
                        json!({ "ok": false, "reason": event.reason, "error": e.to_string() })
                    }
                };
                if let Some(world) = &self.world {
                    let now = now_ms();
                    let _ = world.observe("system2.last_wake", outcome, now, now, "system2");
                }
            }
        }
        decision
    }

    /// Run forever, consuming wake events until the channel closes.
    pub async fn run(self, mut rx: mpsc::Receiver<WakeEvent>) {
        while let Some(event) = rx.recv().await {
            let _ = self.handle(event).await;
        }
        tracing::debug!("System 2 wake channel closed; reasoner task exiting");
    }
}

// ── Sink decorator (System 1 side) ────────────────────────────────────────────

/// [`ActionSink`] decorator: forwards every action to the inner sink and, on
/// [`Action::Escalate`], hands a wake to System 2 without ever blocking —
/// `try_send` on a bounded channel; a full queue drops the wake (the
/// notification path still records it for the operator).
pub struct System2Sink {
    inner: Arc<dyn ActionSink>,
    tx: mpsc::Sender<WakeEvent>,
}

impl System2Sink {
    /// Wrap `inner`, waking System 2 through a bounded channel of `capacity`.
    pub fn new(inner: Arc<dyn ActionSink>, capacity: usize) -> (Self, mpsc::Receiver<WakeEvent>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (Self { inner, tx }, rx)
    }
}

#[async_trait]
impl ActionSink for System2Sink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        self.inner.gpio_write(node_id, pin, value).await
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        self.inner.publish(topic, payload).await
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        // Never block System 1: a full wake queue just drops (System 2 is
        // busy or budget-bound; the notifier still logs the escalation).
        let _ = self.tx.try_send(WakeEvent {
            reason: reason.to_string(),
            at_ms: now_ms(),
        });
        self.inner.escalate(reason).await
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        self.inner.move_actuator(command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingReasoner {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Reasoner for CountingReasoner {
        async fn reason(&self, _objective: &str) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("diagnosed".to_string())
        }
    }

    fn reasoner_with(gate: NoveltyGate) -> (System2Reasoner, Arc<CountingReasoner>) {
        let counter = Arc::new(CountingReasoner {
            calls: AtomicUsize::new(0),
        });
        (
            System2Reasoner::new(gate, counter.clone() as Arc<dyn Reasoner>),
            counter,
        )
    }

    #[test]
    fn fingerprint_strips_quantities_but_keeps_identities() {
        let a = NoveltyGate::fingerprint("mesh node lost: node-3 offline 120s");
        let b = NoveltyGate::fingerprint("mesh node lost: node-3 offline 240s");
        let c = NoveltyGate::fingerprint("mesh node lost: node-4 offline 120s");
        assert_eq!(a, b, "same situation, different duration");
        assert_ne!(a, c, "a different node is a different situation");

        let hot1 = NoveltyGate::fingerprint("overheat: cpu_temp 91.5 exceeds 85");
        let hot2 = NoveltyGate::fingerprint("overheat: cpu_temp 93.0 exceeds 85");
        assert_eq!(hot1, hot2);
    }

    #[test]
    fn gate_admits_novel_suppresses_repeat_and_readmits_after_window() {
        let gate = NoveltyGate::new(10_000, 100);
        assert_eq!(gate.admit("node-3 offline 10s", 1_000), GateDecision::Wake);
        assert_eq!(
            gate.admit("node-3 offline 20s", 2_000),
            GateDecision::Repeat
        );
        // A different situation still wakes inside the window.
        assert_eq!(gate.admit("battery critical 9%", 3_000), GateDecision::Wake);
        // After the window the original situation is novel again.
        assert_eq!(
            gate.admit("node-3 offline 300s", 12_500),
            GateDecision::Wake
        );
    }

    #[test]
    fn hourly_budget_bounds_total_wakes() {
        let gate = NoveltyGate::new(0, 2); // window 0 = everything is novel
        assert_eq!(gate.admit("a", 1), GateDecision::Wake);
        assert_eq!(gate.admit("b", 2), GateDecision::Wake);
        assert_eq!(gate.admit("c", 3), GateDecision::OverBudget);
        // …until the hour rolls over.
        assert_eq!(gate.admit("d", 3_600_010), GateDecision::Wake);
    }

    #[tokio::test]
    async fn reasoner_invoked_only_on_novelty() {
        let (s2, counter) = reasoner_with(NoveltyGate::new(60_000, 100));
        let wake = |reason: &str, at| WakeEvent {
            reason: reason.to_string(),
            at_ms: at,
        };

        assert_eq!(
            s2.handle(wake("node-3 offline 10s", 1_000)).await,
            GateDecision::Wake
        );
        assert_eq!(
            s2.handle(wake("node-3 offline 20s", 2_000)).await,
            GateDecision::Repeat
        );
        assert_eq!(
            s2.handle(wake("node-3 offline 30s", 3_000)).await,
            GateDecision::Repeat
        );
        assert_eq!(
            s2.handle(wake("camera obstructed", 4_000)).await,
            GateDecision::Wake
        );
        assert_eq!(
            counter.calls.load(Ordering::SeqCst),
            2,
            "LLM ran twice for four events"
        );
    }

    #[tokio::test]
    async fn wake_records_outcome_into_world_memory() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let (s2, _counter) = reasoner_with(NoveltyGate::new(60_000, 100));
        let s2 = s2.with_world(Arc::clone(&world));

        s2.handle(WakeEvent {
            reason: "node-3 offline".into(),
            at_ms: 1_000,
        })
        .await;

        let fact = world.current("system2.last_wake").unwrap().unwrap();
        assert_eq!(fact.value["ok"], json!(true));
        assert_eq!(fact.value["reason"], json!("node-3 offline"));
        assert!(fact.value["response_head"]
            .as_str()
            .unwrap()
            .contains("diagnosed"));
    }

    #[tokio::test]
    async fn objective_carries_reason_and_world_snapshot() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        world
            .observe("mesh.node-3", json!({"health": "offline"}), 1, 1, "t")
            .unwrap();
        let (s2, _) = reasoner_with(NoveltyGate::new(0, 100));
        let s2 = s2.with_world(world);
        let obj = s2.build_objective(&WakeEvent {
            reason: "node-3 offline".into(),
            at_ms: 5,
        });
        assert!(obj.contains("SYSTEM 1 ESCALATION"));
        assert!(obj.contains("node-3 offline"));
        assert!(obj.contains("mesh.node-3"));
        assert!(obj.contains("Track 0"));
    }

    struct NullSink;
    #[async_trait]
    impl ActionSink for NullSink {
        async fn gpio_write(&self, _n: &str, _p: i64, _v: i64) -> anyhow::Result<()> {
            Ok(())
        }
        async fn publish(&self, _t: &str, _p: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        async fn escalate(&self, _r: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn move_actuator(&self, _c: &MovementCommand) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn sink_forwards_and_never_blocks_when_queue_is_full() {
        let (sink, mut rx) = System2Sink::new(Arc::new(NullSink), 1);
        // Fill the queue, then escalate twice more — must not block or error.
        sink.escalate("first").await.unwrap();
        sink.escalate("second (dropped)").await.unwrap();
        sink.escalate("third (dropped)").await.unwrap();

        let got = rx.recv().await.unwrap();
        assert_eq!(got.reason, "first");
        // Queue drained; nothing else buffered beyond capacity.
        assert!(rx.try_recv().is_err());
    }
}

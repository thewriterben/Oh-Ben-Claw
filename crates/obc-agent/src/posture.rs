//! Descending posture: the brain's one deterministic reason to move a slot.
//!
//! The spinal tier (`obc_reflex::Condition::SensorSlot`, the node's `descend`
//! command) lets the brain slide a reflex threshold without touching the
//! rule. Until this module nothing decided *when*. This is the first policy,
//! and it is the fly's: the mushroom body's novelty signal reaches the
//! descending neurons as a bias — an unfamiliar situation makes the animal
//! cautious before any reasoning about *why* has happened
//! (`docs/CONNECTOME-2026-09.md` §MB→DN). Here: an objective the mushroom
//! body has no close precedent for lowers the configured slots on the
//! configured nodes to `novel_level`; a familiar one clears them back to
//! the rules' defaults. Nothing else — no model call, no randomness.
//!
//! Two things it is careful about:
//!
//! - **It sends on change, not on every turn.** Posture is per node; a
//!   `descend` goes out only when the posture a turn wants differs from the
//!   one last sent (or nothing has been sent yet). Ten novel objectives in a
//!   row cost one frame.
//! - **A failed send is not a posture.** If the sink refuses, the node's
//!   recorded posture stays what it was, the failure is on the
//!   `descending.<node>` fact and in the log, and the next turn tries again.
//!   The alternative — remembering a posture the node never heard — is the
//!   silent-degradation shape this project keeps refusing.
//!
//! - **It does not block the turn on the radio, and it does not trust the
//!   radio either.** The mesh has no ACK and a plain collision loses about
//!   one frame in five on the bench. So a send is *confirmed* in the
//!   background: a task waits for the node's `cmd_result` to land in world
//!   memory (the gateway's reply path), resends with a fresh id on silence,
//!   and only when the node has answered is it recorded as told. If every
//!   attempt is silent the fact says `answered: false`, and the next turn
//!   sends again. Without world memory there is nothing to wait for; the
//!   send is then taken at face value and the fact says so.
//!
//! `descend` is idempotent, which is what makes resending safe.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use obc_memory::mushroom::Assessment;
use obc_memory::world::{Origin, WorldMemory};
use obc_spine::lora_gateway::{CommandSink, NodeCommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `[descending]` — which nodes' slots the brain's novelty signal moves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PostureConfig {
    /// Off by default: a body with no slot-bound rules has nothing to move.
    #[serde(default)]
    pub enabled: bool,
    /// The slot level sent when the objective is novel, in `[0, 1]`. For a
    /// threshold `min + level·(max − min)`, lower is more cautious. Default
    /// 0.15.
    #[serde(default = "default_novel_level")]
    pub novel_level: f64,
    /// The nodes and slots to move. Empty with `enabled = true` is refused at
    /// load: a policy that moves nothing is a misconfiguration, not a no-op.
    #[serde(default)]
    pub nodes: Vec<PostureTarget>,
    /// How long to wait for the node's reply to one attempt before resending,
    /// in ms. Default 8000, as `[lora_gateway] reply_timeout_ms`.
    #[serde(default = "default_reply_timeout_ms")]
    pub reply_timeout_ms: u64,
    /// Resends after a silent attempt. Default 2. With world memory absent
    /// there is nothing to wait for and this is unused.
    #[serde(default = "default_reply_retries")]
    pub reply_retries: u32,
}

fn default_reply_timeout_ms() -> u64 {
    8_000
}

fn default_reply_retries() -> u32 {
    2
}

impl Default for PostureConfig {
    /// The same values an empty `[descending]` table parses to.
    fn default() -> Self {
        Self {
            enabled: false,
            novel_level: default_novel_level(),
            nodes: Vec::new(),
            reply_timeout_ms: default_reply_timeout_ms(),
            reply_retries: default_reply_retries(),
        }
    }
}

/// One node and the slots on it this policy owns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PostureTarget {
    pub node_id: String,
    pub slots: Vec<u8>,
}

fn default_novel_level() -> f64 {
    0.15
}

impl PostureConfig {
    /// Refuse the shapes that would silently do nothing or send garbage.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.novel_level),
            "[descending] novel_level must be in [0, 1]"
        );
        anyhow::ensure!(
            !self.nodes.is_empty(),
            "[descending] enabled with no nodes: nothing to move — list nodes = [{{ node_id, slots }}] or disable it"
        );
        for t in &self.nodes {
            anyhow::ensure!(
                !t.node_id.trim().is_empty(),
                "[descending] a node entry has an empty node_id"
            );
            anyhow::ensure!(
                !t.slots.is_empty(),
                "[descending] node {} lists no slots",
                t.node_id
            );
            for s in &t.slots {
                anyhow::ensure!(
                    (*s as usize) < obc_reflex::MAX_SLOTS,
                    "[descending] node {}: slot {s} out of range (max {})",
                    t.node_id,
                    obc_reflex::MAX_SLOTS - 1
                );
            }
        }
        Ok(())
    }
}

/// What the brain wants a node's slots to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// The rules' own defaults: nothing sent, or `descend {clear:true}`.
    Default,
    /// Every owned slot at `novel_level`.
    Cautious,
}

impl Posture {
    /// Novel → cautious; anything else → default. The whole policy, so it
    /// can be tested on its own.
    pub fn for_assessment(a: &Assessment) -> Self {
        if a.novel {
            Posture::Cautious
        } else {
            Posture::Default
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Posture::Default => "default",
            Posture::Cautious => "cautious",
        }
    }
}

/// What a graded posture asks of a node's owned slots.
///
/// The `Clear` arm is not "level zero" — it is `descend {clear: true}`,
/// which restores each rule's own default exactly, costs the smallest
/// frame, and is what the node already does today for [`Posture::Default`].
/// A map that only ever emitted levels would never return a slot to the
/// rule's own number, only to a level that happens to equal it.
///
/// `PartialEq` and not `Eq` because the level is an `f64`. That is enough
/// for the change-detection [`PosturePolicy`] does — the levels being
/// compared are quantised by [`LevelMap::descent`], so equal caution gives
/// bit-equal levels — but it is why the quantisation is load-bearing rather
/// than cosmetic: without it almost every turn is a different `f64` and
/// therefore a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Descent {
    /// Clear the owned slots; the rules' own defaults apply.
    Clear,
    /// Hold every owned slot at this quantised level.
    Level(f64),
}

/// The graded map from the body's novelty to a slot level — rung 1 of
/// `docs/NEUROMORPHIC-2026-09.md` §6, as a pure function.
///
/// **Nothing in the agent loop calls this yet.** §6 splits rung 1 into three
/// steps and this is step A: the map exists so the replay in
/// `tests/posture_level_replay.rs` can choose its constants from the brain's
/// own episodes *before* any of them reach a config file or the air.
/// [`Posture`] is still the policy; step B wires this in and moves these
/// fields into `[descending]`. Every field is therefore a parameter with no
/// default here on purpose — a `Default` impl would be six numbers nobody
/// measured, which is the shape this feature exists to avoid.
///
/// **Only `novelty` is read.** [`Assessment`] also carries `success_prior`,
/// and §6 wants it in the map on the argument that an objective resembling
/// past failures deserves caution even when familiar. It is not here,
/// because the prior is `Option<f32>` behind a coverage floor and may be
/// `None` on exactly the objectives this map cares about. The replay prints
/// that coverage; if it is non-trivial the prior earns a term, and if it is
/// not, adding one now would be a weighting fitted to nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelMap {
    /// Novelty at or below which caution is 0.
    pub caution_knee: f32,
    /// Novelty at or above which caution is 1.
    pub caution_full: f32,
    /// The level standing for "the rule's own default" — caution 0.
    pub rule_default: f64,
    /// The level at full caution. Lower is more cautious for a threshold
    /// `min + level·(max − min)`, so this normally sits below `rule_default`.
    pub novel_level: f64,
    /// Quantisation of the emitted level. The deadband: two turns whose
    /// caution rounds to the same step are the same `Descent` and cost no
    /// frame.
    pub step: f64,
    /// Caution below this emits [`Descent::Clear`] instead of a level.
    pub clear_below: f32,
}

impl LevelMap {
    /// Caution in `[0, 1]`, clamped and monotone in novelty.
    ///
    /// The shape is linear between the knees. That is a **declared choice,
    /// not a measured one**: the 99 episodes this was written against are
    /// piled at novelty 0.001–0.03 with four points anywhere else, and a
    /// curve fitted to four points would be a guess wearing evidence. The
    /// replay's job is to size the knees, the step and the floor — which are
    /// questions about this traffic and which this corpus can answer — and
    /// to leave the shape falsifiable.
    pub fn caution(&self, novelty: f32) -> f32 {
        // A novelty that is not a number is a bug upstream of here, and the
        // one thing it must not do is become a level on the radio. Caution 0
        // clears the slot, which is the node's own default and the state it
        // was in before this policy existed.
        if novelty.is_nan() {
            return 0.0;
        }
        let span = self.caution_full - self.caution_knee;
        if span.is_nan() || span <= 0.0 {
            // Degenerate or inverted knees: a step function at the upper
            // knee rather than a division by zero.
            return if novelty >= self.caution_full {
                1.0
            } else {
                0.0
            };
        }
        ((novelty - self.caution_knee) / span).clamp(0.0, 1.0)
    }

    /// The level for a caution, before the floor: `lerp(rule_default,
    /// novel_level, caution)`, quantised to `step`.
    pub fn level(&self, caution: f32) -> f64 {
        let raw = self.rule_default + (self.novel_level - self.rule_default) * caution as f64;
        if self.step > 0.0 {
            (raw / self.step).round() * self.step
        } else {
            raw
        }
    }

    /// What this turn's novelty asks of the owned slots.
    ///
    /// Two ways to arrive at [`Descent::Clear`], and the second was found by
    /// the replay rather than designed: a caution under `clear_below`, and a
    /// level that quantises back onto `rule_default`. The second matters
    /// because without it the map spends a frame telling the node to hold
    /// the number its own rule already holds — on the brain's real episodes
    /// that was most of the traffic the graded map added. `Clear` says the
    /// same thing in fewer bytes and restores the rule's default exactly.
    pub fn descent(&self, novelty: f32) -> Descent {
        let caution = self.caution(novelty);
        if caution < self.clear_below {
            return Descent::Clear;
        }
        let level = self.level(caution);
        // Against `level(0.0)`, not against `rule_default` itself: both sides
        // then come out of the same rounding, so this is an exact equality
        // between two numbers produced the same way rather than a float
        // comparison across a quantisation step.
        if level == self.level(0.0) {
            return Descent::Clear;
        }
        Descent::Level(level)
    }
}

/// Prefix of the per-node facts this policy writes.
pub const FACT_PREFIX: &str = "descending.";
/// Source tag on those facts.
pub const SOURCE: &str = "descending-posture";

/// What a node has been told, as far as this policy knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Told {
    /// A send is out and its confirmation task is still waiting. Another
    /// turn wanting the same posture waits with it; one wanting the other
    /// posture supersedes it.
    Pending(Posture),
    /// The node answered (or, without world memory, the sink accepted).
    Confirmed(Posture),
}

/// The policy, holding the last posture each node was *told* — not the one
/// the brain wanted — so a refused or unanswered send is retried rather
/// than remembered.
pub struct PosturePolicy {
    cfg: PostureConfig,
    sink: Arc<dyn CommandSink>,
    world: Option<Arc<WorldMemory>>,
    told: Arc<Mutex<HashMap<String, Told>>>,
}

/// What one turn's application did, for the log and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub posture: Posture,
    /// Nodes a `descend` was sent to this turn (posture changed and the sink
    /// accepted it). Confirmation follows in the background.
    pub sent: Vec<String>,
    /// Nodes whose send failed, with the reason.
    pub failed: Vec<(String, String)>,
}

impl PosturePolicy {
    pub fn new(
        cfg: PostureConfig,
        sink: Arc<dyn CommandSink>,
        world: Option<Arc<WorldMemory>>,
    ) -> Self {
        Self {
            cfg,
            sink,
            world,
            told: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn config(&self) -> &PostureConfig {
        &self.cfg
    }

    /// What this policy currently believes `node_id` has been told, once a
    /// send has been confirmed. `None` while nothing is confirmed.
    pub fn confirmed(&self, node_id: &str) -> Option<Posture> {
        match self
            .told
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(node_id)
        {
            Some(Told::Confirmed(p)) => Some(*p),
            _ => None,
        }
    }

    fn command_for(
        &self,
        target: &PostureTarget,
        posture: Posture,
        id: &str,
    ) -> anyhow::Result<NodeCommand> {
        match posture {
            Posture::Cautious => {
                let pairs: Vec<(u8, f64)> = target
                    .slots
                    .iter()
                    .map(|s| (*s, self.cfg.novel_level))
                    .collect();
                NodeCommand::descend(&target.node_id, id, &pairs, false)
            }
            Posture::Default => NodeCommand::descend(&target.node_id, id, &[], true),
        }
    }

    /// Bring every configured node to the posture `assessment` calls for.
    /// `objective` is recorded on the fact, truncated, so a person reading
    /// world memory can see what made the brain cautious.
    pub async fn apply(&self, assessment: &Assessment, objective: &str, now_ms: u64) -> Applied {
        let posture = Posture::for_assessment(assessment);
        let mut out = Applied {
            posture,
            sent: Vec::new(),
            failed: Vec::new(),
        };
        if !self.cfg.enabled {
            return out;
        }
        let objective: String = objective.chars().take(120).collect();
        for target in &self.cfg.nodes {
            let already = self
                .told
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&target.node_id)
                .copied();
            if matches!(already, Some(Told::Confirmed(p) | Told::Pending(p)) if p == posture) {
                continue;
            }
            let id = obc_tools::builtin::mesh::short_correlation_id();
            let result = match self.command_for(target, posture, &id) {
                Ok(cmd) => self
                    .sink
                    .send_command(&cmd)
                    .await
                    .map(|()| cmd.encoded_len()),
                Err(e) => Err(e),
            };
            let mut fact = json!({
                "posture": posture.as_str(),
                "novelty": assessment.novelty,
                "novel_level": self.cfg.novel_level,
                "slots": target.slots,
                "objective": objective,
                "id": id,
                "at_ms": now_ms,
                "attempts": 1,
            });
            match result {
                Ok(bytes) => {
                    fact["sent"] = json!(true);
                    fact["bytes"] = json!(bytes);
                    tracing::info!(
                        node = %target.node_id,
                        posture = posture.as_str(),
                        novelty = assessment.novelty,
                        id = %id,
                        "descending posture sent"
                    );
                    out.sent.push(target.node_id.clone());
                    let told = match &self.world {
                        Some(_) => Told::Pending(posture),
                        // Nothing to wait for: the sink's word is all there is.
                        None => Told::Confirmed(posture),
                    };
                    self.told
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(target.node_id.clone(), told);
                    if self.world.is_some() {
                        fact["answered"] = Value::Null; // not yet
                        self.spawn_confirmation(target.clone(), posture, id.clone(), fact.clone());
                    }
                }
                Err(e) => {
                    fact["sent"] = json!(false);
                    fact["error"] = json!(e.to_string());
                    tracing::warn!(
                        node = %target.node_id,
                        posture = posture.as_str(),
                        error = %e,
                        "descending posture NOT sent; will retry next turn"
                    );
                    out.failed.push((target.node_id.clone(), e.to_string()));
                }
            }
            if let Some(world) = &self.world {
                let _ = world.observe_as(
                    &format!("{FACT_PREFIX}{}", target.node_id),
                    fact,
                    now_ms,
                    now_ms,
                    SOURCE,
                    Origin::Observed,
                );
            }
        }
        out
    }

    /// Wait for the node's reply to `first_id`; resend with a fresh id on
    /// silence, up to `reply_retries` times; record the outcome on the fact
    /// and in `told`. Runs on its own task so the turn is not held.
    fn spawn_confirmation(
        &self,
        target: PostureTarget,
        posture: Posture,
        first_id: String,
        mut fact: Value,
    ) {
        let Some(world) = self.world.clone() else {
            return;
        };
        let sink = Arc::clone(&self.sink);
        let told = Arc::clone(&self.told);
        let timeout = std::time::Duration::from_millis(self.cfg.reply_timeout_ms.max(100));
        let retries = self.cfg.reply_retries;
        let cmd_for_retry = |id: &str| self.command_for(&target, posture, id);
        // Commands for the retries are built now, on this thread, so the
        // task holds no reference to the policy.
        let retry_cmds: Vec<NodeCommand> = (1..=retries)
            .filter_map(|n| cmd_for_retry(&format!("{first_id}r{n}")).ok())
            .collect();
        let node_id = target.node_id.clone();
        tokio::spawn(async move {
            let result_key = format!("mesh.{node_id}.cmd_result");
            let mut ids = vec![first_id.clone()];
            let mut attempts = 1u32;
            let mut answered: Option<Value> = None;
            'attempts: loop {
                let deadline = tokio::time::Instant::now() + timeout;
                while tokio::time::Instant::now() < deadline {
                    if let Ok(Some(f)) = world.current(&result_key) {
                        let id = f.value.get("id").and_then(Value::as_str).unwrap_or("");
                        if ids.iter().any(|i| i == id) {
                            answered = Some(f.value.clone());
                            break 'attempts;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                // Superseded by a later turn wanting the other posture? Then
                // this confirmation is moot; leave the fact to that turn.
                let still_ours = matches!(
                    told.lock().unwrap_or_else(|p| p.into_inner()).get(&node_id),
                    Some(Told::Pending(p)) if *p == posture
                );
                if !still_ours {
                    return;
                }
                let Some(cmd) = retry_cmds.get((attempts - 1) as usize) else {
                    break 'attempts;
                };
                attempts += 1;
                ids.push(cmd.id.clone());
                if let Err(e) = sink.send_command(cmd).await {
                    tracing::warn!(node = %node_id, error = %e, "descending posture resend failed");
                    break 'attempts;
                }
                tracing::info!(node = %node_id, id = %cmd.id, attempt = attempts, "descending posture resent (no reply)");
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            fact["attempts"] = json!(attempts);
            let mut told_map = told.lock().unwrap_or_else(|p| p.into_inner());
            match answered {
                Some(reply) => {
                    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    fact["answered"] = json!(true);
                    fact["ok"] = json!(ok);
                    fact["result"] = reply.get("result").cloned().unwrap_or(Value::Null);
                    if ok {
                        told_map.insert(node_id.clone(), Told::Confirmed(posture));
                        tracing::info!(node = %node_id, posture = posture.as_str(), attempts, "descending posture confirmed by the node");
                    } else {
                        // The node refused: it is not in this posture, and
                        // sending the same thing again will not help.
                        // Leave it unconfirmed so the next turn tries once
                        // more with whatever it wants then; the refusal is
                        // on the fact.
                        told_map.remove(&node_id);
                        let why = reply
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("?")
                            .to_string();
                        fact["error"] = json!(why);
                        tracing::warn!(node = %node_id, posture = posture.as_str(), "descending posture REFUSED by the node: {why}");
                    }
                }
                None => {
                    fact["answered"] = json!(false);
                    told_map.remove(&node_id);
                    tracing::warn!(node = %node_id, posture = posture.as_str(), attempts, "descending posture unanswered after {attempts} attempt(s); will send again next turn");
                }
            }
            drop(told_map);
            let _ = world.observe_as(
                &format!("{FACT_PREFIX}{node_id}"),
                fact,
                now,
                now,
                SOURCE,
                Origin::Observed,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Recorder {
        sent: Mutex<Vec<NodeCommand>>,
        refuse: Mutex<bool>,
    }

    impl Recorder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(Vec::new()),
                refuse: Mutex::new(false),
            })
        }
        fn take(&self) -> Vec<NodeCommand> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }
    }

    #[async_trait]
    impl CommandSink for Recorder {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            if *self.refuse.lock().unwrap() {
                anyhow::bail!("serial I/O thread has exited");
            }
            self.sent.lock().unwrap().push(cmd.clone());
            Ok(())
        }
    }

    fn cfg() -> PostureConfig {
        PostureConfig {
            enabled: true,
            novel_level: 0.15,
            nodes: vec![PostureTarget {
                node_id: "obc-esp32-s3-001".into(),
                slots: vec![0],
            }],
            reply_timeout_ms: 150,
            reply_retries: 1,
        }
    }

    /// A node on the far side of the mesh: answers every command it hears
    /// by writing the `cmd_result` fact the gateway would.
    struct AnsweringNode {
        world: Arc<WorldMemory>,
        sent: Mutex<Vec<NodeCommand>>,
        /// Drop this many commands before answering (a collision each).
        drop_first: Mutex<u32>,
    }

    #[async_trait]
    impl CommandSink for AnsweringNode {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(cmd.clone());
            let mut drop = self.drop_first.lock().unwrap();
            if *drop > 0 {
                *drop -= 1;
                return Ok(()); // lost on the air
            }
            let active = if cmd.args.get("clear") == Some(&json!(true)) {
                json!([])
            } else {
                cmd.args["m"].clone()
            };
            let _ = self.world.observe_as(
                &format!("mesh.{}.cmd_result", cmd.to),
                json!({
                    "id": cmd.id,
                    "node_id": cmd.to,
                    "ok": true,
                    "result": json!({ "active": active, "applied": 1 }).to_string(),
                    "type": "cmd_result",
                }),
                1,
                1,
                "lora-gateway",
                Origin::Observed,
            );
            Ok(())
        }
    }

    /// Long enough for `cfg()`'s first attempt plus one retry to time out
    /// (150 ms each, polled every 100 ms) with room to spare.
    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
    }

    fn novel() -> Assessment {
        Assessment {
            novelty: 0.9,
            novel: true,
            success_prior: None,
        }
    }

    fn familiar() -> Assessment {
        Assessment {
            novelty: 0.2,
            novel: false,
            success_prior: Some(0.8),
        }
    }

    #[test]
    fn the_policy_is_novelty_and_nothing_else() {
        assert_eq!(Posture::for_assessment(&novel()), Posture::Cautious);
        assert_eq!(Posture::for_assessment(&familiar()), Posture::Default);
        // A high novelty that the body has not called novel (warm-up) is not
        // cautious: the body's judgement, not the number, is the signal.
        let warming = Assessment {
            novelty: 0.95,
            novel: false,
            success_prior: None,
        };
        assert_eq!(Posture::for_assessment(&warming), Posture::Default);
    }

    #[tokio::test]
    async fn a_novel_objective_lowers_the_slots_once() {
        let sink = Recorder::new();
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let p = PosturePolicy::new(cfg(), sink.clone(), Some(world.clone()));
        let a = p.apply(&novel(), "open the airlock", 1_000).await;
        assert_eq!(a.posture, Posture::Cautious);
        assert_eq!(a.sent, vec!["obc-esp32-s3-001".to_string()]);
        let sent = sink.take();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].cmd, "descend");
        assert_eq!(sent[0].to, "obc-esp32-s3-001");
        assert_eq!(sent[0].args, json!({ "m": [[0, 0.15]] }));
        assert!(sent[0].fits_one_frame());
        // The same posture again costs nothing.
        let a = p.apply(&novel(), "open the airlock again", 2_000).await;
        assert!(a.sent.is_empty());
        assert!(sink.take().is_empty());
        let fact = world
            .current("descending.obc-esp32-s3-001")
            .unwrap()
            .unwrap();
        assert_eq!(fact.value["posture"], "cautious");
        assert_eq!(fact.value["sent"], true);
        assert_eq!(fact.value["objective"], "open the airlock");
        assert!(fact.value["answered"].is_null(), "not yet confirmed");
    }

    #[tokio::test]
    async fn the_nodes_answer_confirms_the_posture() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let node = Arc::new(AnsweringNode {
            world: world.clone(),
            sent: Mutex::new(Vec::new()),
            drop_first: Mutex::new(0),
        });
        let p = PosturePolicy::new(cfg(), node.clone(), Some(world.clone()));
        assert_eq!(p.confirmed("obc-esp32-s3-001"), None);
        p.apply(&novel(), "x", 1).await;
        settle().await;
        assert_eq!(p.confirmed("obc-esp32-s3-001"), Some(Posture::Cautious));
        let fact = world
            .current("descending.obc-esp32-s3-001")
            .unwrap()
            .unwrap();
        assert_eq!(fact.value["answered"], true);
        assert_eq!(fact.value["ok"], true);
        assert_eq!(fact.value["attempts"], 1);
        assert_eq!(node.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_lost_frame_is_resent_and_the_second_answer_confirms() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let node = Arc::new(AnsweringNode {
            world: world.clone(),
            sent: Mutex::new(Vec::new()),
            drop_first: Mutex::new(1),
        });
        let p = PosturePolicy::new(cfg(), node.clone(), Some(world.clone()));
        p.apply(&novel(), "x", 1).await;
        // While the first attempt is pending, the same posture is not resent
        // by a new turn — the confirmation task owns the retry.
        assert!(p.apply(&novel(), "y", 2).await.sent.is_empty());
        settle().await;
        assert_eq!(p.confirmed("obc-esp32-s3-001"), Some(Posture::Cautious));
        let sent = node.sent.lock().unwrap();
        assert_eq!(sent.len(), 2, "one lost, one resent");
        assert!(
            sent[1].id.ends_with("r1"),
            "the retry carries its own id: {}",
            sent[1].id
        );
        assert_eq!(
            sent[1].args, sent[0].args,
            "and the same idempotent command"
        );
        let fact = world
            .current("descending.obc-esp32-s3-001")
            .unwrap()
            .unwrap();
        assert_eq!(fact.value["answered"], true);
        assert_eq!(fact.value["attempts"], 2);
    }

    #[tokio::test]
    async fn silence_on_every_attempt_is_recorded_and_the_next_turn_sends_again() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let node = Arc::new(AnsweringNode {
            world: world.clone(),
            sent: Mutex::new(Vec::new()),
            drop_first: Mutex::new(u32::MAX), // never answers
        });
        let p = PosturePolicy::new(cfg(), node.clone(), Some(world.clone()));
        p.apply(&novel(), "x", 1).await;
        settle().await;
        assert_eq!(p.confirmed("obc-esp32-s3-001"), None);
        let fact = world
            .current("descending.obc-esp32-s3-001")
            .unwrap()
            .unwrap();
        assert_eq!(fact.value["answered"], false);
        assert_eq!(fact.value["attempts"], 2, "first send plus one retry");
        // The next turn tries again from scratch rather than believing the
        // node is cautious.
        let a = p.apply(&novel(), "z", 3).await;
        assert_eq!(a.sent.len(), 1);
        assert_eq!(node.sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_turn_wanting_the_other_posture_supersedes_a_pending_one() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let node = Arc::new(AnsweringNode {
            world: world.clone(),
            sent: Mutex::new(Vec::new()),
            drop_first: Mutex::new(1),
        });
        let p = PosturePolicy::new(cfg(), node.clone(), Some(world.clone()));
        p.apply(&novel(), "x", 1).await; // lost; pending cautious
        let a = p.apply(&familiar(), "y", 2).await; // wants default: sent now
        assert_eq!(a.sent.len(), 1);
        settle().await;
        assert_eq!(p.confirmed("obc-esp32-s3-001"), Some(Posture::Default));
        let sent = node.sent.lock().unwrap();
        // The cautious retry never went: its confirmation saw it was superseded.
        assert!(
            sent.iter().all(|c| !c.id.ends_with("r1")),
            "{:?}",
            sent.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn a_familiar_objective_clears_and_only_on_the_change() {
        let sink = Recorder::new();
        let p = PosturePolicy::new(cfg(), sink.clone(), None);
        // First turn, familiar: the node has been told nothing, so say so once.
        let a = p.apply(&familiar(), "check the weather", 1).await;
        assert_eq!(a.sent.len(), 1);
        assert_eq!(sink.take()[0].args, json!({ "clear": true, "m": [] }));
        assert!(p
            .apply(&familiar(), "check the weather", 2)
            .await
            .sent
            .is_empty());
        // Novel → cautious, familiar → clear: two sends for two changes.
        assert_eq!(p.apply(&novel(), "x", 3).await.sent.len(), 1);
        assert_eq!(p.apply(&familiar(), "y", 4).await.sent.len(), 1);
        let sent = sink.take();
        assert_eq!(sent[0].args, json!({ "m": [[0, 0.15]] }));
        assert_eq!(sent[1].args, json!({ "clear": true, "m": [] }));
    }

    #[tokio::test]
    async fn a_refused_send_is_reported_and_retried_not_remembered() {
        let sink = Recorder::new();
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let p = PosturePolicy::new(cfg(), sink.clone(), Some(world.clone()));
        *sink.refuse.lock().unwrap() = true;
        let a = p.apply(&novel(), "x", 1).await;
        assert!(a.sent.is_empty());
        assert_eq!(a.failed.len(), 1);
        let fact = world
            .current("descending.obc-esp32-s3-001")
            .unwrap()
            .unwrap();
        assert_eq!(fact.value["sent"], false);
        assert!(fact.value["error"].as_str().unwrap().contains("exited"));
        // The sink comes back: the same posture is sent now, because it was
        // never told.
        *sink.refuse.lock().unwrap() = false;
        let a = p.apply(&novel(), "x", 2).await;
        assert_eq!(a.sent.len(), 1);
        assert_eq!(sink.take().len(), 1);
    }

    #[tokio::test]
    async fn every_configured_slot_on_every_node_moves() {
        let sink = Recorder::new();
        let mut c = cfg();
        c.nodes.push(PostureTarget {
            node_id: "obc-esp32-s3-002".into(),
            slots: vec![1, 3],
        });
        let p = PosturePolicy::new(c, sink.clone(), None);
        p.apply(&novel(), "x", 1).await;
        let sent = sink.take();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].to, "obc-esp32-s3-002");
        assert_eq!(sent[1].args, json!({ "m": [[1, 0.15], [3, 0.15]] }));
    }

    #[tokio::test]
    async fn disabled_sends_nothing() {
        let sink = Recorder::new();
        let mut c = cfg();
        c.enabled = false;
        let p = PosturePolicy::new(c, sink.clone(), None);
        let a = p.apply(&novel(), "x", 1).await;
        assert_eq!(a.posture, Posture::Cautious, "the judgement is still made");
        assert!(a.sent.is_empty());
        assert!(sink.take().is_empty());
    }

    #[test]
    fn the_config_refuses_what_would_do_nothing_or_send_garbage() {
        let mut c = cfg();
        assert!(c.validate().is_ok());
        c.nodes.clear();
        assert!(c.validate().is_err(), "enabled with no nodes");
        let mut c = cfg();
        c.nodes[0].slots = vec![obc_reflex::MAX_SLOTS as u8];
        assert!(c.validate().is_err(), "slot out of range");
        let mut c = cfg();
        c.novel_level = 1.5;
        assert!(c.validate().is_err(), "level out of range");
        let mut c = cfg();
        c.enabled = false;
        c.nodes.clear();
        assert!(c.validate().is_ok(), "disabled needs nothing");
        // Wire shape, and a misspelled key is a load failure, not a silent
        // default (the shipped-config test in obc-config covers the TOML).
        let parsed: PostureConfig = serde_json::from_value(json!({
            "enabled": true,
            "novel_level": 0.2,
            "nodes": [{ "node_id": "obc-esp32-s3-001", "slots": [0] }],
        }))
        .unwrap();
        assert_eq!(parsed.nodes[0].slots, vec![0]);
        assert!(serde_json::from_value::<PostureConfig>(
            json!({ "enabled": true, "novel_levle": 0.2 })
        )
        .is_err());
    }

    // ── Rung 1 step A: the graded map ───────────────────────────────────
    //
    // The numbers below are the test's own, not proposed constants. What is
    // asserted is the map's *shape* — monotone, clamped, quantised, and
    // clearing below the floor — because those are the properties step B
    // will rely on. The knees, step and floor come from the replay.

    fn map() -> LevelMap {
        LevelMap {
            caution_knee: 0.05,
            caution_full: 0.45,
            rule_default: 0.5,
            novel_level: 0.15,
            step: 0.05,
            clear_below: 0.1,
        }
    }

    #[test]
    fn caution_is_clamped_monotone_and_flat_outside_the_knees() {
        let m = map();
        assert_eq!(m.caution(0.0), 0.0);
        assert_eq!(m.caution(0.05), 0.0, "at the lower knee");
        assert_eq!(m.caution(0.45), 1.0, "at the upper knee");
        assert_eq!(m.caution(1.0), 1.0);
        assert!((m.caution(0.25) - 0.5).abs() < 1e-6, "linear between");
        let mut last = -1.0;
        for i in 0..=100 {
            let c = m.caution(i as f32 / 100.0);
            assert!(c >= last, "not monotone at {i}");
            assert!((0.0..=1.0).contains(&c));
            last = c;
        }
    }

    #[test]
    fn degenerate_knees_are_a_step_function_not_a_nan() {
        let m = LevelMap {
            caution_knee: 0.4,
            caution_full: 0.4,
            ..map()
        };
        assert_eq!(m.caution(0.39), 0.0);
        assert_eq!(m.caution(0.4), 1.0);
        let inverted = LevelMap {
            caution_knee: 0.6,
            caution_full: 0.2,
            ..map()
        };
        assert!(inverted.caution(0.5).is_finite());
    }

    #[test]
    fn a_novelty_that_is_not_a_number_clears_rather_than_reaching_the_radio() {
        let m = map();
        assert_eq!(m.caution(f32::NAN), 0.0);
        assert_eq!(m.descent(f32::NAN), Descent::Clear);
        // And no path produces a level that is not a number.
        for m in [
            map(),
            LevelMap {
                caution_knee: f32::NAN,
                ..map()
            },
            LevelMap {
                caution_full: f32::NAN,
                ..map()
            },
        ] {
            for n in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 2.0] {
                if let Descent::Level(l) = m.descent(n) {
                    assert!(l.is_finite(), "level {l} from novelty {n}");
                }
            }
        }
    }

    #[test]
    fn the_level_interpolates_between_the_endpoints_and_quantises() {
        let m = map();
        assert!(
            (m.level(0.0) - 0.5).abs() < 1e-9,
            "caution 0 = rule default"
        );
        assert!(
            (m.level(1.0) - 0.15).abs() < 1e-9,
            "caution 1 = novel level"
        );
        // caution 0.4 → 0.5 − 0.35·0.4 = 0.36 raw → 7.2 steps → 0.35.
        assert!((m.level(0.4) - 0.35).abs() < 1e-9, "{}", m.level(0.4));
        // Quantisation is what makes near-equal turns cost no frame:
        // novelty 0.22 and 0.24 are caution 0.425 and 0.475, levels 0.351
        // and 0.334 raw, and both round to 0.35.
        assert_eq!(m.descent(0.22), m.descent(0.24));
        // But the deadband only merges turns inside a bucket. 0.24 and 0.26
        // straddle a boundary (6.675 and 6.325 steps) and so are a frame
        // apart — which is the cost step B is choosing the step size to
        // bound, not a property the map can promise away.
        assert_ne!(m.descent(0.24), m.descent(0.26));
    }

    #[test]
    fn caution_under_the_floor_clears_rather_than_sending_a_level() {
        let m = map();
        // Below the lower knee there is nothing to be cautious about.
        assert_eq!(m.descent(0.01), Descent::Clear);
        assert_eq!(m.descent(0.05), Descent::Clear);
        // Just above the floor (caution 0.1 → novelty 0.09) a level starts.
        assert!(matches!(m.descent(0.2), Descent::Level(_)));
        assert!(matches!(m.descent(1.0), Descent::Level(l) if (l - 0.15).abs() < 1e-9));
    }

    #[test]
    fn a_level_that_quantises_back_onto_the_rule_default_is_a_clear() {
        // Found by the replay, not by design: a caution just over the floor
        // produces a level that rounds to `rule_default`, and sending it
        // spends a frame telling the node to hold the number its own rule
        // already holds. On the brain's 99 episodes that was most of the
        // traffic the graded map added.
        let m = LevelMap {
            caution_knee: 0.0,
            caution_full: 0.45,
            clear_below: 0.0,
            ..map()
        };
        // novelty 0.027 → caution 0.06 → 0.479 raw → 0.50 = the rule default.
        assert!(
            (m.level(m.caution(0.027)) - m.rule_default).abs() < 1e-9,
            "the arithmetic this test is about has changed"
        );
        assert_eq!(m.descent(0.027), Descent::Clear);
        // One step away is a real level and does get sent.
        assert!(matches!(m.descent(0.10), Descent::Level(_)));
    }

    #[test]
    fn the_map_is_a_pure_function() {
        let m = map();
        for i in 0..=50 {
            let n = i as f32 / 50.0;
            assert_eq!(m.descent(n), m.descent(n));
        }
    }
}

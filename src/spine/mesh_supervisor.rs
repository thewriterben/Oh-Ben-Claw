//! Mesh supervisor — fold the LoRa mesh into the brain (Phase B).
//!
//! The inbound gateway bridge lands node messages in world memory; this loop *acts on*
//! them. Each tick it derives a per-node health view (online / degraded / offline) from
//! the mesh facts, writes it back to world memory (so reflexes, foresight, and the agent
//! can see it), and — when a node goes offline — can autonomously issue a rate-limited
//! recovery command over the mesh.
//!
//! ```text
//! perception            decision                 action
//! mesh.<node>.*  ─►  derive health  ─►  observe mesh.<node>.health
//! (world memory)     (online/degraded/    + (if offline) send recovery
//!                     offline)              mesh_command via the sink
//! ```
//!
//! The decision core ([`decide`]) is pure and unit-tested; the driver ([`tick`]) reads
//! the real store and drives the mesh command sink.

use crate::config::MeshSupervisorConfig;
use crate::memory::world::WorldMemory;
use crate::spine::lora_gateway::{CommandSink, NodeCommand};
use serde_json::json;
use std::sync::Arc;

/// Derived health of a mesh node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshHealth {
    /// Heard recently, last command (if any) succeeded.
    Online,
    /// Heard recently, but the last command result was not ok.
    Degraded,
    /// No mesh message within the staleness window.
    Offline,
}

impl MeshHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            MeshHealth::Online => "online",
            MeshHealth::Degraded => "degraded",
            MeshHealth::Offline => "offline",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "online" => Some(Self::Online),
            "degraded" => Some(Self::Degraded),
            "offline" => Some(Self::Offline),
            _ => None,
        }
    }
}

/// A compact per-node snapshot the driver extracts from world memory for a decision.
#[derive(Debug, Clone)]
pub struct MeshNodeView {
    pub node: String,
    /// `valid_from` of the node's latest `mesh.<node>` rollup fact (ms).
    pub last_seen_ms: u64,
    /// `ok` of the node's latest `cmd_result`, if any.
    pub last_cmd_ok: Option<bool>,
    /// Previously-recorded health (so we only write on change).
    pub prev_health: Option<MeshHealth>,
    /// `valid_from` of the current `mesh.<node>.health` fact — marks when the current
    /// health began, for measuring continuous-offline duration.
    pub health_since_ms: Option<u64>,
    /// When we last sent a recovery command to this node (ms), if ever.
    pub last_recovery_ms: Option<u64>,
    /// Whether the node is currently escalated (presumed lost).
    pub escalated: bool,
}

/// A supervisor decision to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshDecision {
    /// Record/refresh the node's derived health (only emitted when it changes).
    Health { node: String, status: &'static str, reason: String },
    /// Issue a recovery command to an offline node.
    Recover { node: String, cmd: NodeCommand },
    /// Escalate: the node has been offline long enough to be presumed lost (recovery
    /// stops).
    Escalate { node: String, reason: String },
    /// Clear a prior escalation: the node came back.
    ClearEscalation { node: String },
}

/// Pure decision core: from per-node views + now + config, produce the actions to apply.
/// Health is emitted only when it *changes* (no churn); recovery only for offline nodes
/// when `recover` is configured and the per-node rate limit has elapsed.
pub fn decide(views: &[MeshNodeView], now_ms: u64, cfg: &MeshSupervisorConfig) -> Vec<MeshDecision> {
    let mut out = Vec::new();
    for v in views {
        let age = now_ms.saturating_sub(v.last_seen_ms);
        let (status, reason) = if age > cfg.stale_ms {
            (MeshHealth::Offline, format!("no mesh message for {age} ms"))
        } else if v.last_cmd_ok == Some(false) {
            (MeshHealth::Degraded, "last command result was not ok".to_string())
        } else {
            (MeshHealth::Online, "healthy".to_string())
        };

        if v.prev_health != Some(status) {
            out.push(MeshDecision::Health { node: v.node.clone(), status: status.as_str(), reason });
        }

        if status == MeshHealth::Offline {
            // Continuous-offline duration: if it was already offline, the health fact's
            // valid_from marks when it began; if it went offline this tick, that's ~now.
            let offline_since = if v.prev_health == Some(MeshHealth::Offline) {
                v.health_since_ms.unwrap_or(now_ms)
            } else {
                now_ms
            };
            let offline_for = now_ms.saturating_sub(offline_since);
            let escalate_now =
                cfg.escalate_after_ms > 0 && !v.escalated && offline_for >= cfg.escalate_after_ms;

            if escalate_now {
                out.push(MeshDecision::Escalate {
                    node: v.node.clone(),
                    reason: format!("offline for {offline_for} ms — presumed lost"),
                });
            }

            // Keep pinging until we've given up (escalated, including this tick).
            if !v.escalated && !escalate_now {
                if let Some(cmd_name) = &cfg.recover {
                    let due = v
                        .last_recovery_ms
                        .is_none_or(|t| now_ms.saturating_sub(t) >= cfg.min_recovery_interval_ms);
                    if due {
                        let id = format!("sup-{}-{}", v.node, now_ms);
                        out.push(MeshDecision::Recover {
                            node: v.node.clone(),
                            cmd: NodeCommand::new(&v.node, id, cmd_name, json!({})),
                        });
                    }
                }
            }
        } else if v.escalated {
            // The node returned after being presumed lost → clear the escalation.
            out.push(MeshDecision::ClearEscalation { node: v.node.clone() });
        }
    }
    out
}

/// Extract the current per-node views from world memory (rollup + last cmd_result +
/// prior health + last recovery). Mesh nodes are the `mesh.<node>` rollup entities
/// (node ids contain no dots, so a rollup splits into exactly two dot-parts).
pub fn snapshot(world: &WorldMemory) -> Vec<MeshNodeView> {
    let entities = world.entities().unwrap_or_default();
    let mut views = Vec::new();
    for e in entities {
        let parts: Vec<&str> = e.split('.').collect();
        // A mesh node rollup is `mesh.<node>` (exactly two dot-parts). Skip the
        // supervisor's own aggregate `mesh.escalated_count`, which also has two parts
        // but is a bare counter, not a node — otherwise it is mis-parsed as a phantom
        // node (appearing from the tick *after* it is first written).
        if parts.len() != 2 || parts[0] != "mesh" || parts[1] == "escalated_count" {
            continue;
        }
        let node = parts[1].to_string();
        let last_seen_ms = match world.current(&e).ok().flatten() {
            Some(f) => f.valid_from,
            None => continue,
        };
        let last_cmd_ok = world
            .current(&format!("mesh.{node}.cmd_result"))
            .ok()
            .flatten()
            .and_then(|f| f.value.get("ok").and_then(|v| v.as_bool()));
        let health_fact = world.current(&format!("mesh.{node}.health")).ok().flatten();
        let prev_health = health_fact
            .as_ref()
            .and_then(|f| f.value.get("status").and_then(|v| v.as_str()).and_then(MeshHealth::parse));
        let health_since_ms = health_fact.as_ref().map(|f| f.valid_from);
        let last_recovery_ms = world
            .current(&format!("mesh.{node}.recovery"))
            .ok()
            .flatten()
            .map(|f| f.valid_from);
        let escalated = world
            .current(&format!("mesh.{node}.escalation"))
            .ok()
            .flatten()
            .and_then(|f| f.value.get("status").and_then(|v| v.as_str()).map(|s| s == "escalated"))
            .unwrap_or(false);
        views.push(MeshNodeView {
            node,
            last_seen_ms,
            last_cmd_ok,
            prev_health,
            health_since_ms,
            last_recovery_ms,
            escalated,
        });
    }
    views
}

/// The last `limit` RSSI readings for a node, oldest→newest, for a sparkline.
///
/// Reads the node's `mesh.<node>` rollup history and pulls each fact's `rssi_dbm`
/// (skipping facts that carried no RSSI). Returns at most `limit` values.
pub fn rssi_series(world: &WorldMemory, node: &str, limit: usize) -> Vec<i64> {
    let mut series: Vec<i64> = world
        .history(&format!("mesh.{node}"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.value.get("rssi_dbm").and_then(|r| r.as_i64()))
        .collect();
    if series.len() > limit {
        series = series.split_off(series.len() - limit); // keep the newest `limit`
    }
    series
}

/// Recent mesh-relevant escalations from the notifications log-of-record, newest first.
///
/// Reads the `notifications.escalation` history (the durable channel written by the
/// notifier), drops periodic digest entries, classifies each by severity, and returns up
/// to `limit` entries as `{ ts_ms, age_s, severity, reason }` (reason trimmed to its
/// first sentence). Shared by the `mesh_status` tool and the gateway route.
pub fn recent_escalations(world: &WorldMemory, limit: usize) -> Vec<serde_json::Value> {
    use crate::agent::notify::{Severity, DIGEST_PREFIX};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut out: Vec<serde_json::Value> = world
        .history("notifications.escalation")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            f.value
                .get("reason")
                .and_then(|r| r.as_str())
                .map(|r| (f.valid_from, r.to_string()))
        })
        .filter(|(_, r)| !r.starts_with(DIGEST_PREFIX))
        .map(|(ts, reason)| {
            let head = reason.split_once(". ").map(|(h, _)| h).unwrap_or(&reason).to_string();
            json!({
                "ts_ms": ts,
                "age_s": now.saturating_sub(ts) / 1000,
                "severity": Severity::classify(&reason).as_str(),
                "reason": head,
            })
        })
        .collect();
    out.reverse(); // history is oldest-first; surface newest first
    out.truncate(limit);
    out
}

/// Build the read-only mesh status JSON — the single source of truth shared by the
/// `mesh_status` agent tool and the `GET /api/v1/mesh/status` gateway route.
///
/// Returns `{ summary: { nodes, online, degraded, offline, escalated }, nodes: [ … ],
/// escalations: [ … ] }`, where each node carries `health`, `escalated`, `rssi_dbm`,
/// `last_type`, `age_s` (seconds since last heard), and `last_cmd_ok`, and each
/// escalation carries `ts_ms`, `age_s`, `severity`, and `reason`.
pub fn status_json(world: &WorldMemory) -> serde_json::Value {
    let views = snapshot(world);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let (mut online, mut degraded, mut offline, mut escalated) = (0u64, 0u64, 0u64, 0u64);
    let mut nodes = Vec::with_capacity(views.len());
    for v in &views {
        let health = v.prev_health.map(|h| h.as_str()).unwrap_or("unknown");
        match health {
            "online" => online += 1,
            "degraded" => degraded += 1,
            "offline" => offline += 1,
            _ => {}
        }
        if v.escalated {
            escalated += 1;
        }
        let rollup = world.current(&format!("mesh.{}", v.node)).ok().flatten();
        let rssi = rollup
            .as_ref()
            .and_then(|f| f.value.get("rssi_dbm").and_then(|r| r.as_i64()));
        let last_type = rollup
            .as_ref()
            .and_then(|f| f.value.get("last_type").and_then(|t| t.as_str()))
            .unwrap_or("-")
            .to_string();
        nodes.push(json!({
            "node": v.node,
            "health": health,
            "escalated": v.escalated,
            "rssi_dbm": rssi,
            "rssi_history": rssi_series(world, &v.node, 24),
            "last_type": last_type,
            "age_s": now.saturating_sub(v.last_seen_ms) / 1000,
            "last_cmd_ok": v.last_cmd_ok,
        }));
    }

    json!({
        "summary": {
            "nodes": views.len(),
            "online": online,
            "degraded": degraded,
            "offline": offline,
            "escalated": escalated,
        },
        "nodes": nodes,
        "escalations": recent_escalations(world, 10),
    })
}

/// One supervisor tick: snapshot → decide → apply. Health decisions are observed into
/// world memory; recovery decisions are sent through `sink` (when present) and recorded.
/// Returns the number of actions applied.
pub async fn tick(
    world: &WorldMemory,
    sink: Option<&Arc<dyn CommandSink>>,
    cfg: &MeshSupervisorConfig,
    now_ms: u64,
) -> usize {
    let views = snapshot(world);
    let decisions = decide(&views, now_ms, cfg);
    let mut applied = 0;
    for d in decisions {
        match d {
            MeshDecision::Health { node, status, reason } => {
                let _ = world.observe(
                    &format!("mesh.{node}.health"),
                    json!({ "status": status, "reason": reason, "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    "mesh-supervisor",
                );
                applied += 1;
            }
            MeshDecision::Recover { node, cmd } => {
                if let Some(s) = sink {
                    if s.send_command(&cmd).await.is_ok() {
                        let _ = world.observe(
                            &format!("mesh.{node}.recovery"),
                            json!({ "cmd": cmd.cmd, "id": cmd.id, "ts_ms": now_ms }),
                            now_ms,
                            now_ms,
                            "mesh-supervisor",
                        );
                        applied += 1;
                    }
                }
            }
            MeshDecision::Escalate { node, reason } => {
                tracing::warn!(node = %node, "mesh supervisor: node presumed lost — {reason}");
                let _ = world.observe(
                    &format!("mesh.{node}.escalation"),
                    json!({ "status": "escalated", "reason": reason, "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    "mesh-supervisor",
                );
                applied += 1;
            }
            MeshDecision::ClearEscalation { node } => {
                tracing::info!(node = %node, "mesh supervisor: node returned — escalation cleared");
                let _ = world.observe(
                    &format!("mesh.{node}.escalation"),
                    json!({ "status": "cleared", "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    "mesh-supervisor",
                );
                applied += 1;
            }
        }
    }

    // Aggregate signal for the reflex engine (health-driven reflex): the number of
    // nodes currently presumed lost. The standard `safe-mesh-node-lost` reflex rule
    // watches `mesh.escalated_count` and escalates to System 2. Recomputed after the
    // decisions above and written only on change (so a plain number, not churn).
    if !views.is_empty() {
        let escalated_count = views
            .iter()
            .filter(|v| {
                world
                    .current(&format!("mesh.{}.escalation", v.node))
                    .ok()
                    .flatten()
                    .and_then(|f| f.value.get("status").and_then(|s| s.as_str()).map(|s| s == "escalated"))
                    .unwrap_or(false)
            })
            .count() as u64;
        let prev = world.current("mesh.escalated_count").ok().flatten().and_then(|f| f.value.as_u64());
        if prev != Some(escalated_count) {
            let _ = world.observe(
                "mesh.escalated_count",
                json!(escalated_count),
                now_ms,
                now_ms,
                "mesh-supervisor",
            );
        }
    }

    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn cfg(recover: Option<&str>) -> MeshSupervisorConfig {
        MeshSupervisorConfig {
            enabled: true,
            stale_ms: 5_000,
            tick_ms: 5_000,
            recover: recover.map(str::to_string),
            min_recovery_interval_ms: 30_000,
            escalate_after_ms: 0,
        }
    }

    fn view(node: &str, last_seen_ms: u64) -> MeshNodeView {
        MeshNodeView {
            node: node.to_string(),
            last_seen_ms,
            last_cmd_ok: None,
            prev_health: None,
            health_since_ms: None,
            last_recovery_ms: None,
            escalated: false,
        }
    }

    #[test]
    fn a_fresh_node_is_online_and_needs_no_recovery() {
        let d = decide(&[view("n", 10_000)], 11_000, &cfg(Some("capabilities")));
        assert_eq!(d.len(), 1);
        assert!(matches!(&d[0], MeshDecision::Health { status: "online", .. }));
    }

    #[test]
    fn a_stale_node_goes_offline_and_is_recovered() {
        // last seen at 1_000, now 11_000, stale_ms 5_000 → offline.
        let d = decide(&[view("n", 1_000)], 11_000, &cfg(Some("capabilities")));
        assert!(d.iter().any(|x| matches!(x, MeshDecision::Health { status: "offline", .. })));
        let rec = d.iter().find_map(|x| match x {
            MeshDecision::Recover { cmd, .. } => Some(cmd),
            _ => None,
        });
        let rec = rec.expect("offline node is recovered");
        assert_eq!(rec.cmd, "capabilities");
        assert_eq!(rec.to, "n");
    }

    #[test]
    fn recovery_is_rate_limited_per_node() {
        let mut v = view("n", 1_000);
        v.last_recovery_ms = Some(10_500); // recovered 500 ms ago; interval is 30 s
        let d = decide(&[v], 11_000, &cfg(Some("capabilities")));
        assert!(!d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })), "within the cooldown");
    }

    #[test]
    fn observe_only_when_no_recover_command_is_set() {
        let d = decide(&[view("n", 1_000)], 11_000, &cfg(None));
        assert!(d.iter().all(|x| matches!(x, MeshDecision::Health { .. })), "no recovery without a command");
    }

    #[test]
    fn a_failed_command_marks_the_node_degraded() {
        let mut v = view("n", 10_500); // fresh
        v.last_cmd_ok = Some(false);
        let d = decide(&[v], 11_000, &cfg(None));
        assert!(matches!(&d[0], MeshDecision::Health { status: "degraded", .. }));
    }

    #[test]
    fn health_is_not_rewritten_when_unchanged() {
        let mut v = view("n", 10_500);
        v.prev_health = Some(MeshHealth::Online);
        let d = decide(&[v], 11_000, &cfg(None));
        assert!(d.is_empty(), "online→online produces no churn");
    }

    struct MockSink {
        sent: Mutex<Vec<NodeCommand>>,
    }
    #[async_trait::async_trait]
    impl CommandSink for MockSink {
        async fn send_command(&self, cmd: &NodeCommand) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(cmd.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn tick_marks_offline_and_sends_one_recovery_then_backs_off() {
        let world = WorldMemory::open_in_memory().unwrap();
        // A node last heard at t=1_000.
        world
            .observe(
                "mesh.node-x",
                json!({ "last_type": "link_state", "rssi_dbm": -50, "seq": 1, "src": "2A" }),
                1_000,
                1_000,
                "test",
            )
            .unwrap();

        let mock = Arc::new(MockSink { sent: Mutex::new(Vec::new()) });
        let sink: Arc<dyn CommandSink> = mock.clone();
        let c = cfg(Some("capabilities"));

        // First tick well past the staleness window → offline + one recovery.
        let n = tick(&world, Some(&sink), &c, 11_000).await;
        assert!(n >= 2, "health + recovery applied");
        let h = world.current("mesh.node-x.health").unwrap().unwrap();
        assert_eq!(h.value["status"], json!("offline"));
        assert_eq!(mock.sent.lock().unwrap().len(), 1);
        assert_eq!(mock.sent.lock().unwrap()[0].cmd, "capabilities");
        assert_eq!(mock.sent.lock().unwrap()[0].to, "node-x");

        // A second tick moments later: still offline (health unchanged → no rewrite) and
        // recovery is rate-limited → no new command.
        let n2 = tick(&world, Some(&sink), &c, 11_500).await;
        assert_eq!(n2, 0, "no churn, no repeat recovery within the cooldown");
        assert_eq!(mock.sent.lock().unwrap().len(), 1);
    }

    fn esc_cfg() -> MeshSupervisorConfig {
        let mut c = cfg(Some("capabilities"));
        c.escalate_after_ms = 20_000;
        c
    }

    fn offline_view(node: &str, offline_since: u64) -> MeshNodeView {
        let mut v = view(node, offline_since);
        v.prev_health = Some(MeshHealth::Offline);
        v.health_since_ms = Some(offline_since);
        v
    }

    #[test]
    fn a_node_offline_past_the_threshold_is_escalated_and_stops_pinging() {
        // offline since 1_000, now 30_000 → offline_for 29_000 >= 20_000.
        let d = decide(&[offline_view("n", 1_000)], 30_000, &esc_cfg());
        assert!(d.iter().any(|x| matches!(x, MeshDecision::Escalate { .. })), "escalates");
        assert!(!d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })), "gives up pinging");
    }

    #[test]
    fn an_escalated_node_is_not_re_escalated_nor_pinged() {
        let mut v = offline_view("n", 1_000);
        v.escalated = true;
        let d = decide(&[v], 30_000, &esc_cfg());
        assert!(d.is_empty(), "no re-escalation, no recovery, no health churn");
    }

    #[test]
    fn recovery_continues_before_the_escalation_threshold() {
        // offline_for 9_000 < 20_000 → still recovering, not escalated.
        let d = decide(&[offline_view("n", 1_000)], 10_000, &esc_cfg());
        assert!(d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })));
        assert!(!d.iter().any(|x| matches!(x, MeshDecision::Escalate { .. })));
    }

    #[test]
    fn a_returning_node_clears_its_escalation() {
        // fresh (online) but previously escalated → clear.
        let mut v = view("n", 29_500);
        v.prev_health = Some(MeshHealth::Offline);
        v.escalated = true;
        let d = decide(&[v], 30_000, &esc_cfg());
        assert!(d.iter().any(|x| matches!(x, MeshDecision::ClearEscalation { .. })));
    }

    #[tokio::test]
    async fn tick_escalates_a_long_offline_node_then_clears_on_return() {
        let world = WorldMemory::open_in_memory().unwrap();
        world.observe("mesh.n", json!({ "last_type": "link_state" }), 1_000, 1_000, "test").unwrap();
        world
            .observe("mesh.n.health", json!({ "status": "offline", "reason": "x" }), 1_000, 1_000, "test")
            .unwrap();
        let c = esc_cfg();
        let mock = Arc::new(MockSink { sent: Mutex::new(Vec::new()) });
        let sink: Arc<dyn CommandSink> = mock.clone();

        // Offline for 29 s (>= 20 s threshold) → escalate, no recovery ping.
        tick(&world, Some(&sink), &c, 30_000).await;
        assert_eq!(
            world.current("mesh.n.escalation").unwrap().unwrap().value["status"],
            json!("escalated")
        );
        assert_eq!(mock.sent.lock().unwrap().len(), 0, "escalated → no ping");

        // Node returns (fresh rollup) → escalation cleared.
        world.observe("mesh.n", json!({ "last_type": "link_state" }), 30_500, 30_500, "test").unwrap();
        tick(&world, Some(&sink), &c, 31_000).await;
        assert_eq!(
            world.current("mesh.n.escalation").unwrap().unwrap().value["status"],
            json!("cleared")
        );
    }

    #[tokio::test]
    async fn escalation_raises_the_count_that_drives_a_reflex() {
        use crate::agent::reflex::{Action, ReflexEngine};
        use crate::agent::safing::{standard_safing_rules, SafingOptions};

        let world = WorldMemory::open_in_memory().unwrap();
        world.observe("mesh.n", json!({ "last_type": "link_state" }), 1_000, 1_000, "test").unwrap();
        world
            .observe("mesh.n.health", json!({ "status": "offline" }), 1_000, 1_000, "test")
            .unwrap();
        let mut c = esc_cfg();
        c.recover = None; // observe-only; escalation is time-based and still fires

        // Supervisor escalates the long-offline node → publishes the aggregate count.
        tick(&world, None, &c, 30_000).await;
        assert_eq!(
            world.current("mesh.escalated_count").unwrap().unwrap().value.as_u64(),
            Some(1)
        );

        // A standard safing engine reads world memory and fires the health-driven
        // escalate — the mesh's presumed-lost node wakes System 2.
        let engine = ReflexEngine::new(standard_safing_rules(&SafingOptions::default()));
        let fired = engine.tick(&world, 40_000).unwrap();
        assert!(
            fired.iter().any(|f| f.rule_id == "safe-mesh-node-lost"
                && matches!(f.action, Action::Escalate { .. })),
            "mesh health drives a reflex escalation"
        );
    }

    #[test]
    fn snapshot_ignores_the_escalated_count_aggregate() {
        // Regression: `mesh.escalated_count` is a bare counter with two dot-parts, so it
        // must NOT be mistaken for a `mesh.<node>` rollup (which would appear as a
        // phantom "online" node from the tick after it is first written).
        let world = WorldMemory::open_in_memory().unwrap();
        world.observe("mesh.n1", json!({ "last_type": "reflex" }), 1_000, 1_000, "t").unwrap();
        world.observe("mesh.escalated_count", json!(0), 1_000, 1_000, "t").unwrap();

        let views = snapshot(&world);
        assert_eq!(views.len(), 1, "only the real node is a node");
        assert_eq!(views[0].node, "n1");
        // And the status JSON shows exactly one node, not the aggregate.
        let v = status_json(&world);
        assert_eq!(v["summary"]["nodes"], json!(1));
    }

    #[test]
    fn status_json_matches_the_mesh_status_shape() {
        // The gateway route and the mesh_status tool both call status_json — one SSOT.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe("mesh.n1", json!({ "last_type": "reflex", "rssi_dbm": -72 }), 1_000, 1_000, "t")
            .unwrap();
        world.observe("mesh.n1.health", json!({ "status": "online" }), 1_000, 1_000, "t").unwrap();
        // A second node that is offline and escalated.
        world.observe("mesh.n2", json!({ "last_type": "-" }), 1_000, 1_000, "t").unwrap();
        world.observe("mesh.n2.health", json!({ "status": "offline" }), 1_000, 1_000, "t").unwrap();
        world
            .observe("mesh.n2.escalation", json!({ "status": "escalated" }), 1_000, 1_000, "t")
            .unwrap();

        let v = status_json(&world);
        assert_eq!(v["summary"]["nodes"], json!(2));
        assert_eq!(v["summary"]["online"], json!(1));
        assert_eq!(v["summary"]["offline"], json!(1));
        assert_eq!(v["summary"]["escalated"], json!(1));

        let nodes = v["nodes"].as_array().unwrap();
        let n1 = nodes.iter().find(|n| n["node"] == json!("n1")).unwrap();
        assert_eq!(n1["health"], json!("online"));
        assert_eq!(n1["rssi_dbm"], json!(-72));
        assert_eq!(n1["last_type"], json!("reflex"));
        assert_eq!(n1["escalated"], json!(false));
        let n2 = nodes.iter().find(|n| n["node"] == json!("n2")).unwrap();
        assert_eq!(n2["health"], json!("offline"));
        assert_eq!(n2["escalated"], json!(true));
        assert!(n2["rssi_dbm"].is_null());
        // Escalations feed is present (empty here — no notifications logged).
        assert_eq!(v["escalations"], json!([]));
    }

    #[test]
    fn rssi_series_keeps_the_newest_readings_oldest_first() {
        let world = WorldMemory::open_in_memory().unwrap();
        // Four rollups for n1; one carries no rssi and is skipped.
        world.observe("mesh.n1", json!({ "rssi_dbm": -60 }), 1_000, 1_000, "t").unwrap();
        world.observe("mesh.n1", json!({ "rssi_dbm": -70 }), 2_000, 2_000, "t").unwrap();
        world.observe("mesh.n1", json!({ "last_type": "reflex" }), 3_000, 3_000, "t").unwrap();
        world.observe("mesh.n1", json!({ "rssi_dbm": -80 }), 4_000, 4_000, "t").unwrap();

        let full = rssi_series(&world, "n1", 24);
        assert_eq!(full, vec![-60, -70, -80], "oldest→newest, rssi-less fact skipped");
        // Limit keeps the newest N.
        let last2 = rssi_series(&world, "n1", 2);
        assert_eq!(last2, vec![-70, -80]);

        // Surfaced per-node in status_json.
        let v = status_json(&world);
        let n1 = v["nodes"].as_array().unwrap().iter().find(|n| n["node"] == json!("n1")).unwrap();
        assert_eq!(n1["rssi_history"], json!([-60, -70, -80]));
    }

    #[test]
    fn recent_escalations_are_newest_first_and_skip_digests() {
        let world = WorldMemory::open_in_memory().unwrap();
        // Two real escalations + one periodic digest that must be filtered out.
        world
            .observe(
                "notifications.escalation",
                json!({ "reason": "node n1 offline. run mesh_status" }),
                1_000, 1_000, "notify",
            )
            .unwrap();
        world
            .observe(
                "notifications.escalation",
                json!({ "reason": "node n2 presumed lost. escalate" }),
                2_000, 2_000, "notify",
            )
            .unwrap();
        world
            .observe(
                "notifications.escalation",
                json!({ "reason": format!("{} 3 events", crate::agent::notify::DIGEST_PREFIX) }),
                3_000, 3_000, "notify",
            )
            .unwrap();

        let esc = recent_escalations(&world, 10);
        assert_eq!(esc.len(), 2, "digest is filtered out");
        // Newest first: the presumed-lost (ts 2_000) leads and is critical.
        assert_eq!(esc[0]["ts_ms"], json!(2_000));
        assert_eq!(esc[0]["severity"], json!("critical"));
        assert_eq!(esc[0]["reason"], json!("node n2 presumed lost"));
        assert_eq!(esc[1]["ts_ms"], json!(1_000));

        // status_json surfaces the same feed.
        world.observe("mesh.n1", json!({ "last_type": "reflex" }), 1_000, 1_000, "t").unwrap();
        let v = status_json(&world);
        assert_eq!(v["escalations"].as_array().unwrap().len(), 2);
    }
}

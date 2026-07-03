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
    /// When we last sent a recovery command to this node (ms), if ever.
    pub last_recovery_ms: Option<u64>,
}

/// A supervisor decision to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshDecision {
    /// Record/refresh the node's derived health (only emitted when it changes).
    Health { node: String, status: &'static str, reason: String },
    /// Issue a recovery command to an offline node.
    Recover { node: String, cmd: NodeCommand },
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
            if let Some(cmd_name) = &cfg.recover {
                let due = v
                    .last_recovery_ms
                    .map_or(true, |t| now_ms.saturating_sub(t) >= cfg.min_recovery_interval_ms);
                if due {
                    let id = format!("sup-{}-{}", v.node, now_ms);
                    out.push(MeshDecision::Recover {
                        node: v.node.clone(),
                        cmd: NodeCommand::new(&v.node, id, cmd_name, json!({})),
                    });
                }
            }
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
        if parts.len() != 2 || parts[0] != "mesh" {
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
        let prev_health = world
            .current(&format!("mesh.{node}.health"))
            .ok()
            .flatten()
            .and_then(|f| f.value.get("status").and_then(|v| v.as_str()).and_then(MeshHealth::parse));
        let last_recovery_ms = world
            .current(&format!("mesh.{node}.recovery"))
            .ok()
            .flatten()
            .map(|f| f.valid_from);
        views.push(MeshNodeView { node, last_seen_ms, last_cmd_ok, prev_health, last_recovery_ms });
    }
    views
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
        }
    }

    fn view(node: &str, last_seen_ms: u64) -> MeshNodeView {
        MeshNodeView {
            node: node.to_string(),
            last_seen_ms,
            last_cmd_ok: None,
            prev_health: None,
            last_recovery_ms: None,
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
}

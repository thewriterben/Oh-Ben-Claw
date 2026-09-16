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

use crate::lora_gateway::{CommandSink, NodeCommand};
use crate::MeshSupervisorConfig;
use obc_memory::world::{Origin, WorldMemory};
use obc_safety::limits::SafetyLimit;
use serde_json::json;
use std::sync::Arc;

/// Source tag stamped on every fact the supervisor writes.
///
/// Not named `SOURCE`: this module also uses the gateway's, and the two are routinely
/// side by side — the gateway reports what came off the air, the supervisor reports
/// what it concluded from that. Naming both `SOURCE` is how a conclusion ends up
/// wearing a radio's label.
///
/// Now that source liveness can retract, this label is load-bearing rather than
/// decorative: it decides which facts stop being believed when the supervisor is
/// switched off.
pub const SUPERVISOR_SOURCE: &str = "mesh-supervisor";

/// Derived health of a mesh node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshHealth {
    /// Heard recently, last command (if any) succeeded.
    Online,
    /// Heard recently, but the last command result was not ok.
    Degraded,
    /// No mesh message within the staleness window.
    Offline,
    /// The host cannot hear the mesh at all — the base station's serial link
    /// is lost — so nothing can be said about this node. Not offline: a check
    /// that could not run must not fail like a check that did (DECISIONS
    /// 2026-09-12). On 2026-09-13 the brain lost its port and, deaf, presumed
    /// both nodes lost while they beaconed normally.
    Unobservable,
}

impl MeshHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            MeshHealth::Online => "online",
            MeshHealth::Degraded => "degraded",
            MeshHealth::Offline => "offline",
            MeshHealth::Unobservable => "unobservable",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "online" => Some(Self::Online),
            "degraded" => Some(Self::Degraded),
            "offline" => Some(Self::Offline),
            "unobservable" => Some(Self::Unobservable),
            _ => None,
        }
    }
}

/// Whether the host can hear the mesh right now — the gateway link, as an
/// input to [`decide`]. Read from the `spine.gateway` fact the gateway
/// supervisor writes; a body with no serial gateway (wired spine, tests) has
/// no such fact and is observable.
#[derive(Debug, Clone, PartialEq)]
pub enum SpineView {
    /// Messages can arrive. `since_ms` is when the link (re)opened: a node
    /// unheard since before an outage is offline from the reopen, not from
    /// its last beacon, so the outage never counts toward its escalation.
    Observable { since_ms: u64 },
    /// Nothing can arrive; every node is unobservable with this reason.
    Unobservable { reason: String },
}

impl SpineView {
    /// The view a body with no gateway link fact has.
    pub const ALWAYS: SpineView = SpineView::Observable { since_ms: 0 };

    /// From the gateway's own fact in world memory.
    pub fn from_world(world: &WorldMemory) -> SpineView {
        use crate::lora_gateway::GatewayLink;
        match GatewayLink::read(world) {
            None => SpineView::ALWAYS,
            Some(GatewayLink::Open { since_ms, .. }) => SpineView::Observable { since_ms },
            Some(link) => SpineView::Unobservable {
                reason: link.refusal(),
            },
        }
    }
}

/// A compact per-node snapshot the driver extracts from world memory for a decision.
#[derive(Debug, Clone)]
pub struct MeshNodeView {
    pub node: String,
    /// Row id of the `mesh.<node>` rollup fact this view was read from.
    ///
    /// The ids on this view exist so decisions can say what they were computed *from*.
    /// The supervisor concludes; the gateway observes; carrying the id is what lets a
    /// conclusion be withdrawn when the observation under it is.
    pub rollup_id: i64,
    /// Row id of the `cmd_result` fact behind [`Self::last_cmd_ok`], if any.
    pub cmd_result_id: Option<i64>,
    /// Row id of the current `mesh.<node>.health` fact, if any.
    pub health_id: Option<i64>,
    /// `valid_from` of the node's latest `mesh.<node>` rollup fact (ms).
    pub last_seen_ms: u64,
    /// Whether the node's latest `cmd_result` reflects a *healthy* node.
    ///
    /// Not simply the reply's `ok`: a Track 0 refusal arrives as `ok: false` but means
    /// the node enforced its own policy correctly, which is the node working. See
    /// [`cmd_result_healthy`].
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
    Health {
        node: String,
        status: &'static str,
        reason: String,
    },
    /// Issue a recovery command to an offline node.
    Recover { node: String, cmd: NodeCommand },
    /// Escalate: the node has been offline long enough to be presumed lost (recovery
    /// stops).
    Escalate { node: String, reason: String },
    /// Clear a prior escalation: the node came back.
    ClearEscalation { node: String },
}

/// Pure decision core: from per-node views + now + config + the state of the host's
/// own link to the mesh, produce the actions to apply. Health is emitted only when it
/// *changes* (no churn); recovery only for offline nodes when `recover` is configured
/// and the per-node rate limit has elapsed. While the spine is unobservable every
/// node is `unobservable` — no escalation, no probe, no offline clock — and an
/// existing escalation is neither cleared nor renewed: nothing is known.
pub fn decide(
    views: &[MeshNodeView],
    now_ms: u64,
    cfg: &MeshSupervisorConfig,
    spine: &SpineView,
) -> Vec<MeshDecision> {
    let mut out = Vec::new();
    let observable_since = match spine {
        SpineView::Observable { since_ms } => *since_ms,
        SpineView::Unobservable { reason } => {
            for v in views {
                if v.prev_health != Some(MeshHealth::Unobservable) {
                    out.push(MeshDecision::Health {
                        node: v.node.clone(),
                        status: MeshHealth::Unobservable.as_str(),
                        reason: reason.clone(),
                    });
                }
            }
            return out;
        }
    };
    for v in views {
        let age = now_ms.saturating_sub(v.last_seen_ms);
        let (status, reason) = if age > cfg.stale_ms {
            (MeshHealth::Offline, format!("no mesh message for {age} ms"))
        } else if v.last_cmd_ok == Some(false) {
            (
                MeshHealth::Degraded,
                "last command result was not ok".to_string(),
            )
        } else {
            (MeshHealth::Online, "healthy".to_string())
        };

        if v.prev_health != Some(status) {
            out.push(MeshDecision::Health {
                node: v.node.clone(),
                status: status.as_str(),
                reason,
            });
        }

        if status == MeshHealth::Offline {
            // Continuous-offline duration: if it was already offline, the health fact's
            // valid_from marks when it began; if it went offline this tick, that's ~now.
            // Never earlier than the link's own (re)open: the host has not been
            // listening for longer than that, so it cannot claim the node was.
            let offline_since = if v.prev_health == Some(MeshHealth::Offline) {
                v.health_since_ms.unwrap_or(now_ms)
            } else {
                now_ms
            }
            .max(observable_since);
            let offline_for = now_ms.saturating_sub(offline_since);
            let escalate_now =
                cfg.escalate_after_ms > 0 && !v.escalated && offline_for >= cfg.escalate_after_ms;

            if escalate_now {
                out.push(MeshDecision::Escalate {
                    node: v.node.clone(),
                    reason: format!("offline for {offline_for} ms — presumed lost"),
                });
            }

            // Recovery probing. Before escalation we ping at the fast cadence to try to
            // wake the node. After escalation we do NOT give up entirely: a node that
            // only answers a direct command — its passive beacons lost to RF, say —
            // still gets a *slow* "are you back?" probe, so the escalation self-heals
            // (a reply refreshes `last_seen` and the next tick clears it). The tick that
            // escalates suppresses a redundant same-tick ping.
            if !escalate_now {
                if let Some(cmd_name) = &cfg.recover {
                    let interval = if v.escalated {
                        cfg.escalated_probe_interval_ms
                    } else {
                        cfg.min_recovery_interval_ms
                    };
                    // `escalated_probe_interval_ms == 0` opts back out of the slow probe
                    // (an escalated node is then left silent, the old behaviour).
                    let probe_enabled = !v.escalated || cfg.escalated_probe_interval_ms > 0;
                    let due = v
                        .last_recovery_ms
                        .is_none_or(|t| now_ms.saturating_sub(t) >= interval);
                    if probe_enabled && due {
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
            out.push(MeshDecision::ClearEscalation {
                node: v.node.clone(),
            });
        }
    }
    out
}

/// Whether a node's `cmd_result` payload indicates a healthy node.
///
/// A Track 0 refusal ("safety: pin 99 not in allow-list") comes back as `ok: false`,
/// because from the command's point of view it did not succeed. But the node refusing an
/// out-of-policy write is the safety system working — arguably the single most important
/// thing the node does. Scoring it as a node fault meant every safety test marked the
/// node that passed it as `degraded`, and with the post-escalation slow probe a node
/// could be held in that wrong state indefinitely.
///
/// So refusals read as healthy here. The refusal itself is not lost — it stays in the
/// `mesh.<node>.cmd_result` fact for anyone asking what the node did, rather than what
/// state it is in.
pub fn cmd_result_healthy(value: &serde_json::Value) -> Option<bool> {
    if is_policy_refusal(value) {
        return Some(true);
    }
    value.get("ok").and_then(|v| v.as_bool())
}

/// Did the node refuse this command on policy, rather than fail it?
fn is_policy_refusal(value: &serde_json::Value) -> bool {
    // Firmware ≥ the `refused` flag says so outright.
    if value.get("refused").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    // Compatibility with nodes flashed before that flag existed: the on-MCU gate
    // prefixes every `SafetyViolation` with "safety:". Kept as a fallback so a
    // half-upgraded fleet doesn't mark its older nodes degraded.
    value
        .get("error")
        .and_then(|v| v.as_str())
        .is_some_and(|e| e.starts_with("safety:"))
}

/// Extract the current per-node views from world memory (rollup + last cmd_result +
/// prior health + last recovery). Mesh nodes are the `mesh.<node>` rollup entities
/// (node ids contain no dots, so a rollup splits into exactly two dot-parts).
///
/// Node discovery is **authoritative, not lexical**: a node exists because the LoRa
/// gateway heard it on the air, so the rollup fact must carry the gateway's source.
/// The shape of an entity name is not evidence of a radio.
///
/// The check is on [`Origin::Observed`] rather than on the source string. Both work
/// today — provenance is stamped by the framework, so an agent cannot claim to be
/// `lora-gateway` (see `tools::builtin::world::AGENT_SOURCE`) — but origin says what we
/// actually mean. "Something reported this off a wire" is the property that makes a node
/// real; "a component called `lora-gateway` wrote it" only implies that by convention.
///
/// It is also the stronger check. A source comparison cannot see when a *trusted*
/// component relays untrusted content — the confused-deputy case that `power` and
/// `sensing` exhibit — whereas origin travels with the content from wherever it entered.
///
/// This matters because `mesh.*` is a shared namespace. Anything else writing a
/// two-part fact under it — the supervisor's own `mesh.escalated_count` aggregate, or
/// (bench, 2026-07-17) a System 2 incident note at `mesh.escalation_status` — used to
/// be mis-parsed as a phantom node. A phantom never transmits, so it goes offline,
/// gets escalated, and pins `escalated_count >= 1` forever: the `safe-mesh-node-lost`
/// reflex then fires every tick, waking System 2, which records another note. The
/// agent manufactures its own emergency and reports it in a loop. Sourcing discovery
/// at the radio closes that loop at the root, rather than blacklisting names one at a
/// time as they appear.
///
/// **One phantom survives that rule, and it is the host's own station**
/// (DECISIONS.md 2026-09-16). `Origin::Observed` asks whether the entity was ever
/// heard on the air, and the station the brain is plugged into may well have been —
/// while it held a different role. Move the console cable to it and it becomes
/// permanently unhearable, because a station transmits its own frames rather than
/// receiving them, but the rollup from its former life stays `Observed` and keeps
/// qualifying. On the bench 2026-09-16 that was `gw-40`: heard as the field bridge,
/// promoted to base, then "offline for 43.5 hours — presumed lost" with
/// `escalated_count` pinned at 1 and `safe-mesh-node-lost` firing at Critical every
/// tick until the System 2 wake budget absorbed it. Exactly the loop the paragraph
/// above closes, re-entered through the one door it left open.
///
/// So discovery is authoritative *and* liveness must be. A board that cannot be
/// heard is not a node whose silence means anything, and its liveness already has a
/// correct signal of its own — `spine.gateway`, the console link. The id comes from
/// [`lora_gateway::OWN_STATION_FACT`], which the operator declares and the gateway
/// checks against the air.
pub fn snapshot(world: &WorldMemory) -> Vec<MeshNodeView> {
    let entities = world.entities().unwrap_or_default();
    let own = crate::lora_gateway::own_station(world);
    let mut views = Vec::new();
    for e in entities {
        let parts: Vec<&str> = e.split('.').collect();
        if parts.len() != 2 || parts[0] != "mesh" {
            continue;
        }
        let node = parts[1].to_string();
        // The console's own board. Not a node: unhearable by construction, so its
        // silence carries no information and must not be read as loss.
        if own.as_deref() == Some(node.as_str()) {
            continue;
        }
        // Heard over the air, or it is not a node.
        let (last_seen_ms, rollup_id) = match world.current(&e).ok().flatten() {
            Some(f) if f.origin == Origin::Observed => (f.valid_from, f.id),
            _ => continue,
        };
        let cmd_result_fact = world
            .current(&format!("mesh.{node}.cmd_result"))
            .ok()
            .flatten();
        let cmd_result_id = cmd_result_fact.as_ref().map(|f| f.id);
        let last_cmd_ok = cmd_result_fact
            .as_ref()
            .and_then(|f| cmd_result_healthy(&f.value));
        let health_fact = world.current(&format!("mesh.{node}.health")).ok().flatten();
        let health_id = health_fact.as_ref().map(|f| f.id);
        let prev_health = health_fact.as_ref().and_then(|f| {
            f.value
                .get("status")
                .and_then(|v| v.as_str())
                .and_then(MeshHealth::parse)
        });
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
            .and_then(|f| {
                f.value
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "escalated")
            })
            .unwrap_or(false);
        views.push(MeshNodeView {
            node,
            rollup_id,
            cmd_result_id,
            health_id,
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
    use obc_reflex::{Severity, DIGEST_PREFIX};
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
            let head = reason
                .split_once(". ")
                .map(|(h, _)| h)
                .unwrap_or(&reason)
                .to_string();
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

    let (mut online, mut degraded, mut offline, mut unobservable, mut escalated) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut nodes = Vec::with_capacity(views.len());
    for v in &views {
        let health = v.prev_health.map(|h| h.as_str()).unwrap_or("unknown");
        match health {
            "online" => online += 1,
            "degraded" => degraded += 1,
            "offline" => offline += 1,
            "unobservable" => unobservable += 1,
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

    // The host's own link, so a reader of the status sees "the brain cannot hear
    // the mesh" as a state of the brain, not as every node going quiet at once.
    let spine = match SpineView::from_world(world) {
        SpineView::Observable { .. } => json!({ "observable": true }),
        SpineView::Unobservable { reason } => json!({ "observable": false, "reason": reason }),
    };

    json!({
        "summary": {
            "nodes": views.len(),
            "online": online,
            "degraded": degraded,
            "offline": offline,
            "unobservable": unobservable,
            "escalated": escalated,
        },
        "spine": spine,
        // Stations whose frames the host is refusing (bad tag or replay): the
        // perceive step `safe-spine-forgery` sends System 2 to.
        "auth_alarms": crate::lora_gateway::LoraAuth::alarmed_stations(world),
        // Stations reporting that *they* are refusing frames on the air — the
        // perceive step `safe-spine-on-air` sends System 2 to. Kept as its own
        // field, next to but never merged with `auth_alarms`: one is what the
        // host proved, the other what a station said over an unauthenticated
        // console (DECISIONS.md 2026-09-15).
        "air_refusals": crate::lora_gateway::AirWatch::refusing_stations(world),
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

    // Retire conclusions drawn about the console's own station while it was still
    // being mistaken for a node. `snapshot` now skips it, so nothing would ever
    // revisit those facts: `escalated_count` recomputes from the views and drops,
    // but `mesh.<own>.escalation` would sit at "escalated" forever, and a standing
    // conclusion nobody will ever withdraw is worse than the count it no longer
    // feeds. Idempotent — after the first tick there is nothing left to clear.
    if let Some(own) = crate::lora_gateway::own_station(world) {
        let key = format!("mesh.{own}.escalation");
        let standing = world
            .current(&key)
            .ok()
            .flatten()
            .filter(|f| f.value.get("status").and_then(|s| s.as_str()) == Some("escalated"));
        if let Some(prev) = standing {
            tracing::info!(
                station = %own,
                "mesh supervisor: {own} is this console's own station, not a node — \
                 retiring the escalation it was given while it was being judged as one"
            );
            let _ = world.observe_derived_from(
                &key,
                json!({
                    "status": "cleared",
                    "reason": "not a mesh node: this is the host's own station, which \
                               cannot be heard on the air",
                    "ts_ms": now_ms,
                }),
                now_ms,
                now_ms,
                SUPERVISOR_SOURCE,
                &[prev.id],
            );
        }
    }

    let spine = SpineView::from_world(world);
    let decisions = decide(&views, now_ms, cfg, &spine);
    let mut applied = 0;

    // Every fact written below is a *conclusion*, and every one of them has inputs the
    // supervisor is holding at the moment it writes. Declaring them builds the chain
    //
    //     mesh.<node>            (lora-gateway, observed off the air)
    //       └─ mesh.<node>.health        (supervisor concluded)
    //            ├─ mesh.<node>.escalation
    //            │    └─ mesh.escalated_count
    //            └─ mesh.<node>.recovery
    //
    // which is what makes "the radio is gone, so none of this is grounded any more" a
    // walk rather than a judgement call. Without it, retiring the gateway closes the
    // rollups and leaves every conclusion drawn from them standing.
    let by_node: std::collections::HashMap<&str, &MeshNodeView> =
        views.iter().map(|v| (v.node.as_str(), v)).collect();
    // Health facts written during this tick supersede the ones the snapshot saw, so
    // decisions later in the same tick must point at the new row, not the stale one.
    let mut health_ids: std::collections::HashMap<String, i64> = views
        .iter()
        .filter_map(|v| v.health_id.map(|id| (v.node.clone(), id)))
        .collect();

    for d in decisions {
        match d {
            MeshDecision::Health {
                node,
                status,
                reason,
            } => {
                // Health rests on the radio evidence it was derived from: when the node
                // was last heard, and how it answered the last command.
                let mut support = Vec::new();
                if let Some(v) = by_node.get(node.as_str()) {
                    support.push(v.rollup_id);
                    support.extend(v.cmd_result_id);
                }
                if let Ok(f) = world.observe_derived_from(
                    &format!("mesh.{node}.health"),
                    json!({ "status": status, "reason": reason, "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    SUPERVISOR_SOURCE,
                    &support,
                ) {
                    health_ids.insert(node.clone(), f.id);
                }
                applied += 1;
            }
            MeshDecision::Recover { node, cmd } => {
                if let Some(s) = sink {
                    if s.send_command(&cmd).await.is_ok() {
                        let support: Vec<i64> =
                            health_ids.get(&node).copied().into_iter().collect();
                        let _ = world.observe_derived_from(
                            &format!("mesh.{node}.recovery"),
                            json!({ "cmd": cmd.cmd, "id": cmd.id, "ts_ms": now_ms }),
                            now_ms,
                            now_ms,
                            SUPERVISOR_SOURCE,
                            &support,
                        );
                        applied += 1;
                    }
                }
            }
            MeshDecision::Escalate { node, reason } => {
                tracing::warn!(node = %node, "mesh supervisor: node presumed lost — {reason}");
                // "Presumed lost" is a conclusion about a health state that has persisted.
                // If that health fact stops being believed, so must this.
                let support: Vec<i64> = health_ids.get(&node).copied().into_iter().collect();
                let _ = world.observe_derived_from(
                    &format!("mesh.{node}.escalation"),
                    json!({ "status": "escalated", "reason": reason, "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    SUPERVISOR_SOURCE,
                    &support,
                );
                applied += 1;
            }
            MeshDecision::ClearEscalation { node } => {
                tracing::info!(node = %node, "mesh supervisor: node returned — escalation cleared");
                // Clearing rests on the *fresh* rollup — the node transmitting again is
                // the evidence, not the health rollup computed from it.
                let support: Vec<i64> = by_node
                    .get(node.as_str())
                    .map(|v| vec![v.rollup_id])
                    .unwrap_or_default();
                let _ = world.observe_derived_from(
                    &format!("mesh.{node}.escalation"),
                    json!({ "status": "cleared", "ts_ms": now_ms }),
                    now_ms,
                    now_ms,
                    SUPERVISOR_SOURCE,
                    &support,
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
        // Record *what the count was computed from* — the JTMS in-list. This loop
        // already read every per-node escalation fact and used to throw the ids away,
        // which is precisely why a disabled supervisor could leave `escalated_count = 2`
        // standing with nothing underneath it while the reflex kept firing on it.
        //
        // Every escalation fact consulted goes in the list, not only the escalated ones:
        // a count of 1 depends on the nodes that were *not* escalated just as much as on
        // the one that was — change either and the number changes.
        //
        // When no node has an escalation fact yet the list is empty, which claims the
        // count is self-standing. It very nearly is: all that is under it then is the
        // supervisor having run, and that becomes explicit support once source-liveness
        // markers land.
        let mut support: Vec<i64> = Vec::new();
        let mut escalated_count: u64 = 0;
        for v in &views {
            if let Some(f) = world
                .current(&format!("mesh.{}.escalation", v.node))
                .ok()
                .flatten()
            {
                if f.value.get("status").and_then(|s| s.as_str()) == Some("escalated") {
                    escalated_count += 1;
                }
                support.push(f.id);
            }
        }
        let prev = world
            .current("mesh.escalated_count")
            .ok()
            .flatten()
            .and_then(|f| f.value.as_u64());
        if prev != Some(escalated_count) {
            let _ = world.observe_derived_from(
                "mesh.escalated_count",
                json!(escalated_count),
                now_ms,
                now_ms,
                SUPERVISOR_SOURCE,
                &support,
            );
        }
    }

    applied
}

// ── Limits hydration: a node that announces a boot gets its limits back ─────────
//
// Since 2026-08-22 the node boots deny-all and *says so*: a `policy_state` line
// with `reason: "boot"` and a fresh `boot_id`, and the same `boot_id` on every
// `set_limits` and `capabilities` reply, "so a host that remembers the boot_id it
// pushed against can detect the reset without polling for it". Until 2026-09-13
// no host code remembered anything: the announcement landed in world memory as
// `mesh.<node>.policy_state` and nothing read it. The gap that comment named —
// "a host that pushed [3,7] will happily go on believing [3,7] is in force while
// the node refuses everything" — was the live state of the bench that afternoon:
// a power cycle wiped the die-temperature rules' pin-21 limit, the brain's
// posture arrived and modulated a rule that could not act.
//
// This closes it in the direction the 08-22 decision chose: authority stays with
// the host, the node still boots deny-all, and the host re-pushes the limits it
// holds for that node (`[[safety.limits]]`) the moment it learns of a boot it has
// not pushed against. Rules are not re-pushed here — a rule set does not fit a
// mesh frame (`tests/spine_payload_budget.rs`), and that is a separate change.

/// What a node last said its boot was, and which boot the host last pushed
/// limits for.
#[derive(Debug, Clone, PartialEq)]
pub struct BootView {
    pub node: String,
    /// The node's current `boot_id`, from its `policy_state` announcement or the
    /// `boot_id` any of its replies carries — whichever the node said most recently.
    pub boot_id: Option<u64>,
    /// Row id of the fact `boot_id` came from — the evidence a push is derived from.
    pub evidence_id: Option<i64>,
    /// The `boot_id` the host last pushed limits for (`mesh.<node>.limits_pushed`).
    pub pushed_boot_id: Option<u64>,
    /// When that push was made (ms), and how many attempts it has taken so far.
    pub pushed_at_ms: Option<u64>,
    pub push_attempts: u64,
    /// Whether the last push was refused before sending (over budget) — not retried.
    pub push_refused: bool,
    /// Whether the node's latest beacon still says `policy: "deny-all"` for the
    /// current boot: the push has not landed (the mesh loses about a frame in
    /// three under chatter), so it is owed again.
    pub still_deny_all: bool,
}

/// How long after a push the host waits before pushing again to a node whose
/// beacon still says deny-all. The beacon is every 30 s, so a retry sooner than
/// that would answer stale evidence.
pub const LIMITS_RETRY_MS: u64 = 20_000;

/// The `boot_id` a node's reply carries, if any. Replies put the node's answer in
/// `result` as a JSON *string* (the firmware formats it by hand), so it is parsed
/// again here.
fn boot_id_in_reply(cmd_result: &serde_json::Value) -> Option<u64> {
    let result = cmd_result.get("result")?;
    let parsed;
    let obj = match result {
        serde_json::Value::String(s) => {
            parsed = serde_json::from_str::<serde_json::Value>(s).ok()?;
            &parsed
        }
        other => other,
    };
    obj.get("boot_id").and_then(|b| b.as_u64())
}

/// Read every mesh node's boot evidence from world memory. Node discovery is the
/// same as [`snapshot`]'s: a node is a `mesh.<node>` rollup the gateway observed.
pub fn boot_snapshot(world: &WorldMemory) -> Vec<BootView> {
    let mut views = Vec::new();
    for e in world.entities().unwrap_or_default() {
        let parts: Vec<&str> = e.split('.').collect();
        if parts.len() != 2 || parts[0] != "mesh" {
            continue;
        }
        let node = parts[1].to_string();
        match world.current(&e).ok().flatten() {
            Some(f) if f.origin == Origin::Observed => {}
            _ => continue,
        }
        // The three places a boot id shows up; take the one the node said last.
        // The announcement is one frame and can be lost to the air (it was, on
        // the bench, twice in a row); the beacon repeats it every 30 s, and any
        // reply carries it.
        let announced = world
            .current(&format!("mesh.{node}.policy_state"))
            .ok()
            .flatten()
            .filter(|f| f.origin == Origin::Observed)
            .and_then(|f| {
                f.value
                    .get("boot_id")
                    .and_then(|b| b.as_u64())
                    .map(|b| (f.valid_from, b, f.id))
            });
        let beaconed = world
            .current(&format!("mesh.{node}.beacon"))
            .ok()
            .flatten()
            .filter(|f| f.origin == Origin::Observed)
            .and_then(|f| {
                f.value
                    .get("boot_id")
                    .and_then(|b| b.as_u64())
                    .map(|b| (f.valid_from, b, f.id))
            });
        let replied = world
            .current(&format!("mesh.{node}.cmd_result"))
            .ok()
            .flatten()
            .filter(|f| f.origin == Origin::Observed)
            .and_then(|f| boot_id_in_reply(&f.value).map(|b| (f.valid_from, b, f.id)));
        let latest = [announced, beaconed, replied]
            .into_iter()
            .flatten()
            .max_by_key(|(at, _, id)| (*at, *id));
        let pushed = world
            .current(&format!("mesh.{node}.limits_pushed"))
            .ok()
            .flatten();
        let pushed_boot_id = pushed
            .as_ref()
            .and_then(|f| f.value.get("boot_id").and_then(|b| b.as_u64()));
        let pushed_at_ms = pushed.as_ref().map(|f| f.valid_from);
        let push_attempts = pushed
            .as_ref()
            .and_then(|f| f.value.get("attempts").and_then(|a| a.as_u64()))
            .unwrap_or(0);
        let push_refused = pushed
            .as_ref()
            .is_some_and(|f| f.value.get("error").is_some());
        // The beacon is the node's standing word on its policy. It says deny-all
        // for the boot it names until a push lands; the moment one does, the
        // field disappears from the next beacon.
        let still_deny_all = world
            .current(&format!("mesh.{node}.beacon"))
            .ok()
            .flatten()
            .filter(|f| f.origin == Origin::Observed)
            .is_some_and(|f| {
                f.value.get("policy").and_then(|p| p.as_str()) == Some("deny-all")
                    && f.value.get("boot_id").and_then(|b| b.as_u64()) == latest.map(|l| l.1)
            });
        views.push(BootView {
            node,
            boot_id: latest.map(|l| l.1),
            evidence_id: latest.map(|l| l.2),
            pushed_boot_id,
            pushed_at_ms,
            push_attempts,
            push_refused,
            still_deny_all,
        });
    }
    views
}

/// A limits push the host owes a node.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitsPush {
    pub node: String,
    pub boot_id: u64,
    pub evidence_id: Option<i64>,
    /// 1 for a boot's first push; counts up while the beacon keeps saying deny-all.
    pub attempt: u64,
    pub cmd: NodeCommand,
}

/// Pure decision: which nodes are owed a limits push. A node is owed one when
/// it has named a boot the host has not pushed against, or when it has and the
/// node's beacon still says deny-all for that boot [`LIMITS_RETRY_MS`] after the
/// push — the mesh loses frames, and a push that never landed is owed again.
/// A node with no configured limits is left deny-all: that is the
/// configuration, not an omission. A push refused for size is not retried; a
/// frame too long today is too long tomorrow.
///
/// The command id is `lim` + the boot id in hex, so the reply says which boot
/// it answered for; a retry carries `r{n}`, like `mesh_command`'s.
pub fn limits_to_push(views: &[BootView], limits: &[SafetyLimit], now_ms: u64) -> Vec<LimitsPush> {
    let mut out = Vec::new();
    for v in views {
        let Some(boot_id) = v.boot_id else { continue };
        let attempt = if v.pushed_boot_id == Some(boot_id) {
            let retry_due = v.still_deny_all
                && !v.push_refused
                && v.pushed_at_ms
                    .is_some_and(|t| now_ms.saturating_sub(t) >= LIMITS_RETRY_MS);
            if !retry_due {
                continue;
            }
            v.push_attempts + 1
        } else {
            1
        };
        let mine: Vec<&SafetyLimit> = limits.iter().filter(|l| l.node_id == v.node).collect();
        if mine.is_empty() {
            continue;
        }
        let id = if attempt > 1 {
            format!("lim{boot_id:08x}r{}", attempt - 1)
        } else {
            format!("lim{boot_id:08x}")
        };
        let cmd = NodeCommand::new(&v.node, id, "set_limits", json!({ "limits": mine }));
        out.push(LimitsPush {
            node: v.node.clone(),
            boot_id,
            evidence_id: v.evidence_id,
            attempt,
            cmd,
        });
    }
    out
}

/// One hydration pass: read the boot evidence, push limits where owed, record what
/// was pushed as `mesh.<node>.limits_pushed { boot_id, id, pins, ts_ms }` derived
/// from the boot evidence. A push the mesh cannot carry is recorded with an
/// `error` and the same `boot_id`, so it is visible and not retried every tick —
/// a frame that is too long today is too long tomorrow. A push the sink fails to
/// send is not recorded, so the next tick tries again. Returns the number of
/// pushes sent.
pub async fn hydrate_limits(
    world: &WorldMemory,
    sink: Option<&Arc<dyn CommandSink>>,
    limits: &[SafetyLimit],
    now_ms: u64,
) -> usize {
    let Some(sink) = sink else { return 0 };
    // Nothing can be pushed through a lost link; the boot evidence keeps, and the
    // first tick after the reopen pushes it.
    if let SpineView::Unobservable { .. } = SpineView::from_world(world) {
        return 0;
    }
    let views = boot_snapshot(world);
    let mut sent = 0;
    for push in limits_to_push(&views, limits, now_ms) {
        let support: Vec<i64> = push.evidence_id.into_iter().collect();
        let pins: Vec<serde_json::Value> = limits
            .iter()
            .filter(|l| l.node_id == push.node)
            .map(|l| json!({ "tool": l.tool, "allowed_pins": l.allowed_pins }))
            .collect();
        if !push.cmd.fits_one_frame() {
            tracing::warn!(
                node = %push.node,
                bytes = push.cmd.encoded_len(),
                budget = crate::lora_gateway::MESH_LINE_BUDGET,
                "mesh supervisor: this node's limits do not fit one mesh frame; it stays deny-all"
            );
            let _ = world.observe_derived_from(
                &format!("mesh.{}.limits_pushed", push.node),
                json!({
                    "boot_id": push.boot_id,
                    "id": push.cmd.id,
                    "attempts": push.attempt,
                    "error": format!(
                        "set_limits is {} bytes; the mesh carries {}",
                        push.cmd.encoded_len(),
                        crate::lora_gateway::MESH_LINE_BUDGET
                    ),
                    "limits": pins,
                    "ts_ms": now_ms,
                }),
                now_ms,
                now_ms,
                SUPERVISOR_SOURCE,
                &support,
            );
            continue;
        }
        match sink.send_command(&push.cmd).await {
            Ok(()) => {
                tracing::info!(
                    node = %push.node,
                    boot_id = push.boot_id,
                    id = %push.cmd.id,
                    attempt = push.attempt,
                    "mesh supervisor: node is deny-all for a boot the host holds limits for — limits pushed"
                );
                let _ = world.observe_derived_from(
                    &format!("mesh.{}.limits_pushed", push.node),
                    json!({
                        "boot_id": push.boot_id,
                        "id": push.cmd.id,
                        "attempts": push.attempt,
                        "limits": pins,
                        "ts_ms": now_ms,
                    }),
                    now_ms,
                    now_ms,
                    SUPERVISOR_SOURCE,
                    &support,
                );
                sent += 1;
            }
            Err(e) => {
                tracing::warn!(node = %push.node, error = %e, "mesh supervisor: limits push not sent; will retry");
            }
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora_gateway::SOURCE;
    use std::sync::Mutex;

    /// Every test written before the spine had a state ran with the host able to
    /// hear the mesh; this keeps them saying so explicitly.
    fn decide(
        views: &[MeshNodeView],
        now_ms: u64,
        cfg: &MeshSupervisorConfig,
    ) -> Vec<MeshDecision> {
        super::decide(views, now_ms, cfg, &SpineView::ALWAYS)
    }

    fn cfg(recover: Option<&str>) -> MeshSupervisorConfig {
        MeshSupervisorConfig {
            enabled: true,
            stale_ms: 5_000,
            tick_ms: 5_000,
            recover: recover.map(str::to_string),
            min_recovery_interval_ms: 30_000,
            escalate_after_ms: 0,
            escalated_probe_interval_ms: 300_000,
        }
    }

    fn view(node: &str, last_seen_ms: u64) -> MeshNodeView {
        MeshNodeView {
            node: node.to_string(),
            // `decide` is pure and never reads these; they exist so the driver can
            // record what it derived a conclusion from. Zero is a deliberate tell: any
            // test that starts caring about support must build a real store.
            rollup_id: 0,
            cmd_result_id: None,
            health_id: None,
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
        assert!(matches!(
            &d[0],
            MeshDecision::Health {
                status: "online",
                ..
            }
        ));
    }

    #[test]
    fn a_stale_node_goes_offline_and_is_recovered() {
        // last seen at 1_000, now 11_000, stale_ms 5_000 → offline.
        let d = decide(&[view("n", 1_000)], 11_000, &cfg(Some("capabilities")));
        assert!(d.iter().any(|x| matches!(
            x,
            MeshDecision::Health {
                status: "offline",
                ..
            }
        )));
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
        assert!(
            !d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })),
            "within the cooldown"
        );
    }

    #[test]
    fn observe_only_when_no_recover_command_is_set() {
        let d = decide(&[view("n", 1_000)], 11_000, &cfg(None));
        assert!(
            d.iter().all(|x| matches!(x, MeshDecision::Health { .. })),
            "no recovery without a command"
        );
    }

    #[test]
    fn a_failed_command_marks_the_node_degraded() {
        let mut v = view("n", 10_500); // fresh
        v.last_cmd_ok = Some(false);
        let d = decide(&[v], 11_000, &cfg(None));
        assert!(matches!(
            &d[0],
            MeshDecision::Health {
                status: "degraded",
                ..
            }
        ));
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
            .observe_as(
                "mesh.node-x",
                json!({ "last_type": "link_state", "rssi_dbm": -50, "seq": 1, "src": "2A" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();

        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
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
        assert!(
            d.iter().any(|x| matches!(x, MeshDecision::Escalate { .. })),
            "escalates"
        );
        assert!(
            !d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })),
            "gives up pinging"
        );
    }

    #[test]
    fn an_escalated_node_is_not_re_escalated() {
        // Escalated + still offline + last probe recent → no re-escalation, no health
        // churn, and not yet due for the slow probe.
        let mut v = offline_view("n", 1_000);
        v.escalated = true;
        v.last_recovery_ms = Some(29_500); // probed 500 ms ago; slow interval is 300 s
        let d = decide(&[v], 30_000, &esc_cfg());
        assert!(
            d.is_empty(),
            "no re-escalation, no health churn, within the slow cooldown"
        );
    }

    #[test]
    fn an_escalated_node_is_slowly_probed_not_abandoned() {
        // The self-healing probe: an escalated node whose beacons are lost to RF but
        // which still answers a direct command must keep getting a slow "are you back?"
        // ping, so it can recover on its own.
        let mut v = offline_view("n", 1_000);
        v.escalated = true;
        v.last_recovery_ms = Some(1_000); // last probed at t=1_000
                                          // Not yet due at +299 s (slow interval is 300 s).
        let early = decide(&[v.clone()], 300_000, &esc_cfg());
        assert!(
            !early
                .iter()
                .any(|x| matches!(x, MeshDecision::Recover { .. })),
            "within the slow cooldown"
        );
        // Due at +300 s.
        let due = decide(&[v], 301_500, &esc_cfg());
        assert!(
            due.iter()
                .any(|x| matches!(x, MeshDecision::Recover { .. })),
            "slow probe fires when due"
        );
        assert!(
            !due.iter()
                .any(|x| matches!(x, MeshDecision::Escalate { .. })),
            "but never re-escalates"
        );
    }

    #[test]
    fn setting_the_escalated_probe_to_zero_restores_give_up_on_escalation() {
        // Opt-out: `escalated_probe_interval_ms == 0` leaves an escalated node silent.
        let mut c = esc_cfg();
        c.escalated_probe_interval_ms = 0;
        let mut v = offline_view("n", 1_000);
        v.escalated = true;
        let d = decide(&[v], 1_000_000, &c);
        assert!(
            d.is_empty(),
            "no probe, no churn — the old give-up behaviour"
        );
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
        assert!(d
            .iter()
            .any(|x| matches!(x, MeshDecision::ClearEscalation { .. })));
    }

    // ── A lost spine (2026-09-13) ────────────────────────────────────────────

    fn lost() -> SpineView {
        SpineView::Unobservable {
            reason: "gateway lost: os error 22".into(),
        }
    }

    #[test]
    fn with_the_spine_lost_a_node_past_the_threshold_is_unobservable_not_escalated() {
        // The 2026-09-13 outage: the same view that escalates today…
        let v = offline_view("n", 1_000);
        let today = super::decide(
            std::slice::from_ref(&v),
            30_000,
            &esc_cfg(),
            &SpineView::ALWAYS,
        );
        assert!(today
            .iter()
            .any(|x| matches!(x, MeshDecision::Escalate { .. })));
        // …yields one health change and nothing else while the host is deaf.
        let d = super::decide(&[v], 30_000, &esc_cfg(), &lost());
        assert_eq!(
            d,
            vec![MeshDecision::Health {
                node: "n".into(),
                status: "unobservable",
                reason: "gateway lost: os error 22".into(),
            }]
        );
    }

    #[test]
    fn an_unobservable_node_is_not_re_reported_probed_or_cleared_while_the_spine_is_down() {
        let mut v = offline_view("n", 1_000);
        v.prev_health = Some(MeshHealth::Unobservable);
        v.escalated = true; // escalated before the loss: stays that way, unknown
        v.last_recovery_ms = None; // a probe would be due — there is nothing to send it on
        let d = super::decide(&[v], 1_000_000, &esc_cfg(), &lost());
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn the_offline_clock_restarts_at_the_reopen_not_at_the_last_beacon() {
        // Unheard since 1_000; the link reopened at 300_000 after a five-minute
        // outage; now is 305_000. Offline for 5 s from the reopen — not 304 s —
        // so no escalation at a 20 s threshold, and the recovery probe runs.
        let mut v = offline_view("n", 1_000);
        v.prev_health = Some(MeshHealth::Unobservable);
        let d = super::decide(
            &[v],
            305_000,
            &esc_cfg(),
            &SpineView::Observable { since_ms: 300_000 },
        );
        assert!(
            d.iter().any(|x| matches!(
                x,
                MeshDecision::Health {
                    status: "offline",
                    ..
                }
            )),
            "{d:?}"
        );
        assert!(!d.iter().any(|x| matches!(x, MeshDecision::Escalate { .. })));
        assert!(d.iter().any(|x| matches!(x, MeshDecision::Recover { .. })));
        // Even a view that was already `offline` before the reopen counts from the reopen.
        let d = super::decide(
            &[offline_view("n", 1_000)],
            305_000,
            &esc_cfg(),
            &SpineView::Observable { since_ms: 300_000 },
        );
        assert!(!d.iter().any(|x| matches!(x, MeshDecision::Escalate { .. })));
    }

    #[tokio::test]
    async fn tick_reads_the_spine_from_the_gateway_fact_and_writes_unobservable_once() {
        use crate::lora_gateway::GatewayLink;
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "beacon" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        let c = esc_cfg();
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();

        GatewayLink::Lost {
            since_ms: 2_000,
            error: "os error 22".into(),
        }
        .record(&world, "COM3", 2_000);
        tick(&world, Some(&sink), &c, 30_000).await;
        tick(&world, Some(&sink), &c, 35_000).await;
        let health = world.history("mesh.n.health").unwrap();
        assert_eq!(health.len(), 1, "one fact per outage, not one per tick");
        assert_eq!(health[0].value["status"], json!("unobservable"));
        assert!(health[0].value["reason"]
            .as_str()
            .unwrap()
            .starts_with("gateway lost"));
        assert!(
            world.current("mesh.n.escalation").unwrap().is_none(),
            "a deaf host presumes nothing"
        );
        assert_eq!(mock.sent.lock().unwrap().len(), 0);

        // Reopened: the node reads offline from the reopen, then online on its beacon.
        GatewayLink::Open {
            since_ms: 40_000,
            attempts: 3,
        }
        .record(&world, "COM3", 40_000);
        tick(&world, Some(&sink), &c, 41_000).await;
        let h = world.current("mesh.n.health").unwrap().unwrap();
        assert_eq!(h.value["status"], json!("offline"));
        assert!(world.current("mesh.n.escalation").unwrap().is_none());
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "beacon" }),
                42_000,
                42_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        tick(&world, Some(&sink), &c, 43_000).await;
        assert_eq!(
            world.current("mesh.n.health").unwrap().unwrap().value["status"],
            json!("online")
        );
        let status = status_json(&world);
        assert_eq!(status["spine"]["observable"], json!(true));
        assert_eq!(status["summary"]["unobservable"], json!(0));
    }

    #[tokio::test]
    async fn tick_escalates_a_long_offline_node_then_clears_on_return() {
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "link_state" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.n.health",
                json!({ "status": "offline", "reason": "x" }),
                1_000,
                1_000,
                "test",
            )
            .unwrap();
        let c = esc_cfg();
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();

        // Offline for 29 s (>= 20 s threshold) → escalate, no recovery ping.
        tick(&world, Some(&sink), &c, 30_000).await;
        assert_eq!(
            world.current("mesh.n.escalation").unwrap().unwrap().value["status"],
            json!("escalated")
        );
        assert_eq!(mock.sent.lock().unwrap().len(), 0, "escalated → no ping");

        // Node returns (fresh rollup) → escalation cleared.
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "link_state" }),
                30_500,
                30_500,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        tick(&world, Some(&sink), &c, 31_000).await;
        assert_eq!(
            world.current("mesh.n.escalation").unwrap().unwrap().value["status"],
            json!("cleared")
        );
    }

    // `escalation_raises_the_count_that_drives_a_reflex` moved to
    // tests/mesh_escalation_drives_safing.rs on 2026-08-13. Its `use` of the
    // agent's safing rules was this 4781-line module's only reference to the
    // agent at all, and it sat in five of the nine dependency cycles the
    // endgame script reported -- one line, inside one test, holding the return
    // arrow that made every path through this module circular.
    //
    // The path is not spelled here on purpose: core_endgame.py counts textual
    // crossings and cannot tell a comment from a call.

    #[tokio::test]
    async fn the_count_records_the_escalation_facts_it_was_computed_from() {
        // The July bench bug in miniature. `mesh.escalated_count = 2` outlived the
        // supervisor that computed it because nothing recorded what it rested on. Now
        // the count carries its in-list, so "what did I believe because of this node?"
        // is a walk rather than an archaeology exercise.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "link_state" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.n.health",
                json!({ "status": "offline" }),
                1_000,
                1_000,
                "test",
            )
            .unwrap();
        let mut c = esc_cfg();
        c.recover = None;

        tick(&world, None, &c, 30_000).await;

        let esc = world.current("mesh.n.escalation").unwrap().unwrap();
        let count = world.current("mesh.escalated_count").unwrap().unwrap();
        assert_eq!(count.value.as_u64(), Some(1));
        assert_eq!(
            count.derived_from,
            Some(vec![vec![esc.id]]),
            "the count names the escalation fact under it"
        );
        // And the edge is walkable from the other end — this is the query that was
        // impossible before: what rests on this node's escalation?
        let deps = world.dependents(esc.id).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].entity, "mesh.escalated_count");
    }

    #[tokio::test]
    async fn losing_the_radio_unwinds_everything_the_supervisor_concluded() {
        // End-to-end, through the real tick rather than a hand-built chain: the gateway
        // observes a node, the supervisor concludes health → escalation → count on top
        // of it, and then the radio goes away. The supervisor is still running and still
        // willing; it simply has nothing left to have concluded from.
        use crate::lora_gateway::SOURCE as GATEWAY_SOURCE;
        use obc_memory::liveness::{stopped, Stopped};

        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "link_state" }),
                1_000,
                1_000,
                GATEWAY_SOURCE,
                Origin::Observed,
            )
            .unwrap();
        let mut c = esc_cfg();
        c.recover = None;

        tick(&world, None, &c, 30_000).await; // health: offline
        tick(&world, None, &c, 160_000).await; // escalate + count

        assert_eq!(
            world
                .current("mesh.escalated_count")
                .unwrap()
                .unwrap()
                .value
                .as_u64(),
            Some(1)
        );
        // Every link in the chain declares its support — this is what makes the sweep
        // below a walk rather than a guess.
        for e in ["mesh.n.health", "mesh.n.escalation", "mesh.escalated_count"] {
            assert!(
                world.current(e).unwrap().unwrap().derived_from.is_some(),
                "{e} did not record what it was derived from"
            );
        }

        let sweep = stopped(
            &world,
            GATEWAY_SOURCE,
            Stopped::Retired,
            200_000,
            "no COM port",
        )
        .unwrap();

        assert_eq!(
            sweep.closed.len(),
            1,
            "only the radio's own fact was its own"
        );
        assert_eq!(sweep.unsupported.len(), 3, "health, escalation, count");
        for e in ["mesh.n.health", "mesh.n.escalation", "mesh.escalated_count"] {
            assert!(
                world.current(e).unwrap().is_none(),
                "{e} outlived the radio"
            );
        }

        // Undercut, not rebutted: the count is not now zero, it is not held at all.
        assert_eq!(
            world
                .at("mesh.escalated_count", 170_000)
                .unwrap()
                .unwrap()
                .value
                .as_u64(),
            Some(1)
        );
    }

    /// The bench case of 2026-09-16. `gw-40` was heard on the air as the field
    /// bridge, so its rollup is legitimately `Origin::Observed` and passes the
    /// phantom guard above. Then the console cable moved to it and it became
    /// unhearable — a station transmits its own frames rather than receiving
    /// them — so it "went offline", got escalated, and pinned `escalated_count`
    /// at 1 with `safe-mesh-node-lost` firing at Critical every tick.
    #[test]
    fn the_consoles_own_station_is_not_a_node_even_though_it_was_heard_once() {
        let world = WorldMemory::open_in_memory().unwrap();
        for station in ["gw-40", "gw-D8"] {
            world
                .observe_as(
                    &format!("mesh.{station}"),
                    json!({ "last_type": "gw_keepalive", "rssi_dbm": -58, "src": station }),
                    1_000,
                    1_000,
                    "lora-gateway",
                    Origin::Observed,
                )
                .unwrap();
        }
        // Without the declaration both qualify — which is the bug, not the fix.
        assert_eq!(snapshot(&world).len(), 2);

        crate::lora_gateway::record_own_station(&world, "gw-40", "COM3", 2_000);
        let views = snapshot(&world);
        assert_eq!(views.len(), 1, "the console's own board is not a node");
        assert_eq!(views[0].node, "gw-D8", "the other station still is one");
    }

    /// Retiring the conclusion, not merely dropping it from the count. A standing
    /// "escalated" that nothing will ever revisit is worse than the count it no
    /// longer feeds: the next person reads world memory, not the views.
    #[tokio::test]
    async fn an_escalation_left_on_the_own_station_is_withdrawn_not_abandoned() {
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.gw-40",
                json!({ "last_type": "gw_keepalive", "rssi_dbm": -58, "src": "40" }),
                1_000,
                1_000,
                "lora-gateway",
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.gw-40.escalation",
                json!({ "status": "escalated", "reason": "offline for 120005 ms", "ts_ms": 1_000 }),
                1_000,
                1_000,
                SUPERVISOR_SOURCE,
                Origin::Derived,
            )
            .unwrap();
        crate::lora_gateway::record_own_station(&world, "gw-40", "COM3", 2_000);

        let cfg = MeshSupervisorConfig::default();
        tick(&world, None, &cfg, 3_000).await;

        let esc = world.current("mesh.gw-40.escalation").unwrap().unwrap();
        assert_eq!(esc.value["status"], json!("cleared"));
        assert!(
            esc.value["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("host's own station"),
            "the withdrawal says why, so it does not read as the node coming back"
        );
        // Idempotent: a second tick has nothing left to clear.
        let before = world.history("mesh.gw-40.escalation").unwrap().len();
        tick(&world, None, &cfg, 4_000).await;
        assert_eq!(
            world.history("mesh.gw-40.escalation").unwrap().len(),
            before,
            "clearing runs once, not every tick"
        );
    }

    #[tokio::test]
    async fn an_agent_note_under_mesh_never_becomes_a_node() {
        // Bench regression, 2026-07-17. The `safe-mesh-node-lost` playbook tells System 2
        // to record the loss; it filed the note at `mesh.escalation_status`, which
        // lexical discovery read back as a node. The phantom can't transmit → offline →
        // escalated → `escalated_count >= 1` forever → the reflex fires every tick →
        // wakes System 2 → another note. The agent invents an emergency and reports it
        // in a loop. Discovery is sourced at the radio, so the note stays a note.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.escalation_status",
                json!({ "nodes_escalated": ["n1"], "action_required": "operator_intervention" }),
                1_000,
                1_000,
                "agent",
                Origin::Asserted, // what an LLM write actually is
            )
            .unwrap();

        let views = snapshot(&world);
        assert_eq!(views.len(), 1, "the agent's own note is not a radio");
        assert_eq!(views[0].node, "n1");

        // And it is never escalated, so it cannot pin the reflex on.
        let mut c = esc_cfg();
        c.recover = None;
        tick(&world, None, &c, 200_000).await;
        assert!(
            world
                .current("mesh.escalation_status.escalation")
                .unwrap()
                .is_none(),
            "a note is never presumed lost"
        );
        let v = status_json(&world);
        assert_eq!(
            v["summary"]["nodes"],
            json!(1),
            "no phantom in the fleet view"
        );
    }

    #[test]
    fn a_track_0_refusal_is_not_a_node_fault() {
        // The reply that came home over LoRa on 2026-07-17, verbatim in shape. `ok` is
        // false because the write did not happen — but the node refusing an out-of-policy
        // pin is the safety system working, not a malfunction.
        let refused = json!({
            "type": "cmd_result",
            "node_id": "obc-esp32-s3-001",
            "ok": false,
            "refused": true,
            "error": "safety: pin 99 not in allow-list",
        });
        assert_eq!(
            cmd_result_healthy(&refused),
            Some(true),
            "a refusal is the node working"
        );

        // A genuine failure still reads as a fault.
        let failed = json!({
            "type": "cmd_result", "ok": false,
            "error": "gpio_set_level failed for pin 2 with error 258",
        });
        assert_eq!(cmd_result_healthy(&failed), Some(false));

        // Success is success; a reply with no `ok` tells us nothing.
        assert_eq!(cmd_result_healthy(&json!({"ok": true})), Some(true));
        assert_eq!(cmd_result_healthy(&json!({"result": "x"})), None);
    }

    #[test]
    fn a_refusal_from_firmware_without_the_flag_is_still_not_a_fault() {
        // Nodes flashed before the `refused` flag only signal a refusal by the gate's
        // "safety:" prefix. A half-upgraded fleet must not mark its older nodes degraded.
        let old = json!({
            "type": "cmd_result", "ok": false,
            "error": "safety: value 7 out of range (min=Some(0), max=Some(1))",
        });
        assert_eq!(cmd_result_healthy(&old), Some(true));
        let old_rate = json!({
            "type": "cmd_result", "ok": false,
            "error": "safety: rate limit (12ms since last, min 500ms)",
        });
        assert_eq!(cmd_result_healthy(&old_rate), Some(true));
    }

    #[test]
    fn a_refused_command_does_not_degrade_the_node_end_to_end() {
        // The bug in full: safety-testing a node used to mark it degraded.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n",
                json!({ "last_type": "cmd_result" }),
                10_000,
                10_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.n.cmd_result",
                json!({ "ok": false, "refused": true, "error": "safety: pin 99 not in allow-list" }),
                10_000,
                10_000,
                SOURCE,
            )
            .unwrap();

        let views = snapshot(&world);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].last_cmd_ok, Some(true));

        // Fresh + refusal → online, not degraded.
        let d = decide(&views, 11_000, &cfg(None));
        assert!(
            matches!(
                &d[0],
                MeshDecision::Health {
                    status: "online",
                    ..
                }
            ),
            "the node that enforced its limits is healthy, got {:?}",
            d[0]
        );
    }

    #[test]
    fn a_relayed_fact_under_the_gateways_own_source_is_still_not_a_node() {
        // What switching from source to origin actually buys. A source comparison sees
        // only "who wrote this", so a trusted component relaying agent-supplied content
        // passes it — the confused-deputy case `power` and `sensing` exhibit. Origin
        // travels with the content from wherever it entered, so the relay is visible
        // even when the writer is the gateway itself.
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.real",
                json!({ "last_type": "beacon" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.relayed",
                json!({ "last_type": "beacon" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Asserted,
            )
            .unwrap();

        let nodes: Vec<String> = snapshot(&world).into_iter().map(|v| v.node).collect();
        assert_eq!(
            nodes,
            vec!["real"],
            "only what was actually heard is a node"
        );
    }

    #[test]
    fn snapshot_ignores_the_escalated_count_aggregate() {
        // Regression: `mesh.escalated_count` is a bare counter with two dot-parts, so it
        // must NOT be mistaken for a `mesh.<node>` rollup (which would appear as a
        // phantom "online" node from the tick after it is first written).
        let world = WorldMemory::open_in_memory().unwrap();
        world
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.escalated_count",
                json!(0),
                1_000,
                1_000,
                SUPERVISOR_SOURCE,
            )
            .unwrap();

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
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex", "rssi_dbm": -72 }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.n1.health",
                json!({ "status": "online" }),
                1_000,
                1_000,
                "t",
            )
            .unwrap();
        // A second node that is offline and escalated.
        world
            .observe_as(
                "mesh.n2",
                json!({ "last_type": "-" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe(
                "mesh.n2.health",
                json!({ "status": "offline" }),
                1_000,
                1_000,
                "t",
            )
            .unwrap();
        world
            .observe(
                "mesh.n2.escalation",
                json!({ "status": "escalated" }),
                1_000,
                1_000,
                "t",
            )
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
        world
            .observe_as(
                "mesh.n1",
                json!({ "rssi_dbm": -60 }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.n1",
                json!({ "rssi_dbm": -70 }),
                2_000,
                2_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex" }),
                3_000,
                3_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        world
            .observe_as(
                "mesh.n1",
                json!({ "rssi_dbm": -80 }),
                4_000,
                4_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();

        let full = rssi_series(&world, "n1", 24);
        assert_eq!(
            full,
            vec![-60, -70, -80],
            "oldest→newest, rssi-less fact skipped"
        );
        // Limit keeps the newest N.
        let last2 = rssi_series(&world, "n1", 2);
        assert_eq!(last2, vec![-70, -80]);

        // Surfaced per-node in status_json.
        let v = status_json(&world);
        let n1 = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["node"] == json!("n1"))
            .unwrap();
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
                1_000,
                1_000,
                "notify",
            )
            .unwrap();
        world
            .observe(
                "notifications.escalation",
                json!({ "reason": "node n2 presumed lost. escalate" }),
                2_000,
                2_000,
                "notify",
            )
            .unwrap();
        world
            .observe(
                "notifications.escalation",
                json!({ "reason": format!("{} 3 events", obc_reflex::DIGEST_PREFIX) }),
                3_000,
                3_000,
                "notify",
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
        world
            .observe_as(
                "mesh.n1",
                json!({ "last_type": "reflex" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        let v = status_json(&world);
        assert_eq!(v["escalations"].as_array().unwrap().len(), 2);
    }

    // ── Limits hydration ────────────────────────────────────────────────────

    const NODE: &str = "obc-esp32-s3-001";

    fn heard(world: &WorldMemory, t: u64) {
        world
            .observe_as(
                &format!("mesh.{NODE}"),
                json!({ "last_type": "beacon", "rssi_dbm": -55, "seq": 1, "src": "40" }),
                t,
                t,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
    }

    /// The node's own boot announcement, as the gateway lands it.
    fn announced_boot(world: &WorldMemory, boot_id: u64, t: u64) {
        world
            .observe_as(
                &format!("mesh.{NODE}.policy_state"),
                json!({
                    "type": "policy_state", "node_id": NODE, "boot_id": boot_id,
                    "policy": "deny-all", "reason": "boot",
                    "detail": "no pin can be driven until set_limits arrives"
                }),
                t,
                t,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
    }

    /// A reply as the gateway lands it: the firmware's answer is a JSON *string*.
    fn replied(world: &WorldMemory, id: &str, result: serde_json::Value, t: u64) {
        world
            .observe_as(
                &format!("mesh.{NODE}.cmd_result"),
                json!({
                    "type": "cmd_result", "node_id": NODE, "id": id, "ok": true,
                    "result": result.to_string()
                }),
                t,
                t,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
    }

    fn led_limit() -> SafetyLimit {
        SafetyLimit {
            node_id: NODE.into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![21]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: Some(500),
        }
    }

    #[tokio::test]
    async fn a_boot_announcement_gets_the_nodes_limits_pushed_once() {
        // The 2026-08-22 gap, closed: the node says it has no policy; the host
        // that holds one for it sends it, once per boot, and records which boot.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        announced_boot(&world, 0x1234_abcd, 1_000);
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();

        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 2_000).await,
            1
        );
        {
            let sent = mock.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].to, NODE);
            assert_eq!(sent[0].cmd, "set_limits");
            assert_eq!(
                sent[0].id, "lim1234abcd",
                "the id names the boot it answers"
            );
            assert_eq!(sent[0].args["limits"][0]["allowed_pins"], json!([21]));
            assert!(sent[0].fits_one_frame(), "{} bytes", sent[0].encoded_len());
        }

        let pushed = world
            .current(&format!("mesh.{NODE}.limits_pushed"))
            .unwrap()
            .unwrap();
        assert_eq!(pushed.value["boot_id"], json!(0x1234_abcd));
        assert_eq!(pushed.source, SUPERVISOR_SOURCE);

        // The mesh repeats; the same boot is not pushed twice on the announcement.
        announced_boot(&world, 0x1234_abcd, 3_000);
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 4_000).await,
            0
        );
        assert_eq!(mock.sent.lock().unwrap().len(), 1);
    }

    fn beacon(world: &WorldMemory, boot_id: u64, deny_all: bool, t: u64) {
        let mut b = json!({ "type": "beacon", "node_id": NODE, "ts_ms": t, "boot_id": boot_id });
        if deny_all {
            b["policy"] = json!("deny-all");
        }
        world
            .observe_as(
                &format!("mesh.{NODE}.beacon"),
                b,
                t,
                t,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
    }

    #[tokio::test]
    async fn a_push_the_mesh_lost_is_pushed_again_while_the_beacon_says_deny_all() {
        // Bench 2026-09-13 14:17: the push was sent, the node never got it (a
        // frame in three is lost under chatter), and the next beacon still said
        // deny-all. The beacon is the node's standing word; while it stands, the
        // push is owed again — after LIMITS_RETRY_MS, with a retry suffix on the
        // id — and the moment limits land the field is gone and so is the debt.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        beacon(&world, 5, true, 1_000);
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 2_000).await,
            1
        );
        // Too soon to judge — the next beacon has not come.
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 10_000).await,
            0
        );
        // The next beacon still says deny-all, and the retry interval has passed.
        beacon(&world, 5, true, 31_000);
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 32_000).await,
            1
        );
        {
            let sent = mock.sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[1].id, "lim00000005r1");
        }
        let pushed = world
            .current(&format!("mesh.{NODE}.limits_pushed"))
            .unwrap()
            .unwrap();
        assert_eq!(pushed.value["attempts"], json!(2));
        // Limits landed: the beacon drops the field; nothing more is owed.
        beacon(&world, 5, false, 61_000);
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 62_000).await,
            0
        );
        assert_eq!(mock.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_new_boot_id_on_any_reply_is_a_reset_and_gets_a_fresh_push() {
        // The announcement can be lost to the air (the base was not listening,
        // or the frame collided). Every set_limits and capabilities reply carries
        // boot_id too, and the supervisor's own recovery probe is `capabilities`.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        announced_boot(&world, 1, 1_000);
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 2_000).await,
            1
        );

        // The node's reply to that push confirms boot 1: nothing more to do.
        replied(
            &world,
            "lim00000001",
            json!({ "applied": true, "boot_id": 1 }),
            3_000,
        );
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 4_000).await,
            0
        );

        // Then a capabilities reply carries boot 2 — the node reset and the
        // announcement never arrived.
        replied(
            &world,
            "sup-x",
            json!({ "node_id": NODE, "boot_id": 2, "tools": [] }),
            5_000,
        );
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 6_000).await,
            1
        );
        let sent = mock.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].id, "lim00000002");
    }

    #[tokio::test]
    async fn a_beacon_that_still_says_deny_all_is_enough_to_push() {
        // Both boot announcements were lost on the bench; the beacon carries the
        // boot id every 30 s until limits land. No announcement, no reply — the
        // beacon alone triggers the push.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        world
            .observe_as(
                &format!("mesh.{NODE}.beacon"),
                json!({ "type": "beacon", "node_id": NODE, "ts_ms": 30_000,
                        "boot_id": 0xbeef, "policy": "deny-all" }),
                1_000,
                1_000,
                SOURCE,
                Origin::Observed,
            )
            .unwrap();
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[led_limit()], 2_000).await,
            1
        );
        assert_eq!(mock.sent.lock().unwrap()[0].id, "lim0000beef");
    }

    #[tokio::test]
    async fn a_node_with_no_configured_limits_stays_deny_all() {
        // Deny-all is the configuration, not an omission to repair.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        announced_boot(&world, 7, 1_000);
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();
        let other = SafetyLimit {
            node_id: "some-other-node".into(),
            ..led_limit()
        };
        assert_eq!(
            hydrate_limits(&world, Some(&sink), &[other], 2_000).await,
            0
        );
        assert!(mock.sent.lock().unwrap().is_empty());
        assert!(world
            .current(&format!("mesh.{NODE}.limits_pushed"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn limits_the_mesh_cannot_carry_are_recorded_not_retried() {
        // A frame over budget is discarded whole by the station; sending it every
        // tick would be silent forever. Record the refusal against the boot id and
        // leave the node deny-all, visibly.
        let world = WorldMemory::open_in_memory().unwrap();
        heard(&world, 1_000);
        announced_boot(&world, 9, 1_000);
        let mock = Arc::new(MockSink {
            sent: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn CommandSink> = mock.clone();
        let wide = SafetyLimit {
            allowed_pins: Some((1..=40).collect()),
            ..led_limit()
        };
        assert_eq!(
            hydrate_limits(&world, Some(&sink), std::slice::from_ref(&wide), 2_000).await,
            0
        );
        assert!(mock.sent.lock().unwrap().is_empty());
        let pushed = world
            .current(&format!("mesh.{NODE}.limits_pushed"))
            .unwrap()
            .unwrap();
        assert_eq!(pushed.value["boot_id"], json!(9));
        assert!(pushed.value["error"].as_str().unwrap().contains("bytes"));
        // …and not again next tick for the same boot.
        assert_eq!(hydrate_limits(&world, Some(&sink), &[wide], 3_000).await, 0);
    }

    #[test]
    fn the_boot_id_is_read_from_a_reply_whether_result_is_a_string_or_an_object() {
        assert_eq!(
            boot_id_in_reply(&json!({ "result": "{\"applied\":true,\"boot_id\":42}" })),
            Some(42)
        );
        assert_eq!(
            boot_id_in_reply(&json!({ "result": { "boot_id": 43 } })),
            Some(43)
        );
        assert_eq!(boot_id_in_reply(&json!({ "result": "0" })), None);
        assert_eq!(boot_id_in_reply(&json!({ "ok": false })), None);
    }
}

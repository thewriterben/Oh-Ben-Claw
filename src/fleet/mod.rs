//! Fleet coordination — one brain, many bodies.
//!
//! Each robot runs the full embodied stack (suites, reflexes, navigation,
//! missions, self-safing). This layer sits *above* a fleet of them: nodes report
//! their state (pose, battery, mode) as heartbeats; the [`Coordinator`] keeps a
//! [`FleetRegistry`], queues [`Task`]s, and **allocates** each to the best node —
//! the nearest online, idle node with enough battery. Assignments are advisory
//! (recorded into world memory `fleet.*`); the chosen node decides to act,
//! bounded by its own Track 0 gate. The coordinator never actuates directly — it
//! orchestrates autonomy, it doesn't bypass it.
//!
//! Pure and deterministic (no I/O of its own); the spine carries the heartbeats
//! and the assignment advisories. Optionally records the fleet view to world
//! memory for observability.

use crate::memory::world::WorldMemory;
use crate::navigation::planning::{plan, OccupancyGrid};
use crate::navigation::{exploration, NavGoal};
use crate::spine::{MessageHandler, SpineClient};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// MQTT topic filter for fleet node heartbeats (`obc/fleet/heartbeat/{node}`).
pub const HEARTBEAT_FILTER: &str = "obc/fleet/heartbeat/+";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

/// A node's last-reported state (a heartbeat).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeState {
    pub id: String,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    /// Battery state of charge (percent), if reported.
    #[serde(default)]
    pub battery: Option<f64>,
    /// The node's power/operating mode (e.g. `"normal"`, `"critical"`).
    #[serde(default)]
    pub mode: String,
    /// Whether the node is currently assigned a task.
    #[serde(default)]
    pub busy: bool,
    /// When this heartbeat was recorded (ms).
    pub last_seen_ms: u64,
}

impl NodeState {
    /// Online if its last heartbeat is within `stale_ms` of `now`.
    pub fn online(&self, now_ms: u64, stale_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) <= stale_ms
    }
}

/// Tracks the last-known state of every node.
#[derive(Debug, Clone, Default)]
pub struct FleetRegistry {
    nodes: HashMap<String, NodeState>,
}

impl FleetRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert/refresh a node's state (a heartbeat). Preserves `busy` unless the
    /// heartbeat explicitly changes it.
    pub fn upsert(&mut self, state: NodeState) {
        self.nodes.insert(state.id.clone(), state);
    }

    pub fn get(&self, id: &str) -> Option<&NodeState> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &NodeState> {
        self.nodes.values()
    }

    pub fn online_count(&self, now_ms: u64, stale_ms: u64) -> usize {
        self.nodes.values().filter(|n| n.online(now_ms, stale_ms)).count()
    }

    fn set_busy(&mut self, id: &str, busy: bool) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.busy = busy;
        }
    }
}

/// A unit of work for the fleet: visit a location, optionally needing battery.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Task {
    pub id: String,
    pub x: f64,
    pub y: f64,
    /// Minimum battery a node must have to take it (percent). Default 0.
    #[serde(default)]
    pub min_battery: f64,
}

/// Allocates a task to the best available node: the nearest *online, idle* node
/// with sufficient battery and a known position.
pub fn allocate(registry: &FleetRegistry, task: &Task, now_ms: u64, stale_ms: u64) -> Option<String> {
    registry
        .nodes()
        .filter(|n| n.online(now_ms, stale_ms) && !n.busy)
        .filter(|n| n.battery.map_or(true, |b| b >= task.min_battery))
        .filter_map(|n| match (n.x, n.y) {
            (Some(x), Some(y)) => {
                let d2 = (x - task.x).powi(2) + (y - task.y).powi(2);
                Some((n.id.clone(), d2))
            }
            _ => None,
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// A node's **bid** to service a task — its cost (lower wins), or `None` if the
/// node is ineligible (offline, busy, insufficient battery, or no known position).
/// Cost is travel distance plus a light battery tie-break (prefer fuller nodes).
pub fn bid(node: &NodeState, task: &Task, now_ms: u64, stale_ms: u64) -> Option<f64> {
    if !node.online(now_ms, stale_ms) || node.busy {
        return None;
    }
    if let Some(b) = node.battery {
        if b < task.min_battery {
            return None;
        }
    }
    let (x, y) = (node.x?, node.y?);
    let travel = ((x - task.x).powi(2) + (y - task.y).powi(2)).sqrt();
    let battery_penalty = node.battery.map_or(0.0, |b| (100.0 - b) * 0.001);
    Some(travel + battery_penalty)
}

/// **Market-based batch allocation** — a sequential single-item auction. Every
/// eligible `(node, task)` pair bids ([`bid`]); awards go to the globally lowest
/// bid first, each node and task taken at most once. Unlike per-task greedy
/// (which commits the first task to its nearest node and can strand a
/// globally-cheaper pairing), the auction is **order-independent** and globally
/// cheaper. Ties broken deterministically by task then node id. Returns
/// `(task_id, node_id)` awards.
pub fn auction_allocate(
    registry: &FleetRegistry,
    tasks: &[Task],
    now_ms: u64,
    stale_ms: u64,
) -> Vec<(String, String)> {
    let mut bids: Vec<(f64, String, String)> = Vec::new(); // (cost, task_id, node_id)
    for task in tasks {
        for node in registry.nodes() {
            if let Some(c) = bid(node, task, now_ms, stale_ms) {
                bids.push((c, task.id.clone(), node.id.clone()));
            }
        }
    }
    // Lowest cost first; deterministic tie-break by task id, then node id.
    bids.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut taken_tasks: HashSet<String> = HashSet::new();
    let mut taken_nodes: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (_, task_id, node_id) in bids {
        if taken_tasks.contains(&task_id) || taken_nodes.contains(&node_id) {
            continue;
        }
        taken_tasks.insert(task_id.clone());
        taken_nodes.insert(node_id.clone());
        out.push((task_id, node_id));
    }
    out
}

/// Coordinates a fleet: ingest heartbeats, queue tasks, allocate them to nodes.
pub struct Coordinator {
    registry: Mutex<FleetRegistry>,
    pending: Mutex<VecDeque<Task>>,
    assignments: Mutex<HashMap<String, String>>, // task_id -> node_id
    /// node_id -> claimed target location (for spatial conflict avoidance).
    claims: Mutex<HashMap<String, (f64, f64)>>,
    world: Option<Arc<WorldMemory>>,
    stale_ms: u64,
    /// Minimum separation between two nodes' targets (conflict avoidance).
    min_separation: f64,
    source: String,
    /// Off-grid: assignment intents `(node, x, y)` awaiting broadcast over a
    /// transport (e.g. the LoRa mesh). Only collected when `collect_outbox` is set;
    /// bounded so it never grows without a drain.
    outbox: Mutex<Vec<(String, f64, f64)>>,
    collect_outbox: bool,
}

impl Coordinator {
    pub fn new() -> Self {
        Self {
            registry: Mutex::new(FleetRegistry::new()),
            pending: Mutex::new(VecDeque::new()),
            assignments: Mutex::new(HashMap::new()),
            claims: Mutex::new(HashMap::new()),
            world: None,
            stale_ms: 30_000,
            min_separation: 2.0,
            source: "fleet".to_string(),
            outbox: Mutex::new(Vec::new()),
            collect_outbox: false,
        }
    }

    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Collect assignment intents into a bounded outbox for off-grid broadcast
    /// (e.g. the LoRa mesh). Off by default so single-brain deployments pay nothing.
    pub fn with_assignment_outbox(mut self) -> Self {
        self.collect_outbox = true;
        self
    }

    fn outbox(&self) -> std::sync::MutexGuard<'_, Vec<(String, f64, f64)>> {
        self.outbox.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Record an assignment intent for off-grid broadcast. No-op unless
    /// `with_assignment_outbox` was set; bounded so it never grows without a drain.
    fn enqueue_assignment(&self, node: &str, x: f64, y: f64) {
        if !self.collect_outbox {
            return;
        }
        const CAP: usize = 256;
        let mut ob = self.outbox();
        ob.push((node.to_string(), x, y));
        if ob.len() > CAP {
            let excess = ob.len() - CAP;
            ob.drain(0..excess);
        }
    }

    /// Drain the assignment outbox; a transport (LoRa mesh) broadcasts these as
    /// `MeshFrame::Assign`. Empty unless assignment-outbox collection is enabled.
    pub fn drain_outbox(&self) -> Vec<(String, f64, f64)> {
        std::mem::take(&mut *self.outbox())
    }

    pub fn with_stale_ms(mut self, stale_ms: u64) -> Self {
        self.stale_ms = stale_ms;
        self
    }

    /// Minimum distance to keep between two nodes' assigned targets.
    pub fn with_min_separation(mut self, sep: f64) -> Self {
        self.min_separation = sep;
        self
    }

    fn claims(&self) -> std::sync::MutexGuard<'_, HashMap<String, (f64, f64)>> {
        self.claims.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, FleetRegistry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn pending(&self) -> std::sync::MutexGuard<'_, VecDeque<Task>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn assignments(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.assignments.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Ingest a node heartbeat.
    pub fn report(&self, state: NodeState) {
        self.registry().upsert(state);
    }

    /// Queue a task for allocation.
    pub fn add_task(&self, task: Task) {
        self.pending().push_back(task);
    }

    /// Mark a task complete: free its node, clear the assignment and its claim.
    pub fn complete(&self, task_id: &str) -> bool {
        let node = self.assignments().remove(task_id);
        if let Some(node) = node {
            self.registry().set_busy(&node, false);
            self.claims().remove(&node);
            true
        } else {
            false
        }
    }

    /// Release a node's claim and free it (e.g. on heartbeat that reports idle).
    pub fn release(&self, node_id: &str) {
        self.claims().remove(node_id);
        self.registry().set_busy(node_id, false);
    }

    /// Coordinated multi-robot exploration: assign each idle online node a
    /// **distinct, separated, reachable** frontier of the shared map — so the
    /// fleet sweeps the unknown in parallel without two robots converging on the
    /// same area (conflict avoidance via `min_separation`). Records assignments
    /// and marks nodes busy. Returns `(node, goal)` per assignment.
    pub fn assign_exploration(&self, grid: &OccupancyGrid, now_ms: u64) -> Vec<(String, NavGoal)> {
        let frontiers: Vec<(f64, f64)> = exploration::frontier_cells(grid)
            .into_iter()
            .map(|(cx, cy)| grid.cell_center(cx, cy))
            .collect();
        if frontiers.is_empty() {
            return Vec::new();
        }
        // Idle, online nodes with a known position, nearest-task ordering applied
        // per node below.
        let mut candidates: Vec<(String, f64, f64)> = {
            let reg = self.registry();
            reg.nodes()
                .filter(|n| n.online(now_ms, self.stale_ms) && !n.busy)
                .filter_map(|n| match (n.x, n.y) {
                    (Some(x), Some(y)) => Some((n.id.clone(), x, y)),
                    _ => None,
                })
                .collect()
        };
        // Stable order for determinism.
        candidates.sort_by(|a, b| a.0.cmp(&b.0));

        // Targets already claimed (by other rounds) start the no-go set.
        let mut claimed: Vec<(f64, f64)> = self.claims().values().copied().collect();
        let mut assigned = Vec::new();

        for (id, nx, ny) in candidates {
            // frontiers far enough from every existing claim, nearest first
            let mut options: Vec<(f64, f64)> = frontiers
                .iter()
                .copied()
                .filter(|f| claimed.iter().all(|c| dist2(*f, *c) >= self.min_separation.powi(2)))
                .collect();
            options.sort_by(|a, b| dist2(*a, (nx, ny)).total_cmp(&dist2(*b, (nx, ny))));
            if let Some(target) = options.into_iter().find(|f| plan(grid, (nx, ny), *f).is_some()) {
                self.registry().set_busy(&id, true);
                self.claims().insert(id.clone(), target);
                self.enqueue_assignment(&id, target.0, target.1);
                claimed.push(target);
                if let Some(world) = &self.world {
                    let _ = world.observe(
                        &format!("fleet.explore.{id}"),
                        json!({ "x": target.0, "y": target.1 }),
                        now_ms,
                        now_ms,
                        &self.source,
                    );
                }
                assigned.push((id, NavGoal { x: target.0, y: target.1, tolerance: 0.5 }));
            }
        }
        self.record_status(now_ms);
        assigned
    }

    /// One coordination tick: allocate each pending task to the best node it can
    /// (leaving un-allocatable tasks queued), record assignments + the fleet view
    /// into world memory. Returns the assignments made this tick.
    pub fn tick(&self, now_ms: u64) -> Vec<(String, String)> {
        let mut made = Vec::new();
        // Try each queued task once; keep the ones that couldn't be placed.
        let mut requeue = VecDeque::new();
        loop {
            let task = { self.pending().pop_front() };
            let Some(task) = task else { break };
            let chosen = {
                let reg = self.registry();
                allocate(&reg, &task, now_ms, self.stale_ms)
            };
            match chosen {
                Some(node) => {
                    self.registry().set_busy(&node, true);
                    self.assignments().insert(task.id.clone(), node.clone());
                    self.enqueue_assignment(&node, task.x, task.y);
                    if let Some(world) = &self.world {
                        let _ = world.observe(
                            &format!("fleet.assignment.{}", task.id),
                            json!({ "node": node, "x": task.x, "y": task.y }),
                            now_ms,
                            now_ms,
                            &self.source,
                        );
                    }
                    made.push((task.id, node));
                }
                None => requeue.push_back(task),
            }
        }
        *self.pending() = requeue;
        self.record_status(now_ms);
        made
    }

    /// One coordination tick using a **market auction**: collect *all* queued
    /// tasks and allocate them together via [`auction_allocate`] (globally cheaper
    /// and queue-order-independent, vs [`tick`]'s one-at-a-time greedy). Winners are
    /// marked busy, assignments recorded, unawarded tasks requeued. Returns the
    /// awards made this tick.
    pub fn auction_tick(&self, now_ms: u64) -> Vec<(String, String)> {
        let tasks: Vec<Task> = self.pending().iter().cloned().collect();
        if tasks.is_empty() {
            self.record_status(now_ms);
            return Vec::new();
        }
        let awards = {
            let reg = self.registry();
            auction_allocate(&reg, &tasks, now_ms, self.stale_ms)
        };
        let awarded: HashSet<String> = awards.iter().map(|(t, _)| t.clone()).collect();
        for (task_id, node) in &awards {
            self.registry().set_busy(node, true);
            self.assignments().insert(task_id.clone(), node.clone());
            if let Some(t) = tasks.iter().find(|t| &t.id == task_id) {
                self.enqueue_assignment(node, t.x, t.y);
                if let Some(world) = &self.world {
                    let _ = world.observe(
                        &format!("fleet.assignment.{task_id}"),
                        json!({ "node": node, "x": t.x, "y": t.y, "via": "auction" }),
                        now_ms,
                        now_ms,
                        &self.source,
                    );
                }
            }
        }
        // Requeue tasks that found no eligible node this round.
        let remaining: VecDeque<Task> =
            tasks.into_iter().filter(|t| !awarded.contains(&t.id)).collect();
        *self.pending() = remaining;
        self.record_status(now_ms);
        awards
    }

    fn record_status(&self, now_ms: u64) {
        let Some(world) = &self.world else { return };
        let reg = self.registry();
        let online = reg.online_count(now_ms, self.stale_ms);
        let total = reg.nodes().count();
        let idle = reg
            .nodes()
            .filter(|n| n.online(now_ms, self.stale_ms) && !n.busy)
            .count();
        drop(reg);
        let body = json!({
            "nodes": total,
            "online": online,
            "idle": idle,
            "queued": self.pending().len(),
            "assignments": self.assignments().len(),
        });
        let _ = world.observe("fleet.status", body, now_ms, now_ms, &self.source);
    }

    /// A snapshot of the fleet for tools/observability.
    pub fn status(&self, now_ms: u64) -> Value {
        let reg = self.registry();
        let nodes: Vec<&NodeState> = reg.nodes().collect();
        json!({
            "nodes": nodes,
            "online": reg.online_count(now_ms, self.stale_ms),
            "queued": self.pending().len(),
            "assignments": self.assignments().clone(),
        })
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Spine bridge (distributed fleet over MQTT) ──────────────────────────────────

/// A spine message handler that ingests node heartbeats (`obc/fleet/heartbeat/
/// {node}`) into the coordinator: the node id is the last topic segment and the
/// payload carries `{x, y, battery, mode}`. Register with
/// [`SpineClient::subscribe_handler`].
pub fn spine_heartbeat_handler(coord: Arc<Coordinator>) -> MessageHandler {
    Arc::new(move |topic: &str, payload: &[u8]| {
        let id = topic.rsplit('/').next().unwrap_or("").to_string();
        if id.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(payload) else {
            return;
        };
        coord.report(NodeState {
            id,
            x: v.get("x").and_then(Value::as_f64),
            y: v.get("y").and_then(Value::as_f64),
            battery: v.get("battery").and_then(Value::as_f64),
            mode: v.get("mode").and_then(Value::as_str).unwrap_or("unknown").to_string(),
            busy: false,
            last_seen_ms: now_ms(),
        });
    })
}

/// The spine topic an assignment for `node` is published on (`obc/fleet/assign/{node}`).
/// Pure — the wire contract, testable without a broker.
pub fn assignment_topic(node: &str) -> String {
    format!("{}/fleet/assign/{node}", crate::spine::TOPIC_PREFIX)
}

/// The assignment payload for `goal`. Pure — the wire contract, testable without
/// a broker (mirrors the LoRa side's `MeshFrame::Assign`).
pub fn assignment_payload(goal: &NavGoal) -> Value {
    json!({ "x": goal.x, "y": goal.y, "tolerance": goal.tolerance })
}

/// Publish an assignment back to a node over the spine (`obc/fleet/assign/{node}`).
pub async fn publish_assignment(spine: &SpineClient, node: &str, goal: &NavGoal) -> anyhow::Result<()> {
    spine.publish(&assignment_topic(node), &assignment_payload(goal)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, x: f64, y: f64, battery: f64, t: u64) -> NodeState {
        NodeState {
            id: id.to_string(),
            x: Some(x),
            y: Some(y),
            battery: Some(battery),
            mode: "normal".to_string(),
            busy: false,
            last_seen_ms: t,
        }
    }

    #[test]
    fn allocates_to_the_nearest_idle_node() {
        let mut reg = FleetRegistry::new();
        reg.upsert(node("a", 0.0, 0.0, 80.0, 1_000));
        reg.upsert(node("b", 10.0, 0.0, 80.0, 1_000));
        let task = Task { id: "t1".into(), x: 9.0, y: 0.0, min_battery: 0.0 };
        assert_eq!(allocate(&reg, &task, 1_000, 30_000).as_deref(), Some("b"));
    }

    #[test]
    fn skips_offline_busy_and_low_battery_nodes() {
        let mut reg = FleetRegistry::new();
        reg.upsert(node("stale", 0.0, 0.0, 90.0, 0)); // last seen at 0, now far later
        let mut busy = node("busy", 0.0, 0.0, 90.0, 100_000);
        busy.busy = true;
        reg.upsert(busy);
        reg.upsert(node("low", 0.0, 0.0, 5.0, 100_000));
        reg.upsert(node("ok", 20.0, 0.0, 90.0, 100_000));
        let task = Task { id: "t".into(), x: 0.0, y: 0.0, min_battery: 20.0 };
        // only "ok" qualifies despite being farthest
        assert_eq!(allocate(&reg, &task, 100_000, 30_000).as_deref(), Some("ok"));
    }

    #[test]
    fn coordinator_assigns_marks_busy_and_completes() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let coord = Coordinator::new().with_world_memory(Arc::clone(&world));
        coord.report(node("a", 0.0, 0.0, 80.0, 1_000));
        coord.report(node("b", 10.0, 0.0, 80.0, 1_000));
        coord.add_task(Task { id: "t1".into(), x: 9.0, y: 0.0, min_battery: 0.0 });

        let made = coord.tick(1_000);
        assert_eq!(made, vec![("t1".to_string(), "b".to_string())]);
        // b is now busy → a second task goes to a
        coord.add_task(Task { id: "t2".into(), x: 0.0, y: 0.0, min_battery: 0.0 });
        let made = coord.tick(1_000);
        assert_eq!(made, vec![("t2".to_string(), "a".to_string())]);
        // assignment + status recorded
        assert_eq!(world.current("fleet.assignment.t1").unwrap().unwrap().value["node"], "b");
        assert!(world.current("fleet.status").unwrap().is_some());

        // completing t1 frees b
        assert!(coord.complete("t1"));
        coord.add_task(Task { id: "t3".into(), x: 10.0, y: 0.0, min_battery: 0.0 });
        let made = coord.tick(1_000);
        assert_eq!(made, vec![("t3".to_string(), "b".to_string())]);
    }

    #[test]
    fn unallocatable_task_stays_queued() {
        let coord = Coordinator::new();
        // no nodes → task can't be placed, stays queued
        coord.add_task(Task { id: "t".into(), x: 0.0, y: 0.0, min_battery: 0.0 });
        assert!(coord.tick(1_000).is_empty());
        // a node appears → next tick places it
        coord.report(node("a", 0.0, 0.0, 50.0, 1_000));
        assert_eq!(coord.tick(1_000).len(), 1);
    }

    use crate::navigation::planning::{Cell, OccupancyGrid};

    #[test]
    fn coordinated_exploration_gives_each_node_a_distinct_separated_frontier() {
        let coord = Coordinator::new().with_min_separation(2.0);
        coord.report(node("a", 1.0, 1.0, 80.0, 1_000)); // near the low pocket
        coord.report(node("b", 8.0, 8.0, 80.0, 1_000)); // near the high pocket
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cx in 0..3 {
            for cy in 0..3 {
                g.set(cx, cy, Cell::Free); // pocket near a
            }
        }
        for cx in 7..10 {
            for cy in 7..10 {
                g.set(cx, cy, Cell::Free); // pocket near b
            }
        }
        let assigned = coord.assign_exploration(&g, 1_000);
        assert_eq!(assigned.len(), 2, "both nodes get a frontier");
        let a = &assigned.iter().find(|(n, _)| n == "a").unwrap().1;
        let b = &assigned.iter().find(|(n, _)| n == "b").unwrap().1;
        let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
        assert!(d >= 2.0, "targets are separated, d={d}");
    }

    #[test]
    fn auction_makes_the_globally_cheap_award_regardless_of_task_order() {
        // a at origin, b far at x=10. t1 sits in the middle (5,0); t2 is right next
        // to b (9,0). Per-task greedy on [t1,t2] with a tie at t1 could send b to t1
        // and strand t2 onto a (cost 1+9=10). The auction awards the cheapest pair
        // (t2→b, cost 1) first, then t1→a — total 6, and crucially t2→b.
        let mut reg = FleetRegistry::new();
        reg.upsert(node("a", 0.0, 0.0, 90.0, 1_000));
        reg.upsert(node("b", 10.0, 0.0, 90.0, 1_000));
        let tasks = vec![
            Task { id: "t1".into(), x: 5.0, y: 0.0, min_battery: 0.0 },
            Task { id: "t2".into(), x: 9.0, y: 0.0, min_battery: 0.0 },
        ];
        let awards: HashMap<String, String> =
            auction_allocate(&reg, &tasks, 1_000, 30_000).into_iter().collect();
        assert_eq!(awards.get("t2").map(String::as_str), Some("b"), "near task → near node");
        assert_eq!(awards.get("t1").map(String::as_str), Some("a"));
    }

    #[test]
    fn auction_respects_battery_eligibility() {
        let mut reg = FleetRegistry::new();
        reg.upsert(node("a", 0.0, 0.0, 10.0, 1_000)); // closest but low battery
        reg.upsert(node("b", 5.0, 0.0, 90.0, 1_000));
        let tasks = vec![Task { id: "hot".into(), x: 0.0, y: 0.0, min_battery: 50.0 }];
        let awards = auction_allocate(&reg, &tasks, 1_000, 30_000);
        assert_eq!(awards, vec![("hot".to_string(), "b".to_string())]);
    }

    #[test]
    fn auction_tick_awards_marks_busy_and_requeues_the_rest() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let coord = Coordinator::new().with_world_memory(Arc::clone(&world));
        coord.report(node("a", 0.0, 0.0, 80.0, 1_000));
        // two tasks, one node → one awarded, one requeued
        coord.add_task(Task { id: "t1".into(), x: 1.0, y: 0.0, min_battery: 0.0 });
        coord.add_task(Task { id: "t2".into(), x: 2.0, y: 0.0, min_battery: 0.0 });
        let awards = coord.auction_tick(1_000);
        assert_eq!(awards.len(), 1, "one node can take one task");
        assert_eq!(awards[0].1, "a");
        // the assignment was recorded via the auction path
        let won = &awards[0].0;
        assert_eq!(
            world.current(&format!("fleet.assignment.{won}")).unwrap().unwrap().value["via"],
            "auction"
        );
        // a second node arrives → the requeued task is placed next tick
        coord.report(node("b", 2.0, 0.0, 80.0, 1_000));
        let awards2 = coord.auction_tick(1_000);
        assert_eq!(awards2.len(), 1);
        assert_eq!(awards2[0].1, "b");
    }

    #[test]
    fn heartbeat_handler_ingests_a_node_report() {
        let coord = Arc::new(Coordinator::new());
        let handler = spine_heartbeat_handler(Arc::clone(&coord));
        let payload =
            serde_json::to_vec(&json!({ "x": 3.0, "y": 4.0, "battery": 72.0, "mode": "normal" }))
                .unwrap();
        handler("obc/fleet/heartbeat/rover-7", &payload);
        let status = coord.status(now_ms());
        let nodes = status["nodes"].as_array().unwrap();
        assert!(nodes.iter().any(|n| n["id"] == "rover-7" && n["battery"] == 72.0));
    }

    #[test]
    fn min_separation_prevents_two_nodes_in_one_small_area() {
        let coord = Coordinator::new().with_min_separation(5.0);
        coord.report(node("a", 1.0, 1.0, 80.0, 1_000));
        coord.report(node("b", 1.5, 1.5, 80.0, 1_000)); // both crowd one pocket
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cx in 0..3 {
            for cy in 0..3 {
                g.set(cx, cy, Cell::Free); // a single small pocket
            }
        }
        // with a large separation, only one node can claim in this pocket
        assert_eq!(coord.assign_exploration(&g, 1_000).len(), 1);
    }
}

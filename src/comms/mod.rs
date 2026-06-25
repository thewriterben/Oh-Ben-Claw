//! Comms subsystem — link/network telemetry into world memory, with a derived
//! per-link state and an aggregate **net mode** reflexes can use for offline /
//! degraded-mode safing.
//!
//! Like power, comms *perceives*: a [`LinkReading`] (signal, latency, loss, up)
//! for a named link is classified into a [`LinkState`] and recorded as
//! `link.{name}`. The cross-cutting value is aggregation — the controller tracks
//! every link it has seen and records the **best** state as `net.mode`. A reflex
//! watching `net.mode` can drop to a low-bandwidth/offline-safe behavior when
//! connectivity degrades (buffer telemetry, stop streaming, fall back to the
//! local System 1), and recover when a link comes back. Perceive → remember →
//! reflex, same spine as the other suites.

use crate::memory::world::WorldMemory;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Health of a single network link / the aggregate network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Up and within thresholds.
    Online,
    /// Up but breaching a threshold (weak signal / high latency / loss).
    Degraded,
    /// Down.
    Offline,
    /// Not enough information to classify.
    Unknown,
}

impl LinkState {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkState::Online => "online",
            LinkState::Degraded => "degraded",
            LinkState::Offline => "offline",
            LinkState::Unknown => "unknown",
        }
    }

    /// Rank for aggregation — higher is healthier. The aggregate `net.mode` is
    /// the highest-ranked (best) link.
    fn rank(&self) -> u8 {
        match self {
            LinkState::Online => 3,
            LinkState::Degraded => 2,
            LinkState::Offline => 1,
            LinkState::Unknown => 0,
        }
    }
}

/// A single link telemetry reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkReading {
    /// Link id (e.g. `"wifi"`, `"lte"`, `"spine"`). Becomes `link.{link}`.
    pub link: String,
    /// Signal strength in dBm (higher is better; e.g. -55 strong, -90 weak).
    #[serde(default)]
    pub rssi_dbm: Option<f64>,
    /// Round-trip latency in ms.
    #[serde(default)]
    pub latency_ms: Option<f64>,
    /// Packet loss percent `0..=100`.
    #[serde(default)]
    pub loss_pct: Option<f64>,
    /// Explicit up/down, if known. `Some(false)` forces [`LinkState::Offline`].
    #[serde(default)]
    pub up: Option<bool>,
    /// Who reported it (node / probe).
    #[serde(default)]
    pub source: Option<String>,
}

/// Thresholds bounding [`LinkState::Online`] vs [`LinkState::Degraded`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkThresholds {
    /// Below this RSSI (dBm) ⇒ degraded.
    pub min_rssi_dbm: f64,
    /// Above this latency (ms) ⇒ degraded.
    pub max_latency_ms: f64,
    /// Above this loss (%) ⇒ degraded.
    pub max_loss_pct: f64,
}

impl Default for LinkThresholds {
    fn default() -> Self {
        Self {
            min_rssi_dbm: -80.0,
            max_latency_ms: 500.0,
            max_loss_pct: 5.0,
        }
    }
}

impl LinkThresholds {
    /// Classify a reading. An explicit `up == false` is offline regardless of
    /// metrics; with no up flag and no metrics the link is unknown; otherwise a
    /// breach of any threshold is degraded, else online.
    pub fn derive(&self, r: &LinkReading) -> LinkState {
        if r.up == Some(false) {
            return LinkState::Offline;
        }
        let has_metric = r.rssi_dbm.is_some() || r.latency_ms.is_some() || r.loss_pct.is_some();
        if r.up.is_none() && !has_metric {
            return LinkState::Unknown;
        }
        let degraded = r.rssi_dbm.is_some_and(|v| v < self.min_rssi_dbm)
            || r.latency_ms.is_some_and(|v| v > self.max_latency_ms)
            || r.loss_pct.is_some_and(|v| v > self.max_loss_pct);
        if degraded {
            LinkState::Degraded
        } else {
            LinkState::Online
        }
    }
}

/// A reading after classification + aggregation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkStatus {
    pub link: String,
    pub state: LinkState,
    /// Aggregate network mode (best link) after this reading.
    pub net_mode: LinkState,
    pub at_ms: u64,
}

/// Ingests link telemetry, classifies each link, and records `link.{name}` plus
/// an aggregate `net.mode` into world memory.
pub struct CommsController {
    world: Option<Arc<WorldMemory>>,
    thresholds: LinkThresholds,
    /// link -> last classified state (for aggregation).
    last: Mutex<HashMap<String, LinkState>>,
    source: String,
}

impl CommsController {
    /// Build a controller with the given thresholds.
    pub fn new(thresholds: LinkThresholds) -> Self {
        Self {
            world: None,
            thresholds,
            last: Mutex::new(HashMap::new()),
            source: "comms".to_string(),
        }
    }

    /// Record telemetry into world memory (enables §3 Remember + reflex safing).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Override the world-memory `source` label (default `"comms"`).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, LinkState>> {
        self.last.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Aggregate the best known link state. `Offline` when no link is known.
    fn net_mode(&self) -> LinkState {
        self.lock()
            .values()
            .copied()
            .max_by_key(|s| s.rank())
            .unwrap_or(LinkState::Offline)
    }

    /// Ingest a reading: classify the link, record `link.{name}`, update the
    /// aggregate, and record `net.mode`.
    pub fn ingest(&self, reading: &LinkReading, now_ms: u64) -> anyhow::Result<LinkStatus> {
        let state = self.thresholds.derive(reading);
        self.lock().insert(reading.link.clone(), state);
        let net_mode = self.net_mode();

        if let Some(world) = &self.world {
            let entity = format!("link.{}", reading.link);
            let value = json!({
                "state": state.as_str(),
                "rssi_dbm": reading.rssi_dbm,
                "latency_ms": reading.latency_ms,
                "loss_pct": reading.loss_pct,
                "up": reading.up,
                "source": reading.source,
            });
            world.observe(&entity, value, now_ms, now_ms, &self.source)?;
            let net = json!({ "mode": net_mode.as_str(), "links": self.lock().len() });
            world.observe("net.mode", net, now_ms, now_ms, &self.source)?;
        }

        Ok(LinkStatus {
            link: reading.link.clone(),
            state,
            net_mode,
            at_ms: now_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> (CommsController, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = CommsController::new(LinkThresholds::default()).with_world_memory(Arc::clone(&world));
        (ctrl, world)
    }

    fn reading(link: &str) -> LinkReading {
        LinkReading {
            link: link.to_string(),
            rssi_dbm: None,
            latency_ms: None,
            loss_pct: None,
            up: None,
            source: Some("probe".to_string()),
        }
    }

    #[test]
    fn derive_classifies_states() {
        let t = LinkThresholds::default();
        // strong, fast, no loss → online
        let mut r = reading("wifi");
        r.rssi_dbm = Some(-55.0);
        r.latency_ms = Some(20.0);
        r.loss_pct = Some(0.0);
        assert_eq!(t.derive(&r), LinkState::Online);
        // high latency → degraded
        r.latency_ms = Some(900.0);
        assert_eq!(t.derive(&r), LinkState::Degraded);
        // explicit down → offline
        let mut d = reading("lte");
        d.up = Some(false);
        assert_eq!(t.derive(&d), LinkState::Offline);
        // nothing known → unknown
        assert_eq!(t.derive(&reading("spine")), LinkState::Unknown);
    }

    #[test]
    fn net_mode_is_best_link() {
        let (ctrl, world) = controller();
        // wifi offline, lte online → net online
        let mut wifi = reading("wifi");
        wifi.up = Some(false);
        ctrl.ingest(&wifi, 1_000).unwrap();
        let mut lte = reading("lte");
        lte.up = Some(true);
        lte.latency_ms = Some(50.0);
        let status = ctrl.ingest(&lte, 1_100).unwrap();
        assert_eq!(status.state, LinkState::Online);
        assert_eq!(status.net_mode, LinkState::Online);
        assert_eq!(world.current("net.mode").unwrap().unwrap().value["mode"], "online");
    }

    #[test]
    fn all_links_down_is_offline_net() {
        let (ctrl, world) = controller();
        let mut wifi = reading("wifi");
        wifi.up = Some(false);
        ctrl.ingest(&wifi, 1_000).unwrap();
        let mut lte = reading("lte");
        lte.up = Some(false);
        let status = ctrl.ingest(&lte, 1_100).unwrap();
        assert_eq!(status.net_mode, LinkState::Offline);
        assert_eq!(world.current("net.mode").unwrap().unwrap().value["mode"], "offline");
    }

    #[test]
    fn link_fact_records_metrics_and_state() {
        let (ctrl, world) = controller();
        let mut wifi = reading("wifi");
        wifi.rssi_dbm = Some(-90.0); // below -80 → degraded
        ctrl.ingest(&wifi, 1_000).unwrap();
        let fact = world.current("link.wifi").unwrap().unwrap();
        assert_eq!(fact.value["state"], "degraded");
        assert!((fact.value["rssi_dbm"].as_f64().unwrap() + 90.0).abs() < 1e-9);
        assert_eq!(fact.source, "comms");
    }

    #[test]
    fn reading_roundtrips() {
        let mut r = reading("wifi");
        r.rssi_dbm = Some(-60.0);
        r.up = Some(true);
        let back: LinkReading = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}

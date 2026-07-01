//! Dynamic trust scoring — a behavioral hardening of Track 0.
//!
//! OBC already authenticates nodes (HMAC pairing) and gates physical actions by
//! [`RiskClass`], but that trust is *static*: a paired node keeps its privileges no
//! matter how it behaves. This layer adds a **continuous trust score** per node
//! that decays on anomalous behavior — latency spikes (rolling-mean + 3σ z-score)
//! and failures — and recovers on sustained normal behavior. The score maps to a
//! [`TrustLevel`], and [`gate`] turns that level into an approval decision: a node
//! on **probation** is forced back through per-call approval for physical actions
//! it could previously auto-run; an **untrusted** node is quarantined from physical
//! actuation entirely. Reads/compute (non-physical) are never blocked by trust.
//!
//! The *idea* is adapted from a sibling project's device-authentication module; the
//! implementation here is original and self-contained (no external deps).

use crate::tools::traits::{BlastRadius, RiskClass};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// A node's behavioral trust tier, derived from its current score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    /// Behaving normally — honor its existing approval scopes.
    Trusted,
    /// Recent anomalies/failures — demote: physical actions need fresh approval.
    Probation,
    /// Sustained bad behavior — quarantine from physical actuation.
    Untrusted,
}

/// The Track 0 decision for an action given a node's trust level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustGate {
    /// Proceed under the normal approval rules.
    Allow,
    /// Force per-call approval (override any auto/forever grant).
    RequireApproval,
    /// Refuse — the node is too untrusted for this physical action.
    Deny,
}

/// Tunables for the scorer.
#[derive(Debug, Clone, Copy)]
pub struct TrustConfig {
    /// Rolling latency window used for anomaly detection.
    pub window: usize,
    /// |z-score| above this marks a latency reading anomalous.
    pub z_threshold: f64,
    /// Score subtracted on an anomalous latency.
    pub anomaly_penalty: f64,
    /// Score subtracted on a failed interaction.
    pub failure_penalty: f64,
    /// Score added on a clean (successful, non-anomalous) interaction.
    pub recovery: f64,
    /// Score at or above this ⇒ `Trusted`.
    pub trusted_at: f64,
    /// Score at or above this (but below `trusted_at`) ⇒ `Probation`; below ⇒ `Untrusted`.
    pub probation_at: f64,
    /// Score a node starts with (a freshly-paired node is trusted).
    pub start_score: f64,
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            window: 16,
            z_threshold: 3.0,
            anomaly_penalty: 0.25,
            failure_penalty: 0.34,
            recovery: 0.05,
            trusted_at: 0.7,
            probation_at: 0.3,
            start_score: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
struct NodeStats {
    latencies: VecDeque<f64>,
    successes: u64,
    failures: u64,
    score: f64,
}

/// Per-node behavioral trust scorer. Shareable (`Arc<TrustScorer>`); interior-mutable.
#[derive(Debug)]
pub struct TrustScorer {
    cfg: TrustConfig,
    nodes: Mutex<HashMap<String, NodeStats>>,
}

impl TrustScorer {
    pub fn new(cfg: TrustConfig) -> Self {
        Self { cfg, nodes: Mutex::new(HashMap::new()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, NodeStats>> {
        self.nodes.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether `latency_ms` is anomalous vs the existing window (rolling mean + σ).
    /// Needs ≥3 samples and non-degenerate spread to fire.
    fn anomalous(&self, latency_ms: f64, window: &VecDeque<f64>) -> bool {
        let n = window.len();
        if n < 3 {
            return false;
        }
        let n_f = n as f64;
        let mean = window.iter().sum::<f64>() / n_f;
        let var = window.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n_f;
        let std = var.sqrt();
        if std <= 1e-9 {
            return false;
        }
        ((latency_ms - mean) / std).abs() > self.cfg.z_threshold
    }

    fn level_for(&self, score: f64) -> TrustLevel {
        if score >= self.cfg.trusted_at {
            TrustLevel::Trusted
        } else if score >= self.cfg.probation_at {
            TrustLevel::Probation
        } else {
            TrustLevel::Untrusted
        }
    }

    /// Record one interaction with `node` (its response latency and whether it
    /// succeeded), update the trust score, and return the node's new level.
    pub fn record(&self, node: &str, latency_ms: f64, success: bool) -> TrustLevel {
        let mut map = self.lock();
        let cfg = self.cfg;
        let stats = map.entry(node.to_string()).or_insert_with(|| NodeStats {
            latencies: VecDeque::with_capacity(cfg.window),
            successes: 0,
            failures: 0,
            score: cfg.start_score,
        });

        // Anomaly is judged against the window *before* adding this sample.
        let anomaly = self.anomalous(latency_ms, &stats.latencies);

        let mut delta = 0.0;
        if success {
            stats.successes += 1;
        } else {
            stats.failures += 1;
            delta -= cfg.failure_penalty;
        }
        if anomaly {
            delta -= cfg.anomaly_penalty;
        }
        if success && !anomaly {
            delta += cfg.recovery;
        }
        stats.score = (stats.score + delta).clamp(0.0, 1.0);

        stats.latencies.push_back(latency_ms);
        if stats.latencies.len() > cfg.window {
            stats.latencies.pop_front();
        }
        self.level_for(stats.score)
    }

    /// The node's current trust score in `[0, 1]` (a never-seen node starts at
    /// `start_score`).
    pub fn score(&self, node: &str) -> f64 {
        self.lock().get(node).map(|s| s.score).unwrap_or(self.cfg.start_score)
    }

    /// The node's current trust level (a never-seen node is `Trusted`).
    pub fn level(&self, node: &str) -> TrustLevel {
        self.level_for(self.score(node))
    }
}

impl Default for TrustScorer {
    fn default() -> Self {
        Self::new(TrustConfig::default())
    }
}

/// Modulate a Track 0 decision by trust level. Non-physical actions are never
/// blocked by trust; physical actions get progressively stricter as trust falls:
/// `Trusted` proceeds normally, `Probation` forces per-call approval, `Untrusted`
/// is denied (quarantine). High-blast physical actions are denied a hair earlier —
/// an untrusted node never drives a motor or unlocks a door.
pub fn gate(level: TrustLevel, risk: RiskClass) -> TrustGate {
    if !risk.physical {
        return TrustGate::Allow;
    }
    match level {
        TrustLevel::Trusted => TrustGate::Allow,
        TrustLevel::Probation => {
            // A high-blast action from a node already on probation is too risky to
            // merely re-approve repeatedly — deny it; lower-blast forces approval.
            if matches!(risk.blast, BlastRadius::High) {
                TrustGate::Deny
            } else {
                TrustGate::RequireApproval
            }
        }
        TrustLevel::Untrusted => TrustGate::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TrustConfig {
        TrustConfig::default()
    }

    #[test]
    fn a_fresh_node_is_trusted() {
        let t = TrustScorer::default();
        assert_eq!(t.level("new-node"), TrustLevel::Trusted);
        assert!((t.score("new-node") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn steady_normal_behavior_stays_trusted() {
        let t = TrustScorer::new(cfg());
        for _ in 0..20 {
            t.record("n", 50.0, true);
        }
        assert_eq!(t.level("n"), TrustLevel::Trusted);
    }

    #[test]
    fn repeated_failures_demote_then_quarantine() {
        let t = TrustScorer::new(cfg());
        // start at 1.0; each failure subtracts 0.34
        assert_eq!(t.record("bad", 50.0, false), TrustLevel::Probation); // 0.66
        assert_eq!(t.record("bad", 50.0, false), TrustLevel::Probation); // 0.32
        assert_eq!(t.record("bad", 50.0, false), TrustLevel::Untrusted); // 0.0
    }

    #[test]
    fn a_latency_spike_is_penalized_as_anomalous() {
        let t = TrustScorer::new(cfg());
        // a realistic, slightly-varied baseline (~50ms, non-zero σ)
        for &b in &[48.0, 50.0, 52.0, 49.0, 51.0, 50.0, 48.0, 52.0] {
            t.record("n", b, true);
        }
        let before = t.score("n");
        // a 10x spike is far outside 3σ → anomaly penalty even though it "succeeded"
        t.record("n", 500.0, true);
        assert!(t.score("n") < before, "anomalous latency should lower trust");
    }

    #[test]
    fn trust_recovers_with_sustained_good_behavior() {
        let t = TrustScorer::new(cfg());
        t.record("n", 50.0, false); // dip to 0.66
        t.record("n", 50.0, false); // 0.32 (probation)
        assert_eq!(t.level("n"), TrustLevel::Probation);
        for _ in 0..20 {
            t.record("n", 50.0, true); // recover at +0.05 each
        }
        assert_eq!(t.level("n"), TrustLevel::Trusted);
    }

    #[test]
    fn gate_leaves_non_physical_actions_alone() {
        for lvl in [TrustLevel::Trusted, TrustLevel::Probation, TrustLevel::Untrusted] {
            assert_eq!(gate(lvl, RiskClass::safe()), TrustGate::Allow);
        }
    }

    #[test]
    fn gate_tightens_physical_actions_as_trust_falls() {
        let low = RiskClass::physical(true, BlastRadius::Low);
        let high = RiskClass::physical(false, BlastRadius::High);

        assert_eq!(gate(TrustLevel::Trusted, low), TrustGate::Allow);
        assert_eq!(gate(TrustLevel::Trusted, high), TrustGate::Allow);

        assert_eq!(gate(TrustLevel::Probation, low), TrustGate::RequireApproval);
        assert_eq!(gate(TrustLevel::Probation, high), TrustGate::Deny);

        assert_eq!(gate(TrustLevel::Untrusted, low), TrustGate::Deny);
        assert_eq!(gate(TrustLevel::Untrusted, high), TrustGate::Deny);
    }
}

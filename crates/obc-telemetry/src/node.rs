//! What a node last said about itself — the heartbeat every other layer reads.
//!
//! Moved out of `fleet` on 2026-08-06, and the reason is the same one that moved
//! `RiskClass` out of the module that consumed it: this 25-line struct was the
//! *only* thing `aerial` and `gnss` needed from `fleet`, and needing it made
//! them depend on a 823-line coordinator they otherwise never touch. Measured
//! before the move — `src/aerial` and `src/gnss` each had exactly one
//! `use crate::fleet::…` line, and it was this type.
//!
//! It belongs here rather than in `fleet` because it is telemetry, not
//! coordination: a battery percentage, an operating mode, a position and a
//! timestamp. `mode` uses the same vocabulary [`crate::power`] derives
//! (`normal` / `low` / `critical` / `charging`), which is the tell — the fleet
//! coordinator *reads* this, it does not define it.
//!
//! `fleet` re-exports it, so `crate::fleet::NodeState` is unchanged at every
//! call site.

use serde::{Deserialize, Serialize};

/// A node's last-reported state (a heartbeat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(last_seen_ms: u64) -> NodeState {
        NodeState {
            id: "node-001".into(),
            x: None,
            y: None,
            battery: None,
            mode: String::new(),
            busy: false,
            last_seen_ms,
        }
    }

    #[test]
    fn a_recent_heartbeat_is_online() {
        assert!(at(1_000).online(1_500, 1_000));
    }

    #[test]
    fn a_stale_heartbeat_is_not() {
        assert!(!at(1_000).online(2_500, 1_000));
    }

    /// Exactly at the boundary counts as online: `stale_ms` is the age at which
    /// a node is still trusted, not the age at which it is dropped. Pinned
    /// because the comparison is `<=` and a later reader may assume `<`.
    #[test]
    fn the_boundary_is_inclusive() {
        assert!(at(1_000).online(2_000, 1_000));
        assert!(!at(1_000).online(2_001, 1_000));
    }

    /// `now` before `last_seen` would underflow a plain subtraction. A clock
    /// that steps backwards must not make a live node look ancient.
    #[test]
    fn a_clock_that_went_backwards_does_not_underflow() {
        assert!(at(5_000).online(1_000, 0));
    }

    /// The type crosses the wire as a heartbeat, so the derives have to survive
    /// a round trip — it gained `Deserialize` in the move, and nothing else.
    #[test]
    fn it_round_trips_as_json() {
        let n = NodeState {
            id: "node-002".into(),
            x: Some(1.5),
            y: Some(-2.0),
            battery: Some(87.5),
            mode: "normal".into(),
            busy: true,
            last_seen_ms: 42,
        };
        let back: NodeState = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(n, back);
    }
}

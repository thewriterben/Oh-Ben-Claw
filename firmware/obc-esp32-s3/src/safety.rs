//! On-MCU Track 0 safety gate — deterministic, host-pushable actuator limits.
//!
//! The host's `oh_ben_claw::security::limits::SafetyGate` decides whether a
//! physical action is *allowed* using fixed rules the model cannot influence.
//! This is the node-side mirror: a compromised host, a poisoned skill, or a
//! hallucinated tool call still cannot drive a pin outside policy, because the
//! final check runs on the MCU itself.
//!
//! Previously this gate was three compile-time constants (`OUTPUT_PINS`, a value
//! max, no rate limit). This module makes it a real, evolvable policy:
//!
//! - **Allow-list** of output pins (default-deny for anything else).
//! - **Value range** (inclusive min/max).
//! - **Per-pin rate limit** (minimum interval between writes) — new; needs the
//!   monotonic `esp_timer` clock the main loop already reads.
//! - **Host-pushable**: a [`SafetyLimit`] set arrives over the retained
//!   `obc/nodes/{id}/limits` topic (or the `set_limits` serial command) and
//!   *replaces* the active policy, so limits can be tightened in the field
//!   without reflashing. The boot default reproduces the old constants exactly,
//!   so behaviour is unchanged until a host pushes something stricter.
//!
//! Wire-compatible with the host [`SafetyLimit`] JSON (same field names), so a
//! limit authored host-side validates identically here. Pure (`std` + `serde`),
//! so it unit-tests on the host like `reflex` and `safing`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A deterministic limit for a `(node, tool)` pair — mirror of the host
/// `SafetyLimit`. The node applies the `gpio_write` limit addressed to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyLimit {
    /// Node this limit applies to. Empty ⇒ "any node" (accepted by this node).
    #[serde(default)]
    pub node_id: String,
    /// Tool this limit governs (this firmware only gates `"gpio_write"`).
    pub tool: String,
    /// Allowed pins; `None` = any pin, empty = no pins (default-deny).
    #[serde(default)]
    pub allowed_pins: Option<Vec<i64>>,
    /// Inclusive minimum value. `None` = unbounded below.
    #[serde(default)]
    pub value_min: Option<i64>,
    /// Inclusive maximum value. `None` = unbounded above.
    #[serde(default)]
    pub value_max: Option<i64>,
    /// Minimum milliseconds between writes to a pin (rate limit). `None` = none.
    #[serde(default)]
    pub min_interval_ms: Option<u64>,
}

/// Why a physical action was refused by the gate (mirror of the host enum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyViolation {
    /// The pin is not in the policy's allow-list.
    PinNotAllowed { pin: i64 },
    /// The value is outside the permitted range.
    ValueOutOfRange { value: i64, min: Option<i64>, max: Option<i64> },
    /// The write fired again before the minimum interval elapsed.
    RateLimited { min_interval_ms: u64, since_last_ms: u64 },
}

impl std::fmt::Display for SafetyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafetyViolation::PinNotAllowed { pin } => {
                write!(f, "safety: pin {pin} not in allow-list")
            }
            SafetyViolation::ValueOutOfRange { value, min, max } => {
                write!(f, "safety: value {value} out of range (min={min:?}, max={max:?})")
            }
            SafetyViolation::RateLimited { min_interval_ms, since_last_ms } => {
                write!(f, "safety: rate limit ({since_last_ms}ms since last, min {min_interval_ms}ms)")
            }
        }
    }
}

impl std::error::Error for SafetyViolation {}

/// Enforces the active `gpio_write` policy on the MCU. Single-threaded on the
/// node, so rate state is a plain map updated via `&mut self` (the host uses a
/// Mutex for shared access).
#[derive(Debug)]
pub struct SafetyGate {
    /// The active `gpio_write` policy: default-deny base, replaceable by a host push.
    policy: SafetyLimit,
    /// pin -> last write time (ms), for the per-pin rate limit.
    last_fire: HashMap<i64, u64>,
}

impl SafetyGate {
    /// Boot default: allow-list the configured output pins, digital range 0..=1,
    /// no rate limit. This reproduces the old compile-time `safety_check_gpio_write`
    /// exactly, so nothing changes until a host pushes a stricter policy.
    pub fn with_output_pins(pins: &[i32]) -> Self {
        Self {
            policy: SafetyLimit {
                node_id: String::new(),
                tool: "gpio_write".to_string(),
                allowed_pins: Some(pins.iter().map(|&p| p as i64).collect()),
                value_min: Some(0),
                value_max: Some(1),
                min_interval_ms: None,
            },
            last_fire: HashMap::new(),
        }
    }

    /// Apply a host-pushed limit set: adopt the `gpio_write` limit addressed to
    /// this node (or to any node), *replacing* the active policy. Returns `true`
    /// if a matching limit was found. Clears rate state so the new policy starts
    /// clean. A push that contains no `gpio_write` limit leaves the policy intact
    /// (a node never silently loses its actuator gate).
    pub fn apply_pushed(&mut self, limits: Vec<SafetyLimit>, node_id: &str) -> bool {
        if let Some(limit) = limits.into_iter().find(|l| {
            l.tool == "gpio_write" && (l.node_id.is_empty() || l.node_id == node_id)
        }) {
            self.policy = limit;
            self.last_fire.clear();
            true
        } else {
            false
        }
    }

    /// A snapshot of the active policy (for the `set_limits` ack / diagnostics).
    pub fn policy(&self) -> &SafetyLimit {
        &self.policy
    }

    /// Check a `gpio_write` against the active policy, recording the fire time for
    /// rate limiting. `Ok(())` if allowed, else the first [`SafetyViolation`].
    pub fn check(&mut self, pin: i64, value: i64, now_ms: u64) -> Result<(), SafetyViolation> {
        // 1) Pin allow-list (default-deny when a list is present).
        if let Some(allowed) = &self.policy.allowed_pins {
            if !allowed.contains(&pin) {
                return Err(SafetyViolation::PinNotAllowed { pin });
            }
        }

        // 2) Value range.
        if self.policy.value_min.map_or(false, |min| value < min)
            || self.policy.value_max.map_or(false, |max| value > max)
        {
            return Err(SafetyViolation::ValueOutOfRange {
                value,
                min: self.policy.value_min,
                max: self.policy.value_max,
            });
        }

        // 3) Per-pin rate limit.
        if let Some(min_interval) = self.policy.min_interval_ms {
            if let Some(&last) = self.last_fire.get(&pin) {
                let since = now_ms.saturating_sub(last);
                if since < min_interval {
                    return Err(SafetyViolation::RateLimited {
                        min_interval_ms: min_interval,
                        since_last_ms: since,
                    });
                }
            }
            self.last_fire.insert(pin, now_ms);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINS: &[i32] = &[3, 14, 26, 33, 46];

    #[test]
    fn default_policy_matches_the_old_constants() {
        let mut g = SafetyGate::with_output_pins(PINS);
        // allow-listed pin, valid level → ok
        assert!(g.check(14, 1, 1_000).is_ok());
        // pin not in the boot output set → denied
        assert!(matches!(
            g.check(99, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { pin: 99 })
        ));
        // value above the digital max → denied
        assert!(matches!(
            g.check(14, 5, 1_100),
            Err(SafetyViolation::ValueOutOfRange { value: 5, .. })
        ));
    }

    #[test]
    fn host_can_push_a_stricter_policy() {
        let mut g = SafetyGate::with_output_pins(PINS);
        // Host tightens to a single pin with a rate limit.
        let applied = g.apply_pushed(
            vec![SafetyLimit {
                node_id: "obc-esp32-s3-001".into(),
                tool: "gpio_write".into(),
                allowed_pins: Some(vec![26]),
                value_min: Some(0),
                value_max: Some(1),
                min_interval_ms: Some(500),
            }],
            "obc-esp32-s3-001",
        );
        assert!(applied);
        // Pin 14 was allowed by default but is now outside the pushed allow-list.
        assert!(matches!(
            g.check(14, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { .. })
        ));
        assert!(g.check(26, 1, 1_000).is_ok());
    }

    #[test]
    fn rate_limit_blocks_then_allows_after_interval() {
        let mut g = SafetyGate::with_output_pins(PINS);
        g.apply_pushed(
            vec![SafetyLimit {
                node_id: String::new(),
                tool: "gpio_write".into(),
                allowed_pins: Some(vec![26]),
                value_min: Some(0),
                value_max: Some(1),
                min_interval_ms: Some(500),
            }],
            "any",
        );
        assert!(g.check(26, 1, 1_000).is_ok());
        assert!(matches!(
            g.check(26, 0, 1_200),
            Err(SafetyViolation::RateLimited { .. })
        ));
        assert!(g.check(26, 0, 1_600).is_ok()); // interval elapsed
    }

    #[test]
    fn a_push_without_gpio_write_leaves_the_gate_intact() {
        let mut g = SafetyGate::with_output_pins(PINS);
        // Host pushes an unrelated tool limit → gpio_write policy unchanged.
        let applied = g.apply_pushed(
            vec![SafetyLimit {
                node_id: String::new(),
                tool: "servo_write".into(),
                allowed_pins: Some(vec![1]),
                value_min: None,
                value_max: None,
                min_interval_ms: None,
            }],
            "n",
        );
        assert!(!applied);
        assert!(g.check(14, 1, 1_000).is_ok()); // still governed by the default
        assert!(matches!(
            g.check(99, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { .. })
        ));
    }

    #[test]
    fn an_empty_allow_list_denies_every_pin() {
        let mut g = SafetyGate::with_output_pins(PINS);
        g.apply_pushed(
            vec![SafetyLimit {
                node_id: String::new(),
                tool: "gpio_write".into(),
                allowed_pins: Some(vec![]),
                value_min: None,
                value_max: None,
                min_interval_ms: None,
            }],
            "n",
        );
        assert!(matches!(
            g.check(14, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { .. })
        ));
    }

    #[test]
    fn wire_format_matches_the_host_limit_json() {
        // A limit authored host-side (snake_case fields) deserializes here.
        let l: SafetyLimit = serde_json::from_str(
            r#"{"node_id":"obc-esp32-s3-001","tool":"gpio_write",
                "allowed_pins":[26,33],"value_min":0,"value_max":1,"min_interval_ms":250}"#,
        )
        .unwrap();
        assert_eq!(l.allowed_pins, Some(vec![26, 33]));
        assert_eq!(l.min_interval_ms, Some(250));
    }
}

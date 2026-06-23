//! Deterministic, model-independent safety limits for physical actions (Track 0).
//!
//! The agent's LLM decides *what* to do; this gate decides whether a physical
//! action is *allowed* — using fixed rules the model cannot influence. It is the
//! host-side mirror of the on-MCU `SafetyGate` in the ESP32-S3 firmware: the
//! same limit table is enforced in both places so a compromised host, a
//! poisoned skill, or a hallucinated tool call still cannot drive an actuator
//! out of bounds.
//!
//! Limits are intentionally simple and total: an allow-list of pins, a value
//! range, and a minimum interval (rate limit) per `(node, tool)`. Anything not
//! covered by a rule is governed by the approval layer, not this gate.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// A single deterministic limit for a `(node, tool)` pair.
///
/// Loadable from the `[[safety.limit]]` config section and mirrored to the node
/// firmware over the spine (`obc/nodes/{id}/limits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyLimit {
    /// Node this limit applies to (e.g. `"obc-esp32-s3-001"`).
    pub node_id: String,
    /// Tool this limit governs (e.g. `"gpio_write"`).
    pub tool: String,
    /// Allowed pins; `None` means "any pin", empty means "no pins" (default-deny).
    #[serde(default)]
    pub allowed_pins: Option<Vec<i64>>,
    /// Inclusive minimum value (e.g. GPIO level). `None` = unbounded below.
    #[serde(default)]
    pub value_min: Option<i64>,
    /// Inclusive maximum value. `None` = unbounded above.
    #[serde(default)]
    pub value_max: Option<i64>,
    /// Minimum milliseconds between fires (rate limit). `None` = no rate limit.
    #[serde(default)]
    pub min_interval_ms: Option<u64>,
}

impl SafetyLimit {
    /// A permissive limit for a node/tool (used as a builder base in tests).
    pub fn new(node_id: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            tool: tool.into(),
            allowed_pins: None,
            value_min: None,
            value_max: None,
            min_interval_ms: None,
        }
    }
}

/// Track 0 safety configuration (root `[safety]` config section).
///
/// When `enabled`, the agent attaches a [`SafetyGate`] built from `limits` plus
/// an action auditor at `audit_log_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SafetyConfig {
    /// Enable the deterministic safety gate + tamper-evident action audit.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the action audit log (JSONL). Defaults to the data dir's
    /// `action_audit.jsonl` when unset.
    #[serde(default)]
    pub audit_log_path: Option<String>,
    /// Key for the audit MAC. Falls back to the pairing secret, then a dev key.
    #[serde(default)]
    pub audit_key: Option<String>,
    /// Deterministic per-`(node, tool)` limits.
    #[serde(default)]
    pub limits: Vec<SafetyLimit>,
}

/// Why a physical action was refused by the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyViolation {
    /// The pin is not in the rule's allow-list.
    PinNotAllowed { pin: i64 },
    /// The value is outside the permitted range.
    ValueOutOfRange { value: i64, min: Option<i64>, max: Option<i64> },
    /// The action fired again before the minimum interval elapsed.
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
                write!(
                    f,
                    "safety: rate limit ({since_last_ms}ms since last, min {min_interval_ms}ms)"
                )
            }
        }
    }
}

impl std::error::Error for SafetyViolation {}

/// Enforces [`SafetyLimit`]s. Cheap to clone the config in; rate state is held
/// behind a mutex so `check` works from the shared (`&self`) agent context.
#[derive(Debug, Default)]
pub struct SafetyGate {
    limits: Vec<SafetyLimit>,
    /// (node, tool, pin) -> last fire time (ms). Poisoned-lock tolerant.
    last_fire: Mutex<HashMap<(String, String, i64), u64>>,
}

impl SafetyGate {
    /// Build a gate from a set of limits.
    pub fn new(limits: Vec<SafetyLimit>) -> Self {
        Self {
            limits,
            last_fire: Mutex::new(HashMap::new()),
        }
    }

    /// Find the limit governing this `(node, tool)`, if any.
    fn rule_for(&self, node_id: &str, tool: &str) -> Option<&SafetyLimit> {
        self.limits
            .iter()
            .find(|l| l.node_id == node_id && l.tool == tool)
    }

    /// Check a physical action against the configured limits.
    ///
    /// Returns `Ok(())` if allowed (and records the fire time for rate
    /// limiting), or the first [`SafetyViolation`] otherwise. If no rule covers
    /// `(node, tool)`, the action is allowed here and left to the approval layer.
    pub fn check(
        &self,
        node_id: &str,
        tool: &str,
        pin: i64,
        value: i64,
        now_ms: u64,
    ) -> Result<(), SafetyViolation> {
        let Some(rule) = self.rule_for(node_id, tool) else {
            return Ok(());
        };

        // 1) Pin allow-list (default-deny when a list is present).
        if let Some(allowed) = &rule.allowed_pins {
            if !allowed.contains(&pin) {
                return Err(SafetyViolation::PinNotAllowed { pin });
            }
        }

        // 2) Value range.
        if rule.value_min.is_some_and(|min| value < min)
            || rule.value_max.is_some_and(|max| value > max)
        {
            return Err(SafetyViolation::ValueOutOfRange {
                value,
                min: rule.value_min,
                max: rule.value_max,
            });
        }

        // 3) Rate limit (per node+tool+pin).
        if let Some(min_interval) = rule.min_interval_ms {
            let key = (node_id.to_string(), tool.to_string(), pin);
            let mut guard = self
                .last_fire
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(&last) = guard.get(&key) {
                let since = now_ms.saturating_sub(last);
                if since < min_interval {
                    return Err(SafetyViolation::RateLimited {
                        min_interval_ms: min_interval,
                        since_last_ms: since,
                    });
                }
            }
            guard.insert(key, now_ms);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> SafetyGate {
        SafetyGate::new(vec![SafetyLimit {
            node_id: "node-1".into(),
            tool: "gpio_write".into(),
            allowed_pins: Some(vec![17, 18]),
            value_min: Some(0),
            value_max: Some(1),
            min_interval_ms: Some(500),
        }])
    }

    #[test]
    fn allows_in_policy_action() {
        let g = gate();
        assert!(g.check("node-1", "gpio_write", 17, 1, 1_000).is_ok());
    }

    #[test]
    fn denies_pin_not_in_allow_list() {
        let g = gate();
        assert_eq!(
            g.check("node-1", "gpio_write", 99, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { pin: 99 })
        );
    }

    #[test]
    fn denies_value_out_of_range() {
        let g = gate();
        match g.check("node-1", "gpio_write", 17, 5, 1_000) {
            Err(SafetyViolation::ValueOutOfRange { value, .. }) => assert_eq!(value, 5),
            other => panic!("expected ValueOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn enforces_rate_limit_then_allows_after_interval() {
        let g = gate();
        assert!(g.check("node-1", "gpio_write", 17, 1, 1_000).is_ok());
        // too soon
        assert!(matches!(
            g.check("node-1", "gpio_write", 17, 0, 1_200),
            Err(SafetyViolation::RateLimited { .. })
        ));
        // after the interval
        assert!(g.check("node-1", "gpio_write", 17, 0, 1_600).is_ok());
    }

    #[test]
    fn unmatched_node_or_tool_is_allowed_here() {
        // No rule for this pair → gate defers to the approval layer.
        let g = gate();
        assert!(g.check("other-node", "gpio_write", 99, 9, 1_000).is_ok());
        assert!(g.check("node-1", "some_other_tool", 99, 9, 1_000).is_ok());
    }

    #[test]
    fn empty_allow_list_denies_all_pins() {
        let g = SafetyGate::new(vec![SafetyLimit {
            allowed_pins: Some(vec![]),
            ..SafetyLimit::new("node-1", "gpio_write")
        }]);
        assert!(matches!(
            g.check("node-1", "gpio_write", 17, 1, 1_000),
            Err(SafetyViolation::PinNotAllowed { .. })
        ));
    }
}

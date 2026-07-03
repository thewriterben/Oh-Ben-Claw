//! On-MCU safing mirror (Phase 18 — System 1 autonomy at the edge).
//!
//! The host's power suite derives a `power.mode` from battery telemetry and the
//! safing reflex rules react to it. But a node must protect *itself* when the
//! host/spine is unreachable — it cannot wait for a pushed rule. This module
//! bakes that in: the node reads its own battery SoC (`sensor.battery_soc`,
//! numeric, from a local fuel gauge) and **built-in** safing rules cut power-
//! hungry loads when charge goes critical, with no host in the loop.
//!
//! These are ordinary firmware [`reflex::ReflexRule`]s expressed numerically
//! (the node measures SoC as a number, so it needs no categorical `State`
//! matching), so they run on the existing on-MCU reflex engine and every
//! `GpioWrite` stays bounded by the Track 0 safety gate. Host-pushed rules are
//! merged *after* the defaults ([`with_defaults`]) so a node never loses
//! self-protection when the host overrides its rule set.
//!
//! Pure (`std` + `serde`), so it unit-tests on the host like `reflex`.

use crate::reflex::{Action, Cmp, Condition, ReflexRule};
use serde::{Deserialize, Serialize};

/// SoC (%) at/below which (and not charging) the node sheds load.
pub const DEFAULT_LOW_PCT: f64 = 20.0;
/// SoC (%) at/below which (and not charging) the node safes itself.
pub const DEFAULT_CRITICAL_PCT: f64 = 10.0;
/// Actuator-enable GPIO driven low to cut power-hungry loads on critical battery.
/// Bounded by the on-MCU Track 0 gate — refused (and logged) if not allow-listed,
/// so expressing the intent here is safe even on a node without that pin wired.
pub const DEFAULT_SAFE_PIN: i64 = 21;

/// Local battery-SoC entity the built-in rules watch (host world-memory naming).
pub const BATTERY_SOC_ENTITY: &str = "sensor.battery_soc";

/// Time (ms) of host silence past which the node considers the link offline.
pub const DEFAULT_LINK_TIMEOUT_MS: u64 = 30_000;
/// Entity carrying ms since last host contact (fed by the main loop watchdog).
pub const LINK_SILENCE_ENTITY: &str = "sensor.link_silence_ms";

/// Ambient/enclosure temperature entity (°C), fed by an environmental sensor
/// (e.g. the DHT22) into the reflex snapshot.
pub const TEMPERATURE_ENTITY: &str = "sensor.temperature";
/// Relative-humidity entity (%RH), same source.
pub const HUMIDITY_ENTITY: &str = "sensor.humidity";

/// Temperature (°C) at/above which the node sheds heat-producing loads (cuts the
/// actuator-enable pin) — hardware-protective over-temperature.
pub const DEFAULT_OVERTEMP_CRITICAL_C: f64 = 75.0;
/// Temperature (°C) at/above which the node escalates an over-temperature warning
/// (advise shed-load / more cooling before it becomes critical).
pub const DEFAULT_OVERTEMP_WARN_C: f64 = 60.0;
/// Relative humidity (%RH) at/above which condensation risk on the electronics is
/// escalated upward.
pub const DEFAULT_HUMIDITY_HIGH_PCT: f64 = 90.0;

/// Whether the host link is offline given the current silence vs. the timeout.
pub fn link_offline(silence_ms: f64, timeout_ms: f64) -> bool {
    silence_ms >= timeout_ms
}

/// Derived operating mode (mirror of the host `power::PowerMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerMode {
    Normal,
    Low,
    Critical,
    Charging,
}

impl PowerMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PowerMode::Normal => "normal",
            PowerMode::Low => "low",
            PowerMode::Critical => "critical",
            PowerMode::Charging => "charging",
        }
    }
}

/// Derive the power mode from a SoC reading. Charging takes precedence; otherwise
/// the SoC is bucketed critical → low → normal (mirror of the host derivation).
pub fn derive(soc_pct: f64, charging: bool, low_pct: f64, critical_pct: f64) -> PowerMode {
    if charging {
        PowerMode::Charging
    } else if soc_pct <= critical_pct {
        PowerMode::Critical
    } else if soc_pct <= low_pct {
        PowerMode::Low
    } else {
        PowerMode::Normal
    }
}

/// The built-in on-MCU safing rules. A node loaded with only these still protects
/// itself: critical battery cuts the actuator-enable pin; low battery escalates a
/// shed-load advisory upward.
pub fn default_safing_rules() -> Vec<ReflexRule> {
    vec![
        ReflexRule {
            id: "safe-battery-critical".to_string(),
            when: Condition::Sensor {
                entity: BATTERY_SOC_ENTITY.to_string(),
                op: Cmp::Le,
                value: DEFAULT_CRITICAL_PCT,
            },
            then: Action::GpioWrite {
                node_id: "self".to_string(),
                pin: DEFAULT_SAFE_PIN,
                value: 0,
            },
            debounce_ms: 5_000,
            max_rate_hz: None,
        },
        ReflexRule {
            id: "safe-battery-low".to_string(),
            when: Condition::Sensor {
                entity: BATTERY_SOC_ENTITY.to_string(),
                op: Cmp::Le,
                value: DEFAULT_LOW_PCT,
            },
            then: Action::Escalate {
                reason: "battery low — shed load".to_string(),
            },
            debounce_ms: 5_000,
            max_rate_hz: None,
        },
        ReflexRule {
            id: "safe-link-offline".to_string(),
            when: Condition::Sensor {
                entity: LINK_SILENCE_ENTITY.to_string(),
                op: Cmp::Ge,
                value: DEFAULT_LINK_TIMEOUT_MS as f64,
            },
            then: Action::Escalate {
                reason: "host link lost — entering offline safing".to_string(),
            },
            debounce_ms: 10_000,
            max_rate_hz: None,
        },
        // Over-temperature critical: shed heat-producing loads by cutting the
        // actuator-enable pin (same protective action as critical battery).
        ReflexRule {
            id: "safe-overtemp-critical".to_string(),
            when: Condition::Sensor {
                entity: TEMPERATURE_ENTITY.to_string(),
                op: Cmp::Ge,
                value: DEFAULT_OVERTEMP_CRITICAL_C,
            },
            then: Action::GpioWrite {
                node_id: "self".to_string(),
                pin: DEFAULT_SAFE_PIN,
                value: 0,
            },
            debounce_ms: 5_000,
            max_rate_hz: None,
        },
        // Over-temperature warning: escalate a shed-load / cooling advisory before
        // it reaches the critical cut-off.
        ReflexRule {
            id: "safe-overtemp-warn".to_string(),
            when: Condition::Sensor {
                entity: TEMPERATURE_ENTITY.to_string(),
                op: Cmp::Ge,
                value: DEFAULT_OVERTEMP_WARN_C,
            },
            then: Action::Escalate {
                reason: "over-temperature — shed load / increase cooling".to_string(),
            },
            debounce_ms: 5_000,
            max_rate_hz: None,
        },
        // High humidity: condensation risk on the electronics — escalate upward.
        ReflexRule {
            id: "safe-humidity-high".to_string(),
            when: Condition::Sensor {
                entity: HUMIDITY_ENTITY.to_string(),
                op: Cmp::Ge,
                value: DEFAULT_HUMIDITY_HIGH_PCT,
            },
            then: Action::Escalate {
                reason: "high humidity — condensation risk".to_string(),
            },
            debounce_ms: 10_000,
            max_rate_hz: None,
        },
    ]
}

/// Host-pushed rules merged *after* the built-in safing rules, so the node keeps
/// self-protection even when the host replaces its rule set.
pub fn with_defaults(host_rules: Vec<ReflexRule>) -> Vec<ReflexRule> {
    let mut rules = default_safing_rules();
    rules.extend(host_rules);
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflex::ReflexEngine;
    use std::collections::HashMap;

    fn snap(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn derive_buckets_and_charging_precedence() {
        assert_eq!(derive(80.0, false, 20.0, 10.0), PowerMode::Normal);
        assert_eq!(derive(15.0, false, 20.0, 10.0), PowerMode::Low);
        assert_eq!(derive(8.0, false, 20.0, 10.0), PowerMode::Critical);
        assert_eq!(derive(5.0, true, 20.0, 10.0), PowerMode::Charging);
    }

    #[test]
    fn critical_battery_cuts_safe_pin() {
        let mut eng = ReflexEngine::new(default_safing_rules());
        let fired = eng.evaluate(&snap(&[(BATTERY_SOC_ENTITY, 6.0)]), 1_000);
        // both critical (gpio cut) and low (escalate) fire below the critical line
        assert!(fired.iter().any(|f| f.rule_id == "safe-battery-critical"
            && matches!(f.action, Action::GpioWrite { pin: DEFAULT_SAFE_PIN, value: 0, .. })));
        assert!(fired.iter().any(|f| f.rule_id == "safe-battery-low"));
    }

    #[test]
    fn healthy_battery_fires_nothing() {
        let mut eng = ReflexEngine::new(default_safing_rules());
        // healthy battery + a fresh link (no silence entity) → nothing fires
        assert!(eng.evaluate(&snap(&[(BATTERY_SOC_ENTITY, 85.0)]), 1_000).is_empty());
    }

    #[test]
    fn link_silence_fires_offline_safing() {
        assert!(!link_offline(5_000.0, DEFAULT_LINK_TIMEOUT_MS as f64));
        assert!(link_offline(40_000.0, DEFAULT_LINK_TIMEOUT_MS as f64));
        let mut eng = ReflexEngine::new(default_safing_rules());
        // host silent past the timeout → offline safing escalates
        let fired = eng.evaluate(&snap(&[(LINK_SILENCE_ENTITY, 35_000.0)]), 1_000);
        assert!(fired.iter().any(|f| f.rule_id == "safe-link-offline"));
        // a fresh link does not
        let mut eng2 = ReflexEngine::new(default_safing_rules());
        assert!(eng2
            .evaluate(&snap(&[(LINK_SILENCE_ENTITY, 1_000.0)]), 1_000)
            .iter()
            .all(|f| f.rule_id != "safe-link-offline"));
    }

    #[test]
    fn missing_battery_entity_does_not_fire() {
        // No fuel gauge → no battery_soc in the snapshot → safing stays dormant.
        let mut eng = ReflexEngine::new(default_safing_rules());
        assert!(eng.evaluate(&snap(&[("sensor.temperature", 22.0)]), 1_000).is_empty());
    }

    #[test]
    fn overtemp_and_humidity_safing() {
        // Critical over-temp fires both the cut (gpio) and the warn escalate.
        let mut eng = ReflexEngine::new(default_safing_rules());
        let fired = eng.evaluate(&snap(&[(TEMPERATURE_ENTITY, 78.0)]), 1_000);
        assert!(fired.iter().any(|f| f.rule_id == "safe-overtemp-critical"
            && matches!(f.action, Action::GpioWrite { pin: DEFAULT_SAFE_PIN, value: 0, .. })));
        assert!(fired.iter().any(|f| f.rule_id == "safe-overtemp-warn"));

        // High humidity escalates a condensation warning.
        let mut eng2 = ReflexEngine::new(default_safing_rules());
        let fired2 = eng2.evaluate(&snap(&[(HUMIDITY_ENTITY, 95.0)]), 1_000);
        assert!(fired2.iter().any(|f| f.rule_id == "safe-humidity-high"));

        // Comfortable room conditions fire nothing.
        let mut eng3 = ReflexEngine::new(default_safing_rules());
        assert!(eng3
            .evaluate(
                &snap(&[(TEMPERATURE_ENTITY, 22.0), (HUMIDITY_ENTITY, 45.0)]),
                1_000,
            )
            .is_empty());
    }

    #[test]
    fn with_defaults_preserves_self_protection() {
        let host = vec![ReflexRule {
            id: "host-rule".to_string(),
            when: Condition::Sensor { entity: "sensor.temperature".into(), op: Cmp::Gt, value: 60.0 },
            then: Action::Escalate { reason: "hot".into() },
            debounce_ms: 0,
            max_rate_hz: None,
        }];
        let merged = with_defaults(host);
        let ids: Vec<&str> = merged.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"safe-battery-critical"));
        assert!(ids.contains(&"safe-battery-low"));
        assert!(ids.contains(&"host-rule"));
    }
}

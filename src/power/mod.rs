//! Power subsystem — battery + charging telemetry into world memory, with a
//! derived **power mode** that reflexes can use to safe the system.
//!
//! Like sensing, power *perceives*: a [`BatteryReading`] (state of charge,
//! voltage, current, charge state) is recorded as a `power.battery` fact. The
//! value sensing-style quality adds here is a domain-meaningful verdict — a
//! [`PowerMode`] (`normal` / `low` / `critical` / `charging`) derived from the
//! SoC and charge state against configured thresholds, recorded as its own
//! `power.mode` fact. That dedicated entity is the System 1 hook: a reflex rule
//! can watch `power.mode` and, on `critical`, escalate or drive low-power safing
//! (stop motors, dim, sleep) — exactly the perceive→remember→reflex→act spine
//! the other suites share.

use crate::memory::world::WorldMemory;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Whether the pack is taking on or giving up charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

impl ChargeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChargeState::Charging => "charging",
            ChargeState::Discharging => "discharging",
            ChargeState::Full => "full",
            ChargeState::Unknown => "unknown",
        }
    }
}

impl Default for ChargeState {
    fn default() -> Self {
        ChargeState::Unknown
    }
}

/// A single battery telemetry reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatteryReading {
    /// State of charge, percent `0.0..=100.0`.
    pub soc_pct: f64,
    /// Pack voltage (V), if known.
    #[serde(default)]
    pub voltage: Option<f64>,
    /// Pack current (A); positive = charging, negative = draw. Optional.
    #[serde(default)]
    pub current_a: Option<f64>,
    /// Charge state.
    #[serde(default)]
    pub charging: ChargeState,
    /// Who reported it (BMS / node).
    #[serde(default)]
    pub source: Option<String>,
}

/// Derived operating mode used for low-power safing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerMode {
    /// Healthy charge, on battery.
    Normal,
    /// Below the low threshold — shed non-essential load.
    Low,
    /// At/below the critical threshold — safe the system.
    Critical,
    /// Actively charging (recovering).
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

/// SoC thresholds (percent) that bound the discharging modes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerThresholds {
    /// At/below this SoC (and not charging) ⇒ [`PowerMode::Low`].
    pub low_pct: f64,
    /// At/below this SoC (and not charging) ⇒ [`PowerMode::Critical`].
    pub critical_pct: f64,
}

impl Default for PowerThresholds {
    fn default() -> Self {
        Self {
            low_pct: 20.0,
            critical_pct: 10.0,
        }
    }
}

impl PowerThresholds {
    /// Derive the operating mode. Active charging takes precedence (the pack is
    /// recovering); otherwise the SoC is bucketed critical → low → normal.
    pub fn derive(&self, soc_pct: f64, charging: ChargeState) -> PowerMode {
        match charging {
            ChargeState::Charging => PowerMode::Charging,
            _ => {
                if soc_pct <= self.critical_pct {
                    PowerMode::Critical
                } else if soc_pct <= self.low_pct {
                    PowerMode::Low
                } else {
                    PowerMode::Normal
                }
            }
        }
    }
}

/// A reading after mode derivation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PowerStatus {
    pub soc_pct: f64,
    pub charging: ChargeState,
    pub mode: PowerMode,
    pub at_ms: u64,
}

/// Ingests battery telemetry, derives the power mode, and records both into world
/// memory (`power.battery` + `power.mode`).
pub struct PowerController {
    world: Option<Arc<WorldMemory>>,
    thresholds: PowerThresholds,
    source: String,
}

impl PowerController {
    /// Build a controller with the given thresholds.
    pub fn new(thresholds: PowerThresholds) -> Self {
        Self {
            world: None,
            thresholds,
            source: "power".to_string(),
        }
    }

    /// Record telemetry into world memory (enables §3 Remember + reflex safing).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Override the world-memory `source` label (default `"power"`).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// The configured thresholds.
    pub fn thresholds(&self) -> PowerThresholds {
        self.thresholds
    }

    /// Ingest a reading: derive the mode, record `power.battery` (full reading +
    /// mode) and `power.mode` (the derived mode + SoC, for reflex matching).
    pub fn ingest(&self, reading: &BatteryReading, now_ms: u64) -> anyhow::Result<PowerStatus> {
        let mode = self.thresholds.derive(reading.soc_pct, reading.charging);
        if let Some(world) = &self.world {
            let battery = json!({
                "soc_pct": reading.soc_pct,
                "voltage": reading.voltage,
                "current_a": reading.current_a,
                "charging": reading.charging.as_str(),
                "mode": mode.as_str(),
                "source": reading.source,
            });
            world.observe("power.battery", battery, now_ms, now_ms, &self.source)?;
            let mode_fact = json!({ "mode": mode.as_str(), "soc_pct": reading.soc_pct });
            world.observe("power.mode", mode_fact, now_ms, now_ms, &self.source)?;
        }
        Ok(PowerStatus {
            soc_pct: reading.soc_pct,
            charging: reading.charging,
            mode,
            at_ms: now_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> (PowerController, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = PowerController::new(PowerThresholds { low_pct: 20.0, critical_pct: 10.0 })
            .with_world_memory(Arc::clone(&world));
        (ctrl, world)
    }

    fn reading(soc: f64, charging: ChargeState) -> BatteryReading {
        BatteryReading {
            soc_pct: soc,
            voltage: Some(11.4),
            current_a: None,
            charging,
            source: Some("bms".to_string()),
        }
    }

    #[test]
    fn mode_derivation_buckets_by_soc() {
        let t = PowerThresholds { low_pct: 20.0, critical_pct: 10.0 };
        assert_eq!(t.derive(80.0, ChargeState::Discharging), PowerMode::Normal);
        assert_eq!(t.derive(15.0, ChargeState::Discharging), PowerMode::Low);
        assert_eq!(t.derive(8.0, ChargeState::Discharging), PowerMode::Critical);
        assert_eq!(t.derive(10.0, ChargeState::Discharging), PowerMode::Critical); // inclusive
    }

    #[test]
    fn charging_takes_precedence_over_low_soc() {
        let t = PowerThresholds::default();
        assert_eq!(t.derive(5.0, ChargeState::Charging), PowerMode::Charging);
    }

    #[test]
    fn ingest_records_battery_and_mode_facts() {
        let (ctrl, world) = controller();
        let status = ctrl.ingest(&reading(8.0, ChargeState::Discharging), 1_000).unwrap();
        assert_eq!(status.mode, PowerMode::Critical);

        let battery = world.current("power.battery").unwrap().unwrap();
        assert!((battery.value["soc_pct"].as_f64().unwrap() - 8.0).abs() < 1e-9);
        assert_eq!(battery.value["charging"], "discharging");
        assert_eq!(battery.value["mode"], "critical");

        let mode = world.current("power.mode").unwrap().unwrap();
        assert_eq!(mode.value["mode"], "critical");
        assert_eq!(mode.source, "power");
    }

    #[test]
    fn full_pack_is_normal_not_charging() {
        let (ctrl, _world) = controller();
        let status = ctrl.ingest(&reading(100.0, ChargeState::Full), 1_000).unwrap();
        assert_eq!(status.mode, PowerMode::Normal);
    }

    #[test]
    fn reading_roundtrips() {
        let r = reading(50.0, ChargeState::Discharging);
        let back: BatteryReading = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn charge_state_defaults_to_unknown() {
        let r: BatteryReading = serde_json::from_str(r#"{"soc_pct":50.0}"#).unwrap();
        assert_eq!(r.charging, ChargeState::Unknown);
    }
}

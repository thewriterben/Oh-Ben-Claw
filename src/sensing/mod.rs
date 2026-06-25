//! Sensing subsystem — quality-aware ingestion of sensor streams into world memory.
//!
//! The third Subsystem Suite alongside vision and movement. Where vision
//! *perceives* subjects and movement *acts*, sensing ingests named scalar
//! streams (temperature, humidity, battery, …), classifies each reading's
//! **quality** against configured expectations (value range + freshness), and
//! records it into bitemporal [`WorldMemory`] as a `sensor.{quantity}` fact
//! carrying value, unit, source, and quality.
//!
//! The quality flag is the §4 *Learn* hook: out-of-range or stale readings are
//! surfaced ([`SensingController::anomalies`]) so reflexes/agents can distrust
//! them and a human can recalibrate. Reflexes already read `sensor.{quantity}`
//! from world memory, so sensing feeds System 1 directly.

use crate::memory::world::WorldMemory;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A single scalar reading from a sensor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Physical quantity / stream name (e.g. `"temperature"`).
    pub quantity: String,
    /// Numeric value.
    pub value: f64,
    /// Unit of measure (e.g. `"C"`, `"%"`, `"hPa"`).
    #[serde(default)]
    pub unit: Option<String>,
    /// Sensor that produced it (e.g. `"bme280"`).
    #[serde(default)]
    pub source: Option<String>,
}

/// Quality classification of a reading / stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// Within the expected range and fresh.
    Ok,
    /// Outside the configured `[min, max]`.
    OutOfRange,
    /// No fresh reading within the configured staleness window (or never seen).
    Stale,
}

impl Quality {
    /// Stable lowercase token (used in world-memory fact values).
    pub fn as_str(&self) -> &'static str {
        match self {
            Quality::Ok => "ok",
            Quality::OutOfRange => "out_of_range",
            Quality::Stale => "stale",
        }
    }
}

/// Expected bounds + freshness for a quantity (drives quality classification).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuantitySpec {
    /// Inclusive minimum acceptable value.
    #[serde(default)]
    pub min: Option<f64>,
    /// Inclusive maximum acceptable value.
    #[serde(default)]
    pub max: Option<f64>,
    /// Max ms between readings before the stream is considered [`Quality::Stale`].
    #[serde(default)]
    pub max_staleness_ms: Option<u64>,
    /// Canonical unit; used when a sample omits its own.
    #[serde(default)]
    pub unit: Option<String>,
}

/// A reading after quality classification.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassifiedReading {
    pub quantity: String,
    pub value: f64,
    pub unit: Option<String>,
    pub source: Option<String>,
    pub quality: Quality,
    pub at_ms: u64,
}

/// Ingests sensor samples, classifies their quality, and records them into world
/// memory. Cheap to share behind an `Arc` — freshness state is held behind a
/// mutex so `ingest`/`status` work from the shared (`&self`) context.
pub struct SensingController {
    specs: HashMap<String, QuantitySpec>,
    world: Option<Arc<WorldMemory>>,
    /// quantity -> (last ingest ms, range quality at that ingest).
    last: Mutex<HashMap<String, (u64, Quality)>>,
    source: String,
}

impl SensingController {
    /// Build a controller from per-quantity specs (quantities without a spec are
    /// accepted and always classed [`Quality::Ok`] on range).
    pub fn new(specs: Vec<(String, QuantitySpec)>) -> Self {
        Self {
            specs: specs.into_iter().collect(),
            world: None,
            last: Mutex::new(HashMap::new()),
            source: "sensing".to_string(),
        }
    }

    /// Record ingested readings into world memory (enables §3 Remember).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Override the world-memory `source` label (default `"sensing"`).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Number of configured quantity specs.
    pub fn spec_count(&self) -> usize {
        self.specs.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u64, Quality)>> {
        self.last.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Range-only classification (staleness is time-based; see [`status`](Self::status)).
    fn classify_range(&self, quantity: &str, value: f64) -> Quality {
        if let Some(spec) = self.specs.get(quantity) {
            if spec.min.is_some_and(|m| value < m) || spec.max.is_some_and(|m| value > m) {
                return Quality::OutOfRange;
            }
        }
        Quality::Ok
    }

    /// Ingest a sample: classify range, record to world memory as a
    /// `sensor.{quantity}` fact, and update freshness. Returns the classified
    /// reading.
    pub fn ingest(&self, sample: &Sample, now_ms: u64) -> anyhow::Result<ClassifiedReading> {
        let quality = self.classify_range(&sample.quantity, sample.value);
        let unit = sample
            .unit
            .clone()
            .or_else(|| self.specs.get(&sample.quantity).and_then(|s| s.unit.clone()));

        if let Some(world) = &self.world {
            let entity = format!("sensor.{}", sample.quantity);
            let value = json!({
                "value": sample.value,
                "unit": unit,
                "source": sample.source,
                "quality": quality.as_str(),
            });
            world.observe(&entity, value, now_ms, now_ms, &self.source)?;
        }

        self.lock().insert(sample.quantity.clone(), (now_ms, quality));

        Ok(ClassifiedReading {
            quantity: sample.quantity.clone(),
            value: sample.value,
            unit,
            source: sample.source.clone(),
            quality,
            at_ms: now_ms,
        })
    }

    /// Current quality of a quantity at `now_ms`: [`Quality::Stale`] if never seen
    /// or past its freshness window, otherwise the range quality of the last
    /// reading.
    pub fn status(&self, quantity: &str, now_ms: u64) -> Quality {
        let Some((last_ms, range_quality)) = self.lock().get(quantity).copied() else {
            return Quality::Stale;
        };
        if let Some(spec) = self.specs.get(quantity) {
            if let Some(max_stale) = spec.max_staleness_ms {
                if now_ms.saturating_sub(last_ms) > max_stale {
                    return Quality::Stale;
                }
            }
        }
        range_quality
    }

    /// All quantities currently anomalous (out-of-range at last ingest, or stale
    /// now). Considers both configured specs and any ingested quantity.
    pub fn anomalies(&self, now_ms: u64) -> Vec<(String, Quality)> {
        let mut names: Vec<String> = self.specs.keys().cloned().collect();
        for k in self.lock().keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
        names.sort();
        names
            .into_iter()
            .filter_map(|q| {
                let s = self.status(&q, now_ms);
                (s != Quality::Ok).then_some((q, s))
            })
            .collect()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(min: f64, max: f64, stale: Option<u64>) -> QuantitySpec {
        QuantitySpec {
            min: Some(min),
            max: Some(max),
            max_staleness_ms: stale,
            unit: Some("C".to_string()),
        }
    }

    fn controller() -> (SensingController, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = SensingController::new(vec![("temperature".to_string(), spec(-40.0, 85.0, Some(10_000)))])
            .with_world_memory(Arc::clone(&world));
        (ctrl, world)
    }

    fn sample(q: &str, v: f64) -> Sample {
        Sample { quantity: q.to_string(), value: v, unit: None, source: Some("bme280".to_string()) }
    }

    #[test]
    fn in_range_reading_is_ok_and_recorded_with_quality() {
        let (ctrl, world) = controller();
        let r = ctrl.ingest(&sample("temperature", 22.5), 1_000).unwrap();
        assert_eq!(r.quality, Quality::Ok);
        assert_eq!(r.unit.as_deref(), Some("C")); // inherited from spec
        let fact = world.current("sensor.temperature").unwrap().unwrap();
        assert!((fact.value["value"].as_f64().unwrap() - 22.5).abs() < 1e-9);
        assert_eq!(fact.value["quality"], "ok");
        assert_eq!(fact.value["unit"], "C");
        assert_eq!(fact.source, "sensing");
    }

    #[test]
    fn out_of_range_reading_is_flagged_but_still_recorded() {
        let (ctrl, world) = controller();
        let r = ctrl.ingest(&sample("temperature", 150.0), 1_000).unwrap();
        assert_eq!(r.quality, Quality::OutOfRange);
        // Field evidence is preserved (recorded), just flagged.
        let fact = world.current("sensor.temperature").unwrap().unwrap();
        assert_eq!(fact.value["quality"], "out_of_range");
        assert_eq!(ctrl.status("temperature", 1_001), Quality::OutOfRange);
    }

    #[test]
    fn staleness_is_time_based() {
        let (ctrl, _world) = controller();
        ctrl.ingest(&sample("temperature", 20.0), 1_000).unwrap();
        assert_eq!(ctrl.status("temperature", 5_000), Quality::Ok); // within 10s
        assert_eq!(ctrl.status("temperature", 20_000), Quality::Stale); // past 10s
    }

    #[test]
    fn never_seen_quantity_is_stale() {
        let (ctrl, _world) = controller();
        assert_eq!(ctrl.status("temperature", 1_000), Quality::Stale);
    }

    #[test]
    fn unspecced_quantity_accepted_as_ok() {
        let (ctrl, world) = controller();
        let r = ctrl.ingest(&sample("lux", 999.0), 1).unwrap();
        assert_eq!(r.quality, Quality::Ok);
        assert!(world.current("sensor.lux").unwrap().is_some());
    }

    #[test]
    fn anomalies_reports_out_of_range_and_stale() {
        let (ctrl, _world) = controller();
        // temperature out of range at ingest; humidity spec exists but never seen.
        let mut ctrl = ctrl;
        ctrl.specs.insert("humidity".to_string(), spec(0.0, 100.0, Some(5_000)));
        ctrl.ingest(&sample("temperature", 200.0), 1_000).unwrap();
        let anomalies = ctrl.anomalies(2_000);
        assert!(anomalies.contains(&("temperature".to_string(), Quality::OutOfRange)));
        assert!(anomalies.contains(&("humidity".to_string(), Quality::Stale)));
    }

    #[test]
    fn sample_roundtrips() {
        let s = sample("temperature", 21.0);
        let back: Sample = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}

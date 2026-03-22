//! Sensor Fusion — combine readings from multiple sensors into a single value.
//!
//! Sensor fusion improves accuracy and reliability by aggregating data from
//! multiple sensors that measure the same physical quantity (e.g. temperature
//! from a BME280 and an SHT31 on the same I2C bus).
//!
//! # Supported Strategies
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | `Average` | Arithmetic mean of all valid readings |
//! | `WeightedAverage` | Weighted mean (higher weights for more accurate sensors) |
//! | `Median` | Median of all valid readings (robust to outliers) |
//! | `Min` | Minimum of all valid readings |
//! | `Max` | Maximum of all valid readings |
//! | `KalmanSimple` | Single-dimensional Kalman filter estimate |
//!
//! # Usage
//!
//! ```rust
//! use oh_ben_claw::peripherals::fusion::{SensorFusion, FusionStrategy, SensorReading};
//!
//! let mut fusion = SensorFusion::new("temperature", FusionStrategy::Average);
//! fusion.add_reading(SensorReading::new("bme280", 23.1));
//! fusion.add_reading(SensorReading::new("sht31", 23.4));
//! let result = fusion.compute().unwrap();
//! assert!((result.value - 23.25).abs() < 0.01);
//! ```

use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// ── Sensor Reading ─────────────────────────────────────────────────────────────

/// A single reading from one sensor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    /// Sensor identifier (e.g. `"bme280"`, `"sht31-0x44"`).
    pub sensor_id: String,
    /// The measured value (numeric).
    pub value: f64,
    /// Optional quality weight in the range `(0, 1]` (default: `1.0`).
    /// Higher values give this sensor more influence in weighted fusion.
    pub weight: f64,
    /// Unix timestamp (seconds) when the reading was taken.
    pub timestamp: u64,
}

impl SensorReading {
    /// Create a new reading with weight `1.0` and the current timestamp.
    pub fn new(sensor_id: impl Into<String>, value: f64) -> Self {
        Self {
            sensor_id: sensor_id.into(),
            value,
            weight: 1.0,
            timestamp: unix_now(),
        }
    }

    /// Attach an explicit weight to this reading.
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight.clamp(f64::EPSILON, 1.0);
        self
    }
}

// ── Fusion Strategy ────────────────────────────────────────────────────────────

/// How multiple sensor readings are combined into a single estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FusionStrategy {
    /// Arithmetic mean.
    #[default]
    Average,
    /// Weighted mean (uses `SensorReading::weight`).
    WeightedAverage,
    /// Middle value (robust to outliers).
    Median,
    /// Minimum of all readings.
    Min,
    /// Maximum of all readings.
    Max,
    /// Simple 1-D Kalman filter.
    KalmanSimple,
}

// ── Fused Result ──────────────────────────────────────────────────────────────

/// The output of a sensor fusion computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusedResult {
    /// Name of the quantity being measured (e.g. `"temperature"`).
    pub quantity: String,
    /// The fused estimate.
    pub value: f64,
    /// Standard deviation of the input readings (population SD).
    pub std_dev: f64,
    /// Strategy used to produce this estimate.
    pub strategy: FusionStrategy,
    /// Number of readings that contributed to this estimate.
    pub reading_count: usize,
    /// IDs of the sensors that contributed.
    pub sensor_ids: Vec<String>,
    /// Unix timestamp (seconds) of the estimate.
    pub timestamp: u64,
}

// ── SensorFusion ──────────────────────────────────────────────────────────────

/// Accumulates readings from multiple sensors and fuses them.
#[derive(Debug, Clone)]
pub struct SensorFusion {
    /// Name of the physical quantity being measured (e.g. `"temperature"`).
    pub quantity: String,
    /// Fusion strategy.
    pub strategy: FusionStrategy,
    /// Accumulated readings.
    readings: Vec<SensorReading>,
    /// Kalman filter state (used only by `KalmanSimple`).
    kalman_state: Option<KalmanState>,
}

#[derive(Debug, Clone)]
struct KalmanState {
    /// Current estimate.
    x: f64,
    /// Estimate uncertainty.
    p: f64,
    /// Process noise.
    q: f64,
    /// Measurement noise.
    r: f64,
}

impl KalmanState {
    fn new(initial: f64) -> Self {
        Self {
            x: initial,
            p: 1.0,
            q: 0.001,
            r: 0.1,
        }
    }

    /// Update the Kalman estimate with a new measurement.
    fn update(&mut self, measurement: f64) {
        // Predict
        self.p += self.q;
        // Update
        let k = self.p / (self.p + self.r);
        self.x += k * (measurement - self.x);
        self.p *= 1.0 - k;
    }
}

impl SensorFusion {
    /// Create a new fusion collector for the given physical quantity.
    pub fn new(quantity: impl Into<String>, strategy: FusionStrategy) -> Self {
        Self {
            quantity: quantity.into(),
            strategy,
            readings: Vec::new(),
            kalman_state: None,
        }
    }

    /// Add a reading to the collection.
    pub fn add_reading(&mut self, reading: SensorReading) {
        if let FusionStrategy::KalmanSimple = &self.strategy {
            match &mut self.kalman_state {
                None => {
                    self.kalman_state = Some(KalmanState::new(reading.value));
                }
                Some(ks) => {
                    ks.update(reading.value);
                }
            }
        }
        self.readings.push(reading);
    }

    /// Clear all accumulated readings (and reset the Kalman state).
    pub fn clear(&mut self) {
        self.readings.clear();
        self.kalman_state = None;
    }

    /// Compute the fused estimate from the current readings.
    ///
    /// Returns `None` if no readings have been added.
    pub fn compute(&self) -> Option<FusedResult> {
        if self.readings.is_empty() {
            return None;
        }

        let values: Vec<f64> = self.readings.iter().map(|r| r.value).collect();
        let sensor_ids: Vec<String> = self.readings.iter().map(|r| r.sensor_id.clone()).collect();
        let reading_count = values.len();

        let fused = match &self.strategy {
            FusionStrategy::Average => arithmetic_mean(&values),
            FusionStrategy::WeightedAverage => {
                let weights: Vec<f64> = self.readings.iter().map(|r| r.weight).collect();
                weighted_mean(&values, &weights)
            }
            FusionStrategy::Median => median(&values),
            FusionStrategy::Min => values.iter().cloned().fold(f64::INFINITY, f64::min),
            FusionStrategy::Max => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            FusionStrategy::KalmanSimple => {
                self.kalman_state.as_ref().map(|ks| ks.x).unwrap_or(0.0)
            }
        };

        let std_dev = population_std_dev(&values, arithmetic_mean(&values));

        Some(FusedResult {
            quantity: self.quantity.clone(),
            value: fused,
            std_dev,
            strategy: self.strategy.clone(),
            reading_count,
            sensor_ids,
            timestamp: unix_now(),
        })
    }
}

// ── Math helpers ───────────────────────────────────────────────────────────────

fn arithmetic_mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn weighted_mean(values: &[f64], weights: &[f64]) -> f64 {
    let total_weight: f64 = weights.iter().sum();
    if total_weight == 0.0 {
        return arithmetic_mean(values);
    }
    values
        .iter()
        .zip(weights.iter())
        .map(|(v, w)| v * w)
        .sum::<f64>()
        / total_weight
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn population_std_dev(values: &[f64], mean: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    variance.sqrt()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── SensorFusionTool ──────────────────────────────────────────────────────────

/// An agent tool that fuses a set of sensor readings provided as JSON.
///
/// The agent (or another tool) collects individual sensor readings and passes
/// them to this tool for fusion, receiving a single aggregated value.
pub struct SensorFusionTool;

#[async_trait]
impl Tool for SensorFusionTool {
    fn name(&self) -> &str {
        "sensor_fusion"
    }

    fn description(&self) -> &str {
        "Combine readings from multiple sensors measuring the same physical quantity \
        into a single reliable estimate. Supports average, weighted_average, median, \
        min, max, and kalman_simple fusion strategies."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "quantity": {
                    "type": "string",
                    "description": "The physical quantity being measured (e.g. 'temperature', 'humidity', 'pressure')."
                },
                "readings": {
                    "type": "array",
                    "description": "Array of sensor readings.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sensor_id": { "type": "string", "description": "Sensor identifier." },
                            "value":     { "type": "number", "description": "Measured value." },
                            "weight":    { "type": "number", "description": "Quality weight (0, 1]. Default: 1.0." }
                        },
                        "required": ["sensor_id", "value"]
                    }
                },
                "strategy": {
                    "type": "string",
                    "enum": ["average", "weighted_average", "median", "min", "max", "kalman_simple"],
                    "description": "Fusion strategy (default: average)."
                }
            },
            "required": ["quantity", "readings"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let quantity = match args.get("quantity").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: quantity")),
        };

        let readings_val = match args.get("readings").and_then(|v| v.as_array()) {
            Some(r) => r,
            None => {
                return Ok(ToolResult::err(
                    "Missing required argument: readings (array)",
                ))
            }
        };

        if readings_val.is_empty() {
            return Ok(ToolResult::err("readings array must not be empty"));
        }

        let strategy_str = args
            .get("strategy")
            .and_then(|v| v.as_str())
            .unwrap_or("average");

        let strategy = match strategy_str {
            "weighted_average" => FusionStrategy::WeightedAverage,
            "median" => FusionStrategy::Median,
            "min" => FusionStrategy::Min,
            "max" => FusionStrategy::Max,
            "kalman_simple" => FusionStrategy::KalmanSimple,
            _ => FusionStrategy::Average,
        };

        let mut fusion = SensorFusion::new(quantity, strategy);

        for rv in readings_val {
            let sensor_id = rv
                .get("sensor_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let value = match rv.get("value").and_then(|v| v.as_f64()) {
                Some(v) => v,
                None => {
                    return Ok(ToolResult::err(
                        "Each reading must have a numeric 'value' field",
                    ))
                }
            };
            let weight = rv.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0);
            fusion.add_reading(SensorReading::new(sensor_id, value).with_weight(weight));
        }

        match fusion.compute() {
            Some(result) => {
                let json_out = serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| format!("{{\"value\": {}}}", result.value));
                Ok(ToolResult::ok(json_out))
            }
            None => Ok(ToolResult::err(
                "No readings provided — cannot compute fusion",
            )),
        }
    }
}

// ── FusionRegistry ─────────────────────────────────────────────────────────────

/// Manages multiple named `SensorFusion` collectors (one per physical quantity).
///
/// Useful when the system tracks several quantities (temperature, humidity,
/// pressure) across multiple sensors simultaneously.
#[derive(Debug, Default)]
pub struct FusionRegistry {
    collectors: HashMap<String, SensorFusion>,
}

impl FusionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a fusion collector for the given quantity.
    pub fn register(&mut self, fusion: SensorFusion) {
        self.collectors.insert(fusion.quantity.clone(), fusion);
    }

    /// Add a reading to the collector for `quantity`.
    ///
    /// Creates a new `Average` collector if none exists for that quantity.
    pub fn add_reading(&mut self, quantity: &str, reading: SensorReading) {
        self.collectors
            .entry(quantity.to_string())
            .or_insert_with(|| SensorFusion::new(quantity, FusionStrategy::Average))
            .add_reading(reading);
    }

    /// Compute fused estimates for all registered quantities.
    pub fn compute_all(&self) -> HashMap<String, FusedResult> {
        self.collectors
            .iter()
            .filter_map(|(q, f)| f.compute().map(|r| (q.clone(), r)))
            .collect()
    }

    /// Clear all readings across all collectors.
    pub fn clear_all(&mut self) {
        for f in self.collectors.values_mut() {
            f.clear();
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_two_readings() {
        let mut fusion = SensorFusion::new("temperature", FusionStrategy::Average);
        fusion.add_reading(SensorReading::new("bme280", 23.0));
        fusion.add_reading(SensorReading::new("sht31", 24.0));
        let result = fusion.compute().unwrap();
        assert!((result.value - 23.5).abs() < 1e-9);
        assert_eq!(result.reading_count, 2);
    }

    #[test]
    fn weighted_average_favours_higher_weight() {
        let mut fusion = SensorFusion::new("temperature", FusionStrategy::WeightedAverage);
        fusion.add_reading(SensorReading::new("cheap_sensor", 20.0).with_weight(0.2));
        fusion.add_reading(SensorReading::new("precise_sensor", 25.0).with_weight(0.8));
        let result = fusion.compute().unwrap();
        // Should be closer to 25.0 than 20.0
        assert!(result.value > 23.0);
        assert!(result.value < 25.0);
    }

    #[test]
    fn median_odd_count() {
        let mut fusion = SensorFusion::new("humidity", FusionStrategy::Median);
        fusion.add_reading(SensorReading::new("s1", 60.0));
        fusion.add_reading(SensorReading::new("s2", 65.0));
        fusion.add_reading(SensorReading::new("s3", 62.0));
        let result = fusion.compute().unwrap();
        assert!((result.value - 62.0).abs() < 1e-9);
    }

    #[test]
    fn median_even_count() {
        let mut fusion = SensorFusion::new("humidity", FusionStrategy::Median);
        fusion.add_reading(SensorReading::new("s1", 60.0));
        fusion.add_reading(SensorReading::new("s2", 64.0));
        let result = fusion.compute().unwrap();
        assert!((result.value - 62.0).abs() < 1e-9);
    }

    #[test]
    fn min_max_strategies() {
        let readings = vec![10.0_f64, 20.0, 30.0];

        let mut min_f = SensorFusion::new("pressure", FusionStrategy::Min);
        let mut max_f = SensorFusion::new("pressure", FusionStrategy::Max);
        for (i, v) in readings.iter().enumerate() {
            let id = format!("s{i}");
            min_f.add_reading(SensorReading::new(&id, *v));
            max_f.add_reading(SensorReading::new(&id, *v));
        }
        assert!((min_f.compute().unwrap().value - 10.0).abs() < 1e-9);
        assert!((max_f.compute().unwrap().value - 30.0).abs() < 1e-9);
    }

    #[test]
    fn kalman_simple_converges() {
        let mut fusion = SensorFusion::new("temperature", FusionStrategy::KalmanSimple);
        // Feed 10 readings at ~20 degrees with noise
        for i in 0..10 {
            let noisy = 20.0 + (i as f64 * 0.1 - 0.5);
            fusion.add_reading(SensorReading::new(format!("s{i}"), noisy));
        }
        let result = fusion.compute().unwrap();
        // Should be within 2 degrees of 20
        assert!((result.value - 20.0).abs() < 2.0);
    }

    #[test]
    fn empty_fusion_returns_none() {
        let fusion = SensorFusion::new("temperature", FusionStrategy::Average);
        assert!(fusion.compute().is_none());
    }

    #[test]
    fn clear_resets_readings() {
        let mut fusion = SensorFusion::new("temperature", FusionStrategy::Average);
        fusion.add_reading(SensorReading::new("s1", 25.0));
        fusion.clear();
        assert!(fusion.compute().is_none());
    }

    #[test]
    fn fusion_registry_aggregates_multiple_quantities() {
        let mut reg = FusionRegistry::new();
        reg.add_reading("temperature", SensorReading::new("bme280", 23.0));
        reg.add_reading("temperature", SensorReading::new("sht31", 24.0));
        reg.add_reading("humidity", SensorReading::new("bme280", 55.0));

        let results = reg.compute_all();
        assert_eq!(results.len(), 2);
        assert!((results["temperature"].value - 23.5).abs() < 1e-9);
        assert!((results["humidity"].value - 55.0).abs() < 1e-9);
    }

    #[test]
    fn std_dev_calculated_correctly() {
        let mut fusion = SensorFusion::new("temperature", FusionStrategy::Average);
        fusion.add_reading(SensorReading::new("s1", 10.0));
        fusion.add_reading(SensorReading::new("s2", 20.0));
        let result = fusion.compute().unwrap();
        // Population std dev of [10, 20] = 5.0
        assert!((result.std_dev - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn fusion_tool_average_strategy() {
        let tool = SensorFusionTool;
        let result = tool
            .execute(json!({
                "quantity": "temperature",
                "readings": [
                    {"sensor_id": "bme280", "value": 23.0},
                    {"sensor_id": "sht31",  "value": 24.0}
                ],
                "strategy": "average"
            }))
            .await
            .unwrap();
        assert!(result.success);
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert!((parsed["value"].as_f64().unwrap() - 23.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn fusion_tool_requires_quantity() {
        let tool = SensorFusionTool;
        let result = tool
            .execute(json!({"readings": [{"sensor_id": "s1", "value": 1.0}]}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("quantity"));
    }

    #[tokio::test]
    async fn fusion_tool_requires_readings() {
        let tool = SensorFusionTool;
        let result = tool
            .execute(json!({"quantity": "temperature"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn fusion_tool_empty_readings_error() {
        let tool = SensorFusionTool;
        let result = tool
            .execute(json!({"quantity": "temperature", "readings": []}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[test]
    fn sensor_reading_weight_clamped() {
        let r = SensorReading::new("s1", 1.0).with_weight(0.0);
        assert!(r.weight > 0.0);
        let r2 = SensorReading::new("s2", 1.0).with_weight(2.0);
        assert!(r2.weight <= 1.0);
    }
}

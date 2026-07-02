//! Foresight — a predictive control layer (Track 1) over bitemporal world memory.
//!
//! Reflexes (System 1) react to the *present*; this layer reacts to the
//! *predicted future*. Because world memory is bitemporal and append-only, every
//! entity carries a time-series — so we can fit its recent trend and forecast
//! **when** a value will cross a threshold. A foresight rule fires *before* the
//! event: `battery predicted ≤ 10% within 60s → return to base now` triggers
//! while the pack is still at 20% but draining fast, buying the time that a
//! purely reactive safing rule cannot.
//!
//! This is the same shape as the reflex layer — rules → [`Action`]s through an
//! [`ActionSink`], with debounce and an escalation budget — but the condition is
//! a forecast, not a snapshot. Forecasts are recorded back into world memory
//! (`foresight.{entity}`) so the prediction itself is observable and auditable.

use crate::agent::reflex::{Action, ActionSink, Cmp, EscalationBudget, FiredReflex};
use crate::memory::world::WorldMemory;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Extract a scalar from a fact value (number, bool, numeric string, `{value}`).
fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse().ok(),
        Value::Object(o) => o.get("value").and_then(value_to_f64),
        _ => None,
    }
}

/// A trend forecast for one entity from its recent world-memory history.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Forecast {
    /// Latest observed value.
    pub current: f64,
    /// Rate of change per millisecond (linear least-squares slope).
    pub rate_per_ms: f64,
    /// Number of points used.
    pub samples: usize,
    /// Timestamp of the latest point (ms).
    pub at_ms: u64,
}

impl Forecast {
    /// Predicted value `horizon_ms` into the future.
    pub fn predict_at(&self, horizon_ms: u64) -> f64 {
        self.current + self.rate_per_ms * horizon_ms as f64
    }

    /// Predicted ms until the value crosses `threshold`, if it is heading there.
    /// `None` if flat, moving away, or already past on the wrong side.
    pub fn time_to_threshold(&self, threshold: f64) -> Option<u64> {
        if self.rate_per_ms == 0.0 {
            return if self.current == threshold { Some(0) } else { None };
        }
        let t = (threshold - self.current) / self.rate_per_ms;
        if t.is_finite() && t >= 0.0 {
            Some(t as u64)
        } else {
            None
        }
    }

    /// Rate of change per second (friendly units for reporting).
    pub fn rate_per_s(&self) -> f64 {
        self.rate_per_ms * 1000.0
    }
}

/// Fits trends from world-memory history over a recent window.
///
/// By default this is ordinary least squares (every point weighted equally). With
/// a `decay` (forgetting factor) below 1.0 it becomes **exponentially-weighted
/// least squares** — an online estimator that down-weights older points by
/// `decay^age`, so the fitted trend tracks the *recent* regime and adapts when the
/// rate of change shifts (e.g. a battery that just started draining faster). This
/// is the recursive-least-squares-with-forgetting model used for non-stationary
/// signals; `decay == 1.0` recovers plain OLS.
#[derive(Debug, Clone, Copy)]
pub struct Forecaster {
    /// Max number of most-recent points to fit.
    window: usize,
    /// Forgetting factor in `(0, 1]`: weight of a point is `decay^age` (age in
    /// samples back from newest). `1.0` = equal weight (OLS).
    decay: f64,
}

impl Default for Forecaster {
    fn default() -> Self {
        Self { window: 16, decay: 1.0 }
    }
}

impl Forecaster {
    pub fn new(window: usize) -> Self {
        Self { window: window.max(1), decay: 1.0 }
    }

    /// Set the forgetting factor (clamped to `(0, 1]`). Below 1.0 turns the fit
    /// into exponentially-weighted least squares — more responsive to recent data.
    pub fn with_decay(mut self, decay: f64) -> Self {
        self.decay = decay.clamp(1e-6, 1.0);
        self
    }

    /// Fit a [`Forecast`] for `entity` from its recent history. `None` if there is
    /// no numeric history.
    pub fn forecast(&self, world: &WorldMemory, entity: &str) -> anyhow::Result<Option<Forecast>> {
        let facts = world.history(entity)?;
        let mut pts: Vec<(f64, f64)> = facts
            .iter()
            .filter_map(|f| value_to_f64(&f.value).map(|v| (f.valid_from as f64, v)))
            .collect();
        if pts.is_empty() {
            return Ok(None);
        }
        // Keep the most-recent `window` points.
        if pts.len() > self.window {
            pts.drain(0..pts.len() - self.window);
        }
        let n = pts.len();
        let (last_t, current) = *pts.last().unwrap();
        if n == 1 {
            return Ok(Some(Forecast { current, rate_per_ms: 0.0, samples: 1, at_ms: last_t as u64 }));
        }
        // Per-point weights: newest weight 1, older decays by `decay^age`.
        // (decay == 1.0 ⇒ all weights 1 ⇒ ordinary least squares.)
        let w: Vec<f64> = (0..n).map(|i| self.decay.powi((n - 1 - i) as i32)).collect();
        let wsum: f64 = w.iter().sum();
        let tm = pts.iter().zip(&w).map(|((t, _), wi)| wi * t).sum::<f64>() / wsum;
        let vm = pts.iter().zip(&w).map(|((_, v), wi)| wi * v).sum::<f64>() / wsum;
        let num: f64 = pts.iter().zip(&w).map(|((t, v), wi)| wi * (t - tm) * (v - vm)).sum();
        let den: f64 = pts.iter().zip(&w).map(|((t, _), wi)| wi * (t - tm).powi(2)).sum();
        let slope = if den != 0.0 { num / den } else { 0.0 };
        Ok(Some(Forecast { current, rate_per_ms: slope, samples: n, at_ms: last_t as u64 }))
    }
}

/// A predictive rule: when `entity` is (or is predicted within `horizon_ms` to
/// be) `op` `threshold`, perform `then`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForesightRule {
    pub id: String,
    pub entity: String,
    pub op: Cmp,
    pub threshold: f64,
    /// Look-ahead window (ms): fire if the crossing is predicted within this.
    pub horizon_ms: u64,
    pub then: Action,
    #[serde(default)]
    pub debounce_ms: u64,
}

impl ForesightRule {
    /// Whether this rule fires given a forecast: already satisfied now, or
    /// predicted to cross within the horizon.
    pub fn fires(&self, fc: &Forecast) -> bool {
        if self.op.test(fc.current, self.threshold) {
            return true;
        }
        fc.time_to_threshold(self.threshold)
            .is_some_and(|eta| eta <= self.horizon_ms)
    }
}

/// Evaluates [`ForesightRule`]s against forecasts, with per-rule debounce.
/// Records each rule's forecast into `foresight.{entity}`.
pub struct ForesightEngine {
    rules: Vec<ForesightRule>,
    /// Approved *learned* rules, shared with the learning layer so the self-
    /// authoring loop can activate new rules live (after approval).
    learned: Option<Arc<Mutex<Vec<ForesightRule>>>>,
    forecaster: Forecaster,
    last_fire: Mutex<HashMap<String, u64>>,
    source: String,
}

impl ForesightEngine {
    pub fn new(rules: Vec<ForesightRule>) -> Self {
        Self {
            rules,
            learned: None,
            forecaster: Forecaster::default(),
            last_fire: Mutex::new(HashMap::new()),
            source: "foresight".to_string(),
        }
    }

    pub fn with_forecaster(mut self, forecaster: Forecaster) -> Self {
        self.forecaster = forecaster;
        self
    }

    /// Attach a shared buffer of approved learned rules (evaluated alongside the
    /// static ones each tick). The learning layer pushes rules in on approval.
    pub fn with_learned_rules(mut self, learned: Arc<Mutex<Vec<ForesightRule>>>) -> Self {
        self.learned = Some(learned);
        self
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Forecast a rule's entity, record the prediction, and fire if its
    /// (predicted) condition holds — respecting debounce. Pushes onto `fired`.
    fn eval_one(
        &self,
        world: &WorldMemory,
        now_ms: u64,
        rule: &ForesightRule,
        guard: &mut HashMap<String, u64>,
        fired: &mut Vec<FiredReflex>,
    ) {
        let Ok(Some(fc)) = self.forecaster.forecast(world, &rule.entity) else {
            return;
        };
        let eta = fc.time_to_threshold(rule.threshold);
        let _ = world.observe(
            &format!("foresight.{}", rule.entity),
            json!({
                "current": fc.current,
                "rate_per_s": fc.rate_per_s(),
                "predicted": fc.predict_at(rule.horizon_ms),
                "eta_ms": eta,
                "threshold": rule.threshold,
            }),
            now_ms,
            now_ms,
            &self.source,
        );
        if !rule.fires(&fc) {
            return;
        }
        if rule.debounce_ms > 0 {
            if let Some(&last) = guard.get(&rule.id) {
                if now_ms.saturating_sub(last) < rule.debounce_ms {
                    return;
                }
            }
        }
        guard.insert(rule.id.clone(), now_ms);
        fired.push(FiredReflex { rule_id: rule.id.clone(), action: rule.then.clone() });
    }

    /// Evaluate all static and approved-learned rules; returns what fired.
    pub fn evaluate(&self, world: &WorldMemory, now_ms: u64) -> Vec<FiredReflex> {
        let mut guard = self.last_fire.lock().unwrap_or_else(|p| p.into_inner());
        let mut fired = Vec::new();
        for rule in &self.rules {
            self.eval_one(world, now_ms, rule, &mut guard, &mut fired);
        }
        if let Some(learned) = &self.learned {
            let rules = learned.lock().unwrap_or_else(|p| p.into_inner()).clone();
            for rule in &rules {
                self.eval_one(world, now_ms, rule, &mut guard, &mut fired);
            }
        }
        fired
    }
}

/// Ties the foresight engine to world memory and an action sink — the Track 1
/// controller. Spawn its [`tick_and_dispatch`](Self::tick_and_dispatch) on a cadence.
pub struct ForesightController {
    engine: ForesightEngine,
    world: Arc<WorldMemory>,
    sink: Arc<dyn ActionSink>,
    escalation_budget: Option<EscalationBudget>,
}

impl ForesightController {
    pub fn new(engine: ForesightEngine, world: Arc<WorldMemory>, sink: Arc<dyn ActionSink>) -> Self {
        Self { engine, world, sink, escalation_budget: None }
    }

    pub fn with_escalation_budget(mut self, budget: EscalationBudget) -> Self {
        self.escalation_budget = Some(budget);
        self
    }

    /// Evaluate predictive rules and dispatch the fired actions. Escalations
    /// beyond the budget are fired-but-not-dispatched.
    pub async fn tick_and_dispatch(&self, now_ms: u64) -> anyhow::Result<Vec<FiredReflex>> {
        let fired = self.engine.evaluate(&self.world, now_ms);
        for f in &fired {
            match &f.action {
                Action::GpioWrite { node_id, pin, value } => {
                    self.sink.gpio_write(node_id, *pin, *value).await?
                }
                Action::Publish { topic, payload } => self.sink.publish(topic, payload).await?,
                Action::Escalate { reason } => {
                    let allowed = self.escalation_budget.as_ref().is_none_or(|b| b.allow(now_ms));
                    if allowed {
                        self.sink.escalate(reason).await?;
                    } else {
                        tracing::debug!(reason, "foresight: escalation suppressed by budget");
                    }
                }
                Action::Move { command } => self.sink.move_actuator(command).await?,
            }
        }
        Ok(fired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn world_with_series(entity: &str, pts: &[(u64, f64)]) -> Arc<WorldMemory> {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        for &(t, v) in pts {
            world.observe(entity, json!({ "value": v }), t, t, "sensor").unwrap();
        }
        world
    }

    #[test]
    fn fits_declining_trend_and_predicts_crossing() {
        // battery: 100 → 80 → 60 over 0..2s ⇒ rate -20/s
        let world = world_with_series("power.soc", &[(0, 100.0), (1_000, 80.0), (2_000, 60.0)]);
        let fc = Forecaster::default().forecast(&world, "power.soc").unwrap().unwrap();
        assert!((fc.current - 60.0).abs() < 1e-9);
        assert!((fc.rate_per_s() - (-20.0)).abs() < 1e-6);
        // predict 1s ahead → ~40
        assert!((fc.predict_at(1_000) - 40.0).abs() < 1e-6);
        // time to reach 10 from 60 at -20/s ⇒ 2.5s = 2500ms
        assert_eq!(fc.time_to_threshold(10.0), Some(2_500));
    }

    #[test]
    fn weighted_forecaster_tracks_an_accelerating_decline() {
        // Drain is slow early (−1/s) then steepens (−8/s). OLS averages the whole
        // window and lags; EWLS (decay 0.5) weights the recent steep part and
        // reports a faster decline — so it predicts the crossing sooner.
        let series = [
            (0u64, 100.0),
            (1_000, 99.0),
            (2_000, 98.0),
            (3_000, 90.0),
            (4_000, 82.0),
            (5_000, 74.0),
        ];
        let world = world_with_series("power.soc", &series);
        let ols = Forecaster::default().forecast(&world, "power.soc").unwrap().unwrap();
        let ewls = Forecaster::default().with_decay(0.5).forecast(&world, "power.soc").unwrap().unwrap();
        assert!(
            ewls.rate_per_s() < ols.rate_per_s(),
            "EWLS tracks the recent steeper decline: ewls {} < ols {}",
            ewls.rate_per_s(),
            ols.rate_per_s()
        );
        // both see the same current value
        assert!((ewls.current - 74.0).abs() < 1e-9 && (ols.current - 74.0).abs() < 1e-9);
        // EWLS predicts reaching 10 sooner than OLS does
        let (e_eta, o_eta) = (ewls.time_to_threshold(10.0), ols.time_to_threshold(10.0));
        assert!(matches!((e_eta, o_eta), (Some(e), Some(o)) if e < o));
    }

    #[test]
    fn decay_one_matches_ordinary_least_squares() {
        let world = world_with_series("x", &[(0, 100.0), (1_000, 80.0), (2_000, 60.0)]);
        let ols = Forecaster::default().forecast(&world, "x").unwrap().unwrap();
        let same = Forecaster::default().with_decay(1.0).forecast(&world, "x").unwrap().unwrap();
        assert!((ols.rate_per_ms - same.rate_per_ms).abs() < 1e-12);
    }

    #[test]
    fn stable_or_rising_series_has_no_downward_crossing() {
        let world = world_with_series("x", &[(0, 50.0), (1_000, 50.0), (2_000, 50.0)]);
        let fc = Forecaster::default().forecast(&world, "x").unwrap().unwrap();
        assert_eq!(fc.rate_per_ms, 0.0);
        assert_eq!(fc.time_to_threshold(10.0), None); // flat, never reaches
    }

    #[test]
    fn rule_fires_before_the_event() {
        // currently 20, draining 20/s → will hit 10 in 0.5s
        let world = world_with_series("power.soc", &[(0, 40.0), (1_000, 20.0)]);
        let rule = ForesightRule {
            id: "predict-critical".into(),
            entity: "power.soc".into(),
            op: Cmp::Le,
            threshold: 10.0,
            horizon_ms: 60_000,
            then: Action::Escalate { reason: "battery predicted critical — return to base".into() },
            debounce_ms: 0,
        };
        let engine = ForesightEngine::new(vec![rule]);
        let fired = engine.evaluate(&world, 1_000);
        assert_eq!(fired.len(), 1, "should fire predictively while still above threshold");
        assert!(matches!(fired[0].action, Action::Escalate { .. }));
        // and the prediction is recorded
        let pred = world.current("foresight.power.soc").unwrap().unwrap();
        assert!(pred.value["eta_ms"].as_u64().is_some());
    }

    #[test]
    fn rule_does_not_fire_when_far_off_and_outside_horizon() {
        // draining slowly (1/s), 20 now, threshold 10, horizon only 1s → eta 10s > horizon
        let world = world_with_series("power.soc", &[(0, 21.0), (1_000, 20.0)]);
        let rule = ForesightRule {
            id: "p".into(),
            entity: "power.soc".into(),
            op: Cmp::Le,
            threshold: 10.0,
            horizon_ms: 1_000,
            then: Action::Escalate { reason: "x".into() },
            debounce_ms: 0,
        };
        let engine = ForesightEngine::new(vec![rule]);
        assert!(engine.evaluate(&world, 1_000).is_empty());
    }

    #[test]
    fn already_satisfied_fires_immediately() {
        let world = world_with_series("temp", &[(0, 70.0), (1_000, 90.0)]); // already > 80
        let rule = ForesightRule {
            id: "hot".into(),
            entity: "temp".into(),
            op: Cmp::Ge,
            threshold: 80.0,
            horizon_ms: 1_000,
            then: Action::Escalate { reason: "overheat".into() },
            debounce_ms: 0,
        };
        let engine = ForesightEngine::new(vec![rule]);
        assert_eq!(engine.evaluate(&world, 1_000).len(), 1);
    }

    #[test]
    fn evaluates_learned_rules_from_shared_buffer() {
        // a rule that arrives via the shared (learned) buffer fires just like a static one
        let world = world_with_series("power.soc", &[(0, 40.0), (1_000, 20.0)]);
        let learned = Arc::new(Mutex::new(vec![ForesightRule {
            id: "learned-x".into(),
            entity: "power.soc".into(),
            op: Cmp::Le,
            threshold: 10.0,
            horizon_ms: 60_000,
            then: Action::Escalate { reason: "learned".into() },
            debounce_ms: 0,
        }]));
        let engine = ForesightEngine::new(vec![]).with_learned_rules(learned);
        let fired = engine.evaluate(&world, 1_000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule_id, "learned-x");
    }

    #[test]
    fn rule_serde_roundtrips() {
        let r = ForesightRule {
            id: "r".into(),
            entity: "power.soc".into(),
            op: Cmp::Le,
            threshold: 10.0,
            horizon_ms: 60_000,
            then: Action::Escalate { reason: "x".into() },
            debounce_ms: 5_000,
        };
        let back: ForesightRule = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}

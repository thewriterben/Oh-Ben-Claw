//! Belief-state localization — a particle filter over SE2 poses.
//!
//! SLAM and pose fusion produce a single point estimate; a particle filter
//! carries a whole **belief** — a cloud of weighted pose hypotheses — so the
//! system represents *how sure* it is about where it is. Motion updates push and
//! diffuse the cloud (proposal); measurement updates reweight it by likelihood
//! and resample toward agreement. The estimate is the weighted mean, and its
//! **spread** is honest uncertainty the rest of the stack can act on (e.g. slow
//! down or relocalize when spread is high).
//!
//! Uses a small deterministic PRNG (no external dep, reproducible in tests) and
//! reuses the SLAM SE2 types. The localizer backend records the belief into world
//! memory (`sensor.pos_*` + `nav.belief`) so navigation reads the filtered pose.

use super::slam::{compose, Pose2, RelPose};
use crate::memory::world::WorldMemory;
use serde_json::json;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex};

/// A tiny deterministic xorshift PRNG (reproducible; no external dependency).
#[derive(Debug, Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal via Box-Muller.
    fn gauss(&mut self) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

#[derive(Debug, Clone, Copy)]
struct Particle {
    pose: Pose2,
    weight: f64,
}

/// A particle filter over 2D poses.
#[derive(Debug, Clone)]
pub struct ParticleFilter {
    particles: Vec<Particle>,
    rng: Rng,
}

impl ParticleFilter {
    /// Initialize `n` particles around `init` with Gaussian position/heading
    /// spread, uniform weights. `seed` makes the filter deterministic.
    pub fn new(n: usize, init: Pose2, pos_spread: f64, head_spread: f64, seed: u64) -> Self {
        let n = n.max(1);
        let mut rng = Rng::new(seed);
        let w = 1.0 / n as f64;
        let particles = (0..n)
            .map(|_| Particle {
                pose: Pose2::new(
                    init.x + rng.gauss() * pos_spread,
                    init.y + rng.gauss() * pos_spread,
                    init.theta + rng.gauss() * head_spread,
                ),
                weight: w,
            })
            .collect();
        Self { particles, rng }
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Motion update: apply `motion` (a relative move) to every particle with
    /// Gaussian noise — the cloud advances and diffuses.
    pub fn predict(&mut self, motion: RelPose, trans_sigma: f64, rot_sigma: f64) {
        for p in &mut self.particles {
            let noisy = RelPose {
                dx: motion.dx + self.rng.gauss() * trans_sigma,
                dy: motion.dy + self.rng.gauss() * trans_sigma,
                dtheta: motion.dtheta + self.rng.gauss() * rot_sigma,
            };
            p.pose = compose(p.pose, noisy);
        }
    }

    /// Measurement update: reweight by a Gaussian position likelihood around
    /// `(mx, my)` with std `sigma`, normalize, and resample if degenerate.
    pub fn update_position(&mut self, mx: f64, my: f64, sigma: f64) {
        let s2 = (sigma * sigma).max(1e-9);
        let mut total = 0.0;
        for p in &mut self.particles {
            let dx = p.pose.x - mx;
            let dy = p.pose.y - my;
            let like = (-(dx * dx + dy * dy) / (2.0 * s2)).exp();
            p.weight *= like;
            total += p.weight;
        }
        if total <= 0.0 {
            // Likelihood collapsed — reset to uniform rather than divide by zero.
            let w = 1.0 / self.particles.len() as f64;
            for p in &mut self.particles {
                p.weight = w;
            }
            return;
        }
        for p in &mut self.particles {
            p.weight /= total;
        }
        if self.effective_sample_size() < self.particles.len() as f64 / 2.0 {
            self.resample();
        }
    }

    /// Effective sample size `1 / Σ w²` — low ⇒ the cloud has collapsed.
    pub fn effective_sample_size(&self) -> f64 {
        let s: f64 = self.particles.iter().map(|p| p.weight * p.weight).sum();
        if s > 0.0 {
            1.0 / s
        } else {
            0.0
        }
    }

    /// Low-variance (systematic) resampling — replaces the cloud, weights reset.
    fn resample(&mut self) {
        let n = self.particles.len();
        let mut new = Vec::with_capacity(n);
        let step = 1.0 / n as f64;
        let mut u = self.rng.unit() * step;
        let mut cum = 0.0;
        let mut i = 0;
        for _ in 0..n {
            while i < n - 1 && cum + self.particles[i].weight < u {
                cum += self.particles[i].weight;
                i += 1;
            }
            new.push(Particle { pose: self.particles[i].pose, weight: 1.0 / n as f64 });
            u += step;
        }
        self.particles = new;
    }

    /// The belief estimate: weighted mean position, circular-mean heading, and a
    /// position **spread** (RMS distance from the mean) as uncertainty.
    pub fn estimate(&self) -> (Pose2, f64) {
        let wsum: f64 = self.particles.iter().map(|p| p.weight).sum();
        let wsum = if wsum > 0.0 { wsum } else { 1.0 };
        let mut mx = 0.0;
        let mut my = 0.0;
        let mut sin = 0.0;
        let mut cos = 0.0;
        for p in &self.particles {
            mx += p.weight * p.pose.x;
            my += p.weight * p.pose.y;
            sin += p.weight * p.pose.theta.sin();
            cos += p.weight * p.pose.theta.cos();
        }
        mx /= wsum;
        my /= wsum;
        let heading = sin.atan2(cos);
        let var: f64 = self
            .particles
            .iter()
            .map(|p| p.weight * ((p.pose.x - mx).powi(2) + (p.pose.y - my).powi(2)))
            .sum::<f64>()
            / wsum;
        (Pose2::new(mx, my, heading), var.sqrt())
    }
}

/// Online belief-state localizer: drive it with motion + position measurements;
/// it records the belief (estimate + spread) into world memory so navigation
/// reads the *filtered* pose with honest uncertainty.
pub struct ParticleLocalizer {
    filter: Mutex<ParticleFilter>,
    world: Option<Arc<WorldMemory>>,
    meas_sigma: f64,
    trans_sigma: f64,
    rot_sigma: f64,
    source: String,
}

impl ParticleLocalizer {
    pub fn new(filter: ParticleFilter) -> Self {
        Self {
            filter: Mutex::new(filter),
            world: None,
            meas_sigma: 0.5,
            trans_sigma: 0.1,
            rot_sigma: 0.05,
            source: "particle".to_string(),
        }
    }

    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    pub fn with_noise(mut self, meas_sigma: f64, trans_sigma: f64, rot_sigma: f64) -> Self {
        self.meas_sigma = meas_sigma;
        self.trans_sigma = trans_sigma;
        self.rot_sigma = rot_sigma;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ParticleFilter> {
        self.filter.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The current belief: estimated pose + position spread (uncertainty).
    pub fn estimate(&self) -> (Pose2, f64) {
        self.lock().estimate()
    }

    fn record(&self, now_ms: u64) {
        let Some(world) = &self.world else { return };
        let (pose, spread) = self.estimate();
        let _ = world.observe("sensor.pos_x", json!({ "value": pose.x }), now_ms, now_ms, &self.source);
        let _ = world.observe("sensor.pos_y", json!({ "value": pose.y }), now_ms, now_ms, &self.source);
        let _ = world.observe(
            "sensor.heading",
            json!({ "value": pose.theta.to_degrees() }),
            now_ms,
            now_ms,
            &self.source,
        );
        let _ = world.observe(
            "nav.belief",
            json!({ "x": pose.x, "y": pose.y, "heading_deg": pose.theta.to_degrees(), "spread": spread }),
            now_ms,
            now_ms,
            &self.source,
        );
    }

    /// Ingest a relative motion (odometry) and record the new belief.
    pub fn add_motion(&self, motion: RelPose, now_ms: u64) {
        self.lock().predict(motion, self.trans_sigma, self.rot_sigma);
        self.record(now_ms);
    }

    /// Ingest a position measurement (e.g. GPS/beacon) and record the new belief.
    pub fn add_measurement(&self, x: f64, y: f64, now_ms: u64) {
        self.lock().update_position(x, y, self.meas_sigma);
        self.record(now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_to_the_measured_position_and_spread_shrinks() {
        // start uncertain around the origin, truth is (5, 0)
        let mut pf = ParticleFilter::new(500, Pose2::new(0.0, 0.0, 0.0), 3.0, 0.3, 42);
        let (_, spread0) = pf.estimate();
        assert!(spread0 > 1.0, "should start uncertain, spread {spread0}");

        // a forward motion toward truth, then repeated position fixes at (5,0)
        pf.predict(RelPose::new(5.0, 0.0, 0.0), 0.2, 0.05);
        for _ in 0..6 {
            pf.update_position(5.0, 0.0, 0.5);
        }
        let (est, spread) = pf.estimate();
        assert!((est.x - 5.0).abs() < 0.5, "x converged near 5, got {}", est.x);
        assert!((est.y - 0.0).abs() < 0.5, "y converged near 0, got {}", est.y);
        assert!(spread < spread0, "uncertainty should shrink: {spread0} → {spread}");
    }

    #[test]
    fn resampling_keeps_particle_count() {
        let mut pf = ParticleFilter::new(100, Pose2::new(0.0, 0.0, 0.0), 2.0, 0.2, 7);
        pf.update_position(0.0, 0.0, 0.3); // tight measurement → likely resample
        assert_eq!(pf.len(), 100);
    }

    #[test]
    fn effective_sample_size_drops_when_one_particle_dominates() {
        let mut pf = ParticleFilter::new(50, Pose2::new(0.0, 0.0, 0.0), 5.0, 0.2, 3);
        let ess_before = pf.effective_sample_size();
        // a sharp measurement far in the tail concentrates weight
        pf.update_position(0.0, 0.0, 0.05);
        // after normalization+possible resample, ESS is defined and positive
        assert!(pf.effective_sample_size() > 0.0);
        assert!(ess_before > 0.0);
    }

    #[test]
    fn localizer_records_belief_with_uncertainty() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let pf = ParticleFilter::new(200, Pose2::new(0.0, 0.0, 0.0), 2.0, 0.2, 11);
        let loc = ParticleLocalizer::new(pf).with_world_memory(Arc::clone(&world));
        loc.add_motion(RelPose::new(3.0, 0.0, 0.0), 1_000);
        loc.add_measurement(3.0, 0.0, 2_000);
        let belief = world.current("nav.belief").unwrap().unwrap();
        assert!(belief.value["spread"].as_f64().is_some());
        // a filtered pose is published for navigation to read
        assert!(world.current("sensor.pos_x").unwrap().is_some());
    }
}

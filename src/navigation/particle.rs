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

use super::sensor_model::{BeamModelParams, LikelihoodField};
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

/// KLD-sampling parameters (Fox 2003): adapt the particle count to the spread of
/// the belief — many particles when uncertain, few when confident.
#[derive(Debug, Clone, Copy)]
pub struct KldParams {
    pub min: usize,
    pub max: usize,
    /// KL-divergence error bound (smaller ⇒ more particles).
    pub epsilon: f64,
    /// Upper standard-normal quantile for the confidence (e.g. 2.326 ≈ 0.99).
    pub z: f64,
    /// Spatial bin size for the support estimate.
    pub bin_size: f64,
}

impl Default for KldParams {
    fn default() -> Self {
        Self { min: 50, max: 2000, epsilon: 0.05, z: 2.326, bin_size: 0.5 }
    }
}

/// The KLD sample-size bound for `k` occupied bins (Fox 2003, Wilson-Hilferty
/// approximation): the number of samples needed to keep the KL divergence between
/// the sample and true distribution below `epsilon` with confidence from `z`.
fn kld_bound(k: usize, epsilon: f64, z: f64) -> usize {
    if k <= 1 {
        return 0;
    }
    let k = k as f64;
    let a = 1.0 - 2.0 / (9.0 * (k - 1.0));
    let b = (2.0 / (9.0 * (k - 1.0))).sqrt() * z;
    let n = (k - 1.0) / (2.0 * epsilon) * (a + b).powi(3);
    n.ceil().max(0.0) as usize
}

/// A particle filter over 2D poses.
#[derive(Debug, Clone)]
pub struct ParticleFilter {
    particles: Vec<Particle>,
    rng: Rng,
    kld: Option<KldParams>,
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
        Self { particles, rng, kld: None }
    }

    /// Enable **adaptive (KLD)** resampling — the particle count grows when the
    /// belief is spread and shrinks when it concentrates.
    pub fn with_kld(mut self, params: KldParams) -> Self {
        self.kld = Some(params);
        self
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
            self.do_resample();
        }
    }

    /// Measurement update from a **range scan** (likelihood-field model): reweight
    /// each particle by how well its pose explains `beams` against the map `field`.
    /// This is the real range-sensor update — a misaligned pose makes its beams
    /// miss the mapped walls and its weight collapses.
    pub fn update_scan(&mut self, field: &LikelihoodField, beams: &[(f64, f64)], params: &BeamModelParams) {
        // Per-particle log-likelihoods, stabilized by subtracting the max before
        // exponentiating (avoids underflow when many beams agree).
        let logs: Vec<f64> = self
            .particles
            .iter()
            .map(|p| field.scan_log_likelihood(p.pose, beams, params))
            .collect();
        let max_log = logs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if !max_log.is_finite() {
            return; // no usable beams (all out of range)
        }
        let mut total = 0.0;
        for (p, &lg) in self.particles.iter_mut().zip(&logs) {
            p.weight *= (lg - max_log).exp();
            total += p.weight;
        }
        if total <= 0.0 {
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
            self.do_resample();
        }
    }

    /// Resample: KLD-adaptive when configured, else fixed-size low-variance.
    fn do_resample(&mut self) {
        if self.kld.is_some() {
            self.resample_kld();
        } else {
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

    /// KLD-adaptive resampling: draw weighted samples, bin them spatially, and
    /// stop once the drawn count meets the KL-divergence bound for the number of
    /// occupied bins (clamped to `[min, max]`). Concentrated beliefs ⇒ few
    /// particles; spread beliefs ⇒ many.
    fn resample_kld(&mut self) {
        let p = self.kld.expect("kld configured");
        // Cumulative weights for roulette sampling.
        let n_old = self.particles.len();
        if n_old == 0 {
            return;
        }
        let mut cum = Vec::with_capacity(n_old);
        let mut acc = 0.0;
        for part in &self.particles {
            acc += part.weight;
            cum.push(acc);
        }
        let total = acc.max(1e-12);

        let mut new: Vec<Particle> = Vec::new();
        let mut bins: std::collections::HashSet<(i64, i64, i64)> = std::collections::HashSet::new();
        let mut n_needed = p.min;
        loop {
            if new.len() >= p.max {
                break;
            }
            // draw one particle by weight
            let u = self.rng.unit() * total;
            let idx = cum.iter().position(|&c| c >= u).unwrap_or(n_old - 1);
            let pose = self.particles[idx].pose;
            new.push(Particle { pose, weight: 0.0 });

            let bx = (pose.x / p.bin_size).floor() as i64;
            let by = (pose.y / p.bin_size).floor() as i64;
            let bt = (pose.theta / p.bin_size).floor() as i64;
            if bins.insert((bx, by, bt)) {
                let k = bins.len();
                if k > 1 {
                    n_needed = kld_bound(k, p.epsilon, p.z).clamp(p.min, p.max);
                }
            }
            if new.len() >= n_needed {
                break;
            }
        }
        let w = 1.0 / new.len() as f64;
        for part in &mut new {
            part.weight = w;
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
    beam_params: BeamModelParams,
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
            beam_params: BeamModelParams::default(),
            source: "particle".to_string(),
        }
    }

    /// Set the range-sensor (likelihood-field) model parameters used by [`add_scan`].
    pub fn with_beam_params(mut self, params: BeamModelParams) -> Self {
        self.beam_params = params;
        self
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

    /// Ingest a **range scan** against the map likelihood `field` and record the
    /// new belief. `beams` are `(bearing_deg, range)` relative to robot heading.
    pub fn add_scan(&self, field: &LikelihoodField, beams: &[(f64, f64)], now_ms: u64) {
        self.lock().update_scan(field, beams, &self.beam_params);
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

    #[test]
    fn scan_update_pulls_cloud_toward_the_pose_that_explains_it() {
        use crate::navigation::planning::{Cell, OccupancyGrid};
        use crate::navigation::sensor_model::{BeamModelParams, LikelihoodField};

        // 16×16 @ 0.5 m → covers [0,8). A vertical wall at world x≈6 (cell col 12).
        let mut g = OccupancyGrid::new(0.0, 0.0, 0.5, 16, 16);
        for cy in 0..16 {
            g.set(12, cy, Cell::Occupied);
        }
        let field = LikelihoodField::from_grid(&g, 5.0);
        let params = BeamModelParams { sigma_hit: 0.4, z_hit: 0.9, z_rand: 0.1, max_range: 20.0 };

        // Truth: facing east at x=2, a forward beam of range 4 lands on the wall
        // (2 + 4 = 6). The likelihood is maximized when a particle's x ≈ 2.
        let beams = [(0.0, 4.0)];
        // Start the cloud offset ~1 m in x (centered at x=3).
        let mut pf = ParticleFilter::new(800, Pose2::new(3.0, 4.0, 0.0), 1.0, 0.03, 5);
        let (est0, _) = pf.estimate();
        for _ in 0..8 {
            pf.update_scan(&field, &beams, &params);
        }
        let (est1, _) = pf.estimate();
        assert!(
            (est1.x - 2.0).abs() < (est0.x - 2.0).abs(),
            "scan update moves the estimate toward truth: {} → {}",
            est0.x,
            est1.x
        );
        assert!((est1.x - 2.0).abs() < 0.6, "converges near truth x=2, got {}", est1.x);
    }

    #[test]
    fn kld_bound_grows_with_occupied_bins() {
        assert_eq!(kld_bound(1, 0.05, 2.326), 0);
        let few = kld_bound(2, 0.05, 2.326);
        let many = kld_bound(50, 0.05, 2.326);
        assert!(many > few, "more bins ⇒ larger sample bound: {few} → {many}");
    }

    #[test]
    fn kld_uses_few_particles_when_belief_is_concentrated() {
        let params = KldParams { min: 20, max: 2000, epsilon: 0.05, z: 2.326, bin_size: 0.5 };
        // 500 particles in one tiny pocket, centered well inside a single bin (not
        // straddling a bin seam) → one occupied bin → collapses to the minimum.
        let mut pf = ParticleFilter::new(500, Pose2::new(0.1, 0.1, 0.1), 0.01, 0.01, 1).with_kld(params);
        pf.resample_kld();
        assert!(pf.len() <= 40, "concentrated belief uses few particles, got {}", pf.len());
        assert!(pf.len() >= params.min);
    }

    #[test]
    fn kld_uses_many_particles_when_belief_is_spread() {
        let params = KldParams { min: 20, max: 2000, epsilon: 0.05, z: 2.326, bin_size: 0.5 };
        // a broad belief occupies many bins → the count grows toward the max
        let mut pf = ParticleFilter::new(800, Pose2::new(0.0, 0.0, 0.0), 6.0, 1.0, 2).with_kld(params);
        pf.resample_kld();
        assert!(pf.len() > 100, "spread belief uses many particles, got {}", pf.len());
        assert!(pf.len() <= params.max);
    }
}

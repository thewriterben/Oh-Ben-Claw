//! Range-finder sensor model — the **likelihood field** (Thrun, *Probabilistic
//! Robotics* §6.4; the model AMCL uses by default).
//!
//! The particle filter's first measurement update was a toy: a Gaussian on a
//! single `(x, y)` position fix. Real robots localize from **range scans** — a
//! fan of beams hitting whatever is around them. This module scores a *candidate
//! pose* against the map: project each beam's endpoint into the world from that
//! pose, and ask "how close is that endpoint to a mapped obstacle?". Endpoints
//! that land on walls are likely; endpoints floating in open space are not.
//!
//! Two pieces:
//!  * [`LikelihoodField`] — a precomputed distance-to-nearest-obstacle field over
//!    the grid (a chamfer Euclidean distance transform, O(cells)). Lookups are
//!    O(1), so scoring hundreds of particles against a scan is cheap.
//!  * [`BeamModelParams`] + [`LikelihoodField::scan_log_likelihood`] — the mixture
//!    model `z_hit·𝒩(dist; σ) + z_rand` summed in log-space over beams.
//!
//! The filter calls [`crate::navigation::particle::ParticleFilter::update_scan`]
//! with these; a wrong pose makes the beams miss the walls and its weight
//! collapses, so the cloud converges on the pose that *explains the scan*.

use super::planning::{Cell, OccupancyGrid};
use super::slam::Pose2;

const SQRT2: f64 = std::f64::consts::SQRT_2;

/// Mixture parameters for the likelihood-field beam model.
#[derive(Debug, Clone, Copy)]
pub struct BeamModelParams {
    /// Std-dev (world units) of the Gaussian on endpoint-to-obstacle distance.
    pub sigma_hit: f64,
    /// Weight of the "hit a real obstacle" component.
    pub z_hit: f64,
    /// Flat floor for random/unexplained readings (keeps likelihood positive).
    pub z_rand: f64,
    /// Sensor max range; beams at or beyond this carry no endpoint and are skipped.
    pub max_range: f64,
}

impl Default for BeamModelParams {
    fn default() -> Self {
        Self { sigma_hit: 0.2, z_hit: 0.95, z_rand: 0.05, max_range: 20.0 }
    }
}

impl BeamModelParams {
    /// The likelihood of a single beam whose endpoint is `dist` (world units) from
    /// the nearest mapped obstacle: `z_hit · exp(−dist²/2σ²) + z_rand`.
    pub fn beam_likelihood(&self, dist: f64) -> f64 {
        let s2 = (self.sigma_hit * self.sigma_hit).max(1e-9);
        self.z_hit * (-(dist * dist) / (2.0 * s2)).exp() + self.z_rand
    }
}

/// A distance-to-nearest-obstacle field over a grid, in **world units**.
///
/// Built once per map (or per map update) with a two-pass chamfer distance
/// transform — a linear-time Euclidean-distance approximation (weights `1` for
/// orthogonal steps, `√2` for diagonal). Cells with no obstacle anywhere report
/// the field's saturation distance.
#[derive(Debug, Clone)]
pub struct LikelihoodField {
    origin_x: f64,
    origin_y: f64,
    resolution: f64,
    width: usize,
    height: usize,
    /// Distance (world units) to the nearest occupied cell, per cell.
    dist: Vec<f64>,
    /// Distance reported for points outside the grid / with no obstacle near.
    saturation: f64,
}

impl LikelihoodField {
    /// Build the field from a grid. `saturation` caps the distance (and is what
    /// out-of-map endpoints report), so a far-off endpoint gets a fixed small
    /// likelihood rather than an unbounded one.
    pub fn from_grid(grid: &OccupancyGrid, saturation: f64) -> Self {
        let (w, h) = (grid.width(), grid.height());
        let res = grid.resolution();
        let big = f64::from(u16::MAX);
        let mut d = vec![big; w * h];
        for cy in 0..h {
            for cx in 0..w {
                if matches!(grid.get(cx, cy), Cell::Occupied) {
                    d[cy * w + cx] = 0.0;
                }
            }
        }

        // Forward pass (top-left → bottom-right).
        for cy in 0..h {
            for cx in 0..w {
                let i = cy * w + cx;
                let mut best = d[i];
                if cx > 0 {
                    best = best.min(d[i - 1] + 1.0);
                }
                if cy > 0 {
                    best = best.min(d[i - w] + 1.0);
                    if cx > 0 {
                        best = best.min(d[i - w - 1] + SQRT2);
                    }
                    if cx + 1 < w {
                        best = best.min(d[i - w + 1] + SQRT2);
                    }
                }
                d[i] = best;
            }
        }
        // Backward pass (bottom-right → top-left).
        for cy in (0..h).rev() {
            for cx in (0..w).rev() {
                let i = cy * w + cx;
                let mut best = d[i];
                if cx + 1 < w {
                    best = best.min(d[i + 1] + 1.0);
                }
                if cy + 1 < h {
                    best = best.min(d[i + w] + 1.0);
                    if cx + 1 < w {
                        best = best.min(d[i + w + 1] + SQRT2);
                    }
                    if cx > 0 {
                        best = best.min(d[i + w - 1] + SQRT2);
                    }
                }
                d[i] = best;
            }
        }

        // Convert cell-distances to world units and saturate.
        let sat_cells = if res > 0.0 { saturation / res } else { saturation };
        for v in &mut d {
            *v = (*v).min(sat_cells) * res;
        }

        Self {
            origin_x: grid_origin_x(grid),
            origin_y: grid_origin_y(grid),
            resolution: res,
            width: w,
            height: h,
            dist: d,
            saturation,
        }
    }

    /// Distance (world units) from a world point to the nearest mapped obstacle.
    /// Points outside the grid report the saturation distance.
    pub fn distance_at(&self, x: f64, y: f64) -> f64 {
        let cx = ((x - self.origin_x) / self.resolution).floor();
        let cy = ((y - self.origin_y) / self.resolution).floor();
        if cx < 0.0 || cy < 0.0 || cx as usize >= self.width || cy as usize >= self.height {
            return self.saturation;
        }
        self.dist[cy as usize * self.width + cx as usize]
    }

    /// Log-likelihood of a full range scan taken from candidate `pose`.
    ///
    /// `beams` are `(bearing_deg, range)` pairs (bearing relative to the robot's
    /// heading). Each in-range beam's endpoint is projected into the world, its
    /// distance to the nearest obstacle looked up, and the per-beam likelihood
    /// accumulated in log-space (stable across many beams). Out-of-range beams
    /// carry no endpoint and are skipped.
    pub fn scan_log_likelihood(&self, pose: Pose2, beams: &[(f64, f64)], params: &BeamModelParams) -> f64 {
        let mut log_l = 0.0;
        for &(bearing_deg, range) in beams {
            if range <= 0.0 || range >= params.max_range {
                continue;
            }
            let ang = pose.theta + bearing_deg.to_radians();
            let ex = pose.x + range * ang.cos();
            let ey = pose.y + range * ang.sin();
            let dist = self.distance_at(ex, ey);
            log_l += params.beam_likelihood(dist).max(1e-12).ln();
        }
        log_l
    }
}

fn grid_origin_x(grid: &OccupancyGrid) -> f64 {
    // OccupancyGrid keeps origin private; recover it from a cell center.
    let (cx0, _) = grid.cell_center(0, 0);
    cx0 - 0.5 * grid.resolution()
}
fn grid_origin_y(grid: &OccupancyGrid) -> f64 {
    let (_, cy0) = grid.cell_center(0, 0);
    cy0 - 0.5 * grid.resolution()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walled_grid() -> OccupancyGrid {
        // 20x20 @ 0.5 m, origin (0,0): covers [0,10) × [0,10).
        let mut g = OccupancyGrid::new(0.0, 0.0, 0.5, 20, 20);
        // a vertical wall at world x≈5 (cell col 10), spanning all rows
        for cy in 0..20 {
            g.set(10, cy, Cell::Occupied);
        }
        g
    }

    #[test]
    fn distance_is_zero_on_an_obstacle_and_grows_away() {
        let g = walled_grid();
        let f = LikelihoodField::from_grid(&g, 5.0);
        let on_wall = f.distance_at(5.1, 2.0); // on the wall column
        let near = f.distance_at(4.0, 2.0); // ~1 m from the wall
        let far = f.distance_at(1.0, 2.0); // ~4 m from the wall
        assert!(on_wall < 0.6, "near-zero on the wall, got {on_wall}");
        assert!(near > on_wall && far > near, "distance grows away: {on_wall} < {near} < {far}");
    }

    #[test]
    fn distance_saturates_outside_the_grid() {
        let g = walled_grid();
        let f = LikelihoodField::from_grid(&g, 3.0);
        assert_eq!(f.distance_at(-100.0, -100.0), 3.0);
    }

    #[test]
    fn beam_likelihood_peaks_at_zero_distance() {
        let p = BeamModelParams::default();
        assert!(p.beam_likelihood(0.0) > p.beam_likelihood(0.5));
        assert!(p.beam_likelihood(0.5) > p.beam_likelihood(2.0));
        assert!(p.beam_likelihood(100.0) > 0.0, "floored by z_rand");
    }

    #[test]
    fn correct_pose_explains_the_scan_better_than_a_wrong_one() {
        let g = walled_grid();
        let f = LikelihoodField::from_grid(&g, 5.0);
        let params = BeamModelParams { sigma_hit: 0.3, z_hit: 0.95, z_rand: 0.05, max_range: 20.0 };

        // Truth: robot at (2,5) facing east (+x); the wall is ~3 m ahead at x≈5.
        // A single forward beam of range 3 lands on the wall.
        let beams = [(0.0, 3.0)];
        let truth = Pose2::new(2.0, 5.0, 0.0);
        let wrong = Pose2::new(2.0, 5.0, std::f64::consts::PI); // facing away → endpoint in open space

        let l_true = f.scan_log_likelihood(truth, &beams, &params);
        let l_wrong = f.scan_log_likelihood(wrong, &beams, &params);
        assert!(l_true > l_wrong, "correct heading scores higher: {l_true} > {l_wrong}");
    }
}

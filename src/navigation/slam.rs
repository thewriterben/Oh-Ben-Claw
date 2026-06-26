//! Pose-graph SLAM back-end — 2D (SE2) trajectory optimization with loop closure.
//!
//! Odometry drifts: integrate enough relative motions and the estimated
//! trajectory bends away from truth. **Loop closure** is the fix — when the robot
//! recognizes a place it has visited before, that adds a constraint tying a late
//! pose back to an early one, and re-optimizing the whole **pose graph**
//! distributes the accumulated drift around the loop so the trajectory closes.
//!
//! This is a real (if compact) pose-graph back-end: nodes are poses, edges are
//! relative-transform constraints (odometry between consecutive poses, plus
//! loop-closure edges), and [`PoseGraph::optimize`] runs an anchored Gauss-Seidel
//! relaxation that monotonically reduces the total squared edge error. It is not
//! a full sparse Gauss-Newton solver, but it converges on consistent constraints
//! and is the mechanism loop closure needs. Place recognition (the *front end*
//! that proposes closures) is approximated here by spatial proximity
//! ([`PoseGraph::find_revisit`]); a real system would use scan/feature matching.

use crate::memory::world::WorldMemory;
use serde_json::json;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex};

/// Wrap an angle (radians) to `(-π, π]`.
fn norm_angle(a: f64) -> f64 {
    let mut x = (a + PI).rem_euclid(2.0 * PI) - PI;
    if x <= -PI {
        x += 2.0 * PI;
    }
    x
}

/// A 2D pose (SE2): position + heading (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose2 {
    pub x: f64,
    pub y: f64,
    pub theta: f64,
}

impl Pose2 {
    pub fn new(x: f64, y: f64, theta: f64) -> Self {
        Self { x, y, theta: norm_angle(theta) }
    }
}

/// A relative transform between two poses, expressed in the source pose's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelPose {
    pub dx: f64,
    pub dy: f64,
    pub dtheta: f64,
}

impl RelPose {
    pub fn new(dx: f64, dy: f64, dtheta: f64) -> Self {
        Self { dx, dy, dtheta }
    }
    /// The identity transform (used for "I am exactly back at that place").
    pub fn identity() -> Self {
        Self { dx: 0.0, dy: 0.0, dtheta: 0.0 }
    }
}

/// `a ⊕ rel` — apply a relative transform (in `a`'s frame) to get a world pose.
pub fn compose(a: Pose2, rel: RelPose) -> Pose2 {
    let (s, c) = a.theta.sin_cos();
    Pose2 {
        x: a.x + c * rel.dx - s * rel.dy,
        y: a.y + s * rel.dx + c * rel.dy,
        theta: norm_angle(a.theta + rel.dtheta),
    }
}

/// `a⁻¹ ⊕ b` — the relative transform from `a` to `b`, in `a`'s frame.
pub fn relative_between(a: Pose2, b: Pose2) -> RelPose {
    let (s, c) = a.theta.sin_cos();
    let ddx = b.x - a.x;
    let ddy = b.y - a.y;
    RelPose {
        dx: c * ddx + s * ddy,
        dy: -s * ddx + c * ddy,
        dtheta: norm_angle(b.theta - a.theta),
    }
}

/// A relative-transform constraint between two nodes.
#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub meas: RelPose,
    pub weight: f64,
    /// `true` for a loop-closure edge (vs. sequential odometry).
    pub loop_closure: bool,
}

/// A 2D pose graph: a trajectory of poses tied by relative constraints. Node 0 is
/// the fixed anchor (it removes the global gauge freedom).
#[derive(Debug, Clone, Default)]
pub struct PoseGraph {
    nodes: Vec<Pose2>,
    edges: Vec<Edge>,
}

impl PoseGraph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// A graph seeded with an anchor pose (node 0).
    pub fn with_anchor(start: Pose2) -> Self {
        Self { nodes: vec![start], edges: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn node(&self, i: usize) -> Pose2 {
        self.nodes[i]
    }
    pub fn nodes(&self) -> &[Pose2] {
        &self.nodes
    }
    pub fn latest(&self) -> Option<Pose2> {
        self.nodes.last().copied()
    }
    pub fn loop_closures(&self) -> usize {
        self.edges.iter().filter(|e| e.loop_closure).count()
    }

    /// Add a free-standing node, returning its id.
    pub fn add_node(&mut self, pose: Pose2) -> usize {
        self.nodes.push(pose);
        self.nodes.len() - 1
    }

    /// Add an odometry edge between consecutive (or any) nodes.
    pub fn add_odometry(&mut self, from: usize, to: usize, meas: RelPose, weight: f64) {
        self.edges.push(Edge { from, to, meas, weight, loop_closure: false });
    }

    /// Add a loop-closure constraint between two nodes.
    pub fn add_loop_closure(&mut self, from: usize, to: usize, meas: RelPose, weight: f64) {
        self.edges.push(Edge { from, to, meas, weight, loop_closure: true });
    }

    /// Extend the trajectory by a relative motion: append a node at
    /// `compose(latest, rel)` and an odometry edge to it. Returns the new id.
    pub fn append_motion(&mut self, rel: RelPose, weight: f64) -> usize {
        let last = self.nodes.len() - 1;
        let pose = compose(self.nodes[last], rel);
        let id = self.add_node(pose);
        self.add_odometry(last, id, rel, weight);
        id
    }

    /// Propose a loop closure for the latest node: the earliest node within
    /// `radius` that is at least `min_gap` steps back. Approximates place
    /// recognition by spatial proximity in the current estimate.
    pub fn find_revisit(&self, radius: f64, min_gap: usize) -> Option<usize> {
        let last = self.nodes.len().checked_sub(1)?;
        if last < min_gap {
            return None;
        }
        let cur = self.nodes[last];
        for j in 0..=(last - min_gap) {
            let p = self.nodes[j];
            if (p.x - cur.x).hypot(p.y - cur.y) <= radius {
                return Some(j);
            }
        }
        None
    }

    /// Total weighted squared error across all edges (the quantity `optimize`
    /// reduces). Lower is a more self-consistent trajectory.
    pub fn total_error(&self) -> f64 {
        let mut sum = 0.0;
        for e in &self.edges {
            let pred = compose(self.nodes[e.from], e.meas);
            let to = self.nodes[e.to];
            let ex = pred.x - to.x;
            let ey = pred.y - to.y;
            let eth = norm_angle(pred.theta - to.theta);
            sum += e.weight * (ex * ex + ey * ey + eth * eth);
        }
        sum
    }

    fn nudge(&mut self, i: usize, dx: f64, dy: f64, dth: f64) {
        if i == 0 {
            return; // anchor is fixed
        }
        let p = &mut self.nodes[i];
        p.x += dx;
        p.y += dy;
        p.theta = norm_angle(p.theta + dth);
    }

    /// **Gauss-Newton** least-squares optimization (the SOTA pose-graph approach,
    /// à la g2o/Ceres/SPA): linearizes each edge with analytic SE2 Jacobians,
    /// assembles the normal equations `H Δ = −b`, fixes node 0 as the anchor,
    /// solves densely, and applies the update — repeated for `iters` steps.
    /// Converges far faster and more accurately than the relaxation below.
    /// Returns the final total error.
    pub fn optimize_gn(&mut self, iters: usize) -> f64 {
        let n = self.nodes.len();
        if n == 0 {
            return 0.0;
        }
        let dim = 3 * n;
        for _ in 0..iters {
            let mut h = vec![vec![0.0f64; dim]; dim];
            let mut b = vec![0.0f64; dim];
            for e in &self.edges {
                let (a, bj, err) = linearize_edge(self.nodes[e.from], self.nodes[e.to], e.meas);
                let w = e.weight.max(1e-6);
                let at = transpose3(&a);
                let bt = transpose3(&bj);
                // H blocks (scaled by the scalar information weight)
                accum_block(&mut h, 3 * e.from, 3 * e.from, &matmul3(&at, &a), w);
                accum_block(&mut h, 3 * e.from, 3 * e.to, &matmul3(&at, &bj), w);
                accum_block(&mut h, 3 * e.to, 3 * e.from, &matmul3(&bt, &a), w);
                accum_block(&mut h, 3 * e.to, 3 * e.to, &matmul3(&bt, &bj), w);
                // b blocks
                let ate = matvec3(&at, &err);
                let bte = matvec3(&bt, &err);
                for k in 0..3 {
                    b[3 * e.from + k] += w * ate[k];
                    b[3 * e.to + k] += w * bte[k];
                }
            }
            // Anchor node 0: pin its update to zero (removes the gauge freedom).
            for r in 0..3 {
                for c in 0..dim {
                    h[r][c] = 0.0;
                    h[c][r] = 0.0;
                }
                h[r][r] = 1.0;
                b[r] = 0.0;
            }
            // Solve H Δ = −b.
            let rhs: Vec<f64> = b.iter().map(|v| -v).collect();
            let Some(delta) = gauss_solve(h, rhs) else {
                break; // singular (under-constrained) — stop rather than diverge
            };
            for k in 0..n {
                self.nodes[k].x += delta[3 * k];
                self.nodes[k].y += delta[3 * k + 1];
                self.nodes[k].theta = norm_angle(self.nodes[k].theta + delta[3 * k + 2]);
            }
        }
        self.total_error()
    }

    /// Anchored Gauss-Seidel relaxation: for `iters` passes, pull each edge's
    /// endpoints toward satisfying their constraint (learning rate `alpha`,
    /// node 0 fixed). Monotonically reduces [`total_error`](Self::total_error) for
    /// small `alpha`. Returns the final total error. (Simpler/cheaper than
    /// [`optimize_gn`](Self::optimize_gn); kept for comparison.)
    pub fn optimize(&mut self, iters: usize, alpha: f64) -> f64 {
        for _ in 0..iters {
            for k in 0..self.edges.len() {
                let e = self.edges[k];
                let pred = compose(self.nodes[e.from], e.meas);
                let to = self.nodes[e.to];
                let ex = pred.x - to.x;
                let ey = pred.y - to.y;
                let eth = norm_angle(pred.theta - to.theta);
                let g = (alpha * e.weight).min(0.5);
                // Move `to` toward where `from`+meas predicts it, and `from` the
                // opposite way, so they meet in the middle (anchor excepted).
                self.nudge(e.to, g * ex, g * ey, g * eth);
                self.nudge(e.from, -g * ex, -g * ey, -g * eth);
            }
        }
        self.total_error()
    }
}

// ── Gauss-Newton linear algebra (SE2) ──────────────────────────────────────────

type Mat3 = [[f64; 3]; 3];
type Vec3 = [f64; 3];
type Mat2 = [[f64; 2]; 2];

fn mul2(a: &Mat2, b: &Mat2) -> Mat2 {
    [
        [a[0][0] * b[0][0] + a[0][1] * b[1][0], a[0][0] * b[0][1] + a[0][1] * b[1][1]],
        [a[1][0] * b[0][0] + a[1][1] * b[1][0], a[1][0] * b[0][1] + a[1][1] * b[1][1]],
    ]
}
fn mv2(m: &Mat2, v: &[f64; 2]) -> [f64; 2] {
    [m[0][0] * v[0] + m[0][1] * v[1], m[1][0] * v[0] + m[1][1] * v[1]]
}
fn transpose3(m: &Mat3) -> Mat3 {
    let mut t = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            t[i][j] = m[j][i];
        }
    }
    t
}
fn matmul3(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                o[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    o
}
fn matvec3(m: &Mat3, v: &Vec3) -> Vec3 {
    let mut o = [0.0; 3];
    for i in 0..3 {
        for k in 0..3 {
            o[i] += m[i][k] * v[k];
        }
    }
    o
}
fn accum_block(h: &mut [Vec<f64>], r0: usize, c0: usize, block: &Mat3, w: f64) {
    for i in 0..3 {
        for j in 0..3 {
            h[r0 + i][c0 + j] += w * block[i][j];
        }
    }
}

/// Analytic linearization of an edge `i→j` with measurement `z`: returns the
/// Jacobians `(A = ∂e/∂x_i, B = ∂e/∂x_j)` and the error `e` (Grisetti SE2 form).
fn linearize_edge(xi: Pose2, xj: Pose2, z: RelPose) -> (Mat3, Mat3, Vec3) {
    let (si, ci) = xi.theta.sin_cos();
    let (sz, cz) = z.dtheta.sin_cos();
    let rit: Mat2 = [[ci, si], [-si, ci]]; // R_i^T
    let rzt: Mat2 = [[cz, sz], [-sz, cz]]; // R_z^T
    let drit: Mat2 = [[-si, ci], [-ci, -si]]; // ∂R_i^T/∂θ_i
    let d = [xj.x - xi.x, xj.y - xi.y];
    let m = mul2(&rzt, &rit); // R_z^T R_i^T

    let rit_d = mv2(&rit, &d);
    let e_xy = mv2(&rzt, &[rit_d[0] - z.dx, rit_d[1] - z.dy]);
    let e_th = norm_angle(xj.theta - xi.theta - z.dtheta);

    let dd = mv2(&drit, &d);
    let col = mv2(&rzt, &dd);
    let a: Mat3 = [
        [-m[0][0], -m[0][1], col[0]],
        [-m[1][0], -m[1][1], col[1]],
        [0.0, 0.0, -1.0],
    ];
    let b: Mat3 = [[m[0][0], m[0][1], 0.0], [m[1][0], m[1][1], 0.0], [0.0, 0.0, 1.0]];
    (a, b, [e_xy[0], e_xy[1], e_th])
}

/// Dense linear solve `A x = b` by Gaussian elimination with partial pivoting.
/// `None` if singular.
fn gauss_solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let mut piv = col;
        let mut best = a[col][col].abs();
        for r in (col + 1)..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                piv = r;
            }
        }
        if best < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivval = a[col][col];
        for r in (col + 1)..n {
            let factor = a[r][col] / pivval;
            if factor != 0.0 {
                for c in col..n {
                    a[r][c] -= factor * a[col][c];
                }
                b[r] -= factor * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = b[i];
        for c in (i + 1)..n {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

/// Online pose-graph SLAM: ingest relative motions, auto-propose loop closures by
/// proximity, optimize on closure, and record the corrected current pose into
/// world memory (`sensor.pos_*` + `nav.slam`) — so the navigation localizer reads
/// the *drift-corrected* pose, not the raw odometry.
pub struct SlamBackend {
    graph: Mutex<PoseGraph>,
    world: Option<Arc<WorldMemory>>,
    loop_radius: f64,
    min_gap: usize,
    opt_iters: usize,
    alpha: f64,
    source: String,
}

impl SlamBackend {
    /// Start a backend anchored at `start`.
    pub fn new(start: Pose2) -> Self {
        Self {
            graph: Mutex::new(PoseGraph::with_anchor(start)),
            world: None,
            loop_radius: 2.0,
            min_gap: 3,
            opt_iters: 200,
            alpha: 0.1,
            source: "slam".to_string(),
        }
    }

    /// Record corrected poses into world memory (degrees heading, localizer-ready).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Loop-closure proximity radius and minimum node gap before a revisit counts.
    pub fn with_params(mut self, loop_radius: f64, min_gap: usize) -> Self {
        self.loop_radius = loop_radius;
        self.min_gap = min_gap;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PoseGraph> {
        self.graph.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The current (possibly corrected) latest pose.
    pub fn latest(&self) -> Pose2 {
        self.lock().latest().unwrap_or(Pose2::new(0.0, 0.0, 0.0))
    }

    /// Ingest a relative motion: append a node, detect+add a loop closure by
    /// proximity, optimize if closed, and record the corrected pose. Returns
    /// `true` when a loop closure fired this step.
    pub fn add_motion(&self, rel: RelPose, now_ms: u64) -> anyhow::Result<bool> {
        let (closed, latest, nodes, loops) = {
            let mut g = self.lock();
            let id = g.append_motion(rel, 1.0);
            let closed = if let Some(j) = g.find_revisit(self.loop_radius, self.min_gap) {
                g.add_loop_closure(id, j, RelPose::identity(), 2.0);
                // Gauss-Newton converges in a handful of iterations.
                g.optimize_gn(self.opt_iters.min(20));
                true
            } else {
                false
            };
            (closed, g.latest().unwrap(), g.len(), g.loop_closures())
        };
        if let Some(world) = &self.world {
            world.observe("sensor.pos_x", json!({ "value": latest.x }), now_ms, now_ms, &self.source)?;
            world.observe("sensor.pos_y", json!({ "value": latest.y }), now_ms, now_ms, &self.source)?;
            world.observe(
                "sensor.heading",
                json!({ "value": latest.theta.to_degrees() }),
                now_ms,
                now_ms,
                &self.source,
            )?;
            world.observe(
                "nav.slam",
                json!({
                    "nodes": nodes,
                    "loop_closures": loops,
                    "x": latest.x,
                    "y": latest.y,
                    "heading_deg": latest.theta.to_degrees(),
                }),
                now_ms,
                now_ms,
                &self.source,
            )?;
        }
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn compose_and_relative_roundtrip() {
        let a = Pose2::new(1.0, 2.0, 0.5);
        let b = Pose2::new(4.0, -1.0, -1.2);
        let rel = relative_between(a, b);
        let b2 = compose(a, rel);
        assert!(approx(b2.x, b.x, 1e-9) && approx(b2.y, b.y, 1e-9));
        assert!(approx(norm_angle(b2.theta - b.theta), 0.0, 1e-9));
    }

    #[test]
    fn identity_compose_is_noop() {
        let a = Pose2::new(3.0, 4.0, 1.0);
        let b = compose(a, RelPose::identity());
        assert!(approx(a.x, b.x, 1e-12) && approx(a.y, b.y, 1e-12) && approx(a.theta, b.theta, 1e-12));
    }

    #[test]
    fn angle_normalization_wraps() {
        assert!(approx(norm_angle(3.0 * PI), PI, 1e-9) || approx(norm_angle(3.0 * PI), -PI, 1e-9));
        assert!(approx(norm_angle(-3.0 * PI), PI, 1e-9) || approx(norm_angle(-3.0 * PI), -PI, 1e-9));
        assert!(approx(norm_angle(0.5), 0.5, 1e-12));
    }

    /// Build a unit-square loop with drift on the first leg, then close the loop.
    fn drifting_square() -> PoseGraph {
        let mut g = PoseGraph::with_anchor(Pose2::new(0.0, 0.0, 0.0));
        // Perfect square motion = forward 10, turn +90°. Inject drift on leg 1.
        let turn = PI / 2.0;
        g.append_motion(RelPose::new(11.0, 0.0, turn), 1.0); // drifted (11 not 10)
        g.append_motion(RelPose::new(10.0, 0.0, turn), 1.0);
        g.append_motion(RelPose::new(10.0, 0.0, turn), 1.0);
        g.append_motion(RelPose::new(10.0, 0.0, turn), 1.0);
        g
    }

    #[test]
    fn loop_closure_corrects_drift_and_reduces_error() {
        let mut g = drifting_square();
        let last = g.len() - 1;
        // Without closure the trajectory does not return to the origin.
        let before = g.node(last);
        let residual_before = before.x.hypot(before.y);
        assert!(residual_before > 0.5, "expected drift, got {residual_before}");

        // Loop closure: the robot recognizes it is back at the start (node 0),
        // i.e. node `last` coincides with node 0 (identity relative transform).
        g.add_loop_closure(last, 0, RelPose::identity(), 2.0);
        let err_before = g.total_error();
        g.optimize(500, 0.1);
        let err_after = g.total_error();

        assert!(err_after < err_before, "optimization must reduce error");
        let after = g.node(last);
        let residual_after = after.x.hypot(after.y);
        assert!(
            residual_after < residual_before * 0.25,
            "loop should close: {residual_before} → {residual_after}"
        );
        // The anchor never moves.
        assert_eq!(g.node(0), Pose2::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn gauss_newton_closes_the_loop_accurately() {
        let mut g = drifting_square();
        let last = g.len() - 1;
        let before = g.node(last);
        let residual_before = before.x.hypot(before.y);
        assert!(residual_before > 0.5, "expected drift, got {residual_before}");

        g.add_loop_closure(last, 0, RelPose::identity(), 2.0);
        let err_before = g.total_error();
        g.optimize_gn(20);
        let err_after = g.total_error();

        assert!(err_after < err_before, "GN must reduce error: {err_before} → {err_after}");
        let after = g.node(last);
        let residual_after = after.x.hypot(after.y);
        // Gauss-Newton closes the loop tightly (far better than relaxation).
        assert!(residual_after < 0.2, "loop should close near-perfectly: {residual_before} → {residual_after}");
        assert_eq!(g.node(0), Pose2::new(0.0, 0.0, 0.0), "anchor never moves");
    }

    #[test]
    fn find_revisit_detects_proximity() {
        let mut g = PoseGraph::with_anchor(Pose2::new(0.0, 0.0, 0.0));
        g.append_motion(RelPose::new(10.0, 0.0, 0.0), 1.0); // (10,0)
        g.append_motion(RelPose::new(10.0, 0.0, 0.0), 1.0); // (20,0)
        // a long way out — no revisit near the start
        assert!(g.find_revisit(2.0, 1).is_none());
        // come back near the origin
        g.append_motion(RelPose::new(-21.0, 0.0, 0.0), 1.0); // ≈(-1,0) near node 0
        assert_eq!(g.find_revisit(2.0, 2), Some(0));
    }

    #[test]
    fn backend_closes_loop_and_records_corrected_pose() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let slam = SlamBackend::new(Pose2::new(0.0, 0.0, 0.0))
            .with_world_memory(Arc::clone(&world))
            .with_params(2.0, 3);
        let turn = PI / 2.0;
        // drifting unit-square loop that returns near the origin
        slam.add_motion(RelPose::new(11.0, 0.0, turn), 1).unwrap();
        slam.add_motion(RelPose::new(10.0, 0.0, turn), 2).unwrap();
        slam.add_motion(RelPose::new(10.0, 0.0, turn), 3).unwrap();
        let closed = slam.add_motion(RelPose::new(10.0, 0.0, turn), 4).unwrap();
        assert!(closed, "returning near the origin should trigger a loop closure");

        // after closure the corrected latest pose is back near the origin
        let p = slam.latest();
        assert!(p.x.hypot(p.y) < 1.0, "loop should close, got {p:?}");

        // a drift-corrected, localizer-ready pose was recorded
        let slam_fact = world.current("nav.slam").unwrap().unwrap();
        assert!(slam_fact.value["loop_closures"].as_u64().unwrap() >= 1);
        assert!(world.current("sensor.pos_x").unwrap().is_some());
        assert!(world.current("sensor.heading").unwrap().is_some());
    }
}

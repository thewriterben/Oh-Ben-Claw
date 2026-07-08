//! Grid deployment / coverage optimizer — Conservation Grid **G1**.
//!
//! Given a [`Site`](crate::geo::Site) (boundary polygon + origin) and a node budget, this
//! places camera/sensor nodes on a lattice to **maximize detection coverage** of the area
//! while respecting a minimum node spacing and keeping the nodes **mesh-connected**. It is
//! the piece that turns "here's a 40-hectare reserve and 12 nodes" into an actual layout.
//!
//! ## Method
//! Everything runs in the site's local ENU metric frame (`geo::GeoFrame`) so distances are
//! metres, not degrees:
//! 1. Sample the polygon interior with two lattices — coarse **candidate** positions and a
//!    finer set of **demand points** (the area to cover).
//! 2. Greedy maximum-coverage: repeatedly place the candidate that newly covers the most
//!    demand points (each node covers demand within `detection_radius_m`), subject to
//!    `min_spacing_m` from already-placed nodes and — when `require_mesh_connectivity` —
//!    being within `mesh_range_m` of the placed set. Stop at the budget or when no
//!    placement adds coverage.
//!
//! Greedy set-cover is the standard, well-behaved approximation here (coverage is
//! submodular, so greedy is within `1 - 1/e` of optimal). Deterministic: candidate order is
//! fixed and ties break to the first. Output positions carry both ENU and geodetic
//! coordinates (via `GeoFrame::from_enu`) so they feed straight into `deployment` codegen.
//!
//! This reuses the same spacing geometry idea as `fleet` conflict-avoidance and the same
//! ENU frame as `navigation`, and depends only on `crate::geo`.

use crate::geo::{Enu, GeoPoint, Site};
use serde::{Deserialize, Serialize};

/// Tuning for a placement run. Distances are metres.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlacementSpec {
    /// Number of nodes to place (upper bound; may place fewer if coverage saturates).
    pub budget: usize,
    /// Radius within which a node covers demand (effective detection range).
    pub detection_radius_m: f64,
    /// Minimum distance enforced between any two placed nodes.
    pub min_spacing_m: f64,
    /// Two nodes are mesh-linked when within this range.
    pub mesh_range_m: f64,
    /// Spacing of the coarse candidate-position lattice.
    pub lattice_step_m: f64,
    /// Spacing of the fine demand-point lattice (coverage resolution).
    pub demand_step_m: f64,
    /// Require each node (after the first) to be within `mesh_range_m` of the placed set.
    pub require_mesh_connectivity: bool,
}

impl Default for PlacementSpec {
    fn default() -> Self {
        Self {
            budget: 8,
            detection_radius_m: 30.0,
            min_spacing_m: 20.0,
            mesh_range_m: 250.0,
            lattice_step_m: 10.0,
            demand_step_m: 10.0,
            require_mesh_connectivity: true,
        }
    }
}

/// A single placed node with both local and geodetic coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlacedNode {
    pub enu: Enu,
    pub geo: GeoPoint,
    /// How many demand points this node covers (independent of overlap).
    pub covers: usize,
}

/// The result of a placement run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SitePlan {
    pub nodes: Vec<PlacedNode>,
    /// Fraction of demand points covered by at least one node (0..1).
    pub coverage_fraction: f64,
    pub demand_points: usize,
    pub covered_points: usize,
    /// Whether the placed nodes form a single connected mesh graph (≤1 node = true).
    pub mesh_connected: bool,
}

fn point_in_polygon(x: f64, y: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Sample the polygon interior on a lattice of the given step (ENU metres).
fn lattice(poly: &[(f64, f64)], e0: f64, e1: f64, n0: f64, n1: f64, step: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if step <= 0.0 {
        return out;
    }
    let mut e = e0;
    while e <= e1 {
        let mut n = n0;
        while n <= n1 {
            if point_in_polygon(e, n, poly) {
                out.push((e, n));
            }
            n += step;
        }
        e += step;
    }
    out
}

fn mesh_connected(nodes: &[(f64, f64)], range: f64) -> bool {
    if nodes.len() <= 1 {
        return true;
    }
    let mut seen = vec![false; nodes.len()];
    let mut stack = vec![0usize];
    seen[0] = true;
    let mut count = 1;
    while let Some(u) = stack.pop() {
        for v in 0..nodes.len() {
            if !seen[v] && dist(nodes[u], nodes[v]) <= range {
                seen[v] = true;
                count += 1;
                stack.push(v);
            }
        }
    }
    count == nodes.len()
}

/// Optimize node placement over a site. See the module docs for the method.
pub fn plan_site(site: &Site, spec: &PlacementSpec) -> SitePlan {
    let frame = site.frame();
    let poly: Vec<(f64, f64)> = site
        .boundary
        .iter()
        .map(|p| {
            let e = frame.to_enu(*p);
            (e.e, e.n)
        })
        .collect();

    let empty = SitePlan {
        nodes: Vec::new(),
        coverage_fraction: 0.0,
        demand_points: 0,
        covered_points: 0,
        mesh_connected: true,
    };
    if poly.len() < 3 || spec.budget == 0 {
        return empty;
    }

    let e0 = poly.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let e1 = poly.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let n0 = poly.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let n1 = poly.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);

    let demand = lattice(&poly, e0, e1, n0, n1, spec.demand_step_m);
    let cands = lattice(&poly, e0, e1, n0, n1, spec.lattice_step_m);
    if demand.is_empty() || cands.is_empty() {
        return empty;
    }

    // Precompute the demand indices each candidate covers.
    let r2 = spec.detection_radius_m * spec.detection_radius_m;
    let covers: Vec<Vec<usize>> = cands
        .iter()
        .map(|c| {
            demand
                .iter()
                .enumerate()
                .filter(|(_, d)| (c.0 - d.0).powi(2) + (c.1 - d.1).powi(2) <= r2)
                .map(|(i, _)| i)
                .collect()
        })
        .collect();

    let mut covered = vec![false; demand.len()];
    let mut chosen: Vec<(f64, f64)> = Vec::new();
    let mut nodes: Vec<PlacedNode> = Vec::new();

    for _ in 0..spec.budget {
        let mut best: Option<usize> = None;
        let mut best_gain = 0usize;
        for (ci, c) in cands.iter().enumerate() {
            if chosen.iter().any(|&ch| dist(*c, ch) < spec.min_spacing_m) {
                continue;
            }
            if spec.require_mesh_connectivity
                && !chosen.is_empty()
                && !chosen.iter().any(|&ch| dist(*c, ch) <= spec.mesh_range_m)
            {
                continue;
            }
            let gain = covers[ci].iter().filter(|&&d| !covered[d]).count();
            if gain > best_gain {
                best_gain = gain;
                best = Some(ci);
            }
        }
        match best {
            Some(ci) if best_gain > 0 => {
                for &d in &covers[ci] {
                    covered[d] = true;
                }
                let (e, n) = cands[ci];
                let enu = Enu::new(e, n, 0.0);
                nodes.push(PlacedNode {
                    enu,
                    geo: frame.from_enu(enu),
                    covers: covers[ci].len(),
                });
                chosen.push(cands[ci]);
            }
            _ => break,
        }
    }

    let covered_points = covered.iter().filter(|&&c| c).count();
    SitePlan {
        coverage_fraction: covered_points as f64 / demand.len() as f64,
        demand_points: demand.len(),
        covered_points,
        mesh_connected: mesh_connected(&chosen, spec.mesh_range_m),
        nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::{GeoPoint, Site};

    /// A ~100 m × 100 m square site near (45.5, -122.6).
    fn square_site() -> Site {
        // ~0.00045 deg lat ≈ 50 m; lon scaled by cos(45.5).
        let dlat = 50.0 / 111_194.9;
        let dlon = 50.0 / (111_194.9 * 45.5_f64.to_radians().cos());
        let (lat, lon) = (45.5, -122.6);
        Site::new(
            "s1",
            "square",
            vec![
                GeoPoint::new(lat - dlat, lon - dlon, 0.0),
                GeoPoint::new(lat - dlat, lon + dlon, 0.0),
                GeoPoint::new(lat + dlat, lon + dlon, 0.0),
                GeoPoint::new(lat + dlat, lon - dlon, 0.0),
            ],
        )
    }

    fn spec(budget: usize) -> PlacementSpec {
        PlacementSpec { budget, ..Default::default() }
    }

    #[test]
    fn empty_boundary_yields_empty_plan() {
        let site = Site::new("e", "", vec![]);
        let plan = plan_site(&site, &spec(5));
        assert!(plan.nodes.is_empty());
        assert_eq!(plan.coverage_fraction, 0.0);
    }

    #[test]
    fn zero_budget_places_nothing() {
        let plan = plan_site(&square_site(), &spec(0));
        assert!(plan.nodes.is_empty());
    }

    #[test]
    fn respects_budget_and_spacing() {
        let s = spec(4);
        let plan = plan_site(&square_site(), &s);
        assert!(plan.nodes.len() <= 4);
        for i in 0..plan.nodes.len() {
            for j in (i + 1)..plan.nodes.len() {
                let a = (plan.nodes[i].enu.e, plan.nodes[i].enu.n);
                let b = (plan.nodes[j].enu.e, plan.nodes[j].enu.n);
                assert!(dist(a, b) >= s.min_spacing_m - 1e-6, "spacing violated");
            }
        }
    }

    #[test]
    fn all_nodes_inside_polygon() {
        let site = square_site();
        let frame = site.frame();
        let poly: Vec<(f64, f64)> = site
            .boundary
            .iter()
            .map(|p| { let e = frame.to_enu(*p); (e.e, e.n) })
            .collect();
        for node in plan_site(&site, &spec(4)).nodes {
            assert!(point_in_polygon(node.enu.e, node.enu.n, &poly));
        }
    }

    #[test]
    fn more_budget_never_reduces_coverage() {
        let c1 = plan_site(&square_site(), &spec(1)).coverage_fraction;
        let c4 = plan_site(&square_site(), &spec(4)).coverage_fraction;
        assert!(c4 >= c1, "c4={} c1={}", c4, c1);
        assert!(c1 > 0.0);
    }

    #[test]
    fn placement_is_deterministic() {
        let a = plan_site(&square_site(), &spec(4));
        let b = plan_site(&square_site(), &spec(4));
        let ax: Vec<_> = a.nodes.iter().map(|n| (n.enu.e, n.enu.n)).collect();
        let bx: Vec<_> = b.nodes.iter().map(|n| (n.enu.e, n.enu.n)).collect();
        assert_eq!(ax, bx);
    }

    #[test]
    fn nodes_are_mesh_connected_with_ample_range() {
        // Default mesh_range 250 m ≫ 100 m site → the placed set is connected.
        let plan = plan_site(&square_site(), &spec(4));
        assert!(plan.mesh_connected);
        assert!(plan.nodes.len() >= 2);
    }

    #[test]
    fn placed_nodes_carry_geodetic_coords_near_site() {
        let plan = plan_site(&square_site(), &spec(2));
        for node in plan.nodes {
            assert!((node.geo.lat - 45.5).abs() < 0.01);
            assert!((node.geo.lon + 122.6).abs() < 0.01);
        }
    }
}

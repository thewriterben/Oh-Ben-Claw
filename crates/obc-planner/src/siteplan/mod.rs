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
    /// Mast/antenna height of a node above the terrain, used for line-of-sight when a
    /// terrain heightfield is supplied (ignored on flat/None terrain).
    pub node_height_m: f64,
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
            node_height_m: 3.0,
        }
    }
}

/// A terrain surface sampled on a regular ENU grid, for line-of-sight occlusion.
///
/// ``data`` is row-major (`data[r * cols + c]`) giving the elevation at ENU
/// `(origin_e + c*step, origin_n + r*step)`. Sampling is bilinear, clamped at the edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heightfield {
    pub origin_e: f64,
    pub origin_n: f64,
    pub step: f64,
    pub cols: usize,
    pub rows: usize,
    pub data: Vec<f64>,
}

impl Heightfield {
    /// Build from row-major elevation rows (`rows[r][c]`); each row must have the same
    /// length. Empty input yields a 0×0 field (elevation always 0).
    pub fn from_rows(origin_e: f64, origin_n: f64, step: f64, rows: Vec<Vec<f64>>) -> Self {
        let r = rows.len();
        let c = rows.first().map_or(0, |row| row.len());
        let mut data = Vec::with_capacity(r * c);
        for row in &rows {
            data.extend_from_slice(row);
        }
        Self {
            origin_e,
            origin_n,
            step: if step > 0.0 { step } else { 1.0 },
            cols: c,
            rows: r,
            data,
        }
    }

    /// Bilinearly-interpolated elevation at ENU `(e, n)`, clamped to the grid edges.
    pub fn elevation(&self, e: f64, n: f64) -> f64 {
        if self.cols == 0 || self.rows == 0 {
            return 0.0;
        }
        let fc = (e - self.origin_e) / self.step;
        let fr = (n - self.origin_n) / self.step;
        let c0 = (fc.floor() as isize).clamp(0, self.cols as isize - 1) as usize;
        let r0 = (fr.floor() as isize).clamp(0, self.rows as isize - 1) as usize;
        let c1 = (c0 + 1).min(self.cols - 1);
        let r1 = (r0 + 1).min(self.rows - 1);
        let tc = (fc - c0 as f64).clamp(0.0, 1.0);
        let tr = (fr - r0 as f64).clamp(0.0, 1.0);
        let at = |r: usize, c: usize| self.data[r * self.cols + c];
        let top = at(r0, c0) + (at(r0, c1) - at(r0, c0)) * tc;
        let bot = at(r1, c0) + (at(r1, c1) - at(r1, c0)) * tc;
        top + (bot - top) * tr
    }
}

/// Whether the straight line from `(ax, ay, az)` to `(bx, by, bz)` clears the terrain —
/// true if no interior sample of the ground rises above the sight ray.
#[allow(clippy::too_many_arguments)] // two 3D endpoints + terrain + sampling: inherent arity
fn los_clear(
    hf: &Heightfield,
    ax: f64,
    ay: f64,
    az: f64,
    bx: f64,
    by: f64,
    bz: f64,
    samples: usize,
) -> bool {
    let n = samples.max(2);
    for k in 1..n {
        let t = k as f64 / n as f64;
        let x = ax + (bx - ax) * t;
        let y = ay + (by - ay) * t;
        let ray_h = az + (bz - az) * t;
        if hf.elevation(x, y) > ray_h + 1e-6 {
            return false;
        }
    }
    true
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

impl SitePlan {
    /// A one-line human summary for logs / operator display.
    pub fn summary(&self) -> String {
        format!(
            "{} node(s), coverage {:.0}%, {} ({} demand pts)",
            self.nodes.len(),
            self.coverage_fraction * 100.0,
            if self.mesh_connected {
                "mesh-connected"
            } else {
                "MESH SPLIT"
            },
            self.demand_points,
        )
    }

    /// Render the placement as paste-ready TOML that slots alongside a deployment
    /// config: a `[site]` header plus one `[[site.node]]` per placed node carrying its
    /// id, geodetic `(lat, lon, alt)` and site-local ENU. The `deployment` codegen (or the
    /// generator UI) can merge this so each placed node is provisioned at its position.
    pub fn to_toml(&self, site_id: &str) -> String {
        let mut s = String::new();
        s.push_str("[site]\n");
        s.push_str(&format!("id = {:?}\n", site_id));
        s.push_str(&format!("nodes = {}\n", self.nodes.len()));
        s.push_str(&format!(
            "coverage_fraction = {:.4}\n",
            self.coverage_fraction
        ));
        s.push_str(&format!("mesh_connected = {}\n", self.mesh_connected));
        for (i, n) in self.nodes.iter().enumerate() {
            s.push_str("\n[[site.node]]\n");
            s.push_str(&format!(
                "id = {:?}\n",
                format!("{}-n{:02}", site_id, i + 1)
            ));
            s.push_str(&format!("lat = {:.6}\n", n.geo.lat));
            s.push_str(&format!("lon = {:.6}\n", n.geo.lon));
            s.push_str(&format!("alt = {:.2}\n", n.geo.alt));
            s.push_str(&format!("enu_e = {:.2}\n", n.enu.e));
            s.push_str(&format!("enu_n = {:.2}\n", n.enu.n));
            s.push_str(&format!("covers = {}\n", n.covers));
        }
        s
    }
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

/// Optimize node placement over a site (flat terrain). See the module docs for the method.
pub fn plan_site(site: &Site, spec: &PlacementSpec) -> SitePlan {
    plan_site_on(site, spec, None)
}

/// Optimize node placement, optionally honoring a terrain heightfield: a node covers a
/// demand point only when it's within range **and** the ground doesn't occlude the sight
/// line (the node's mast sits `spec.node_height_m` above the terrain). `terrain = None`
/// reproduces the flat-plane behaviour exactly.
pub fn plan_site_on(site: &Site, spec: &PlacementSpec, terrain: Option<&Heightfield>) -> SitePlan {
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

    // Precompute the demand indices each candidate covers (within range, and — with
    // terrain — not occluded by the ground along the sight line).
    let r2 = spec.detection_radius_m * spec.detection_radius_m;
    let covers: Vec<Vec<usize>> = cands
        .iter()
        .map(|c| {
            let node_z = terrain.map_or(0.0, |hf| hf.elevation(c.0, c.1)) + spec.node_height_m;
            demand
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    if (c.0 - d.0).powi(2) + (c.1 - d.1).powi(2) > r2 {
                        return false;
                    }
                    match terrain {
                        None => true,
                        Some(hf) => {
                            los_clear(hf, c.0, c.1, node_z, d.0, d.1, hf.elevation(d.0, d.1), 32)
                        }
                    }
                })
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
        PlacementSpec {
            budget,
            ..Default::default()
        }
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
            .map(|p| {
                let e = frame.to_enu(*p);
                (e.e, e.n)
            })
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

    #[test]
    fn toml_has_one_table_per_node() {
        let plan = plan_site(&square_site(), &spec(3));
        let toml = plan.to_toml("s1");
        assert_eq!(toml.matches("[[site.node]]").count(), plan.nodes.len());
        assert!(toml.contains("id = \"s1\""));
        assert!(toml.contains("id = \"s1-n01\""));
        assert!(toml.contains("mesh_connected = true"));
        assert!(toml.contains("lat = 45."));
    }

    #[test]
    fn empty_plan_toml_has_no_node_tables() {
        let toml = plan_site(&Site::new("e", "", vec![]), &spec(3)).to_toml("e");
        assert!(toml.contains("nodes = 0"));
        assert_eq!(toml.matches("[[site.node]]").count(), 0);
    }

    #[test]
    fn summary_mentions_coverage() {
        assert!(plan_site(&square_site(), &spec(4))
            .summary()
            .contains("coverage"));
    }

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn heightfield_bilinear_and_clamp() {
        let hf = Heightfield::from_rows(0.0, 0.0, 10.0, vec![vec![0.0, 10.0], vec![0.0, 10.0]]);
        assert!(approx(hf.elevation(0.0, 0.0), 0.0, 1e-9));
        assert!(approx(hf.elevation(10.0, 0.0), 10.0, 1e-9)); // east edge
        assert!(approx(hf.elevation(5.0, 0.0), 5.0, 1e-9)); // interpolated midpoint
        assert!(approx(hf.elevation(-100.0, 0.0), 0.0, 1e-9)); // clamped west
    }

    #[test]
    fn line_of_sight_ridge_blocks_low_clears() {
        // Tall ridge (20 m) at the middle column blocks a 3 m mast → ground sightline.
        let tall = Heightfield::from_rows(
            -10.0,
            -10.0,
            10.0,
            vec![
                vec![0.0, 0.0, 0.0],
                vec![0.0, 20.0, 0.0],
                vec![0.0, 0.0, 0.0],
            ],
        );
        assert!(!los_clear(&tall, -10.0, 0.0, 3.0, 10.0, 0.0, 0.0, 32));
        // A 1 m bump sits below the ~1.5 m sight ray at the midpoint → clear.
        let low = Heightfield::from_rows(
            -10.0,
            -10.0,
            10.0,
            vec![
                vec![0.0, 0.0, 0.0],
                vec![0.0, 1.0, 0.0],
                vec![0.0, 0.0, 0.0],
            ],
        );
        assert!(los_clear(&low, -10.0, 0.0, 3.0, 10.0, 0.0, 0.0, 32));
    }

    #[test]
    fn no_terrain_matches_flat_plan() {
        let site = square_site();
        let a = plan_site(&site, &spec(4));
        let b = plan_site_on(&site, &spec(4), None);
        let ax: Vec<_> = a.nodes.iter().map(|n| (n.enu.e, n.enu.n)).collect();
        let bx: Vec<_> = b.nodes.iter().map(|n| (n.enu.e, n.enu.n)).collect();
        assert_eq!(ax, bx);
        assert_eq!(a.coverage_fraction, b.coverage_fraction);
    }

    #[test]
    fn terrain_occlusion_reduces_coverage() {
        // Corrugated terrain: sharp 40 m ridges every 20 m in N, so any node's disc
        // straddles a ridge and loses the demand behind it.
        let dim = 25;
        let origin = -60.0;
        let step = 5.0;
        let mut rows = Vec::new();
        for r in 0..dim {
            let n = origin + r as f64 * step;
            let hv = if (n.round() as i64) % 20 == 0 {
                40.0
            } else {
                0.0
            };
            rows.push(vec![hv; dim]);
        }
        let hf = Heightfield::from_rows(origin, origin, step, rows);
        let site = square_site();
        let flat = plan_site(&site, &spec(4)).coverage_fraction;
        let terr = plan_site_on(&site, &spec(4), Some(&hf)).coverage_fraction;
        assert!(
            terr < flat,
            "terrain should occlude: terr={} flat={}",
            terr,
            flat
        );
    }
}

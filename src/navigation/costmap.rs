//! Costmap inflation + clearance-aware planning.
//!
//! Plain A* over a binary grid will hug walls and try to squeeze through gaps too
//! tight for the robot. Real stacks (Nav2's inflation layer) fix this by spreading
//! a **graded cost** outward from every obstacle: cells within the robot's
//! inscribed radius are **lethal** (the footprint would collide), and cells out to
//! an inflation radius carry an exponentially-decaying penalty so the planner
//! *prefers* clearance. This module computes that [`CostField`] (via a brushfire
//! distance transform) and plans over it ([`plan_inflated`]), so OBC navigation
//! keeps a safety margin and refuses gaps narrower than the robot.

use super::planning::{Cell, OccupancyGrid};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};

/// Extra A* cost charged per unit of inflation cost (how strongly to avoid
/// obstacle proximity). Higher ⇒ wider berth.
const COST_WEIGHT: f64 = 12.0;
const SQRT2: f64 = std::f64::consts::SQRT_2;

/// A per-cell cost field inflated from obstacles: `1.0` = lethal (collision),
/// `(0,1)` = proximity penalty, `0.0` = clear.
#[derive(Debug, Clone)]
pub struct CostField {
    width: usize,
    height: usize,
    cost: Vec<f64>,
}

impl CostField {
    fn idx(&self, cx: usize, cy: usize) -> usize {
        cy * self.width + cx
    }
    /// The inflation cost of a cell (`1.0` lethal).
    pub fn cost(&self, cx: usize, cy: usize) -> f64 {
        self.cost[self.idx(cx, cy)]
    }
    /// Whether the cell is lethal (robot footprint would collide).
    pub fn lethal(&self, cx: usize, cy: usize) -> bool {
        self.cost(cx, cy) >= 1.0
    }
}

/// Build a [`CostField`] from a grid. `inscribed_radius` (world units) marks the
/// lethal halo; out to `inflation_radius` the cost decays as
/// `exp(-decay · (dist − inscribed))`. Distances come from a brushfire
/// (multi-source BFS) over occupied cells.
pub fn inflate(
    grid: &OccupancyGrid,
    inscribed_radius: f64,
    inflation_radius: f64,
    decay: f64,
) -> CostField {
    let (w, h) = (grid.width(), grid.height());
    let res = grid.resolution();

    // Brushfire: step distance (in cells) to the nearest occupied cell.
    let mut dist = vec![u32::MAX; w * h];
    let mut q = VecDeque::new();
    for cy in 0..h {
        for cx in 0..w {
            if matches!(grid.get(cx, cy), Cell::Occupied) {
                dist[cy * w + cx] = 0;
                q.push_back((cx as i64, cy as i64));
            }
        }
    }
    while let Some((cx, cy)) = q.pop_front() {
        let d = dist[(cy as usize) * w + cx as usize];
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (cx + dx, cy + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let ni = (ny as usize) * w + nx as usize;
                if dist[ni] > d + 1 {
                    dist[ni] = d + 1;
                    q.push_back((nx, ny));
                }
            }
        }
    }

    let mut cost = vec![0.0; w * h];
    for i in 0..w * h {
        if dist[i] == u32::MAX {
            continue; // no obstacle anywhere → clear
        }
        let d_world = dist[i] as f64 * res;
        if d_world <= inscribed_radius {
            cost[i] = 1.0; // lethal (includes occupied cells, dist 0)
        } else if d_world <= inflation_radius {
            cost[i] = (-decay * (d_world - inscribed_radius)).exp().clamp(0.0, 0.999);
        }
    }
    CostField { width: w, height: h, cost }
}

// ── Clearance-aware A* ──────────────────────────────────────────────────────────

#[derive(PartialEq)]
struct Frontier {
    f: f64,
    cell: (usize, usize),
}
impl Eq for Frontier {}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f)
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn heuristic(a: (usize, usize), b: (usize, usize)) -> f64 {
    let dx = a.0 as f64 - b.0 as f64;
    let dy = a.1 as f64 - b.1 as f64;
    (dx * dx + dy * dy).sqrt()
}

/// Plan a clearance-aware path of world waypoints from `start` to `goal` over the
/// grid + inflation field. Lethal cells (occupied or within the inscribed radius)
/// are impassable; the path is penalized for obstacle proximity so it keeps a
/// margin. `None` if endpoints are out of bounds, the goal is lethal, or no path
/// exists. Simplified to turn points; excludes the start.
pub fn plan_inflated(
    grid: &OccupancyGrid,
    field: &CostField,
    start: (f64, f64),
    goal: (f64, f64),
) -> Option<Vec<(f64, f64)>> {
    let start_c = grid.world_to_cell(start.0, start.1)?;
    let goal_c = grid.world_to_cell(goal.0, goal.1)?;
    let blocked = |c: (usize, usize)| matches!(grid.get(c.0, c.1), Cell::Occupied) || field.lethal(c.0, c.1);
    if blocked(goal_c) {
        return None;
    }

    let mut open = BinaryHeap::new();
    let mut g_score: HashMap<(usize, usize), f64> = HashMap::new();
    let mut came_from: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    g_score.insert(start_c, 0.0);
    open.push(Frontier { f: heuristic(start_c, goal_c), cell: start_c });

    while let Some(Frontier { cell, .. }) = open.pop() {
        if cell == goal_c {
            return Some(reconstruct(grid, &came_from, cell));
        }
        let g_cur = *g_score.get(&cell).unwrap_or(&f64::INFINITY);
        let (cx, cy) = (cell.0 as i64, cell.1 as i64);
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (cx + dx, cy + dy);
                if nx < 0 || ny < 0 || nx as usize >= grid.width() || ny as usize >= grid.height() {
                    continue;
                }
                let nc = (nx as usize, ny as usize);
                if blocked(nc) {
                    continue;
                }
                let step = if dx != 0 && dy != 0 { SQRT2 } else { 1.0 };
                let tentative = g_cur + step + COST_WEIGHT * field.cost(nc.0, nc.1);
                if tentative < *g_score.get(&nc).unwrap_or(&f64::INFINITY) {
                    came_from.insert(nc, cell);
                    g_score.insert(nc, tentative);
                    open.push(Frontier { f: tentative + heuristic(nc, goal_c), cell: nc });
                }
            }
        }
    }
    None
}

fn reconstruct(
    grid: &OccupancyGrid,
    came_from: &HashMap<(usize, usize), (usize, usize)>,
    goal: (usize, usize),
) -> Vec<(f64, f64)> {
    let mut cells = vec![goal];
    let mut cur = goal;
    while let Some(&prev) = came_from.get(&cur) {
        cells.push(prev);
        cur = prev;
    }
    cells.reverse();
    simplify(&cells).into_iter().skip(1).map(|(cx, cy)| grid.cell_center(cx, cy)).collect()
}

fn simplify(cells: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if cells.len() <= 2 {
        return cells.to_vec();
    }
    let dir = |a: (usize, usize), b: (usize, usize)| {
        ((b.0 as i64 - a.0 as i64).signum(), (b.1 as i64 - a.1 as i64).signum())
    };
    let mut out = vec![cells[0]];
    for i in 1..cells.len() - 1 {
        if dir(cells[i - 1], cells[i]) != dir(cells[i], cells[i + 1]) {
            out.push(cells[i]);
        }
    }
    out.push(*cells.last().unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflation_cost_decreases_with_distance() {
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 12, 12);
        g.set(5, 5, Cell::Occupied);
        let f = inflate(&g, 0.0, 3.0, 1.0);
        assert!(f.lethal(5, 5), "the obstacle cell is lethal");
        let near = f.cost(5, 6); // 1 cell away
        let mid = f.cost(5, 8); // 3 cells away
        let far = f.cost(5, 9); // 4 cells away — outside inflation radius
        assert!(near > mid && mid > 0.0, "cost decays with distance: {near} > {mid} > 0");
        assert_eq!(far, 0.0, "beyond the inflation radius is clear");
    }

    #[test]
    fn inscribed_radius_blocks_a_too_tight_gap() {
        // wall down column 5, rows 1..9, leaving a one-cell gap at (5,0)
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        for cy in 1..10 {
            g.set(5, cy, Cell::Occupied);
        }
        // robot footprint (inscribed 1.5 m) makes the gap cell lethal → no path
        let f_wide = inflate(&g, 1.5, 3.0, 1.0);
        assert!(plan_inflated(&g, &f_wide, (0.5, 0.5), (9.5, 0.5)).is_none());
        // a point robot (inscribed 0) can thread the same gap
        let f_point = inflate(&g, 0.0, 1.0, 1.0);
        assert!(plan_inflated(&g, &f_point, (0.5, 0.5), (9.5, 0.5)).is_some());
    }

    #[test]
    fn open_grid_still_plans() {
        let g = OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        let f = inflate(&g, 0.3, 1.0, 2.0);
        let path = plan_inflated(&g, &f, (0.5, 0.5), (9.5, 0.5)).unwrap();
        assert!(!path.is_empty());
    }

    #[test]
    fn path_keeps_clearance_from_a_wall() {
        // a single obstacle the straight path would clip; inflated plan detours
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 12, 12);
        for cy in 0..6 {
            g.set(6, cy, Cell::Occupied); // partial wall from the bottom
        }
        let f = inflate(&g, 0.5, 2.5, 1.0);
        let path = plan_inflated(&g, &f, (0.5, 0.5), (11.5, 0.5)).expect("a clear path exists");
        // no waypoint sits on a lethal cell
        for &(x, y) in &path {
            let (cx, cy) = g.world_to_cell(x, y).unwrap();
            assert!(!f.lethal(cx, cy), "waypoint ({x},{y}) is in the lethal halo");
        }
    }
}

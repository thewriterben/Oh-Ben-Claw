//! Occupancy-grid mapping + A* path planning for obstacle-aware navigation.
//!
//! Upgrades navigation from "drive straight at the goal" to "plan a path around
//! known obstacles". A coarse 2D [`OccupancyGrid`] records which cells are free
//! or blocked; [`plan`] runs A* (8-connected, Euclidean heuristic) from the
//! robot's pose to the goal and returns a simplified list of world-coordinate
//! waypoints (turn points only). Those feed straight into the navigation suite's
//! waypoint queue, so obstacle avoidance composes with everything already built —
//! no change to the driving loop.
//!
//! This is the mapping + planning half of a SLAM stack (no online map building /
//! loop closure here); the grid is the structure those would populate.

use super::NavGoal;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// State of a single grid cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    /// Not yet observed (treated as traversable when planning).
    Unknown,
    /// Known clear.
    Free,
    /// Known blocked — impassable.
    Occupied,
}

/// A coarse 2D occupancy grid in world coordinates.
#[derive(Debug, Clone)]
pub struct OccupancyGrid {
    origin_x: f64,
    origin_y: f64,
    resolution: f64,
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl OccupancyGrid {
    /// A grid covering `[origin, origin + size*resolution)` in each axis, all
    /// cells `Unknown`. `resolution` is the cell size in world units.
    pub fn new(origin_x: f64, origin_y: f64, resolution: f64, width: usize, height: usize) -> Self {
        Self {
            origin_x,
            origin_y,
            resolution: resolution.max(f64::EPSILON),
            width,
            height,
            cells: vec![Cell::Unknown; width * height],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    /// Cell size in world units.
    pub fn resolution(&self) -> f64 {
        self.resolution
    }

    fn in_bounds(&self, cx: i64, cy: i64) -> bool {
        cx >= 0 && cy >= 0 && (cx as usize) < self.width && (cy as usize) < self.height
    }

    /// World point → cell indices, or `None` if outside the grid.
    pub fn world_to_cell(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let cx = ((x - self.origin_x) / self.resolution).floor() as i64;
        let cy = ((y - self.origin_y) / self.resolution).floor() as i64;
        if self.in_bounds(cx, cy) {
            Some((cx as usize, cy as usize))
        } else {
            None
        }
    }

    /// World point → (possibly out-of-bounds) signed cell indices, for raycasting.
    pub fn world_to_cell_signed(&self, x: f64, y: f64) -> (i64, i64) {
        (
            ((x - self.origin_x) / self.resolution).floor() as i64,
            ((y - self.origin_y) / self.resolution).floor() as i64,
        )
    }

    /// Whether signed cell indices are within the grid.
    pub fn contains(&self, cx: i64, cy: i64) -> bool {
        self.in_bounds(cx, cy)
    }

    /// World coordinates of a cell's center.
    pub fn cell_center(&self, cx: usize, cy: usize) -> (f64, f64) {
        (
            self.origin_x + (cx as f64 + 0.5) * self.resolution,
            self.origin_y + (cy as f64 + 0.5) * self.resolution,
        )
    }

    /// The cell state.
    pub fn get(&self, cx: usize, cy: usize) -> Cell {
        self.cells[cy * self.width + cx]
    }

    /// Set a cell's state. Returns `false` if out of bounds.
    pub fn set(&mut self, cx: usize, cy: usize, cell: Cell) -> bool {
        if cx < self.width && cy < self.height {
            self.cells[cy * self.width + cx] = cell;
            true
        } else {
            false
        }
    }

    /// Mark the cell containing a world point. Returns `false` if out of bounds.
    pub fn set_world(&mut self, x: f64, y: f64, cell: Cell) -> bool {
        match self.world_to_cell(x, y) {
            Some((cx, cy)) => self.set(cx, cy, cell),
            None => false,
        }
    }

    /// Whether a cell blocks travel (only `Occupied` blocks; `Unknown` is optimistic).
    fn blocked(&self, cx: usize, cy: usize) -> bool {
        matches!(self.get(cx, cy), Cell::Occupied)
    }

    /// Count of occupied cells.
    pub fn occupied_count(&self) -> usize {
        self.cells.iter().filter(|c| matches!(c, Cell::Occupied)).count()
    }
}

// ── A* ────────────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
struct Frontier {
    f: f64,
    cell: (usize, usize),
}
impl Eq for Frontier {}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so the BinaryHeap (max-heap) yields the smallest f first.
        other.f.total_cmp(&self.f)
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const SQRT2: f64 = std::f64::consts::SQRT_2;

fn heuristic(a: (usize, usize), b: (usize, usize)) -> f64 {
    let dx = a.0 as f64 - b.0 as f64;
    let dy = a.1 as f64 - b.1 as f64;
    (dx * dx + dy * dy).sqrt()
}

/// Plan an obstacle-free path of **world waypoints** from `start` to `goal` over
/// the grid. Returns `None` if either endpoint is outside the grid, the goal cell
/// is blocked, or no path exists. The path is simplified to turn points only and
/// excludes the start (the robot is already there).
pub fn plan(grid: &OccupancyGrid, start: (f64, f64), goal: (f64, f64)) -> Option<Vec<(f64, f64)>> {
    let start_c = grid.world_to_cell(start.0, start.1)?;
    let goal_c = grid.world_to_cell(goal.0, goal.1)?;
    if grid.blocked(goal_c.0, goal_c.1) {
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
        for (nx, ny, step) in neighbors(grid, cell) {
            let tentative = g_cur + step;
            if tentative < *g_score.get(&(nx, ny)).unwrap_or(&f64::INFINITY) {
                came_from.insert((nx, ny), cell);
                g_score.insert((nx, ny), tentative);
                open.push(Frontier { f: tentative + heuristic((nx, ny), goal_c), cell: (nx, ny) });
            }
        }
    }
    None
}

/// Like [`plan`], but returns navigation goals with the given arrival tolerance.
pub fn plan_goals(
    grid: &OccupancyGrid,
    start: (f64, f64),
    goal: (f64, f64),
    tolerance: f64,
) -> Option<Vec<NavGoal>> {
    plan(grid, start, goal).map(|pts| {
        pts.into_iter()
            .map(|(x, y)| NavGoal { x, y, tolerance })
            .collect()
    })
}

/// The 8-connected, non-blocked neighbors of a cell with their step costs.
fn neighbors(grid: &OccupancyGrid, (cx, cy): (usize, usize)) -> Vec<(usize, usize, f64)> {
    let mut out = Vec::with_capacity(8);
    for dx in -1i64..=1 {
        for dy in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = cx as i64 + dx;
            let ny = cy as i64 + dy;
            if nx < 0 || ny < 0 || nx as usize >= grid.width || ny as usize >= grid.height {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if grid.blocked(nx, ny) {
                continue;
            }
            let cost = if dx != 0 && dy != 0 { SQRT2 } else { 1.0 };
            out.push((nx, ny, cost));
        }
    }
    out
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
    let simplified = simplify(&cells);
    // Drop the start cell — the robot is already there.
    simplified
        .into_iter()
        .skip(1)
        .map(|(cx, cy)| grid.cell_center(cx, cy))
        .collect()
}

/// Keep only cells where the direction of travel changes (turn points).
fn simplify(cells: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if cells.len() <= 2 {
        return cells.to_vec();
    }
    let dir = |a: (usize, usize), b: (usize, usize)| {
        let dx = (b.0 as i64 - a.0 as i64).signum();
        let dy = (b.1 as i64 - a.1 as i64).signum();
        (dx, dy)
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

    fn grid() -> OccupancyGrid {
        // 10x10 grid, 1.0 resolution, origin at (0,0): covers [0,10) x [0,10)
        OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10)
    }

    #[test]
    fn world_cell_roundtrip() {
        let g = grid();
        assert_eq!(g.world_to_cell(0.5, 0.5), Some((0, 0)));
        assert_eq!(g.world_to_cell(9.9, 9.9), Some((9, 9)));
        assert_eq!(g.world_to_cell(-0.1, 0.0), None);
        assert_eq!(g.world_to_cell(10.0, 0.0), None);
        let (x, y) = g.cell_center(0, 0);
        assert!((x - 0.5).abs() < 1e-9 && (y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn straight_path_simplifies_to_goal() {
        let g = grid();
        let path = plan(&g, (0.5, 0.5), (9.5, 0.5)).unwrap();
        // straight line east → only the goal remains after simplification
        assert_eq!(path.len(), 1);
        assert!((path[0].0 - 9.5).abs() < 1e-9 && (path[0].1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn plans_around_a_wall() {
        let mut g = grid();
        // vertical wall at x-cell 5 from y=0..8, leaving a gap at the top
        for cy in 0..8 {
            g.set(5, cy, Cell::Occupied);
        }
        let path = plan(&g, (0.5, 0.5), (9.5, 0.5)).expect("a path exists around the wall");
        // must detour (more than one waypoint) and never pass through an occupied cell
        assert!(path.len() >= 2, "expected a detour, got {path:?}");
        for &(x, y) in &path {
            let (cx, cy) = g.world_to_cell(x, y).unwrap();
            assert!(!matches!(g.get(cx, cy), Cell::Occupied));
        }
        // the detour should route through the gap (high y)
        assert!(path.iter().any(|&(_, y)| y >= 7.0), "path should use the top gap");
    }

    #[test]
    fn blocked_goal_has_no_path() {
        let mut g = grid();
        g.set(9, 0, Cell::Occupied);
        assert!(plan(&g, (0.5, 0.5), (9.5, 0.5)).is_none());
    }

    #[test]
    fn fully_walled_goal_has_no_path() {
        let mut g = grid();
        // wall the entire column 5 → goal on the far side is unreachable
        for cy in 0..10 {
            g.set(5, cy, Cell::Occupied);
        }
        assert!(plan(&g, (0.5, 0.5), (9.5, 0.5)).is_none());
    }

    #[test]
    fn out_of_bounds_endpoint_has_no_path() {
        let g = grid();
        assert!(plan(&g, (0.5, 0.5), (100.0, 100.0)).is_none());
    }

    #[test]
    fn plan_goals_carries_tolerance() {
        let g = grid();
        let goals = plan_goals(&g, (0.5, 0.5), (5.5, 0.5), 0.25).unwrap();
        assert!(goals.iter().all(|gl| (gl.tolerance - 0.25).abs() < 1e-9));
        assert!((goals.last().unwrap().x - 5.5).abs() < 1e-9);
    }
}

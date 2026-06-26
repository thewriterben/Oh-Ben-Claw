//! Autonomous exploration — frontier-based self-mapping.
//!
//! A **frontier** is the boundary between the known and the unknown: a `Free`
//! cell next to an `Unknown` one. Driving to a frontier and scanning there
//! reveals new territory, which creates new frontiers — so repeatedly heading to
//! the nearest reachable frontier sweeps the whole reachable space until none
//! remain (the map is complete). This composes the rest of the navigation stack:
//! mapping fills the grid, SLAM corrects the pose, A* plans the route, and the
//! drive controller follows it — the robot explores an unknown room on its own.

use super::planning::{plan, Cell, OccupancyGrid};
use super::NavGoal;

/// All frontier cells: `Free` cells with at least one 4-neighbor that is
/// `Unknown` (in-bounds). These border the unexplored space.
pub fn frontier_cells(grid: &OccupancyGrid) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for cy in 0..grid.height() {
        for cx in 0..grid.width() {
            if grid.get(cx, cy) != Cell::Free {
                continue;
            }
            let cxi = cx as i64;
            let cyi = cy as i64;
            let unknown_neighbor = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                let nx = cxi + dx;
                let ny = cyi + dy;
                grid.contains(nx, ny) && grid.get(nx as usize, ny as usize) == Cell::Unknown
            });
            if unknown_neighbor {
                out.push((cx, cy));
            }
        }
    }
    out
}

/// The nearest *reachable* frontier as a navigation goal, or `None` when the
/// reachable space is fully explored. Frontiers are ranked by straight-line
/// distance from the pose; the first that A* can reach is returned.
pub fn nearest_frontier_goal(
    grid: &OccupancyGrid,
    pose: (f64, f64),
    tolerance: f64,
) -> Option<NavGoal> {
    let mut frontiers: Vec<(usize, usize)> = frontier_cells(grid);
    if frontiers.is_empty() {
        return None;
    }
    // Rank by distance from the pose (cell centers).
    frontiers.sort_by(|&a, &b| {
        let da = dist2(grid.cell_center(a.0, a.1), pose);
        let db = dist2(grid.cell_center(b.0, b.1), pose);
        da.total_cmp(&db)
    });
    for (cx, cy) in frontiers {
        let (gx, gy) = grid.cell_center(cx, cy);
        if plan(grid, pose, (gx, gy)).is_some() {
            return Some(NavGoal { x: gx, y: gy, tolerance });
        }
    }
    None
}

fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid with a small known-free pocket surrounded by unknown.
    fn pocket() -> OccupancyGrid {
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 10, 10);
        // mark a 3x3 free region at cells (1..3, 1..3); the rest stays Unknown
        for cx in 1..4 {
            for cy in 1..4 {
                g.set(cx, cy, Cell::Free);
            }
        }
        g
    }

    #[test]
    fn frontiers_are_the_known_unknown_boundary() {
        let g = pocket();
        let f = frontier_cells(&g);
        // the 8 border cells of the 3x3 pocket touch unknown; the center (2,2) is
        // surrounded by free, so it is NOT a frontier
        assert_eq!(f.len(), 8);
        assert!(f.contains(&(1, 1)) && f.contains(&(3, 3)));
        assert!(!f.contains(&(2, 2)));
    }

    #[test]
    fn nearest_frontier_goal_picks_a_reachable_boundary() {
        let g = pocket();
        // pose at the pocket center (2.5, 2.5)
        let goal = nearest_frontier_goal(&g, (2.5, 2.5), 0.4).expect("a frontier exists");
        // the goal is one of the free pocket cells (centers at x,y in {1.5,2.5,3.5})
        assert!(goal.x >= 1.0 && goal.x <= 4.0 && goal.y >= 1.0 && goal.y <= 4.0);
    }

    #[test]
    fn no_unknown_means_exploration_complete() {
        // a fully-known grid (all Free) has no frontiers
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 5, 5);
        for cx in 0..5 {
            for cy in 0..5 {
                g.set(cx, cy, Cell::Free);
            }
        }
        assert!(frontier_cells(&g).is_empty());
        assert!(nearest_frontier_goal(&g, (2.5, 2.5), 0.5).is_none());
    }

    #[test]
    fn exploration_shrinks_frontiers_as_space_is_revealed() {
        let mut g = pocket();
        let before = frontier_cells(&g).len();
        // "scan" reveals a ring of free cells around the pocket → some interior
        // cells are no longer on the boundary
        for cx in 0..5 {
            for cy in 0..5 {
                if g.get(cx, cy) == Cell::Unknown {
                    g.set(cx, cy, Cell::Free);
                }
            }
        }
        let after = frontier_cells(&g).len();
        // the pocket's interior cells (e.g. (2,2)) are now surrounded by free
        assert!(!frontier_cells(&g).contains(&(2, 2)));
        assert!(after != before);
    }
}

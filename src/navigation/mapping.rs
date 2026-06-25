//! Online occupancy mapping — build the grid from range sensor scans.
//!
//! The planning grid was filled by hand (`nav_map mark`). This module fills it
//! from perception instead: given the robot's pose and a set of range beams, it
//! **ray-casts** each beam — the cells the beam passes through are cleared
//! (`Free`), and the cell where the beam stops on an obstacle is marked
//! `Occupied`. A beam that reaches its max range without hitting anything just
//! clears space. This is the mapping front end of SLAM: combined with the
//! pose-graph back end (which corrects the pose) and the A* planner (which plans
//! over the map), the robot now builds and navigates its own world.
//!
//! Marking is "sticky" — a passing free-beam never erases a cell another beam
//! found occupied — so the map accretes obstacles rather than flickering. (A full
//! system would use log-odds; this is the deterministic, testable core.)

use super::planning::{Cell, OccupancyGrid};

/// Integrate one range beam into the grid from pose `(x, y, heading_deg)`.
/// `bearing_deg` is the beam angle relative to the robot heading; `range` the
/// measured distance; `max_range` the sensor limit (a `range >= max_range` beam
/// is treated as "no hit" and only clears space).
pub fn integrate_beam(
    grid: &mut OccupancyGrid,
    x: f64,
    y: f64,
    heading_deg: f64,
    bearing_deg: f64,
    range: f64,
    max_range: f64,
) {
    let ang = (heading_deg + bearing_deg).to_radians();
    let ex = x + range * ang.cos();
    let ey = y + range * ang.sin();
    let (x0, y0) = grid.world_to_cell_signed(x, y);
    let (x1, y1) = grid.world_to_cell_signed(ex, ey);
    let hit = range < max_range;

    // Bresenham from the robot cell to the beam endpoint.
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut cx, mut cy) = (x0, y0);
    loop {
        let is_end = cx == x1 && cy == y1;
        if grid.contains(cx, cy) {
            let (ux, uy) = (cx as usize, cy as usize);
            if is_end && hit {
                grid.set(ux, uy, Cell::Occupied);
            } else if grid.get(ux, uy) != Cell::Occupied {
                // Clear free space, but never erase a known obstacle.
                grid.set(ux, uy, Cell::Free);
            }
        }
        if is_end {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
}

/// Integrate a full scan: a set of `(bearing_deg, range)` beams from one pose.
pub fn integrate_scan(
    grid: &mut OccupancyGrid,
    x: f64,
    y: f64,
    heading_deg: f64,
    beams: &[(f64, f64)],
    max_range: f64,
) {
    for &(bearing, range) in beams {
        integrate_beam(grid, x, y, heading_deg, bearing, range, max_range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> OccupancyGrid {
        OccupancyGrid::new(0.0, 0.0, 1.0, 12, 12)
    }

    #[test]
    fn beam_hit_marks_obstacle_and_clears_path() {
        let mut g = grid();
        // from (0.5,0.5) facing east, beam straight ahead hits at range 5
        integrate_beam(&mut g, 0.5, 0.5, 0.0, 0.0, 5.0, 10.0);
        assert_eq!(g.get(5, 0), Cell::Occupied, "endpoint is an obstacle");
        assert_eq!(g.get(3, 0), Cell::Free, "cells along the beam are cleared");
        assert_eq!(g.get(0, 0), Cell::Free, "robot cell is free");
    }

    #[test]
    fn max_range_beam_only_clears() {
        let mut g = grid();
        // range == max_range ⇒ no hit ⇒ endpoint is cleared, not occupied
        integrate_beam(&mut g, 0.5, 0.5, 0.0, 0.0, 6.0, 6.0);
        assert_eq!(g.get(6, 0), Cell::Free);
        assert_eq!(g.get(2, 0), Cell::Free);
    }

    #[test]
    fn free_beam_does_not_erase_known_obstacle() {
        let mut g = grid();
        g.set(5, 0, Cell::Occupied); // a known wall cell
        // a beam that overshoots past it (no hit) must not clear it
        integrate_beam(&mut g, 0.5, 0.5, 0.0, 0.0, 8.0, 8.0);
        assert_eq!(g.get(5, 0), Cell::Occupied, "obstacle survives a passing free beam");
    }

    #[test]
    fn scan_marks_a_wall_the_planner_avoids() {
        use super::super::planning::plan;
        let mut g = OccupancyGrid::new(0.0, 0.0, 1.0, 12, 12);
        // a scan from (0.5,0.5) facing east, fanning beams that hit a wall at
        // column x≈5 across rows 0..6 (a gap remains at the top).
        let (px, py, ph) = (0.5, 0.5, 0.0);
        for cy in 0..7 {
            let tx = 5.5 - px;
            let ty = (cy as f64 + 0.5) - py;
            let bearing = ty.atan2(tx).to_degrees() - ph;
            let range = (tx * tx + ty * ty).sqrt();
            integrate_beam(&mut g, px, py, ph, bearing, range, 20.0);
        }
        // the scanned wall blocks a straight eastward path → the planner detours
        let path = plan(&g, (0.5, 0.5), (9.5, 0.5)).expect("a detour should exist");
        for &(x, y) in &path {
            let (cx, cy) = g.world_to_cell_signed(x, y);
            assert!(!matches!(g.get(cx as usize, cy as usize), Cell::Occupied));
        }
    }
}

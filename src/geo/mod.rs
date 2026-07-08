//! Geospatial coordinate contract — the shared geometry for the Conservation Grid.
//!
//! This is the **G0 unlock** from `docs/CONSERVATION-GRID-STRATEGY.md`: a single,
//! canonical way to move between earth coordinates (WGS84 latitude/longitude/altitude)
//! and a site-local metric frame. It exists so the rest of the stack keeps operating in
//! flat metric coordinates — `navigation` occupancy grids and SE2 poses, `fleet` spacing,
//! `deployment` placement — while geography is attached only at the edges.
//!
//! ## Frame
//! A [`GeoFrame`] is anchored at a site **origin** and converts to/from local **ENU**
//! (East, North, Up) metres via an equirectangular local-tangent-plane approximation. At
//! camera-trap / reserve scale (a few km) the error is negligible, and the map is exactly
//! invertible so round-trips are stable. ENU `(e, n)` *is* the metric plane the navigation
//! stack already uses — a site origin simply pins that plane to the earth. This **wraps,
//! not replaces**, `src/navigation`: nothing there needs to change.
//!
//! ## Contract with ClawCam
//! [`Site`] mirrors the ClawCam `sites` table (`gateway/.../storage/database.py`): an id,
//! name, origin, boundary polygon of `[lat, lon]` points, and an optional DEM reference.
//! [`Site::contains`] uses the same ray-casting point-in-polygon convention as ClawCam's
//! `events_in_site`, so "is this detection inside the survey area?" answers identically on
//! both sides of the wire.

use serde::{Deserialize, Serialize};

/// Mean Earth radius in metres (spherical approximation).
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// A WGS84 geodetic point: degrees latitude/longitude, metres altitude.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt: f64,
}

impl GeoPoint {
    pub fn new(lat: f64, lon: f64, alt: f64) -> Self {
        Self { lat, lon, alt }
    }
}

/// A local East-North-Up position in metres, relative to a [`GeoFrame`] origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Enu {
    pub e: f64,
    pub n: f64,
    pub u: f64,
}

impl Enu {
    pub fn new(e: f64, n: f64, u: f64) -> Self {
        Self { e, n, u }
    }
    /// Planar (East/North) distance in metres, ignoring Up.
    pub fn planar_distance(&self, other: &Enu) -> f64 {
        ((self.e - other.e).powi(2) + (self.n - other.n).powi(2)).sqrt()
    }
}

/// An equirectangular local-tangent-plane frame anchored at a geodetic origin.
///
/// The `cos(lat0)` scale on East is cached at construction. Conversions are exact
/// analytic inverses of one another, so `from_enu(to_enu(p)) == p` up to float rounding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GeoFrame {
    origin: GeoPoint,
    cos_lat0: f64,
}

impl GeoFrame {
    pub fn new(origin: GeoPoint) -> Self {
        Self { origin, cos_lat0: origin.lat.to_radians().cos() }
    }

    pub fn origin(&self) -> GeoPoint {
        self.origin
    }

    /// Project a geodetic point into local ENU metres.
    pub fn to_enu(&self, p: GeoPoint) -> Enu {
        let e = (p.lon - self.origin.lon).to_radians() * self.cos_lat0 * EARTH_RADIUS_M;
        let n = (p.lat - self.origin.lat).to_radians() * EARTH_RADIUS_M;
        let u = p.alt - self.origin.alt;
        Enu { e, n, u }
    }

    /// Lift a local ENU position back to a geodetic point.
    pub fn from_enu(&self, enu: Enu) -> GeoPoint {
        let lat = self.origin.lat + (enu.n / EARTH_RADIUS_M).to_degrees();
        // cos_lat0 is bounded away from 0 for any realistic site (|lat| < 90).
        let lon = self.origin.lon
            + (enu.e / (EARTH_RADIUS_M * self.cos_lat0)).to_degrees();
        GeoPoint { lat, lon, alt: self.origin.alt + enu.u }
    }
}

/// Ray-casting point-in-polygon over `[lat, lon]` vertices (same convention as ClawCam).
///
/// Returns `true` if `(lat, lon)` is inside `boundary`. Fewer than 3 vertices → `false`.
fn point_in_polygon(lat: f64, lon: f64, boundary: &[GeoPoint]) -> bool {
    let n = boundary.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (boundary[i].lat, boundary[i].lon);
        let (xj, yj) = (boundary[j].lat, boundary[j].lon);
        let intersect = ((yi > lon) != (yj > lon))
            && (lat < (xj - xi) * (lon - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A survey-area site: the OBC mirror of the ClawCam `sites` contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub origin: GeoPoint,
    #[serde(default)]
    pub boundary: Vec<GeoPoint>,
    #[serde(default)]
    pub dem_ref: Option<String>,
}

impl Site {
    /// Build a site, defaulting the origin to the boundary centroid when the boundary is
    /// non-empty (matching ClawCam's `upsert_site` centroid default).
    pub fn new(id: impl Into<String>, name: impl Into<String>, boundary: Vec<GeoPoint>) -> Self {
        let origin = Self::centroid(&boundary).unwrap_or(GeoPoint::new(0.0, 0.0, 0.0));
        Self { id: id.into(), name: name.into(), origin, boundary, dem_ref: None }
    }

    fn centroid(boundary: &[GeoPoint]) -> Option<GeoPoint> {
        if boundary.is_empty() {
            return None;
        }
        let k = boundary.len() as f64;
        let lat = boundary.iter().map(|p| p.lat).sum::<f64>() / k;
        let lon = boundary.iter().map(|p| p.lon).sum::<f64>() / k;
        Some(GeoPoint::new(lat, lon, 0.0))
    }

    /// The local metric frame for this site (anchored at its origin).
    pub fn frame(&self) -> GeoFrame {
        GeoFrame::new(self.origin)
    }

    /// Whether a geodetic point lies inside the site boundary polygon.
    pub fn contains(&self, p: GeoPoint) -> bool {
        point_in_polygon(p.lat, p.lon, &self.boundary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn one_degree_north_is_111km() {
        let f = GeoFrame::new(GeoPoint::new(45.0, -122.0, 0.0));
        let enu = f.to_enu(GeoPoint::new(46.0, -122.0, 0.0));
        assert!(approx(enu.n, 111_194.93, 0.1), "n={}", enu.n);
        assert!(approx(enu.e, 0.0, 1e-6), "e={}", enu.e);
    }

    #[test]
    fn one_degree_east_scales_by_cos_lat() {
        let f = GeoFrame::new(GeoPoint::new(45.0, -122.0, 0.0));
        let enu = f.to_enu(GeoPoint::new(45.0, -121.0, 0.0));
        assert!(approx(enu.e, 78_626.69, 0.1), "e={}", enu.e);
        assert!(approx(enu.n, 0.0, 1e-6), "n={}", enu.n);
    }

    #[test]
    fn enu_round_trip_is_stable() {
        let f = GeoFrame::new(GeoPoint::new(45.5, -122.6, 100.0));
        let p = GeoPoint::new(45.501, -122.601, 105.0);
        let back = f.from_enu(f.to_enu(p));
        assert!(approx(back.lat, p.lat, 1e-9), "lat={}", back.lat);
        assert!(approx(back.lon, p.lon, 1e-9), "lon={}", back.lon);
        assert!(approx(back.alt, p.alt, 1e-9), "alt={}", back.alt);
    }

    #[test]
    fn origin_maps_to_zero() {
        let o = GeoPoint::new(10.0, 20.0, 30.0);
        let enu = GeoFrame::new(o).to_enu(o);
        assert_eq!(enu, Enu::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn site_centroid_default_origin() {
        let sq = vec![
            GeoPoint::new(45.4, -122.7, 0.0),
            GeoPoint::new(45.4, -122.5, 0.0),
            GeoPoint::new(45.6, -122.5, 0.0),
            GeoPoint::new(45.6, -122.7, 0.0),
        ];
        let site = Site::new("s1", "North Ridge", sq);
        assert!(approx(site.origin.lat, 45.5, 1e-9));
        assert!(approx(site.origin.lon, -122.6, 1e-9));
    }

    #[test]
    fn site_contains_square() {
        let sq = vec![
            GeoPoint::new(45.4, -122.7, 0.0),
            GeoPoint::new(45.4, -122.5, 0.0),
            GeoPoint::new(45.6, -122.5, 0.0),
            GeoPoint::new(45.6, -122.7, 0.0),
        ];
        let site = Site::new("s1", "", sq);
        assert!(site.contains(GeoPoint::new(45.5, -122.6, 0.0)));
        assert!(!site.contains(GeoPoint::new(10.0, 10.0, 0.0)));
    }

    #[test]
    fn site_contains_triangle_excludes_bbox_corner() {
        // Triangle A(0,0) B(0,10) C(10,0): region lat>=0, lon>=0, lat+lon<=10.
        let tri = vec![
            GeoPoint::new(0.0, 0.0, 0.0),
            GeoPoint::new(0.0, 10.0, 0.0),
            GeoPoint::new(10.0, 0.0, 0.0),
        ];
        let site = Site::new("tri", "", tri);
        assert!(site.contains(GeoPoint::new(2.0, 2.0, 0.0))); // inside
        assert!(!site.contains(GeoPoint::new(9.0, 9.0, 0.0))); // in bbox, outside triangle
    }

    #[test]
    fn degenerate_boundary_never_contains() {
        let site = Site::new("empty", "", vec![]);
        assert!(!site.contains(GeoPoint::new(0.0, 0.0, 0.0)));
    }
}

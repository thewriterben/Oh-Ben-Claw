//! World-memory-anchored site frame — the Conservation Grid **G0 exit**.
//!
//! `src/geo` gives us the math ([`GeoFrame`]) and the contract ([`Site`]); this module
//! pins one live site to the running system by storing it as a time-valid fact in
//! [`WorldMemory`] under the well-known entity [`SITE_ENTITY`]. Once anchored, any node
//! pose expressed in local ENU metres ↔ earth `(lat, lon)` through one shared frame —
//! `gnss`, `siteplan`, `aerial`, `fleet`, and the deployment codegen all agree on where
//! the world is.
//!
//! Anchoring is **non-destructive** like every world-memory write: re-anchoring a site
//! closes the previous anchor fact, so "where was the origin last month?" stays
//! answerable via [`anchored_site_at`].

use anyhow::Result;
use serde_json::Value;

use crate::geo::{Enu, GeoFrame, GeoPoint, Site};
use crate::memory::world::{Fact, WorldMemory};

/// The well-known world-memory entity holding the current site anchor.
pub const SITE_ENTITY: &str = "geo.site";

/// Anchor `site` as the live geospatial frame, valid from `now_ms`.
///
/// Non-destructive: a previous anchor (if any) is closed at `now_ms` and remains
/// queryable via [`anchored_site_at`]. Returns the recorded [`Fact`].
pub fn anchor_site(world: &WorldMemory, site: &Site, now_ms: u64, source: &str) -> Result<Fact> {
    let value = serde_json::to_value(site)?;
    world.observe(SITE_ENTITY, value, now_ms, now_ms, source)
}

fn site_from_value(value: &Value) -> Option<Site> {
    serde_json::from_value(value.clone()).ok()
}

/// The currently anchored [`Site`], if any.
pub fn anchored_site(world: &WorldMemory) -> Result<Option<Site>> {
    Ok(world
        .current(SITE_ENTITY)?
        .and_then(|f| site_from_value(&f.value)))
}

/// The [`Site`] that was anchored at time `ts` (ms since epoch), if any.
pub fn anchored_site_at(world: &WorldMemory, ts: u64) -> Result<Option<Site>> {
    Ok(world
        .at(SITE_ENTITY, ts)?
        .and_then(|f| site_from_value(&f.value)))
}

/// The live [`GeoFrame`] (anchored site's local tangent plane), if a site is anchored.
pub fn anchored_frame(world: &WorldMemory) -> Result<Option<GeoFrame>> {
    Ok(anchored_site(world)?.map(|s| s.frame()))
}

/// Lift a local ENU pose to earth coordinates through the anchored frame.
///
/// `None` when no site is anchored.
pub fn enu_to_geodetic(world: &WorldMemory, enu: Enu) -> Result<Option<GeoPoint>> {
    Ok(anchored_frame(world)?.map(|f| f.from_enu(enu)))
}

/// Project an earth position into the anchored frame's local ENU metres.
///
/// `None` when no site is anchored.
pub fn geodetic_to_enu(world: &WorldMemory, p: GeoPoint) -> Result<Option<Enu>> {
    Ok(anchored_frame(world)?.map(|f| f.to_enu(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_site(id: &str) -> Site {
        Site::new(
            id,
            "North Ridge",
            vec![
                GeoPoint::new(45.4, -122.7, 0.0),
                GeoPoint::new(45.4, -122.5, 0.0),
                GeoPoint::new(45.6, -122.5, 0.0),
                GeoPoint::new(45.6, -122.7, 0.0),
            ],
        )
    }

    #[test]
    fn no_anchor_means_none_everywhere() {
        let w = WorldMemory::open_in_memory().unwrap();
        assert!(anchored_site(&w).unwrap().is_none());
        assert!(anchored_frame(&w).unwrap().is_none());
        assert!(enu_to_geodetic(&w, Enu::new(1.0, 2.0, 0.0))
            .unwrap()
            .is_none());
        assert!(geodetic_to_enu(&w, GeoPoint::new(45.5, -122.6, 0.0))
            .unwrap()
            .is_none());
    }

    #[test]
    fn anchor_then_load_round_trips() {
        let w = WorldMemory::open_in_memory().unwrap();
        let site = square_site("s1");
        anchor_site(&w, &site, 1_000, "operator").unwrap();

        let loaded = anchored_site(&w).unwrap().unwrap();
        assert_eq!(loaded.id, "s1");
        assert_eq!(loaded.name, "North Ridge");
        assert_eq!(loaded.boundary.len(), 4);
        assert!((loaded.origin.lat - 45.5).abs() < 1e-9);
        assert!((loaded.origin.lon - (-122.6)).abs() < 1e-9);
    }

    #[test]
    fn pose_round_trips_through_the_anchored_frame() {
        let w = WorldMemory::open_in_memory().unwrap();
        anchor_site(&w, &square_site("s1"), 1_000, "operator").unwrap();

        // A node 100 m east / 50 m north of the origin gets an earth position…
        let enu = Enu::new(100.0, 50.0, 0.0);
        let geo = enu_to_geodetic(&w, enu).unwrap().unwrap();
        assert!(geo.lat > 45.5 && geo.lon > -122.6);

        // …and projecting it back lands on the same local pose.
        let back = geodetic_to_enu(&w, geo).unwrap().unwrap();
        assert!((back.e - enu.e).abs() < 1e-6, "e={}", back.e);
        assert!((back.n - enu.n).abs() < 1e-6, "n={}", back.n);
    }

    #[test]
    fn reanchoring_is_non_destructive_and_time_correct() {
        let w = WorldMemory::open_in_memory().unwrap();
        anchor_site(&w, &square_site("old"), 1_000, "operator").unwrap();
        anchor_site(&w, &square_site("new"), 2_000, "operator").unwrap();

        // Current is the new anchor.
        assert_eq!(anchored_site(&w).unwrap().unwrap().id, "new");
        // The old anchor is still answerable at its valid time.
        assert_eq!(anchored_site_at(&w, 1_500).unwrap().unwrap().id, "old");
        // Both facts are retained.
        assert_eq!(w.history(SITE_ENTITY).unwrap().len(), 2);
    }

    #[test]
    fn garbled_anchor_value_is_none_not_panic() {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe(SITE_ENTITY, serde_json::json!("not a site"), 1, 1, "x")
            .unwrap();
        assert!(anchored_site(&w).unwrap().is_none());
    }
}

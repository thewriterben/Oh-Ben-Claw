//! Where a node actually is — position sources projected into a site's frame.
//!
//! Two adapters that are one pattern, which is why they are one crate rather
//! than two 200-line ones. Each takes a *real-world* position report, projects
//! it through a site [`obc_planner::geo::GeoFrame`] into the local metric frame
//! the fleet coordinates in, and hands back an
//! [`obc_telemetry::NodeState`] — the heartbeat everything else already reads.
//! Neither knows anything about the coordinator, and that is the point: a drone
//! and a bare GPS module join the same auction and the same exploration
//! geometry as a ground robot, with no new coordination code.
//!
//! | module | input | notes |
//! |---|---|---|
//! | [`aerial`] | already-decoded geodetic telemetry (MAVLink-style lat/lon/alt) | also carries [`aerial::flight_safe`], the Track-0-flavoured refusal on low battery or outside the geofence |
//! | [`gnss`] | a raw NMEA 0183 `GGA` sentence from a receiver | decodes only `GGA` and rejects other talkers, so a caller fails loudly rather than mis-parsing |
//!
//! Both are pure and hardware-free. A real MAVLink link or serial port feeds
//! them; here it is data and text.
//!
//! # Why this could be extracted
//!
//! The eighth and ninth pieces of the agent to move out (2026-08-06), and the
//! first pair unlocked by a deliberate change rather than found already loose.
//! Until the day before, each of these named exactly one thing from `fleet` —
//! `NodeState` — and that single import pinned both to an 823-line coordinator
//! they never otherwise touch. Moving that 25-line struct to `obc-telemetry`,
//! where it belonged, took both from one blocking edge to zero.
//!
//! Verified the way the others were: compiled verbatim in a scratch crate whose
//! entire dependency universe is serde plus the two crates already extracted,
//! and its 16 tests passed there before any of this was written.

pub mod aerial;
pub mod gnss;

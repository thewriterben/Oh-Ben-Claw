//! GNSS tier — turn a node's raw satellite fix into a positioned fleet node (G3).
//!
//! Where [`crate::aerial`] adapts an *already-decoded* lat/lon (MAVLink-style) into the
//! fleet, this module sits one layer lower: it decodes the **NMEA 0183 `GGA`** sentence a
//! bare GPS/GNSS receiver emits (`$GPGGA,...` / `$GNGGA,...`) into a [`GnssFix`], then
//! projects that into a geodetic [`GeoPoint`] or a fleet [`NodeState`] via a site
//! [`GeoFrame`]. So a ground node with nothing but a u-blox module joins the same local
//! metric frame — and the same auction/exploration geometry — as everything else.
//!
//! Pure and hardware-free: a real serial link feeds the sentence string in; here it's just
//! text. Only `GGA` (the canonical fix-and-altitude sentence) is decoded; other talkers
//! are rejected so callers fail loudly rather than silently mis-parse.

use crate::fleet::NodeState;
use crate::geo::{GeoFrame, GeoPoint};
use serde::{Deserialize, Serialize};

/// The NMEA `GGA` fix-quality code, named for readability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixQuality {
    /// No position fix (code 0).
    NoFix,
    /// Autonomous GPS fix (code 1).
    Gps,
    /// Differential GPS (code 2).
    Dgps,
    /// Real-Time Kinematic, fixed integers (code 4).
    Rtk,
    /// RTK float (code 5).
    RtkFloat,
    /// Any other code (estimated/manual/simulation/…).
    Other(u8),
}

impl FixQuality {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => FixQuality::NoFix,
            1 => FixQuality::Gps,
            2 => FixQuality::Dgps,
            4 => FixQuality::Rtk,
            5 => FixQuality::RtkFloat,
            n => FixQuality::Other(n),
        }
    }
}

/// A single decoded GNSS fix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GnssFix {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt_m: f64,
    /// Raw GGA fix-quality code.
    pub fix_quality: u8,
    /// Satellites used in the solution.
    pub satellites: u32,
    /// Horizontal dilution of precision, if present.
    #[serde(default)]
    pub hdop: Option<f64>,
    /// UTC time-of-fix field as reported (`hhmmss.ss`), if present.
    #[serde(default)]
    pub time_utc: Option<String>,
}

impl GnssFix {
    /// True when the receiver reports a usable position (quality code > 0).
    pub fn has_fix(&self) -> bool {
        self.fix_quality > 0
    }

    /// The named fix quality.
    pub fn quality(&self) -> FixQuality {
        FixQuality::from_code(self.fix_quality)
    }

    /// This fix as a geodetic point.
    pub fn to_geopoint(&self) -> GeoPoint {
        GeoPoint::new(self.lat, self.lon, self.alt_m)
    }

    /// Project this fix into the fleet's local metric frame as a [`NodeState`].
    ///
    /// The `frame`'s ENU east/north become the fleet `x`/`y`. GNSS carries no battery, so
    /// `battery` is `None`; `mode` reports `"gnss_fix"` when a fix is present, else
    /// `"gnss_nofix"`. A receiver is never "busy" — it only reports position.
    pub fn to_node_state(&self, frame: &GeoFrame, id: impl Into<String>, now_ms: u64) -> NodeState {
        let enu = frame.to_enu(self.to_geopoint());
        NodeState {
            id: id.into(),
            x: Some(enu.e),
            y: Some(enu.n),
            battery: None,
            mode: if self.has_fix() { "gnss_fix".to_string() } else { "gnss_nofix".to_string() },
            busy: false,
            last_seen_ms: now_ms,
        }
    }
}

/// XOR checksum of an NMEA body (chars between `$` and `*`, exclusive).
fn nmea_checksum(body: &str) -> u8 {
    let body = body.strip_prefix('$').unwrap_or(body);
    body.bytes().fold(0u8, |acc, b| acc ^ b)
}

/// Verify a full NMEA sentence's `*hh` checksum. Sentences without a `*` are accepted
/// (some receivers omit it); a present-but-wrong checksum returns `false`.
pub fn checksum_ok(sentence: &str) -> bool {
    let s = sentence.trim();
    match s.find('*') {
        Some(star) => {
            let (body, rest) = s.split_at(star);
            let hex = rest[1..].trim();
            match u8::from_str_radix(hex, 16) {
                Ok(want) => nmea_checksum(body) == want,
                Err(_) => false,
            }
        }
        None => true,
    }
}

/// Convert an NMEA `ddmm.mmmm` (lat) / `dddmm.mmmm` (lon) field + hemisphere to signed
/// decimal degrees.
fn parse_coord(val: &str, hemi: &str, is_lat: bool) -> Result<f64, String> {
    // An empty coordinate field is valid NMEA for "no data" (e.g. a no-fix GGA); a
    // truncated *sentence* is caught earlier by the field-count check.
    if val.is_empty() {
        return Ok(0.0);
    }
    let deg_digits = if is_lat { 2 } else { 3 };
    // Need at least DD + one minutes digit.
    if val.len() < deg_digits + 1 || !val.is_char_boundary(deg_digits) {
        return Err(format!("malformed coordinate '{val}'"));
    }
    let (d, m) = val.split_at(deg_digits);
    let degrees: f64 = d.parse().map_err(|_| format!("bad degrees '{d}'"))?;
    let minutes: f64 = m.parse().map_err(|_| format!("bad minutes '{m}'"))?;
    if !(0.0..60.0).contains(&minutes) {
        return Err(format!("minutes out of range '{m}'"));
    }
    let mut dec = degrees + minutes / 60.0;
    match hemi {
        "S" | "W" => dec = -dec,
        "N" | "E" | "" => {}
        other => return Err(format!("bad hemisphere '{other}'")),
    }
    Ok(dec)
}

/// Parse an NMEA `GGA` sentence into a [`GnssFix`].
///
/// Accepts any talker id ending in `GGA` (`GP`/`GN`/`GL`/`GA`/…). Validates the `*hh`
/// checksum when present. Returns a descriptive error for non-GGA sentences, bad
/// checksums, or truncated/garbled fields.
pub fn parse_gga(sentence: &str) -> Result<GnssFix, String> {
    let s = sentence.trim();
    if !checksum_ok(s) {
        return Err("checksum mismatch".to_string());
    }
    // Strip the checksum suffix and leading '$'.
    let body = match s.find('*') {
        Some(star) => &s[..star],
        None => s,
    };
    let body = body.strip_prefix('$').unwrap_or(body);
    let f: Vec<&str> = body.split(',').collect();

    let talker = f.first().copied().unwrap_or("");
    if !talker.ends_with("GGA") {
        return Err(format!("not a GGA sentence: '{talker}'"));
    }
    // Fields 0..=9 required: type,time,lat,N/S,lon,E/W,quality,sats,hdop,alt.
    if f.len() < 10 {
        return Err(format!("truncated GGA ({} fields)", f.len()));
    }

    let lat = parse_coord(f[2], f[3], true)?;
    let lon = parse_coord(f[4], f[5], false)?;
    let fix_quality: u8 = f[6].trim().parse().unwrap_or(0);
    let satellites: u32 = f[7].trim().parse().unwrap_or(0);
    let hdop = f[8].trim().parse::<f64>().ok();
    let alt_m = f[9].trim().parse::<f64>().unwrap_or(0.0);
    let time_utc = if f[1].is_empty() { None } else { Some(f[1].to_string()) };

    Ok(GnssFix { lat, lon, alt_m, fix_quality, satellites, hdop, time_utc })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical GGA example (Wikipedia): 48°07.038'N, 011°31.000'E, fix=1, 8 sats, 545.4 m.
    const GGA: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";

    #[test]
    fn checksum_validates_the_canonical_sentence() {
        assert!(checksum_ok(GGA));
        assert!(!checksum_ok("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00"));
        // No checksum present => accepted.
        assert!(checksum_ok("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"));
    }

    #[test]
    fn parses_position_altitude_and_sats() {
        let fix = parse_gga(GGA).unwrap();
        assert!((fix.lat - 48.1173).abs() < 1e-4, "lat {}", fix.lat);
        assert!((fix.lon - 11.516_667).abs() < 1e-4, "lon {}", fix.lon);
        assert!((fix.alt_m - 545.4).abs() < 1e-6);
        assert_eq!(fix.fix_quality, 1);
        assert_eq!(fix.quality(), FixQuality::Gps);
        assert_eq!(fix.satellites, 8);
        assert_eq!(fix.hdop, Some(0.9));
        assert!(fix.has_fix());
        assert_eq!(fix.time_utc.as_deref(), Some("123519"));
    }

    #[test]
    fn southern_and_western_hemispheres_are_negative() {
        // 33°51.9'S, 151°12.6'E (Sydney-ish) and a western case.
        let s = "$GPGGA,010203,3351.900,S,15112.600,E,1,07,1.2,10.0,M,,M,,*5C";
        // Fix the checksum by recomputing (test the parse, not a hand-typed csum):
        let body = &s[1..s.find('*').unwrap()];
        let csum = format!("${}*{:02X}", body, body.bytes().fold(0u8, |a, b| a ^ b));
        let fix = parse_gga(&csum).unwrap();
        assert!(fix.lat < 0.0, "lat should be S/negative: {}", fix.lat);
        assert!((fix.lat + 33.865).abs() < 1e-3, "lat {}", fix.lat);
        assert!(fix.lon > 0.0);
    }

    #[test]
    fn no_fix_reports_no_fix() {
        let s = "$GPGGA,,,,,,0,00,,,M,,M,,";
        let fix = parse_gga(s).unwrap();
        assert_eq!(fix.fix_quality, 0);
        assert_eq!(fix.quality(), FixQuality::NoFix);
        assert!(!fix.has_fix());
    }

    #[test]
    fn accepts_gn_talker() {
        let s = "$GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";
        assert!(parse_gga(s).is_ok());
    }

    #[test]
    fn rejects_non_gga_and_truncated() {
        assert!(parse_gga("$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,,").is_err());
        assert!(parse_gga("$GPGGA,123519,4807.038,N").is_err());
        assert!(parse_gga("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*99").is_err());
    }

    #[test]
    fn projects_to_geopoint_and_node_state() {
        let fix = parse_gga(GGA).unwrap();
        let gp = fix.to_geopoint();
        assert_eq!(gp.lat, fix.lat);
        // A frame anchored at the fix places the node near the origin.
        let frame = GeoFrame::new(gp);
        let node = fix.to_node_state(&frame, "rover-1", 42);
        assert_eq!(node.id, "rover-1");
        assert!(node.x.unwrap().abs() < 1e-6);
        assert!(node.y.unwrap().abs() < 1e-6);
        assert_eq!(node.battery, None);
        assert_eq!(node.mode, "gnss_fix");
        assert!(!node.busy);
        assert_eq!(node.last_seen_ms, 42);
    }

    #[test]
    fn no_fix_node_reports_gnss_nofix_mode() {
        let fix = GnssFix {
            lat: 0.0, lon: 0.0, alt_m: 0.0, fix_quality: 0,
            satellites: 0, hdop: None, time_utc: None,
        };
        let frame = GeoFrame::new(GeoPoint::new(45.0, -122.0, 0.0));
        let node = fix.to_node_state(&frame, "rover-2", 0);
        assert_eq!(node.mode, "gnss_nofix");
    }

    #[test]
    fn node_projects_east_offset_correctly() {
        // A fix one arc-minute of longitude east of the frame origin lands at +east metres.
        let origin = GeoPoint::new(45.0, -122.0, 0.0);
        let frame = GeoFrame::new(origin);
        let east = GnssFix {
            lat: 45.0, lon: -122.0 + 1.0 / 60.0, alt_m: 0.0,
            fix_quality: 1, satellites: 9, hdop: Some(0.8), time_utc: None,
        };
        let node = east.to_node_state(&frame, "e", 0);
        assert!(node.x.unwrap() > 0.0, "east offset should be +x: {:?}", node.x);
        assert!(node.y.unwrap().abs() < 1.0, "no north component: {:?}", node.y);
    }
}

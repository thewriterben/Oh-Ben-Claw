//! MAC → node id. Pure arithmetic over strings, no ESP dependencies.
//!
//! Split from `identity.rs` for the same reason `sensor_math` is split from
//! `sensors`: the hardware read needs a bench, but the *mapping* is
//! deterministic and needs only a test. Behind an `esp_idf_svc` import it could
//! never be executed on the host, and a rule about fleet identity that nothing
//! can run is a rule on trust. `tests/firmware_identity_roster.rs` includes this
//! file by `#[path]` and exercises it for real.
//!
//! See `identity.rs` for why identity is derived from the chip at all.

/// Known boards, by factory MAC.
///
/// **Keep in step with the host's `crates/obc-planner/src/peripherals/registry.rs`
/// and with `scripts/which_esp32.ps1`.** Three copies of one roster is a smell,
/// and this codebase has been bitten by exactly that — a file contradicting its
/// own manifest two directories away, unnoticed because nothing compared them.
/// `tests/firmware_identity_roster.rs` compares all three and fails on drift.
pub const ROSTER: &[(&str, &str)] = &[
    // Verified 2026-09-13 (walkthrough run, 18/18) and again by espflash 2026-09-16.
    ("64:E8:33:7E:BB:98", "obc-esp32-s3-001"),
    // Camera node. MAC read by espflash while flashing it, 2026-09-16:
    //   MAC address: 64:e8:33:7e:7e:04
    ("64:E8:33:7E:7E:04", "obc-esp32-s3-002"),
    // LILYGO T-CameraPlus-S3 V1.1 (OV5640). MAC read by espflash 2026-09-16:
    //   MAC address: 48:ca:43:4b:95:f8
    //
    // A different OUI from the two XIAOs (48:CA:43 rather than 64:E8:33), and a
    // different board entirely — the first fleet member that is not a XIAO.
    //
    // Until now it was unrostered and self-named `obc-esp32-s3-4b95f8` from its
    // own MAC, which is the fallback working exactly as designed: it booted on a
    // bench next to the live node and could not have taken its name. That is
    // what the fallback is for, and it is also why rostering is not urgent —
    // only tidy. The derived name is correct, just unreadable.
    ("48:CA:43:4B:95:F8", "obc-esp32-s3-003"),
    // Spare XIAO ESP32S3. MAC read from Windows PnP 2026-09-25, where native
    // USB carries it as the composite device's serial:
    //   USB\VID_303A&PID_1001\64:E8:33:7F:84:CC
    //
    // It had been on the bench unrostered, self-naming `obc-esp32-s3-7f84cc`.
    // Same OUI as 001 and 002; its fourth byte (7F) is the first to differ from
    // theirs (7E), so it was never at risk of a name collision either way.
    ("64:E8:33:7F:84:CC", "obc-esp32-s3-004"),
    // Second XIAO ESP32S3 Sense, fitted with an OV3660 (PID 0x3660 at SCCB
    // 0x3c) where 002 has an OV2640. MAC read by espflash while flashing it,
    // 2026-09-25:
    //   MAC address: ac:27:6e:a8:4d:e4
    //
    // A different OUI from the other XIAOs (AC:27:6E rather than 64:E8:33),
    // so the Seeed parts on this bench come from more than one batch. It ran
    // unrostered as `obc-esp32-s3-a84de4` while it served as the second board
    // that cleared the Sense capture fault: 10/10 frames once the I2S mic
    // stopped resetting the camera's GDMA channel.
    ("AC:27:6E:A8:4D:E4", "obc-esp32-s3-005"),
];

/// What a board calls itself when its MAC cannot be read at all.
///
/// Not a fleet name, and deliberately not parseable as one: a node that does not
/// know what it is should be obviously broken, not quietly someone else.
pub const UNIDENTIFIED: &str = "obc-esp32-s3-unidentified";

/// The all-zero MAC, used as the sentinel for "efuse read failed".
pub const NO_MAC: &str = "00:00:00:00:00:00";

/// Format six raw bytes as `AA:BB:CC:DD:EE:FF`.
pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Resolve a formatted MAC to a node id: roster first, then the derived tail.
///
/// The fallback is the last **three** bytes, because the bench's own two boards
/// differ only in those — `64:E8:33:7E:BB:98` and `64:E8:33:7E:7E:04` share the
/// first three. A shorter tail would have reproduced the collision this whole
/// module exists to end.
pub fn id_for(mac_str: &str) -> String {
    if mac_str == NO_MAC {
        return UNIDENTIFIED.to_string();
    }
    for (mac, id) in ROSTER {
        if mac.eq_ignore_ascii_case(mac_str) {
            return (*id).to_string();
        }
    }
    let tail: String = mac_str
        .split(':')
        .skip(3)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    format!("obc-esp32-s3-{tail}")
}

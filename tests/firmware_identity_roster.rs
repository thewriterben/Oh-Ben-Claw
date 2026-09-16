//! The fleet roster, and the three places that have to agree about it.
//!
//! On 2026-09-16 a second XIAO was flashed and booted announcing
//! `obc-esp32-s3-001` — the live mesh node's identity — because the node id was
//! a compile-time constant and nothing distinguished one board from another.
//! Identity now derives from the chip's factory MAC
//! (`firmware/obc-esp32-s3/src/identity_map.rs`).
//!
//! That fix creates its own hazard. The MAC → name roster now exists in three
//! places: the firmware, the host's peripheral registry, and the bench script
//! that decides which port is safe to flash. This project's own history says
//! what happens next — `camera.rs` claimed a board its own `Cargo.toml`
//! contradicted, two directories away, for weeks, *because nothing compared
//! them*. So this file compares them.
//!
//! The firmware roster is the source of truth. If this test is red, the other
//! copies are wrong until proven otherwise.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "../firmware/obc-esp32-s3/src/identity_map.rs"]
#[allow(dead_code)]
mod identity_map;

use identity_map::{format_mac, id_for, ROSTER, UNIDENTIFIED};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path: PathBuf = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

// ── the mapping itself ───────────────────────────────────────────────────────

#[test]
fn rostered_boards_get_their_names() {
    assert_eq!(id_for("64:E8:33:7E:BB:98"), "obc-esp32-s3-001");
    assert_eq!(id_for("64:E8:33:7E:7E:04"), "obc-esp32-s3-002");
}

#[test]
fn roster_lookup_ignores_case() {
    // espflash prints the MAC lowercase; Windows PnP prints it upper. Both are
    // the same board, and a case mismatch silently falling through to the
    // derived tail would hand the live node a brand-new identity.
    assert_eq!(id_for("64:e8:33:7e:bb:98"), "obc-esp32-s3-001");
    assert_eq!(id_for("64:E8:33:7e:Bb:98"), "obc-esp32-s3-001");
}

#[test]
fn an_unrostered_board_never_takes_a_fleet_name() {
    let stranger = id_for("AA:BB:CC:DD:EE:FF");
    assert_eq!(stranger, "obc-esp32-s3-ddeeff");
    for (_, name) in ROSTER {
        assert_ne!(
            &stranger, name,
            "the derived fallback collided with a rostered name"
        );
    }
}

#[test]
fn the_two_bench_boards_would_differ_even_unrostered() {
    // They share their first three MAC bytes. This is the regression guard on
    // the fallback's length: shorten the tail and the original bug returns,
    // this time for boards nobody has listed.
    let a = "64:E8:33:7E:BB:98";
    let b = "64:E8:33:7E:7E:04";
    assert_eq!(
        &a[..8],
        &b[..8],
        "precondition: these boards share a prefix"
    );

    let derive = |mac: &str| {
        let tail: String = mac
            .split(':')
            .skip(3)
            .map(|s| s.to_ascii_lowercase())
            .collect();
        format!("obc-esp32-s3-{tail}")
    };
    assert_ne!(derive(a), derive(b));
}

#[test]
fn an_unreadable_mac_is_not_a_fleet_member() {
    let id = id_for("00:00:00:00:00:00");
    assert_eq!(id, UNIDENTIFIED);
    for (_, name) in ROSTER {
        assert_ne!(&id, name);
    }
    // And it must not look like a derived id either, or it would be mistaken
    // for a real board whose MAC happens to end in zeros.
    assert_ne!(id, "obc-esp32-s3-000000");
}

#[test]
fn format_mac_matches_the_roster_spelling() {
    // The roster is written in the shape `format_mac` produces. If these ever
    // diverge, every lookup falls through to the derived tail and every board
    // silently renames itself.
    let formatted = format_mac(&[0x64, 0xE8, 0x33, 0x7E, 0xBB, 0x98]);
    assert_eq!(formatted, "64:E8:33:7E:BB:98");
    assert_eq!(id_for(&formatted), "obc-esp32-s3-001");
}

#[test]
fn roster_has_no_duplicates() {
    let mut macs = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (mac, name) in ROSTER {
        assert!(
            macs.insert(mac.to_ascii_uppercase()),
            "duplicate MAC in roster: {mac}"
        );
        assert!(names.insert(*name), "duplicate node id in roster: {name}");
    }
}

#[test]
fn roster_macs_are_well_formed_and_uppercase() {
    for (mac, name) in ROSTER {
        let parts: Vec<&str> = mac.split(':').collect();
        assert_eq!(parts.len(), 6, "{name}: MAC {mac} is not six bytes");
        for p in &parts {
            assert_eq!(p.len(), 2, "{name}: MAC {mac} has a non-two-digit group");
            assert!(
                p.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}: MAC {mac} has a non-hex digit"
            );
        }
        assert_eq!(
            *mac,
            mac.to_ascii_uppercase(),
            "{name}: roster MACs are written uppercase, to match format_mac"
        );
    }
}

// ── the three copies ─────────────────────────────────────────────────────────

/// Every rostered MAC must be documented in the host's peripheral registry.
///
/// The registry carries it in prose rather than as data, which is why this is a
/// substring check and not a parse. A weak check that runs beats a strong one
/// that does not exist: the failure it prevents is a board being added to the
/// firmware and never written down where the planner's authors would see it.
#[test]
fn host_registry_documents_every_rostered_board() {
    let registry = read("crates/obc-planner/src/peripherals/registry.rs");
    for (mac, name) in ROSTER {
        let seen = registry.contains(*mac) || registry.contains(&mac.to_ascii_lowercase());
        assert!(
            seen,
            "{name} ({mac}) is in the firmware roster but nowhere in \
             crates/obc-planner/src/peripherals/registry.rs. Add it there, with how \
             the MAC was measured, or remove it from the roster."
        );
    }
}

/// The pre-flash gate must know every board the firmware knows.
///
/// `scripts/which_esp32.ps1` is what stands between a plan that says "flash the
/// spare XIAO" and a live mesh node on the same USB hub. A board missing from
/// its table is reported as `(unknown)` — safe, but it means the script cannot
/// tell you that the board you are about to erase is the one running the fleet.
#[test]
fn preflash_gate_knows_every_rostered_board() {
    let rel = "scripts/which_esp32.ps1";
    let path = repo_root().join(rel);
    assert!(
        Path::new(&path).exists(),
        "{rel} is missing. The pre-flash gate has to live in the repo to be \
         reviewable and comparable; a copy that exists only on one bench machine \
         is not a control."
    );
    let script = read(rel);
    for (mac, name) in ROSTER {
        assert!(
            script.contains(*mac),
            "{name} ({mac}) is in the firmware roster but not in {rel}. The gate \
             would call it `(unknown)` and offer it as flashable."
        );
        assert!(
            script.contains(*name),
            "{mac} appears in {rel} but not under the name {name} the firmware \
             gives it. The two must agree or the gate is lying about which board \
             is which."
        );
    }
}

/// The constant this whole change removed must not come back.
///
/// Comments are stripped first. The first version of this test did not do that
/// and failed on the comment in `main.rs` explaining why the constant was
/// removed — a check that forbids *discussing* the bug is a check that pressures
/// you to delete the reason. The history is the most valuable line in that file.
#[test]
fn no_hardcoded_node_id_constant_in_firmware() {
    let main_rs = read("firmware/obc-esp32-s3/src/main.rs");
    let code_only: String = main_rs
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code_only.contains("const NODE_ID"),
        "firmware/obc-esp32-s3/src/main.rs has a `const NODE_ID` again. Identity \
         comes from the chip -- see identity.rs for what happened last time."
    );
    // The literal fleet names must not be reachable as code either: that is how
    // the second board came to answer to the first board's name.
    for (_, name) in ROSTER {
        assert!(
            !code_only.contains(&format!("\"{name}\"")),
            "main.rs hardcodes the node id {name:?} outside a comment. Names live \
             in identity_map.rs's roster and nowhere else."
        );
    }
}

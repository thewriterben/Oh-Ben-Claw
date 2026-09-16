//! Who this board is — derived from the chip, not from a constant.
//!
//! # Why this module exists
//!
//! Until 2026-09-16 the node id was `const NODE_ID: &str = "obc-esp32-s3-001"`,
//! with a doc comment saying it "should be read from NVS in production". That
//! day a second XIAO was flashed for camera bring-up and booted announcing
//! `Node ID: obc-esp32-s3-001` — the same identity as the live mesh node, and
//! already emitting `link_state` JSON under it on the spine UART. Nothing was on
//! the air only because that board's UART was not yet wired to a radio, and the
//! next step in the plan was to wire it to one.
//!
//! The mesh supervisor keys everything on node id. Two boards sharing one is not
//! cosmetic: it is the same identity confusion the on-air forgery work was built
//! to detect, arriving from inside the fleet rather than from an attacker.
//!
//! # The rule
//!
//! Identity comes from the chip's factory MAC, which is unique per device and
//! cannot be forgotten, mis-set, or copied along with a build command. The
//! mapping itself lives in [`crate::identity_map`], where it can be tested on the
//! host; this module is only the hardware read and the once-per-boot cache.
//!
//! The fallback for an unrostered board is deliberately ugly and deliberately
//! *not* a fixed string. A fixed fallback is what produced the collision — a
//! default is what lets a wrong value survive unnoticed (see `camera.rs`, where
//! a default pin map outlived two corrections).
//!
//! # NVS
//!
//! Not used, though the old comment promised it and the machinery exists
//! (`rules_store` is NVS-backed). NVS would let a board be renamed without a
//! reflash — a capability nothing needs today — and it would need an answer for
//! an unprovisioned board, which is the question that has no safe default.
//! Revisit when a board must be renamed in the field, or when identity has to
//! survive a chip swap.

use crate::identity_map::{format_mac, id_for, NO_MAC, UNIDENTIFIED};
use esp_idf_svc::sys::{esp_efuse_mac_get_default, ESP_OK};
use std::sync::OnceLock;

static NODE_ID: OnceLock<String> = OnceLock::new();
static MAC: OnceLock<String> = OnceLock::new();

/// Read the factory MAC from efuse. `None` if the call fails.
///
/// `esp_efuse_mac_get_default` rather than `esp_read_mac`: the latter derives
/// per-interface addresses (WiFi STA/AP, BT, ETH) from the base, so it answers a
/// different question — "what address does this interface use" — and its answer
/// changes with the interface. Identity wants the one value underneath them all.
fn read_mac() -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let err = unsafe { esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    if err != ESP_OK {
        log::error!("esp_efuse_mac_get_default failed (error {err}); identity is degraded");
        return None;
    }
    Some(mac)
}

/// Resolve this board's identity. Call once, early in `main`.
///
/// Returns `(node_id, mac)` for logging. Idempotent; later calls return the
/// first result.
pub fn init() -> (&'static str, &'static str) {
    let mac_str = MAC.get_or_init(|| match read_mac() {
        Some(mac) => format_mac(&mac),
        None => NO_MAC.to_string(),
    });
    let id = NODE_ID.get_or_init(|| id_for(mac_str));
    if id == UNIDENTIFIED {
        log::error!(
            "identity: factory MAC unreadable. This node is `{UNIDENTIFIED}` and must \
             not be put on a shared mesh -- it cannot prove which board it is."
        );
    }
    (id.as_str(), mac_str.as_str())
}

/// This board's node id. Panics if [`init`] has not run — that is a programming
/// error at boot, not a runtime condition, and answering to a wrong name is
/// worse than a crash.
pub fn node_id() -> &'static str {
    NODE_ID
        .get()
        .expect("identity::init() must run before node_id()")
        .as_str()
}

/// This board's factory MAC, formatted `AA:BB:CC:DD:EE:FF`.
pub fn mac() -> &'static str {
    MAC.get()
        .expect("identity::init() must run before mac()")
        .as_str()
}

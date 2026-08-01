//! Spine frame authentication, node side — the primitive, ahead of the wire.
//!
//! Step 2 of `SPINE-AUTH.md`: the node can compute the same tag the host does.
//! **Nothing calls this yet.** No frame carries a tag, no receiver checks one,
//! and `spine.rs` is untouched. That is deliberate: step 4 changes the wire
//! format, and a wire change is worth making once both ends have been proven to
//! agree on the arithmetic rather than after.
//!
//! The host's copy is `crates/obc-safety/src/spine_tag.rs`. This file is the
//! mirror, and `tests/spine_auth_vectors.rs` compiles both and fails if they
//! ever disagree — the arrangement `safety.rs` already uses to stay
//! wire-compatible with the host's `SafetyLimit`.
//!
//! ## Two deliberate choices
//!
//! **Portable SHA, not the ESP32-S3's hardware accelerator.** `SPINE-AUTH.md`
//! notes the silicon and it is the right destination, but the design has never
//! been run on a board and this crate cannot be compiled anywhere the author can
//! check it. A pure-Rust `sha2` compiles for host and target alike, so this
//! module is testable *today*, on the machine writing it. Swapping in
//! `esp_idf_sys`'s mbedtls is an optimisation with a known answer to check
//! against — which is a much better position than the reverse.
//!
//! **No `no_std`.** This firmware is an ESP-IDF `std` crate; matching that
//! keeps the module includable by the host harness with no cfg dance.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Bytes of HMAC carried on the wire. 8, not 32: on a 240-byte frame a full tag
/// is 13% of every transmission and eight bytes is 3.3%. The trade is a 2⁻⁶⁴
/// per-attempt forgery probability against a link that carries a few frames a
/// second.
pub const TAG_LEN: usize = 8;

/// Domain separator for key derivation; must match the host's `KDF_SALT`.
pub const KDF_SALT: &[u8] = b"obc-spine-v1";

/// Per-node key: `HKDF-SHA256(root_secret, salt = "obc-spine-v1", info = node_id)`.
///
/// In service this node is flashed with its derived key and never sees the root
/// secret — the function is here so the derivation can be tested against the
/// host's, and so a provisioning tool can run the same code.
pub fn derive_node_key(root_secret: &[u8], node_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(KDF_SALT), root_secret);
    let mut key = [0u8; 32];
    hk.expand(node_id.as_bytes(), &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// The authenticated bytes: `src ‖ ctr(big-endian) ‖ payload`.
///
/// `ttl` is excluded on purpose. Relays decrement it in flight, so a tag over
/// `ttl` would verify at one hop and fail at the next — the flood relay this
/// mesh depends on would stop working, in a way that looks like an attack.
fn signed_region(src: u8, ctr: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 4 + payload.len());
    buf.push(src);
    buf.extend_from_slice(&ctr.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Compute the truncated tag for a frame.
pub fn tag(node_key: &[u8; 32], src: u8, ctr: u32, payload: &[u8]) -> [u8; TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(node_key).expect("HMAC accepts any key length");
    mac.update(&signed_region(src, ctr, payload));
    let full = mac.finalize().into_bytes();
    let mut out = [0u8; TAG_LEN];
    out.copy_from_slice(&full[..TAG_LEN]);
    out
}

/// Verify a tag in constant time.
///
/// Constant-time matters more here than on the host: this end is the one an
/// attacker can hammer over the air, and an early-return comparison would let
/// them recover a valid tag a byte at a time instead of guessing all eight.
pub fn verify(node_key: &[u8; 32], src: u8, ctr: u32, payload: &[u8], received: &[u8]) -> bool {
    if received.len() != TAG_LEN {
        return false;
    }
    let mut mac = HmacSha256::new_from_slice(node_key).expect("HMAC accepts any key length");
    mac.update(&signed_region(src, ctr, payload));
    mac.verify_truncated_left(received).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_verifies_against_itself() {
        let k = derive_node_key(b"root", "node-a");
        let t = tag(&k, 3, 9, b"hello");
        assert!(verify(&k, 3, 9, b"hello", &t));
    }

    #[test]
    fn a_forged_payload_does_not_verify() {
        let k = derive_node_key(b"root", "node-a");
        let t = tag(&k, 3, 9, b"hello");
        assert!(!verify(&k, 3, 9, b"hellp", &t));
    }

    #[test]
    fn the_counter_is_covered() {
        let k = derive_node_key(b"root", "node-a");
        let t = tag(&k, 3, 9, b"hello");
        assert!(!verify(&k, 3, 10, b"hello", &t));
    }
}

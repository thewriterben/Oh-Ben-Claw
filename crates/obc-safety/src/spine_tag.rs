//! Spine frame authentication — the primitive, ahead of the wire format.
//!
//! Step 2 of `OBC-Prime/docs/SPINE-AUTH.md`: *"Node-side HMAC-SHA256 …
//! verified against a host-side test vector. **No wire change yet.**"* Nothing
//! here is sent or checked on any link. It is the shared arithmetic both ends
//! will use in step 4, landed first so that host and node can be proven to agree
//! before either starts depending on the other being right.
//!
//! The mirror is `firmware/heltec-lora-linktest/src/auth.rs`, and
//! `tests/spine_auth_vectors.rs` compiles both and fails if they ever disagree —
//! the same arrangement that keeps the node's `SafetyLimit` wire-compatible with
//! this crate's.
//!
//! ## What this is not
//!
//! - **Not confidentiality.** A MAC stops forgery, not reading. MQTT stays
//!   cleartext and `SAFETY.md` should keep saying so.
//! - **Not a defence against a compromised host.** The host holds the root
//!   secret. Track 0's limit table on the microcontroller remains the only
//!   boundary that survives that, and this does not replace it.
//! - **Not 256-bit security.** The tag is truncated to 64 bits, a deliberate
//!   trade against a 240-byte frame: an online forgery attempt succeeds with
//!   probability 2⁻⁶⁴ per try against a link that carries a few frames a second.
//!   Say the number rather than implying the hash length.
//!
//! ## Why one key per node
//!
//! `derive_node_key` gives each node a key it cannot use to impersonate a
//! sibling, from one secret the host actually has to store. The cost is
//! revocation: retiring one node means a new root and a reflash of every other,
//! which is the honest price of pre-shared symmetric keys and the reason
//! `SPINE-AUTH.md` §5 keeps an asymmetric option open.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Bytes of HMAC kept on the wire. See the truncation note above.
pub const TAG_LEN: usize = 8;

/// Domain separator for key derivation. Bump the version if the scheme changes,
/// so keys derived under the old rules cannot verify under the new ones.
pub const KDF_SALT: &[u8] = b"obc-spine-v1";

/// Per-node key: `HKDF-SHA256(root_secret, salt = "obc-spine-v1", info = node_id)`.
///
/// Infallible for every output length this uses (32 bytes is one block).
pub fn derive_node_key(root_secret: &[u8], node_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(KDF_SALT), root_secret);
    let mut key = [0u8; 32];
    hk.expand(node_id.as_bytes(), &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// The authenticated bytes: `src ‖ ctr(big-endian) ‖ payload`.
///
/// `ttl` is deliberately absent — relays decrement it in flight, so covering it
/// would make every relayed frame fail verification at the far end.
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
/// `verify_truncated_left` rather than `tag(..) == received`: a byte-by-byte
/// comparison that returns early leaks, through timing, how much of a forged tag
/// was right — which is enough to reconstruct one byte at a time and defeats the
/// 2⁻⁶⁴ argument above entirely.
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

    const ROOT: &[u8] = b"a-root-secret-for-one-deployment";

    #[test]
    fn a_tag_verifies_against_itself() {
        let k = derive_node_key(ROOT, "obc-esp32-s3-001");
        let t = tag(&k, 28, 1837, b"{\"cmd\":\"gpio_write\"}");
        assert!(verify(&k, 28, 1837, b"{\"cmd\":\"gpio_write\"}", &t));
    }

    #[test]
    fn every_authenticated_field_changes_the_tag() {
        let k = derive_node_key(ROOT, "obc-esp32-s3-001");
        let base = tag(&k, 28, 1837, b"payload");
        assert_ne!(base, tag(&k, 29, 1837, b"payload"), "src is not covered");
        assert_ne!(base, tag(&k, 28, 1838, b"payload"), "ctr is not covered");
        assert_ne!(
            base,
            tag(&k, 28, 1837, b"payloae"),
            "payload is not covered"
        );
    }

    /// The replay defence is the counter, and it only works because the counter
    /// is inside the MAC: a captured frame replayed with a fresh counter has to
    /// fail, or the counter is decoration.
    #[test]
    fn replaying_a_frame_under_a_new_counter_fails() {
        let k = derive_node_key(ROOT, "obc-esp32-s3-001");
        let captured = tag(
            &k,
            28,
            100,
            b"{\"cmd\":\"gpio_write\",\"args\":{\"pin\":3,\"value\":1}}",
        );
        assert!(!verify(
            &k,
            28,
            101,
            b"{\"cmd\":\"gpio_write\",\"args\":{\"pin\":3,\"value\":1}}",
            &captured
        ));
    }

    /// The point of per-node derivation: a node cannot sign for its neighbour.
    #[test]
    fn one_nodes_key_does_not_verify_anothers_frame() {
        let a = derive_node_key(ROOT, "obc-esp32-s3-001");
        let b = derive_node_key(ROOT, "obc-esp32-s3-002");
        assert_ne!(a, b);
        let from_a = tag(&a, 28, 7, b"telemetry");
        assert!(!verify(&b, 28, 7, b"telemetry", &from_a));
    }

    #[test]
    fn a_different_root_secret_gives_a_different_key() {
        let a = derive_node_key(b"root-one", "obc-esp32-s3-001");
        let b = derive_node_key(b"root-two", "obc-esp32-s3-001");
        assert_ne!(a, b);
    }

    /// A tag of the wrong length is refused rather than compared against a
    /// prefix — otherwise a one-byte tag would verify one byte of the truth.
    #[test]
    fn a_short_tag_is_refused() {
        let k = derive_node_key(ROOT, "obc-esp32-s3-001");
        let t = tag(&k, 28, 1, b"x");
        assert!(!verify(&k, 28, 1, b"x", &t[..4]));
        assert!(!verify(&k, 28, 1, b"x", &[]));
    }

    /// `ttl` is not in the signed region, which is a design decision rather than
    /// an oversight: relays decrement it. Expressed as a test because the
    /// alternative — discovering it at the third hop on a bench — is expensive.
    #[test]
    fn the_signed_region_is_exactly_src_ctr_payload() {
        assert_eq!(
            signed_region(0x1c, 0x0000_072d, b"ab"),
            vec![0x1c, 0x00, 0x00, 0x07, 0x2d, b'a', b'b']
        );
    }
}

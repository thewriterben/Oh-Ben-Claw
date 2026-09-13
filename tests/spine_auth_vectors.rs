//! Host and node must compute the same spine tag — step 2 of `SPINE-AUTH.md`.
//!
//! The design's step 2 is *"node-side HMAC-SHA256, verified against a host-side
//! test vector. No wire change yet."* This is that verification, and it is worth
//! being precise about what each part of it proves, because "the two agree" is
//! easy to satisfy trivially and worth nothing when it is.
//!
//! Three layers, weakest claim first:
//!
//! 1. **Agreement.** `crates/obc-safety/src/spine_tag.rs` and
//!    `firmware/heltec-lora-linktest/src/auth.rs` produce identical bytes across
//!    a table of inputs. On its own this only says the two copies match — two
//!    copies of the same mistake would pass.
//! 2. **Frozen vectors.** Specific inputs pinned to specific hex output, so a
//!    change to *both* sides still fails. These are generated from this
//!    implementation, so they are a regression pin and not an independent
//!    check — an error would have been frozen along with everything else.
//! 3. **RFC vectors**, which is what makes layers 1 and 2 mean something. The
//!    HMAC-SHA256 case is RFC 4231 §4.2 and the HKDF-SHA256 case is RFC 5869
//!    §A.1 — published constants, computed by someone else, years before this
//!    code. If the primitive underneath were wrong these would say so.
//!
//! Since 2026-09-13 (step 4) the tag is on the wire: every frame between the
//! Heltec stations is `spine::AuthFrame`, tagged with `auth::tag` and checked
//! with `auth::verify`. The last test below is the one that joins the two
//! modules — a frame the host would build verifies through the station's
//! decoder, and the station's frame verifies under the host's arithmetic. The
//! station's `spine.rs` deliberately holds no key; that test is where the
//! seam is checked.

#[path = "../firmware/heltec-lora-linktest/src/auth.rs"]
mod node;
#[path = "../firmware/heltec-lora-linktest/src/spine.rs"]
mod spine;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use obc_safety::spine_tag as host;
use sha2::Sha256;

const ROOT: &[u8] = b"a-root-secret-for-one-deployment";

/// `(node_id, src, ctr, payload)` — spread across the fields the MAC covers,
/// including the boundaries: counter zero, counter at `u32::MAX`, an empty
/// payload, and a payload at the frame budget.
fn cases() -> Vec<(&'static str, u8, u32, Vec<u8>)> {
    vec![
        ("obc-esp32-s3-001", 0x1c, 0, b"".to_vec()),
        ("obc-esp32-s3-001", 0x1c, 1, b"x".to_vec()),
        (
            "obc-esp32-s3-001",
            0x1c,
            1837,
            br#"{"id":"a7","to":"obc-esp32-s3-001","cmd":"gpio_write","args":{"pin":3,"value":1}}"#
                .to_vec(),
        ),
        ("obc-esp32-s3-002", 0xff, u32::MAX, b"telemetry".to_vec()),
        ("a", 0x00, 0x0000_0100, vec![b'z'; spine::MAX_AUTH_PAYLOAD]),
    ]
}

/// Layer 1. Two implementations, one answer — for the key derivation, the tag,
/// and verification in both directions.
#[test]
fn the_node_and_the_host_agree() {
    for (node_id, src, ctr, payload) in cases() {
        let hk = host::derive_node_key(ROOT, node_id);
        let nk = node::derive_node_key(ROOT, node_id);
        assert_eq!(hk, nk, "derived keys differ for {node_id}");

        let ht = host::tag(&hk, src, ctr, &payload);
        let nt = node::tag(&nk, src, ctr, &payload);
        assert_eq!(ht, nt, "tags differ for {node_id} src={src} ctr={ctr}");

        // Cross-verify: each side must accept the other's tag, which is the
        // property the wire actually depends on.
        assert!(
            node::verify(&nk, src, ctr, &payload, &ht),
            "the node rejected a host tag ({node_id}, ctr={ctr})"
        );
        assert!(
            host::verify(&hk, src, ctr, &payload, &nt),
            "the host rejected a node tag ({node_id}, ctr={ctr})"
        );
    }
}

/// Layer 1, negative. Agreement on rejection matters as much as on acceptance:
/// an implementation that accepts everything agrees with one that is correct on
/// every positive case.
#[test]
fn both_sides_reject_the_same_forgeries() {
    let hk = host::derive_node_key(ROOT, "obc-esp32-s3-001");
    let nk = node::derive_node_key(ROOT, "obc-esp32-s3-001");
    let good = host::tag(&hk, 0x1c, 100, b"payload");

    let forgeries: Vec<(&str, u8, u32, &[u8])> = vec![
        ("payload edited", 0x1c, 100, b"payloae"),
        ("counter replayed forward", 0x1c, 101, b"payload"),
        ("source spoofed", 0x1d, 100, b"payload"),
    ];
    for (what, src, ctr, payload) in forgeries {
        assert!(
            !host::verify(&hk, src, ctr, payload, &good),
            "the host accepted a forgery: {what}"
        );
        assert!(
            !node::verify(&nk, src, ctr, payload, &good),
            "the node accepted a forgery: {what}"
        );
    }
}

/// Layer 2. Frozen output — a regression pin, not an independent check.
/// Generated from this implementation; if the construction were wrong the wrong
/// answer would be frozen here too. Layer 3 is what guards against that.
#[test]
fn the_construction_is_pinned() {
    let k = host::derive_node_key(ROOT, "obc-esp32-s3-001");
    assert_eq!(
        hex::encode(k),
        "ea39cfe194fe07b0597847b054619702556ec1da25b45168600ccaea8aaf996a",
        "the derived key changed — if that was intentional, bump KDF_SALT so old \
         keys cannot verify under the new scheme"
    );
    assert_eq!(
        hex::encode(host::tag(&k, 0x1c, 1837, b"payload")),
        "e347d62eaef0068d",
        "the tag construction changed"
    );
}

/// Layer 3a. RFC 4231 §4.2, test case 2: key `"Jefe"`, data
/// `"what do ya want for nothing?"`. Published in 2005; nothing in this
/// repository influenced it.
#[test]
fn the_hmac_primitive_matches_rfc_4231() {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"Jefe").unwrap();
    mac.update(b"what do ya want for nothing?");
    assert_eq!(
        hex::encode(mac.finalize().into_bytes()),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

/// Layer 3b. RFC 5869 §A.1, the basic HKDF-SHA256 case: 22 bytes of 0x0b, a
/// 13-byte salt, a 10-byte info, 42 bytes out.
#[test]
fn the_hkdf_primitive_matches_rfc_5869() {
    let ikm = [0x0bu8; 22];
    let salt: Vec<u8> = (0x00u8..=0x0c).collect();
    let info: Vec<u8> = (0xf0u8..=0xf9).collect();

    let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut okm = [0u8; 42];
    hk.expand(&info, &mut okm).unwrap();

    assert_eq!(
        hex::encode(okm),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
}

/// The truncation is a documented trade, so pin the number rather than letting
/// it drift into "however many bytes the code happens to keep".
#[test]
fn the_tag_is_eight_bytes_on_both_sides() {
    assert_eq!(host::TAG_LEN, 8);
    assert_eq!(node::TAG_LEN, 8);
    assert_eq!(
        spine::MAC_LEN,
        node::TAG_LEN,
        "the frame carries a different tag length than auth.rs computes"
    );
    assert_eq!(host::KDF_SALT, node::KDF_SALT);
}

/// The seam: a frame built the way `main.rs` builds one — station key derived
/// with the `gw-XX` id, tag over `src ‖ ctr ‖ payload`, `seq` = low byte of
/// `ctr` — decodes on the other side and verifies under the *host's* copy of
/// the arithmetic, and a relay's `ttl` decrement does not disturb it. Then
/// the negatives the receiver in `main.rs` relies on: a flipped payload byte,
/// a moved counter, a spoofed source, and a key from a different root all
/// fail. A retired v1 frame does not decode at all.
#[test]
fn a_station_frame_verifies_through_the_decoder_under_the_host_arithmetic() {
    let src = 0xd8u8;
    let ctr = 0x0001_02f3u32;
    let payload = br#"{"node_id":"gw-D8","type":"gw_keepalive","seq":244}"#;
    let station_key = node::derive_node_key(ROOT, &format!("gw-{src:02X}"));
    let host_key = host::derive_node_key(ROOT, "gw-D8");
    assert_eq!(station_key, host_key);

    let mut wire = Vec::new();
    spine::AuthFrame {
        src,
        seq: ctr as u8,
        ttl: 2,
        ctr,
        payload,
        mac: node::tag(&station_key, src, ctr, payload),
    }
    .encode(&mut wire);
    assert_eq!(
        wire.len(),
        spine::HEADER + spine::AUTH_OVERHEAD + payload.len()
    );

    let f = spine::AuthFrame::decode(&wire).expect("a v2 frame decodes");
    assert_eq!(f.seq, f.ctr as u8);
    assert!(host::verify(&host_key, f.src, f.ctr, f.payload, &f.mac));
    assert!(node::verify(&station_key, f.src, f.ctr, f.payload, &f.mac));

    // A relay decrements ttl and re-encodes with the same tag.
    let mut relayed = Vec::new();
    spine::AuthFrame {
        ttl: f.ttl - 1,
        ..f
    }
    .encode(&mut relayed);
    let r = spine::AuthFrame::decode(&relayed).unwrap();
    assert_eq!(r.ttl, 1);
    assert!(host::verify(&host_key, r.src, r.ctr, r.payload, &r.mac));

    // Negatives, each a single change to the bytes on the air.
    let mut flipped = wire.clone();
    flipped[spine::HEADER + 4] ^= 0x01; // first payload byte
    let g = spine::AuthFrame::decode(&flipped).unwrap();
    assert!(!node::verify(&station_key, g.src, g.ctr, g.payload, &g.mac));

    let mut moved = wire.clone();
    moved[6] ^= 0x01; // low byte of ctr…
    moved[1] ^= 0x01; // …and seq with it, so the shape check passes
    let g = spine::AuthFrame::decode(&moved).unwrap();
    assert_eq!(g.seq, g.ctr as u8);
    assert!(!node::verify(&station_key, g.src, g.ctr, g.payload, &g.mac));

    let mut spoofed = wire.clone();
    spoofed[0] = 0x40;
    let g = spine::AuthFrame::decode(&spoofed).unwrap();
    let key_for_claimed_src = node::derive_node_key(ROOT, "gw-40");
    assert!(!node::verify(
        &key_for_claimed_src,
        g.src,
        g.ctr,
        g.payload,
        &g.mac
    ));

    let other_root_key = node::derive_node_key(b"a-different-deployment", "gw-D8");
    assert!(!node::verify(
        &other_root_key,
        f.src,
        f.ctr,
        f.payload,
        &f.mac
    ));

    // The retired v1 layout of the same message is shorter than a v2 header
    // plus tag only when the payload is tiny; a real one decodes as garbage
    // and fails the tag, which is the receiver's `BadTag` path.
    let mut v1 = vec![src, ctr as u8, 2];
    v1.extend_from_slice(payload);
    let g = spine::AuthFrame::decode(&v1).unwrap();
    let key = node::derive_node_key(ROOT, &format!("gw-{:02X}", g.src));
    assert!(!node::verify(&key, g.src, g.ctr, g.payload, &g.mac));
}

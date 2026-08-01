//! A tagged tool result survives the wire; a tampered one does not.
//!
//! `obc-safety` tests the verifier against byte slices it makes up. This tests
//! the join — that `ToolCallResult::signed_bytes` produces the *same* bytes on
//! the sending and receiving side, after a round trip through JSON.
//!
//! That join is where this kind of scheme usually breaks. The receiver cannot
//! re-derive the sender's exact text: field order, spacing and the presence of
//! `null`s are all serializer choices, and a MAC over "the message as it
//! arrived" is a MAC over a string neither end agrees on. Re-serializing a
//! canonical subset — the message minus its own signature — is the shape that
//! works, and this is the test that says so.
//!
//! Scope, stated because the module name overpromises: this covers **MQTT/P2P**,
//! where `SPINE-AUTH.md` §3.3 puts the tag in a field. The LoRa frame format is
//! step 4 and is untouched — no frame carries a tag over the radio.

use obc_safety::frame_auth::{FrameAuth, FrameVerdict};
use oh_ben_claw::spine::ToolCallResult;

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const NODE: &str = "obc-esp32-s3-001";

fn result(output: &str) -> ToolCallResult {
    ToolCallResult {
        call_id: "6f1a3c58".to_string(),
        ok: true,
        output: Some(output.to_string()),
        error: None,
        ctr: None,
        mac: None,
    }
}

/// Sign, serialize, parse, verify — the actual path a result takes.
#[test]
fn a_signed_result_verifies_after_a_json_round_trip() {
    let auth = FrameAuth::new(Some(SECRET.to_string()), true);

    let mut sent = result("21.5");
    sent.ctr = Some(1);
    sent.mac = auth.tag_outbound(NODE, 1, &sent.signed_bytes());

    let wire = serde_json::to_vec(&sent).expect("serializes");
    let received: ToolCallResult = serde_json::from_slice(&wire).expect("parses");

    assert_eq!(
        auth.verify_inbound(
            NODE,
            received.ctr,
            received.mac.as_deref(),
            &received.signed_bytes()
        ),
        FrameVerdict::Accept
    );
}

/// The attack this is for: the reading is changed in flight, everything else
/// left alone.
#[test]
fn an_edited_reading_does_not_verify() {
    let auth = FrameAuth::new(Some(SECRET.to_string()), true);

    let mut sent = result("21.5");
    sent.ctr = Some(1);
    sent.mac = auth.tag_outbound(NODE, 1, &sent.signed_bytes());

    let mut tampered = sent.clone();
    tampered.output = Some("3.0".to_string()); // the value a safing rule watches

    assert!(matches!(
        auth.verify_inbound(
            NODE,
            tampered.ctr,
            tampered.mac.as_deref(),
            &tampered.signed_bytes()
        ),
        FrameVerdict::Reject { .. }
    ));
}

/// `signed_bytes` must not depend on whether the signature fields are populated,
/// or the sender signs one thing and the receiver checks another.
#[test]
fn the_signed_bytes_ignore_the_signature_fields() {
    let unsigned = result("21.5");
    let mut signed = result("21.5");
    signed.ctr = Some(99);
    signed.mac = Some("deadbeefdeadbeef".to_string());

    assert_eq!(unsigned.signed_bytes(), signed.signed_bytes());
}

/// An old result cannot be replayed even though its MAC is genuine — the
/// distinction the counter exists to make.
#[test]
fn a_captured_result_cannot_be_replayed() {
    let auth = FrameAuth::new(Some(SECRET.to_string()), true);

    let mut sent = result("21.5");
    sent.ctr = Some(5);
    sent.mac = auth.tag_outbound(NODE, 5, &sent.signed_bytes());

    let bytes = sent.signed_bytes();
    assert_eq!(
        auth.verify_inbound(NODE, sent.ctr, sent.mac.as_deref(), &bytes),
        FrameVerdict::Accept
    );
    assert!(matches!(
        auth.verify_inbound(NODE, sent.ctr, sent.mac.as_deref(), &bytes),
        FrameVerdict::Reject { .. }
    ));
}

/// A node that has not been upgraded sends neither field. It parses — the wire
/// format stays backward compatible — and whether it is *accepted* is the config
/// key, which is the distinction `SPINE-AUTH.md` §4 insists on: a migration
/// window is a config key that logs loudly and defaults off, not a silent
/// fallback to no authentication.
#[test]
fn an_untagged_result_still_parses_and_the_config_decides() {
    let wire = br#"{"call_id":"6f1a3c58","ok":true,"output":"21.5"}"#;
    let received: ToolCallResult = serde_json::from_slice(wire).expect("parses without ctr/mac");
    assert!(received.ctr.is_none());
    assert!(received.mac.is_none());

    let off = FrameAuth::new(Some(SECRET.to_string()), false);
    assert_eq!(
        off.verify_inbound(
            NODE,
            received.ctr,
            received.mac.as_deref(),
            &received.signed_bytes()
        ),
        FrameVerdict::Accept
    );

    let on = FrameAuth::new(Some(SECRET.to_string()), true);
    assert!(matches!(
        on.verify_inbound(
            NODE,
            received.ctr,
            received.mac.as_deref(),
            &received.signed_bytes()
        ),
        FrameVerdict::Reject { .. }
    ));
}

/// A signed result must not serialize its fields when they are absent, or every
/// existing consumer sees two new nulls it did not have.
#[test]
fn the_wire_is_unchanged_for_an_unsigned_result() {
    let wire = serde_json::to_string(&result("21.5")).expect("serializes");
    assert!(!wire.contains("ctr"), "got: {wire}");
    assert!(!wire.contains("mac"), "got: {wire}");
}

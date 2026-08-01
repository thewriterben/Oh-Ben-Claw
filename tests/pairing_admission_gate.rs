//! `[security] require_pairing = true` refuses unpaired nodes — step 5, first bullet.
//!
//! `OBC-Prime/docs/SPINE-AUTH.md` §3.4 lists this as *"the behaviour that key
//! already promises and does not deliver"*, and `MIGRATION.md` §2.4 measured the
//! shape of the lie: `NodePairingManager` is a field on `SecurityManager`, it is
//! constructed at startup, it is unit-tested — and `pair_node` had zero callers,
//! `is_trusted` had zero, and `require_pairing` was consulted only by config
//! validation, which refused to boot without a secret and then gated nothing.
//! *Reachable, instantiated, unit-tested, inert.*
//!
//! It matters beyond the key itself. `security/trust.rs` — dynamic trust scoring,
//! which *is* wired — opens with "OBC already authenticates nodes (HMAC pairing)
//! … that trust is *static*" and builds behavioural hardening on that premise.
//! The premise was false, so the hardening was scoring the trustworthiness of
//! nodes nobody had ever identified.
//!
//! What these tests drive is `spine::admit_announcement`, which exists as a
//! separate function precisely so they can: the decision used to be a line inside
//! an MQTT poll loop, and a security control no test can reach is the kind that
//! turns out not to work on the day it is needed.

use oh_ben_claw::security::pairing::PairingToken;
use oh_ben_claw::security::NodePairingManager;
use oh_ben_claw::spine::{admit_announcement, Admission};
use serde_json::json;

/// 32 hex chars, as `NodePairingManager::validate_secret` demands.
const SECRET: &str = "0123456789abcdef0123456789abcdef";
const NODE: &str = "obc-esp32-s3-001";

fn metadata_with_token(secret: &str, token_node: &str) -> serde_json::Value {
    let token = PairingToken::generate(secret, token_node).expect("token generates");
    json!({ "pairing_token": token })
}

#[test]
fn a_node_with_a_valid_token_is_admitted() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    assert_eq!(
        admit_announcement(&pairing, true, NODE, &metadata_with_token(SECRET, NODE)),
        Admission::Admit
    );
}

/// The case the key exists for, and the one that did nothing until 2026-08-01.
#[test]
fn a_node_with_no_token_is_refused_when_pairing_is_required() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    let admission = admit_announcement(&pairing, true, NODE, &json!({ "board": "esp32-s3" }));
    match admission {
        Admission::Refuse { reason } => assert!(
            reason.contains("no pairing token"),
            "the refusal should say what was missing, got: {reason}"
        ),
        Admission::Admit => panic!("an unpaired node was admitted while require_pairing was on"),
    }
}

/// A token signed with the wrong secret is a forgery attempt, not a mistake.
#[test]
fn a_token_signed_with_the_wrong_secret_is_refused() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    let forged = metadata_with_token("ffffffffffffffffffffffffffffffff", NODE);
    assert!(matches!(
        admit_announcement(&pairing, true, NODE, &forged),
        Admission::Refuse { .. }
    ));
}

/// A valid token for *another* node is the interesting forgery: the signature
/// verifies, and it is still not this node's.
#[test]
fn a_valid_token_for_a_different_node_is_refused() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    let someone_elses = metadata_with_token(SECRET, "obc-esp32-s3-002");
    match admit_announcement(&pairing, true, NODE, &someone_elses) {
        Admission::Refuse { reason } => assert!(
            reason.contains("mismatch"),
            "the refusal should name the mismatch, got: {reason}"
        ),
        Admission::Admit => panic!("a node replayed another node's token and was admitted"),
    }
}

/// With the key off, an unpaired node still announces — that is the documented
/// default and the behaviour every existing deployment has. The gate must be a
/// gate, not a global behaviour change dressed as one.
#[test]
fn an_unpaired_node_is_admitted_when_pairing_is_not_required() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    assert_eq!(
        admit_announcement(&pairing, false, NODE, &json!({})),
        Admission::Admit
    );
}

/// No secret configured means pairing is disabled outright: `pair_node` marks
/// every node Paired, so even `require_pairing = true` admits. That is
/// `NodePairingManager::new(None)`'s documented behaviour and it is worth
/// pinning, because it is also the failure mode where someone sets the key,
/// forgets the secret, and believes they are protected.
#[test]
fn require_pairing_without_a_secret_protects_nothing() {
    let pairing = NodePairingManager::new(None);
    assert_eq!(
        admit_announcement(&pairing, true, NODE, &json!({})),
        Admission::Admit,
        "this is not an endorsement — see the config-validation note below"
    );

    // Which is why config validation refuses to boot in that state. The gate
    // and the boot check protect different halves of the same mistake.
    assert!(
        NodePairingManager::validate_secret("").is_err(),
        "an empty secret must not validate"
    );
    assert!(
        NodePairingManager::validate_secret("too-short").is_err(),
        "a weak secret must not validate"
    );
    assert!(NodePairingManager::validate_secret(SECRET).is_ok());
}

/// Pairing status is evaluated even when the gate is off, so an operator can see
/// which nodes *would* be refused before turning the key on. A security control
/// you cannot dry-run is one people turn on in production and immediately off.
#[test]
fn status_is_observable_before_the_gate_is_switched_on() {
    let pairing = NodePairingManager::new(Some(SECRET.to_string()));
    let _ = admit_announcement(&pairing, false, NODE, &json!({}));
    let status = pairing.status(NODE);
    assert_ne!(
        status.to_string(),
        "Paired",
        "an unpaired node should be recorded as such even when it is admitted"
    );
}

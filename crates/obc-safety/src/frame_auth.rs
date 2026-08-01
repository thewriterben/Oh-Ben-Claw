//! Verifying an inbound message: the tag, the counter, and the answer.
//!
//! [`spine_tag`](crate::spine_tag) computes a tag and [`replay`](crate::replay)
//! judges a counter. Neither is a decision. This is the decision, and it exists
//! as one type because a caller that has to remember to do both, in order, on
//! every path, will eventually do one.
//!
//! ## Which inbound messages
//!
//! Not announcements — those already carry a `PairingToken`, which is an HMAC
//! over `node_id:timestamp` with a five-minute window, so they are authenticated
//! and replay-bounded already (`security/pairing.rs`, gated since 2026-08-01).
//!
//! This is for the traffic that is *not*: tool-call results, and telemetry. That
//! is the half `SPINE-AUTH.md` §2 calls non-obvious — a result or a reading does
//! not actuate anything directly, it lands in world memory, and **reflexes act
//! on world memory without waking the model**. A forged `battery.soc = 3` fires
//! a safing rule. Both directions need authentication; only one of them looks
//! like it does.
//!
//! ## Why `src` is zero here
//!
//! The wire design signs `src ‖ ctr ‖ payload`, where `src` is the LoRa frame's
//! one-byte origin. MQTT has no such byte — identity there is a string, and it
//! is already bound into the key by
//! [`derive_node_key`](crate::spine_tag::derive_node_key), which mixes the node
//! id into the HKDF info. So `src` is fixed at zero and the key carries the
//! identity.
//!
//! The alternative was a second signed-region layout for string identities,
//! which would need a matching change in the firmware's mirror and a second set
//! of cross-implementation vectors, to express something the key already says.
//! One primitive, two transports.
//!
//! ## What this does not do
//!
//! Persist the replay window. `SPINE-REPLAY.md` §3 works out that a receiver has
//! to persist a *ceiling* rather than its position — the opposite rounding from
//! the sender, and the classic form of that bug. On the host the answer is cheap
//! and not yet written; a restart collapses the window, which fails closed and
//! costs at most the re-acceptance of frames the sender has not yet moved past.

use std::sync::Mutex;

use crate::replay::{ReplayVerdict, ReplayWindow};
use crate::spine_tag;

/// The outcome of checking one inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameVerdict {
    /// Authenticated and fresh, or authentication is not required here.
    Accept,
    /// Refused. The reason is for an operator reading a log, not for the sender.
    Reject { reason: String },
}

/// Authenticates inbound messages against per-node keys and a replay window.
#[derive(Debug)]
pub struct FrameAuth {
    /// `None` disables verification entirely — the deployment has no root secret.
    root_secret: Option<String>,
    /// Whether an unauthenticated or stale message is refused, or merely noted.
    require: bool,
    window: Mutex<ReplayWindow>,
}

impl FrameAuth {
    pub fn new(root_secret: Option<String>, require: bool) -> Self {
        Self {
            root_secret,
            require,
            window: Mutex::new(ReplayWindow::new()),
        }
    }

    /// Whether a root secret is configured at all.
    pub fn is_enabled(&self) -> bool {
        self.root_secret.is_some()
    }

    /// Whether an unverifiable message is refused.
    pub fn is_enforcing(&self) -> bool {
        self.require && self.is_enabled()
    }

    /// Check one inbound message from `node_id`.
    ///
    /// `ctr` and `mac_hex` are what the message carried, both absent on a sender
    /// that has not been upgraded. `payload` is the exact bytes the tag covers.
    ///
    /// When not enforcing, this still evaluates and records — so an operator can
    /// see what *would* be refused before turning the key on, the same way the
    /// pairing gate does. A security control you cannot dry-run is one people
    /// enable in production and immediately disable.
    pub fn verify_inbound(
        &self,
        node_id: &str,
        ctr: Option<u32>,
        mac_hex: Option<&str>,
        payload: &[u8],
    ) -> FrameVerdict {
        let Some(secret) = self.root_secret.as_deref() else {
            return FrameVerdict::Accept;
        };

        let reject = |reason: String| {
            if self.require {
                FrameVerdict::Reject { reason }
            } else {
                FrameVerdict::Accept
            }
        };

        let (Some(ctr), Some(mac_hex)) = (ctr, mac_hex) else {
            return reject("message carried no ctr/mac".to_string());
        };

        let Ok(mac) = hex::decode(mac_hex) else {
            return reject("mac is not hex".to_string());
        };

        let key = spine_tag::derive_node_key(secret.as_bytes(), node_id);
        if !spine_tag::verify(&key, 0, ctr, payload, &mac) {
            return reject(format!("mac does not verify for counter {ctr}"));
        }

        // Only a message that proved who sent it may move the replay window.
        // Judging the counter first would let anyone advance a node's window and
        // lock it out — a denial of service handed over by the ordering alone.
        let verdict = self
            .window
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit(node_id, ctr);

        match verdict {
            ReplayVerdict::Fresh => FrameVerdict::Accept,
            ReplayVerdict::Duplicate => reject(format!("counter {ctr} already seen")),
            ReplayVerdict::TooOld => reject(format!("counter {ctr} is older than the window")),
        }
    }

    /// Tag an outbound message with the next counter for this node.
    ///
    /// Present so both directions use one implementation; the caller owns the
    /// counter, because a sender's counter has to survive restarts and this type
    /// deliberately holds no persistent state (see the module header).
    pub fn tag_outbound(&self, node_id: &str, ctr: u32, payload: &[u8]) -> Option<String> {
        let secret = self.root_secret.as_deref()?;
        let key = spine_tag::derive_node_key(secret.as_bytes(), node_id);
        Some(hex::encode(spine_tag::tag(&key, 0, ctr, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const NODE: &str = "obc-esp32-s3-001";
    const BODY: &[u8] = br#"{"call_id":"a7","ok":true,"output":"1"}"#;

    fn enforcing() -> FrameAuth {
        FrameAuth::new(Some(SECRET.to_string()), true)
    }

    fn tag_for(node: &str, ctr: u32, payload: &[u8]) -> String {
        let key = spine_tag::derive_node_key(SECRET.as_bytes(), node);
        hex::encode(spine_tag::tag(&key, 0, ctr, payload))
    }

    #[test]
    fn a_correctly_tagged_message_is_accepted() {
        let auth = enforcing();
        assert_eq!(
            auth.verify_inbound(NODE, Some(1), Some(&tag_for(NODE, 1, BODY)), BODY),
            FrameVerdict::Accept
        );
    }

    #[test]
    fn an_untagged_message_is_refused_when_enforcing() {
        let auth = enforcing();
        assert!(matches!(
            auth.verify_inbound(NODE, None, None, BODY),
            FrameVerdict::Reject { .. }
        ));
    }

    /// The whole point: a captured message replayed verbatim verifies its MAC —
    /// it is genuine — and must still be refused.
    #[test]
    fn a_verbatim_replay_is_refused() {
        let auth = enforcing();
        let mac = tag_for(NODE, 42, BODY);
        assert_eq!(
            auth.verify_inbound(NODE, Some(42), Some(&mac), BODY),
            FrameVerdict::Accept
        );
        match auth.verify_inbound(NODE, Some(42), Some(&mac), BODY) {
            FrameVerdict::Reject { reason } => assert!(reason.contains("already seen")),
            FrameVerdict::Accept => panic!("a replayed message was accepted"),
        }
    }

    #[test]
    fn an_edited_payload_is_refused() {
        let auth = enforcing();
        let mac = tag_for(NODE, 1, BODY);
        let edited = br#"{"call_id":"a7","ok":true,"output":"0"}"#;
        assert!(matches!(
            auth.verify_inbound(NODE, Some(1), Some(&mac), edited),
            FrameVerdict::Reject { .. }
        ));
    }

    /// A tag made with another node's key must not verify here, which is what
    /// per-node derivation buys.
    #[test]
    fn another_nodes_tag_is_refused() {
        let auth = enforcing();
        let theirs = tag_for("obc-esp32-s3-002", 1, BODY);
        assert!(matches!(
            auth.verify_inbound(NODE, Some(1), Some(&theirs), BODY),
            FrameVerdict::Reject { .. }
        ));
    }

    #[test]
    fn a_malformed_mac_is_refused_rather_than_panicking() {
        let auth = enforcing();
        assert!(matches!(
            auth.verify_inbound(NODE, Some(1), Some("not-hex!!"), BODY),
            FrameVerdict::Reject { .. }
        ));
        assert!(matches!(
            auth.verify_inbound(NODE, Some(1), Some("ab"), BODY),
            FrameVerdict::Reject { .. }
        ));
    }

    /// The ordering that matters: a forged message must not be able to advance a
    /// node's replay window, or an attacker who cannot forge a tag can still
    /// lock the node out by claiming a huge counter.
    #[test]
    fn a_message_that_fails_its_mac_does_not_move_the_window() {
        let auth = enforcing();
        let forged = tag_for("someone-else", u32::MAX - 1, BODY);
        assert!(matches!(
            auth.verify_inbound(NODE, Some(u32::MAX - 1), Some(&forged), BODY),
            FrameVerdict::Reject { .. }
        ));
        // The node's own next message still lands.
        assert_eq!(
            auth.verify_inbound(NODE, Some(1), Some(&tag_for(NODE, 1, BODY)), BODY),
            FrameVerdict::Accept
        );
    }

    /// Off by default, and off means off: nothing is refused, but the check is
    /// still evaluated so the state is there to look at.
    #[test]
    fn nothing_is_refused_when_not_enforcing() {
        let auth = FrameAuth::new(Some(SECRET.to_string()), false);
        assert_eq!(
            auth.verify_inbound(NODE, None, None, BODY),
            FrameVerdict::Accept
        );
        assert!(!auth.is_enforcing());
        assert!(auth.is_enabled());
    }

    /// No secret at all is the shipped default and must be a clean no-op — not
    /// an accidental deny-everything on upgrade.
    #[test]
    fn no_secret_accepts_everything() {
        let auth = FrameAuth::new(None, true);
        assert_eq!(
            auth.verify_inbound(NODE, None, None, BODY),
            FrameVerdict::Accept
        );
        assert!(!auth.is_enabled());
        assert!(!auth.is_enforcing());
        assert!(auth.tag_outbound(NODE, 1, BODY).is_none());
    }

    /// Both directions have to agree, or the first authenticated deployment
    /// discovers it at the worst moment.
    #[test]
    fn what_this_tags_it_also_verifies() {
        let auth = enforcing();
        let mac = auth.tag_outbound(NODE, 7, BODY).expect("a secret is set");
        assert_eq!(
            auth.verify_inbound(NODE, Some(7), Some(&mac), BODY),
            FrameVerdict::Accept
        );
    }
}

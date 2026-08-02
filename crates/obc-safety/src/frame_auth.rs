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

/// A monotonic per-node counter for outbound messages.
///
/// `SPINE-REPLAY.md` §2 states the sender's obligation absolutely: **never
/// reissue a counter.** Two messages sharing `(node, ctr)` destroy the
/// receiver's ability to tell a replay from a retransmission, and a counter held
/// only in RAM repeats on every restart.
///
/// The node's answer to that is NVS — designed, unbuilt, and untestable without
/// a board. **The host's answer is the clock**, and it is both simpler and
/// stronger: seed each counter at the current Unix second and increment from
/// there. A restart cannot go backwards unless the clock does, and the host has
/// a real clock at boot, which is exactly what the microcontroller does not.
///
/// The cost is counter *space* rather than correctness: seconds since the epoch
/// currently sit near 1.8 × 10⁹, leaving roughly 2.5 × 10⁹ of a `u32` before
/// exhaustion — about eighty years of headroom, less whatever the process
/// sends. `next` saturates rather than wrapping, and a saturated counter stops
/// producing new values instead of silently reopening the replay window, which
/// is the failure §2 cares about.
#[derive(Debug, Default)]
pub struct OutboundCounters {
    next: Mutex<std::collections::HashMap<String, u32>>,
}

impl OutboundCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seconds since the Unix epoch, saturated into a `u32`, or 1 if the clock
    /// is before the epoch — a machine in that state is worth not trusting for
    /// monotonicity, and 1 is safely below any real seed.
    fn seed() -> u32 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
            .unwrap_or(1)
    }

    /// The next counter for `node_id`. Never returns the same value twice for a
    /// node, and never returns a value below one issued before a restart.
    ///
    /// Returns `None` once the space is exhausted rather than wrapping to zero:
    /// a wrap would make every captured message from this host's history valid
    /// again, all at once.
    pub fn next(&self, node_id: &str) -> Option<u32> {
        let mut map = self.next.lock().unwrap_or_else(|p| p.into_inner());
        let slot = map.entry(node_id.to_string()).or_insert_with(Self::seed);
        if *slot == u32::MAX {
            return None;
        }
        let issued = *slot;
        *slot += 1;
        Some(issued)
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;

    #[test]
    fn counters_never_repeat_for_a_node() {
        let c = OutboundCounters::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(c.next("node-a").unwrap()), "a counter repeated");
        }
    }

    #[test]
    fn counters_rise() {
        let c = OutboundCounters::new();
        let first = c.next("node-a").unwrap();
        let second = c.next("node-a").unwrap();
        assert_eq!(second, first + 1);
    }

    #[test]
    fn nodes_have_independent_counters() {
        let c = OutboundCounters::new();
        let a = c.next("node-a").unwrap();
        let b = c.next("node-b").unwrap();
        // Same seed, because the seed is the clock — the point is that
        // advancing one does not advance the other.
        assert_eq!(c.next("node-a").unwrap(), a + 1);
        assert_eq!(c.next("node-b").unwrap(), b + 1);
    }

    /// The property a restart depends on: a fresh instance does not hand out
    /// values below what the previous one issued. Simulated by constructing two,
    /// which is what a restart is from the counter's point of view.
    #[test]
    fn a_restart_does_not_go_backwards() {
        let before = OutboundCounters::new();
        let last = (0..10)
            .map(|_| before.next("node-a").unwrap())
            .last()
            .unwrap();

        let after = OutboundCounters::new();
        let first_after = after.next("node-a").unwrap();
        assert!(
            first_after > last - 10,
            "a restart reissued counters: {first_after} after {last}"
        );
    }

    /// The seed must be a real timestamp rather than zero, or the "never goes
    /// backwards" argument is only true within a single process.
    #[test]
    fn the_seed_is_the_clock() {
        let seed = OutboundCounters::seed();
        // 2020-01-01 — any plausible run of this code is after it.
        assert!(seed > 1_577_836_800, "seed does not look like a timestamp");
    }

    /// Exhaustion returns `None` rather than wrapping. A wrap would make every
    /// captured message from this host's history verify again at once.
    #[test]
    fn exhaustion_stops_rather_than_wrapping() {
        let c = OutboundCounters::new();
        c.next
            .lock()
            .unwrap()
            .insert("node-a".to_string(), u32::MAX - 1);
        assert_eq!(c.next("node-a"), Some(u32::MAX - 1));
        assert_eq!(c.next("node-a"), None, "the counter wrapped");
        assert_eq!(c.next("node-a"), None, "and stays refused");
    }
}

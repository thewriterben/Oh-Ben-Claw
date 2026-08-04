//! Multi-party consent — gating a frame that contains more than one person.
//!
//! The class gate ([`crate::consent::PerceptionGate`]) decides by *class*:
//! "humans, allowed." It cannot express what a room full of people actually
//! requires — "Alice opted in, Bob didn't" — which the crate's honest-limits note
//! calls out (§4/§6): *semantic consent is not expressible by class.* This is the
//! layer that expresses it.
//!
//! ## The design problem, and the resolution
//!
//! The naive way to check "did this person consent?" is to **recognize** the
//! person and look them up — which means running face recognition on everyone in
//! frame. That is a surveillance capability strictly worse than the one the
//! conscience exists to constrain: to check consent you'd first identify everyone,
//! consented or not. It cannot be the answer on a privacy body.
//!
//! So consent here is **affirmative, opt-in, and keyed to a token the subject
//! presents** — a beacon, a badge, a scanned code — *not* an identity the system
//! derives. The system never learns *who* someone is; it only checks whether a
//! valid consent **token** accompanies them. Three consequences fall out, all in
//! the fail-closed direction:
//!
//! - **No token ⇒ not consented.** A person is never captured because they failed
//!   to object; capture requires their *affirmative* grant. Opt-in, never opt-out.
//! - **No recognition.** Absent a presented token there is nothing to look up, and
//!   the system is not built to look anyone up. Privacy is the default state, not
//!   a thing you switch on.
//! - **The unconsented are never singled out.** Under the default policy one
//!   un-tokened person refuses the *whole frame*; the system does not build a
//!   "who didn't consent" list, because that list would itself be surveillance.
//!
//! ## What a grant is
//!
//! A [`ConsentGrant`] is scoped (a `purpose`), **expiring** (consent is not
//! forever), and **revocable**. Validity fails closed on every axis: unknown
//! token, wrong purpose, past expiry, or revoked all read as *not consented*. A
//! grant with no real expiry (`expires_at_ms == 0`) is treated as already expired,
//! so an operator cannot accidentally mint eternal consent.
//!
//! ## Honest limits (labeled, not solved)
//!
//! - **Redaction needs localization.** [`ConsentPolicy::Redact`] returns which
//!   subjects to blank, but a detector that can't localize per subject can't act
//!   on it — the caller must then treat Redact as Refuse (fail closed). Said, not
//!   hidden.
//! - **A token is not a person's true will.** This checks that a valid consent
//!   token is present, not that consent was freely given or that the right person
//!   holds the token. Binding a token to a willing human is an out-of-band,
//!   physical-world problem this layer cannot close.

use crate::classifier::SubjectClassifier;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What to do with a frame containing a person who has not consented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsentPolicy {
    /// Any un-consented restricted subject refuses the **whole frame**. The most
    /// conservative policy and the default: it never singles anyone out and never
    /// stores a frame anyone in it didn't agree to.
    RequireAll,
    /// Keep the frame but redact the un-consented subjects. Requires the body to
    /// localize and blank per subject; a body that can't must treat this as
    /// [`FrameConsent::Refuse`] (fail closed).
    Redact,
}

impl Default for ConsentPolicy {
    fn default() -> Self {
        ConsentPolicy::RequireAll
    }
}

/// An affirmative, scoped, expiring, revocable consent grant, keyed to a token
/// the subject **presents** — never an identity the system derives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentGrant {
    /// Opaque consent token (a beacon/badge/code the subject carries). NOT a
    /// recognized identity — the system checks for this, it does not deduce it.
    pub subject: String,
    /// What the subject consented to; a decision must match this purpose.
    pub purpose: String,
    /// Hard expiry, ms since epoch. `0` is treated as already-expired so consent
    /// is never accidentally eternal — an operator must set a real bound.
    pub expires_at_ms: u64,
    /// Set when consent is withdrawn; a revoked grant never consents.
    #[serde(default)]
    pub revoked: bool,
}

/// A registry of consent grants, keyed by token. A token absent here is simply
/// *not consented* — there is nothing to recognize and no default-yes.
#[derive(Debug, Clone, Default)]
pub struct ConsentLedger {
    grants: HashMap<String, ConsentGrant>,
}

impl ConsentLedger {
    /// Build from a list of grants (later grants win on a duplicate token).
    pub fn from_grants(grants: impl IntoIterator<Item = ConsentGrant>) -> Self {
        let mut m = HashMap::new();
        for g in grants {
            m.insert(g.subject.clone(), g);
        }
        Self { grants: m }
    }

    /// Is `token` a currently-valid consent for `purpose` at `now_ms`? Fails
    /// closed on every axis: unknown token, wrong purpose, expired, or revoked.
    pub fn is_consented(&self, token: &str, purpose: &str, now_ms: u64) -> bool {
        match self.grants.get(token) {
            Some(g) => !g.revoked && g.purpose == purpose && now_ms < g.expires_at_ms,
            None => false,
        }
    }
}

/// One subject detected in a frame: its consent class, and the consent token it
/// presented (if any). A subject that presented no token has `consent_token:
/// None`, which is *not consented* for any restricted class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectPresence {
    /// Consent class (`"human"`, `"wildlife"`, …) — as produced by the classifier.
    pub class: String,
    /// The consent token this subject presented, if any.
    #[serde(default)]
    pub consent_token: Option<String>,
}

impl SubjectPresence {
    /// Build a presence from a raw detector label by classifying it — so the
    /// caller can feed detector output directly and get the consent class.
    pub fn from_label(label: &str, consent_token: Option<String>, c: &SubjectClassifier) -> Self {
        Self {
            class: c.classify(label).class,
            consent_token,
        }
    }
}

/// The consent decision for a whole frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameConsent {
    /// No restricted subject present, or every one of them consented — capture it.
    Allow,
    /// `RequireAll`: `unconsented` restricted subjects lacked valid consent, so the
    /// whole frame is refused. The count is reported; *which* subjects is not, on
    /// purpose — a "who didn't consent" list is itself surveillance.
    Refuse { unconsented: usize },
    /// `Redact`: blank these subject indices before the frame is stored. A body
    /// that cannot redact per subject must treat this as a refusal.
    Redact { indices: Vec<usize> },
}

/// Decide a frame under multi-party consent.
///
/// Only subjects whose class equals `restricted_class` (the class that requires
/// consent — `"human"` by default) are checked; wildlife and other non-restricted
/// classes never need a token. A restricted subject is consented iff it presented
/// a token that is a valid grant for `purpose` at `now_ms`.
pub fn decide_frame(
    subjects: &[SubjectPresence],
    ledger: &ConsentLedger,
    restricted_class: &str,
    purpose: &str,
    now_ms: u64,
    policy: ConsentPolicy,
) -> FrameConsent {
    let unconsented: Vec<usize> = subjects
        .iter()
        .enumerate()
        .filter(|(_, s)| s.class == restricted_class)
        .filter(|(_, s)| {
            !s.consent_token
                .as_deref()
                .is_some_and(|t| ledger.is_consented(t, purpose, now_ms))
        })
        .map(|(i, _)| i)
        .collect();

    if unconsented.is_empty() {
        return FrameConsent::Allow;
    }
    match policy {
        ConsentPolicy::RequireAll => FrameConsent::Refuse {
            unconsented: unconsented.len(),
        },
        ConsentPolicy::Redact => FrameConsent::Redact {
            indices: unconsented,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(token: &str, purpose: &str, expires: u64) -> ConsentGrant {
        ConsentGrant {
            subject: token.to_string(),
            purpose: purpose.to_string(),
            expires_at_ms: expires,
            revoked: false,
        }
    }

    fn human(token: Option<&str>) -> SubjectPresence {
        SubjectPresence {
            class: "human".to_string(),
            consent_token: token.map(String::from),
        }
    }

    fn wildlife() -> SubjectPresence {
        SubjectPresence {
            class: "wildlife".to_string(),
            consent_token: None,
        }
    }

    fn decide(subjects: &[SubjectPresence], ledger: &ConsentLedger, now: u64, p: ConsentPolicy) -> FrameConsent {
        decide_frame(subjects, ledger, "human", "record", now, p)
    }

    #[test]
    fn an_empty_or_wildlife_only_frame_is_allowed() {
        let led = ConsentLedger::default();
        assert_eq!(decide(&[], &led, 100, ConsentPolicy::RequireAll), FrameConsent::Allow);
        assert_eq!(
            decide(&[wildlife(), wildlife()], &led, 100, ConsentPolicy::RequireAll),
            FrameConsent::Allow
        );
    }

    #[test]
    fn a_human_with_a_valid_grant_is_allowed() {
        let led = ConsentLedger::from_grants([grant("tok-alice", "record", 1_000)]);
        assert_eq!(
            decide(&[human(Some("tok-alice"))], &led, 500, ConsentPolicy::RequireAll),
            FrameConsent::Allow
        );
    }

    #[test]
    fn a_human_with_no_token_is_not_consented_opt_in_not_opt_out() {
        let led = ConsentLedger::default();
        assert_eq!(
            decide(&[human(None)], &led, 500, ConsentPolicy::RequireAll),
            FrameConsent::Refuse { unconsented: 1 }
        );
    }

    #[test]
    fn expired_revoked_and_wrong_purpose_all_fail_closed() {
        let now = 1_000;
        // expired (now >= expiry)
        let led = ConsentLedger::from_grants([grant("t", "record", 1_000)]);
        assert!(!led.is_consented("t", "record", now), "expiry is exclusive");
        // revoked
        let mut g = grant("t", "record", 9_999);
        g.revoked = true;
        let led = ConsentLedger::from_grants([g]);
        assert!(!led.is_consented("t", "record", now));
        // wrong purpose
        let led = ConsentLedger::from_grants([grant("t", "livestream", 9_999)]);
        assert!(!led.is_consented("t", "record", now));
        // no-expiry sentinel (0) is treated as already expired
        let led = ConsentLedger::from_grants([grant("t", "record", 0)]);
        assert!(!led.is_consented("t", "record", 0));
    }

    #[test]
    fn require_all_refuses_the_whole_frame_when_anyone_lacks_consent() {
        // Alice consented, Bob didn't → the whole frame is refused, and the count
        // (not the identity) is what's reported.
        let led = ConsentLedger::from_grants([grant("tok-alice", "record", 9_999)]);
        let frame = [human(Some("tok-alice")), human(None), wildlife()];
        assert_eq!(
            decide(&frame, &led, 100, ConsentPolicy::RequireAll),
            FrameConsent::Refuse { unconsented: 1 }
        );
    }

    #[test]
    fn redact_policy_lists_only_the_unconsented_restricted_subjects() {
        let led = ConsentLedger::from_grants([grant("tok-alice", "record", 9_999)]);
        // idx0 consented human, idx1 wildlife (never needs consent), idx2 un-tokened human.
        let frame = [human(Some("tok-alice")), wildlife(), human(None)];
        assert_eq!(
            decide(&frame, &led, 100, ConsentPolicy::Redact),
            FrameConsent::Redact { indices: vec![2] }
        );
    }

    #[test]
    fn all_consented_is_allowed_under_both_policies() {
        let led = ConsentLedger::from_grants([
            grant("a", "record", 9_999),
            grant("b", "record", 9_999),
        ]);
        let frame = [human(Some("a")), human(Some("b")), wildlife()];
        assert_eq!(decide(&frame, &led, 1, ConsentPolicy::RequireAll), FrameConsent::Allow);
        assert_eq!(decide(&frame, &led, 1, ConsentPolicy::Redact), FrameConsent::Allow);
    }

    #[test]
    fn from_label_classifies_the_subject() {
        let c = SubjectClassifier::with_defaults();
        let s = SubjectPresence::from_label("person", Some("tok".into()), &c);
        assert_eq!(s.class, "human");
        let s = SubjectPresence::from_label("deer", None, &c);
        assert_eq!(s.class, "wildlife");
    }

    #[test]
    fn grants_and_subjects_round_trip_through_json() {
        let grants = vec![grant("a", "record", 9_999)];
        let json = serde_json::to_string(&grants).unwrap();
        let back: Vec<ConsentGrant> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, grants);

        let subjects = vec![human(Some("a")), wildlife()];
        let json = serde_json::to_string(&subjects).unwrap();
        let back: Vec<SubjectPresence> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, subjects);
    }
}

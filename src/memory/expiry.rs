//! Retention policy for beliefs that nothing else will ever retract.
//!
//! # The gap this fills
//!
//! Three mechanisms already withdraw beliefs, and none of them reaches an agent's
//! own notes:
//!
//! - **Supersession** needs a newer value for the *same entity*. An incident note at
//!   `incident.mesh-node-lost` is written once and never written again.
//! - **Source liveness** needs the author to stop existing. The agent does not stop
//!   existing, and retiring it for silence would retract every conclusion it has ever
//!   drawn.
//! - **Dependency withdrawal** needs a recorded in-list. An assertion is not derived
//!   from anything in the store; it is a claim.
//!
//! So the bench store carried six agent assertions from a July investigation that
//! concluded ten days earlier — `incident.*` notes, plus a `mesh.escalation_status`
//! that a supervisor once read back as a phantom node. All still open, all still
//! "believed", with nothing in the system able to notice.
//!
//! # Why age, and why it is declared rather than inferred
//!
//! Expiry is the one withdrawal that is not a consequence of the world changing. It is
//! a consequence of a rule someone wrote, which makes it the most dangerous of the four
//! — a wrong prefix retracts beliefs that were perfectly good. Three constraints follow:
//!
//! 1. **Nothing expires by default.** An empty policy list is inert. There is no
//!    built-in "notes last a week"; someone has to say so, in config, per prefix.
//! 2. **Prefixes are never global.** An empty prefix is rejected rather than treated as
//!    "everything", because the failure mode of that typo is the entire store.
//! 3. **A policy is a claim about a namespace, not about the store.** `incident.` says
//!    incident notes go stale. It says nothing about sensor readings, and cannot.
//!
//! # Age from `ingested_at`, not `valid_from`
//!
//! `valid_from` is caller-supplied — the `world_memory` tool takes it as a parameter, so
//! an agent can backdate or postdate a claim. `ingested_at` is stamped by the store when
//! the row was written. Retention has to run on the timestamp the writer cannot choose,
//! or "this expires in a decade" is one tool argument away.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::liveness;
use super::world::{Closure, Fact, Origin, WorldMemory};

/// One retention rule: beliefs under `prefix` stop being current after `max_age_ms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpiryPolicy {
    /// Entity prefix this applies to, e.g. `"incident."`. Must be non-empty.
    pub prefix: String,
    /// How long after it was *recorded* a fact stops being believed (ms).
    pub max_age_ms: u64,
    /// Restrict to these origins. Empty = any origin under the prefix.
    ///
    /// The usual value is `["asserted"]`: an agent's conclusions go stale in a way a
    /// sensor reading does not, because a reading that stops arriving is already handled
    /// by source liveness and a reading that keeps arriving is not stale at all.
    #[serde(default)]
    pub origins: Vec<String>,
}

impl ExpiryPolicy {
    /// Whether this policy is well-formed enough to run.
    ///
    /// Rejected policies are reported, never silently skipped — a retention rule that
    /// quietly does nothing is how you find out a year later that nothing expired.
    pub fn validate(&self) -> Result<(), String> {
        if self.prefix.trim().is_empty() {
            return Err("prefix is empty; a retention policy must name a namespace, \
                        and an empty prefix would match the entire store"
                .into());
        }
        if self.max_age_ms == 0 {
            return Err(format!(
                "prefix '{}' has max_age_ms = 0, which would expire facts the moment \
                 they are written",
                self.prefix
            ));
        }
        for o in &self.origins {
            if !matches!(o.as_str(), "observed" | "derived" | "asserted" | "instructed") {
                return Err(format!(
                    "prefix '{}': unknown origin '{o}' (expected observed / derived / \
                     asserted / instructed)",
                    self.prefix
                ));
            }
        }
        Ok(())
    }

    fn accepts(&self, fact: &Fact) -> bool {
        if !fact.entity.starts_with(&self.prefix) {
            return false;
        }
        if self.origins.is_empty() {
            return true;
        }
        self.origins.iter().any(|o| Origin::parse(o) == fact.origin)
    }
}

/// What an expiry run did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expired {
    /// Facts closed because a policy matched them.
    pub expired: Vec<i64>,
    /// Beliefs that rested on those and lost their justification.
    pub unsupported: Vec<i64>,
    /// Policies that could not be run, with the reason. Surfaced rather than swallowed.
    pub rejected: Vec<String>,
}

impl Expired {
    pub fn is_empty(&self) -> bool {
        self.expired.is_empty() && self.unsupported.is_empty()
    }
}

/// Entities that no policy may ever expire, whatever it says.
///
/// The liveness namespace records which sources are alive, including the record of
/// having swept them. Expiring it would make a retired source look like it had never
/// been retired, and the next boot would sweep it again — with a store that has since
/// been cleaned, producing a second, emptier "retirement" of a source that stopped days
/// ago. Bookkeeping about withdrawal cannot be subject to withdrawal.
const NEVER_EXPIRE: &[&str] = &["source."];

fn protected(entity: &str) -> bool {
    NEVER_EXPIRE.iter().any(|p| entity.starts_with(p))
}

/// Facts a policy set *would* close right now, without closing anything.
///
/// The dry run exists because the first thing anyone should do with a new retention rule
/// is find out what it eats. A policy is the only withdrawal mechanism here whose blast
/// radius is chosen by a human typing a string.
pub fn due(world: &WorldMemory, policies: &[ExpiryPolicy], now_ms: u64) -> Result<Vec<Fact>> {
    let mut out = Vec::new();
    for policy in policies {
        if policy.validate().is_err() {
            continue;
        }
        for fact in world.open_facts_with_prefix(&policy.prefix)? {
            if protected(&fact.entity) || !policy.accepts(&fact) {
                continue;
            }
            if now_ms.saturating_sub(fact.ingested_at) >= policy.max_age_ms
                && !out.iter().any(|f: &Fact| f.id == fact.id)
            {
                out.push(fact);
            }
        }
    }
    Ok(out)
}

/// Apply every valid policy, then withdraw whatever rested on what was expired.
///
/// Idempotent: a second run finds the facts already closed and reports nothing.
pub fn expire(world: &WorldMemory, policies: &[ExpiryPolicy], now_ms: u64) -> Result<Expired> {
    let mut out = Expired::default();

    for policy in policies {
        if let Err(why) = policy.validate() {
            out.rejected.push(why);
            continue;
        }
        let tag = Closure::Expired(policy.prefix.clone());
        for fact in world.open_facts_with_prefix(&policy.prefix)? {
            if protected(&fact.entity) || !policy.accepts(&fact) {
                continue;
            }
            if now_ms.saturating_sub(fact.ingested_at) < policy.max_age_ms {
                continue;
            }
            if world.close_fact(fact.id, now_ms, &tag)? {
                out.expired.push(fact.id);
            }
        }
    }

    if !out.expired.is_empty() {
        // Anything derived from an expired belief is undercut by it, exactly as if its
        // source had gone away. Tagged `Unsupported` rather than `Expired`, because the
        // dependent did not age out — it lost its grounds, and the distinction is what
        // an operator needs to see to know which rule to change.
        let (unsupported, _skipped) = liveness::cascade(
            world,
            &out.expired,
            now_ms,
            &Closure::Unsupported("expiry".into()),
        )?;
        out.unsupported = unsupported;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(prefix: &str, max_age_ms: u64, origins: &[&str]) -> ExpiryPolicy {
        ExpiryPolicy {
            prefix: prefix.into(),
            max_age_ms,
            origins: origins.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The six rows that motivated this: agent notes about an investigation that
    /// concluded, which supersession, liveness and dependency withdrawal all miss.
    fn note_store() -> WorldMemory {
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as(
            "incident.mesh-node-lost",
            json!({"detail": "awaiting operator approval"}),
            1_000,
            1_000,
            "agent",
            Origin::Asserted,
        )
        .unwrap();
        w.observe_as(
            "incident.gw-40",
            json!({"detail": "probe not attempted"}),
            1_000,
            1_000,
            "agent",
            Origin::Asserted,
        )
        .unwrap();
        // A reading in the same era that must survive: not an assertion.
        w.observe_as("trail.cam1.motion", json!(true), 1_000, 1_000, "clawcam", Origin::Observed)
            .unwrap();
        w
    }

    #[test]
    fn nothing_expires_without_a_policy() {
        // The default has to be inert. A retention mechanism that ships with opinions
        // retracts someone's data the first time they upgrade.
        let w = note_store();
        let out = expire(&w, &[], 999_999_999).unwrap();
        assert!(out.is_empty());
        assert!(w.current("incident.gw-40").unwrap().is_some());
    }

    #[test]
    fn a_policy_expires_stale_notes_and_leaves_readings_alone() {
        let w = note_store();
        let p = [policy("incident.", 10_000, &["asserted"])];

        // Not yet old enough.
        assert!(expire(&w, &p, 5_000).unwrap().is_empty());
        assert!(w.current("incident.gw-40").unwrap().is_some());

        let out = expire(&w, &p, 20_000).unwrap();
        assert_eq!(out.expired.len(), 2);
        assert!(w.current("incident.gw-40").unwrap().is_none());
        assert!(w.current("incident.mesh-node-lost").unwrap().is_none());
        // Different namespace, different origin, untouched.
        assert!(w.current("trail.cam1.motion").unwrap().is_some());

        // Withdrawn, and it says why — distinguishable from the camera going away.
        let hist = &w.history("incident.gw-40").unwrap()[0];
        assert_eq!(Closure::of(hist), Closure::Expired("incident.".into()));
        assert!(Closure::of(hist).is_withdrawal());

        // Idempotent.
        assert!(expire(&w, &p, 30_000).unwrap().is_empty());
    }

    #[test]
    fn the_origin_filter_is_what_keeps_a_prefix_from_eating_evidence() {
        // `mesh.` holds both agent notes and radio observations. Ageing out the notes
        // must not age out the mesh itself — this is the 17 July phantom shape, where a
        // note and a node shared a namespace.
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as("mesh.escalation_status", json!({"status": "critical"}), 1_000, 1_000, "agent", Origin::Asserted)
            .unwrap();
        w.observe_as("mesh.n1", json!({"seq": 1}), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();

        let out = expire(&w, &[policy("mesh.", 10_000, &["asserted"])], 20_000).unwrap();
        assert_eq!(out.expired.len(), 1);
        assert!(w.current("mesh.escalation_status").unwrap().is_none());
        assert!(w.current("mesh.n1").unwrap().is_some(), "the radio's evidence survives");
    }

    #[test]
    fn expiring_a_belief_undercuts_what_rested_on_it() {
        let w = WorldMemory::open_in_memory().unwrap();
        let note = w
            .observe_as("incident.n1", json!({"status": "lost"}), 1_000, 1_000, "agent", Origin::Asserted)
            .unwrap();
        let plan = w
            .observe_derived_from("plan.n1_replacement", json!({"step": 1}), 1_100, 1_100, "system2", &[note.id])
            .unwrap();

        let out = expire(&w, &[policy("incident.", 10_000, &["asserted"])], 20_000).unwrap();
        assert_eq!(out.expired, vec![note.id]);
        assert_eq!(out.unsupported, vec![plan.id]);
        // The dependent did not age out; it lost its grounds. Different tag, because an
        // operator needs to know which rule to change.
        assert_eq!(
            Closure::of(&w.history("plan.n1_replacement").unwrap()[0]),
            Closure::Unsupported("expiry".into())
        );
    }

    #[test]
    fn the_liveness_namespace_is_never_expired() {
        // Expiring it would make a retired source look like it had never been retired,
        // and the next boot would "retire" it again against an already-clean store.
        let w = WorldMemory::open_in_memory().unwrap();
        liveness::stopped(&w, "clawcam", liveness::Stopped::Retired, 1_000, "unplugged").unwrap();
        let out = expire(&w, &[policy("source.", 10, &[])], 900_000).unwrap();
        assert!(out.expired.is_empty());
        assert!(w.current(&liveness::entity_for("clawcam")).unwrap().is_some());
    }

    #[test]
    fn a_malformed_policy_is_reported_not_skipped() {
        // A retention rule that quietly does nothing is how you find out a year later
        // that nothing expired.
        let w = note_store();
        let bad = [
            policy("", 10_000, &[]),
            policy("incident.", 0, &[]),
            policy("incident.", 10_000, &["trusted"]),
        ];
        let out = expire(&w, &bad, 999_999).unwrap();
        assert_eq!(out.rejected.len(), 3);
        assert!(out.expired.is_empty(), "nothing ran");
        assert!(out.rejected[0].contains("entire store"));
        assert!(out.rejected[1].contains("max_age_ms = 0"));
        assert!(out.rejected[2].contains("unknown origin"));
    }

    #[test]
    fn due_reports_without_changing_anything() {
        let w = note_store();
        let p = [policy("incident.", 10_000, &["asserted"])];
        let preview = due(&w, &p, 20_000).unwrap();
        assert_eq!(preview.len(), 2);
        assert!(w.current("incident.gw-40").unwrap().is_some(), "dry run wrote nothing");
    }

    #[test]
    fn age_is_measured_from_ingest_not_from_caller_supplied_valid_from() {
        // `valid_from` is a tool parameter, so an agent can postdate a claim. Retention
        // has to run on the timestamp the writer cannot choose.
        let w = WorldMemory::open_in_memory().unwrap();
        w.observe_as(
            "incident.postdated",
            json!({"detail": "valid until the heat death of the universe"}),
            u64::MAX / 2, // absurd valid_from
            1_000,        // but recorded now
            "agent",
            Origin::Asserted,
        )
        .unwrap();
        let out = expire(&w, &[policy("incident.", 10_000, &["asserted"])], 20_000).unwrap();
        assert_eq!(out.expired.len(), 1, "postdating did not buy immortality");
    }
}

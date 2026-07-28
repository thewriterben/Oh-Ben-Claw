//! What the agent currently believes, rendered for its own context.
//!
//! # The gap this closes
//!
//! Before this, `Agent::build_context` produced exactly two things: the system prompt
//! and the last 50 messages. Not one world fact. OBC could hold tens of thousands of
//! bitemporal facts with origin typing, a support graph and four withdrawal mechanisms,
//! and the agent reasoning on top of it knew none of it — it could *ask*, via the
//! `world_memory` tool, but only if something prompted it to, and nothing did.
//!
//! An agent that does not know what it believes cannot notice that a belief is missing.
//! That is not a hypothetical: on 2026-07-17 System 2 woke to a stale mesh count, had no
//! way to see that its author had been switched off, filed an incident note about it, and
//! the note was read back as a mesh node — which went offline, escalated, and re-woke
//! System 2 in a loop. The agent manufactured an emergency out of a number nobody was
//! maintaining.
//!
//! # Why withdrawals are in here
//!
//! The obvious version of this feature lists current facts. The useful version also lists
//! what was recently **stopped** being believed, and why.
//!
//! "`mesh.escalated_count` was withdrawn because `mesh-supervisor` was switched off" is
//! precisely the context that stops an agent re-opening a closed incident. Current state
//! alone cannot express it: the fact is simply *absent*, and absence reads as "never
//! existed" rather than "we deliberately stopped holding this". The same principle the
//! store already follows — absence of data is a datum, and must be written — applies to
//! the agent's view of the store.
//!
//! # Why the omissions are counted
//!
//! The rendering is capped, and when it drops facts it says so, by name of what is left
//! and how to get it. A truncated view that looks complete is worse than no view: it
//! invites exactly the confident wrong answer this whole subsystem exists to prevent.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::memory::world::{Closure, Fact, Origin, WorldMemory};

/// How much world state to put in front of the model, and which.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldContextConfig {
    /// Render the preamble at all.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Maximum currently-believed facts to show.
    #[serde(default = "default_max_facts")]
    pub max_facts: usize,
    /// Maximum recent withdrawals to show.
    #[serde(default = "default_max_withdrawals")]
    pub max_withdrawals: usize,
    /// Only show withdrawals from the last this-many ms. `0` = no age limit.
    #[serde(default = "default_withdrawal_window_ms")]
    pub withdrawal_window_ms: u64,
    /// Hard character cap on the whole preamble. The last line of defence: a run of
    /// unusually large fact values must not silently eat the context window.
    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
    /// Only these entity prefixes. Empty = everything.
    #[serde(default)]
    pub include: Vec<String>,
}

fn yes() -> bool {
    true
}
fn default_max_facts() -> usize {
    24
}
fn default_max_withdrawals() -> usize {
    5
}
fn default_withdrawal_window_ms() -> u64 {
    24 * 60 * 60 * 1000
}
fn default_max_chars() -> usize {
    2400
}

impl Default for WorldContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_facts: default_max_facts(),
            max_withdrawals: default_max_withdrawals(),
            withdrawal_window_ms: default_withdrawal_window_ms(),
            max_chars: default_max_chars(),
            include: Vec::new(),
        }
    }
}

fn age(now_ms: u64, then_ms: u64) -> String {
    let s = now_ms.saturating_sub(then_ms) / 1000;
    if s < 90 {
        format!("{s}s ago")
    } else if s < 5400 {
        format!("{}m ago", s / 60)
    } else if s < 172_800 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

fn compact(value: &serde_json::Value, budget: usize) -> String {
    let s = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() > budget {
        format!("{}…", s.chars().take(budget.saturating_sub(1)).collect::<String>())
    } else {
        s
    }
}

fn included(entity: &str, include: &[String]) -> bool {
    include.is_empty() || include.iter().any(|p| entity.starts_with(p.as_str()))
}

/// Render the preamble, or `None` when there is nothing worth saying.
///
/// Facts are ordered newest-first so a cap keeps what changed most recently — the part
/// a conversation is most likely to be about. Origin is always shown: the difference
/// between a radio reading and the agent's own earlier claim is the difference between
/// evidence and a memory of having guessed, and a model that cannot see which is which
/// will treat them alike.
pub fn render(world: &WorldMemory, cfg: &WorldContextConfig, now_ms: u64) -> Option<String> {
    if !cfg.enabled {
        return None;
    }

    // Current beliefs, newest first.
    let mut facts: Vec<Fact> = Vec::new();
    for entity in world.entities().ok()? {
        if !included(&entity, &cfg.include) {
            continue;
        }
        if let Ok(Some(f)) = world.current(&entity) {
            facts.push(f);
        }
    }
    facts.sort_by(|a, b| b.valid_from.cmp(&a.valid_from).then(b.id.cmp(&a.id)));
    let total_facts = facts.len();
    let shown: Vec<&Fact> = facts.iter().take(cfg.max_facts).collect();

    // Recent withdrawals — not supersessions. An entity changing value is ordinary and
    // would bury the signal; a belief we stopped holding is the thing worth saying.
    let since = if cfg.withdrawal_window_ms == 0 {
        0
    } else {
        now_ms.saturating_sub(cfg.withdrawal_window_ms)
    };
    let withdrawn: Vec<Fact> = world
        .withdrawn_since(since)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| included(&f.entity, &cfg.include))
        .collect();
    let total_withdrawn = withdrawn.len();

    if shown.is_empty() && withdrawn.is_empty() {
        return None;
    }

    // Build the withdrawals block FIRST, and reserve its length out of the budget.
    //
    // This ordering is not cosmetic. The first version appended withdrawals last and let
    // the character cap fall where it may; run against the real bench store, the cap hit
    // inside the fact list and the withdrawals — the part that exists to stop the agent
    // re-opening a closed incident — were cut entirely. The section most likely to be
    // dropped was the one with the highest value per character.
    //
    // It is also naturally small: bounded by `max_withdrawals` and one line each, where
    // the fact list is bounded by whatever the store happens to hold.
    let mut withdrawals = String::new();
    if !withdrawn.is_empty() {
        withdrawals.push_str("\n**No longer believed**\n");
        for f in withdrawn.iter().take(cfg.max_withdrawals) {
            let why = match Closure::of(f) {
                Closure::SourceStopped(s) => format!("its source `{s}` stopped reporting"),
                Closure::Unsupported(s) => {
                    format!("undercut — something it rested on went away with `{s}`")
                }
                Closure::Expired(p) => format!("aged out under the `{p}` retention policy"),
                // Superseded rows are excluded by `withdrawn_since`; an unparseable tag
                // lands here and is reported honestly rather than guessed at.
                _ => "withdrawn (reason unrecorded)".to_string(),
            };
            withdrawals.push_str(&format!(
                "- `{}` was {} — {}, {}\n",
                f.entity,
                compact(&f.value, 48),
                why,
                age(now_ms, f.valid_to.unwrap_or(now_ms)),
            ));
        }
        if total_withdrawn > cfg.max_withdrawals {
            withdrawals.push_str(&format!("_…and {} more._\n", total_withdrawn - cfg.max_withdrawals));
        }
        withdrawals.push_str(
            "\nThese were not contradicted — the grounds for them went away. Do not \
             re-investigate one without new evidence.\n",
        );
    }

    let mut out = String::new();
    out.push_str("## World state\n\n");
    out.push_str(
        "What you currently believe about the physical world, from your own perception \
         layer. `observed` came off a wire; `derived` you computed; `asserted` is a claim \
         you or another agent made and is not evidence. Use the `world_memory` tool for \
         anything not listed.\n\n",
    );

    // What the fact list may spend: everything except the header already written, the
    // withdrawals block, and a margin for the "not shown" line.
    let fact_budget = cfg
        .max_chars
        .saturating_sub(out.chars().count() + withdrawals.chars().count() + 120);

    if !shown.is_empty() {
        // Collapse repeated entities under a common head so a mesh of ten nodes does not
        // read as ten unrelated topics.
        let mut by_head: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
        for f in &shown {
            by_head.entry(f.entity.split('.').next().unwrap_or("")).or_default().push(f);
        }
        let mut rendered = 0usize;
        'outer: for (head, group) in &by_head {
            out.push_str(&format!("**{head}**\n"));
            for f in group {
                let line = format!(
                    "- `{}` = {}  ({}, {}, {})\n",
                    f.entity,
                    compact(&f.value, 64),
                    f.origin.as_str(),
                    f.source,
                    age(now_ms, f.valid_from),
                );
                if out.chars().count() + line.chars().count() > fact_budget {
                    break 'outer;
                }
                out.push_str(&line);
                rendered += 1;
            }
        }
        if total_facts > rendered {
            // Say what is missing. A truncated list that looks complete invites exactly
            // the confident wrong answer this subsystem exists to prevent.
            out.push_str(&format!(
                "\n_{} further belief(s) not shown — query `world_memory` with action \
                 `entities` to see them all._\n",
                total_facts - rendered
            ));
        }
    }

    out.push_str(&withdrawals);
    Some(out)
}

/// Whether a fact's origin means "the world said so".
///
/// Exposed because the preamble is the one place a model sees origin at all, and a caller
/// building a narrower view should not have to re-derive the rule.
pub fn is_evidence(origin: Origin) -> bool {
    crate::memory::world::OriginSet::EVIDENCE.accepts(origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::liveness::{stopped, Stopped};
    use serde_json::json;

    fn store() -> WorldMemory {
        WorldMemory::open_in_memory().unwrap()
    }

    #[test]
    fn an_empty_store_produces_no_preamble() {
        // Nothing to say beats a heading with nothing under it — the model should not be
        // taught to expect a section that is usually empty.
        assert!(render(&store(), &WorldContextConfig::default(), 10_000).is_none());
    }

    #[test]
    fn beliefs_carry_origin_source_and_age() {
        let w = store();
        w.observe_as("mesh.n1", json!({"rssi": -70}), 1_000, 1_000, "lora-gateway", Origin::Observed)
            .unwrap();
        let out = render(&w, &WorldContextConfig::default(), 61_000).unwrap();
        assert!(out.contains("`mesh.n1`"), "{out}");
        assert!(out.contains("observed"), "origin is shown: {out}");
        assert!(out.contains("lora-gateway"), "source is shown: {out}");
        assert!(out.contains("60s ago"), "age is shown: {out}");
    }

    #[test]
    fn an_agent_assertion_is_not_dressed_up_as_evidence() {
        // The whole point of surfacing origin. A model that cannot tell a radio reading
        // from its own earlier guess will treat them alike.
        let w = store();
        w.observe_as("incident.n1", json!("presumed lost"), 1_000, 1_000, "agent", Origin::Asserted)
            .unwrap();
        let out = render(&w, &WorldContextConfig::default(), 2_000).unwrap();
        assert!(out.contains("asserted"), "{out}");
        assert!(out.contains("is not evidence"), "the legend explains it: {out}");
    }

    #[test]
    fn withdrawals_appear_with_the_reason_they_went() {
        // The July 17 shape: a belief whose author was switched off. Without this the
        // fact is merely absent, and absence reads as "never existed".
        let w = store();
        let esc = w
            .observe("mesh.n.escalation", json!({"status": "escalated"}), 1_000, 1_000, "mesh-supervisor")
            .unwrap();
        w.observe_derived_from("mesh.escalated_count", json!(1), 1_100, 1_100, "mesh-supervisor", &[esc.id])
            .unwrap();
        stopped(&w, "mesh-supervisor", Stopped::Retired, 2_000, "configured off").unwrap();

        let out = render(&w, &WorldContextConfig::default(), 3_000).unwrap();
        assert!(out.contains("No longer believed"), "{out}");
        assert!(out.contains("mesh.escalated_count"), "{out}");
        assert!(out.contains("stopped reporting"), "{out}");
        assert!(
            out.contains("not contradicted"),
            "the distinction that stops a re-investigation: {out}"
        );
    }

    #[test]
    fn a_capped_list_says_what_it_dropped() {
        // A truncated view that looks complete is worse than no view.
        let w = store();
        for i in 0..40 {
            w.observe(&format!("s.reading{i}"), json!(i), 1_000 + i, 1_000 + i, "sensor")
                .unwrap();
        }
        let cfg = WorldContextConfig { max_facts: 5, ..Default::default() };
        let out = render(&w, &cfg, 2_000).unwrap();
        assert!(out.contains("35 further belief(s) not shown"), "{out}");
        assert!(out.contains("world_memory"), "and how to get them: {out}");
    }

    #[test]
    fn the_newest_beliefs_survive_the_cap() {
        // A cap should keep what changed most recently — the part a conversation is most
        // likely to be about.
        let w = store();
        w.observe("old.thing", json!(1), 1_000, 1_000, "s").unwrap();
        w.observe("new.thing", json!(2), 9_000, 9_000, "s").unwrap();
        let cfg = WorldContextConfig { max_facts: 1, ..Default::default() };
        let out = render(&w, &cfg, 10_000).unwrap();
        assert!(out.contains("new.thing"), "{out}");
        assert!(!out.contains("`old.thing`"), "{out}");
    }

    #[test]
    fn the_include_list_scopes_both_halves() {
        let w = store();
        w.observe("mesh.n1", json!(1), 1_000, 1_000, "lora").unwrap();
        w.observe("kitchen.temp", json!(21), 1_000, 1_000, "sensor").unwrap();
        let cfg = WorldContextConfig { include: vec!["mesh.".into()], ..Default::default() };
        let out = render(&w, &cfg, 2_000).unwrap();
        assert!(out.contains("mesh.n1"), "{out}");
        assert!(!out.contains("kitchen.temp"), "{out}");
    }

    #[test]
    fn a_stale_withdrawal_falls_out_of_the_window() {
        let w = store();
        w.observe("a.b", json!(1), 1_000, 1_000, "src").unwrap();
        stopped(&w, "src", Stopped::Retired, 2_000, "off").unwrap();
        // Long after the fact: the withdrawal is old news and should not crowd the view.
        let cfg = WorldContextConfig { withdrawal_window_ms: 1_000, ..Default::default() };
        let out = render(&w, &cfg, 900_000);
        assert!(out.is_none() || !out.unwrap().contains("No longer believed"));
    }

    #[test]
    fn the_character_cap_is_enforced_and_announced() {
        let w = store();
        for i in 0..30 {
            w.observe(&format!("s.r{i}"), json!("x".repeat(200)), 1_000 + i, 1_000 + i, "sensor")
                .unwrap();
        }
        let cfg = WorldContextConfig { max_chars: 900, ..Default::default() };
        let out = render(&w, &cfg, 2_000).unwrap();
        assert!(out.chars().count() <= 900, "hard cap holds: {}", out.chars().count());
        assert!(out.contains("not shown"), "and it says what it dropped: {out}");
    }

    #[test]
    fn withdrawals_survive_a_budget_the_fact_list_would_have_eaten() {
        // Found by running against the real store, not by reasoning. The first version
        // appended withdrawals last and let the cap fall where it may: with 45 open
        // beliefs of verbose JSON, the cap hit inside the fact list and the withdrawals
        // were cut entirely — the highest-value section was the one most likely to be
        // dropped.
        let w = store();
        let doomed = w
            .observe("mesh.escalated_count", json!(2), 1_000, 1_000, "mesh-supervisor")
            .unwrap();
        assert!(doomed.id > 0);
        stopped(&w, "mesh-supervisor", Stopped::Retired, 1_500, "configured off").unwrap();
        // Now bury it under a large, verbose current-state list.
        for i in 0..40 {
            w.observe(&format!("noise.r{i}"), json!("y".repeat(300)), 2_000 + i, 2_000 + i, "sensor")
                .unwrap();
        }

        let cfg = WorldContextConfig { max_chars: 1_200, ..Default::default() };
        let out = render(&w, &cfg, 3_000).unwrap();
        assert!(out.chars().count() <= 1_200);
        assert!(out.contains("No longer believed"), "the section survives: {out}");
        assert!(out.contains("mesh.escalated_count"), "{out}");
        assert!(out.contains("not shown"), "and the facts are what got cut: {out}");
    }

    #[test]
    fn disabled_renders_nothing() {
        let w = store();
        w.observe("a.b", json!(1), 1_000, 1_000, "s").unwrap();
        let cfg = WorldContextConfig { enabled: false, ..Default::default() };
        assert!(render(&w, &cfg, 2_000).is_none());
    }
}

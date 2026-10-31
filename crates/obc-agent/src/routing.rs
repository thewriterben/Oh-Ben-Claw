//! Per-turn routing between two brains (parity plan item 3, 2026-09-11).
//!
//! The `[provider]` block is the local, routine brain; `[provider.routing.cloud]`
//! is the one for turns that deserve it. The decision is a pure function of a
//! few facts about the turn ([`decide`]) so the policy is tested without a
//! model, and the agent writes the outcome to world memory as `agent.brain`
//! so the world-state block says which brain answered.
//!
//! The rules, in order — the first that applies wins:
//!
//! 1. the cloud provider failed recently (backing off) → local;
//! 2. the day's cloud budget is spent → local;
//! 3. the context carries private facts (camera detections, people) → local.
//!    Privacy is decided from what the world-state block would show this
//!    turn, by the fact's `source` and entity prefix, not by parsing text;
//! 4. a background session (System 2 wakes, the harness, an edge node) → local;
//! 5. the local brain failed recently (backing off) and
//!    `escalate_on_local_failure` → cloud. Placed after the privacy and
//!    background rules on purpose: a private turn still goes local while the
//!    local brain is down, and fails there, because the alternative is
//!    sending it up;
//! 6. an operator turn (console, channels, gateway) → cloud when
//!    `console_to_cloud`;
//! 7. otherwise cloud when at least `tool_threshold` tools are registered;
//! 8. otherwise local.
//!
//! Two rules apply after the decision, inside the turn. The result of a tool
//! named in `private_tools` (`hear`, `sense`, `audio_transcribe`,
//! `vision_analyze`) moves the rest of the turn local, whichever brain began
//! it ([`private_tool`]). Rule 3 sees the world-state block the turn starts
//! with; this is the one thing it cannot see, because the result does not
//! exist yet when the route is chosen. And when the local brain fails on a
//! turn it was given for a reason that is not privacy ([`may_escalate`]:
//! `routine turn`, `background session`), the call is retried once on the
//! cloud brain, unless the cloud is cooling or the day's budget is spent
//! (plan P1, 2026-10-30). The retry is recorded as `local brain failed,
//! escalated` with `escalated: true` on the `agent.brain` fact and an audit
//! row, and starts the local back-off that rule 5 reads. A turn kept local
//! for privacy never escalates; neither does one with `routing disabled`.

use obc_memory::world::Fact;
use obc_providers::RoutingConfig;

/// Which brain answers this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Cloud,
    Local,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Cloud => "cloud",
            Route::Local => "local",
        }
    }
}

/// What the router knows about a turn before the model is called.
#[derive(Debug, Clone)]
pub struct TurnFacts<'a> {
    pub session_id: &'a str,
    pub tool_count: usize,
    /// The world-state block this turn would carry a private fact.
    pub private_facts: bool,
    /// The cloud provider failed within `offline_backoff_secs`.
    pub cloud_cooling: bool,
    /// Today's estimated cloud spend has reached `daily_budget_usd`.
    pub budget_exceeded: bool,
    /// The local provider failed within `offline_backoff_secs`.
    pub local_cooling: bool,
}

/// The route for a turn and the reason, in the words the log and the
/// `agent.brain` fact use.
pub fn decide(cfg: &RoutingConfig, facts: &TurnFacts<'_>) -> (Route, &'static str) {
    if !cfg.enabled {
        return (Route::Local, "routing disabled");
    }
    if facts.cloud_cooling {
        return (Route::Local, "cloud unreachable, backing off");
    }
    if facts.budget_exceeded {
        return (Route::Local, "daily cloud budget spent");
    }
    if facts.private_facts {
        return (Route::Local, "private facts in context");
    }
    if cfg
        .local_session_prefixes
        .iter()
        .any(|p| facts.session_id.starts_with(p.as_str()))
    {
        return (Route::Local, "background session");
    }
    if facts.local_cooling && cfg.escalate_on_local_failure {
        return (Route::Cloud, "local brain down, backing off");
    }
    if cfg.console_to_cloud {
        return (Route::Cloud, "operator turn");
    }
    if cfg.tool_threshold > 0 && facts.tool_count >= cfg.tool_threshold {
        return (Route::Cloud, "tool-heavy turn");
    }
    (Route::Local, "routine turn")
}

/// Whether a turn routed local for this reason may be retried on the cloud
/// brain when the local one fails. Only the reasons that say nothing about
/// privacy: `routine turn` and `background session`. A privacy reason never
/// escalates, `routing disabled` never escalates, and a turn sent local
/// because the cloud is cooling or over budget has nowhere to go.
pub fn may_escalate(reason: &str) -> bool {
    matches!(reason, "routine turn" | "background session")
}

/// Whether any of these facts is private under the config: its `source` is one
/// of `private_sources` (exactly, or as a `source:qualifier` prefix — ClawCam
/// health facts are `clawcam:<node>`), or its entity starts with one of
/// `private_entity_prefixes`.
pub fn private_facts<'a>(facts: impl IntoIterator<Item = &'a Fact>, cfg: &RoutingConfig) -> bool {
    facts.into_iter().any(|f| is_private(f, cfg))
}

/// Whether a tool of this name is one whose result keeps the rest of the turn
/// on this host (`private_tools`). Exact match: a peer's `p2p:<node>:hear` is
/// that node's room, not this one's, and its result already crossed the LAN.
pub fn private_tool(cfg: &RoutingConfig, tool_name: &str) -> bool {
    cfg.private_tools.iter().any(|t| t == tool_name)
}

fn is_private(f: &Fact, cfg: &RoutingConfig) -> bool {
    cfg.private_sources.iter().any(|s| {
        f.source == *s
            || f.source
                .strip_prefix(s.as_str())
                .is_some_and(|rest| rest.starts_with(':'))
    }) || cfg
        .private_entity_prefixes
        .iter()
        .any(|p| f.entity.starts_with(p.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_memory::world::Origin;

    fn cfg() -> RoutingConfig {
        RoutingConfig {
            cloud: obc_providers::ProviderConfig {
                name: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                ..Default::default()
            },
            ..RoutingConfig::default_with_cloud(obc_providers::ProviderConfig::default())
        }
    }

    fn facts<'a>(session: &'a str) -> TurnFacts<'a> {
        TurnFacts {
            session_id: session,
            tool_count: 31,
            private_facts: false,
            cloud_cooling: false,
            budget_exceeded: false,
            local_cooling: false,
        }
    }

    #[test]
    fn an_operator_turn_goes_to_the_cloud() {
        assert_eq!(
            decide(&cfg(), &facts("3f2a-uuid")),
            (Route::Cloud, "operator turn")
        );
    }

    #[test]
    fn the_guards_win_in_order() {
        let c = cfg();
        let f = TurnFacts {
            cloud_cooling: true,
            budget_exceeded: true,
            private_facts: true,
            ..facts("system2")
        };
        assert_eq!(decide(&c, &f).1, "cloud unreachable, backing off");
        let f = TurnFacts {
            cloud_cooling: false,
            ..f
        };
        assert_eq!(decide(&c, &f).1, "daily cloud budget spent");
        let f = TurnFacts {
            budget_exceeded: false,
            ..f
        };
        assert_eq!(decide(&c, &f).1, "private facts in context");
        let f = TurnFacts {
            private_facts: false,
            ..f
        };
        assert_eq!(decide(&c, &f), (Route::Local, "background session"));
        assert_eq!(decide(&c, &facts("harness-mission")).0, Route::Local);
        assert_eq!(decide(&c, &facts("edge-node3")).0, Route::Local);
    }

    #[test]
    fn a_cooling_local_brain_sends_turns_up_after_the_privacy_and_background_rules() {
        let c = cfg();
        // Every rule above it still wins.
        let f = TurnFacts {
            local_cooling: true,
            cloud_cooling: true,
            ..facts("uuid")
        };
        assert_eq!(decide(&c, &f).1, "cloud unreachable, backing off");
        let f = TurnFacts {
            local_cooling: true,
            budget_exceeded: true,
            ..facts("uuid")
        };
        assert_eq!(decide(&c, &f).1, "daily cloud budget spent");
        let f = TurnFacts {
            local_cooling: true,
            private_facts: true,
            ..facts("uuid")
        };
        assert_eq!(
            decide(&c, &f),
            (Route::Local, "private facts in context"),
            "a private turn stays local and fails there"
        );
        let f = TurnFacts {
            local_cooling: true,
            ..facts("system2")
        };
        assert_eq!(decide(&c, &f), (Route::Local, "background session"));
        // Then it beats the operator and tool rules: a routine turn that would
        // have stayed local goes up without paying the timeout.
        let c = RoutingConfig {
            console_to_cloud: false,
            tool_threshold: 0,
            ..cfg()
        };
        let f = TurnFacts {
            local_cooling: true,
            ..facts("uuid")
        };
        assert_eq!(
            decide(&c, &f),
            (Route::Cloud, "local brain down, backing off")
        );
        // Unless escalation is off, when the rule does not exist.
        let c = RoutingConfig {
            escalate_on_local_failure: false,
            ..c
        };
        assert_eq!(decide(&c, &f), (Route::Local, "routine turn"));
    }

    #[test]
    fn only_routine_and_background_turns_may_escalate() {
        assert!(may_escalate("routine turn"));
        assert!(may_escalate("background session"));
        for r in [
            "private facts in context",
            "private tool result in context",
            "routing disabled",
            "cloud unreachable, backing off",
            "daily cloud budget spent",
            "no router",
        ] {
            assert!(!may_escalate(r), "{r}");
        }
    }

    #[test]
    fn without_console_to_cloud_only_tool_heavy_turns_go_up() {
        let c = RoutingConfig {
            console_to_cloud: false,
            tool_threshold: 8,
            ..cfg()
        };
        assert_eq!(
            decide(&c, &facts("uuid")),
            (Route::Cloud, "tool-heavy turn")
        );
        let f = TurnFacts {
            tool_count: 3,
            ..facts("uuid")
        };
        assert_eq!(decide(&c, &f), (Route::Local, "routine turn"));
        let c = RoutingConfig {
            tool_threshold: 0,
            ..c
        };
        assert_eq!(decide(&c, &facts("uuid")).1, "routine turn");
    }

    #[test]
    fn disabled_routing_is_always_local() {
        let c = RoutingConfig {
            enabled: false,
            ..cfg()
        };
        assert_eq!(
            decide(&c, &facts("uuid")),
            (Route::Local, "routing disabled")
        );
    }

    #[test]
    fn privacy_is_decided_by_source_and_entity_prefix() {
        let world = obc_memory::world::WorldMemory::open_in_memory().unwrap();
        let mesh = world
            .observe_as(
                "mesh.node1.rssi",
                serde_json::json!(-70),
                1,
                1,
                "supervisor",
                Origin::Observed,
            )
            .unwrap();
        assert!(!private_facts([&mesh], &cfg()));
        let cam = world
            .observe_as(
                "vision.subject.person-3",
                serde_json::json!({"label":"person"}),
                2,
                2,
                "clawcam",
                Origin::Derived,
            )
            .unwrap();
        assert!(private_facts([&mesh, &cam], &cfg()));
        let health = world
            .observe_as(
                "clawcam.node9.health",
                serde_json::json!("ok"),
                3,
                3,
                "clawcam:node9",
                Origin::Observed,
            )
            .unwrap();
        assert!(
            private_facts([&health], &cfg()),
            "a qualified source counts"
        );
        let lookalike = world
            .observe_as(
                "printer.state",
                serde_json::json!("idle"),
                4,
                4,
                "clawcamera",
                Origin::Observed,
            )
            .unwrap();
        assert!(
            !private_facts([&lookalike], &cfg()),
            "a source that merely starts with the name does not"
        );
    }
}

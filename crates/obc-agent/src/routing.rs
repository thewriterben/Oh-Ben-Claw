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
//! 5. an operator turn (console, channels, gateway) → cloud when
//!    `console_to_cloud`;
//! 6. otherwise cloud when at least `tool_threshold` tools are registered;
//! 7. otherwise local.

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
    if cfg.console_to_cloud {
        return (Route::Cloud, "operator turn");
    }
    if cfg.tool_threshold > 0 && facts.tool_count >= cfg.tool_threshold {
        return (Route::Cloud, "tool-heavy turn");
    }
    (Route::Local, "routine turn")
}

/// Whether any of these facts is private under the config: its `source` is one
/// of `private_sources` (exactly, or as a `source:qualifier` prefix — ClawCam
/// health facts are `clawcam:<node>`), or its entity starts with one of
/// `private_entity_prefixes`.
pub fn private_facts<'a>(facts: impl IntoIterator<Item = &'a Fact>, cfg: &RoutingConfig) -> bool {
    facts.into_iter().any(|f| is_private(f, cfg))
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

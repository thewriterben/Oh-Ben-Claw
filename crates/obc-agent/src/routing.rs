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
//!
//! Two more ways up, both inside the turn and both once per turn (plan P2,
//! 2026-10-30). Capability: a local turn that runs a tool named in
//! `cloud_tools` hands the rest of the turn to the cloud brain as `cloud tool
//! called` ([`cloud_tool_escalates`]), the mirror image of the private-tool
//! rule, and privacy wins when both apply: a turn local for a privacy reason,
//! or in which a private tool has already run, stays. Confidence: with
//! `escalate_on_low_confidence` the local brain is told
//! ([`LOW_CONFIDENCE_CONTRACT`]) that a final answer of exactly
//! `{"escalate": "<why>"}` asks for the cloud; the agent drops that answer and
//! re-runs the turn there as `local brain asked to escalate: <why>`
//! ([`escalation_request`]). Both respect the cloud back-off and budget, and
//! both are recorded like P1: `escalated: true` and a `routing.escalate` row.

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

/// Whether a tool of this name is one whose call deserves the cloud brain
/// (`cloud_tools`). Exact match, like [`private_tool`].
pub fn cloud_tool(cfg: &RoutingConfig, tool_name: &str) -> bool {
    cfg.cloud_tools.iter().any(|t| t == tool_name)
}

/// Whether running `tool_name` on a turn routed local for `reason` sends the
/// rest of the turn to the cloud brain (plan P2, capability axis): the tool is
/// in `cloud_tools`, the reason is one that may escalate ([`may_escalate`]),
/// and no private tool has run in the turn so far. Privacy wins: a turn kept
/// local for `private facts in context` or `private tool result in context`
/// stays, and so does one in which `hear` ran before `plan_site`. The caller
/// still checks the cloud back-off and budget.
pub fn cloud_tool_escalates(
    cfg: &RoutingConfig,
    reason: &str,
    private_tool_ran: bool,
    tool_name: &str,
) -> bool {
    !private_tool_ran && may_escalate(reason) && cloud_tool(cfg, tool_name)
}

/// What the local brain is told when `escalate_on_low_confidence` is on, and
/// only then: a model that was never told about the contract never triggers
/// it. Appended to the system prompt of a turn routed local.
pub const LOW_CONFIDENCE_CONTRACT: &str = "### Asking for the larger model\n\
If this request needs more than you can do well (long-horizon planning, deep \
reasoning, knowledge you are unsure of), answer with exactly this JSON object \
and nothing else: {\"escalate\": \"<one sentence on why>\"}. The turn is then \
re-run on the larger model and your answer is dropped. Do not use it for \
requests you can answer, and never after a tool result that must stay on this \
device (what the room said, what the camera saw).";

/// The longest `why` kept from an escalation request; the rest is cut.
pub const MAX_ESCALATION_WHY: usize = 200;

/// Whether a local completion's final text is a request to escalate (plan P2,
/// confidence axis): a JSON object with the single key `escalate` and a
/// non-empty string value, allowing surrounding whitespace or one fenced
/// block (with or without a `json` tag) and nothing else. Anything looser,
/// such as prose around it, other keys, an array or a non-string value, is an
/// ordinary answer. Returns the why, trimmed and cut to
/// [`MAX_ESCALATION_WHY`] chars.
pub fn escalation_request(text: &str) -> Option<String> {
    let body = unfence(text.trim());
    if !body.starts_with('{') {
        return None;
    }
    let obj = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(body).ok()?;
    if obj.len() != 1 {
        return None;
    }
    let why = obj.get("escalate")?.as_str()?.trim();
    if why.is_empty() {
        return None;
    }
    let cut = why
        .char_indices()
        .nth(MAX_ESCALATION_WHY)
        .map_or(why.len(), |(i, _)| i);
    Some(why[..cut].trim_end().to_string())
}

/// Strip one fenced code block around `s`, if that is all there is.
fn unfence(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let Some(inner) = rest.strip_suffix("```") else {
        return s;
    };
    // An optional language tag ends at the first line break.
    let inner = match inner.find('\n') {
        Some(nl)
            if inner[..nl]
                .trim()
                .chars()
                .all(|c| c.is_ascii_alphanumeric()) =>
        {
            &inner[nl + 1..]
        }
        _ => inner,
    };
    inner.trim()
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
    fn a_cloud_tool_escalates_a_routine_turn_unless_privacy_applies() {
        let c = RoutingConfig {
            cloud_tools: vec!["plan_site".into(), "foresight".into()],
            ..cfg()
        };
        assert!(cloud_tool(&c, "plan_site"));
        assert!(!cloud_tool(&c, "p2p:node3:plan_site"), "exact match");
        assert!(!cloud_tool(&cfg(), "plan_site"), "default list is empty");
        assert!(cloud_tool_escalates(&c, "routine turn", false, "plan_site"));
        assert!(cloud_tool_escalates(
            &c,
            "background session",
            false,
            "foresight"
        ));
        assert!(!cloud_tool_escalates(&c, "routine turn", false, "shell"));
        // Privacy wins over capability, both ways it can arrive.
        assert!(!cloud_tool_escalates(
            &c,
            "private facts in context",
            false,
            "plan_site"
        ));
        assert!(!cloud_tool_escalates(
            &c,
            "private tool result in context",
            false,
            "plan_site"
        ));
        assert!(
            !cloud_tool_escalates(&c, "routine turn", true, "plan_site"),
            "a private tool ran earlier in the turn"
        );
        // A turn with nowhere to go, or no router, stays where it is.
        for r in [
            "routing disabled",
            "cloud unreachable, backing off",
            "daily cloud budget spent",
            "no router",
        ] {
            assert!(!cloud_tool_escalates(&c, r, false, "plan_site"), "{r}");
        }
    }

    #[test]
    fn an_escalation_request_is_exactly_one_json_object() {
        assert_eq!(
            escalation_request(r#"{"escalate": "needs a site plan"}"#).as_deref(),
            Some("needs a site plan")
        );
        assert_eq!(
            escalation_request("  \n{\"escalate\":\"why\"}\n\n").as_deref(),
            Some("why"),
            "surrounding whitespace"
        );
        assert_eq!(
            escalation_request("```json\n{\"escalate\": \"why\"}\n```").as_deref(),
            Some("why"),
            "one fenced block with a tag"
        );
        assert_eq!(
            escalation_request("```\n{\"escalate\": \"why\"}\n```").as_deref(),
            Some("why"),
            "one fenced block without a tag"
        );
        for text in [
            "",
            "I think {\"escalate\": \"why\"} is best",
            "{\"escalate\": \"why\"} and more",
            "{\"escalate\": \"\"}",
            "{\"escalate\": \"   \"}",
            "{\"escalate\": true}",
            "{\"escalate\": \"why\", \"answer\": \"x\"}",
            "{\"reason\": \"why\"}",
            "[{\"escalate\": \"why\"}]",
            "```json\n{\"escalate\": \"why\"}\n```\nso there",
            "```json\n{\"escalate\": \"why\"}",
            "The answer is 42.",
        ] {
            assert_eq!(escalation_request(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_long_why_is_cut_on_a_char_boundary() {
        let why = "x".repeat(MAX_ESCALATION_WHY + 50);
        let got = escalation_request(&format!("{{\"escalate\": \"{why}\"}}")).unwrap();
        assert_eq!(got.chars().count(), MAX_ESCALATION_WHY);
        let why = "é".repeat(MAX_ESCALATION_WHY + 5);
        let got = escalation_request(&format!("{{\"escalate\": \"{why}\"}}")).unwrap();
        assert_eq!(got.chars().count(), MAX_ESCALATION_WHY);
    }

    #[test]
    fn the_contract_names_the_shape_the_parser_accepts() {
        assert!(LOW_CONFIDENCE_CONTRACT.contains(r#"{"escalate": "<one sentence on why>"}"#));
        assert_eq!(
            escalation_request(r#"{"escalate": "<one sentence on why>"}"#).as_deref(),
            Some("<one sentence on why>")
        );
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

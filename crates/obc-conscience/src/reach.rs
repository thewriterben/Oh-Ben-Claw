//! The reach gate (egress allowlist) — Track 0 for what the agent may TOUCH.
//!
//! The lesson of the July 2026 sandbox-escape breach, made structural:
//! containment cannot rely on the model's own refusals. An agent with network
//! reach, stored credentials, and a code-execution tool is a lateral-movement
//! engine unless something outside the model bounds where it may reach.
//!
//! Default-deny egress. Every host, credential, and externally-effecting tool
//! is opt-in and (per the breach) scoped — never standing, never `forever`.
//! Credentials are referenced by name and injected at the boundary for
//! allowlisted hosts only, so a poisoned skill exfiltrates a name, not a secret.
//! A perception tool has no general egress: the "camera becomes a launchpad"
//! chain is refused by configuration, not by the model declining.

use serde::{Deserialize, Serialize};

/// What a tool is permitted to reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReachScope {
    /// No external reach at all (the default for any unlisted tool).
    #[default]
    None,
    /// Local network only — may not open arbitrary internet sockets.
    LanOnly,
    /// May reach allowlisted egress hosts.
    Egress,
}

/// An allowlisted egress host, optionally carrying a named credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRule {
    /// Exact host (e.g. `"api.anthropic.com"`).
    pub host: String,
    /// Purpose, for the audit trail.
    #[serde(default)]
    pub purpose: String,
    /// Name of the credential to inject at the boundary (never the secret).
    #[serde(default)]
    pub credential: Option<String>,
}

/// A tool's declared reach scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReach {
    pub tool: String,
    #[serde(default)]
    pub scope: ReachScope,
}

/// Why a reach was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReachRefusal {
    /// Host is not in the egress allowlist — default-deny.
    HostNotAllowed { host: String },
    /// The tool has no egress scope (its declared scope is None or LanOnly).
    ToolHasNoEgress { tool: String, scope: ReachScope },
}

impl std::fmt::Display for ReachRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReachRefusal::HostNotAllowed { host } => {
                write!(f, "conscience: egress to '{host}' denied (not allowlisted)")
            }
            ReachRefusal::ToolHasNoEgress { tool, scope } => {
                write!(
                    f,
                    "conscience: tool '{tool}' has no egress (scope {scope:?})"
                )
            }
        }
    }
}

impl std::error::Error for ReachRefusal {}

/// The decision the reach gate returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReachDecision {
    /// Reach permitted; inject this named credential (if any) at the boundary.
    Allow {
        credential: Option<String>,
    },
    Refuse(ReachRefusal),
}

impl ReachDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, ReachDecision::Allow { .. })
    }
}

/// Enforces the egress allowlist and per-tool reach scopes.
#[derive(Debug, Clone, Default)]
pub struct ReachGate {
    hosts: Vec<HostRule>,
    tools: Vec<ToolReach>,
}

impl ReachGate {
    pub fn new(hosts: Vec<HostRule>, tools: Vec<ToolReach>) -> Self {
        Self { hosts, tools }
    }

    fn tool_scope(&self, tool: &str) -> ReachScope {
        self.tools
            .iter()
            .find(|t| t.tool == tool)
            .map(|t| t.scope)
            .unwrap_or(ReachScope::None) // default-deny: unlisted tools have no reach
    }

    /// May `tool` reach `host`? Both gates apply: the tool must have egress
    /// scope AND the host must be allowlisted. Returns the named credential to
    /// inject on success (the secret itself is never handled by the model).
    pub fn check(&self, tool: &str, host: &str) -> ReachDecision {
        let scope = self.tool_scope(tool);
        if scope != ReachScope::Egress {
            return ReachDecision::Refuse(ReachRefusal::ToolHasNoEgress {
                tool: tool.to_string(),
                scope,
            });
        }
        match self.hosts.iter().find(|h| h.host == host) {
            None => ReachDecision::Refuse(ReachRefusal::HostNotAllowed {
                host: host.to_string(),
            }),
            Some(rule) => ReachDecision::Allow {
                credential: rule.credential.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> ReachGate {
        ReachGate::new(
            vec![HostRule {
                host: "api.anthropic.com".into(),
                purpose: "brain".into(),
                credential: Some("brain-key".into()),
            }],
            vec![
                ToolReach {
                    tool: "brain".into(),
                    scope: ReachScope::Egress,
                },
                ToolReach {
                    tool: "clawcam".into(),
                    scope: ReachScope::LanOnly,
                },
            ],
        )
    }

    #[test]
    fn allows_scoped_tool_to_allowlisted_host_with_named_credential() {
        let d = gate().check("brain", "api.anthropic.com");
        assert_eq!(
            d,
            ReachDecision::Allow {
                credential: Some("brain-key".into())
            }
        );
    }

    #[test]
    fn denies_egress_to_unlisted_host() {
        let d = gate().check("brain", "evil.example.com");
        assert!(matches!(
            d,
            ReachDecision::Refuse(ReachRefusal::HostNotAllowed { .. })
        ));
    }

    #[test]
    fn perception_tool_has_no_general_egress() {
        // the "camera becomes a launchpad" chain, refused by config
        let d = gate().check("clawcam", "api.anthropic.com");
        assert!(matches!(
            d,
            ReachDecision::Refuse(ReachRefusal::ToolHasNoEgress { .. })
        ));
    }

    #[test]
    fn unlisted_tool_defaults_to_no_reach() {
        let d = gate().check("mystery_tool", "api.anthropic.com");
        assert!(matches!(
            d,
            ReachDecision::Refuse(ReachRefusal::ToolHasNoEgress { .. })
        ));
    }
}

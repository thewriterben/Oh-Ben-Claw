//! Deployment scheme — the output of the `DeploymentPlanner`.
//!
//! A `DeploymentScheme` describes the full agent topology for a given hardware
//! inventory: which sub-agents to spawn, what roles they play, which hardware
//! they own, and a ready-to-paste TOML configuration snippet.

use serde::{Deserialize, Serialize};

// ── Node Role ─────────────────────────────────────────────────────────────────

/// The role of a node in the deployed agent topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// Top-level orchestrator that coordinates all other agents.
    Orchestrator,
    /// Vision agent — handles camera capture and image analysis.
    VisionAgent,
    /// Audio/listening agent — handles microphone input and STT.
    AudioAgent,
    /// Speech/display agent — handles TTS output and display rendering.
    SpeechDisplayAgent,
    /// Environmental sensing agent — reads temperature, humidity, etc.
    SensingAgent,
    /// Generic peripheral agent for GPIO / actuator control.
    PeripheralAgent,
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Orchestrator => write!(f, "orchestrator"),
            Self::VisionAgent => write!(f, "vision-agent"),
            Self::AudioAgent => write!(f, "audio-agent"),
            Self::SpeechDisplayAgent => write!(f, "speech-display-agent"),
            Self::SensingAgent => write!(f, "sensing-agent"),
            Self::PeripheralAgent => write!(f, "peripheral-agent"),
        }
    }
}

// ── Agent Assignment ──────────────────────────────────────────────────────────

/// An agent assignment — one sub-agent in the deployment topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAssignment {
    /// Unique agent name (used in `[orchestrator]` sub-agent config).
    pub name: String,
    /// The logical role this agent plays.
    pub role: NodeRole,
    /// The hardware item this agent owns / drives.
    pub hardware_item: String,
    /// Natural-language role description for the sub-agent's system prompt.
    pub role_description: String,
    /// Tool names this agent should have access to.
    pub tools: Vec<String>,
    /// TOML configuration snippet for this agent.
    pub config_snippet: String,
}

// ── Suggested Hardware ────────────────────────────────────────────────────────

/// A hardware suggestion for satisfying an unsatisfied feature desire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedHardware {
    /// The missing capability token.
    pub missing_capability: String,
    /// The unsatisfied feature desire.
    pub for_feature: String,
    /// Suggested board names that provide the capability.
    pub suggested_boards: Vec<String>,
    /// Human-readable explanation.
    pub reason: String,
}

// ── Deployment Scheme ─────────────────────────────────────────────────────────

/// The complete deployment plan produced by the `DeploymentPlanner`.
///
/// It describes:
/// - The agent topology (which sub-agents to spawn and what they do)
/// - Hardware gaps and suggestions for satisfying unsatisfied desires
/// - Validation warnings
/// - A ready-to-use TOML config snippet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentScheme {
    /// Human-readable scenario name.
    pub scenario_name: String,
    /// The host board name.
    pub host_board: String,
    /// All agent assignments in the swarm.
    pub assignments: Vec<AgentAssignment>,
    /// Unsatisfied feature desires with suggestions for how to fulfil them.
    pub suggested_hardware: Vec<SuggestedHardware>,
    /// Non-fatal warnings the operator should be aware of.
    pub warnings: Vec<String>,
    /// Ready-to-use TOML configuration for `~/.oh-ben-claw/config.toml`.
    pub config_toml: String,
    /// Summary suitable for display or logging.
    pub summary: String,
}

impl DeploymentScheme {
    /// Returns true if all feature desires were satisfied by the available hardware.
    pub fn is_complete(&self) -> bool {
        self.suggested_hardware.is_empty()
    }

    /// Returns the number of sub-agents in the swarm (excluding the orchestrator).
    pub fn sub_agent_count(&self) -> usize {
        self.assignments
            .iter()
            .filter(|a| a.role != NodeRole::Orchestrator)
            .count()
    }

    /// Pretty-print the scheme as a human-readable report.
    pub fn report(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!(
            "# Oh-Ben-Claw Deployment Scheme — {}\n\n",
            self.scenario_name
        ));
        out.push_str(&format!("**Host board:** {}\n\n", self.host_board));

        out.push_str("## Agent Topology\n\n");
        for a in &self.assignments {
            out.push_str(&format!(
                "- **{}** (`{}`) — {}\n",
                a.name, a.role, a.role_description
            ));
            if !a.tools.is_empty() {
                out.push_str(&format!("  Tools: {}\n", a.tools.join(", ")));
            }
        }

        if !self.suggested_hardware.is_empty() {
            out.push_str("\n## Missing Hardware Suggestions\n\n");
            for s in &self.suggested_hardware {
                out.push_str(&format!(
                    "- **{}** (for `{}`) — {}\n",
                    s.missing_capability, s.for_feature, s.reason
                ));
                if !s.suggested_boards.is_empty() {
                    out.push_str(&format!("  Suggested: {}\n", s.suggested_boards.join(", ")));
                }
            }
        }

        if !self.warnings.is_empty() {
            out.push_str("\n## Warnings\n\n");
            for w in &self.warnings {
                out.push_str(&format!("- ⚠️  {}\n", w));
            }
        }

        out.push_str("\n## Generated Configuration\n\n```toml\n");
        out.push_str(&self.config_toml);
        out.push_str("```\n");

        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scheme(
        assignments: Vec<AgentAssignment>,
        suggestions: Vec<SuggestedHardware>,
    ) -> DeploymentScheme {
        DeploymentScheme {
            scenario_name: "test".to_string(),
            host_board: "nanopi-neo3".to_string(),
            assignments,
            suggested_hardware: suggestions,
            warnings: vec![],
            config_toml: "[agent]\nname = \"test\"".to_string(),
            summary: "test summary".to_string(),
        }
    }

    #[test]
    fn is_complete_with_no_suggestions() {
        let scheme = make_scheme(vec![], vec![]);
        assert!(scheme.is_complete());
    }

    #[test]
    fn is_not_complete_with_suggestions() {
        let scheme = make_scheme(
            vec![],
            vec![SuggestedHardware {
                missing_capability: "camera_capture".to_string(),
                for_feature: "vision".to_string(),
                suggested_boards: vec!["xiao-esp32s3-sense".to_string()],
                reason: "No camera hardware found".to_string(),
            }],
        );
        assert!(!scheme.is_complete());
    }

    #[test]
    fn sub_agent_count_excludes_orchestrator() {
        let assignments = vec![
            AgentAssignment {
                name: "orchestrator".to_string(),
                role: NodeRole::Orchestrator,
                hardware_item: "nanopi-neo3".to_string(),
                role_description: "main brain".to_string(),
                tools: vec![],
                config_snippet: String::new(),
            },
            AgentAssignment {
                name: "vision-agent".to_string(),
                role: NodeRole::VisionAgent,
                hardware_item: "xiao".to_string(),
                role_description: "vision".to_string(),
                tools: vec!["camera_capture".to_string()],
                config_snippet: String::new(),
            },
        ];
        let scheme = make_scheme(assignments, vec![]);
        assert_eq!(scheme.sub_agent_count(), 1);
    }

    #[test]
    fn report_includes_scenario_name() {
        let scheme = make_scheme(vec![], vec![]);
        let report = scheme.report();
        assert!(report.contains("test"));
        assert!(report.contains("nanopi-neo3"));
    }
}

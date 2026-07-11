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

// ── Node Position ─────────────────────────────────────────────────────────────

/// Where a deployed node physically sits: geodetic (WGS84) plus site-local ENU
/// metres. Produced by the Conservation Grid coverage optimizer
/// ([`crate::siteplan`]) and attached via [`DeploymentScheme::with_site_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NodePosition {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt: f64,
    /// East metres in the site frame.
    pub enu_e: f64,
    /// North metres in the site frame.
    pub enu_n: f64,
}

impl From<&crate::siteplan::PlacedNode> for NodePosition {
    fn from(n: &crate::siteplan::PlacedNode) -> Self {
        Self {
            lat: n.geo.lat,
            lon: n.geo.lon,
            alt: n.geo.alt,
            enu_e: n.enu.e,
            enu_n: n.enu.n,
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
    /// Physical position from the site coverage optimizer, when a site plan has
    /// been attached ([`DeploymentScheme::with_site_plan`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<NodePosition>,
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

    /// Generate a starter firmware sketch for every node in this scheme that maps to
    /// a **flashable** registry board (serial/probe MCUs; native SBC hosts are
    /// skipped). Closes the scout → registry → firmware → orchestrate loop: the
    /// planner already emits TOML, and now emits the matching starter firmware too.
    pub fn firmware_sketches(&self) -> Vec<crate::deployment::firmware_scaffold::FirmwareSketch> {
        use crate::deployment::firmware_scaffold::{scaffold_firmware, ScaffoldOptions};
        use crate::peripherals::registry::{known_boards, BoardInfo};

        let flashable = |name: &str| -> Option<&'static BoardInfo> {
            known_boards()
                .iter()
                .find(|b| b.name == name && matches!(b.transport, "serial" | "probe"))
        };

        let mut sketches = Vec::new();
        if let Some(b) = flashable(&self.host_board) {
            let opts = ScaffoldOptions {
                node_id: "host".into(),
                ..Default::default()
            };
            sketches.push(scaffold_firmware(b, &opts));
        }
        for a in &self.assignments {
            if let Some(b) = flashable(&a.hardware_item) {
                let opts = ScaffoldOptions {
                    node_id: a.name.clone(),
                    ..Default::default()
                };
                sketches.push(scaffold_firmware(b, &opts));
            }
        }
        sketches
    }

    /// Attach an optimized site plan (Conservation Grid G1): placed positions are
    /// bound onto the **sub-agent** assignments in order (the orchestrator/host is
    /// assumed co-located or off-site and keeps no position), and the plan's
    /// `[site]` TOML is appended to `config_toml` so the rendered config carries
    /// the full layout. A count mismatch becomes a warning, never an error.
    pub fn with_site_plan(mut self, site_id: &str, plan: &crate::siteplan::SitePlan) -> Self {
        let mut nodes = plan.nodes.iter();
        let mut placed = 0usize;
        for a in self
            .assignments
            .iter_mut()
            .filter(|a| a.role != NodeRole::Orchestrator)
        {
            match nodes.next() {
                Some(n) => {
                    a.position = Some(NodePosition::from(n));
                    placed += 1;
                }
                None => break,
            }
        }

        let sub_agents = self.sub_agent_count();
        if placed < sub_agents {
            self.warnings.push(format!(
                "Site plan '{}' has {} node position(s) for {} sub-agent(s); {} agent(s) \
                 have no position",
                site_id,
                plan.nodes.len(),
                sub_agents,
                sub_agents - placed
            ));
        } else if plan.nodes.len() > sub_agents {
            self.warnings.push(format!(
                "Site plan '{}' places {} node(s) but the scheme has only {} sub-agent(s); \
                 extra positions remain available in the [site] TOML",
                site_id,
                plan.nodes.len(),
                sub_agents
            ));
        }

        if !self.config_toml.is_empty() && !self.config_toml.ends_with('\n') {
            self.config_toml.push('\n');
        }
        self.config_toml.push('\n');
        self.config_toml.push_str(&plan.to_toml(site_id));
        self.summary = format!("{} · site '{}': {}", self.summary, site_id, plan.summary());
        self
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
            if let Some(p) = &a.position {
                out.push_str(&format!(
                    "  Position: ({:.6}, {:.6}) · ENU ({:.1} m E, {:.1} m N)\n",
                    p.lat, p.lon, p.enu_e, p.enu_n
                ));
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
                position: None,
            },
            AgentAssignment {
                name: "vision-agent".to_string(),
                role: NodeRole::VisionAgent,
                hardware_item: "xiao".to_string(),
                role_description: "vision".to_string(),
                tools: vec!["camera_capture".to_string()],
                config_snippet: String::new(),
                position: None,
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

    fn make_agent(name: &str, role: NodeRole) -> AgentAssignment {
        AgentAssignment {
            name: name.to_string(),
            role,
            hardware_item: "hw".to_string(),
            role_description: String::new(),
            tools: vec![],
            config_snippet: String::new(),
            position: None,
        }
    }

    fn square_plan(budget: usize) -> crate::siteplan::SitePlan {
        use crate::geo::GeoPoint;
        let dlat = 50.0 / 111_194.9;
        let dlon = 50.0 / (111_194.9 * 45.5_f64.to_radians().cos());
        let site = crate::geo::Site::new(
            "s1",
            "",
            vec![
                GeoPoint::new(45.5 - dlat, -122.6 - dlon, 0.0),
                GeoPoint::new(45.5 - dlat, -122.6 + dlon, 0.0),
                GeoPoint::new(45.5 + dlat, -122.6 + dlon, 0.0),
                GeoPoint::new(45.5 + dlat, -122.6 - dlon, 0.0),
            ],
        );
        let spec = crate::siteplan::PlacementSpec {
            budget,
            ..Default::default()
        };
        crate::siteplan::plan_site(&site, &spec)
    }

    #[test]
    fn with_site_plan_binds_positions_to_sub_agents_only() {
        let scheme = make_scheme(
            vec![
                make_agent("orchestrator", NodeRole::Orchestrator),
                make_agent("vision-agent", NodeRole::VisionAgent),
                make_agent("sensing-agent", NodeRole::SensingAgent),
            ],
            vec![],
        );
        let plan = square_plan(4);
        assert!(plan.nodes.len() >= 2, "test needs ≥2 placed nodes");
        let bound = scheme.with_site_plan("s1", &plan);

        assert!(
            bound.assignments[0].position.is_none(),
            "orchestrator keeps no position"
        );
        let p1 = bound.assignments[1].position.expect("vision agent placed");
        let p2 = bound.assignments[2].position.expect("sensing agent placed");
        assert!((p1.lat - 45.5).abs() < 0.01);
        assert_ne!(p1, p2, "distinct nodes for distinct agents");
        // The site layout is carried in the rendered config.
        assert!(bound.config_toml.contains("[[site.node]]"));
        assert!(bound.summary.contains("site 's1'"));
        // Report surfaces the positions.
        assert!(bound.report().contains("Position: ("));
    }

    #[test]
    fn with_site_plan_short_plan_warns_and_leaves_agents_unplaced() {
        let scheme = make_scheme(
            vec![
                make_agent("orchestrator", NodeRole::Orchestrator),
                make_agent("a1", NodeRole::VisionAgent),
                make_agent("a2", NodeRole::SensingAgent),
            ],
            vec![],
        );
        let plan = square_plan(1); // one node for two sub-agents
        let bound = scheme.with_site_plan("s1", &plan);
        assert!(bound.assignments[1].position.is_some());
        assert!(bound.assignments[2].position.is_none());
        assert!(
            bound.warnings.iter().any(|w| w.contains("no position")),
            "{:?}",
            bound.warnings
        );
    }

    #[test]
    fn with_site_plan_extra_nodes_warn_but_stay_in_toml() {
        let scheme = make_scheme(vec![make_agent("a1", NodeRole::VisionAgent)], vec![]);
        let plan = square_plan(4);
        assert!(plan.nodes.len() > 1, "test needs >1 placed nodes");
        let n_nodes = plan.nodes.len();
        let bound = scheme.with_site_plan("s1", &plan);
        assert!(bound.assignments[0].position.is_some());
        assert!(
            bound.warnings.iter().any(|w| w.contains("extra positions")),
            "{:?}",
            bound.warnings
        );
        assert_eq!(bound.config_toml.matches("[[site.node]]").count(), n_nodes);
    }

    #[test]
    fn position_round_trips_through_serde_and_defaults_to_none() {
        let mut a = make_agent("a1", NodeRole::VisionAgent);
        a.position = Some(NodePosition {
            lat: 45.5,
            lon: -122.6,
            alt: 10.0,
            enu_e: 5.0,
            enu_n: -3.0,
        });
        let json = serde_json::to_string(&a).unwrap();
        let back: AgentAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.position, a.position);
        // Old serialized assignments (no position field) still deserialize.
        let legacy = r#"{"name":"x","role":"vision_agent","hardware_item":"h","role_description":"","tools":[],"config_snippet":""}"#;
        let old: AgentAssignment = serde_json::from_str(legacy).unwrap();
        assert!(old.position.is_none());
    }

    #[test]
    fn firmware_sketches_target_only_flashable_boards() {
        let assignments = vec![AgentAssignment {
            name: "vision-agent".to_string(),
            role: NodeRole::VisionAgent,
            hardware_item: "esp32-s3-cam".to_string(), // serial MCU → flashable
            role_description: "vision".to_string(),
            tools: vec![],
            config_snippet: String::new(),
            position: None,
        }];
        // host is nanopi-neo3 (native SBC) → skipped; only the MCU gets firmware.
        let scheme = make_scheme(assignments, vec![]);
        let sketches = scheme.firmware_sketches();
        assert_eq!(
            sketches.len(),
            1,
            "only the flashable MCU node gets a sketch"
        );
        assert_eq!(sketches[0].board, "esp32-s3-cam");
        assert!(sketches[0].source.contains("void setup()"));
        assert!(
            sketches[0].source.contains("vision-agent"),
            "node id baked in"
        );
    }
}

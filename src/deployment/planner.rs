//! Rule-based deployment planner.
//!
//! The `DeploymentPlanner` maps a `HardwareInventory` to a `DeploymentScheme`
//! using deterministic rules — no LLM required.  It:
//!
//! 1. Identifies the host board.
//! 2. Assigns agent roles to hardware items based on their capabilities.
//! 3. Generates a sub-agent spec for each role.
//! 4. Runs the `HardwareAdvisor` to detect gaps.
//! 5. Renders a complete TOML configuration snippet.
//!
//! The `DeploymentSwarm` wraps this planner with an LLM-powered multi-agent
//! layer that can refine the scheme, add contextual annotations, and answer
//! follow-up questions.

use crate::deployment::advisor::HardwareAdvisor;
use crate::deployment::inventory::{FeatureDesire, HardwareInventory, ItemRole};
use crate::deployment::scheme::{AgentAssignment, DeploymentScheme, NodeRole};

// ── Deployment Planner ────────────────────────────────────────────────────────

/// Deterministic rule-based deployment planner.
pub struct DeploymentPlanner;

impl DeploymentPlanner {
    /// Plan a complete deployment from a hardware inventory.
    pub fn plan(inventory: &HardwareInventory) -> DeploymentScheme {
        let mut assignments: Vec<AgentAssignment> = Vec::new();
        let mut warnings = HardwareAdvisor::validate(inventory);

        // ── Identify host board ───────────────────────────────────────────────
        let host_name = inventory
            .find_role(&ItemRole::Host)
            .or_else(|| inventory.items.iter().find(|i| i.transport == "native"))
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "unknown-host".to_string());

        let host_board = inventory
            .find_role(&ItemRole::Host)
            .or_else(|| inventory.items.iter().find(|i| i.transport == "native"))
            .map(|i| i.board_name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // ── Orchestrator ──────────────────────────────────────────────────────
        assignments.push(AgentAssignment {
            name: "orchestrator".to_string(),
            role: NodeRole::Orchestrator,
            hardware_item: host_name.clone(),
            role_description: format!(
                "Top-level orchestrator running on {}. Coordinates all sub-agents, \
                 manages conversation context, and delegates specialised tasks.",
                host_name
            ),
            tools: vec![
                "spawn_agent".to_string(),
                "delegate_task".to_string(),
                "list_agents".to_string(),
                "stop_agent".to_string(),
                "shell".to_string(),
                "file_read".to_string(),
                "file_write".to_string(),
                "http_get".to_string(),
                "memory_note".to_string(),
            ],
            config_snippet: String::new(),
        });

        // ── Vision agent ──────────────────────────────────────────────────────
        let vision_items = inventory.items_with_capability("camera_capture");
        if !vision_items.is_empty() {
            let item = vision_items[0];
            assignments.push(AgentAssignment {
                name: "vision-agent".to_string(),
                role: NodeRole::VisionAgent,
                hardware_item: item.name.clone(),
                role_description: format!(
                    "Vision specialist running on {}. Captures images, analyses visual \
                     context, detects objects and scenes, and reports findings to the orchestrator.",
                    item.name
                ),
                tools: vec![
                    "camera_capture".to_string(),
                    "sensor_read".to_string(),
                ],
                config_snippet: format!(
                    "# vision-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
            });
        } else if inventory.feature_desires.contains(&FeatureDesire::Vision) {
            warnings.push(
                "Vision desired but no camera_capture hardware found. \
                 Add a XIAO ESP32S3-Sense or similar camera board."
                    .to_string(),
            );
        }

        // ── Audio / Listening agent ───────────────────────────────────────────
        let audio_items = inventory.items_with_capability("audio_sample");
        // Prefer dedicated listening boards (Sipeed mic array) over multi-role boards
        let dedicated_audio = audio_items
            .iter()
            .find(|i| i.role == ItemRole::Listening)
            .or_else(|| audio_items.first());
        if let Some(item) = dedicated_audio {
            assignments.push(AgentAssignment {
                name: "audio-agent".to_string(),
                role: NodeRole::AudioAgent,
                hardware_item: item.name.clone(),
                role_description: format!(
                    "Audio specialist running on {}. Captures microphone audio, performs \
                     speech-to-text transcription, detects wake words, and forwards \
                     transcriptions to the orchestrator.",
                    item.name
                ),
                tools: vec![
                    "audio_sample".to_string(),
                    "sensor_read".to_string(),
                ],
                config_snippet: format!(
                    "# audio-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
            });
        } else if inventory
            .feature_desires
            .contains(&FeatureDesire::Listening)
        {
            warnings.push(
                "Listening desired but no audio_sample hardware found. \
                 Add a Sipeed 6+1 Mic Array or similar microphone board."
                    .to_string(),
            );
        }

        // ── Speech / Display agent ────────────────────────────────────────────
        let display_items = inventory.items_with_capability("display");
        if !display_items.is_empty() {
            let item = display_items[0];
            let has_touch = item.has_capability("touch");
            let has_speaker = item.has_capability("audio_sample");
            assignments.push(AgentAssignment {
                name: "speech-display-agent".to_string(),
                role: NodeRole::SpeechDisplayAgent,
                hardware_item: item.name.clone(),
                role_description: format!(
                    "Display and speech output specialist running on {}. Renders text \
                     and status information on the display{}{}.",
                    item.name,
                    if has_touch { ", accepts touch input" } else { "" },
                    if has_speaker { ", and plays synthesised speech through the integrated speaker" } else { "" }
                ),
                tools: vec![
                    "sensor_read".to_string(),
                    "gpio_write".to_string(),
                ],
                config_snippet: format!(
                    "# speech-display-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
            });
        } else if inventory
            .feature_desires
            .contains(&FeatureDesire::DisplayOutput)
        {
            warnings.push(
                "Display output desired but no 'display' capability hardware found. \
                 Add a Waveshare ESP32-S3-Touch-LCD-2.1 or similar display board."
                    .to_string(),
            );
        }

        // ── Sensing agent ─────────────────────────────────────────────────────
        // Collect boards/accessories that provide sensor_read without being used
        // for another primary role already.
        let assigned: Vec<String> = assignments
            .iter()
            .map(|a| a.hardware_item.clone())
            .collect();
        let sensing_items: Vec<_> = inventory
            .items
            .iter()
            .filter(|i| {
                i.has_capability("sensor_read")
                    && !assigned.contains(&i.name)
                    && i.name != host_name
            })
            .collect();

        // Also: if the host has accessories with sensor_read (e.g. DHT22), add a sensing agent.
        let host_has_sensors = inventory
            .find_role(&ItemRole::Host)
            .map(|h| h.has_capability("sensor_read"))
            .unwrap_or(false);

        if !sensing_items.is_empty() || host_has_sensors {
            let hw_name = sensing_items
                .first()
                .map(|i| i.name.as_str())
                .unwrap_or(host_name.as_str());
            assignments.push(AgentAssignment {
                name: "sensing-agent".to_string(),
                role: NodeRole::SensingAgent,
                hardware_item: hw_name.to_string(),
                role_description: format!(
                    "Environmental sensing specialist. Reads temperature, humidity, and \
                     other environmental data from sensors attached to {}. Logs readings \
                     and triggers alerts when thresholds are exceeded.",
                    hw_name
                ),
                tools: vec![
                    "sensor_read".to_string(),
                    "i2c_read".to_string(),
                    "gpio_read".to_string(),
                    "memory_note".to_string(),
                ],
                config_snippet: String::new(),
            });
        } else if inventory
            .feature_desires
            .contains(&FeatureDesire::EnvironmentalSensing)
        {
            warnings.push(
                "Environmental sensing desired but no sensor_read hardware found. \
                 Add a DHT22 (connected to GPIO), BME280 (I2C), or similar sensor."
                    .to_string(),
            );
        }

        // ── Hardware gap analysis ─────────────────────────────────────────────
        let suggested_hardware = HardwareAdvisor::suggest_missing(inventory);

        // ── Generate TOML config ──────────────────────────────────────────────
        let config_toml = Self::render_config(inventory, &assignments, &host_board);

        // ── Summary ───────────────────────────────────────────────────────────
        let summary = format!(
            "Deployment '{}': {} agent(s), host={}, sub-agents=[{}], gaps={}",
            inventory.scenario_name,
            assignments.len(),
            host_board,
            assignments
                .iter()
                .filter(|a| a.role != NodeRole::Orchestrator)
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            suggested_hardware.len()
        );

        DeploymentScheme {
            scenario_name: inventory.scenario_name.clone(),
            host_board,
            assignments,
            suggested_hardware,
            warnings,
            config_toml,
            summary,
        }
    }

    /// Render the TOML configuration snippet for the deployment.
    fn render_config(
        inventory: &HardwareInventory,
        assignments: &[AgentAssignment],
        host_board: &str,
    ) -> String {
        let mut out = String::new();

        out.push_str("# Generated by Oh-Ben-Claw DeploymentPlanner\n");
        out.push_str(&format!("# Scenario: {}\n\n", inventory.scenario_name));

        // [agent]
        out.push_str("[agent]\n");
        out.push_str(&format!(
            "name = \"Oh-Ben-Claw ({})\"\n",
            inventory.scenario_name
        ));
        out.push_str(
            "system_prompt = \"\"\"\n\
            You are Oh-Ben-Claw, an advanced multi-device AI assistant deployed on a\n\
            hardware swarm. You can see, hear, sense, and display information through\n\
            your connected peripheral agents. Coordinate them efficiently.\n\
            \"\"\"\n",
        );
        out.push_str("max_tool_iterations = 15\n\n");

        // [provider]
        out.push_str("[provider]\n");
        out.push_str("name = \"openai\"\n");
        out.push_str("model = \"gpt-4o\"\n");
        out.push_str("# api_key = \"sk-...\"  # Or set OPENAI_API_KEY\n\n");

        // [spine]
        out.push_str("[spine]\n");
        out.push_str("kind = \"mqtt\"\n");
        out.push_str("host = \"localhost\"\n");
        out.push_str("port = 1883\n");
        out.push_str("tool_timeout_secs = 30\n\n");

        // [edge] — for native host boards
        if host_board == "nanopi-neo3" || host_board.starts_with("raspberry-pi") {
            out.push_str("[edge]\n");
            out.push_str("enabled = true\n");
            out.push_str("max_history_messages = 20\n");
            out.push_str("max_tool_iterations = 5\n");
            out.push_str("p2p_enabled = true\n\n");
        }

        // [peripherals]
        out.push_str("[peripherals]\n");
        out.push_str("enabled = true\n");
        out.push_str("datasheet_dir = \"docs/datasheets\"\n\n");

        // [[peripherals.boards]] for each non-host item
        for item in &inventory.items {
            if item.role == ItemRole::Host {
                // Host is native, just emit the native board entry
                out.push_str("[[peripherals.boards]]\n");
                out.push_str(&format!("board = \"{}\"\n", item.board_name));
                out.push_str("transport = \"native\"\n");
                if !item.accessories.is_empty() {
                    out.push_str(&format!("# accessories: {}\n", item.accessories.join(", ")));
                }
                out.push('\n');
            } else {
                out.push_str("[[peripherals.boards]]\n");
                out.push_str(&format!("board = \"{}\"\n", item.board_name));
                out.push_str(&format!("transport = \"{}\"\n", item.transport));
                if item.transport == "serial" {
                    out.push_str(&format!(
                        "path = \"{}\"  # adjust to actual port\n",
                        item.path.as_deref().unwrap_or("/dev/ttyUSB0")
                    ));
                    out.push_str("baud = 115200\n");
                } else if item.transport == "mqtt" {
                    out.push_str(&format!(
                        "node_id = \"{}\"\n",
                        item.node_id.as_deref().unwrap_or(&item.name)
                    ));
                }
                out.push('\n');
            }
        }

        // [orchestrator]
        let sub_agents: Vec<_> = assignments
            .iter()
            .filter(|a| a.role != NodeRole::Orchestrator)
            .collect();

        if !sub_agents.is_empty() {
            out.push_str("[orchestrator]\n");
            out.push_str("enabled = true\n");
            out.push_str("routing = \"manual\"\n\n");

            for sa in &sub_agents {
                out.push_str("[[orchestrator.agents]]\n");
                out.push_str(&format!("name = \"{}\"\n", sa.name));
                out.push_str(&format!("role = \"{}\"\n", sa.role_description));
                if !sa.tools.is_empty() {
                    out.push_str(&format!(
                        "tools = [{}]\n",
                        sa.tools
                            .iter()
                            .map(|t| format!("\"{}\"", t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                out.push('\n');
            }
        }

        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::inventory::HardwareInventory;

    #[test]
    fn plan_nanopi_scenario_produces_orchestrator() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme
            .assignments
            .iter()
            .any(|a| a.role == NodeRole::Orchestrator));
    }

    #[test]
    fn plan_nanopi_scenario_has_vision_agent() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme
            .assignments
            .iter()
            .any(|a| a.role == NodeRole::VisionAgent));
        let va = scheme
            .assignments
            .iter()
            .find(|a| a.role == NodeRole::VisionAgent)
            .unwrap();
        assert!(va.hardware_item.contains("xiao"));
    }

    #[test]
    fn plan_nanopi_scenario_has_audio_agent() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme
            .assignments
            .iter()
            .any(|a| a.role == NodeRole::AudioAgent));
    }

    #[test]
    fn plan_nanopi_scenario_has_speech_display_agent() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme
            .assignments
            .iter()
            .any(|a| a.role == NodeRole::SpeechDisplayAgent));
        let sda = scheme
            .assignments
            .iter()
            .find(|a| a.role == NodeRole::SpeechDisplayAgent)
            .unwrap();
        assert!(sda.hardware_item.contains("waveshare"));
    }

    #[test]
    fn plan_nanopi_scenario_has_sensing_agent() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme
            .assignments
            .iter()
            .any(|a| a.role == NodeRole::SensingAgent));
    }

    #[test]
    fn plan_nanopi_scenario_has_no_suggestions() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(
            scheme.suggested_hardware.is_empty(),
            "Unexpected suggestions: {:?}",
            scheme.suggested_hardware
        );
    }

    #[test]
    fn plan_host_identified_correctly() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert_eq!(scheme.host_board, "nanopi-neo3");
    }

    #[test]
    fn plan_config_toml_contains_peripherals() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(scheme.config_toml.contains("[peripherals]"));
        assert!(scheme.config_toml.contains("nanopi-neo3"));
        assert!(scheme.config_toml.contains("[orchestrator]"));
    }

    #[test]
    fn plan_summary_contains_agent_count() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(!scheme.summary.is_empty());
        assert!(scheme.summary.contains("NanoPi-Neo3 Reference Deployment"));
    }

    #[test]
    fn plan_empty_inventory_produces_warnings() {
        let inv = HardwareInventory::new("empty");
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(!scheme.warnings.is_empty());
    }

    #[test]
    fn plan_scheme_report_is_non_empty() {
        let inv = HardwareInventory::nanopi_scenario();
        let scheme = DeploymentPlanner::plan(&inv);
        let report = scheme.report();
        assert!(report.contains("Agent Topology"));
        assert!(report.contains("orchestrator"));
    }
}

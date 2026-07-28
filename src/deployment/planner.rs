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
            // Every name here is a tool the agent actually registers. `file_read`,
            // `file_write`, `http_get` and `memory_note` were not — the real ones are
            // `file`, `http` and `memory` — so a generated config named four tools
            // that do not exist, in the agent that coordinates all the others.
            tools: vec![
                "spawn_agent".to_string(),
                "delegate_task".to_string(),
                "list_agents".to_string(),
                "stop_agent".to_string(),
                "shell".to_string(),
                "file".to_string(),
                "http".to_string(),
                "memory".to_string(),
            ],
            config_snippet: String::new(),
            position: None,
        });

        // ── Vision agent ──────────────────────────────────────────────────────
        let vision_items = inventory.items_with_capability("camera_capture");
        // An item the user explicitly marked `Vision` beats the first one that merely
        // has a camera; several boards carry `camera_capture` incidentally.
        let vision_pick = vision_items
            .iter()
            .find(|i| i.role == ItemRole::Vision)
            .or_else(|| vision_items.first())
            .copied();
        if let Some(item) = vision_pick {
            assignments.push(AgentAssignment {
                name: "vision-agent".to_string(),
                role: NodeRole::VisionAgent,
                hardware_item: item.name.clone(),
                role_description: format!(
                    "Vision specialist running on {}. Captures images, analyses visual \
                     context, detects objects and scenes, and reports findings to the orchestrator.",
                    item.name
                ),
                // The node captures, the host analyses. `sensor_read` had nothing to
                // do with the role description this agent is given.
                tools: vec![
                    "camera_capture".to_string(),
                    "vision_analyze".to_string(),
                ],
                config_snippet: format!(
                    "# vision-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
                position: None,
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
                    "audio_transcribe".to_string(),
                ],
                config_snippet: format!(
                    "# audio-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
                position: None,
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
            // `audio_sample` is the MICROPHONE capability; `audio_output` is the
            // speaker. Checking the former described a board that can only listen as
            // one that "plays synthesised speech through the integrated speaker" —
            // the TypeScript port had already found and fixed this.
            let has_speaker = item.has_capability("audio_output");
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
                    "gpio_write".to_string(),
                    "speak".to_string(),
                ],
                config_snippet: format!(
                    "# speech-display-agent peripheral\n[[peripherals.boards]]\nboard = \"{}\"\ntransport = \"{}\"{}",
                    item.board_name,
                    item.transport,
                    item.path.as_deref().map(|p| format!("\npath = \"{}\"", p)).unwrap_or_default()
                ),
                position: None,
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
                    "Environmental sensing specialist running on {}. Reads temperature, \
                     humidity, and other sensor data, and forwards readings to the \
                     orchestrator.",
                    hw_name
                ),
                tools: vec![
                    "sensor_read".to_string(),
                    "i2c_read".to_string(),
                    "gpio_read".to_string(),
                ],
                config_snippet: String::new(),
                position: None,
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
    /// Render the TOML configuration for the deployment.
    ///
    /// # This output is fixtured, byte for byte
    ///
    /// The TypeScript port in the deployment generator must produce exactly these
    /// bytes for the same inventory, and `parity/fixtures/deployment/*/expected-config.toml`
    /// in OBC-Prime holds both to it. Before that fixture existed the two had quietly
    /// diverged into different *sections*: this side emitted `[edge]` and a hardcoded
    /// `[provider]`, that side emitted `[fleet.lora_serial]`, `[memory]` and the
    /// `[deployment]` block, and the drift gate saw none of it because the goldens
    /// only covered `[deployment]`.
    ///
    /// # Every key here exists in the agent's config schema
    ///
    /// That is the rule that settled the merge, and it is not a style preference —
    /// the root `Config` does not reject unknown keys, so a key that is not in the
    /// schema parses cleanly and does nothing. Three such keys were found while
    /// merging (`[memory] backend`/`path`, `accessories` on a board entry,
    /// `datasheet_dir`, whose only consumer was deleted). A generated config is the
    /// first one a user ever reads; it should not contain instructions the runtime
    /// ignores.
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
            "name = \"{}\"\n",
            Self::slug(&inventory.scenario_name)
        ));
        out.push_str(
            "system_prompt = \"\"\"\n\
            You are an embodied agent deployed on a hardware swarm. You can see, hear,\n\
            sense and act through your connected peripheral nodes. Reach for a tool when\n\
            the answer depends on the current state of the machine; otherwise just answer.\n\
            Confirm before any command that moves hardware.\n\
            \"\"\"\n",
        );
        out.push_str("max_tool_iterations = 20\n\n");

        // [provider] — commented on purpose.
        //
        // A hardcoded vendor here overrides first-run resolution, which exists so that
        // someone holding an ANTHROPIC_API_KEY is not told OPENAI_API_KEY is missing.
        // Pinning is a choice the reader makes; this offers it rather than making it.
        out.push_str(
            "# Brain: export ANTHROPIC_API_KEY, OPENAI_API_KEY or OPENROUTER_API_KEY and\n",
        );
        out.push_str(
            "# the agent uses that provider. No key anywhere falls back to a local Ollama.\n",
        );
        out.push_str("# Uncomment to pin one instead:\n");
        out.push_str("# [provider]\n");
        out.push_str("# name = \"anthropic\"\n");
        out.push_str("# model = \"claude-sonnet-4-5\"\n\n");

        // [peripherals]
        out.push_str("[peripherals]\n");
        out.push_str("enabled = true\n\n");

        for item in &inventory.items {
            out.push_str("[[peripherals.boards]]\n");
            out.push_str(&format!("board = \"{}\"\n", item.board_name));
            out.push_str(&format!("transport = \"{}\"\n", item.transport));
            if item.transport == "serial" {
                out.push_str(&format!(
                    "path = \"{}\"  # adjust to the port your board enumerates as\n",
                    item.path.as_deref().unwrap_or("/dev/ttyUSB0")
                ));
                out.push_str("baud = 115200\n");
            }
            if let Some(node_id) = item.node_id.as_deref() {
                out.push_str(&format!("node_id = \"{}\"\n", node_id));
            }
            // Accessories are deliberately not emitted here: PeripheralBoardConfig has
            // no such field. They reach the runtime through [[deployment.hardware]],
            // which does carry them and is what the planner reads back.
            out.push('\n');
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

        // [spine] — only when something is not on the host itself.
        if inventory.items.iter().any(|i| i.transport != "native") {
            out.push_str("[spine]\n");
            out.push_str("kind = \"mqtt\"\n");
            out.push_str("host = \"localhost\"\n");
            out.push_str("port = 1883\n");
            out.push_str("tool_timeout_secs = 30\n\n");
        }

        // [edge] — small single-board hosts get the reduced limits.
        if host_board == "nanopi-neo3" || host_board.starts_with("raspberry-pi") {
            out.push_str("[edge]\n");
            out.push_str("enabled = true\n");
            out.push_str("max_history_messages = 20\n");
            out.push_str("max_tool_iterations = 5\n");
            out.push_str("p2p_enabled = true\n\n");
        }

        // [fleet] — LoRa mesh coordination, when the inventory can actually carry it.
        if inventory
            .feature_desires
            .iter()
            .any(|d| matches!(d, FeatureDesire::WirelessMesh))
        {
            out.push_str("[fleet]\n");
            out.push_str("enabled = true\n\n");

            // No transport filter: a LoRa board reached over MQTT still needs the
            // serial bridge described, and  falls back to the usual device
            // path. Requiring "serial" here silently dropped the block for boards
            // the user had every reason to expect it for.
            if let Some(lora) = inventory
                .items
                .iter()
                .find(|i| i.has_capability("lora") || i.has_capability("mesh"))
            {
                out.push_str(
                    "# Serial LoRa-mesh node (firmware/lora-node). Set the real device path.\n",
                );
                out.push_str("[fleet.lora_serial]\n");
                out.push_str(&format!(
                    "port = \"{}\"\n",
                    lora.path.as_deref().unwrap_or("/dev/ttyUSB0")
                ));
                out.push_str("baud = 115200\n");
                out.push_str("relay_hops = 3\n\n");
            }
        }

        // Persistent memory needs no config block. Saying where the database goes is
        // the useful part, and it is the question people actually ask.
        if inventory
            .feature_desires
            .iter()
            .any(|d| matches!(d, FeatureDesire::PersistentMemory))
        {
            out.push_str(
                "# Persistent memory is on by default; the database lives in the data root\n",
            );
            out.push_str(
                "# (platform-specific). Set OBC_DATA_DIR, or [paths].data_dir, to move it —\n",
            );
            out.push_str(
                "# and give a second agent on the same machine its own, or they share one\n",
            );
            out.push_str("# database and one set of standing approval grants.\n\n");
        }

        out.push_str(&inventory.to_deployment_toml());

        out
    }

    /// `"Trailwatch Camera"` → `"trailwatch-camera"`.
    ///
    /// The TypeScript port does `.toLowerCase().replace(/\s+/g, "-")`, and this has
    /// to match it byte for byte, so it collapses runs of whitespace rather than
    /// mapping each space.
    fn slug(name: &str) -> String {
        name.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
    }

    // The role description is emitted whole. The TypeScript port used to take
    // `.split(".")[0]`, which truncates at the *first* full stop — and board names
    // carry version dots, so "running on waveshare-esp32-s3-touch-lcd-2.1" became
    // "...-lcd-2". Matching that would have made the parity gate enforce a bug.
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

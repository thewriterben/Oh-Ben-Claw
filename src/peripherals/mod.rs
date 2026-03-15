//! Oh-Ben-Claw Peripheral Subsystem
//!
//! This module manages connections to physical hardware peripheral nodes.
//! It supports three transport modes:
//!
//! - **Serial**: Direct USB/UART connection to a microcontroller (ESP32, Arduino, STM32).
//! - **Native**: Direct access to hardware on the host machine (Raspberry Pi GPIO, NanoPi GPIO).
//! - **MQTT**: Network-based connection via the Oh-Ben-Claw MQTT spine.
//!
//! # Multi-Board Support
//!
//! Multiple boards can be configured simultaneously. The `create_peripheral_tools`
//! function connects to all configured boards and merges their tools into a single
//! unified registry for the agent.

pub mod registry;
pub mod sensors;
pub mod traits;

#[cfg(all(feature = "peripheral-nanopi", target_os = "linux"))]
pub mod nanopi;

#[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
pub mod rpi;

pub use traits::Peripheral;

use crate::spine::{SpineClient, NodeAnnouncement, NodeToolSpec};
use crate::config::{PeripheralBoardConfig, PeripheralsConfig};
use crate::tools::traits::Tool;
use anyhow::Result;
use std::sync::Arc;

/// Create the unified tool registry from all configured peripheral boards.
///
/// This function iterates over all boards in the configuration, connects to each
/// one using the appropriate transport, and collects all their tools into a
/// single `Vec`. The agent can then use any of these tools transparently.
pub async fn create_peripheral_tools(
    config: &PeripheralsConfig,
    spine: Option<Arc<SpineClient>>,
) -> Result<Vec<Box<dyn Tool>>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();

    for board in &config.boards {
        tracing::info!(board = %board.board, transport = %board.transport, "Connecting to peripheral board");

        match board.transport.as_str() {
            "mqtt" => {
                if let Some(ref spine_client) = spine {
                    let node_id = board
                        .node_id
                        .clone()
                        .unwrap_or_else(|| board.board.clone());
                    tracing::info!(
                        board = %board.board,
                        node_id = %node_id,
                        "Registering MQTT peripheral node (tools will be discovered via spine)"
                    );
                    // MQTT peripheral tools are registered dynamically when the node
                    // announces itself on the spine. See `spine::SpineClient::subscribe_announcements`.
                    let _ = spine_client;
                } else {
                    tracing::warn!(
                        board = %board.board,
                        "MQTT transport configured but no spine client available; skipping"
                    );
                }
            }

            #[cfg(all(feature = "peripheral-nanopi", target_os = "linux"))]
            "native" if board.board == "nanopi-neo3" || board.board == "nanopi-gpio" => {
                match nanopi::NanoPiGpioPeripheral::connect_from_config(board).await {
                    Ok(peripheral) => {
                        tools.extend(peripheral.tools());
                        tracing::info!(board = %board.board, "NanoPi GPIO peripheral connected");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to connect NanoPi GPIO {}: {}", board.board, e);
                    }
                }
            }

            #[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
            "native" if board.board == "rpi-gpio" => {
                match rpi::RpiGpioPeripheral::connect_from_config(board).await {
                    Ok(peripheral) => {
                        tools.extend(peripheral.tools());
                        tracing::info!(board = %board.board, "Raspberry Pi GPIO peripheral connected");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to connect RPi GPIO {}: {}", board.board, e);
                    }
                }
            }

            "serial" => {
                // Serial transport: ESP32, ESP32-S3, Arduino, STM32, etc.
                // TODO: Implement serial transport connection
                tracing::info!(
                    board = %board.board,
                    path = ?board.path,
                    baud = board.baud,
                    "Serial peripheral (connection logic pending)"
                );
            }

            other => {
                tracing::warn!(
                    board = %board.board,
                    transport = %other,
                    "Unknown transport; skipping"
                );
            }
        }
    }

    tracing::info!(tool_count = tools.len(), "Peripheral tool registry built");
    Ok(tools)
}

/// Build a `NodeAnnouncement` for a peripheral board, suitable for publishing
/// to the MQTT spine when the board starts up.
pub fn build_announcement(
    node_id: &str,
    board_config: &PeripheralBoardConfig,
    tool_specs: Vec<NodeToolSpec>,
) -> NodeAnnouncement {
    NodeAnnouncement {
        node_id: node_id.to_string(),
        board: board_config.board.clone(),
        firmware_version: env!("CARGO_PKG_VERSION").to_string(),
        tools: tool_specs,
        metadata: serde_json::json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_peripheral_tools_returns_empty_when_disabled() {
        let config = PeripheralsConfig {
            enabled: false,
            ..Default::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tools = rt.block_on(create_peripheral_tools(&config, None)).unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn build_announcement_includes_node_id_and_board() {
        let board_config = PeripheralBoardConfig {
            board: "esp32-s3".to_string(),
            transport: "mqtt".to_string(),
            path: None,
            baud: 115_200,
            node_id: Some("esp32-s3-living-room".to_string()),
        };
        let announcement = build_announcement("esp32-s3-living-room", &board_config, vec![]);
        assert_eq!(announcement.node_id, "esp32-s3-living-room");
        assert_eq!(announcement.board, "esp32-s3");
    }
}

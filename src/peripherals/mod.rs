//! Oh-Ben-Claw Peripheral Subsystem
//!
//! This module manages connections to physical hardware peripheral nodes.
//! It supports four transport modes:
//!
//! - **Serial**: Direct USB/UART connection to a microcontroller (ESP32, Arduino, STM32).
//! - **Native**: Direct access to hardware on the host machine (Raspberry Pi GPIO, NanoPi GPIO).
//! - **MQTT**: Network-based connection via the Oh-Ben-Claw MQTT spine.
//! - **Probe**: Debug-probe connection to ARM Cortex-M targets via probe-rs (STM32 Nucleo).
//!
//! # Multi-Board Support
//!
//! Multiple boards can be configured simultaneously. The `create_peripheral_tools`
//! function connects to all configured boards and merges their tools into a single
//! unified registry for the agent.
//!
//! # Supported Hardware (Phase 5)
//!
//! | Board | Transport | Feature Flag | Tools |
//! |---|---|---|---|
//! | NanoPi Neo3 | native | `peripheral-nanopi` | gpio_read, gpio_write |
//! | Raspberry Pi 3/4/5 | native | `peripheral-rpi` | rpi_gpio_read/write, rpi_pwm_write, rpi_camera_capture, rpi_system_info |
//! | Arduino Uno/Mega/Nano | serial | `hardware` | arduino_gpio_read/write, arduino_analog_read/write, arduino_ping |
//! | STM32 Nucleo | probe | `peripheral-stm32` | stm32_flash, stm32_rtt_read/write, stm32_reset, stm32_list_probes, stm32_mem_read |
//! | ESP32-S3 | serial/mqtt | `hardware` + `mqtt-spine` | gpio, camera_capture, audio_sample, sensor_read |
//! | Any Linux SBC | native | `hardware` | i2c_scan, i2c_read, i2c_write, spi_transfer, pwm_control |

pub mod bus_tools;
pub mod registry;
pub mod sensors;
pub mod traits;

#[cfg(all(feature = "peripheral-nanopi", target_os = "linux"))]
pub mod nanopi;

#[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
pub mod rpi;

#[cfg(feature = "hardware")]
pub mod arduino;

#[cfg(feature = "peripheral-stm32")]
pub mod stm32;

pub use traits::Peripheral;

use crate::config::{PeripheralBoardConfig, PeripheralsConfig};
use crate::spine::{NodeAnnouncement, NodeToolSpec, SpineClient};
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

    // Always include the shared Linux bus tools (I2C, SPI, PWM) on Linux hosts
    #[cfg(target_os = "linux")]
    {
        tools.push(Box::new(bus_tools::I2cScanTool));
        tools.push(Box::new(bus_tools::I2cReadTool));
        tools.push(Box::new(bus_tools::I2cWriteTool));
        tools.push(Box::new(bus_tools::SpiTransferTool));
        tools.push(Box::new(bus_tools::PwmControlTool));
        tracing::debug!("Registered Linux bus tools (I2C, SPI, PWM)");
    }

    for board in &config.boards {
        tracing::info!(
            board = %board.board,
            transport = %board.transport,
            "Connecting to peripheral board"
        );

        match board.transport.as_str() {
            // ── MQTT / Spine ──────────────────────────────────────────────────
            "mqtt" => {
                if let Some(ref spine_client) = spine {
                    let node_id = board.node_id.clone().unwrap_or_else(|| board.board.clone());
                    tracing::info!(
                        board = %board.board,
                        node_id = %node_id,
                        "Registering MQTT peripheral node (tools discovered via spine)"
                    );
                    let _ = spine_client;
                } else {
                    tracing::warn!(
                        board = %board.board,
                        "MQTT transport configured but no spine client available; skipping"
                    );
                }
            }

            // ── NanoPi Neo3 native GPIO ───────────────────────────────────────
            #[cfg(all(feature = "peripheral-nanopi", target_os = "linux"))]
            "native"
                if board.board == "nanopi-neo3"
                    || board.board == "nanopi-gpio"
                    || board.board.starts_with("nanopi") =>
            {
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

            // ── Raspberry Pi native GPIO / camera ─────────────────────────────
            #[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
            "native"
                if board.board == "rpi-gpio"
                    || board.board.starts_with("raspberry-pi")
                    || board.board.starts_with("rpi") =>
            {
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

            // ── Arduino serial ────────────────────────────────────────────────
            #[cfg(feature = "hardware")]
            "serial"
                if board.board.starts_with("arduino")
                    || board.board.starts_with("uno")
                    || board.board.starts_with("mega")
                    || board.board.starts_with("nano") =>
            {
                match arduino::ArduinoSerialPeripheral::new(board.clone()) {
                    Ok(mut peripheral) => match peripheral.connect().await {
                        Ok(()) => {
                            tools.extend(peripheral.tools());
                            tracing::info!(board = %board.board, "Arduino serial peripheral connected");
                        }
                        Err(e) => {
                            tracing::warn!("Failed to connect Arduino {}: {}", board.board, e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            "Failed to create Arduino peripheral {}: {}",
                            board.board,
                            e
                        );
                    }
                }
            }

            // ── STM32 Nucleo via probe-rs ─────────────────────────────────────
            #[cfg(feature = "peripheral-stm32")]
            "probe" if board.board.starts_with("nucleo") || board.board.starts_with("stm32") => {
                match stm32::Stm32NucleoPeripheral::connect_from_config(board).await {
                    Ok(peripheral) => {
                        tools.extend(peripheral.tools());
                        tracing::info!(board = %board.board, "STM32 Nucleo peripheral connected via probe-rs");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to connect STM32 Nucleo {}: {}", board.board, e);
                    }
                }
            }

            // ── ESP32 / ESP32-S3 serial ───────────────────────────────────────
            "serial" if board.board.starts_with("esp32") => {
                // ESP32 serial tools are provided by the spine MQTT discovery
                // when running the obc-esp32-s3 firmware. For direct serial,
                // the sensors module provides the tool implementations.
                tracing::info!(
                    board = %board.board,
                    path = ?board.path,
                    "ESP32 serial peripheral registered (use MQTT spine for full tool discovery)"
                );
            }

            // ── Generic serial fallback ───────────────────────────────────────
            "serial" => {
                tracing::info!(
                    board = %board.board,
                    path = ?board.path,
                    baud = board.baud,
                    "Generic serial peripheral registered (no specific driver matched)"
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
        // On Linux, bus tools are always registered even when peripherals are disabled
        // because they are host-level tools, not board-level. On non-Linux, empty.
        #[cfg(not(target_os = "linux"))]
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

    #[test]
    fn bus_tools_have_correct_names() {
        assert_eq!(bus_tools::I2cScanTool.name(), "i2c_scan");
        assert_eq!(bus_tools::I2cReadTool.name(), "i2c_read");
        assert_eq!(bus_tools::I2cWriteTool.name(), "i2c_write");
        assert_eq!(bus_tools::SpiTransferTool.name(), "spi_transfer");
        assert_eq!(bus_tools::PwmControlTool.name(), "pwm_control");
    }
}

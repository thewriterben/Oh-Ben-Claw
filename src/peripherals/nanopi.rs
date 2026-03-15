//! NanoPi Neo3 GPIO peripheral — Linux sysfs interface.
//!
//! Only compiled when `peripheral-nanopi` feature is enabled and target is Linux.
//! Uses sysfs GPIO numbering: `gpio_number = 32 * bank + group * 8 + pin`
//! (e.g. GPIO0_A0 = 0, GPIO0_B0 = 8, GPIO1_A0 = 32, GPIO2_A0 = 64).
//!
//! Requires appropriate permissions on `/sys/class/gpio`.
//! Either run as root or add the user to the `gpio` group:
//! ```bash
//! sudo usermod -aG gpio $USER
//! ```

use crate::config::PeripheralBoardConfig;
use crate::peripherals::traits::Peripheral;
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

const SYSFS_GPIO: &str = "/sys/class/gpio";

/// Export a GPIO pin via sysfs (no-op if already exported).
fn export_pin(pin: u32) -> anyhow::Result<()> {
    let value_path = PathBuf::from(SYSFS_GPIO).join(format!("gpio{}", pin));
    if !value_path.exists() {
        std::fs::write(format!("{}/export", SYSFS_GPIO), pin.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to export GPIO {}: {}", pin, e))?;
    }
    Ok(())
}

/// Set pin direction (`"in"` or `"out"`).
fn set_direction(pin: u32, direction: &str) -> anyhow::Result<()> {
    let path = PathBuf::from(SYSFS_GPIO)
        .join(format!("gpio{}", pin))
        .join("direction");
    std::fs::write(&path, direction)
        .map_err(|e| anyhow::anyhow!("Failed to set direction for GPIO {}: {}", pin, e))
}

/// Read GPIO value (0 or 1) via sysfs.
fn read_value(pin: u32) -> anyhow::Result<u8> {
    export_pin(pin)?;
    set_direction(pin, "in")?;
    let path = PathBuf::from(SYSFS_GPIO)
        .join(format!("gpio{}", pin))
        .join("value");
    let s = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Failed to read GPIO {}: {}", pin, e))?;
    s.trim()
        .parse::<u8>()
        .map_err(|e| anyhow::anyhow!("Invalid GPIO value '{}' for pin {}: {}", s.trim(), pin, e))
}

/// Write GPIO value (0 or 1) via sysfs.
fn write_value(pin: u32, value: u8) -> anyhow::Result<()> {
    export_pin(pin)?;
    set_direction(pin, "out")?;
    let path = PathBuf::from(SYSFS_GPIO)
        .join(format!("gpio{}", pin))
        .join("value");
    std::fs::write(&path, value.to_string())
        .map_err(|e| anyhow::anyhow!("Failed to write GPIO {}: {}", pin, e))
}

/// NanoPi Neo3 GPIO peripheral via Linux sysfs.
pub struct NanoPiGpioPeripheral {
    board: PeripheralBoardConfig,
}

impl NanoPiGpioPeripheral {
    /// Create a new NanoPi GPIO peripheral from config.
    pub fn new(board: PeripheralBoardConfig) -> Self {
        Self { board }
    }

    /// Attempt to connect (verify sysfs GPIO is available).
    pub async fn connect_from_config(board: &PeripheralBoardConfig) -> anyhow::Result<Self> {
        let mut peripheral = Self::new(board.clone());
        peripheral.connect().await?;
        Ok(peripheral)
    }
}

#[async_trait]
impl Peripheral for NanoPiGpioPeripheral {
    fn name(&self) -> &str {
        &self.board.board
    }

    fn board_type(&self) -> &str {
        "nanopi-gpio"
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        let gpio_path = std::path::Path::new(SYSFS_GPIO);
        if !gpio_path.exists() {
            anyhow::bail!("sysfs GPIO not available at {}", SYSFS_GPIO);
        }
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        std::path::Path::new(SYSFS_GPIO).exists()
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(NanoPiGpioReadTool),
            Box::new(NanoPiGpioWriteTool),
        ]
    }
}

// ── GPIO Read Tool ────────────────────────────────────────────────────────────

struct NanoPiGpioReadTool;

#[async_trait]
impl Tool for NanoPiGpioReadTool {
    fn name(&self) -> &str {
        "gpio_read"
    }

    fn description(&self) -> &str {
        "Read the value (0 or 1) of a GPIO pin on NanoPi Neo3 via Linux sysfs. \
         Uses sysfs GPIO numbers (e.g. 0 = GPIO0_A0, 8 = GPIO0_B0, 64 = GPIO2_A0). \
         The formula is: gpio_number = 32 * bank + group * 8 + bit."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "sysfs GPIO number (gpio_number = 32*bank + group*8 + bit)"
                }
            },
            "required": ["pin"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))? as u32;

        let value = tokio::task::spawn_blocking(move || read_value(pin)).await??;

        Ok(ToolResult {
            success: true,
            output: format!("pin {} = {}", pin, value),
            error: None,
        })
    }
}

// ── GPIO Write Tool ───────────────────────────────────────────────────────────

struct NanoPiGpioWriteTool;

#[async_trait]
impl Tool for NanoPiGpioWriteTool {
    fn name(&self) -> &str {
        "gpio_write"
    }

    fn description(&self) -> &str {
        "Set a GPIO pin high (1) or low (0) on NanoPi Neo3 via Linux sysfs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "sysfs GPIO number"
                },
                "value": {
                    "type": "integer",
                    "description": "0 for low, 1 for high",
                    "enum": [0, 1]
                }
            },
            "required": ["pin", "value"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))? as u32;
        let value = args
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'value' parameter"))? as u8;

        tokio::task::spawn_blocking(move || write_value(pin, value)).await??;

        Ok(ToolResult {
            success: true,
            output: format!("pin {} = {}", pin, value),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpio_read_tool_has_correct_name() {
        let tool = NanoPiGpioReadTool;
        assert_eq!(tool.name(), "gpio_read");
    }

    #[test]
    fn gpio_write_tool_has_correct_name() {
        let tool = NanoPiGpioWriteTool;
        assert_eq!(tool.name(), "gpio_write");
    }

    #[tokio::test]
    async fn gpio_read_requires_pin_parameter() {
        let tool = NanoPiGpioReadTool;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn gpio_write_requires_pin_and_value() {
        let tool = NanoPiGpioWriteTool;
        let result = tool.execute(serde_json::json!({"pin": 64})).await;
        assert!(result.is_err());
    }
}

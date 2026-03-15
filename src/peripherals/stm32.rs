//! STM32 Nucleo peripheral driver via probe-rs and RTT (Real-Time Transfer).
//!
//! This module communicates with STM32 Nucleo boards using the ST-Link V2/V3
//! debug probe embedded on every Nucleo board. It uses the `probe-rs` CLI
//! tool to flash firmware and read/write memory, and the RTT (SEGGER Real-Time
//! Transfer) channel for high-speed bidirectional communication.
//!
//! # Communication Protocol
//! Commands are sent over RTT channel 0 as newline-delimited JSON. The STM32
//! firmware reads from the RTT down-channel and writes responses to the
//! RTT up-channel.
//!
//! # Feature Flag
//! Enabled with `--features peripheral-stm32`. Requires `probe-rs` to be
//! installed on the host: `cargo install probe-rs-tools --locked`
//!
//! # Supported Boards
//! - STM32 Nucleo-F401RE (ARM Cortex-M4, 84 MHz)
//! - STM32 Nucleo-F411RE (ARM Cortex-M4, 100 MHz)
//! - STM32 Nucleo-L476RG (ARM Cortex-M4, 80 MHz, ultra-low-power)
//! - STM32 Nucleo-H743ZI (ARM Cortex-M7, 480 MHz)
//! - STM32 Nucleo-G474RE (ARM Cortex-M4, 170 MHz, HRTIM)
//!
//! # Companion Firmware
//! See `firmware/obc-stm32/` for the STM32CubeIDE project that implements
//! the RTT command protocol.

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

use crate::{
    config::PeripheralBoardConfig,
    peripherals::traits::Peripheral,
    tools::traits::{Tool, ToolResult},
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default probe-rs target chip for Nucleo-F401RE.
const DEFAULT_CHIP: &str = "STM32F401RETx";

/// Timeout for probe-rs RTT operations.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

// ── Peripheral Struct ─────────────────────────────────────────────────────────

/// STM32 Nucleo peripheral driver.
///
/// Uses the `probe-rs` CLI for flash operations and RTT for runtime
/// communication. The ST-Link debug probe on the Nucleo board provides
/// the physical connection over USB.
pub struct Stm32NucleoPeripheral {
    board: PeripheralBoardConfig,
    chip: String,
    probe_index: u32,
}

impl Stm32NucleoPeripheral {
    /// Create a new STM32 Nucleo peripheral from config.
    pub fn new(board: PeripheralBoardConfig) -> Self {
        // Infer chip from board name
        let chip = match board.board.as_str() {
            "nucleo-f401re" => "STM32F401RETx",
            "nucleo-f411re" => "STM32F411RETx",
            "nucleo-l476rg" => "STM32L476RGTx",
            "nucleo-h743zi" => "STM32H743ZITx",
            "nucleo-g474re" => "STM32G474RETx",
            _ => DEFAULT_CHIP,
        };
        Self {
            board,
            chip: chip.to_string(),
            probe_index: 0,
        }
    }

    /// Attempt to connect (verify probe-rs is available and probe is detected).
    pub async fn connect_from_config(board: &PeripheralBoardConfig) -> anyhow::Result<Self> {
        let mut peripheral = Self::new(board.clone());
        peripheral.connect().await?;
        Ok(peripheral)
    }

    /// Check if probe-rs is installed.
    async fn probe_rs_available() -> bool {
        tokio::process::Command::new("probe-rs")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// List connected debug probes via probe-rs.
    pub async fn list_probes() -> anyhow::Result<Vec<String>> {
        let output = tokio::process::Command::new("probe-rs")
            .args(["list"])
            .output()
            .await
            .with_context(|| "Failed to run probe-rs. Install with: cargo install probe-rs-tools --locked")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect())
    }
}

#[async_trait]
impl Peripheral for Stm32NucleoPeripheral {
    fn name(&self) -> &str {
        &self.board.board
    }

    fn board_type(&self) -> &str {
        "stm32-nucleo"
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if !Self::probe_rs_available().await {
            anyhow::bail!(
                "probe-rs not found. Install with: cargo install probe-rs-tools --locked\n\
                 Then add udev rules: sudo probe-rs complete install"
            );
        }
        tracing::info!(
            board = %self.board.board,
            chip = %self.chip,
            "STM32 Nucleo peripheral connected via probe-rs"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        Self::probe_rs_available().await
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let chip = self.chip.clone();
        let probe_index = self.probe_index;
        vec![
            Box::new(Stm32FlashTool { chip: chip.clone(), probe_index }),
            Box::new(Stm32RttReadTool { chip: chip.clone(), probe_index }),
            Box::new(Stm32RttWriteTool { chip: chip.clone(), probe_index }),
            Box::new(Stm32ResetTool { chip: chip.clone(), probe_index }),
            Box::new(Stm32ListProbesTool),
            Box::new(Stm32MemReadTool { chip: chip.clone(), probe_index }),
        ]
    }
}

// ── Flash Tool ────────────────────────────────────────────────────────────────

struct Stm32FlashTool {
    chip: String,
    probe_index: u32,
}

#[async_trait]
impl Tool for Stm32FlashTool {
    fn name(&self) -> &str {
        "stm32_flash"
    }

    fn description(&self) -> &str {
        "Flash a compiled binary (.elf or .bin) to the STM32 Nucleo board via \
         the ST-Link debug probe using probe-rs. The board resets and starts \
         running the new firmware immediately after flashing."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "binary_path": {
                    "type": "string",
                    "description": "Absolute path to the compiled .elf or .bin firmware file"
                },
                "reset_after": {
                    "type": "boolean",
                    "description": "Reset the chip after flashing (default: true)"
                }
            },
            "required": ["binary_path"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let binary = args
            .get("binary_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'binary_path' parameter"))?
            .to_string();
        let reset = args
            .get("reset_after")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if !std::path::Path::new(&binary).exists() {
            anyhow::bail!("Binary file not found: {binary}");
        }

        let mut cmd = tokio::process::Command::new("probe-rs");
        cmd.args([
            "download",
            "--chip",
            &self.chip,
            "--probe-index",
            &self.probe_index.to_string(),
            &binary,
        ]);
        if reset {
            cmd.arg("--reset-halt");
        }

        let output = tokio::time::timeout(Duration::from_secs(60), cmd.output())
            .await
            .with_context(|| "Flash operation timed out after 60s")?
            .with_context(|| "Failed to run probe-rs download")?;

        if output.status.success() {
            Ok(ToolResult::ok(format!(
                "Flashed {binary} to {} successfully",
                self.chip
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Flash failed: {stderr}")
        }
    }
}

// ── RTT Read Tool ─────────────────────────────────────────────────────────────

struct Stm32RttReadTool {
    chip: String,
    probe_index: u32,
}

#[async_trait]
impl Tool for Stm32RttReadTool {
    fn name(&self) -> &str {
        "stm32_rtt_read"
    }

    fn description(&self) -> &str {
        "Read output from the STM32 firmware via RTT (SEGGER Real-Time Transfer) \
         channel 0. RTT provides high-speed, non-intrusive debug output from the \
         running firmware. Returns up to `max_lines` lines of output."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum number of RTT output lines to read (default: 20)",
                    "minimum": 1,
                    "maximum": 500
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Read timeout in milliseconds (default: 2000)",
                    "minimum": 100,
                    "maximum": 30000
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let max_lines = args
            .get("max_lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000);

        let output = tokio::time::timeout(
            Duration::from_millis(timeout_ms + 1000),
            tokio::process::Command::new("probe-rs")
                .args([
                    "rtt",
                    "--chip",
                    &self.chip,
                    "--probe-index",
                    &self.probe_index.to_string(),
                    "--timeout",
                    &timeout_ms.to_string(),
                ])
                .output(),
        )
        .await
        .with_context(|| "RTT read timed out")?
        .with_context(|| "Failed to run probe-rs rtt")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().take(max_lines as usize).collect();
        Ok(ToolResult::ok(lines.join("\n")))
    }
}

// ── RTT Write Tool ────────────────────────────────────────────────────────────

struct Stm32RttWriteTool {
    chip: String,
    probe_index: u32,
}

#[async_trait]
impl Tool for Stm32RttWriteTool {
    fn name(&self) -> &str {
        "stm32_rtt_write"
    }

    fn description(&self) -> &str {
        "Send a command to the STM32 firmware via RTT down-channel 0. \
         The firmware must implement an RTT command handler. \
         Commands are sent as newline-terminated strings."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command string to send to the firmware (e.g., a JSON command)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?
            .to_string();

        // probe-rs does not yet have a direct RTT write CLI command;
        // we use a temporary file approach via probe-rs run with stdin
        let output = tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::process::Command::new("probe-rs")
                .args([
                    "rtt",
                    "--chip",
                    &self.chip,
                    "--probe-index",
                    &self.probe_index.to_string(),
                    "--down",
                    &command,
                ])
                .output(),
        )
        .await
        .with_context(|| "RTT write timed out")?
        .with_context(|| "Failed to run probe-rs rtt write")?;

        if output.status.success() {
            Ok(ToolResult::ok(format!("Sent to RTT: {command}")))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("RTT write failed: {stderr}")
        }
    }
}

// ── Reset Tool ────────────────────────────────────────────────────────────────

struct Stm32ResetTool {
    chip: String,
    probe_index: u32,
}

#[async_trait]
impl Tool for Stm32ResetTool {
    fn name(&self) -> &str {
        "stm32_reset"
    }

    fn description(&self) -> &str {
        "Reset the STM32 Nucleo board via the ST-Link debug probe. \
         Use 'hard' for a full hardware reset, 'soft' for a software reset \
         via the NVIC SYSRESETREQ bit."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "reset_type": {
                    "type": "string",
                    "description": "Reset type: 'hard' or 'soft' (default: 'hard')",
                    "enum": ["hard", "soft"]
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let reset_type = args
            .get("reset_type")
            .and_then(|v| v.as_str())
            .unwrap_or("hard");

        let mut cmd_args = vec![
            "reset".to_string(),
            "--chip".to_string(),
            self.chip.clone(),
            "--probe-index".to_string(),
            self.probe_index.to_string(),
        ];
        if reset_type == "soft" {
            cmd_args.push("--soft".to_string());
        }

        let output = tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::process::Command::new("probe-rs")
                .args(&cmd_args)
                .output(),
        )
        .await
        .with_context(|| "Reset timed out")?
        .with_context(|| "Failed to run probe-rs reset")?;

        if output.status.success() {
            Ok(ToolResult::ok(format!(
                "{} reset applied to {}",
                reset_type, self.chip
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Reset failed: {stderr}")
        }
    }
}

// ── List Probes Tool ──────────────────────────────────────────────────────────

struct Stm32ListProbesTool;

#[async_trait]
impl Tool for Stm32ListProbesTool {
    fn name(&self) -> &str {
        "stm32_list_probes"
    }

    fn description(&self) -> &str {
        "List all connected debug probes (ST-Link, J-Link, CMSIS-DAP) detected \
         by probe-rs. Use this to find the probe index when multiple boards \
         are connected simultaneously."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let probes = Stm32NucleoPeripheral::list_probes().await?;
        if probes.is_empty() {
            Ok(ToolResult::ok("No debug probes detected. Is the Nucleo board connected via USB?".into()))
        } else {
            Ok(ToolResult::ok(format!(
                "Detected {} probe(s):\n{}",
                probes.len(),
                probes.join("\n")
            )))
        }
    }
}

// ── Memory Read Tool ──────────────────────────────────────────────────────────

struct Stm32MemReadTool {
    chip: String,
    probe_index: u32,
}

#[async_trait]
impl Tool for Stm32MemReadTool {
    fn name(&self) -> &str {
        "stm32_mem_read"
    }

    fn description(&self) -> &str {
        "Read bytes from the STM32 memory map via the debug probe. \
         Useful for reading peripheral registers, SRAM variables, or flash \
         contents. Address must be a valid address in the STM32 memory map \
         (e.g., 0x40000000 for peripheral base, 0x20000000 for SRAM)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "string",
                    "description": "Memory address in hex (e.g., '0x40021000' for RCC base on F4)"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of 32-bit words to read (default: 4)",
                    "minimum": 1,
                    "maximum": 256
                }
            },
            "required": ["address"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let address = args
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'address' parameter"))?
            .to_string();
        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(4);

        let output = tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::process::Command::new("probe-rs")
                .args([
                    "read",
                    "--chip",
                    &self.chip,
                    "--probe-index",
                    &self.probe_index.to_string(),
                    "w32",
                    &address,
                    &count.to_string(),
                ])
                .output(),
        )
        .await
        .with_context(|| "Memory read timed out")?
        .with_context(|| "Failed to run probe-rs read")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(ToolResult::ok(format!(
                "Memory @ {address} ({count} words):\n{stdout}"
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Memory read failed: {stderr}")
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_inferred_from_board_name() {
        let board = PeripheralBoardConfig {
            board: "nucleo-f401re".into(),
            transport: "native".into(),
            path: None,
            baud: 0,
            node_id: None,
        };
        let p = Stm32NucleoPeripheral::new(board);
        assert_eq!(p.chip, "STM32F401RETx");
    }

    #[test]
    fn chip_inferred_for_h743() {
        let board = PeripheralBoardConfig {
            board: "nucleo-h743zi".into(),
            transport: "native".into(),
            path: None,
            baud: 0,
            node_id: None,
        };
        let p = Stm32NucleoPeripheral::new(board);
        assert_eq!(p.chip, "STM32H743ZITx");
    }

    #[test]
    fn flash_tool_name() {
        let t = Stm32FlashTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        assert_eq!(t.name(), "stm32_flash");
    }

    #[test]
    fn rtt_read_tool_name() {
        let t = Stm32RttReadTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        assert_eq!(t.name(), "stm32_rtt_read");
    }

    #[test]
    fn rtt_write_tool_name() {
        let t = Stm32RttWriteTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        assert_eq!(t.name(), "stm32_rtt_write");
    }

    #[test]
    fn reset_tool_name() {
        let t = Stm32ResetTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        assert_eq!(t.name(), "stm32_reset");
    }

    #[test]
    fn list_probes_tool_name() {
        assert_eq!(Stm32ListProbesTool.name(), "stm32_list_probes");
    }

    #[test]
    fn mem_read_tool_name() {
        let t = Stm32MemReadTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        assert_eq!(t.name(), "stm32_mem_read");
    }

    #[tokio::test]
    async fn flash_requires_binary_path() {
        let t = Stm32FlashTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn flash_rejects_missing_file() {
        let t = Stm32FlashTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        let result = t.execute(json!({"binary_path": "/nonexistent/firmware.elf"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mem_read_requires_address() {
        let t = Stm32MemReadTool { chip: "STM32F401RETx".into(), probe_index: 0 };
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
    }
}

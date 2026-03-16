//! I2C bus scan, SPI transfer, and PWM control tools for Linux SBC peripherals.
//!
//! These tools operate on Linux SBCs (Raspberry Pi, NanoPi Neo3, etc.) using
//! the standard Linux kernel interfaces:
//! - **I2C**: `/dev/i2c-N` via the `i2cdetect` utility and direct ioctl
//! - **SPI**: `/dev/spidevN.M` via the Linux SPI userspace API
//! - **PWM**: `/sys/class/pwm/` sysfs interface
//!
//! All tools work on any Linux board that exposes these kernel interfaces,
//! making them reusable across Raspberry Pi, NanoPi Neo3, and other SBCs.
//!
//! # Prerequisites
//! - I2C: `sudo apt-get install -y i2c-tools` and enable I2C in raspi-config/board config
//! - SPI: Enable SPI in raspi-config/board config
//! - PWM: Enable PWM overlay in /boot/config.txt or equivalent

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

use crate::tools::traits::{Tool, ToolResult};

// ── I2C Bus Scan Tool ─────────────────────────────────────────────────────────

/// Scan an I2C bus for connected devices using `i2cdetect`.
pub struct I2cScanTool;

#[async_trait]
impl Tool for I2cScanTool {
    fn name(&self) -> &str {
        "i2c_scan"
    }

    fn description(&self) -> &str {
        "Scan an I2C bus for connected devices and return their 7-bit addresses. \
         Uses the Linux i2cdetect utility. Common I2C devices: \
         0x3C = SSD1306 OLED display, 0x48 = ADS1115 ADC, \
         0x68 = MPU-6050 IMU, 0x76 = BME280 sensor, 0x27 = PCF8574 I/O expander."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number (e.g., 1 for /dev/i2c-1 on Raspberry Pi, 0 for NanoPi Neo3)",
                    "minimum": 0,
                    "maximum": 9
                },
                "start_addr": {
                    "type": "integer",
                    "description": "Start address for scan range (default: 0x03)",
                    "minimum": 3,
                    "maximum": 119
                },
                "end_addr": {
                    "type": "integer",
                    "description": "End address for scan range (default: 0x77)",
                    "minimum": 3,
                    "maximum": 119
                }
            },
            "required": ["bus"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let bus = args
            .get("bus")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'bus' parameter"))?;
        let start = args
            .get("start_addr")
            .and_then(|v| v.as_u64())
            .unwrap_or(0x03);
        let end = args
            .get("end_addr")
            .and_then(|v| v.as_u64())
            .unwrap_or(0x77);

        // Verify the bus device exists
        let dev_path = format!("/dev/i2c-{bus}");
        if !std::path::Path::new(&dev_path).exists() {
            anyhow::bail!(
                "I2C bus {bus} not found ({dev_path}). \
                 Enable I2C in raspi-config or board config, then reboot."
            );
        }

        let output = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new("i2cdetect")
                .args([
                    "-y",
                    "-r",
                    &bus.to_string(),
                    &format!("{start:#04x}"),
                    &format!("{end:#04x}"),
                ])
                .output(),
        )
        .await
        .with_context(|| "i2cdetect timed out")?
        .with_context(|| {
            "Failed to run i2cdetect. Install with: sudo apt-get install -y i2c-tools"
        })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

            // Parse detected addresses from i2cdetect output
            let mut detected = Vec::new();
            for line in stdout.lines().skip(1) {
                // Lines look like: "20: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- --"
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }
                let row_start =
                    u64::from_str_radix(parts[0].trim_end_matches(':'), 16).unwrap_or(0);
                for (i, token) in parts.iter().skip(1).enumerate() {
                    if *token != "--" && *token != "UU" {
                        let addr = row_start + i as u64;
                        detected.push(format!("0x{addr:02X}"));
                    }
                }
            }

            if detected.is_empty() {
                Ok(ToolResult::ok(format!(
                    "I2C bus {bus}: no devices detected in range {start:#04x}–{end:#04x}"
                )))
            } else {
                Ok(ToolResult::ok(format!(
                    "I2C bus {bus}: {} device(s) found: {}\n\n{}",
                    detected.len(),
                    detected.join(", "),
                    stdout
                )))
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("i2cdetect failed: {stderr}")
        }
    }
}

// ── I2C Read Register Tool ────────────────────────────────────────────────────

pub struct I2cReadTool;

#[async_trait]
impl Tool for I2cReadTool {
    fn name(&self) -> &str {
        "i2c_read"
    }

    fn description(&self) -> &str {
        "Read one or more bytes from an I2C device register using i2cget. \
         Useful for reading sensor data, configuration registers, and status bytes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number (e.g., 1 for /dev/i2c-1)",
                    "minimum": 0,
                    "maximum": 9
                },
                "address": {
                    "type": "string",
                    "description": "Device 7-bit address in hex (e.g., '0x48')"
                },
                "register": {
                    "type": "string",
                    "description": "Register address in hex (e.g., '0x00')"
                },
                "mode": {
                    "type": "string",
                    "description": "Read mode: 'b' (byte), 'w' (word/16-bit), 'i' (I2C block) (default: 'b')",
                    "enum": ["b", "w", "i"]
                }
            },
            "required": ["bus", "address", "register"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let bus = args
            .get("bus")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'bus' parameter"))?;
        let address = args
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'address' parameter"))?
            .to_string();
        let register = args
            .get("register")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'register' parameter"))?
            .to_string();
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("b")
            .to_string();

        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("i2cget")
                .args(["-y", &bus.to_string(), &address, &register, &mode])
                .output(),
        )
        .await
        .with_context(|| "i2cget timed out")?
        .with_context(|| "Failed to run i2cget")?;

        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(ToolResult::ok(format!(
                "I2C bus {bus} device {address} reg {register} = {value}"
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("i2cget failed: {stderr}")
        }
    }
}

// ── I2C Write Register Tool ───────────────────────────────────────────────────

pub struct I2cWriteTool;

#[async_trait]
impl Tool for I2cWriteTool {
    fn name(&self) -> &str {
        "i2c_write"
    }

    fn description(&self) -> &str {
        "Write a byte to an I2C device register using i2cset. \
         Useful for configuring sensors, setting output values, and controlling \
         I2C peripherals like OLED displays and motor drivers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number",
                    "minimum": 0,
                    "maximum": 9
                },
                "address": {
                    "type": "string",
                    "description": "Device 7-bit address in hex (e.g., '0x3C')"
                },
                "register": {
                    "type": "string",
                    "description": "Register address in hex (e.g., '0x00')"
                },
                "value": {
                    "type": "string",
                    "description": "Value to write in hex (e.g., '0xFF')"
                }
            },
            "required": ["bus", "address", "register", "value"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let bus = args
            .get("bus")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'bus' parameter"))?;
        let address = args
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'address' parameter"))?
            .to_string();
        let register = args
            .get("register")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'register' parameter"))?
            .to_string();
        let value = args
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'value' parameter"))?
            .to_string();

        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("i2cset")
                .args(["-y", &bus.to_string(), &address, &register, &value])
                .output(),
        )
        .await
        .with_context(|| "i2cset timed out")?
        .with_context(|| "Failed to run i2cset")?;

        if output.status.success() {
            Ok(ToolResult::ok(format!(
                "I2C bus {bus} device {address} reg {register} = {value} (written)"
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("i2cset failed: {stderr}")
        }
    }
}

// ── SPI Transfer Tool ─────────────────────────────────────────────────────────

pub struct SpiTransferTool;

#[async_trait]
impl Tool for SpiTransferTool {
    fn name(&self) -> &str {
        "spi_transfer"
    }

    fn description(&self) -> &str {
        "Perform a full-duplex SPI transfer via the Linux spidev interface (/dev/spidevN.M). \
         Sends the given bytes and returns the received bytes. \
         Commonly used for SPI sensors (MAX31855 thermocouple, MCP3008 ADC, etc.)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "device": {
                    "type": "string",
                    "description": "SPI device path (e.g., '/dev/spidev0.0' for bus 0, CS 0)"
                },
                "bytes": {
                    "type": "array",
                    "description": "Bytes to send as an array of integers (0–255)",
                    "items": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 255
                    },
                    "minItems": 1,
                    "maxItems": 64
                },
                "speed_hz": {
                    "type": "integer",
                    "description": "SPI clock speed in Hz (default: 1000000 = 1 MHz)",
                    "minimum": 1000,
                    "maximum": 50000000
                },
                "mode": {
                    "type": "integer",
                    "description": "SPI mode (0–3, default: 0). Mode 0: CPOL=0 CPHA=0",
                    "enum": [0, 1, 2, 3]
                }
            },
            "required": ["device", "bytes"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let device = args
            .get("device")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'device' parameter"))?
            .to_string();
        let bytes_val = args
            .get("bytes")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing 'bytes' parameter"))?;
        let speed_hz = args
            .get("speed_hz")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_000_000);
        let mode = args.get("mode").and_then(|v| v.as_u64()).unwrap_or(0) as u8;

        let tx_bytes: Vec<u8> = bytes_val
            .iter()
            .filter_map(|v| v.as_u64().map(|b| b as u8))
            .collect();

        if !std::path::Path::new(&device).exists() {
            anyhow::bail!(
                "SPI device {device} not found. Enable SPI in raspi-config or board config."
            );
        }

        // Use spidev-test if available, otherwise use a Python one-liner
        let tx_hex: Vec<String> = tx_bytes.iter().map(|b| format!("{b:02X}")).collect();
        let tx_str = tx_hex.join(" ");

        let python_script = format!(
            r#"
import spidev, sys
spi = spidev.SpiDev()
bus, dev = {device_parts}
spi.open(bus, dev)
spi.max_speed_hz = {speed_hz}
spi.mode = {mode}
tx = [{tx_bytes}]
rx = spi.xfer2(tx)
spi.close()
print(' '.join(f'{{b:02X}}' for b in rx))
"#,
            device_parts = {
                // Parse /dev/spidevN.M
                let parts: Vec<&str> = device
                    .trim_start_matches("/dev/spidev")
                    .split('.')
                    .collect();
                if parts.len() == 2 {
                    format!("{}, {}", parts[0], parts[1])
                } else {
                    "0, 0".to_string()
                }
            },
            speed_hz = speed_hz,
            mode = mode,
            tx_bytes = tx_bytes
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("python3")
                .args(["-c", &python_script])
                .output(),
        )
        .await
        .with_context(|| "SPI transfer timed out")?
        .with_context(|| "Failed to run SPI transfer")?;

        if output.status.success() {
            let rx_hex = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(ToolResult::ok(format!(
                "SPI {device} @ {speed_hz}Hz mode{mode}\nTX: {tx_str}\nRX: {rx_hex}"
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "SPI transfer failed: {stderr}\n\
                 Ensure spidev Python module is installed: pip3 install spidev"
            )
        }
    }
}

// ── PWM Control Tool ──────────────────────────────────────────────────────────

pub struct PwmControlTool;

#[async_trait]
impl Tool for PwmControlTool {
    fn name(&self) -> &str {
        "pwm_control"
    }

    fn description(&self) -> &str {
        "Control a hardware PWM channel via the Linux sysfs PWM interface \
         (/sys/class/pwm/). Supports setting frequency, duty cycle, and \
         enable/disable. Works on Raspberry Pi (pwmchip0), NanoPi Neo3 \
         (pwmchip1), and other Linux SBCs with PWM kernel drivers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "chip": {
                    "type": "integer",
                    "description": "PWM chip number (e.g., 0 for /sys/class/pwm/pwmchip0)",
                    "minimum": 0,
                    "maximum": 9
                },
                "channel": {
                    "type": "integer",
                    "description": "PWM channel on the chip (usually 0 or 1)",
                    "minimum": 0,
                    "maximum": 7
                },
                "frequency_hz": {
                    "type": "number",
                    "description": "PWM frequency in Hz (e.g., 50 for servo, 1000 for LED dimming)",
                    "minimum": 1.0,
                    "maximum": 100000.0
                },
                "duty_cycle_pct": {
                    "type": "number",
                    "description": "Duty cycle as a percentage (0.0–100.0)",
                    "minimum": 0.0,
                    "maximum": 100.0
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Enable (true) or disable (false) the PWM output (default: true)"
                }
            },
            "required": ["chip", "channel", "frequency_hz", "duty_cycle_pct"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let chip = args
            .get("chip")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'chip' parameter"))?;
        let channel = args
            .get("channel")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'channel' parameter"))?;
        let freq_hz = args
            .get("frequency_hz")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'frequency_hz' parameter"))?;
        let duty_pct = args
            .get("duty_cycle_pct")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'duty_cycle_pct' parameter"))?;
        let enabled = args
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let period_ns = (1_000_000_000.0 / freq_hz) as u64;
        let duty_ns = ((duty_pct / 100.0) * period_ns as f64) as u64;
        let pwm_base = format!("/sys/class/pwm/pwmchip{chip}/pwm{channel}");
        let chip_path = format!("/sys/class/pwm/pwmchip{chip}");

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if !std::path::Path::new(&chip_path).exists() {
                anyhow::bail!(
                    "PWM chip {chip} not found at {chip_path}. \
                     Enable PWM in board config (e.g., dtoverlay=pwm on Raspberry Pi)."
                );
            }

            // Export the channel if not already exported
            if !std::path::Path::new(&pwm_base).exists() {
                std::fs::write(format!("{chip_path}/export"), channel.to_string())
                    .with_context(|| format!("Failed to export PWM channel {channel}"))?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Disable before changing period to avoid invalid state
            let _ = std::fs::write(format!("{pwm_base}/enable"), "0");

            std::fs::write(format!("{pwm_base}/period"), period_ns.to_string())
                .with_context(|| "Failed to set PWM period")?;
            std::fs::write(format!("{pwm_base}/duty_cycle"), duty_ns.to_string())
                .with_context(|| "Failed to set PWM duty cycle")?;

            if enabled {
                std::fs::write(format!("{pwm_base}/enable"), "1")
                    .with_context(|| "Failed to enable PWM")?;
            }
            Ok(())
        })
        .await??;

        Ok(ToolResult::ok(format!(
            "PWM chip{chip}/pwm{channel}: {freq_hz:.1}Hz, {duty_pct:.1}% duty ({duty_ns}ns/{period_ns}ns), {}",
            if enabled { "enabled" } else { "disabled" }
        )))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i2c_scan_tool_name() {
        assert_eq!(I2cScanTool.name(), "i2c_scan");
    }

    #[test]
    fn i2c_read_tool_name() {
        assert_eq!(I2cReadTool.name(), "i2c_read");
    }

    #[test]
    fn i2c_write_tool_name() {
        assert_eq!(I2cWriteTool.name(), "i2c_write");
    }

    #[test]
    fn spi_transfer_tool_name() {
        assert_eq!(SpiTransferTool.name(), "spi_transfer");
    }

    #[test]
    fn pwm_control_tool_name() {
        assert_eq!(PwmControlTool.name(), "pwm_control");
    }

    #[tokio::test]
    async fn i2c_scan_requires_bus() {
        let result = I2cScanTool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn i2c_read_requires_all_params() {
        let result = I2cReadTool
            .execute(json!({"bus": 1, "address": "0x48"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn i2c_write_requires_all_params() {
        let result = I2cWriteTool
            .execute(json!({"bus": 1, "address": "0x3C", "register": "0x00"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn spi_requires_device_and_bytes() {
        let result = SpiTransferTool
            .execute(json!({"device": "/dev/spidev0.0"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn pwm_requires_all_params() {
        let result = PwmControlTool
            .execute(json!({"chip": 0, "channel": 0, "frequency_hz": 50.0}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn i2c_scan_schema_has_bus_property() {
        let schema = I2cScanTool.parameters_schema();
        assert!(schema["properties"]["bus"].is_object());
    }

    #[test]
    fn spi_schema_has_bytes_array() {
        let schema = SpiTransferTool.parameters_schema();
        assert_eq!(schema["properties"]["bytes"]["type"], "array");
    }

    #[test]
    fn pwm_schema_has_frequency() {
        let schema = PwmControlTool.parameters_schema();
        assert!(schema["properties"]["frequency_hz"].is_object());
    }
}

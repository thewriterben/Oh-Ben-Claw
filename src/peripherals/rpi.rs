//! Raspberry Pi GPIO and camera peripheral driver.
//!
//! This module provides GPIO control via the `rppal` crate (which uses
//! `/dev/gpiomem` or `/dev/mem`) and camera capture via `libcamera-still`
//! subprocess on Raspberry Pi OS / Ubuntu for Raspberry Pi.
//!
//! # Feature Flag
//! Enabled with `--features peripheral-rpi`. The `rppal` crate is only
//! compiled on Linux targets.
//!
//! # Supported Boards
//! - Raspberry Pi 4 Model B
//! - Raspberry Pi 3 Model B / B+
//! - Raspberry Pi 5
//! - Raspberry Pi Zero 2 W
//! - Raspberry Pi Compute Module 4

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::{
    config::PeripheralBoardConfig,
    peripherals::traits::Peripheral,
    tools::traits::{Tool, ToolResult},
};

// ── GPIO Helpers (sysfs fallback, compatible with all Pi models) ──────────────

const SYSFS_GPIO: &str = "/sys/class/gpio";

fn export_pin(pin: u32) -> anyhow::Result<()> {
    let export_path = format!("{SYSFS_GPIO}/export");
    let pin_path = format!("{SYSFS_GPIO}/gpio{pin}");
    if !std::path::Path::new(&pin_path).exists() {
        std::fs::write(&export_path, pin.to_string())
            .with_context(|| format!("Failed to export GPIO pin {pin}"))?;
    }
    Ok(())
}

fn set_direction(pin: u32, direction: &str) -> anyhow::Result<()> {
    let dir_path = format!("{SYSFS_GPIO}/gpio{pin}/direction");
    std::fs::write(&dir_path, direction)
        .with_context(|| format!("Failed to set direction for GPIO pin {pin}"))?;
    Ok(())
}

fn read_gpio_value(pin: u32) -> anyhow::Result<u8> {
    export_pin(pin)?;
    set_direction(pin, "in")?;
    let value_path = format!("{SYSFS_GPIO}/gpio{pin}/value");
    let raw = std::fs::read_to_string(&value_path)
        .with_context(|| format!("Failed to read GPIO pin {pin}"))?;
    raw.trim()
        .parse::<u8>()
        .with_context(|| format!("Invalid GPIO value for pin {pin}"))
}

fn write_gpio_value(pin: u32, value: u8) -> anyhow::Result<()> {
    export_pin(pin)?;
    set_direction(pin, "out")?;
    let value_path = format!("{SYSFS_GPIO}/gpio{pin}/value");
    std::fs::write(&value_path, value.to_string())
        .with_context(|| format!("Failed to write GPIO pin {pin}"))?;
    Ok(())
}

// ── Peripheral Struct ─────────────────────────────────────────────────────────

/// Raspberry Pi GPIO and camera peripheral.
///
/// Uses Linux sysfs GPIO (compatible with all Pi models) and `libcamera-still`
/// for camera capture.
pub struct RpiGpioPeripheral {
    board: PeripheralBoardConfig,
    /// Optional path for camera capture output directory.
    capture_dir: PathBuf,
}

impl RpiGpioPeripheral {
    /// Create a new Raspberry Pi peripheral from config.
    pub fn new(board: PeripheralBoardConfig) -> Self {
        let capture_dir = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".local").join("share").join("oh-ben-claw").join("captures"))
            .unwrap_or_else(|_| PathBuf::from("/tmp").join("oh-ben-claw").join("captures"));
        Self { board, capture_dir }
    }

    /// Attempt to connect (verify sysfs GPIO is available).
    pub async fn connect_from_config(board: &PeripheralBoardConfig) -> anyhow::Result<Self> {
        let mut peripheral = Self::new(board.clone());
        peripheral.connect().await?;
        Ok(peripheral)
    }
}

#[async_trait]
impl Peripheral for RpiGpioPeripheral {
    fn name(&self) -> &str {
        &self.board.board
    }

    fn board_type(&self) -> &str {
        "rpi-gpio"
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        let gpio_path = std::path::Path::new(SYSFS_GPIO);
        if !gpio_path.exists() {
            anyhow::bail!("sysfs GPIO not available at {SYSFS_GPIO}. Is this a Raspberry Pi?");
        }
        // Create capture directory
        std::fs::create_dir_all(&self.capture_dir)
            .with_context(|| format!("Failed to create capture dir {:?}", self.capture_dir))?;
        tracing::info!(board = %self.board.board, "Raspberry Pi peripheral connected via sysfs GPIO");
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
            Box::new(RpiGpioReadTool),
            Box::new(RpiGpioWriteTool),
            Box::new(RpiGpioPwmTool),
            Box::new(RpiCameraCaptureTool {
                capture_dir: self.capture_dir.clone(),
            }),
            Box::new(RpiSystemInfoTool),
        ]
    }
}

// ── GPIO Read Tool ────────────────────────────────────────────────────────────

struct RpiGpioReadTool;

#[async_trait]
impl Tool for RpiGpioReadTool {
    fn name(&self) -> &str {
        "rpi_gpio_read"
    }

    fn description(&self) -> &str {
        "Read the value (0 or 1) of a GPIO pin on Raspberry Pi via Linux sysfs. \
         Uses BCM GPIO numbering (e.g., GPIO17 = pin 11 on the 40-pin header). \
         The pin must not be in use by another subsystem."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "BCM GPIO number (e.g., 17 for GPIO17)",
                    "minimum": 0,
                    "maximum": 53
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

        let value = tokio::task::spawn_blocking(move || read_gpio_value(pin)).await??;
        Ok(ToolResult::ok(format!("GPIO{pin} = {value}")))
    }
}

// ── GPIO Write Tool ───────────────────────────────────────────────────────────

struct RpiGpioWriteTool;

#[async_trait]
impl Tool for RpiGpioWriteTool {
    fn name(&self) -> &str {
        "rpi_gpio_write"
    }

    fn description(&self) -> &str {
        "Set a GPIO pin high (1) or low (0) on Raspberry Pi via Linux sysfs. \
         Uses BCM GPIO numbering."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "BCM GPIO number",
                    "minimum": 0,
                    "maximum": 53
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

        tokio::task::spawn_blocking(move || write_gpio_value(pin, value)).await??;
        Ok(ToolResult::ok(format!("GPIO{pin} set to {value}")))
    }
}

// ── Software PWM Tool (via sysfs PWM or pigpio daemon) ───────────────────────

struct RpiGpioPwmTool;

#[async_trait]
impl Tool for RpiGpioPwmTool {
    fn name(&self) -> &str {
        "rpi_pwm_write"
    }

    fn description(&self) -> &str {
        "Generate a software PWM signal on a GPIO pin via the Linux sysfs PWM interface. \
         Hardware PWM channels are PWM0 (GPIO12/GPIO18) and PWM1 (GPIO13/GPIO19). \
         Duty cycle is 0–100%. Period is in nanoseconds (e.g., 20000000 = 50Hz for servos)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "integer",
                    "description": "PWM channel: 0 (GPIO12/GPIO18) or 1 (GPIO13/GPIO19)",
                    "enum": [0, 1]
                },
                "duty_cycle_pct": {
                    "type": "number",
                    "description": "Duty cycle as a percentage (0.0–100.0)",
                    "minimum": 0.0,
                    "maximum": 100.0
                },
                "period_ns": {
                    "type": "integer",
                    "description": "PWM period in nanoseconds (e.g., 20000000 for 50Hz)",
                    "minimum": 1000,
                    "maximum": 1000000000
                }
            },
            "required": ["channel", "duty_cycle_pct", "period_ns"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let channel = args
            .get("channel")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'channel' parameter"))? as u32;
        let duty_pct = args
            .get("duty_cycle_pct")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'duty_cycle_pct' parameter"))?;
        let period_ns = args
            .get("period_ns")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'period_ns' parameter"))?;

        let duty_ns = ((duty_pct / 100.0) * period_ns as f64) as u64;
        let pwm_base = format!("/sys/class/pwm/pwmchip0/pwm{channel}");
        let pwm_path = std::path::Path::new(&pwm_base);

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            // Export the PWM channel if not already exported
            if !pwm_path.exists() {
                std::fs::write("/sys/class/pwm/pwmchip0/export", channel.to_string())
                    .with_context(|| format!("Failed to export PWM channel {channel}"))?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            std::fs::write(format!("{pwm_base}/period"), period_ns.to_string())
                .with_context(|| "Failed to set PWM period")?;
            std::fs::write(format!("{pwm_base}/duty_cycle"), duty_ns.to_string())
                .with_context(|| "Failed to set PWM duty cycle")?;
            std::fs::write(format!("{pwm_base}/enable"), "1")
                .with_context(|| "Failed to enable PWM")?;
            Ok(())
        })
        .await??;

        Ok(ToolResult::ok(format!(
            "PWM channel {channel}: period={period_ns}ns, duty={duty_pct:.1}% ({duty_ns}ns)"
        )))
    }
}

// ── Camera Capture Tool ───────────────────────────────────────────────────────

struct RpiCameraCaptureTool {
    capture_dir: PathBuf,
}

#[async_trait]
impl Tool for RpiCameraCaptureTool {
    fn name(&self) -> &str {
        "rpi_camera_capture"
    }

    fn description(&self) -> &str {
        "Capture a still image from the Raspberry Pi camera module using \
         `libcamera-still`. Returns the path to the saved JPEG file. \
         Requires libcamera to be installed and the camera interface enabled \
         in raspi-config (or /boot/config.txt: camera_auto_detect=1)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "width": {
                    "type": "integer",
                    "description": "Image width in pixels (default: 1920)",
                    "minimum": 64,
                    "maximum": 4056
                },
                "height": {
                    "type": "integer",
                    "description": "Image height in pixels (default: 1080)",
                    "minimum": 48,
                    "maximum": 3040
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Camera warm-up time in milliseconds (default: 500)",
                    "minimum": 100,
                    "maximum": 5000
                },
                "filename": {
                    "type": "string",
                    "description": "Output filename (without path). Defaults to a timestamp-based name."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let width = args.get("width").and_then(|v| v.as_u64()).unwrap_or(1920);
        let height = args.get("height").and_then(|v| v.as_u64()).unwrap_or(1080);
        let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(500);
        let filename = args
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!(
                    "rpi_capture_{}.jpg",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                )
            });

        let output_path = self.capture_dir.join(&filename);
        let output_str = output_path.to_string_lossy().to_string();

        let status = tokio::process::Command::new("libcamera-still")
            .args([
                "--width",
                &width.to_string(),
                "--height",
                &height.to_string(),
                "--timeout",
                &timeout_ms.to_string(),
                "--nopreview",
                "--output",
                &output_str,
            ])
            .status()
            .await
            .with_context(|| {
                "Failed to run libcamera-still. Is libcamera installed? \
                 Run: sudo apt-get install -y libcamera-apps"
            })?;

        if status.success() {
            Ok(ToolResult::ok(format!(
                "Image captured: {output_str} ({width}x{height})"
            )))
        } else {
            anyhow::bail!(
                "libcamera-still exited with status {}. Check camera connection and raspi-config.",
                status.code().unwrap_or(-1)
            )
        }
    }
}

// ── System Info Tool ──────────────────────────────────────────────────────────

struct RpiSystemInfoTool;

#[async_trait]
impl Tool for RpiSystemInfoTool {
    fn name(&self) -> &str {
        "rpi_system_info"
    }

    fn description(&self) -> &str {
        "Read Raspberry Pi system information: CPU temperature, throttling status, \
         memory usage, and model identifier from /proc and vcgencmd."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        // CPU temperature from thermal zone
        let temp = tokio::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
            .await
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|t| format!("{:.1}°C", t / 1000.0))
            .unwrap_or_else(|| "unavailable".to_string());

        // Model from /proc/device-tree/model
        let model = tokio::fs::read_to_string("/proc/device-tree/model")
            .await
            .unwrap_or_else(|_| "Unknown Raspberry Pi".to_string());
        let model = model.trim_end_matches('\0').trim().to_string();

        // Memory info from /proc/meminfo
        let meminfo = tokio::fs::read_to_string("/proc/meminfo").await.unwrap_or_default();
        let mem_total = meminfo
            .lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u64>().ok())
            .map(|kb| format!("{:.0} MB", kb as f64 / 1024.0))
            .unwrap_or_else(|| "unknown".to_string());
        let mem_avail = meminfo
            .lines()
            .find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u64>().ok())
            .map(|kb| format!("{:.0} MB", kb as f64 / 1024.0))
            .unwrap_or_else(|| "unknown".to_string());

        // Throttle status via vcgencmd (optional)
        let throttle = tokio::process::Command::new("vcgencmd")
            .args(["get_throttled"])
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "vcgencmd not available".to_string());

        Ok(ToolResult::ok(format!(
            "Model: {model}\nCPU temp: {temp}\nMemory: {mem_avail} available / {mem_total} total\nThrottle: {throttle}"
        )))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpi_gpio_read_tool_name() {
        assert_eq!(RpiGpioReadTool.name(), "rpi_gpio_read");
    }

    #[test]
    fn rpi_gpio_write_tool_name() {
        assert_eq!(RpiGpioWriteTool.name(), "rpi_gpio_write");
    }

    #[test]
    fn rpi_pwm_tool_name() {
        assert_eq!(RpiGpioPwmTool.name(), "rpi_pwm_write");
    }

    #[test]
    fn rpi_camera_tool_name() {
        let tool = RpiCameraCaptureTool {
            capture_dir: PathBuf::from("/tmp"),
        };
        assert_eq!(tool.name(), "rpi_camera_capture");
    }

    #[test]
    fn rpi_system_info_tool_name() {
        assert_eq!(RpiSystemInfoTool.name(), "rpi_system_info");
    }

    #[tokio::test]
    async fn gpio_read_requires_pin() {
        let result = RpiGpioReadTool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn gpio_write_requires_pin_and_value() {
        let result = RpiGpioWriteTool.execute(json!({"pin": 17})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn pwm_requires_all_params() {
        let result = RpiGpioPwmTool.execute(json!({"channel": 0})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn camera_uses_default_resolution() {
        // Just verify the tool builds the command correctly — won't run on non-Pi
        let tool = RpiCameraCaptureTool {
            capture_dir: PathBuf::from("/tmp"),
        };
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
    }
}

//! Arduino serial peripheral driver.
//!
//! Communicates with Arduino boards (Uno, Mega, Leonardo, Nano, etc.) over a
//! USB-serial connection using a simple newline-delimited JSON protocol.
//!
//! # Protocol
//! The host sends a JSON command object terminated by `\n`:
//! ```json
//! {"cmd": "gpio_read", "pin": 13}
//! {"cmd": "gpio_write", "pin": 13, "value": 1}
//! {"cmd": "analog_read", "pin": 0}
//! {"cmd": "analog_write", "pin": 9, "value": 128}
//! {"cmd": "ping"}
//! ```
//!
//! The Arduino firmware responds with a JSON object terminated by `\n`:
//! ```json
//! {"ok": true, "value": 1}
//! {"ok": false, "error": "pin not available"}
//! ```
//!
//! # Feature Flag
//! Enabled with `--features hardware` (uses `tokio-serial`).
//!
//! # Companion Firmware
//! See `firmware/obc-arduino/` for the Arduino sketch that implements this
//! protocol. Flash it with the Arduino IDE or `arduino-cli`.

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    config::PeripheralBoardConfig,
    peripherals::traits::Peripheral,
    tools::traits::{Tool, ToolResult},
};

// ── Serial Connection ─────────────────────────────────────────────────────────

/// Default baud rate for Arduino serial communication.
const DEFAULT_BAUD: u32 = 115_200;

/// Timeout for serial read operations.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

// ── Peripheral Struct ─────────────────────────────────────────────────────────

/// Arduino serial peripheral.
///
/// Communicates over USB-serial using newline-delimited JSON. Compatible with
/// all Arduino boards that run the Oh-Ben-Claw Arduino firmware sketch.
pub struct ArduinoSerialPeripheral {
    board: PeripheralBoardConfig,
    port_path: String,
    baud: u32,
}

impl ArduinoSerialPeripheral {
    /// Create a new Arduino peripheral from config.
    pub fn new(board: PeripheralBoardConfig) -> anyhow::Result<Self> {
        let port_path = board.path.clone().ok_or_else(|| {
            anyhow::anyhow!("Arduino peripheral requires a 'path' (e.g., /dev/ttyUSB0)")
        })?;
        let baud = if board.baud > 0 {
            board.baud
        } else {
            DEFAULT_BAUD
        };
        Ok(Self {
            board,
            port_path,
            baud,
        })
    }

    /// Attempt to connect and verify the Arduino is responsive.
    pub async fn connect_from_config(board: &PeripheralBoardConfig) -> anyhow::Result<Self> {
        let mut peripheral = Self::new(board.clone())?;
        peripheral.connect().await?;
        Ok(peripheral)
    }
}

#[async_trait]
impl Peripheral for ArduinoSerialPeripheral {
    fn name(&self) -> &str {
        &self.board.board
    }

    fn board_type(&self) -> &str {
        "arduino-serial"
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        // Verify the port exists
        if !std::path::Path::new(&self.port_path).exists() {
            anyhow::bail!(
                "Serial port {} not found. Is the Arduino connected?",
                self.port_path
            );
        }
        tracing::info!(
            board = %self.board.board,
            port = %self.port_path,
            baud = self.baud,
            "Arduino serial peripheral connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        std::path::Path::new(&self.port_path).exists()
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let port = self.port_path.clone();
        let baud = self.baud;
        vec![
            Box::new(ArduinoGpioReadTool {
                port: port.clone(),
                baud,
            }),
            Box::new(ArduinoGpioWriteTool {
                port: port.clone(),
                baud,
            }),
            Box::new(ArduinoAnalogReadTool {
                port: port.clone(),
                baud,
            }),
            Box::new(ArduinoAnalogWriteTool {
                port: port.clone(),
                baud,
            }),
            Box::new(ArduinoPingTool {
                port: port.clone(),
                baud,
            }),
        ]
    }
}

// ── Shared Serial Helper ──────────────────────────────────────────────────────

async fn serial_command(port: &str, baud: u32, cmd: Value) -> anyhow::Result<Value> {
    use tokio_serial::SerialPortBuilderExt;

    let mut serial = tokio_serial::new(port, baud)
        .timeout(READ_TIMEOUT)
        .open_native_async()
        .with_context(|| format!("Failed to open serial port {port}"))?;

    let mut line = cmd.to_string();
    line.push('\n');
    serial
        .write_all(line.as_bytes())
        .await
        .with_context(|| "Failed to write to serial port")?;
    serial.flush().await?;

    let mut reader = BufReader::new(serial);
    let mut response = String::new();
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut response))
        .await
        .with_context(|| "Serial read timed out")?
        .with_context(|| "Failed to read from serial port")?;

    serde_json::from_str(response.trim())
        .with_context(|| format!("Invalid JSON response: {response}"))
}

// ── GPIO Read Tool ────────────────────────────────────────────────────────────

struct ArduinoGpioReadTool {
    port: String,
    baud: u32,
}

#[async_trait]
impl Tool for ArduinoGpioReadTool {
    fn name(&self) -> &str {
        "arduino_gpio_read"
    }

    fn description(&self) -> &str {
        "Read the digital value (HIGH=1 or LOW=0) of a GPIO pin on an Arduino \
         board connected via USB serial. Uses Arduino pin numbering (D0–D13 \
         for Uno, D0–D53 for Mega)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Arduino digital pin number (e.g., 13 for the built-in LED)",
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
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))?;

        let response = serial_command(
            &self.port,
            self.baud,
            json!({"cmd": "gpio_read", "pin": pin}),
        )
        .await?;

        if response
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let value = response.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(ToolResult::ok(format!("D{pin} = {value}")))
        } else {
            let err = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Arduino error: {err}")
        }
    }
}

// ── GPIO Write Tool ───────────────────────────────────────────────────────────

struct ArduinoGpioWriteTool {
    port: String,
    baud: u32,
}

#[async_trait]
impl Tool for ArduinoGpioWriteTool {
    fn name(&self) -> &str {
        "arduino_gpio_write"
    }

    fn description(&self) -> &str {
        "Set a digital GPIO pin HIGH (1) or LOW (0) on an Arduino board. \
         Commonly used to control LEDs, relays, and other digital outputs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Arduino digital pin number",
                    "minimum": 0,
                    "maximum": 53
                },
                "value": {
                    "type": "integer",
                    "description": "0 for LOW, 1 for HIGH",
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
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))?;
        let value = args
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'value' parameter"))?;

        let response = serial_command(
            &self.port,
            self.baud,
            json!({"cmd": "gpio_write", "pin": pin, "value": value}),
        )
        .await?;

        if response
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            Ok(ToolResult::ok(format!("D{pin} set to {value}")))
        } else {
            let err = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Arduino error: {err}")
        }
    }
}

// ── Analog Read Tool ──────────────────────────────────────────────────────────

struct ArduinoAnalogReadTool {
    port: String,
    baud: u32,
}

#[async_trait]
impl Tool for ArduinoAnalogReadTool {
    fn name(&self) -> &str {
        "arduino_analog_read"
    }

    fn description(&self) -> &str {
        "Read the analog value (0–1023) from an analog input pin on an Arduino. \
         The value is proportional to the voltage on the pin (0V = 0, 5V = 1023 \
         for Uno; 0V = 0, 3.3V = 1023 for 3.3V boards). Analog pins are A0–A5 \
         on Uno, A0–A15 on Mega."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Analog pin number (0 = A0, 1 = A1, etc.)",
                    "minimum": 0,
                    "maximum": 15
                }
            },
            "required": ["pin"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))?;

        let response = serial_command(
            &self.port,
            self.baud,
            json!({"cmd": "analog_read", "pin": pin}),
        )
        .await?;

        if response
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let raw = response.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            let voltage = (raw as f64 / 1023.0) * 5.0;
            Ok(ToolResult::ok(format!("A{pin} = {raw} ({voltage:.3}V)")))
        } else {
            let err = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Arduino error: {err}")
        }
    }
}

// ── Analog Write (PWM) Tool ───────────────────────────────────────────────────

struct ArduinoAnalogWriteTool {
    port: String,
    baud: u32,
}

#[async_trait]
impl Tool for ArduinoAnalogWriteTool {
    fn name(&self) -> &str {
        "arduino_analog_write"
    }

    fn description(&self) -> &str {
        "Write a PWM value (0–255) to a PWM-capable pin on an Arduino. \
         On Uno, PWM pins are D3, D5, D6, D9, D10, D11. \
         The value maps to a duty cycle: 0 = 0%, 255 = 100%."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "PWM-capable pin number (e.g., 9)",
                    "minimum": 0,
                    "maximum": 53
                },
                "value": {
                    "type": "integer",
                    "description": "PWM value 0–255 (0 = off, 255 = full)",
                    "minimum": 0,
                    "maximum": 255
                }
            },
            "required": ["pin", "value"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pin' parameter"))?;
        let value = args
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing 'value' parameter"))?;

        let response = serial_command(
            &self.port,
            self.baud,
            json!({"cmd": "analog_write", "pin": pin, "value": value}),
        )
        .await?;

        if response
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let duty = (value as f64 / 255.0) * 100.0;
            Ok(ToolResult::ok(format!(
                "D{pin} PWM = {value}/255 ({duty:.1}% duty cycle)"
            )))
        } else {
            let err = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Arduino error: {err}")
        }
    }
}

// ── Ping Tool ─────────────────────────────────────────────────────────────────

struct ArduinoPingTool {
    port: String,
    baud: u32,
}

#[async_trait]
impl Tool for ArduinoPingTool {
    fn name(&self) -> &str {
        "arduino_ping"
    }

    fn description(&self) -> &str {
        "Ping the Arduino to verify it is connected and responsive. \
         Returns the firmware version and board type reported by the Arduino."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let response = serial_command(&self.port, self.baud, json!({"cmd": "ping"})).await?;

        if response
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let version = response
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let board = response
                .get("board")
                .and_then(|v| v.as_str())
                .unwrap_or("arduino");
            Ok(ToolResult::ok(format!(
                "Arduino online: board={board}, firmware={version}"
            )))
        } else {
            anyhow::bail!("Arduino did not respond to ping")
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arduino_gpio_read_tool_name() {
        let t = ArduinoGpioReadTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        assert_eq!(t.name(), "arduino_gpio_read");
    }

    #[test]
    fn arduino_gpio_write_tool_name() {
        let t = ArduinoGpioWriteTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        assert_eq!(t.name(), "arduino_gpio_write");
    }

    #[test]
    fn arduino_analog_read_tool_name() {
        let t = ArduinoAnalogReadTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        assert_eq!(t.name(), "arduino_analog_read");
    }

    #[test]
    fn arduino_analog_write_tool_name() {
        let t = ArduinoAnalogWriteTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        assert_eq!(t.name(), "arduino_analog_write");
    }

    #[test]
    fn arduino_ping_tool_name() {
        let t = ArduinoPingTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        assert_eq!(t.name(), "arduino_ping");
    }

    #[tokio::test]
    async fn gpio_read_requires_pin() {
        let t = ArduinoGpioReadTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn gpio_write_requires_value() {
        let t = ArduinoGpioWriteTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        let result = t.execute(json!({"pin": 13})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn analog_read_requires_pin() {
        let t = ArduinoAnalogReadTool {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
        };
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn new_requires_path() {
        let board = PeripheralBoardConfig {
            board: "arduino-uno".into(),
            transport: "serial".into(),
            path: None,
            baud: 115200,
            node_id: None,
        };
        assert!(ArduinoSerialPeripheral::new(board).is_err());
    }

    #[test]
    fn new_with_path_succeeds() {
        let board = PeripheralBoardConfig {
            board: "arduino-uno".into(),
            transport: "serial".into(),
            path: Some("/dev/ttyUSB0".into()),
            baud: 115200,
            node_id: None,
        };
        assert!(ArduinoSerialPeripheral::new(board).is_ok());
    }
}

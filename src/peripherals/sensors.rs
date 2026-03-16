//! Sensor tools for ESP32-S3 — camera capture, audio sampling, and generic sensor read.
//!
//! These tools extend the serial JSON protocol with sensor-specific commands:
//! - `camera_capture` — capture a JPEG image from an attached camera module
//! - `audio_sample`   — read an audio RMS level (or raw samples) from an I2S microphone
//! - `sensor_read`    — read a named I2C/SPI sensor field (temperature, humidity, etc.)
//!
//! Requires the `obc-esp32-s3` firmware flashed to the board.
//!
//! # Protocol
//!
//! The host sends a newline-delimited JSON command:
//! ```json
//! {"id":"1","cmd":"camera_capture","args":{"quality":5,"format":"jpeg"}}
//! ```
//!
//! The board responds with a newline-delimited JSON result:
//! ```json
//! {"id":"1","ok":true,"result":"<base64-encoded JPEG>"}
//! ```

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};

/// JPEG quality bounds — must match firmware constants.
pub(crate) const CAMERA_QUALITY_MIN: u64 = 1;
pub(crate) const CAMERA_QUALITY_MAX: u64 = 10;
pub(crate) const CAMERA_QUALITY_DEFAULT: u64 = 5;

/// Audio sample duration bounds (ms) — must match firmware constants.
pub(crate) const AUDIO_DURATION_MIN_MS: u64 = 10;
pub(crate) const AUDIO_DURATION_MAX_MS: u64 = 1000;
pub(crate) const AUDIO_DURATION_DEFAULT_MS: u64 = 100;

// ── Camera Capture Tool ───────────────────────────────────────────────────────

/// Tool: capture a JPEG image from the camera module on an ESP32-S3 board.
pub struct CameraCaptureTool {
    node_id: String,
}

impl CameraCaptureTool {
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
        }
    }
}

#[async_trait]
impl Tool for CameraCaptureTool {
    fn name(&self) -> &str {
        "camera_capture"
    }

    fn description(&self) -> &str {
        "Capture a JPEG image from the camera module attached to the ESP32-S3 board. \
         Returns a base64-encoded JPEG image. Use this to see what the camera sees."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "quality": {
                    "type": "integer",
                    "description": "JPEG quality (1=lowest, 10=highest). Default: 5.",
                    "minimum": CAMERA_QUALITY_MIN,
                    "maximum": CAMERA_QUALITY_MAX,
                    "default": CAMERA_QUALITY_DEFAULT
                },
                "format": {
                    "type": "string",
                    "description": "Image format. Currently only 'jpeg' is supported.",
                    "enum": ["jpeg"],
                    "default": "jpeg"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let quality = args
            .get("quality")
            .and_then(|v| v.as_u64())
            .unwrap_or(CAMERA_QUALITY_DEFAULT)
            .clamp(CAMERA_QUALITY_MIN, CAMERA_QUALITY_MAX);
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("jpeg")
            .to_string();

        tracing::debug!(
            node_id = %self.node_id,
            quality = quality,
            format = %format,
            "Capturing camera image"
        );

        // TODO: Send command to the peripheral node via serial or MQTT spine
        // and return the base64-encoded JPEG result.
        Ok(ToolResult {
            success: true,
            output: format!(
                "Camera capture requested from node '{}' (quality={}, format={}). \
                 Full implementation requires serial/MQTT connection.",
                self.node_id, quality, format
            ),
            error: None,
        })
    }
}

// ── Audio Sample Tool ─────────────────────────────────────────────────────────

/// Tool: sample audio from the I2S microphone on an ESP32-S3 board.
pub struct AudioSampleTool {
    node_id: String,
}

impl AudioSampleTool {
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
        }
    }
}

#[async_trait]
impl Tool for AudioSampleTool {
    fn name(&self) -> &str {
        "audio_sample"
    }

    fn description(&self) -> &str {
        "Sample audio from the I2S microphone attached to the ESP32-S3 board. \
         Returns the RMS audio level (0.0–1.0) or raw PCM samples. \
         Use this to detect sound levels or record audio snippets."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "duration_ms": {
                    "type": "integer",
                    "description": "Duration to sample in milliseconds (10–1000). Default: 100.",
                    "minimum": AUDIO_DURATION_MIN_MS,
                    "maximum": AUDIO_DURATION_MAX_MS,
                    "default": AUDIO_DURATION_DEFAULT_MS
                },
                "raw": {
                    "type": "boolean",
                    "description": "If true, return raw PCM samples. If false, return RMS level. Default: false.",
                    "default": false
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let duration_ms = args
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(AUDIO_DURATION_DEFAULT_MS)
            .clamp(AUDIO_DURATION_MIN_MS, AUDIO_DURATION_MAX_MS);
        let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);

        tracing::debug!(
            node_id = %self.node_id,
            duration_ms = duration_ms,
            raw = raw,
            "Sampling audio"
        );

        // TODO: Send command to the peripheral node via serial or MQTT spine.
        Ok(ToolResult {
            success: true,
            output: format!(
                "Audio sample requested from node '{}' (duration={}ms, raw={}). \
                 Full implementation requires serial/MQTT connection.",
                self.node_id, duration_ms, raw
            ),
            error: None,
        })
    }
}

// ── Sensor Read Tool ──────────────────────────────────────────────────────────

/// Tool: read a value from an I2C/SPI sensor on an ESP32-S3 board.
pub struct SensorReadTool {
    node_id: String,
}

impl SensorReadTool {
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
        }
    }
}

#[async_trait]
impl Tool for SensorReadTool {
    fn name(&self) -> &str {
        "sensor_read"
    }

    fn description(&self) -> &str {
        "Read a value from an I2C or SPI sensor attached to the ESP32-S3 board. \
         Supported sensors include BME280 (temperature, humidity, pressure), \
         MPU6050 (accelerometer, gyroscope), SHT31 (temperature, humidity), \
         and BMP180 (temperature, pressure)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sensor": {
                    "type": "string",
                    "description": "The sensor to read from (e.g., 'bme280', 'mpu6050', 'sht31', 'bmp180').",
                    "enum": ["bme280", "mpu6050", "sht31", "bmp180"]
                },
                "field": {
                    "type": "string",
                    "description": "The field to read (e.g., 'temperature', 'humidity', 'pressure', 'accel_x')."
                }
            },
            "required": ["sensor", "field"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let sensor = args
            .get("sensor")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'sensor' parameter"))?
            .to_string();
        let field = args
            .get("field")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'field' parameter"))?
            .to_string();

        tracing::debug!(
            node_id = %self.node_id,
            sensor = %sensor,
            field = %field,
            "Reading sensor"
        );

        // TODO: Send command to the peripheral node via serial or MQTT spine.
        Ok(ToolResult {
            success: true,
            output: format!(
                "Sensor read requested from node '{}' (sensor={}, field={}). \
                 Full implementation requires serial/MQTT connection.",
                self.node_id, sensor, field
            ),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn camera_capture_tool_has_correct_name() {
        let tool = CameraCaptureTool::new("test-node");
        assert_eq!(tool.name(), "camera_capture");
    }

    #[tokio::test]
    async fn audio_sample_tool_has_correct_name() {
        let tool = AudioSampleTool::new("test-node");
        assert_eq!(tool.name(), "audio_sample");
    }

    #[tokio::test]
    async fn sensor_read_tool_requires_sensor_and_field() {
        let tool = SensorReadTool::new("test-node");
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn sensor_read_tool_executes_with_valid_args() {
        let tool = SensorReadTool::new("test-node");
        let result = tool
            .execute(serde_json::json!({"sensor": "bme280", "field": "temperature"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("bme280"));
    }
}

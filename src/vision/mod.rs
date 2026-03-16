//! Vision Pipeline — camera capture → LLM vision analysis → action.
//!
//! The vision pipeline ties together hardware camera capture (from an attached
//! peripheral such as an ESP32-S3 or Raspberry Pi camera module) with a
//! multimodal LLM vision model to produce structured descriptions and drive
//! downstream agent actions.
//!
//! # Design
//!
//! ```text
//!  ┌─────────────┐    JPEG/PNG     ┌──────────────┐   description   ┌──────────┐
//!  │   Camera    │ ─────────────▶  │  VisionModel │ ──────────────▶ │  Action  │
//!  │  Peripheral │                 │  (LLM API)   │                 │ Dispatch │
//!  └─────────────┘                 └──────────────┘                 └──────────┘
//! ```
//!
//! The pipeline can be used directly or exposed as an agent tool
//! (`VisionPipelineTool`) so the LLM can trigger it autonomously.

use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

// ── Configuration ─────────────────────────────────────────────────────────────

/// Where the vision pipeline should obtain images from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CameraSource {
    /// A local file path (useful for testing or stored images).
    File { path: String },
    /// A remote URL (HTTP/HTTPS image endpoint).
    Url { url: String },
    /// Capture via `libcamera-still` on a Raspberry Pi.
    LibCamera {
        /// Width in pixels (default: 1280).
        width: u32,
        /// Height in pixels (default: 720).
        height: u32,
        /// Capture timeout in milliseconds (default: 2000).
        timeout_ms: u32,
    },
    /// Capture via a system-level video device (V4L2 / `ffmpeg`).
    V4l2 {
        /// Device path, e.g. `/dev/video0`.
        device: String,
    },
}

impl Default for CameraSource {
    fn default() -> Self {
        CameraSource::LibCamera {
            width: 1280,
            height: 720,
            timeout_ms: 2000,
        }
    }
}

/// Configuration for the vision pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionPipelineConfig {
    /// Where to capture images from.
    pub camera_source: CameraSource,
    /// OpenAI-compatible vision API base URL.
    pub api_base: String,
    /// Vision model to use (must support image inputs).
    pub model: String,
    /// Detail level passed to the vision API: `"low"`, `"high"`, or `"auto"`.
    pub detail: String,
    /// Maximum tokens to request for the vision response.
    pub max_tokens: u32,
    /// Directory where captured frames are saved.
    pub capture_dir: PathBuf,
}

impl Default for VisionPipelineConfig {
    fn default() -> Self {
        Self {
            camera_source: CameraSource::default(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            model: "gpt-4o".to_string(),
            detail: "auto".to_string(),
            max_tokens: 1024,
            capture_dir: default_capture_dir(),
        }
    }
}

fn default_capture_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("oh-ben-claw")
                .join("captures")
        })
        .unwrap_or_else(|_| PathBuf::from("/tmp/oh-ben-claw/captures"))
}

// ── Frame ────────────────────────────────────────────────────────────────────

/// A captured frame, ready to be sent to a vision model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedFrame {
    /// MIME type of the image (e.g. `"image/jpeg"`).
    pub mime_type: String,
    /// Base64-encoded image bytes.
    pub data: String,
    /// Path where the frame was saved (if any).
    pub path: Option<PathBuf>,
    /// Unix timestamp (seconds) when the frame was captured.
    pub captured_at: u64,
}

impl CapturedFrame {
    /// Build an OpenAI-compatible image content part.
    pub fn to_openai_part(&self, detail: &str) -> Value {
        json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", self.mime_type, self.data),
                "detail": detail
            }
        })
    }
}

// ── Vision Analysis Result ────────────────────────────────────────────────────

/// The result of running the full vision pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionAnalysis {
    /// The textual description / answer from the LLM.
    pub description: String,
    /// The prompt that was sent to the model.
    pub prompt: String,
    /// Frame metadata.
    pub frame: CapturedFrame,
    /// Model used for analysis.
    pub model: String,
}

// ── Vision Pipeline ───────────────────────────────────────────────────────────

/// End-to-end vision pipeline.
///
/// Captures an image from the configured source, encodes it, sends it to a
/// multimodal LLM vision endpoint, and returns the structured analysis.
pub struct VisionPipeline {
    pub config: VisionPipelineConfig,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl VisionPipeline {
    /// Create a new pipeline with the given configuration.
    ///
    /// Reads `OPENAI_API_KEY` from the environment if present.
    pub fn new(config: VisionPipelineConfig) -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").ok();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            config,
            api_key,
            client,
        }
    }

    /// Capture a frame from the configured camera source.
    pub async fn capture(&self) -> anyhow::Result<CapturedFrame> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        match &self.config.camera_source {
            CameraSource::File { path } => capture_from_file(path, now),
            CameraSource::Url { url } => capture_from_url(url, &self.client, now).await,
            CameraSource::LibCamera {
                width,
                height,
                timeout_ms,
            } => {
                capture_libcamera(*width, *height, *timeout_ms, &self.config.capture_dir, now)
                    .await
            }
            CameraSource::V4l2 { device } => {
                capture_v4l2(device, &self.config.capture_dir, now).await
            }
        }
    }

    /// Analyse a captured frame using the configured vision model.
    pub async fn analyse(
        &self,
        frame: CapturedFrame,
        prompt: &str,
    ) -> anyhow::Result<VisionAnalysis> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("OPENAI_API_KEY not set"))?;

        let request = json!({
            "model": self.config.model,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    frame.to_openai_part(&self.config.detail)
                ]
            }],
            "max_tokens": self.config.max_tokens
        });

        let url = format!("{}/chat/completions", self.config.api_base);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&request)
            .send()
            .await?;

        let body: Value = resp.json().await?;

        if let Some(err) = body.get("error") {
            anyhow::bail!("Vision API error: {}", err);
        }

        let description = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(VisionAnalysis {
            description,
            prompt: prompt.to_string(),
            frame,
            model: self.config.model.clone(),
        })
    }

    /// Capture and analyse in one call.
    pub async fn run(&self, prompt: &str) -> anyhow::Result<VisionAnalysis> {
        let frame = self.capture().await?;
        self.analyse(frame, prompt).await
    }
}

// ── Capture helpers ───────────────────────────────────────────────────────────

fn capture_from_file(path: &str, now: u64) -> anyhow::Result<CapturedFrame> {
    let p = std::path::Path::new(path);
    let mime_type = mime_from_extension(
        p.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg"),
    );
    let bytes = std::fs::read(p)?;
    Ok(CapturedFrame {
        mime_type,
        data: B64.encode(&bytes),
        path: Some(p.to_path_buf()),
        captured_at: now,
    })
}

async fn capture_from_url(
    url: &str,
    client: &reqwest::Client,
    now: u64,
) -> anyhow::Result<CapturedFrame> {
    let resp = client.get(url).send().await?;
    let mime_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .split(';')
        .next()
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp.bytes().await?;
    Ok(CapturedFrame {
        mime_type,
        data: B64.encode(&bytes),
        path: None,
        captured_at: now,
    })
}

async fn capture_libcamera(
    width: u32,
    height: u32,
    timeout_ms: u32,
    capture_dir: &PathBuf,
    now: u64,
) -> anyhow::Result<CapturedFrame> {
    std::fs::create_dir_all(capture_dir)?;
    let out_path = capture_dir.join(format!("frame_{now}.jpg"));

    let status = tokio::process::Command::new("libcamera-still")
        .args([
            "--output",
            out_path.to_str().unwrap_or("/tmp/frame.jpg"),
            "--width",
            &width.to_string(),
            "--height",
            &height.to_string(),
            "--timeout",
            &timeout_ms.to_string(),
            "--nopreview",
            "--immediate",
        ])
        .status()
        .await;

    match status {
        Ok(s) if s.success() => capture_from_file(out_path.to_str().unwrap_or(""), now),
        Ok(s) => anyhow::bail!("libcamera-still exited with status {s}"),
        Err(e) => anyhow::bail!("Failed to run libcamera-still: {e}"),
    }
}

async fn capture_v4l2(
    device: &str,
    capture_dir: &PathBuf,
    now: u64,
) -> anyhow::Result<CapturedFrame> {
    std::fs::create_dir_all(capture_dir)?;
    let out_path = capture_dir.join(format!("frame_{now}.jpg"));

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "v4l2",
            "-frames:v",
            "1",
            "-i",
            device,
            out_path.to_str().unwrap_or("/tmp/frame.jpg"),
        ])
        .status()
        .await;

    match status {
        Ok(s) if s.success() => capture_from_file(out_path.to_str().unwrap_or(""), now),
        Ok(s) => anyhow::bail!("ffmpeg exited with status {s}"),
        Err(e) => anyhow::bail!("Failed to run ffmpeg: {e}"),
    }
}

/// Infer a MIME type from a file extension.
pub fn mime_from_extension(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
    .to_string()
}

// ── VisionPipelineTool ────────────────────────────────────────────────────────

/// An agent tool that runs the full vision pipeline.
///
/// The agent can call this tool to capture an image from the configured camera
/// and get a textual description or answer to a question about the scene.
pub struct VisionPipelineTool {
    pipeline: VisionPipeline,
}

impl VisionPipelineTool {
    /// Create with the given pipeline configuration.
    pub fn new(config: VisionPipelineConfig) -> Self {
        Self {
            pipeline: VisionPipeline::new(config),
        }
    }

    /// Create with default configuration (reads API key + base from env).
    pub fn default_config() -> Self {
        Self::new(VisionPipelineConfig::default())
    }
}

#[async_trait]
impl Tool for VisionPipelineTool {
    fn name(&self) -> &str {
        "vision_pipeline"
    }

    fn description(&self) -> &str {
        "Capture an image from the connected camera and analyse it with a vision-capable \
        LLM. Returns a detailed description of the scene or answers a specific question \
        about what the camera sees. Supports libcamera (Raspberry Pi), V4L2 devices, \
        local files, and remote image URLs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What to ask about the image. Defaults to 'Describe this scene in detail.'"
                },
                "source": {
                    "type": "string",
                    "description": "Override the camera source for this call. Accepts a local file path or HTTP(S) URL."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Describe this scene in detail.")
            .to_string();

        // Allow ad-hoc source override
        if let Some(source) = args.get("source").and_then(|v| v.as_str()) {
            let override_config = VisionPipelineConfig {
                camera_source: if source.starts_with("http://") || source.starts_with("https://") {
                    CameraSource::Url {
                        url: source.to_string(),
                    }
                } else {
                    CameraSource::File {
                        path: source.to_string(),
                    }
                },
                ..self.pipeline.config.clone()
            };
            let override_pipeline = VisionPipeline::new(override_config);
            return match override_pipeline.run(&prompt).await {
                Ok(analysis) => Ok(ToolResult::ok(analysis.description)),
                Err(e) => Ok(ToolResult::err(format!("Vision pipeline error: {e}"))),
            };
        }

        match self.pipeline.run(&prompt).await {
            Ok(analysis) => Ok(ToolResult::ok(analysis.description)),
            Err(e) => Ok(ToolResult::err(format!("Vision pipeline error: {e}"))),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_detection_works() {
        assert_eq!(mime_from_extension("jpg"), "image/jpeg");
        assert_eq!(mime_from_extension("jpeg"), "image/jpeg");
        assert_eq!(mime_from_extension("png"), "image/png");
        assert_eq!(mime_from_extension("webp"), "image/webp");
        assert_eq!(mime_from_extension("gif"), "image/gif");
        assert_eq!(mime_from_extension("bmp"), "image/bmp");
        assert_eq!(mime_from_extension("unknown"), "image/jpeg");
    }

    #[test]
    fn captured_frame_openai_part_format() {
        let frame = CapturedFrame {
            mime_type: "image/jpeg".to_string(),
            data: "abc123".to_string(),
            path: None,
            captured_at: 0,
        };
        let part = frame.to_openai_part("auto");
        assert_eq!(part["type"], "image_url");
        let url = part["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        assert_eq!(part["image_url"]["detail"], "auto");
    }

    #[test]
    fn camera_source_default_is_libcamera() {
        let src = CameraSource::default();
        assert!(matches!(src, CameraSource::LibCamera { .. }));
    }

    #[test]
    fn pipeline_config_default_model_is_gpt4o() {
        let cfg = VisionPipelineConfig::default();
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.detail, "auto");
        assert_eq!(cfg.max_tokens, 1024);
    }

    #[test]
    fn vision_pipeline_tool_name() {
        let tool = VisionPipelineTool::default_config();
        assert_eq!(tool.name(), "vision_pipeline");
    }

    #[test]
    fn vision_pipeline_tool_schema_has_prompt() {
        let tool = VisionPipelineTool::default_config();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["prompt"].is_object());
        assert!(schema["properties"]["source"].is_object());
    }

    #[tokio::test]
    async fn vision_pipeline_tool_fails_gracefully_on_missing_file() {
        // Clear the API key so the file-not-found error is reported
        let prev = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("OPENAI_API_KEY");

        let config = VisionPipelineConfig {
            camera_source: CameraSource::File {
                path: "/nonexistent/frame.jpg".to_string(),
            },
            ..VisionPipelineConfig::default()
        };
        let tool = VisionPipelineTool::new(config);
        let result = tool.execute(json!({"prompt": "test"})).await.unwrap();
        assert!(!result.success);

        if let Some(key) = prev {
            std::env::set_var("OPENAI_API_KEY", key);
        }
    }

    #[test]
    fn capture_from_file_reads_bytes() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"\xff\xd8\xff\xe0test").unwrap();
        let path_str = tmp.path().to_str().unwrap();
        let frame = capture_from_file(path_str, 42).unwrap();
        assert_eq!(frame.mime_type, "image/jpeg");
        assert!(!frame.data.is_empty());
        assert_eq!(frame.captured_at, 42);
    }
}

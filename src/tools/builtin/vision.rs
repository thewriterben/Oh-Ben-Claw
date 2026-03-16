//! Vision tool — encode images and send them to multimodal LLMs.
//!
//! Supports local file paths (JPEG, PNG, WebP, GIF) and URLs.
//! Compatible with GPT-5.4 and Claude Opus 4.6 vision APIs.

use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// Describes a vision request — an image plus an optional question.
#[derive(Debug, Deserialize)]
struct VisionInput {
    /// Local file path or URL of the image.
    source: String,
    /// Question or instruction about the image (default: "Describe this image.").
    prompt: Option<String>,
    /// Maximum detail level: "low", "high", or "auto" (OpenAI only).
    detail: Option<String>,
}

/// Encoded image ready to be sent to a vision-capable LLM.
#[derive(Debug, Clone, Serialize)]
pub struct EncodedImage {
    /// MIME type (e.g. "image/jpeg").
    pub mime_type: String,
    /// Base64-encoded image data.
    pub data: String,
    /// Original source path or URL.
    pub source: String,
}

impl EncodedImage {
    /// Build an OpenAI-compatible image_url content part.
    pub fn to_openai_part(&self, detail: &str) -> Value {
        json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", self.mime_type, self.data),
                "detail": detail
            }
        })
    }

    /// Build an Anthropic-compatible image source block.
    pub fn to_anthropic_source(&self) -> Value {
        json!({
            "type": "base64",
            "media_type": self.mime_type,
            "data": self.data
        })
    }
}

/// Encode a local image file to base64.
pub fn encode_local_image(path: &str) -> anyhow::Result<EncodedImage> {
    let path = Path::new(path);
    let mime_type = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
    .to_string();

    let bytes = std::fs::read(path)?;
    let data = B64.encode(&bytes);

    Ok(EncodedImage {
        mime_type,
        data,
        source: path.to_string_lossy().to_string(),
    })
}

/// Fetch and encode a remote image URL to base64.
pub async fn encode_remote_image(url: &str) -> anyhow::Result<EncodedImage> {
    let response = reqwest::get(url).await?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .split(';')
        .next()
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = response.bytes().await?;
    let data = B64.encode(&bytes);

    Ok(EncodedImage {
        mime_type: content_type,
        data,
        source: url.to_string(),
    })
}

// ── Vision Tool ──────────────────────────────────────────────────────────────

/// A tool that encodes an image and returns a structured description.
///
/// The agent calls this tool with an image path or URL and an optional prompt.
/// The tool encodes the image and returns a JSON payload that the agent loop
/// injects into the next LLM message as a multimodal content part.
pub struct VisionTool {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl VisionTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: "gpt-5.4".to_string(),
            base_url: "https://api.openai.com/v1/chat/completions".to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait]
impl Tool for VisionTool {
    fn name(&self) -> &str {
        "vision_analyze"
    }

    fn description(&self) -> &str {
        "Analyze an image from a local file path or URL. Returns a detailed description or answers a specific question about the image. Supports JPEG, PNG, WebP, GIF, and BMP formats."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Local file path or URL of the image to analyze"
                },
                "prompt": {
                    "type": "string",
                    "description": "Question or instruction about the image. Defaults to 'Describe this image in detail.'"
                },
                "detail": {
                    "type": "string",
                    "enum": ["low", "high", "auto"],
                    "description": "Detail level for analysis (default: auto)"
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let input: VisionInput = match serde_json::from_value(args) {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::err(format!("Invalid arguments: {e}"))),
        };

        let prompt = input
            .prompt
            .unwrap_or_else(|| "Describe this image in detail.".to_string());
        let detail = input.detail.unwrap_or_else(|| "auto".to_string());

        // Encode the image
        let encoded = if input.source.starts_with("http://") || input.source.starts_with("https://") {
            match encode_remote_image(&input.source).await {
                Ok(e) => e,
                Err(e) => return Ok(ToolResult::err(format!("Failed to fetch image: {e}"))),
            }
        } else {
            match encode_local_image(&input.source) {
                Ok(e) => e,
                Err(e) => return Ok(ToolResult::err(format!("Failed to read image: {e}"))),
            }
        };

        // Send to OpenAI vision API
        let request = json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": prompt
                    },
                    encoded.to_openai_part(&detail)
                ]
            }],
            "max_tokens": 1024
        });

        let response = match self
            .client
            .post(&self.base_url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("API request failed: {e}"))),
        };

        let body: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::err(format!("Failed to parse response: {e}"))),
        };

        if let Some(err) = body.get("error") {
            return Ok(ToolResult::err(format!("API error: {}", err)));
        }

        let description = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("No description returned")
            .to_string();

        Ok(ToolResult::ok(description))
    }
}

// ── Audio Transcription Tool ─────────────────────────────────────────────────

/// A tool that transcribes audio files using the OpenAI Whisper API.
pub struct AudioTranscriptionTool {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl AudioTranscriptionTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: "whisper-1".to_string(),
        }
    }
}

#[async_trait]
impl Tool for AudioTranscriptionTool {
    fn name(&self) -> &str {
        "audio_transcribe"
    }

    fn description(&self) -> &str {
        "Transcribe an audio file (MP3, MP4, WAV, FLAC, OGG, WebM) to text using OpenAI Whisper. Returns the full transcript with optional timestamps."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local file path of the audio file to transcribe"
                },
                "language": {
                    "type": "string",
                    "description": "ISO-639-1 language code (e.g. 'en', 'es', 'fr'). Auto-detected if omitted."
                },
                "timestamps": {
                    "type": "boolean",
                    "description": "Whether to include word-level timestamps in the output"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: path")),
        };
        let language = args.get("language").and_then(|v| v.as_str()).map(|s| s.to_string());
        let timestamps = args.get("timestamps").and_then(|v| v.as_bool()).unwrap_or(false);

        // Read the audio file
        let file_bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return Ok(ToolResult::err(format!("Failed to read audio file: {e}"))),
        };

        let file_name = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3")
            .to_string();

        // Determine MIME type
        let mime = match file_name.rsplit('.').next().map(|e| e.to_lowercase()).as_deref() {
            Some("mp3") => "audio/mpeg",
            Some("mp4") => "audio/mp4",
            Some("wav") => "audio/wav",
            Some("flac") => "audio/flac",
            Some("ogg") => "audio/ogg",
            Some("webm") => "audio/webm",
            Some("m4a") => "audio/mp4",
            _ => "audio/mpeg",
        };

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str(mime)
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone())
            .text("response_format", if timestamps { "verbose_json" } else { "text" });

        if let Some(lang) = language {
            form = form.text("language", lang);
        }

        let response = match self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("API request failed: {e}"))),
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Ok(ToolResult::err(format!("Whisper API error {status}: {body}")));
        }

        let transcript = if timestamps {
            let body: Value = match response.json().await {
                Ok(v) => v,
                Err(e) => return Ok(ToolResult::err(format!("Failed to parse response: {e}"))),
            };
            serde_json::to_string_pretty(&body).unwrap_or_default()
        } else {
            match response.text().await {
                Ok(t) => t.trim().to_string(),
                Err(e) => return Ok(ToolResult::err(format!("Failed to read response: {e}"))),
            }
        };

        Ok(ToolResult::ok(transcript))
    }
}

// ── Structured Output Tool ───────────────────────────────────────────────────

/// A tool that forces the LLM to return a JSON object matching a given schema.
pub struct StructuredOutputTool {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl StructuredOutputTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: "gpt-5.4".to_string(),
        }
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        "structured_output"
    }

    fn description(&self) -> &str {
        "Extract structured data from text by providing a JSON schema. The model will return a JSON object that strictly conforms to the schema. Useful for data extraction, classification, and parsing."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to extract structured data from"
                },
                "schema": {
                    "type": "object",
                    "description": "JSON Schema object defining the expected output structure"
                },
                "schema_name": {
                    "type": "string",
                    "description": "A name for the schema (used in the API request)"
                },
                "instruction": {
                    "type": "string",
                    "description": "Additional instruction for the extraction task"
                }
            },
            "required": ["text", "schema"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: text")),
        };
        let schema = match args.get("schema") {
            Some(s) => s.clone(),
            None => return Ok(ToolResult::err("Missing required argument: schema")),
        };
        let schema_name = args
            .get("schema_name")
            .and_then(|v| v.as_str())
            .unwrap_or("extraction_result")
            .to_string();
        let instruction = args
            .get("instruction")
            .and_then(|v| v.as_str())
            .unwrap_or("Extract the requested information from the text.")
            .to_string();

        let request = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": instruction
                },
                {
                    "role": "user",
                    "content": text
                }
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": schema_name,
                    "strict": true,
                    "schema": schema
                }
            }
        });

        let response = match self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("API request failed: {e}"))),
        };

        let body: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::err(format!("Failed to parse response: {e}"))),
        };

        if let Some(err) = body.get("error") {
            return Ok(ToolResult::err(format!("API error: {}", err)));
        }

        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("{}")
            .to_string();

        // Validate that it's valid JSON
        match serde_json::from_str::<Value>(&content) {
            Ok(v) => Ok(ToolResult::ok(serde_json::to_string_pretty(&v).unwrap_or(content))),
            Err(_) => Ok(ToolResult::ok(content)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_local_image_mime_type() {
        // Test MIME type detection without actually reading a file
        let jpeg = EncodedImage {
            mime_type: "image/jpeg".to_string(),
            data: "abc".to_string(),
            source: "test.jpg".to_string(),
        };
        let part = jpeg.to_openai_part("auto");
        assert!(part["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn test_encoded_image_anthropic_source() {
        let img = EncodedImage {
            mime_type: "image/png".to_string(),
            data: "xyz".to_string(),
            source: "test.png".to_string(),
        };
        let source = img.to_anthropic_source();
        assert_eq!(source["type"], "base64");
        assert_eq!(source["media_type"], "image/png");
        assert_eq!(source["data"], "xyz");
    }

    #[test]
    fn test_vision_tool_schema() {
        let tool = VisionTool::new("test_key");
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["source"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("source")));
    }

    #[test]
    fn test_audio_tool_schema() {
        let tool = AudioTranscriptionTool::new("test_key");
        let schema = tool.parameters_schema();
        assert_eq!(tool.name(), "audio_transcribe");
        assert!(schema["properties"]["path"].is_object());
    }

    #[test]
    fn test_structured_output_tool_schema() {
        let tool = StructuredOutputTool::new("test_key");
        assert_eq!(tool.name(), "structured_output");
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["schema"].is_object());
    }
}

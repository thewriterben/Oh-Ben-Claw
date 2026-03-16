//! Audio transcription and speech tools.
//!
//! Supports:
//! - OpenAI Whisper API (whisper-1, whisper-large-v3)
//! - Local whisper.cpp via subprocess
//! - Audio recording from peripheral microphones via the MQTT Spine

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

// ── Transcription Tool ────────────────────────────────────────────────────────

/// Tool for transcribing audio files to text using Whisper.
pub struct AudioTranscribeTool {
    pub api_key: Option<String>,
    pub api_base: String,
    pub model: String,
}

impl Default for AudioTranscribeTool {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            model: "whisper-1".to_string(),
        }
    }
}

#[async_trait]
impl Tool for AudioTranscribeTool {
    fn name(&self) -> &str {
        "audio_transcribe"
    }

    fn description(&self) -> &str {
        "Transcribe an audio file to text using OpenAI Whisper. Supports MP3, MP4, MPEG, \
        MPGA, M4A, WAV, and WEBM formats up to 25MB. Returns the transcribed text with \
        optional timestamps and language detection."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the audio file to transcribe"
                },
                "language": {
                    "type": "string",
                    "description": "Language code (e.g. 'en', 'es', 'fr'). Auto-detected if not specified."
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional context prompt to improve transcription accuracy"
                },
                "timestamps": {
                    "type": "boolean",
                    "description": "Include word-level timestamps in output",
                    "default": false
                },
                "use_local": {
                    "type": "boolean",
                    "description": "Use local whisper.cpp instead of the API (requires whisper.cpp installed)",
                    "default": false
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => PathBuf::from(p),
            None => return Ok(ToolResult::err("Missing required parameter: file_path")),
        };

        if !file_path.exists() {
            return Ok(ToolResult::err(&format!(
                "Audio file not found: {}",
                file_path.display()
            )));
        }

        let use_local = args.get("use_local").and_then(|v| v.as_bool()).unwrap_or(false);

        if use_local {
            return transcribe_local(&file_path, &args).await;
        }

        // Use OpenAI Whisper API
        let api_key = match &self.api_key {
            Some(k) => k.clone(),
            None => {
                return Ok(ToolResult::err(
                    "OPENAI_API_KEY not set. Use use_local=true for local whisper.cpp transcription."
                ))
            }
        };

        transcribe_openai(&file_path, &api_key, &self.api_base, &self.model, &args).await
    }
}

async fn transcribe_openai(
    file_path: &PathBuf,
    api_key: &str,
    api_base: &str,
    model: &str,
    args: &Value,
) -> anyhow::Result<ToolResult> {
    let language = args.get("language").and_then(|v| v.as_str());
    let prompt = args.get("prompt").and_then(|v| v.as_str());
    let timestamps = args.get("timestamps").and_then(|v| v.as_bool()).unwrap_or(false);

    let file_bytes = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(e) => return Ok(ToolResult::err(&format!("Failed to read audio file: {e}"))),
    };

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.mp3");

    let mime_type = match file_path.extension().and_then(|e| e.to_str()) {
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("webm") => "audio/webm",
        Some("m4a") => "audio/m4a",
        _ => "audio/mpeg",
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(c) => c,
        Err(e) => return Ok(ToolResult::err(&format!("Failed to build HTTP client: {e}"))),
    };

    let response_format = if timestamps { "verbose_json" } else { "text" };

    let mut form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(file_bytes)
                .file_name(file_name.to_string())
                .mime_str(mime_type)
                .unwrap(),
        )
        .text("model", model.to_string())
        .text("response_format", response_format.to_string());

    if let Some(lang) = language {
        form = form.text("language", lang.to_string());
    }
    if let Some(p) = prompt {
        form = form.text("prompt", p.to_string());
    }

    let url = format!("{api_base}/audio/transcriptions");
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let text = r.text().await.unwrap_or_default();
            Ok(ToolResult::ok(text))
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            Ok(ToolResult::err(&format!("Whisper API error {status}: {body}")))
        }
        Err(e) => Ok(ToolResult::err(&format!("HTTP request failed: {e}"))),
    }
}

async fn transcribe_local(file_path: &PathBuf, args: &Value) -> anyhow::Result<ToolResult> {
    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    // Try whisper.cpp CLI
    let mut cmd_args = vec![
        "-m",
        "/usr/local/share/whisper/ggml-base.en.bin",
        "-f",
        file_path.to_str().unwrap_or(""),
        "-l",
        language,
        "--output-txt",
        "--no-timestamps",
    ];

    let output = tokio::process::Command::new("whisper")
        .args(&cmd_args)
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            Ok(ToolResult::ok(text.trim().to_string()))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Ok(ToolResult::err(&format!("whisper.cpp failed: {stderr}")))
        }
        Err(_) => {
            // Try whisper-cpp as alternative binary name
            let output2 = tokio::process::Command::new("whisper-cpp")
                .args(&cmd_args)
                .output()
                .await;

            match output2 {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout).to_string();
                    Ok(ToolResult::ok(text.trim().to_string()))
                }
                _ => Ok(ToolResult::err(
                    "whisper.cpp not found. Install from https://github.com/ggerganov/whisper.cpp \
                    or set use_local=false to use the OpenAI Whisper API."
                )),
            }
        }
    }
}

// ── Text-to-Speech Tool ───────────────────────────────────────────────────────

/// Tool for converting text to speech using OpenAI TTS.
pub struct TextToSpeechTool {
    pub api_key: Option<String>,
    pub api_base: String,
}

impl Default for TextToSpeechTool {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        }
    }
}

#[async_trait]
impl Tool for TextToSpeechTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }

    fn description(&self) -> &str {
        "Convert text to speech audio using OpenAI TTS. Saves the audio to a file \
        and returns the file path. Supports multiple voices and output formats."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to convert to speech (max 4096 characters)"
                },
                "voice": {
                    "type": "string",
                    "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                    "description": "Voice to use for synthesis",
                    "default": "nova"
                },
                "model": {
                    "type": "string",
                    "enum": ["tts-1", "tts-1-hd"],
                    "description": "TTS model (tts-1 is faster, tts-1-hd is higher quality)",
                    "default": "tts-1"
                },
                "output_path": {
                    "type": "string",
                    "description": "Path to save the audio file (default: /tmp/obc_tts_<timestamp>.mp3)"
                },
                "format": {
                    "type": "string",
                    "enum": ["mp3", "opus", "aac", "flac"],
                    "description": "Output audio format",
                    "default": "mp3"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return Ok(ToolResult::err("Missing required parameter: text")),
        };

        if text.len() > 4096 {
            return Ok(ToolResult::err("Text exceeds 4096 character limit for TTS"));
        }

        let api_key = match &self.api_key {
            Some(k) => k.clone(),
            None => return Ok(ToolResult::err("OPENAI_API_KEY not set")),
        };

        let voice = args
            .get("voice")
            .and_then(|v| v.as_str())
            .unwrap_or("nova");
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("tts-1");
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("mp3");

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("/tmp/obc_tts_{timestamp}.{format}"));

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
        {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(&format!("Failed to build HTTP client: {e}"))),
        };

        let body = json!({
            "model": model,
            "input": text,
            "voice": voice,
            "response_format": format
        });

        let url = format!("{}/audio/speech", self.api_base);
        let resp = client
            .post(&url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().await.unwrap_or_default();
                match std::fs::write(&output_path, &bytes) {
                    Ok(_) => Ok(ToolResult::ok(format!(
                        "Speech generated successfully.\n\
                        File: {output_path}\n\
                        Size: {} bytes\n\
                        Voice: {voice}\n\
                        Model: {model}",
                        bytes.len())
                    )),
                    Err(e) => Ok(ToolResult::err(&format!("Failed to save audio file: {e}"))),
                }
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                Ok(ToolResult::err(&format!("TTS API error {status}: {body}")))
            }
            Err(e) => Ok(ToolResult::err(&format!("HTTP request failed: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcribe_tool_name() {
        let tool = AudioTranscribeTool::default();
        assert_eq!(tool.name(), "audio_transcribe");
    }

    #[test]
    fn test_tts_tool_name() {
        let tool = TextToSpeechTool::default();
        assert_eq!(tool.name(), "text_to_speech");
    }

    #[tokio::test]
    async fn test_transcribe_missing_file_path() {
        let tool = AudioTranscribeTool::default();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or(&result.output).contains("file_path"));
    }

    #[tokio::test]
    async fn test_transcribe_nonexistent_file() {
        let tool = AudioTranscribeTool::default();
        let result = tool
            .execute(json!({"file_path": "/nonexistent/audio.mp3"}))
            .await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or(&result.output).contains("not found"));
    }

    #[tokio::test]
    async fn test_tts_missing_text() {
        let tool = TextToSpeechTool::default();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or(&result.output).contains("text"));
    }

    #[tokio::test]
    async fn test_tts_text_too_long() {
        let tool = TextToSpeechTool::default();
        let long_text = "a".repeat(5000);
        let result = tool.execute(json!({"text": long_text})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or(&result.output).contains("4096"));
    }

    #[test]
    fn test_transcribe_schema() {
        let tool = AudioTranscribeTool::default();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema["properties"].get("file_path").is_some());
    }

    #[test]
    fn test_tts_schema() {
        let tool = TextToSpeechTool::default();
        let schema = tool.parameters_schema();
        assert!(schema["properties"].get("voice").is_some());
        assert!(schema["properties"].get("model").is_some());
    }
}

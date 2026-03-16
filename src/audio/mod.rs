//! Audio Pipeline — microphone capture → speech-to-text → agent → text-to-speech.
//!
//! The audio pipeline connects the microphone input to the agent's language
//! model and feeds the agent's response back through a text-to-speech engine.
//!
//! # Design
//!
//! ```text
//!  ┌─────────────┐   audio file   ┌────────────┐   text   ┌────────┐
//!  │  Microphone │ ─────────────▶ │  Whisper   │ ───────▶ │ Agent  │
//!  │  / file     │                │  STT API   │          │  LLM   │
//!  └─────────────┘                └────────────┘          └───┬────┘
//!                                                              │ response text
//!                                                         ┌────▼────┐
//!                                                         │   TTS   │
//!                                                         │  (mp3)  │
//!                                                         └─────────┘
//! ```
//!
//! The pipeline is exposed as an agent tool (`AudioPipelineTool`) so the LLM
//! can trigger it, and the raw pipeline struct can be used directly from Rust.

use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

// ── Configuration ─────────────────────────────────────────────────────────────

/// Where the audio pipeline should record audio from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MicrophoneSource {
    /// A pre-recorded local audio file.
    File { path: String },
    /// Record from the default system microphone using `arecord` (Linux ALSA).
    Alsa {
        /// Recording duration in seconds.
        duration_secs: u32,
        /// ALSA device name (default: `"default"`).
        device: String,
    },
    /// Record from a macOS microphone using `sox`.
    Sox {
        /// Recording duration in seconds.
        duration_secs: u32,
    },
    /// Record from any system using `ffmpeg`.
    Ffmpeg {
        /// Recording duration in seconds.
        duration_secs: u32,
        /// Input device string passed to ffmpeg (e.g. `"default"`, `"hw:0,0"`).
        device: String,
    },
}

impl Default for MicrophoneSource {
    fn default() -> Self {
        MicrophoneSource::Alsa {
            duration_secs: 5,
            device: "default".to_string(),
        }
    }
}

/// TTS voice selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TtsVoice {
    Alloy,
    Echo,
    Fable,
    Onyx,
    #[default]
    Nova,
    Shimmer,
}

impl TtsVoice {
    fn as_str(&self) -> &'static str {
        match self {
            TtsVoice::Alloy => "alloy",
            TtsVoice::Echo => "echo",
            TtsVoice::Fable => "fable",
            TtsVoice::Onyx => "onyx",
            TtsVoice::Nova => "nova",
            TtsVoice::Shimmer => "shimmer",
        }
    }
}

/// Configuration for the audio pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPipelineConfig {
    /// Microphone / audio source.
    pub source: MicrophoneSource,
    /// OpenAI API base URL (used for both Whisper and TTS).
    pub api_base: String,
    /// Whisper model to use for transcription.
    pub stt_model: String,
    /// Optional ISO-639-1 language code hint for Whisper.
    pub language: Option<String>,
    /// TTS model (`"tts-1"` or `"tts-1-hd"`).
    pub tts_model: String,
    /// TTS voice.
    pub tts_voice: TtsVoice,
    /// Directory where recorded audio and TTS output are stored.
    pub audio_dir: PathBuf,
}

impl Default for AudioPipelineConfig {
    fn default() -> Self {
        Self {
            source: MicrophoneSource::default(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            stt_model: "whisper-1".to_string(),
            language: None,
            tts_model: "tts-1".to_string(),
            tts_voice: TtsVoice::Nova,
            audio_dir: default_audio_dir(),
        }
    }
}

fn default_audio_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("oh-ben-claw")
                .join("audio")
        })
        .unwrap_or_else(|_| PathBuf::from("/tmp/oh-ben-claw/audio"))
}

// ── Pipeline steps ────────────────────────────────────────────────────────────

/// Result from the speech-to-text step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    /// Transcribed text.
    pub text: String,
    /// Path to the source audio file.
    pub audio_path: PathBuf,
    /// Detected or specified language code, if available.
    pub language: Option<String>,
}

/// Result from the text-to-speech step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisResult {
    /// Path to the generated audio file.
    pub audio_path: PathBuf,
    /// Number of bytes written.
    pub size_bytes: usize,
    /// Voice used for synthesis.
    pub voice: String,
}

/// The combined output of a full pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPipelineResult {
    /// Transcribed input text.
    pub transcript: String,
    /// Agent response text.
    pub response: String,
    /// Path to the synthesised response audio.
    pub audio_path: Option<PathBuf>,
}

// ── Audio Pipeline ────────────────────────────────────────────────────────────

/// End-to-end audio pipeline.
pub struct AudioPipeline {
    pub config: AudioPipelineConfig,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl AudioPipeline {
    /// Create a new pipeline from the given configuration.
    ///
    /// Reads `OPENAI_API_KEY` from the environment if present.
    pub fn new(config: AudioPipelineConfig) -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").ok();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self {
            config,
            api_key,
            client,
        }
    }

    /// Record audio from the configured microphone source.
    ///
    /// Returns the path to the recorded audio file.
    pub async fn record(&self) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.config.audio_dir)?;
        let now = unix_now();
        let out = self.config.audio_dir.join(format!("rec_{now}.wav"));

        match &self.config.source {
            MicrophoneSource::File { path } => Ok(PathBuf::from(path)),
            MicrophoneSource::Alsa {
                duration_secs,
                device,
            } => record_alsa(*duration_secs, device, &out).await,
            MicrophoneSource::Sox { duration_secs } => record_sox(*duration_secs, &out).await,
            MicrophoneSource::Ffmpeg {
                duration_secs,
                device,
            } => record_ffmpeg(*duration_secs, device, &out).await,
        }
    }

    /// Transcribe an audio file using the Whisper API.
    pub async fn transcribe(&self, audio_path: &PathBuf) -> anyhow::Result<TranscriptionResult> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("OPENAI_API_KEY not set"))?;

        let file_bytes = std::fs::read(audio_path)?;
        let file_name = audio_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.wav")
            .to_string();

        let mime = mime_for_audio_ext(
            audio_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("wav"),
        );

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str(mime)
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.config.stt_model.clone())
            .text("response_format", "text");

        if let Some(lang) = &self.config.language {
            form = form.text("language", lang.clone());
        }

        let url = format!("{}/audio/transcriptions", self.config.api_base);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Whisper API error {status}: {body}");
        }

        let text = resp.text().await?.trim().to_string();

        Ok(TranscriptionResult {
            text,
            audio_path: audio_path.clone(),
            language: self.config.language.clone(),
        })
    }

    /// Synthesise text to speech and save it to an audio file.
    ///
    /// Returns the path to the generated audio file.
    pub async fn synthesise(&self, text: &str) -> anyhow::Result<SynthesisResult> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("OPENAI_API_KEY not set"))?;

        std::fs::create_dir_all(&self.config.audio_dir)?;
        let now = unix_now();
        let out = self.config.audio_dir.join(format!("tts_{now}.mp3"));

        let body = json!({
            "model": self.config.tts_model,
            "input": text,
            "voice": self.config.tts_voice.as_str(),
            "response_format": "mp3"
        });

        let url = format!("{}/audio/speech", self.config.api_base);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("TTS API error {status}: {body_text}");
        }

        let bytes = resp.bytes().await?;
        let size_bytes = bytes.len();
        std::fs::write(&out, &bytes)?;

        Ok(SynthesisResult {
            audio_path: out,
            size_bytes,
            voice: self.config.tts_voice.as_str().to_string(),
        })
    }
}

// ── Recording helpers ─────────────────────────────────────────────────────────

async fn record_alsa(duration_secs: u32, device: &str, out: &PathBuf) -> anyhow::Result<PathBuf> {
    let status = tokio::process::Command::new("arecord")
        .args([
            "-D",
            device,
            "-d",
            &duration_secs.to_string(),
            "-f",
            "cd",
            "-t",
            "wav",
            out.to_str().unwrap_or("/tmp/rec.wav"),
        ])
        .status()
        .await;

    match status {
        Ok(s) if s.success() => Ok(out.clone()),
        Ok(s) => anyhow::bail!("arecord exited with status {s}"),
        Err(e) => anyhow::bail!("Failed to run arecord: {e}"),
    }
}

async fn record_sox(duration_secs: u32, out: &PathBuf) -> anyhow::Result<PathBuf> {
    let status = tokio::process::Command::new("sox")
        .args([
            "-d",
            "-r",
            "44100",
            "-c",
            "1",
            out.to_str().unwrap_or("/tmp/rec.wav"),
            "trim",
            "0",
            &duration_secs.to_string(),
        ])
        .status()
        .await;

    match status {
        Ok(s) if s.success() => Ok(out.clone()),
        Ok(s) => anyhow::bail!("sox exited with status {s}"),
        Err(e) => anyhow::bail!("Failed to run sox: {e}"),
    }
}

async fn record_ffmpeg(
    duration_secs: u32,
    device: &str,
    out: &PathBuf,
) -> anyhow::Result<PathBuf> {
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "alsa",
            "-i",
            device,
            "-t",
            &duration_secs.to_string(),
            out.to_str().unwrap_or("/tmp/rec.wav"),
        ])
        .status()
        .await;

    match status {
        Ok(s) if s.success() => Ok(out.clone()),
        Ok(s) => anyhow::bail!("ffmpeg exited with status {s}"),
        Err(e) => anyhow::bail!("Failed to run ffmpeg: {e}"),
    }
}

fn mime_for_audio_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "mp4" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "webm" => "audio/webm",
        "m4a" => "audio/mp4",
        _ => "audio/wav",
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── AudioPipelineTool ─────────────────────────────────────────────────────────

/// An agent tool that transcribes an audio file or runs the full audio
/// pipeline (record → transcribe → respond → synthesise).
pub struct AudioPipelineTool {
    pipeline: AudioPipeline,
}

impl AudioPipelineTool {
    /// Create with explicit configuration.
    pub fn new(config: AudioPipelineConfig) -> Self {
        Self {
            pipeline: AudioPipeline::new(config),
        }
    }

    /// Create with default configuration (reads API key from env).
    pub fn default_config() -> Self {
        Self::new(AudioPipelineConfig::default())
    }
}

#[async_trait]
impl Tool for AudioPipelineTool {
    fn name(&self) -> &str {
        "audio_pipeline"
    }

    fn description(&self) -> &str {
        "Transcribe an audio file to text using OpenAI Whisper, and optionally convert \
        a response text back to speech (TTS). Use 'transcribe' action to convert audio \
        to text, or 'synthesise' action to convert text to audio."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["transcribe", "synthesise"],
                    "description": "Action to perform: 'transcribe' converts audio to text; 'synthesise' converts text to audio."
                },
                "path": {
                    "type": "string",
                    "description": "Path to the audio file to transcribe (required for 'transcribe' action)."
                },
                "text": {
                    "type": "string",
                    "description": "Text to convert to speech (required for 'synthesise' action)."
                },
                "language": {
                    "type": "string",
                    "description": "ISO-639-1 language code for transcription (e.g. 'en', 'es'). Auto-detected if omitted."
                },
                "voice": {
                    "type": "string",
                    "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                    "description": "Voice for speech synthesis (default: nova)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: action")),
        };

        match action.as_str() {
            "transcribe" => {
                let path = match args.get("path").and_then(|v| v.as_str()) {
                    Some(p) => PathBuf::from(p),
                    None => return Ok(ToolResult::err("Missing required argument: path for transcribe action")),
                };
                if !path.exists() {
                    return Ok(ToolResult::err(format!(
                        "Audio file not found: {}",
                        path.display()
                    )));
                }

                // Apply per-call language override if provided
                let pipeline = if let Some(lang) = args.get("language").and_then(|v| v.as_str()) {
                    let mut cfg = self.pipeline.config.clone();
                    cfg.language = Some(lang.to_string());
                    AudioPipeline::new(cfg)
                } else {
                    AudioPipeline::new(self.pipeline.config.clone())
                };

                match pipeline.transcribe(&path).await {
                    Ok(r) => Ok(ToolResult::ok(r.text)),
                    Err(e) => Ok(ToolResult::err(format!("Transcription error: {e}"))),
                }
            }

            "synthesise" | "synthesize" => {
                let text = match args.get("text").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(),
                    None => return Ok(ToolResult::err("Missing required argument: text for synthesise action")),
                };

                // Apply per-call voice override if provided
                let pipeline = if let Some(voice_str) = args.get("voice").and_then(|v| v.as_str()) {
                    let voice = match voice_str {
                        "alloy" => TtsVoice::Alloy,
                        "echo" => TtsVoice::Echo,
                        "fable" => TtsVoice::Fable,
                        "onyx" => TtsVoice::Onyx,
                        "shimmer" => TtsVoice::Shimmer,
                        _ => TtsVoice::Nova,
                    };
                    let mut cfg = self.pipeline.config.clone();
                    cfg.tts_voice = voice;
                    AudioPipeline::new(cfg)
                } else {
                    AudioPipeline::new(self.pipeline.config.clone())
                };

                match pipeline.synthesise(&text).await {
                    Ok(r) => Ok(ToolResult::ok(format!(
                        "Speech synthesised successfully.\nFile: {}\nSize: {} bytes\nVoice: {}",
                        r.audio_path.display(),
                        r.size_bytes,
                        r.voice
                    ))),
                    Err(e) => Ok(ToolResult::err(format!("TTS error: {e}"))),
                }
            }

            other => Ok(ToolResult::err(format!(
                "Unknown action '{other}'. Use 'transcribe' or 'synthesise'."
            ))),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_voice_as_str() {
        assert_eq!(TtsVoice::Alloy.as_str(), "alloy");
        assert_eq!(TtsVoice::Nova.as_str(), "nova");
        assert_eq!(TtsVoice::Shimmer.as_str(), "shimmer");
    }

    #[test]
    fn mime_for_audio_ext_coverage() {
        assert_eq!(mime_for_audio_ext("mp3"), "audio/mpeg");
        assert_eq!(mime_for_audio_ext("wav"), "audio/wav");
        assert_eq!(mime_for_audio_ext("flac"), "audio/flac");
        assert_eq!(mime_for_audio_ext("ogg"), "audio/ogg");
        assert_eq!(mime_for_audio_ext("webm"), "audio/webm");
        assert_eq!(mime_for_audio_ext("m4a"), "audio/mp4");
        assert_eq!(mime_for_audio_ext("xyz"), "audio/wav");
    }

    #[test]
    fn default_config_model_is_whisper() {
        let cfg = AudioPipelineConfig::default();
        assert_eq!(cfg.stt_model, "whisper-1");
        assert_eq!(cfg.tts_model, "tts-1");
        assert_eq!(cfg.tts_voice, TtsVoice::Nova);
    }

    #[test]
    fn audio_pipeline_tool_name() {
        let tool = AudioPipelineTool::default_config();
        assert_eq!(tool.name(), "audio_pipeline");
    }

    #[test]
    fn audio_pipeline_tool_schema_has_action() {
        let tool = AudioPipelineTool::default_config();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("action")));
    }

    #[tokio::test]
    async fn tool_returns_error_on_missing_action() {
        let tool = AudioPipelineTool::default_config();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("action"));
    }

    #[tokio::test]
    async fn tool_returns_error_on_missing_path_for_transcribe() {
        let tool = AudioPipelineTool::default_config();
        let result = tool
            .execute(json!({"action": "transcribe"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn tool_returns_error_on_nonexistent_audio_file() {
        let tool = AudioPipelineTool::default_config();
        let result = tool
            .execute(json!({"action": "transcribe", "path": "/nonexistent/audio.wav"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("not found"));
    }

    #[tokio::test]
    async fn tool_returns_error_on_missing_text_for_synthesise() {
        let tool = AudioPipelineTool::default_config();
        let result = tool
            .execute(json!({"action": "synthesise"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn tool_returns_error_on_unknown_action() {
        let tool = AudioPipelineTool::default_config();
        let result = tool
            .execute(json!({"action": "unknown_action"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("Unknown action"));
    }
}

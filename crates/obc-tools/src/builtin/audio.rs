//! Audio transcription and speech tools.
//!
//! Supports:
//! - OpenAI Whisper API (whisper-1, whisper-large-v3)
//! - Local whisper.cpp via subprocess
//! - Audio recording from peripheral microphones via the MQTT Spine
//! - A local Wyoming server ahead of all of those, when `[voice]` names one
//!   (plan P5): [`stt_plan`] and [`tts_plan`] decide the order

use crate::traits::{Tool, ToolResult};
use async_trait::async_trait;
use obc_audio::suite::{AudioController, HeardEvent};
use obc_audio::wav;
use obc_audio::wyoming::WyomingClient;
use obc_memory::world::Origin;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

// ── Backend selection (plan P5) ───────────────────────────────────────────────

/// Where a speech request can be served, in the order a plan tries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechBackend {
    /// A local Wyoming server (`[voice] stt` / `tts`).
    Wyoming,
    /// whisper.cpp on this host (`use_local = true`).
    WhisperCpp,
    /// The OpenAI-compatible API at `OPENAI_API_BASE`.
    Cloud,
}

impl SpeechBackend {
    pub fn label(self) -> &'static str {
        match self {
            SpeechBackend::Wyoming => "on-device (Wyoming)",
            SpeechBackend::WhisperCpp => "whisper.cpp",
            SpeechBackend::Cloud => "cloud",
        }
    }
}

/// The backends `audio_transcribe` tries, in order. The Wyoming server takes
/// PCM WAV only (there is no decoder here), so other audio skips it.
/// `use_local` asks for whisper.cpp and never reaches the cloud, as before.
/// Empty when nothing may serve the request.
pub fn stt_plan(
    wyoming: bool,
    is_wav: bool,
    use_local: bool,
    cloud_fallback: bool,
) -> Vec<SpeechBackend> {
    let mut plan = Vec::new();
    if wyoming && is_wav {
        plan.push(SpeechBackend::Wyoming);
    }
    if use_local {
        plan.push(SpeechBackend::WhisperCpp);
    } else if !wyoming || cloud_fallback {
        plan.push(SpeechBackend::Cloud);
    }
    plan
}

/// The backends `text_to_speech` tries, and the format it writes. The Wyoming
/// server renders WAV, so on-device the default format is `wav` and any other
/// format is the cloud's. Empty when nothing may serve the request.
pub fn tts_plan(
    wyoming: bool,
    format: Option<&str>,
    cloud_fallback: bool,
) -> (Vec<SpeechBackend>, &str) {
    let cloud = if cloud_fallback {
        vec![SpeechBackend::Cloud]
    } else {
        Vec::new()
    };
    match (wyoming, format) {
        (true, None | Some("wav")) => {
            let mut plan = vec![SpeechBackend::Wyoming];
            plan.extend(cloud);
            (plan, "wav")
        }
        (true, Some(format)) => (cloud, format),
        (false, format) => (vec![SpeechBackend::Cloud], format.unwrap_or("mp3")),
    }
}

/// Every backend in a plan failed: one failure keeps its own message, several
/// are listed in the order tried.
fn all_failed(failures: Vec<(SpeechBackend, String)>) -> ToolResult {
    if let [(_, only)] = failures.as_slice() {
        return ToolResult::err(only.clone());
    }
    ToolResult::err(
        failures
            .iter()
            .map(|(backend, e)| format!("{}: {e}", backend.label()))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Transcription Tool ────────────────────────────────────────────────────────

/// Tool for transcribing audio files to text using Whisper.
pub struct AudioTranscribeTool {
    pub api_key: Option<String>,
    pub api_base: String,
    pub model: String,
    wyoming: Option<WyomingClient>,
    cloud_fallback: bool,
    recorder: Option<(Arc<AudioController>, String)>,
}

impl Default for AudioTranscribeTool {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            model: "whisper-1".to_string(),
            wyoming: None,
            cloud_fallback: true,
            recorder: None,
        }
    }
}

impl AudioTranscribeTool {
    /// Try a Wyoming STT server first (`[voice] stt`). With `cloud_fallback`
    /// false a failed or unsuitable call fails instead of reaching the API.
    pub fn with_wyoming(mut self, client: WyomingClient, cloud_fallback: bool) -> Self {
        self.wyoming = Some(client);
        self.cloud_fallback = cloud_fallback;
        self
    }

    /// Record each transcript as an `audio.{stream}` event, so `hear` sees it.
    pub fn with_recorder(mut self, audio: Arc<AudioController>, stream: impl Into<String>) -> Self {
        self.recorder = Some((audio, stream.into()));
        self
    }

    fn record(&self, text: &str, backend: SpeechBackend) {
        let Some((audio, stream)) = &self.recorder else {
            return;
        };
        let heard = HeardEvent {
            stream: stream.clone(),
            text: Some(text.trim().to_string()),
            label: None,
            confidence: 1.0,
            source: Some(format!("audio_transcribe: {}", backend.label())),
        };
        if let Err(e) = audio.observe(&heard, now_ms(), Origin::Observed) {
            tracing::warn!(error = %e, "transcript not recorded");
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
            return Ok(ToolResult::err(format!(
                "Audio file not found: {}",
                file_path.display()
            )));
        }

        let use_local = args
            .get("use_local")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Read only when there is a Wyoming server to send it to; the other
        // backends take the path.
        let audio = match &self.wyoming {
            Some(_) => match std::fs::read(&file_path) {
                Ok(bytes) => Some(bytes),
                Err(e) => return Ok(ToolResult::err(format!("Failed to read audio file: {e}"))),
            },
            None => None,
        };
        let is_wav = audio.as_deref().is_some_and(wav::is_wav);
        let plan = stt_plan(
            self.wyoming.is_some(),
            is_wav,
            use_local,
            self.cloud_fallback,
        );
        if plan.is_empty() {
            return Ok(ToolResult::err(
                "The on-device STT server takes PCM WAV only and voice.cloud_fallback is off; \
                convert the audio to WAV or pass use_local=true for whisper.cpp.",
            ));
        }

        let mut failures = Vec::new();
        for backend in plan {
            let result = match backend {
                SpeechBackend::Wyoming => {
                    let language = args.get("language").and_then(|v| v.as_str());
                    transcribe_wyoming(
                        self.wyoming.as_ref().expect("planned only with a client"),
                        audio.as_deref().unwrap_or_default(),
                        language,
                    )
                    .await
                }
                SpeechBackend::WhisperCpp => transcribe_local(&file_path, &args).await?,
                // Use OpenAI Whisper API
                SpeechBackend::Cloud => match &self.api_key {
                    Some(api_key) => {
                        transcribe_openai(&file_path, api_key, &self.api_base, &self.model, &args)
                            .await?
                    }
                    None => ToolResult::err(
                        "OPENAI_API_KEY not set. Use use_local=true for local whisper.cpp transcription.",
                    ),
                },
            };
            if result.success {
                if !failures.is_empty() {
                    tracing::warn!(
                        backend = backend.label(),
                        ?failures,
                        "transcribed after a fallback"
                    );
                }
                // Timestamped output is the API's JSON, not an utterance.
                if !args
                    .get("timestamps")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    self.record(&result.output, backend);
                }
                return Ok(result);
            }
            failures.push((backend, result.error.unwrap_or_default()));
        }
        Ok(all_failed(failures))
    }
}

async fn transcribe_wyoming(
    client: &WyomingClient,
    audio: &[u8],
    language: Option<&str>,
) -> ToolResult {
    let pcm = match wav::parse(audio) {
        Ok(pcm) => pcm,
        Err(e) => return ToolResult::err(format!("Not a WAV the STT server can take: {e}")),
    };
    match client.transcribe(&pcm, language).await {
        Ok(text) => ToolResult::ok(text),
        Err(e) => ToolResult::err(format!("{e:#}")),
    }
}

async fn transcribe_openai(
    file_path: &Path,
    api_key: &str,
    api_base: &str,
    model: &str,
    args: &Value,
) -> anyhow::Result<ToolResult> {
    let language = args.get("language").and_then(|v| v.as_str());
    let prompt = args.get("prompt").and_then(|v| v.as_str());
    let timestamps = args
        .get("timestamps")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let file_bytes = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(e) => return Ok(ToolResult::err(format!("Failed to read audio file: {e}"))),
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
        Err(e) => return Ok(ToolResult::err(format!("Failed to build HTTP client: {e}"))),
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
            Ok(ToolResult::err(format!(
                "Whisper API error {status}: {body}"
            )))
        }
        Err(e) => Ok(ToolResult::err(format!("HTTP request failed: {e}"))),
    }
}

async fn transcribe_local(file_path: &Path, args: &Value) -> anyhow::Result<ToolResult> {
    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    // Try whisper.cpp CLI
    let cmd_args = vec![
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
            Ok(ToolResult::err(format!("whisper.cpp failed: {stderr}")))
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
                    or set use_local=false to use the OpenAI Whisper API.",
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
    wyoming: Option<WyomingClient>,
    wyoming_voice: Option<String>,
    cloud_fallback: bool,
}

impl Default for TextToSpeechTool {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            wyoming: None,
            wyoming_voice: None,
            cloud_fallback: true,
        }
    }
}

impl TextToSpeechTool {
    /// Try a Wyoming TTS server first (`[voice] tts`), asking for `voice` (the
    /// server's default when `None`). The call's own `voice` names a cloud
    /// voice and is not sent there.
    pub fn with_wyoming(
        mut self,
        client: WyomingClient,
        voice: Option<String>,
        cloud_fallback: bool,
    ) -> Self {
        self.wyoming = Some(client);
        self.wyoming_voice = voice;
        self.cloud_fallback = cloud_fallback;
        self
    }

    /// The format a call that names none is written in: `wav` on-device,
    /// `mp3` from the cloud.
    pub fn default_format(&self) -> &'static str {
        tts_plan(self.wyoming.is_some(), None, self.cloud_fallback).1
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
                    "description": format!("Path to save the audio file (default: <system temp dir>/obc_tts_<timestamp>.{})", self.default_format())
                },
                "format": {
                    "type": "string",
                    "enum": ["mp3", "opus", "aac", "flac", "wav"],
                    "description": "Output audio format",
                    "default": self.default_format()
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

        let voice = args.get("voice").and_then(|v| v.as_str()).unwrap_or("nova");
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("tts-1");
        let (plan, format) = tts_plan(
            self.wyoming.is_some(),
            args.get("format").and_then(|v| v.as_str()),
            self.cloud_fallback,
        );
        if plan.is_empty() {
            return Ok(ToolResult::err(format!(
                "The on-device TTS server renders WAV only and voice.cloud_fallback is off; \
                ask for format \"wav\" instead of \"{format}\"."
            )));
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // The platform temp dir, not `/tmp`: on Windows `/tmp` does not exist
        // and the save failed unless the model passed `output_path` itself.
        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join(format!("obc_tts_{timestamp}.{format}"))
                    .to_string_lossy()
                    .into_owned()
            });

        let mut failures = Vec::new();
        for backend in plan {
            let result = match backend {
                SpeechBackend::Wyoming => {
                    let client = self.wyoming.as_ref().expect("planned only with a client");
                    synthesize_wyoming(client, &text, self.wyoming_voice.as_deref(), &output_path)
                        .await
                }
                SpeechBackend::Cloud => match &self.api_key {
                    Some(api_key) => {
                        let cloud = CloudSpeech {
                            api_base: &self.api_base,
                            api_key,
                            voice,
                            model,
                            format,
                        };
                        cloud.synthesize(&text, &output_path).await
                    }
                    None => ToolResult::err("OPENAI_API_KEY not set"),
                },
                SpeechBackend::WhisperCpp => unreachable!("not a TTS backend"),
            };
            if result.success {
                if !failures.is_empty() {
                    tracing::warn!(
                        backend = backend.label(),
                        ?failures,
                        "speech rendered after a fallback"
                    );
                }
                return Ok(result);
            }
            failures.push((backend, result.error.unwrap_or_default()));
        }
        Ok(all_failed(failures))
    }
}

fn speech_written(output_path: &str, bytes: &[u8], voice: &str, model: &str) -> ToolResult {
    match std::fs::write(output_path, bytes) {
        Ok(_) => ToolResult::ok(format!(
            "Speech generated successfully.\n\
            File: {output_path}\n\
            Size: {} bytes\n\
            Voice: {voice}\n\
            Model: {model}",
            bytes.len()
        )),
        Err(e) => ToolResult::err(format!("Failed to save audio file: {e}")),
    }
}

async fn synthesize_wyoming(
    client: &WyomingClient,
    text: &str,
    voice: Option<&str>,
    output_path: &str,
) -> ToolResult {
    let pcm = match client.synthesize(text, voice).await {
        Ok(pcm) => pcm,
        Err(e) => return ToolResult::err(format!("{e:#}")),
    };
    match wav::encode(&pcm) {
        Ok(bytes) => speech_written(
            output_path,
            &bytes,
            voice.unwrap_or("server default"),
            SpeechBackend::Wyoming.label(),
        ),
        Err(e) => ToolResult::err(format!("TTS server audio not writable as WAV: {e}")),
    }
}

/// One call to the OpenAI-compatible speech endpoint.
struct CloudSpeech<'a> {
    api_base: &'a str,
    api_key: &'a str,
    voice: &'a str,
    model: &'a str,
    format: &'a str,
}

impl CloudSpeech<'_> {
    async fn synthesize(&self, text: &str, output_path: &str) -> ToolResult {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("Failed to build HTTP client: {e}")),
        };

        let body = json!({
            "model": self.model,
            "input": text,
            "voice": self.voice,
            "response_format": self.format
        });

        let url = format!("{}/audio/speech", self.api_base);
        let resp = client
            .post(&url)
            .bearer_auth(self.api_key)
            .json(&body)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().await.unwrap_or_default();
                speech_written(output_path, &bytes, self.voice, self.model)
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                ToolResult::err(format!("TTS API error {status}: {body}"))
            }
            Err(e) => ToolResult::err(format!("HTTP request failed: {e}")),
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
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("file_path"));
    }

    #[tokio::test]
    async fn test_transcribe_nonexistent_file() {
        let tool = AudioTranscribeTool::default();
        let result = tool
            .execute(json!({"file_path": "/nonexistent/audio.mp3"}))
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
    async fn test_tts_missing_text() {
        let tool = TextToSpeechTool::default();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("text"));
    }

    #[tokio::test]
    async fn test_tts_text_too_long() {
        let tool = TextToSpeechTool::default();
        let long_text = "a".repeat(5000);
        let result = tool.execute(json!({"text": long_text})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("4096"));
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
        assert_eq!(schema["properties"]["format"]["default"], "mp3");
    }

    // ── Plan P5: on-device first, cloud as the fallback ─────────────────────

    use obc_audio::suite::LoggingSpeechSink;
    use obc_audio::wyoming::{read_event, write_event, Event};
    use obc_memory::world::WorldMemory;
    use SpeechBackend::{Cloud, WhisperCpp, Wyoming};

    #[test]
    fn plans_put_the_wyoming_server_first_and_the_cloud_last() {
        // (wyoming, wav, use_local, cloud_fallback)
        assert_eq!(stt_plan(false, true, false, true), [Cloud]);
        assert_eq!(stt_plan(false, true, true, true), [WhisperCpp]);
        assert_eq!(stt_plan(true, true, false, true), [Wyoming, Cloud]);
        assert_eq!(stt_plan(true, true, false, false), [Wyoming]);
        assert_eq!(stt_plan(true, false, false, true), [Cloud]);
        assert!(stt_plan(true, false, false, false).is_empty());
        // use_local never reaches the cloud, with or without a server.
        assert_eq!(stt_plan(true, true, true, true), [Wyoming, WhisperCpp]);

        assert_eq!(tts_plan(false, None, true), (vec![Cloud], "mp3"));
        assert_eq!(tts_plan(false, Some("wav"), false), (vec![Cloud], "wav"));
        assert_eq!(tts_plan(true, None, true), (vec![Wyoming, Cloud], "wav"));
        assert_eq!(tts_plan(true, Some("wav"), false), (vec![Wyoming], "wav"));
        assert_eq!(tts_plan(true, Some("opus"), true), (vec![Cloud], "opus"));
        assert_eq!(tts_plan(true, Some("opus"), false), (vec![], "opus"));
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("obc_p5_{}_{name}", std::process::id()))
    }

    fn silence_wav(path: &Path) {
        let pcm = wav::Pcm {
            rate: 16_000,
            width: 2,
            channels: 1,
            data: vec![0; 3_200],
        };
        std::fs::write(path, wav::encode(&pcm).unwrap()).unwrap();
    }

    /// A one-connection Wyoming server on loopback: a transcript for STT, a
    /// tenth of a second of silence for TTS.
    async fn fake_wyoming() -> WyomingClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = tokio::io::BufReader::new(read);
            while let Some(event) = read_event(&mut read).await.unwrap() {
                let format = json!({ "rate": 16_000, "width": 2, "channels": 1 });
                let answer = match event.kind.as_str() {
                    "audio-stop" => vec![Event::new(
                        "transcript",
                        json!({ "text": " porch light on " }),
                    )],
                    "synthesize" => vec![
                        Event::new("audio-start", format.clone()),
                        Event::new("audio-chunk", format).with_payload(vec![0; 3_200]),
                        Event::new("audio-stop", json!({})),
                    ],
                    _ => continue,
                };
                for event in answer {
                    write_event(&mut write, &event).await.unwrap();
                }
                break;
            }
        });
        WyomingClient::new(&addr, Duration::from_secs(5)).unwrap()
    }

    /// A port with nothing listening on it.
    async fn dead_wyoming() -> WyomingClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        WyomingClient::new(&addr, Duration::from_millis(500)).unwrap()
    }

    fn no_cloud_stt() -> AudioTranscribeTool {
        AudioTranscribeTool {
            api_key: None,
            ..AudioTranscribeTool::default()
        }
    }

    #[tokio::test]
    async fn an_on_device_transcript_is_returned_and_recorded() {
        let path = temp_path("heard.wav");
        silence_wav(&path);
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let audio =
            AudioController::new(Arc::new(LoggingSpeechSink)).with_world_memory(Arc::clone(&world));
        let tool = no_cloud_stt()
            .with_wyoming(fake_wyoming().await, false)
            .with_recorder(Arc::new(audio), "mic0");

        let result = tool.execute(json!({ "file_path": path })).await.unwrap();
        std::fs::remove_file(&path).ok();
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output, "porch light on");
        let fact = world.current("audio.mic0").unwrap().unwrap();
        assert_eq!(fact.value["text"], "porch light on");
    }

    #[tokio::test]
    async fn a_failed_server_falls_back_to_the_cloud_only_when_allowed() {
        let path = temp_path("fallback.wav");
        silence_wav(&path);
        let args = json!({ "file_path": path });

        let tool = no_cloud_stt().with_wyoming(dead_wyoming().await, true);
        let error = tool.execute(args.clone()).await.unwrap().error.unwrap();
        assert!(error.starts_with("on-device (Wyoming): "), "{error}");
        assert!(error.contains("; cloud: OPENAI_API_KEY not set"), "{error}");

        let tool = no_cloud_stt().with_wyoming(dead_wyoming().await, false);
        let error = tool.execute(args).await.unwrap().error.unwrap();
        std::fs::remove_file(&path).ok();
        assert!(error.contains("Wyoming STT server"), "{error}");
        assert!(!error.contains("OPENAI_API_KEY"), "{error}");
    }

    #[tokio::test]
    async fn audio_the_server_cannot_take_stays_off_the_cloud_without_fallback() {
        let path = temp_path("heard.mp3");
        std::fs::write(&path, b"ID3 not a wav").unwrap();
        let tool = no_cloud_stt().with_wyoming(dead_wyoming().await, false);
        let error = tool
            .execute(json!({ "file_path": path }))
            .await
            .unwrap()
            .error
            .unwrap();
        std::fs::remove_file(&path).ok();
        assert!(error.contains("PCM WAV only"), "{error}");
    }

    #[tokio::test]
    async fn on_device_speech_is_written_as_wav() {
        let path = temp_path("said.wav");
        let tool = TextToSpeechTool {
            api_key: None,
            ..TextToSpeechTool::default()
        }
        .with_wyoming(
            fake_wyoming().await,
            Some("en_US-lessac-medium".into()),
            false,
        );
        assert_eq!(tool.default_format(), "wav");

        let result = tool
            .execute(json!({ "text": "Porch light on.", "output_path": path }))
            .await
            .unwrap();
        let written = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(result.success, "{:?}", result.error);
        assert!(
            result.output.contains("Model: on-device (Wyoming)"),
            "{}",
            result.output
        );
        assert!(wav::is_wav(&written));
        assert_eq!(wav::parse(&written).unwrap().data.len(), 3_200);

        // Another format is the cloud's, and the cloud is off.
        let error = tool
            .execute(json!({ "text": "Porch light on.", "format": "mp3" }))
            .await
            .unwrap()
            .error
            .unwrap();
        assert!(error.contains("WAV only"), "{error}");
    }
}

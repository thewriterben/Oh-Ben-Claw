//! The tool layer's speech sink — utterances rendered to local audio files.
//!
//! This lived in `src/audio/suite.rs` until 2026-08-13, holding a
//! `TextToSpeechTool`. Same move as [`obc_spine::speech`] the same day, and
//! the same rule: the `SpeechSink` trait stays in `audio::suite` where the
//! abstraction is, and an implementation that needs the tool layer lives in the
//! tool layer.
//!
//! Between the two of them they were `audio`'s last edges into this tree. A
//! 389-line module that describes hearing and speaking had been carrying a
//! reference to a 8802-line tool module and to the spine, in two struct fields.
//!
//! Best-effort by construction: with no `OPENAI_API_KEY` configured, or any
//! render error, the utterance is logged and skipped. `speak` never errors, so
//! a reflex or agent that spoke is never broken by a missing renderer.

use crate::tools::builtin::audio::TextToSpeechTool;
use async_trait::async_trait;
use obc_audio::suite::{SpeechSink, Utterance};
use serde_json::json;

/// Renders utterances to local audio files via the OpenAI TTS tool.
pub struct TtsSpeechSink {
    tts: TextToSpeechTool,
    out_dir: String,
}

impl TtsSpeechSink {
    /// Render into `out_dir` (created on demand by the TTS tool's file write).
    pub fn new(out_dir: impl Into<String>) -> Self {
        Self {
            tts: TextToSpeechTool::default(),
            out_dir: out_dir.into(),
        }
    }

    /// The output file path for an utterance at `at_ms`.
    pub fn out_path(&self, at_ms: u64) -> String {
        format!(
            "{}/obc_tts_{}.mp3",
            self.out_dir.trim_end_matches('/'),
            at_ms
        )
    }
}

#[async_trait]
impl SpeechSink for TtsSpeechSink {
    async fn speak(&self, u: &Utterance) -> anyhow::Result<()> {
        use obc_tool_api::Tool;
        let path = self.out_path(u.at_ms);
        let args = json!({ "text": u.text, "voice": u.voice, "output_path": path });
        match self.tts.execute(args).await {
            Ok(res) if res.success => tracing::info!(path = %path, "rendered speech via TTS"),
            Ok(res) => tracing::warn!(error = ?res.error, "TTS render skipped (best-effort)"),
            Err(e) => tracing::warn!(error = %e, "TTS render failed (best-effort)"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_sink_out_path_is_stable() {
        let sink = TtsSpeechSink::new("/tmp/obc/");
        assert_eq!(sink.out_path(42), "/tmp/obc/obc_tts_42.mp3"); // trailing slash trimmed
        assert_eq!(
            TtsSpeechSink::new("/var/audio").out_path(7),
            "/var/audio/obc_tts_7.mp3"
        );
    }
}

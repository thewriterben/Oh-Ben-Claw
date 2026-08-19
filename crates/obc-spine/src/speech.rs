//! The spine's speech sink — utterances out over MQTT to `obc/speech`.
//!
//! This lived in `src/audio/suite.rs` until 2026-08-13, holding an
//! `Arc<SpineClient>`, and it is the third sink in this repository to make the
//! same journey: `SpineActuatorSink` left `movement` on 2026-08-08,
//! `SpineActionSink` left `reflex` on 2026-08-12.
//!
//! The rule, now stated for the seventh time this month and no longer a
//! coincidence: **the trait stays where the abstraction is, the implementation
//! goes where the dependency is.** `SpeechSink` is declared in `audio::suite`
//! and belongs there — it is the audio suite's own notion of "somewhere an
//! utterance can go". This implementation is a fact about MQTT.
//!
//! Best-effort like the other two: a publish failure is logged, not propagated,
//! so a transient outage never breaks the caller — or a reflex that spoke.

use crate::{SpineClient, TOPIC_PREFIX};
use async_trait::async_trait;
use obc_audio::suite::{SpeechSink, Utterance};
use serde_json::json;
use std::sync::Arc;

/// Emits utterances over the MQTT spine to `obc/speech`, where a speaker node /
/// TTS bridge renders them.
pub struct SpineSpeechSink {
    spine: Arc<SpineClient>,
}

impl SpineSpeechSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<SpineClient>) -> Self {
        Self { spine }
    }
}

#[async_trait]
impl SpeechSink for SpineSpeechSink {
    async fn speak(&self, u: &Utterance) -> anyhow::Result<()> {
        let topic = format!("{TOPIC_PREFIX}/speech");
        let payload = json!({ "text": u.text, "voice": u.voice, "at_ms": u.at_ms });
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(voice = %u.voice, error = %e, "speech publish over spine failed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpineConfig;

    #[tokio::test]
    async fn spine_speech_sink_is_best_effort_when_disconnected() {
        let spine = Arc::new(SpineClient::new(SpineConfig::default(), "test"));
        let sink = SpineSpeechSink::new(spine);
        // An unconnected spine fails the publish, but the sink logs and returns Ok
        // so a reflex/agent that spoke is never broken by a transient outage.
        let u = Utterance {
            text: "hello".into(),
            voice: "nova".into(),
            at_ms: 1,
        };
        assert!(sink.speak(&u).await.is_ok());
    }
}

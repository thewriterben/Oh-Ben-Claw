//! Audio subsystem (first-class suite) — perceive heard events into world memory
//! and act by speaking.
//!
//! Audio is the first suite that is genuinely **both** sides of the loop: like
//! vision/sensing it *perceives* (a transcribed utterance or a detected sound
//! becomes an `audio.{stream}` fact in world memory, classified for reliability
//! against a confidence floor — the §4 *Learn* hook), and like movement it
//! *acts* (an [`Utterance`] is emitted through a [`SpeechSink`] and recorded as
//! `speech.last`). Reflexes already read `audio.*`, so heard events feed System 1
//! directly — e.g. a high-confidence "alarm" sound can trigger a reflex.
//!
//! Recording happens *before* emission (as in movement), so memory reflects
//! intent even if the sink fails.

use crate::memory::world::WorldMemory;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

fn default_confidence() -> f64 {
    1.0
}

/// A perceived audio event — a transcribed utterance and/or a classified sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardEvent {
    /// Input stream id (e.g. `"mic0"`). Becomes `audio.{stream}` in world memory.
    pub stream: String,
    /// Transcribed text, if this was speech.
    #[serde(default)]
    pub text: Option<String>,
    /// Classified sound label, if this was a sound event (e.g. `"alarm"`).
    #[serde(default)]
    pub label: Option<String>,
    /// Recognizer confidence in `0.0..=1.0`.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// Who produced it (recognizer / node).
    #[serde(default)]
    pub source: Option<String>,
}

/// A heard event after reliability classification.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassifiedHeard {
    pub stream: String,
    pub text: Option<String>,
    pub label: Option<String>,
    pub confidence: f64,
    /// `true` when `confidence >= min_confidence` (trustworthy for reflexes).
    pub reliable: bool,
    pub at_ms: u64,
}

/// A spoken utterance (the act side).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Utterance {
    pub text: String,
    pub voice: String,
    pub at_ms: u64,
}

/// What an utterance is emitted through (TTS engine, speaker over the spine, …).
///
/// Mirrors movement's `ActuatorSink`: the controller owns policy (record, then
/// emit), the sink owns the physical channel. The default
/// [`LoggingSpeechSink`] makes audio output a safe dry-run until a real engine
/// is wired.
#[async_trait]
pub trait SpeechSink: Send + Sync {
    async fn speak(&self, utterance: &Utterance) -> anyhow::Result<()>;
}

/// Default dry-run sink — logs the utterance, emits no sound.
pub struct LoggingSpeechSink;

#[async_trait]
impl SpeechSink for LoggingSpeechSink {
    async fn speak(&self, u: &Utterance) -> anyhow::Result<()> {
        tracing::info!(voice = %u.voice, text = %u.text, "[audio dry-run] speak");
        Ok(())
    }
}

/// Emits utterances over the MQTT spine to `obc/speech`, where a speaker node /
/// TTS bridge renders them. Best-effort like the movement spine sink: a publish
/// failure is logged, not propagated, so a transient outage never breaks the
/// caller (or a reflex that spoke).
pub struct SpineSpeechSink {
    spine: Arc<crate::spine::SpineClient>,
}

impl SpineSpeechSink {
    /// Build a sink over a (connected) spine client.
    pub fn new(spine: Arc<crate::spine::SpineClient>) -> Self {
        Self { spine }
    }
}

#[async_trait]
impl SpeechSink for SpineSpeechSink {
    async fn speak(&self, u: &Utterance) -> anyhow::Result<()> {
        let topic = format!("{}/speech", crate::spine::TOPIC_PREFIX);
        let payload = json!({ "text": u.text, "voice": u.voice, "at_ms": u.at_ms });
        if let Err(e) = self.spine.publish(&topic, &payload).await {
            tracing::warn!(voice = %u.voice, error = %e, "speech publish over spine failed");
        }
        Ok(())
    }
}

/// The Audio suite controller: perceive ([`observe`](Self::observe)) and act
/// ([`speak`](Self::speak)), both recorded into world memory.
pub struct AudioController {
    world: Option<Arc<WorldMemory>>,
    sink: Arc<dyn SpeechSink>,
    min_confidence: f64,
    source: String,
}

impl AudioController {
    /// Build a controller over a speech sink (use [`LoggingSpeechSink`] for a
    /// dry run). Confidence floor defaults to `0.5`.
    pub fn new(sink: Arc<dyn SpeechSink>) -> Self {
        Self {
            world: None,
            sink,
            min_confidence: 0.5,
            source: "audio".to_string(),
        }
    }

    /// Record perceived/spoken events into world memory (enables §3 Remember).
    pub fn with_world_memory(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Set the reliability floor for heard events (default `0.5`).
    pub fn with_min_confidence(mut self, c: f64) -> Self {
        self.min_confidence = c;
        self
    }

    /// Override the world-memory `source` label (default `"audio"`).
    pub fn with_source(mut self, s: impl Into<String>) -> Self {
        self.source = s.into();
        self
    }

    /// Perceive: classify reliability and record a heard event into world memory
    /// as `audio.{stream}`.
    pub fn observe(&self, event: &HeardEvent, now_ms: u64) -> anyhow::Result<ClassifiedHeard> {
        let reliable = event.confidence >= self.min_confidence;
        if let Some(world) = &self.world {
            let entity = format!("audio.{}", event.stream);
            let value = json!({
                "text": event.text,
                "label": event.label,
                "confidence": event.confidence,
                "reliable": reliable,
                "source": event.source,
            });
            world.observe(&entity, value, now_ms, now_ms, &self.source)?;
        }
        Ok(ClassifiedHeard {
            stream: event.stream.clone(),
            text: event.text.clone(),
            label: event.label.clone(),
            confidence: event.confidence,
            reliable,
            at_ms: now_ms,
        })
    }

    /// Act: record the intended utterance as `speech.last`, then emit it through
    /// the sink.
    pub async fn speak(
        &self,
        text: impl Into<String>,
        voice: impl Into<String>,
        now_ms: u64,
    ) -> anyhow::Result<Utterance> {
        let utterance = Utterance {
            text: text.into(),
            voice: voice.into(),
            at_ms: now_ms,
        };
        if let Some(world) = &self.world {
            let value = json!({ "text": utterance.text, "voice": utterance.voice });
            world.observe("speech.last", value, now_ms, now_ms, &self.source)?;
        }
        self.sink.speak(&utterance).await?;
        Ok(utterance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> (AudioController, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = AudioController::new(Arc::new(LoggingSpeechSink))
            .with_world_memory(Arc::clone(&world))
            .with_min_confidence(0.6);
        (ctrl, world)
    }

    fn heard(stream: &str, text: Option<&str>, label: Option<&str>, conf: f64) -> HeardEvent {
        HeardEvent {
            stream: stream.to_string(),
            text: text.map(str::to_string),
            label: label.map(str::to_string),
            confidence: conf,
            source: Some("whisper".to_string()),
        }
    }

    #[test]
    fn high_confidence_event_is_reliable_and_recorded() {
        let (ctrl, world) = controller();
        let c = ctrl.observe(&heard("mic0", Some("lights on"), None, 0.92), 1_000).unwrap();
        assert!(c.reliable);
        let fact = world.current("audio.mic0").unwrap().unwrap();
        assert_eq!(fact.value["text"], "lights on");
        assert_eq!(fact.value["reliable"], true);
        assert_eq!(fact.source, "audio");
    }

    #[test]
    fn low_confidence_event_is_unreliable_but_still_recorded() {
        let (ctrl, world) = controller();
        let c = ctrl.observe(&heard("mic0", Some("maybe?"), None, 0.3), 1_000).unwrap();
        assert!(!c.reliable);
        let fact = world.current("audio.mic0").unwrap().unwrap();
        assert_eq!(fact.value["reliable"], false);
    }

    #[test]
    fn sound_event_label_is_recorded() {
        let (ctrl, world) = controller();
        ctrl.observe(&heard("mic0", None, Some("alarm"), 0.99), 1_000).unwrap();
        let fact = world.current("audio.mic0").unwrap().unwrap();
        assert_eq!(fact.value["label"], "alarm");
    }

    #[tokio::test]
    async fn speak_records_speech_last() {
        let (ctrl, world) = controller();
        let u = ctrl.speak("hello there", "nova", 2_000).await.unwrap();
        assert_eq!(u.text, "hello there");
        let fact = world.current("speech.last").unwrap().unwrap();
        assert_eq!(fact.value["text"], "hello there");
        assert_eq!(fact.value["voice"], "nova");
    }

    #[test]
    fn heard_event_defaults_confidence_to_one() {
        let e: HeardEvent =
            serde_json::from_str(r#"{"stream":"mic0","text":"hi"}"#).unwrap();
        assert_eq!(e.confidence, 1.0);
    }

    #[test]
    fn heard_event_roundtrips() {
        let e = heard("mic0", Some("x"), Some("speech"), 0.7);
        let back: HeardEvent = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }

    #[tokio::test]
    async fn spine_speech_sink_is_best_effort_when_disconnected() {
        use crate::config::SpineConfig;
        let spine = Arc::new(crate::spine::SpineClient::new(SpineConfig::default(), "test"));
        let sink = SpineSpeechSink::new(spine);
        // An unconnected spine fails the publish, but the sink logs and returns Ok
        // so a reflex/agent that spoke is never broken by a transient outage.
        let u = Utterance { text: "hello".into(), voice: "nova".into(), at_ms: 1 };
        assert!(sink.speak(&u).await.is_ok());
    }
}

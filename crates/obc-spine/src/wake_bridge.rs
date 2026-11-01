//! The voice wake bridge (plan P5): wake events in from the mesh, listen
//! commands back out.
//!
//! A microcontroller's wake-word detector (microWakeWord on the ESP32-S3)
//! publishes `obc/voice/wake/{node}` with `{word, confidence, stream?}`. The SBC
//! gates it through [`WakeGate`], records the wake as an `audio.{stream}` event
//! labelled `wake` (so `hear` sees that someone spoke to the room), and
//! publishes `obc/voice/listen/{node}` with the window, which is the node's cue
//! to stream its microphone for that long.
//!
//! Same split as `fleet_bridge`: [`wake_event`], [`listen_topic`] and
//! [`listen_payload`] are the wire contract and need no broker; the handler is
//! the only part that touches the spine. The gate is policy and lives in
//! `obc_audio::wake`, which knows nothing about MQTT.

use crate::{MessageHandler, SpineClient, TOPIC_PREFIX};
use obc_audio::suite::{AudioController, HeardEvent};
use obc_audio::wake::{ListenWindow, WakeDecision, WakeEvent, WakeGate};
use obc_memory::world::Origin;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// MQTT topic filter for wake events (`obc/voice/wake/{node}`).
pub const WAKE_FILTER: &str = "obc/voice/wake/+";

/// Wall-clock milliseconds, local for the same reason as `fleet_bridge`'s.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The topic a listen command for `node` goes out on (`obc/voice/listen/{node}`).
pub fn listen_topic(node: &str) -> String {
    format!("{TOPIC_PREFIX}/voice/listen/{node}")
}

/// The listen command: which stream to open and until when.
pub fn listen_payload(window: &ListenWindow) -> Value {
    json!({
        "stream": window.stream,
        "word": window.word,
        "confidence": window.confidence,
        "opened_ms": window.opened_ms,
        "until_ms": window.until_ms,
        "listen_ms": window.until_ms.saturating_sub(window.opened_ms),
    })
}

/// A wake event from a message on [`WAKE_FILTER`]. The node is the last topic
/// segment, whatever the payload says; an empty payload is a wake with no word.
/// `None` for a payload that is not a JSON object or a topic with no node.
pub fn wake_event(topic: &str, payload: &[u8]) -> Option<WakeEvent> {
    let node = topic.rsplit('/').next().unwrap_or("").trim();
    if node.is_empty() {
        return None;
    }
    let payload: &[u8] = if payload.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        payload
    };
    let mut event: WakeEvent = serde_json::from_slice(payload).ok()?;
    event.node = node.to_string();
    Some(event)
}

/// A spine handler for [`WAKE_FILTER`]. An accepted wake is recorded through
/// `audio` (when the audio suite is on) and answered with a listen command; an
/// ignored one is logged at debug. Register with
/// [`SpineClient::subscribe_handler`].
pub fn spine_wake_handler(
    gate: Arc<Mutex<WakeGate>>,
    audio: Option<Arc<AudioController>>,
    spine: Arc<SpineClient>,
) -> MessageHandler {
    Arc::new(move |topic: &str, payload: &[u8]| {
        let Some(event) = wake_event(topic, payload) else {
            return;
        };
        let now = now_ms();
        let decision = gate
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .on_wake(&event, now);
        let window = match decision {
            WakeDecision::Open(window) => window,
            WakeDecision::Ignored(why) => {
                tracing::debug!(node = %event.node, word = %event.word, why, "wake event ignored");
                return;
            }
        };
        tracing::info!(node = %window.node, stream = %window.stream, word = %window.word, "wake word: listening");
        if let Some(audio) = &audio {
            let heard = HeardEvent {
                stream: window.stream.clone(),
                text: None,
                label: Some("wake".into()),
                confidence: window.confidence,
                source: Some(window.node.clone()),
            };
            if let Err(e) = audio.observe(&heard, now, Origin::Observed) {
                tracing::warn!(error = %e, "wake event not recorded");
            }
        }
        // Handlers are called synchronously from the spine's event loop; the
        // publish goes on the runtime so the loop is never held by it.
        let topic = listen_topic(&window.node);
        let payload = listen_payload(&window);
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                let spine = Arc::clone(&spine);
                rt.spawn(async move {
                    if let Err(e) = spine.publish(&topic, &payload).await {
                        tracing::warn!(%topic, error = %e, "listen command not published");
                    }
                });
            }
            Err(_) => tracing::warn!(%topic, "no runtime to publish the listen command on"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpineConfig;
    use obc_audio::suite::LoggingSpeechSink;
    use obc_audio::wake::WakePolicy;
    use obc_memory::world::WorldMemory;

    #[test]
    fn the_wire_contract() {
        assert_eq!(listen_topic("kitchen"), "obc/voice/listen/kitchen");
        let window = ListenWindow {
            node: "kitchen".into(),
            stream: "mic0".into(),
            word: "hey jarvis".into(),
            confidence: 0.9,
            opened_ms: 1_000,
            until_ms: 9_000,
        };
        let payload = listen_payload(&window);
        assert_eq!(payload["stream"], "mic0");
        assert_eq!(payload["listen_ms"], 8_000);
    }

    #[test]
    fn wake_event_takes_the_node_from_the_topic() {
        let event = wake_event(
            "obc/voice/wake/kitchen",
            br#"{"node":"spoofed","word":"hey jarvis","confidence":0.91}"#,
        )
        .unwrap();
        assert_eq!(event.node, "kitchen");
        assert_eq!(event.word, "hey jarvis");
        assert_eq!(event.confidence, 0.91);
        let bare = wake_event("obc/voice/wake/hall", b"").unwrap();
        assert_eq!((bare.word.as_str(), bare.confidence), ("", 1.0));
        assert!(wake_event("obc/voice/wake/hall", b"not json").is_none());
        assert!(wake_event("obc/voice/wake/", b"{}").is_none());
    }

    #[tokio::test]
    async fn an_accepted_wake_is_heard_and_an_ignored_one_is_not() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let audio =
            AudioController::new(Arc::new(LoggingSpeechSink)).with_world_memory(Arc::clone(&world));
        let gate = WakeGate::new(WakePolicy {
            words: vec!["hey jarvis".into()],
            ..WakePolicy::default()
        });
        // Unconnected: the listen publish fails and is logged, as it would be
        // during a broker outage.
        let spine = Arc::new(SpineClient::new(SpineConfig::default(), "test"));
        let handler = spine_wake_handler(Arc::new(Mutex::new(gate)), Some(Arc::new(audio)), spine);

        handler("obc/voice/wake/porch", br#"{"word":"alexa"}"#);
        assert!(world.current("audio.porch").unwrap().is_none());

        handler(
            "obc/voice/wake/kitchen",
            br#"{"word":"Hey Jarvis","confidence":0.95}"#,
        );
        let fact = world.current("audio.kitchen").unwrap().unwrap();
        assert_eq!(fact.value["label"], "wake");
        assert_eq!(fact.value["source"], "kitchen");
        assert!(fact.value["text"].is_null());
    }
}

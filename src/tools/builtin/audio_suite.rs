//! Audio suite tools — perceive (`hear`) and act (`speak`) for the agent.
//!
//! These wrap the first-class Audio suite ([`crate::audio::suite`]). `hear` is
//! non-actuating (observe a heard event, query the stream) and classed
//! [`RiskClass::safe`]. `speak` emits sound — a real-world effect — so it is
//! classed physical with a **low** blast radius: recorded into world memory but
//! not approval-gated (speech is reversible and contained; Suite §7).

use crate::audio::suite::{AudioController, HeardEvent};
use crate::memory::world::WorldMemory;
use crate::memory::world::Origin;
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── hear (perceive) ───────────────────────────────────────────────────────────

/// Tool: observe heard events and query `audio.{stream}` from world memory.
pub struct HearTool {
    controller: Arc<AudioController>,
    world: Arc<WorldMemory>,
}

impl HearTool {
    pub fn new(controller: Arc<AudioController>, world: Arc<WorldMemory>) -> Self {
        Self { controller, world }
    }

    fn observe(&self, args: &Value) -> ToolResult {
        let event: HeardEvent = match serde_json::from_value(args.clone()) {
            Ok(e) => e,
            Err(e) => return ToolResult::err(format!("invalid heard event: {e}")),
        };
        match self.controller.observe(&event, now_ms(), Origin::Asserted) {
            Ok(c) => ToolResult::ok(serde_json::to_string(&c).unwrap_or_else(|_| "{}".to_string())),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn current(&self, stream: &str) -> ToolResult {
        match self.world.current(&format!("audio.{stream}")) {
            Ok(Some(fact)) => ToolResult::ok(json!({ "stream": stream, "fact": fact }).to_string()),
            Ok(None) => {
                ToolResult::ok(json!({ "stream": stream, "fact": Value::Null }).to_string())
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn history(&self, stream: &str) -> ToolResult {
        match self.world.history(&format!("audio.{stream}")) {
            Ok(facts) => ToolResult::ok(json!({ "stream": stream, "history": facts }).to_string()),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }
}

#[async_trait]
impl Tool for HearTool {
    fn name(&self) -> &str {
        "hear"
    }

    fn description(&self) -> &str {
        "Observe and query heard audio events. Set `action` to: 'observe' (record \
         a heard event: stream, optional text/label, confidence 0..1, optional \
         source), 'current' (latest event for a stream), or 'history' (full \
         bitemporal history of audio.{stream}). Events are classified reliable \
         when confidence clears the floor. Non-actuating — no approval needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["observe", "current", "history"],
                    "description": "Operation to perform."
                },
                "stream": {
                    "type": "string",
                    "description": "Input stream id (e.g. 'mic0'). Required for all actions."
                },
                "text": { "type": "string", "description": "Transcribed text (observe, speech)." },
                "label": { "type": "string", "description": "Sound label (observe, e.g. 'alarm')." },
                "confidence": {
                    "type": "number", "minimum": 0.0, "maximum": 1.0,
                    "description": "Recognizer confidence (observe; default 1.0)."
                },
                "source": { "type": "string", "description": "Recognizer/node id (observe)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        RiskClass::safe()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        let stream = args
            .get("stream")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(match action {
            "observe" => self.observe(&args),
            "current" => match stream {
                Some(s) => self.current(&s),
                None => ToolResult::err("'current' requires 'stream'"),
            },
            "history" => match stream {
                Some(s) => self.history(&s),
                None => ToolResult::err("'history' requires 'stream'"),
            },
            other => ToolResult::err(format!("unknown action: '{other}'")),
        })
    }
}

// ── speak (act) ───────────────────────────────────────────────────────────────

/// Tool: emit a spoken utterance through the audio suite (recorded as `speech.last`).
pub struct SpeakTool {
    controller: Arc<AudioController>,
    default_voice: String,
}

impl SpeakTool {
    pub fn new(controller: Arc<AudioController>, default_voice: impl Into<String>) -> Self {
        Self {
            controller,
            default_voice: default_voice.into(),
        }
    }
}

#[async_trait]
impl Tool for SpeakTool {
    fn name(&self) -> &str {
        "speak"
    }

    fn description(&self) -> &str {
        "Speak a short utterance aloud. Provide `text` and optionally a `voice`. \
         The utterance is recorded as speech.last in world memory and emitted \
         through the configured speech output. Physical (emits sound) but \
         reversible and contained — recorded, not approval-gated."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": { "type": "string", "description": "What to say." },
                "voice": { "type": "string", "description": "Voice id (optional; uses the configured default)." }
            }
        })
    }

    fn risk_class(&self) -> RiskClass {
        // Emits a real-world effect (sound), but reversible and low blast: the
        // approval layer records it without requiring per-call approval.
        RiskClass::physical(true, BlastRadius::Low)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let text = match args.get("text").and_then(Value::as_str) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return Ok(ToolResult::err("'speak' requires non-empty 'text'")),
        };
        let voice = args
            .get("voice")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| self.default_voice.clone());
        match self.controller.speak(text, voice, now_ms()).await {
            Ok(u) => Ok(ToolResult::ok(
                serde_json::to_string(&u).unwrap_or_else(|_| "{}".to_string()),
            )),
            Err(e) => Ok(ToolResult::err(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::suite::LoggingSpeechSink;

    fn tools() -> (HearTool, SpeakTool, Arc<WorldMemory>) {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ctrl = Arc::new(
            AudioController::new(Arc::new(LoggingSpeechSink))
                .with_world_memory(Arc::clone(&world))
                .with_min_confidence(0.6),
        );
        (
            HearTool::new(Arc::clone(&ctrl), Arc::clone(&world)),
            SpeakTool::new(ctrl, "nova"),
            world,
        )
    }

    #[test]
    fn hear_is_safe_speak_is_physical_low_no_approval() {
        let (h, s, _) = tools();
        assert!(!h.risk_class().physical);
        let rc = s.risk_class();
        assert!(rc.physical);
        assert_eq!(rc.blast, BlastRadius::Low);
        assert!(!rc.requires_per_call_approval());
    }

    #[tokio::test]
    async fn observe_then_current_reports_event() {
        let (h, _, _) = tools();
        let r = h
            .execute(json!({ "action": "observe", "stream": "mic0", "text": "lights on", "confidence": 0.95 }))
            .await
            .unwrap();
        assert!(r.success, "observe failed: {:?}", r.error);

        let r = h
            .execute(json!({ "action": "current", "stream": "mic0" }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["fact"]["value"]["text"], "lights on");
        assert_eq!(v["fact"]["value"]["reliable"], true);
    }

    #[tokio::test]
    async fn speak_records_and_returns_utterance() {
        let (_, s, world) = tools();
        let r = s.execute(json!({ "text": "hello" })).await.unwrap();
        assert!(r.success, "speak failed: {:?}", r.error);
        let v: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(v["text"], "hello");
        assert_eq!(v["voice"], "nova"); // default applied
        assert_eq!(
            world.current("speech.last").unwrap().unwrap().value["text"],
            "hello"
        );
    }

    #[tokio::test]
    async fn empty_speak_is_soft_error() {
        let (_, s, _) = tools();
        let r = s.execute(json!({ "text": "" })).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn hear_missing_stream_is_soft_error() {
        let (h, _, _) = tools();
        let r = h.execute(json!({ "action": "current" })).await.unwrap();
        assert!(!r.success);
    }
}

//! Wake-word gating on the SBC (plan P5).
//!
//! The detector is not here. It runs on the microcontroller (microWakeWord on
//! the ESP32-S3), which publishes a wake event on the mesh when it hears the
//! word. This is the SBC's half: whether that event opens a listen window. A
//! detector fires on near-misses, on the television, and twice for one word, so
//! the gate takes only the configured words, only above a confidence, and only
//! once per node per cooldown.
//!
//! Pure and clock-free (the caller passes `now_ms`), so the whole policy is
//! tested without a microphone, a broker or a sleep.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What a wake event must satisfy to open a window.
#[derive(Debug, Clone, PartialEq)]
pub struct WakePolicy {
    /// Accepted wake words, compared case-insensitively. Empty accepts any word
    /// the detector reports (it was flashed with one model, so it has one).
    pub words: Vec<String>,
    /// Lowest detector confidence accepted, 0..=1.
    pub min_confidence: f64,
    /// How long a window stays open after the word.
    pub listen_ms: u64,
    /// After a window opens, further wake events from the same node are ignored
    /// for this long.
    pub cooldown_ms: u64,
}

impl Default for WakePolicy {
    fn default() -> Self {
        Self {
            words: Vec::new(),
            min_confidence: 0.8,
            listen_ms: 8_000,
            cooldown_ms: 2_000,
        }
    }
}

fn one() -> f64 {
    1.0
}

/// A wake event as the detector publishes it. `node` normally comes from the
/// topic, not the payload.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WakeEvent {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub word: String,
    /// A detector that does not report one is taken at its word.
    #[serde(default = "one")]
    pub confidence: f64,
    /// The audio stream to open; the node id when absent.
    #[serde(default)]
    pub stream: Option<String>,
}

/// An open listen window: what the SBC tells the node to stream, and for how
/// long.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListenWindow {
    pub node: String,
    pub stream: String,
    pub word: String,
    pub confidence: f64,
    pub opened_ms: u64,
    pub until_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WakeDecision {
    Open(ListenWindow),
    /// Why not, for the debug log.
    Ignored(&'static str),
}

/// The gate: a [`WakePolicy`] and when each node last opened a window.
#[derive(Debug, Clone)]
pub struct WakeGate {
    policy: WakePolicy,
    last_open: HashMap<String, u64>,
}

impl WakeGate {
    pub fn new(policy: WakePolicy) -> Self {
        Self {
            policy,
            last_open: HashMap::new(),
        }
    }

    pub fn policy(&self) -> &WakePolicy {
        &self.policy
    }

    /// Decide on one wake event at `now_ms`. An accepted event starts the
    /// node's cooldown; an ignored one does not.
    pub fn on_wake(&mut self, event: &WakeEvent, now_ms: u64) -> WakeDecision {
        let node = event.node.trim();
        if node.is_empty() {
            return WakeDecision::Ignored("no node");
        }
        let word = event.word.trim();
        let listed = self.policy.words.is_empty()
            || self
                .policy
                .words
                .iter()
                .any(|w| w.trim().eq_ignore_ascii_case(word));
        if !listed {
            return WakeDecision::Ignored("not a configured wake word");
        }
        if event.confidence.is_nan() || event.confidence < self.policy.min_confidence {
            return WakeDecision::Ignored("below the minimum confidence");
        }
        if let Some(&last) = self.last_open.get(node) {
            if now_ms.saturating_sub(last) < self.policy.cooldown_ms {
                return WakeDecision::Ignored("cooling down");
            }
        }
        self.last_open.insert(node.to_string(), now_ms);
        let stream = event
            .stream
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(node);
        WakeDecision::Open(ListenWindow {
            node: node.to_string(),
            stream: stream.to_string(),
            word: word.to_string(),
            confidence: event.confidence,
            opened_ms: now_ms,
            until_ms: now_ms.saturating_add(self.policy.listen_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wake(node: &str, word: &str, confidence: f64) -> WakeEvent {
        WakeEvent {
            node: node.into(),
            word: word.into(),
            confidence,
            stream: None,
        }
    }

    fn gate(words: &[&str]) -> WakeGate {
        WakeGate::new(WakePolicy {
            words: words.iter().map(|w| w.to_string()).collect(),
            ..WakePolicy::default()
        })
    }

    #[test]
    fn a_listed_word_above_confidence_opens_a_window() {
        let mut g = gate(&["Okay Nabu"]);
        let WakeDecision::Open(w) = g.on_wake(&wake("kitchen", "okay nabu", 0.93), 1_000) else {
            panic!("should open");
        };
        assert_eq!(w.node, "kitchen");
        assert_eq!(w.stream, "kitchen");
        assert_eq!((w.opened_ms, w.until_ms), (1_000, 9_000));
    }

    #[test]
    fn the_wrong_word_low_confidence_or_no_node_is_ignored() {
        let mut g = gate(&["hey jarvis"]);
        assert_eq!(
            g.on_wake(&wake("kitchen", "alexa", 0.99), 0),
            WakeDecision::Ignored("not a configured wake word")
        );
        assert_eq!(
            g.on_wake(&wake("kitchen", "hey jarvis", 0.5), 0),
            WakeDecision::Ignored("below the minimum confidence")
        );
        assert_eq!(
            g.on_wake(&wake("kitchen", "hey jarvis", f64::NAN), 0),
            WakeDecision::Ignored("below the minimum confidence")
        );
        assert_eq!(
            g.on_wake(&wake(" ", "hey jarvis", 1.0), 0),
            WakeDecision::Ignored("no node")
        );
        // None of those started a cooldown.
        assert!(matches!(
            g.on_wake(&wake("kitchen", "hey jarvis", 0.9), 1),
            WakeDecision::Open(_)
        ));
    }

    #[test]
    fn an_empty_word_list_takes_whatever_the_detector_heard() {
        let mut g = gate(&[]);
        assert!(matches!(
            g.on_wake(&wake("hall", "", 1.0), 0),
            WakeDecision::Open(_)
        ));
    }

    #[test]
    fn cooldown_is_per_node_and_ends() {
        let mut g = gate(&[]);
        assert!(matches!(
            g.on_wake(&wake("a", "w", 1.0), 10_000),
            WakeDecision::Open(_)
        ));
        assert_eq!(
            g.on_wake(&wake("a", "w", 1.0), 11_999),
            WakeDecision::Ignored("cooling down")
        );
        assert!(matches!(
            g.on_wake(&wake("b", "w", 1.0), 11_999),
            WakeDecision::Open(_)
        ));
        assert!(matches!(
            g.on_wake(&wake("a", "w", 1.0), 12_000),
            WakeDecision::Open(_)
        ));
        // A clock that steps back does not underflow into a window.
        assert_eq!(
            g.on_wake(&wake("a", "w", 1.0), 5),
            WakeDecision::Ignored("cooling down")
        );
    }

    #[test]
    fn a_named_stream_wins_and_payload_defaults_hold() {
        let event: WakeEvent = serde_json::from_str(r#"{"word":"hey","stream":"mic1"}"#).unwrap();
        assert_eq!(event.confidence, 1.0);
        let mut g = gate(&[]);
        let event = WakeEvent {
            node: "porch".into(),
            ..event
        };
        let WakeDecision::Open(w) = g.on_wake(&event, 0) else {
            panic!("should open");
        };
        assert_eq!(w.stream, "mic1");
    }
}

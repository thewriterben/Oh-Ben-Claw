//! Escalation notifications — wire reflex escalations to real channels.
//!
//! Every reflex `Action::Escalate` (mesh node lost, battery critical, alarm heard, …)
//! flows through [`ActionSink::escalate`](super::reflex::ActionSink::escalate). This
//! module fans those escalations out to operator-facing channels: a durable
//! **log-of-record** in world memory and an optional **webhook** (Slack/Discord/generic).
//!
//! It plugs in as a [`NotifyingActionSink`] decorator that notifies, then delegates to the
//! inner sink — so the existing wake-System-2 path is unchanged; notification is additive
//! and best-effort (a down webhook never stalls System 1).

use super::reflex::ActionSink;
use crate::movement::MovementCommand;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A single escalation to notify about.
#[derive(Debug, Clone, PartialEq)]
pub struct Escalation {
    pub reason: String,
    pub ts_ms: u64,
}

/// A destination an escalation is delivered to.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Channel name (for logs).
    fn name(&self) -> &str;
    /// Deliver the escalation. Errors are logged by the [`Notifier`], not propagated.
    async fn deliver(&self, esc: &Escalation) -> anyhow::Result<()>;
}

/// Log-of-record: append each escalation to world memory as a `notifications.escalation`
/// fact (non-destructive, so `history` gives the full trail; `current` the latest).
pub struct WorldMemoryChannel {
    world: Arc<crate::memory::world::WorldMemory>,
}

impl WorldMemoryChannel {
    pub fn new(world: Arc<crate::memory::world::WorldMemory>) -> Self {
        Self { world }
    }
}

#[async_trait]
impl NotificationChannel for WorldMemoryChannel {
    fn name(&self) -> &str {
        "world-memory"
    }
    async fn deliver(&self, esc: &Escalation) -> anyhow::Result<()> {
        self.world.observe(
            "notifications.escalation",
            json!({ "reason": esc.reason, "ts_ms": esc.ts_ms }),
            esc.ts_ms,
            esc.ts_ms,
            "notifier",
        )?;
        Ok(())
    }
}

/// Webhook channel: POST a Slack/Discord-compatible `{ "text": … }` payload to a URL.
pub struct WebhookChannel {
    url: String,
    client: reqwest::Client,
}

impl WebhookChannel {
    pub fn new(url: String) -> Self {
        Self { url, client: reqwest::Client::new() }
    }
    /// The JSON body posted for an escalation (broken out for testing).
    pub fn payload(esc: &Escalation) -> Value {
        json!({ "text": format!("OBC escalation: {}", esc.reason) })
    }
}

#[async_trait]
impl NotificationChannel for WebhookChannel {
    fn name(&self) -> &str {
        "webhook"
    }
    async fn deliver(&self, esc: &Escalation) -> anyhow::Result<()> {
        self.client.post(&self.url).json(&Self::payload(esc)).send().await?;
        Ok(())
    }
}

/// The short spoken form of an escalation reason: just the first sentence, so a full
/// triage directive isn't read aloud in its entirety.
fn speech_headline(reason: &str) -> String {
    let first = reason.split_once(". ").map(|(h, _)| h).unwrap_or(reason);
    format!("Attention. {}.", first.trim_end_matches('.'))
}

/// Speak channel: renders the escalation aloud through a [`SpeechSink`] (a TTS engine or
/// a speaker over the spine) so a nearby human *hears* the alarm, not just sees a log.
/// Speaks only the [`speech_headline`] — reasons may carry a full triage directive.
pub struct SpeechChannel {
    speech: Arc<dyn crate::audio::suite::SpeechSink>,
    voice: String,
}

impl SpeechChannel {
    pub fn new(speech: Arc<dyn crate::audio::suite::SpeechSink>) -> Self {
        Self { speech, voice: "nova".to_string() }
    }
    pub fn with_voice(mut self, voice: impl Into<String>) -> Self {
        self.voice = voice.into();
        self
    }
}

#[async_trait]
impl NotificationChannel for SpeechChannel {
    fn name(&self) -> &str {
        "speech"
    }
    async fn deliver(&self, esc: &Escalation) -> anyhow::Result<()> {
        let u = crate::audio::suite::Utterance {
            text: speech_headline(&esc.reason),
            voice: self.voice.clone(),
            at_ms: esc.ts_ms,
        };
        self.speech.speak(&u).await
    }
}

/// Per-reason de-dup bookkeeping.
#[derive(Default)]
struct DedupEntry {
    last_sent_ms: u64,
    suppressed: u64,
}

/// Fans an escalation out to every configured channel, best-effort, with optional
/// **de-duplication**: identical escalations (same reason) within `dedup_window_ms` are
/// suppressed and counted, so a flapping condition doesn't spam every channel each tick.
/// The next alert *after* the window carries a `[+N repeats suppressed]` note, so nothing
/// is silently lost — repeats are collapsed into a digest, not dropped.
#[derive(Default)]
pub struct Notifier {
    channels: Vec<Arc<dyn NotificationChannel>>,
    dedup_window_ms: u64,
    seen: Mutex<HashMap<String, DedupEntry>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_channel(mut self, ch: Arc<dyn NotificationChannel>) -> Self {
        self.channels.push(ch);
        self
    }
    /// Suppress identical escalations within this window (ms). `0` disables de-dup.
    pub fn with_dedup_window(mut self, window_ms: u64) -> Self {
        self.dedup_window_ms = window_ms;
        self
    }
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
    /// Deliver to all channels; a failing channel is logged and skipped so one bad
    /// destination never blocks the others (or the escalate that follows). Identical
    /// recent escalations are de-duplicated when a `dedup_window_ms` is set.
    pub async fn notify(&self, esc: &Escalation) {
        let mut reason = esc.reason.clone();
        if self.dedup_window_ms > 0 {
            let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            let entry = seen.entry(esc.reason.clone()).or_default();
            if entry.last_sent_ms != 0
                && esc.ts_ms.saturating_sub(entry.last_sent_ms) < self.dedup_window_ms
            {
                entry.suppressed += 1;
                return; // identical + within the window → don't fan out
            }
            if entry.suppressed > 0 {
                reason = format!("{} [+{} repeats suppressed]", esc.reason, entry.suppressed);
            }
            entry.last_sent_ms = esc.ts_ms;
            entry.suppressed = 0;
        }
        let out = Escalation { reason, ts_ms: esc.ts_ms };
        for ch in &self.channels {
            if let Err(e) = ch.deliver(&out).await {
                tracing::warn!(channel = ch.name(), error = %e, "escalation notification failed");
            }
        }
    }
}

/// An [`ActionSink`] decorator that fans escalations out to a [`Notifier`], then delegates
/// every action (including the escalate itself) to the inner sink. Non-escalate actions
/// pass straight through untouched.
pub struct NotifyingActionSink {
    inner: Arc<dyn ActionSink>,
    notifier: Arc<Notifier>,
}

impl NotifyingActionSink {
    pub fn new(inner: Arc<dyn ActionSink>, notifier: Arc<Notifier>) -> Self {
        Self { inner, notifier }
    }
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[async_trait]
impl ActionSink for NotifyingActionSink {
    async fn gpio_write(&self, node_id: &str, pin: i64, value: i64) -> anyhow::Result<()> {
        self.inner.gpio_write(node_id, pin, value).await
    }
    async fn publish(&self, topic: &str, payload: &Value) -> anyhow::Result<()> {
        self.inner.publish(topic, payload).await
    }
    async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
        let esc = Escalation { reason: reason.to_string(), ts_ms: Self::now_ms() };
        self.notifier.notify(&esc).await;
        self.inner.escalate(reason).await
    }
    async fn move_actuator(&self, command: &MovementCommand) -> anyhow::Result<()> {
        self.inner.move_actuator(command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::world::WorldMemory;
    use std::sync::Mutex;

    #[tokio::test]
    async fn world_memory_channel_records_a_durable_escalation() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let ch = WorldMemoryChannel::new(Arc::clone(&world));
        ch.deliver(&Escalation { reason: "node lost".into(), ts_ms: 1_000 }).await.unwrap();
        let f = world.current("notifications.escalation").unwrap().unwrap();
        assert_eq!(f.value["reason"], json!("node lost"));
        assert_eq!(f.source, "notifier");
    }

    #[test]
    fn webhook_payload_is_slack_compatible() {
        let p = WebhookChannel::payload(&Escalation { reason: "battery critical".into(), ts_ms: 0 });
        assert!(p.get("text").and_then(|t| t.as_str()).unwrap().contains("battery critical"));
    }

    /// Records what it was asked to deliver / delegate.
    struct Recorder {
        delivered: Mutex<Vec<String>>,
        escalated: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl NotificationChannel for Recorder {
        fn name(&self) -> &str {
            "recorder"
        }
        async fn deliver(&self, esc: &Escalation) -> anyhow::Result<()> {
            self.delivered.lock().unwrap().push(esc.reason.clone());
            Ok(())
        }
    }
    #[async_trait]
    impl ActionSink for Recorder {
        async fn gpio_write(&self, _n: &str, _p: i64, _v: i64) -> anyhow::Result<()> {
            Ok(())
        }
        async fn publish(&self, _t: &str, _p: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        async fn escalate(&self, reason: &str) -> anyhow::Result<()> {
            self.escalated.lock().unwrap().push(reason.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn notifying_sink_notifies_then_delegates_the_escalate() {
        let inner = Arc::new(Recorder { delivered: Mutex::new(vec![]), escalated: Mutex::new(vec![]) });
        let channel = Arc::new(Recorder { delivered: Mutex::new(vec![]), escalated: Mutex::new(vec![]) });
        let notifier = Arc::new(Notifier::new().with_channel(Arc::clone(&channel) as Arc<dyn NotificationChannel>));
        let sink = NotifyingActionSink::new(Arc::clone(&inner) as Arc<dyn ActionSink>, notifier);

        sink.escalate("mesh node lost").await.unwrap();

        // The channel was notified …
        assert_eq!(channel.delivered.lock().unwrap().as_slice(), &["mesh node lost".to_string()]);
        // … and the escalate still reached the inner sink (System 2 wake path intact).
        assert_eq!(inner.escalated.lock().unwrap().as_slice(), &["mesh node lost".to_string()]);
    }

    struct SpeakRecorder {
        spoken: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl crate::audio::suite::SpeechSink for SpeakRecorder {
        async fn speak(&self, u: &crate::audio::suite::Utterance) -> anyhow::Result<()> {
            self.spoken.lock().unwrap().push(u.text.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn speech_channel_speaks_only_the_headline() {
        let rec = Arc::new(SpeakRecorder { spoken: Mutex::new(vec![]) });
        let ch = SpeechChannel::new(Arc::clone(&rec) as Arc<dyn crate::audio::suite::SpeechSink>);
        ch.deliver(&Escalation {
            reason: "A mesh node is presumed lost (LoRa escalation). Triage: call mesh_status and \
                     then mesh_command a capabilities ping."
                .into(),
            ts_ms: 5,
        })
        .await
        .unwrap();
        let spoken = rec.spoken.lock().unwrap();
        assert_eq!(spoken.len(), 1);
        assert!(spoken[0].contains("presumed lost"), "speaks the headline");
        assert!(!spoken[0].contains("Triage"), "does not read the full directive aloud");
    }

    #[tokio::test]
    async fn dedup_suppresses_repeats_within_the_window_then_reports_the_count() {
        let rec = Arc::new(Recorder { delivered: Mutex::new(vec![]), escalated: Mutex::new(vec![]) });
        let notifier = Notifier::new()
            .with_dedup_window(10_000)
            .with_channel(Arc::clone(&rec) as Arc<dyn NotificationChannel>);

        // First alert goes out.
        notifier.notify(&Escalation { reason: "node lost".into(), ts_ms: 1_000 }).await;
        // Two identical repeats within the 10 s window are suppressed (and counted).
        notifier.notify(&Escalation { reason: "node lost".into(), ts_ms: 3_000 }).await;
        notifier.notify(&Escalation { reason: "node lost".into(), ts_ms: 5_000 }).await;
        // After the window it fires again, carrying the suppressed count as a digest.
        notifier.notify(&Escalation { reason: "node lost".into(), ts_ms: 12_000 }).await;

        let d = rec.delivered.lock().unwrap();
        assert_eq!(d.len(), 2, "only two alerts left the channel");
        assert_eq!(d[0], "node lost");
        assert!(d[1].contains("node lost") && d[1].contains("+2"), "digest reports repeats: {}", d[1]);
    }

    #[tokio::test]
    async fn different_reasons_are_not_deduped_against_each_other() {
        let rec = Arc::new(Recorder { delivered: Mutex::new(vec![]), escalated: Mutex::new(vec![]) });
        let notifier = Notifier::new()
            .with_dedup_window(10_000)
            .with_channel(Arc::clone(&rec) as Arc<dyn NotificationChannel>);
        notifier.notify(&Escalation { reason: "node lost".into(), ts_ms: 1_000 }).await;
        notifier.notify(&Escalation { reason: "battery critical".into(), ts_ms: 1_100 }).await;
        assert_eq!(rec.delivered.lock().unwrap().len(), 2, "distinct alerts both go out");
    }
}

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

/// Escalation severity, for routing (a channel can require a minimum). `Info < Warning
/// < Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Severity {
    Info,
    #[default]
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
    /// Parse a severity name for a channel's *minimum*; unknown/none → `Info` (accept all).
    pub fn from_name(s: Option<&str>) -> Severity {
        match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
            Some("critical") => Severity::Critical,
            Some("warning") => Severity::Warning,
            _ => Severity::Info,
        }
    }
    /// Classify an escalation reason by keywords. Escalations default to `Warning`; clear
    /// danger words raise it to `Critical`.
    pub fn classify(reason: &str) -> Severity {
        let r = reason.to_ascii_lowercase();
        const CRIT: [&str; 6] = [
            "critical",
            "presumed lost",
            "alarm",
            "overheat",
            "emergency",
            "over limit",
        ];
        if CRIT.iter().any(|k| r.contains(k)) {
            Severity::Critical
        } else {
            Severity::Warning
        }
    }
}

/// A single escalation to notify about.
#[derive(Debug, Clone, PartialEq)]
pub struct Escalation {
    pub reason: String,
    pub ts_ms: u64,
    pub severity: Severity,
}

impl Escalation {
    /// Build an escalation, classifying its severity from the reason.
    pub fn new(reason: impl Into<String>, ts_ms: u64) -> Self {
        let reason = reason.into();
        let severity = Severity::classify(&reason);
        Self {
            reason,
            ts_ms,
            severity,
        }
    }
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
        Self {
            url,
            client: reqwest::Client::new(),
        }
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
        self.client
            .post(&self.url)
            .json(&Self::payload(esc))
            .send()
            .await?;
        Ok(())
    }
}

/// Prefix on every periodic digest message. Also used to exclude prior digests from the
/// raw escalation history when the next digest is built (so digests don't compound).
pub const DIGEST_PREFIX: &str = "OBC escalation digest";

/// The first sentence of a reason (reasons may carry a full triage directive).
fn first_sentence(reason: &str) -> &str {
    reason
        .split_once(". ")
        .map(|(h, _)| h)
        .unwrap_or(reason)
        .trim_end_matches('.')
}

/// The short spoken form of an escalation reason: just the first sentence, so a full
/// triage directive isn't read aloud in its entirety.
fn speech_headline(reason: &str) -> String {
    format!("Attention. {}.", first_sentence(reason))
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
        Self {
            speech,
            voice: "nova".to_string(),
        }
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

// ── Periodic digest (scheduled roll-up of the escalation log) ────────────────────

/// One recorded escalation, read from the `notifications.escalation` world-memory log.
#[derive(Debug, Clone, PartialEq)]
pub struct EscalationRecord {
    pub reason: String,
    pub ts_ms: u64,
}

/// A grouped line in a digest: one distinct reason, how often it fired, and its span.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestLine {
    pub reason: String,
    pub count: u64,
    pub first_ms: u64,
    pub last_ms: u64,
}

/// Roll `records` up by reason (most frequent first), keeping only those within
/// `[now_ms - window_ms, now_ms]`. Pure and testable.
pub fn build_digest(records: &[EscalationRecord], window_ms: u64, now_ms: u64) -> Vec<DigestLine> {
    let cutoff = now_ms.saturating_sub(window_ms);
    let mut by_reason: std::collections::BTreeMap<String, DigestLine> = Default::default();
    for r in records {
        if r.ts_ms < cutoff {
            continue;
        }
        let e = by_reason.entry(r.reason.clone()).or_insert(DigestLine {
            reason: r.reason.clone(),
            count: 0,
            first_ms: r.ts_ms,
            last_ms: r.ts_ms,
        });
        e.count += 1;
        e.first_ms = e.first_ms.min(r.ts_ms);
        e.last_ms = e.last_ms.max(r.ts_ms);
    }
    let mut lines: Vec<DigestLine> = by_reason.into_values().collect();
    lines.sort_by(|a, b| b.count.cmp(&a.count).then(a.reason.cmp(&b.reason)));
    lines
}

/// Format a digest into a one-line human summary (prefixed with [`DIGEST_PREFIX`]).
/// `None` when there's nothing to report. `window_label` is a human span like `"24h"`.
pub fn format_digest(lines: &[DigestLine], window_label: &str) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let total: u64 = lines.iter().map(|l| l.count).sum();
    let mut s = format!("{DIGEST_PREFIX} — {total} in the last {window_label}:");
    for l in lines {
        s.push_str(&format!(" {}x {};", l.count, first_sentence(&l.reason)));
    }
    Some(s)
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
    channels: Vec<(Arc<dyn NotificationChannel>, Severity)>,
    dedup_window_ms: u64,
    seen: Mutex<HashMap<String, DedupEntry>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }
    /// Add a channel that receives all severities.
    pub fn with_channel(self, ch: Arc<dyn NotificationChannel>) -> Self {
        self.with_channel_min(ch, Severity::Info)
    }
    /// Add a channel that only receives escalations at or above `min`.
    pub fn with_channel_min(mut self, ch: Arc<dyn NotificationChannel>, min: Severity) -> Self {
        self.channels.push((ch, min));
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
    /// Deliver to matching channels; a failing channel is logged and skipped so one bad
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
        self.fan_out(&Escalation {
            reason,
            ts_ms: esc.ts_ms,
            severity: esc.severity,
        })
        .await;
    }

    /// Deliver to every channel that accepts this severity, best-effort.
    async fn fan_out(&self, esc: &Escalation) {
        for (ch, min) in &self.channels {
            if esc.severity < *min {
                continue; // below this channel's threshold
            }
            if let Err(e) = ch.deliver(esc).await {
                tracing::warn!(channel = ch.name(), error = %e, "escalation notification failed");
            }
        }
    }

    /// Deliver a periodic digest / summary. No de-dup — digests are schedule-limited.
    /// Sent at `Info` so it reaches every accept-all channel.
    pub async fn deliver_summary(&self, text: String, ts_ms: u64) {
        self.fan_out(&Escalation {
            reason: text,
            ts_ms,
            severity: Severity::Info,
        })
        .await;
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
        self.notifier
            .notify(&Escalation::new(reason, Self::now_ms()))
            .await;
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
        ch.deliver(&Escalation::new("node lost", 1_000))
            .await
            .unwrap();
        let f = world.current("notifications.escalation").unwrap().unwrap();
        assert_eq!(f.value["reason"], json!("node lost"));
        assert_eq!(f.source, "notifier");
    }

    #[test]
    fn webhook_payload_is_slack_compatible() {
        let p = WebhookChannel::payload(&Escalation::new("battery critical", 0));
        assert!(p
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap()
            .contains("battery critical"));
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
        let inner = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let channel = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let notifier = Arc::new(
            Notifier::new().with_channel(Arc::clone(&channel) as Arc<dyn NotificationChannel>),
        );
        let sink = NotifyingActionSink::new(Arc::clone(&inner) as Arc<dyn ActionSink>, notifier);

        sink.escalate("mesh node lost").await.unwrap();

        // The channel was notified …
        assert_eq!(
            channel.delivered.lock().unwrap().as_slice(),
            &["mesh node lost".to_string()]
        );
        // … and the escalate still reached the inner sink (System 2 wake path intact).
        assert_eq!(
            inner.escalated.lock().unwrap().as_slice(),
            &["mesh node lost".to_string()]
        );
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
        let rec = Arc::new(SpeakRecorder {
            spoken: Mutex::new(vec![]),
        });
        let ch = SpeechChannel::new(Arc::clone(&rec) as Arc<dyn crate::audio::suite::SpeechSink>);
        ch.deliver(&Escalation::new(
            "A mesh node is presumed lost (LoRa escalation). Triage: call mesh_status and \
             then mesh_command a capabilities ping.",
            5,
        ))
        .await
        .unwrap();
        let spoken = rec.spoken.lock().unwrap();
        assert_eq!(spoken.len(), 1);
        assert!(spoken[0].contains("presumed lost"), "speaks the headline");
        assert!(
            !spoken[0].contains("Triage"),
            "does not read the full directive aloud"
        );
    }

    #[tokio::test]
    async fn dedup_suppresses_repeats_within_the_window_then_reports_the_count() {
        let rec = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let notifier = Notifier::new()
            .with_dedup_window(10_000)
            .with_channel(Arc::clone(&rec) as Arc<dyn NotificationChannel>);

        // First alert goes out.
        notifier.notify(&Escalation::new("node lost", 1_000)).await;
        // Two identical repeats within the 10 s window are suppressed (and counted).
        notifier.notify(&Escalation::new("node lost", 3_000)).await;
        notifier.notify(&Escalation::new("node lost", 5_000)).await;
        // After the window it fires again, carrying the suppressed count as a digest.
        notifier.notify(&Escalation::new("node lost", 12_000)).await;

        let d = rec.delivered.lock().unwrap();
        assert_eq!(d.len(), 2, "only two alerts left the channel");
        assert_eq!(d[0], "node lost");
        assert!(
            d[1].contains("node lost") && d[1].contains("+2"),
            "digest reports repeats: {}",
            d[1]
        );
    }

    #[tokio::test]
    async fn different_reasons_are_not_deduped_against_each_other() {
        let rec = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let notifier = Notifier::new()
            .with_dedup_window(10_000)
            .with_channel(Arc::clone(&rec) as Arc<dyn NotificationChannel>);
        notifier.notify(&Escalation::new("node lost", 1_000)).await;
        notifier
            .notify(&Escalation::new("battery critical", 1_100))
            .await;
        assert_eq!(
            rec.delivered.lock().unwrap().len(),
            2,
            "distinct alerts both go out"
        );
    }

    #[test]
    fn digest_groups_windows_and_ranks_escalations() {
        let recs = vec![
            EscalationRecord {
                reason: "node lost".into(),
                ts_ms: 1_000,
            },
            EscalationRecord {
                reason: "node lost".into(),
                ts_ms: 2_000,
            },
            EscalationRecord {
                reason: "battery critical".into(),
                ts_ms: 2_500,
            },
            EscalationRecord {
                reason: "stale".into(),
                ts_ms: 100,
            }, // outside the window
        ];
        let lines = build_digest(&recs, 5_000, 6_000); // cutoff = 1_000
        assert_eq!(lines.len(), 2, "the stale record is excluded");
        assert_eq!(lines[0].reason, "node lost", "most frequent first");
        assert_eq!(lines[0].count, 2);
        assert_eq!(lines[0].first_ms, 1_000);
        assert_eq!(lines[0].last_ms, 2_000);

        let s = format_digest(&lines, "24h").unwrap();
        assert!(s.starts_with(DIGEST_PREFIX));
        assert!(s.contains("3 in the last 24h"));
        assert!(s.contains("2x node lost"));
    }

    #[test]
    fn an_empty_digest_is_none() {
        assert!(format_digest(&[], "24h").is_none());
        assert!(build_digest(&[], 1_000, 10_000).is_empty());
    }

    #[test]
    fn severity_classifies_from_the_reason() {
        assert_eq!(
            Severity::classify("a mesh node is presumed lost"),
            Severity::Critical
        );
        assert_eq!(
            Severity::classify("battery critical — safing"),
            Severity::Critical
        );
        assert_eq!(
            Severity::classify("sensor humidity out of range"),
            Severity::Warning
        );
        assert!(Severity::Critical > Severity::Warning && Severity::Warning > Severity::Info);
    }

    #[tokio::test]
    async fn severity_routes_to_channels_by_minimum() {
        let all = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let crit = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        let notifier = Notifier::new()
            .with_channel(Arc::clone(&all) as Arc<dyn NotificationChannel>)
            .with_channel_min(
                Arc::clone(&crit) as Arc<dyn NotificationChannel>,
                Severity::Critical,
            );

        notifier
            .notify(&Escalation::new("sensor reading unreliable", 1))
            .await; // Warning
        notifier
            .notify(&Escalation::new("a node is presumed lost", 2))
            .await; // Critical

        assert_eq!(
            all.delivered.lock().unwrap().len(),
            2,
            "accept-all channel gets both"
        );
        assert_eq!(
            crit.delivered.lock().unwrap().len(),
            1,
            "critical-only channel gets one"
        );
    }

    #[tokio::test]
    async fn deliver_summary_fans_out_without_dedup() {
        let rec = Arc::new(Recorder {
            delivered: Mutex::new(vec![]),
            escalated: Mutex::new(vec![]),
        });
        // Same window that would de-dup a repeated escalation …
        let notifier = Notifier::new()
            .with_dedup_window(10_000)
            .with_channel(Arc::clone(&rec) as Arc<dyn NotificationChannel>);
        notifier.deliver_summary("digest A".into(), 1_000).await;
        notifier.deliver_summary("digest A".into(), 2_000).await; // … does not suppress digests
        assert_eq!(rec.delivered.lock().unwrap().len(), 2);
    }
}

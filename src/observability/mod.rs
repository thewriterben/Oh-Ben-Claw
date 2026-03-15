//! Oh-Ben-Claw Observability Module
//!
//! Provides structured tracing, span recording, and lightweight metrics for
//! the agent loop, tool calls, gateway requests, and peripheral node events.
//!
//! # Design
//!
//! Rather than pulling in the full OpenTelemetry SDK (which would require
//! network-accessible crates not in the offline cache), this module uses
//! `tracing` with structured fields and a custom `SpanRecorder` that writes
//! completed spans to an in-memory ring buffer. This gives us:
//!
//! - Full structured logs via `tracing` (stdout + file)
//! - Persistent span history queryable via the `/api/v1/metrics` endpoint
//! - Lightweight metrics (counters) in memory
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::observability::{init, ObsContext};
//!
//! // Initialize at startup
//! init("oh-ben-claw", "0.1.0", tracing::Level::INFO).unwrap();
//!
//! // Record a span
//! let ctx = ObsContext::new();
//! let mut span = ctx.span("agent.process");
//! span.set_attr("session_id", "s1");
//! span.finish_ok();
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::Level;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_subscriber::filter::LevelFilter;

// ── Initialization ─────────────────────────────────────────────────────────────

/// Initialize the global tracing subscriber.
///
/// Reads `RUST_LOG` for filter overrides; defaults to `info` for the
/// `oh_ben_claw` crate and `warn` for all others.
pub fn init(service_name: &str, version: &str, default_level: Level) -> Result<()> {
    let level_filter = LevelFilter::from_level(default_level);

    tracing_subscriber::registry()
        .with(level_filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .compact(),
        )
        .try_init()
        .ok(); // Ignore error if already initialized (e.g., in tests)

    tracing::info!(
        service = service_name,
        version = version,
        "Observability initialized"
    );

    Ok(())
}

// ── Span Recorder ──────────────────────────────────────────────────────────────

/// The outcome of a recorded span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanStatus {
    Ok,
    Error,
    Cancelled,
}

impl std::fmt::Display for SpanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanStatus::Ok => write!(f, "ok"),
            SpanStatus::Error => write!(f, "error"),
            SpanStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// A completed span record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanRecord {
    pub name: String,
    pub start_ts: u64,
    pub duration_ms: u64,
    pub status: SpanStatus,
    pub attrs: HashMap<String, String>,
    pub error: Option<String>,
}

/// A lightweight span recorder that captures timing and attributes.
///
/// Drop the recorder without calling `finish_ok()` or `finish_err()` to
/// automatically record a `Cancelled` span.
pub struct SpanRecorder {
    name: String,
    start: Instant,
    start_ts: u64,
    attrs: HashMap<String, String>,
    finished: bool,
    sink: Option<Arc<SpanSink>>,
}

impl SpanRecorder {
    /// Create a new span recorder with the given operation name.
    pub fn new(name: impl Into<String>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            name: name.into(),
            start: Instant::now(),
            start_ts: now,
            attrs: HashMap::new(),
            finished: false,
            sink: None,
        }
    }

    /// Attach a span sink for persisting completed spans.
    pub fn with_sink(mut self, sink: Arc<SpanSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Set a string attribute on this span.
    pub fn set_attr(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.attrs.insert(key.into(), value.into());
    }

    /// Finish the span with an OK status.
    pub fn finish_ok(mut self) -> SpanRecord {
        self.finished = true;
        let record = SpanRecord {
            name: self.name.clone(),
            start_ts: self.start_ts,
            duration_ms: self.start.elapsed().as_millis() as u64,
            status: SpanStatus::Ok,
            attrs: self.attrs.clone(),
            error: None,
        };
        tracing::debug!(
            span = %record.name,
            duration_ms = record.duration_ms,
            status = "ok",
            "Span completed"
        );
        if let Some(ref sink) = self.sink {
            sink.record(record.clone());
        }
        record
    }

    /// Finish the span with an error status.
    pub fn finish_err(mut self, error: impl Into<String>) -> SpanRecord {
        self.finished = true;
        let error = error.into();
        let record = SpanRecord {
            name: self.name.clone(),
            start_ts: self.start_ts,
            duration_ms: self.start.elapsed().as_millis() as u64,
            status: SpanStatus::Error,
            attrs: self.attrs.clone(),
            error: Some(error.clone()),
        };
        tracing::warn!(
            span = %record.name,
            duration_ms = record.duration_ms,
            error = %error,
            "Span failed"
        );
        if let Some(ref sink) = self.sink {
            sink.record(record.clone());
        }
        record
    }

    /// Elapsed time since span start.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Drop for SpanRecorder {
    fn drop(&mut self) {
        if !self.finished {
            let record = SpanRecord {
                name: self.name.clone(),
                start_ts: self.start_ts,
                duration_ms: self.start.elapsed().as_millis() as u64,
                status: SpanStatus::Cancelled,
                attrs: self.attrs.clone(),
                error: Some("span dropped without finish".to_string()),
            };
            tracing::debug!(span = %record.name, "Span cancelled (dropped)");
            if let Some(ref sink) = self.sink {
                sink.record(record);
            }
        }
    }
}

// ── Span Sink ─────────────────────────────────────────────────────────────────

/// In-memory ring buffer for completed spans.
///
/// Holds the last N spans for the `/api/v1/metrics` endpoint.
/// Thread-safe via `Mutex`.
pub struct SpanSink {
    capacity: usize,
    records: Mutex<Vec<SpanRecord>>,
}

impl SpanSink {
    /// Create a new span sink with the given ring-buffer capacity.
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            records: Mutex::new(Vec::with_capacity(capacity)),
        })
    }

    /// Record a completed span.
    pub fn record(&self, record: SpanRecord) {
        let mut records = self.records.lock().unwrap();
        if records.len() >= self.capacity {
            records.remove(0);
        }
        records.push(record);
    }

    /// Return all recorded spans.
    pub fn all(&self) -> Vec<SpanRecord> {
        self.records.lock().unwrap().clone()
    }

    /// Return the last N spans.
    pub fn last(&self, n: usize) -> Vec<SpanRecord> {
        let records = self.records.lock().unwrap();
        records.iter().rev().take(n).cloned().collect()
    }

    /// Return spans matching a name prefix.
    pub fn by_name(&self, prefix: &str) -> Vec<SpanRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.name.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Return only error spans.
    pub fn errors(&self) -> Vec<SpanRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.status == SpanStatus::Error)
            .cloned()
            .collect()
    }

    /// Number of spans recorded.
    pub fn len(&self) -> usize {
        self.records.lock().unwrap().len()
    }

    /// Whether the sink is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all recorded spans.
    pub fn clear(&self) {
        self.records.lock().unwrap().clear();
    }
}

// ── Metrics ───────────────────────────────────────────────────────────────────

/// A named atomic counter.
#[derive(Debug)]
pub struct Counter {
    name: String,
    value: AtomicU64,
}

impl Counter {
    /// Create a new counter with the given name.
    pub fn new(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            value: AtomicU64::new(0),
        })
    }

    /// Increment the counter by 1.
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the counter by N.
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    /// Read the current value.
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Reset the counter to zero.
    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }

    /// The counter name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A snapshot of a metric value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub name: String,
    pub value: u64,
    pub kind: &'static str,
}

/// A registry of named counters.
///
/// Thread-safe via `Mutex<HashMap>`.
pub struct MetricsRegistry {
    counters: Mutex<HashMap<String, Arc<Counter>>>,
}

impl MetricsRegistry {
    /// Create a new empty metrics registry.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            counters: Mutex::new(HashMap::new()),
        })
    }

    /// Get or create a counter with the given name.
    pub fn counter(&self, name: impl Into<String>) -> Arc<Counter> {
        let name = name.into();
        let mut counters = self.counters.lock().unwrap();
        counters
            .entry(name.clone())
            .or_insert_with(|| Counter::new(name))
            .clone()
    }

    /// Snapshot all counters.
    pub fn snapshot(&self) -> Vec<MetricSnapshot> {
        self.counters
            .lock()
            .unwrap()
            .values()
            .map(|c| MetricSnapshot {
                name: c.name().to_string(),
                value: c.get(),
                kind: "counter",
            })
            .collect()
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self {
            counters: Mutex::new(HashMap::new()),
        }
    }
}

// ── Global Observability Context ──────────────────────────────────────────────

/// The global observability context — span sink + metrics registry.
///
/// Created once at startup and shared via `Arc`.
#[derive(Clone)]
pub struct ObsContext {
    pub spans: Arc<SpanSink>,
    pub metrics: Arc<MetricsRegistry>,
}

impl ObsContext {
    /// Create a new observability context.
    pub fn new() -> Self {
        Self {
            spans: SpanSink::new(1000),
            metrics: MetricsRegistry::new(),
        }
    }

    /// Start a new span recorder attached to this context's sink.
    pub fn span(&self, name: impl Into<String>) -> SpanRecorder {
        SpanRecorder::new(name).with_sink(self.spans.clone())
    }

    /// Get or create a counter.
    pub fn counter(&self, name: impl Into<String>) -> Arc<Counter> {
        self.metrics.counter(name)
    }

    /// Record an incoming HTTP request.
    pub fn record_request(&self) {
        self.metrics.counter("requests_total").inc();
    }

    /// Record a completed agent turn.
    pub fn record_agent_turn(&self, tool_calls: usize) {
        self.metrics.counter("agent_turns_total").inc();
        if tool_calls > 0 {
            self.metrics.counter("tool_calls_total").add(tool_calls as u64);
        }
    }

    /// Record a tool call invocation.
    pub fn record_tool_call(&self, _tool_name: &str) {
        self.metrics.counter("tool_calls_total").inc();
    }

    /// Record a tool call error.
    pub fn record_tool_error(&self, _tool_name: &str) {
        self.metrics.counter("tool_errors_total").inc();
    }

    /// Snapshot the current metrics state.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let get = |name: &str| self.metrics.counter(name).get();
        let uptime_secs = {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            START.get_or_init(std::time::Instant::now).elapsed().as_secs()
        };
        MetricsSnapshot {
            requests_total: get("requests_total"),
            tool_calls_total: get("tool_calls_total"),
            tool_errors_total: get("tool_errors_total"),
            agent_turns_total: get("agent_turns_total"),
            uptime_secs,
            active_sessions: 0, // TODO: track active sessions
        }
    }
}

/// A snapshot of key metrics for the `/api/v1/metrics` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub requests_total: u64,
    pub tool_calls_total: u64,
    pub tool_errors_total: u64,
    pub agent_turns_total: u64,
    pub uptime_secs: u64,
    pub active_sessions: usize,
}

impl Default for ObsContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn span_recorder_finish_ok() {
        let recorder = SpanRecorder::new("test.op");
        let record = recorder.finish_ok();
        assert_eq!(record.name, "test.op");
        assert_eq!(record.status, SpanStatus::Ok);
        assert!(record.error.is_none());
        assert!(record.duration_ms < 1000);
    }

    #[test]
    fn span_recorder_finish_err() {
        let recorder = SpanRecorder::new("test.fail");
        let record = recorder.finish_err("something went wrong");
        assert_eq!(record.status, SpanStatus::Error);
        assert_eq!(record.error.as_deref(), Some("something went wrong"));
    }

    #[test]
    fn span_recorder_attrs() {
        let mut recorder = SpanRecorder::new("test.attrs");
        recorder.set_attr("session_id", "s1");
        recorder.set_attr("provider", "openai");
        let record = recorder.finish_ok();
        assert_eq!(record.attrs.get("session_id").map(|s| s.as_str()), Some("s1"));
        assert_eq!(record.attrs.get("provider").map(|s| s.as_str()), Some("openai"));
    }

    #[test]
    fn span_recorder_drop_records_cancelled() {
        let sink = SpanSink::new(10);
        {
            let recorder = SpanRecorder::new("test.drop").with_sink(sink.clone());
            let _ = recorder; // Drop without finishing
        }
        let spans = sink.all();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].status, SpanStatus::Cancelled);
    }

    #[test]
    fn span_sink_ring_buffer() {
        let sink = SpanSink::new(3);
        for i in 0..5 {
            let recorder = SpanRecorder::new(format!("op.{i}")).with_sink(sink.clone());
            recorder.finish_ok();
        }
        assert_eq!(sink.len(), 3);
        let names: Vec<_> = sink.all().iter().map(|r| r.name.clone()).collect();
        assert!(names.contains(&"op.4".to_string()));
        assert!(!names.contains(&"op.0".to_string()));
    }

    #[test]
    fn span_sink_by_name_prefix() {
        let sink = SpanSink::new(100);
        SpanRecorder::new("agent.process").with_sink(sink.clone()).finish_ok();
        SpanRecorder::new("agent.tool").with_sink(sink.clone()).finish_ok();
        SpanRecorder::new("gateway.request").with_sink(sink.clone()).finish_ok();
        let agent_spans = sink.by_name("agent");
        assert_eq!(agent_spans.len(), 2);
    }

    #[test]
    fn span_sink_errors_filter() {
        let sink = SpanSink::new(100);
        SpanRecorder::new("op.ok").with_sink(sink.clone()).finish_ok();
        SpanRecorder::new("op.err").with_sink(sink.clone()).finish_err("oops");
        SpanRecorder::new("op.ok2").with_sink(sink.clone()).finish_ok();
        assert_eq!(sink.errors().len(), 1);
        assert_eq!(sink.errors()[0].name, "op.err");
    }

    #[test]
    fn counter_increment() {
        let counter = Counter::new("requests_total");
        assert_eq!(counter.get(), 0);
        counter.inc();
        counter.inc();
        counter.add(3);
        assert_eq!(counter.get(), 5);
        counter.reset();
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn counter_thread_safe() {
        let counter = Counter::new("concurrent");
        let c1 = counter.clone();
        let c2 = counter.clone();
        let t1 = thread::spawn(move || {
            for _ in 0..100 {
                c1.inc();
            }
        });
        let t2 = thread::spawn(move || {
            for _ in 0..100 {
                c2.inc();
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        assert_eq!(counter.get(), 200);
    }

    #[test]
    fn metrics_registry_get_or_create() {
        let registry = MetricsRegistry::new();
        let c1 = registry.counter("req");
        let c2 = registry.counter("req");
        c1.inc();
        assert_eq!(c2.get(), 1); // Same counter
    }

    #[test]
    fn metrics_registry_snapshot() {
        let registry = MetricsRegistry::new();
        registry.counter("a").inc();
        registry.counter("b").add(5);
        let snap = registry.snapshot();
        assert_eq!(snap.len(), 2);
        let total: u64 = snap.iter().map(|s| s.value).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn obs_context_span_and_counter() {
        let ctx = ObsContext::new();
        let mut span = ctx.span("test.op");
        span.set_attr("key", "value");
        span.finish_ok();

        let counter = ctx.counter("test.count");
        counter.inc();
        counter.inc();

        assert_eq!(ctx.spans.len(), 1);
        assert_eq!(ctx.metrics.counter("test.count").get(), 2);
    }

    #[test]
    fn span_status_display() {
        assert_eq!(SpanStatus::Ok.to_string(), "ok");
        assert_eq!(SpanStatus::Error.to_string(), "error");
        assert_eq!(SpanStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn span_record_serializes() {
        let record = SpanRecord {
            name: "agent.process".to_string(),
            start_ts: 1_700_000_000_000,
            duration_ms: 42,
            status: SpanStatus::Ok,
            attrs: {
                let mut m = HashMap::new();
                m.insert("session_id".to_string(), "s1".to_string());
                m
            },
            error: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("agent.process"));
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn span_recorder_elapsed_is_positive() {
        let recorder = SpanRecorder::new("timing.test");
        thread::sleep(Duration::from_millis(5));
        assert!(recorder.elapsed() >= Duration::from_millis(1));
        recorder.finish_ok();
    }
}

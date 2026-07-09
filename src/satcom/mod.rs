//! Satellite tier — store-and-forward outbox for constrained links (G6, connectivity).
//!
//! Off-grid sites often have no cell and no LoRa gateway in range, but *do* have an
//! intermittent, expensive, tiny-MTU satellite path (Iridium SBD, Swarm). This module is
//! the buffer between the rest of the stack and that link: messages are **enqueued** any
//! time, held in a bounded priority queue, and **drained in bytes-budgeted batches** only
//! when a transmission window opens (a satellite pass). Urgent messages jump the queue;
//! stale ones can be aged out; when the buffer is full the least-urgent message is evicted
//! so an alert never loses its slot to routine telemetry.
//!
//! Pure and transport-free: a real modem driver calls [`SatOutbox::drain_window`] when it
//! has airtime and ships the returned batch. No I/O here — just the queueing policy, so it
//! is fully deterministic and testable. Pairs with [`crate::spine`] / [`crate::comms`],
//! which decide *when* the satellite path is the one to use.

use serde::{Deserialize, Serialize};

/// A message waiting for a satellite transmission window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SatMessage {
    pub id: String,
    /// Higher = more urgent. Drained first; survives eviction longest.
    pub priority: u8,
    /// Raw payload bytes (kept small — satellite MTUs are tiny).
    pub payload: Vec<u8>,
    /// Creation time (ms); drives FIFO tie-breaks and TTL expiry.
    pub created_ms: u64,
}

impl SatMessage {
    pub fn new(id: impl Into<String>, priority: u8, payload: impl Into<Vec<u8>>, created_ms: u64) -> Self {
        Self { id: id.into(), priority, payload: payload.into(), created_ms }
    }

    pub fn len(&self) -> usize {
        self.payload.len()
    }

    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }
}

/// Outbox limits. Defaults model an Iridium SBD-ish link: ~340-byte messages, a modest
/// per-pass byte budget, and a bounded backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxConfig {
    /// Maximum queued messages before eviction kicks in.
    pub max_messages: usize,
    /// Largest single payload accepted (bytes). Bigger enqueues are rejected.
    pub max_message_bytes: usize,
    /// Bytes drainable in one transmission window.
    pub window_bytes: usize,
    /// Optional message time-to-live (ms); older messages expire.
    pub ttl_ms: Option<u64>,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self { max_messages: 64, max_message_bytes: 340, window_bytes: 340, ttl_ms: None }
    }
}

/// Why an enqueue was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Payload exceeds `max_message_bytes`.
    TooLarge { size: usize, max: usize },
    /// Queue is full and the incoming message is not more urgent than any queued one.
    Full,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::TooLarge { size, max } => {
                write!(f, "payload {size} B exceeds max {max} B")
            }
            RejectReason::Full => write!(f, "outbox full and message not urgent enough"),
        }
    }
}

/// A store-and-forward outbox: enqueue any time, drain in windowed batches.
#[derive(Debug, Clone)]
pub struct SatOutbox {
    cfg: OutboxConfig,
    queue: Vec<SatMessage>,
}

impl SatOutbox {
    pub fn new(cfg: OutboxConfig) -> Self {
        Self { cfg, queue: Vec::new() }
    }

    pub fn with_default() -> Self {
        Self::new(OutboxConfig::default())
    }

    pub fn config(&self) -> &OutboxConfig {
        &self.cfg
    }

    /// Enqueue a message. Returns `Ok(None)` when it was simply queued, `Ok(Some(evicted))`
    /// when it displaced a lower-priority message (queue was full), or `Err(reason)` when
    /// rejected (too large, or full with nothing less urgent to drop).
    pub fn enqueue(&mut self, msg: SatMessage) -> Result<Option<SatMessage>, RejectReason> {
        if msg.payload.len() > self.cfg.max_message_bytes {
            return Err(RejectReason::TooLarge { size: msg.payload.len(), max: self.cfg.max_message_bytes });
        }
        if self.queue.len() < self.cfg.max_messages {
            self.queue.push(msg);
            return Ok(None);
        }
        // Full: find the least-urgent queued message (lowest priority, oldest on ties).
        let victim = self
            .queue
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.priority.cmp(&b.priority).then(a.created_ms.cmp(&b.created_ms))
            })
            .map(|(i, _)| i);
        match victim {
            Some(i) if msg.priority > self.queue[i].priority => {
                let evicted = self.queue.remove(i);
                self.queue.push(msg);
                Ok(Some(evicted))
            }
            _ => Err(RejectReason::Full),
        }
    }

    /// Drop and return messages older than the configured TTL. No-op without a TTL.
    pub fn expire(&mut self, now_ms: u64) -> Vec<SatMessage> {
        let Some(ttl) = self.cfg.ttl_ms else { return Vec::new() };
        let mut expired = Vec::new();
        let mut kept = Vec::with_capacity(self.queue.len());
        for m in self.queue.drain(..) {
            if now_ms.saturating_sub(m.created_ms) > ttl {
                expired.push(m);
            } else {
                kept.push(m);
            }
        }
        self.queue = kept;
        expired
    }

    /// Indices into `queue` in send order: priority descending, then oldest first.
    fn send_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.queue.len()).collect();
        order.sort_by(|&a, &b| {
            self.queue[b]
                .priority
                .cmp(&self.queue[a].priority)
                .then(self.queue[a].created_ms.cmp(&self.queue[b].created_ms))
        });
        order
    }

    /// Select and remove the next batch to transmit: TTL-expire first, then take messages
    /// in send order while they fit the window budget. The single most-urgent message is
    /// always taken (so an oversized top-priority message can never permanently stall the
    /// queue); after that, strict priority order stops at the first message that won't fit.
    pub fn drain_window(&mut self, now_ms: u64) -> Vec<SatMessage> {
        self.expire(now_ms);
        if self.queue.is_empty() {
            return Vec::new();
        }
        let order = self.send_order();
        let mut used = 0usize;
        let mut chosen_ids: Vec<String> = Vec::new();
        for (rank, &idx) in order.iter().enumerate() {
            let len = self.queue[idx].payload.len();
            if rank == 0 || used + len <= self.cfg.window_bytes {
                used += len;
                chosen_ids.push(self.queue[idx].id.clone());
            } else {
                break;
            }
        }
        let mut sent = Vec::with_capacity(chosen_ids.len());
        for id in &chosen_ids {
            if let Some(pos) = self.queue.iter().position(|m| &m.id == id) {
                sent.push(self.queue.remove(pos));
            }
        }
        sent
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    pub fn pending_bytes(&self) -> usize {
        self.queue.iter().map(|m| m.payload.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Queued messages in send order (for status/inspection); does not mutate the queue.
    pub fn peek_order(&self) -> Vec<&SatMessage> {
        self.send_order().into_iter().map(|i| &self.queue[i]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max_messages: usize, max_message_bytes: usize, window_bytes: usize, ttl_ms: Option<u64>) -> OutboxConfig {
        OutboxConfig { max_messages, max_message_bytes, window_bytes, ttl_ms }
    }

    fn msg(id: &str, priority: u8, bytes: usize, created_ms: u64) -> SatMessage {
        SatMessage::new(id, priority, vec![0u8; bytes], created_ms)
    }

    #[test]
    fn enqueue_accepts_under_capacity() {
        let mut ob = SatOutbox::new(cfg(4, 100, 100, None));
        assert_eq!(ob.enqueue(msg("a", 1, 10, 0)), Ok(None));
        assert_eq!(ob.pending(), 1);
        assert_eq!(ob.pending_bytes(), 10);
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let mut ob = SatOutbox::new(cfg(4, 50, 340, None));
        let r = ob.enqueue(msg("big", 5, 100, 0));
        assert_eq!(r, Err(RejectReason::TooLarge { size: 100, max: 50 }));
        assert!(ob.is_empty());
    }

    #[test]
    fn full_queue_evicts_lowest_priority_for_a_more_urgent_message() {
        let mut ob = SatOutbox::new(cfg(2, 100, 100, None));
        ob.enqueue(msg("low", 1, 10, 0)).unwrap();
        ob.enqueue(msg("mid", 3, 10, 1)).unwrap();
        // Full. An urgent message evicts the lowest-priority ("low").
        let evicted = ob.enqueue(msg("hi", 9, 10, 2)).unwrap();
        assert_eq!(evicted.map(|m| m.id), Some("low".to_string()));
        assert_eq!(ob.pending(), 2);
        let ids: Vec<_> = ob.peek_order().iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["hi".to_string(), "mid".to_string()]);
    }

    #[test]
    fn full_queue_rejects_a_message_no_more_urgent_than_the_least() {
        let mut ob = SatOutbox::new(cfg(2, 100, 100, None));
        ob.enqueue(msg("a", 5, 10, 0)).unwrap();
        ob.enqueue(msg("b", 5, 10, 1)).unwrap();
        // Incoming priority equals the lowest queued priority => rejected.
        assert_eq!(ob.enqueue(msg("c", 5, 10, 2)), Err(RejectReason::Full));
        assert_eq!(ob.pending(), 2);
    }

    #[test]
    fn drain_takes_highest_priority_first() {
        let mut ob = SatOutbox::new(cfg(8, 100, 1000, None));
        ob.enqueue(msg("low", 1, 10, 0)).unwrap();
        ob.enqueue(msg("hi", 9, 10, 1)).unwrap();
        ob.enqueue(msg("mid", 5, 10, 2)).unwrap();
        let batch = ob.drain_window(10);
        let ids: Vec<_> = batch.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["hi".to_string(), "mid".to_string(), "low".to_string()]);
        assert!(ob.is_empty());
    }

    #[test]
    fn same_priority_drains_oldest_first() {
        let mut ob = SatOutbox::new(cfg(8, 100, 1000, None));
        ob.enqueue(msg("newer", 5, 10, 20)).unwrap();
        ob.enqueue(msg("older", 5, 10, 5)).unwrap();
        let batch = ob.drain_window(30);
        let ids: Vec<_> = batch.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["older".to_string(), "newer".to_string()]);
    }

    #[test]
    fn drain_respects_the_window_byte_budget() {
        let mut ob = SatOutbox::new(cfg(8, 100, 25, None));
        ob.enqueue(msg("a", 9, 10, 0)).unwrap();
        ob.enqueue(msg("b", 8, 10, 1)).unwrap();
        ob.enqueue(msg("c", 7, 10, 2)).unwrap();
        // Window = 25 B: a(10)+b(10)=20 fit, c(10) would make 30 > 25 => held.
        let batch = ob.drain_window(10);
        let ids: Vec<_> = batch.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(ob.pending(), 1);
        assert_eq!(ob.peek_order()[0].id, "c");
    }

    #[test]
    fn oversized_top_priority_message_still_drains_alone() {
        // A single top-priority message larger than the window must not stall forever.
        let mut ob = SatOutbox::new(cfg(8, 500, 100, None));
        ob.enqueue(msg("huge", 9, 400, 0)).unwrap();
        ob.enqueue(msg("small", 1, 10, 1)).unwrap();
        let batch = ob.drain_window(10);
        // Rank-0 "huge" is always taken; "small" then can't fit (used already > window).
        assert_eq!(batch.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec!["huge".to_string()]);
        assert_eq!(ob.pending(), 1);
    }

    #[test]
    fn ttl_expiry_drops_stale_messages_on_drain() {
        let mut ob = SatOutbox::new(cfg(8, 100, 1000, Some(100)));
        ob.enqueue(msg("stale", 9, 10, 0)).unwrap();
        ob.enqueue(msg("fresh", 1, 10, 950)).unwrap();
        // now=1000: stale age=1000 > ttl 100 (expired even though top priority); fresh age=50 ok.
        let batch = ob.drain_window(1000);
        let ids: Vec<_> = batch.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["fresh".to_string()]);
        assert!(ob.is_empty());
    }

    #[test]
    fn expire_returns_the_dropped_messages() {
        let mut ob = SatOutbox::new(cfg(8, 100, 1000, Some(50)));
        ob.enqueue(msg("old", 5, 10, 0)).unwrap();
        ob.enqueue(msg("new", 5, 10, 100)).unwrap();
        let dropped = ob.expire(120);
        assert_eq!(dropped.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec!["old".to_string()]);
        assert_eq!(ob.pending(), 1);
    }

    #[test]
    fn draining_an_empty_outbox_is_empty() {
        let mut ob = SatOutbox::with_default();
        assert!(ob.drain_window(0).is_empty());
    }
}

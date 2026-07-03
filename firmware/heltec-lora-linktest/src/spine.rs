//! OBC spine frame, carried over the LoRa link.
//!
//! The LoRa spine is a **content-agnostic transport**: it moves opaque OBC
//! payloads (a node's newline-delimited JSON messages) wrapped in a tiny header
//! for source routing and de-duplication. This is the on-air format that the
//! host-side `lora_mesh` spine transport mirrors.
//!
//! Wire format (single frame): `[src:u8][seq:u8][ttl:u8][payload…]`
//!   - `src`  — originating node id (low byte of its MAC).
//!   - `seq`  — per-source sequence number (wraps); with `src` it de-dups relays.
//!   - `ttl`  — remaining hop count for flood-relay (mesh; 0 = don't relay).
//!   - `payload` — the OBC message bytes (≤ [`MAX_PAYLOAD`]).

/// Conservative single-frame LoRa payload budget (SX1262 supports up to 255, but
/// we leave headroom for the header and radio overhead).
pub const MAX_PAYLOAD: usize = 240;
/// Header length: src + seq + ttl.
pub const HEADER: usize = 3;

/// A decoded spine frame borrowing its payload from the receive buffer.
pub struct SpineFrame<'a> {
    pub src: u8,
    pub seq: u8,
    pub ttl: u8,
    pub payload: &'a [u8],
}

impl<'a> SpineFrame<'a> {
    /// Serialize into `out` (cleared first).
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.clear();
        out.push(self.src);
        out.push(self.seq);
        out.push(self.ttl);
        let n = self.payload.len().min(MAX_PAYLOAD);
        out.extend_from_slice(&self.payload[..n]);
    }

    /// Parse a received frame. `None` if too short to hold a header.
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < HEADER {
            return None;
        }
        Some(Self {
            src: bytes[0],
            seq: bytes[1],
            ttl: bytes[2],
            payload: &bytes[HEADER..],
        })
    }
}

/// A small fixed ring of recently-seen `(src, seq)` pairs for de-duplication —
/// the seed of mesh flood-relay (drop a frame you've already handled, so relays
/// don't loop). A linear scan over a tiny ring is plenty fast on the MCU.
pub struct SeenSet {
    ring: [(u8, u8); Self::CAP],
    head: usize,
    len: usize,
}

impl SeenSet {
    const CAP: usize = 32;

    pub const fn new() -> Self {
        Self { ring: [(0, 0); Self::CAP], head: 0, len: 0 }
    }

    /// Returns `true` if `(src, seq)` was already recorded; otherwise records it
    /// and returns `false`.
    pub fn seen_or_insert(&mut self, src: u8, seq: u8) -> bool {
        for i in 0..self.len {
            let idx = (self.head + Self::CAP - 1 - i) % Self::CAP;
            if self.ring[idx] == (src, seq) {
                return true;
            }
        }
        self.ring[self.head] = (src, seq);
        self.head = (self.head + 1) % Self::CAP;
        if self.len < Self::CAP {
            self.len += 1;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_dedup() {
        let mut buf = Vec::new();
        SpineFrame { src: 0x40, seq: 7, ttl: 2, payload: b"{\"t\":\"hb\"}" }.encode(&mut buf);
        let f = SpineFrame::decode(&buf).unwrap();
        assert_eq!((f.src, f.seq, f.ttl), (0x40, 7, 2));
        assert_eq!(f.payload, b"{\"t\":\"hb\"}");

        let mut seen = SeenSet::new();
        assert!(!seen.seen_or_insert(0x40, 7));
        assert!(seen.seen_or_insert(0x40, 7)); // duplicate
        assert!(!seen.seen_or_insert(0x40, 8));
        assert!(!seen.seen_or_insert(0x41, 7)); // different source
    }
}

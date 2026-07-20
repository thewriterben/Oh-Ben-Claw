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

/// True if a framed line is an OBC message rather than console noise.
///
/// The spine is content-agnostic about *payloads*, but not about what deserves
/// airtime. GPIO43 on the XIAO is also the ROM's `U0TXD`, so every node reset
/// dumps the ROM and bootloader log down the uplink wire before the application
/// ever configures UART1. Observed 2026-07-19: a single reboot put fifteen frames
/// on the air — `ESP-ROM:esp32s3-20210327`, `load:0x3fce2820,len:0x158c`, and so
/// on — several of them mangled, because the early output is not even at the same
/// baud. On a duty-cycle-limited band that is airtime and sequence numbers spent
/// on nothing, and it crowds out the traffic the mesh exists to carry.
///
/// Every OBC message is a JSON object, so the test is cheap and total: an opening
/// brace and a closing brace. Anything else is noise by construction.
pub fn is_spine_payload(line: &[u8]) -> bool {
    let t = trim_ascii_ws(line);
    matches!((t.first(), t.last()), (Some(b'{'), Some(b'}')))
}

fn trim_ascii_ws(mut s: &[u8]) -> &[u8] {
    while let [f, rest @ ..] = s {
        if f.is_ascii_whitespace() { s = rest } else { break }
    }
    while let [rest @ .., l] = s {
        if l.is_ascii_whitespace() { s = rest } else { break }
    }
    s
}

/// Accumulates bytes and yields complete newline-delimited lines.
///
/// Both origins on this board — the USB console and the UART1 compute uplink —
/// framed lines by hand, and both carried the same defect: a line that outgrew
/// [`MAX_PAYLOAD`] had its overflow silently dropped and its *prefix* sent on as
/// if it were the whole message. For JSON that produces a corrupt command which
/// still transmits, still costs airtime, and still has to be parsed and rejected
/// at the far end. Failing loudly and dropping the line is strictly better.
///
/// Feed bytes with [`push`](Self::push); it yields [`Framed::Line`] exactly once
/// per complete, in-budget line. The caller borrows that line to send it and does
/// nothing else — the buffer is reset on the following `push`, because a caller
/// holding the borrow cannot also hand the framer back a `&mut` to clear it.
pub struct LineFramer {
    line: Vec<u8>,
    overflowed: bool,
    /// The last `push` handed out a line; clear it before accumulating more.
    emitted: bool,
}

/// What [`LineFramer::push`] decided about the byte just fed to it.
pub enum Framed<'a> {
    /// Still accumulating.
    Pending,
    /// A complete line, within budget.
    Line(&'a [u8]),
    /// A line ended, but it exceeded [`MAX_PAYLOAD`] and was discarded.
    Overflow,
}

impl LineFramer {
    pub const fn new() -> Self {
        Self { line: Vec::new(), overflowed: false, emitted: false }
    }

    pub fn push(&mut self, b: u8) -> Framed<'_> {
        if self.emitted {
            self.line.clear();
            self.emitted = false;
        }
        if b == b'\n' || b == b'\r' {
            if self.overflowed {
                self.overflowed = false;
                self.line.clear();
                return Framed::Overflow;
            }
            if self.line.is_empty() {
                // Bare newline, or the second half of a CRLF. Not a line.
                return Framed::Pending;
            }
            self.emitted = true;
            return Framed::Line(&self.line);
        }
        if self.line.len() < MAX_PAYLOAD {
            self.line.push(b);
        } else {
            self.overflowed = true;
        }
        Framed::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a byte slice; collect the lines it yields and count the overflows.
    fn run(f: &mut LineFramer, bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
        let (mut lines, mut overflows) = (Vec::new(), 0);
        for &b in bytes {
            match f.push(b) {
                Framed::Line(l) => lines.push(l.to_vec()),
                Framed::Overflow => overflows += 1,
                Framed::Pending => {}
            }
        }
        (lines, overflows)
    }

    #[test]
    fn two_commands_in_one_burst_survive_intact() {
        // The 2026-07-19 failure: the host wrote two commands back to back and the
        // board saw them as one 176-byte burst. Whatever else is true, arriving
        // together must not corrupt either one.
        let mut f = LineFramer::new();
        let burst = b"{\"cmd\":\"capabilities\",\"to\":\"obc-esp32-s3-001\"}\n\
                      {\"cmd\":\"capabilities\",\"to\":\"gw-40\"}\n";
        let (lines, overflows) = run(&mut f, burst);
        assert_eq!(overflows, 0);
        assert_eq!(lines.len(), 2, "both commands must survive the burst");
        assert_eq!(lines[0], b"{\"cmd\":\"capabilities\",\"to\":\"obc-esp32-s3-001\"}");
        assert_eq!(lines[1], b"{\"cmd\":\"capabilities\",\"to\":\"gw-40\"}");
    }

    #[test]
    fn an_oversized_line_is_discarded_whole_not_truncated() {
        // The important half: no prefix escapes. A clipped JSON command is not a
        // shorter command, it is a corrupt one.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_PAYLOAD + 50];
        burst.push(b'\n');
        let (lines, overflows) = run(&mut f, &burst);
        assert!(lines.is_empty(), "a truncated prefix must never be emitted");
        assert_eq!(overflows, 1);
    }

    #[test]
    fn the_framer_recovers_after_an_overflow() {
        // An oversized line must not poison the next one.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_PAYLOAD + 1];
        burst.extend_from_slice(b"\n{\"cmd\":\"capabilities\"}\n");
        let (lines, overflows) = run(&mut f, &burst);
        assert_eq!(overflows, 1);
        assert_eq!(lines, vec![b"{\"cmd\":\"capabilities\"}".to_vec()]);
    }

    #[test]
    fn crlf_and_blank_lines_do_not_produce_empty_frames() {
        let mut f = LineFramer::new();
        let (lines, overflows) = run(&mut f, b"\r\n\r\n{\"cmd\":\"x\"}\r\n\r\n");
        assert_eq!(overflows, 0);
        assert_eq!(lines, vec![b"{\"cmd\":\"x\"}".to_vec()]);
    }

    #[test]
    fn a_line_split_across_reads_is_reassembled() {
        // Bulk reads land on arbitrary boundaries; the framer must not care.
        let mut f = LineFramer::new();
        let (a, _) = run(&mut f, b"{\"cmd\":\"cap");
        assert!(a.is_empty());
        let (b, _) = run(&mut f, b"abilities\"}\n");
        assert_eq!(b, vec![b"{\"cmd\":\"capabilities\"}".to_vec()]);
    }

    #[test]
    fn a_line_exactly_at_the_budget_is_kept() {
        // Off-by-one guard: MAX_PAYLOAD bytes fit, MAX_PAYLOAD + 1 does not.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_PAYLOAD];
        burst.push(b'\n');
        let (lines, overflows) = run(&mut f, &burst);
        assert_eq!(overflows, 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_PAYLOAD);
    }

    #[test]
    fn boot_chatter_is_not_a_spine_payload() {
        // Real lines captured off the uplink wire after a node reset, 2026-07-19.
        for junk in [
            &b"ESP-ROM:esp32s3-20210327"[..],
            b"Build:Mar 27 2021",
            b"rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)",
            b"SPIWP:0xee",
            b"mode:DIO, clock div:2",
            b"load:0x3fce2820,len:0x158c",
            b"entry 0x403c8924",
            b"I (29) boot: ESP-IDF v5.5.1-838-gd66ebb86d2e 2nd stage bootloader",
            // Mangled mid-line splices, also observed on air.
            b"I (30) boot: compile time Nov 26 20size=c276ch (796524) map",
            b"",
        ] {
            assert!(!is_spine_payload(junk), "would have transmitted: {:?}", junk);
        }
    }

    #[test]
    fn real_node_messages_are_spine_payloads() {
        for msg in [
            &br#"{"node_id":"obc-esp32-s3-001","ts_ms":30148,"type":"beacon"}"#[..],
            br#"{"node_id":"gw-90","type":"gw_keepalive","seq":59}"#,
            br#"  {"type":"reflex","applied":false}  "#, // surrounding whitespace
        ] {
            assert!(is_spine_payload(msg), "would have dropped: {:?}", msg);
        }
    }

    #[test]
    fn a_brace_alone_is_not_enough() {
        // Guard against a filter that only checks the first byte: a truncated or
        // interleaved line can open a brace and never close it.
        assert!(!is_spine_payload(br#"{"node_id":"obc-esp32-s3-0"#));
        assert!(!is_spine_payload(b"{"));
        assert!(is_spine_payload(b"{}"));
    }

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

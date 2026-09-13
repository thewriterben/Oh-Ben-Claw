//! OBC spine frame, carried over the LoRa link.
//!
//! The LoRa spine is a **content-agnostic transport**: it moves opaque OBC
//! payloads (a node's newline-delimited JSON messages) wrapped in a tiny header
//! for source routing and de-duplication. This is the on-air format that the
//! host-side `lora_mesh` spine transport mirrors.
//!
//! Wire format v2 — `SPINE-AUTH.md` §3.2, on the air since 2026-09-13:
//! `[src:u8][seq:u8][ttl:u8][ctr:u32 BE][payload…][mac:8]`
//!   - `src`  — originating station id (low byte of its MAC).
//!   - `seq`  — low byte of `ctr`, kept in the header so the log lines and the
//!     host's parser (`seq=` field) read as they always have.
//!   - `ttl`  — remaining hop count for flood-relay (mesh; 0 = don't relay).
//!   - `ctr`  — the sender's frame counter ([`SeqCounter`]), never reissued
//!     across a reboot. Covered by the tag and judged by the receiver's
//!     [`ReplayWindow`], which is also what de-duplicates flood relays.
//!   - `payload` — the OBC message bytes (≤ [`MAX_AUTH_PAYLOAD`]).
//!   - `mac`  — the first 8 bytes of HMAC-SHA256 over `src ‖ ctr ‖ payload`
//!     under the sender's key (`auth.rs`). `ttl` is deliberately outside the
//!     tag: relays decrement it in flight.
//!
//! Format v1 — `[src][seq][ttl][payload]`, no counter, no tag — was retired
//! with this change rather than kept beside it. There is no permissive mode:
//! an authentication layer with a fallback to no authentication is one an
//! attacker turns off (`SPINE-AUTH.md` §4). A station receiving a v1 frame
//! sees a tag that does not verify and drops it, logged.

/// Conservative single-frame LoRa payload budget (SX1262 supports up to 255, but
/// we leave headroom for the header and radio overhead).
pub const MAX_PAYLOAD: usize = 240;
/// Header length: src + seq + ttl.
pub const HEADER: usize = 3;
/// Bytes the v2 frame adds: `ctr:u32` + `mac:8`.
pub const AUTH_OVERHEAD: usize = 4 + 8;
/// Payload budget of an authenticated frame. The host's `MESH_LINE_BUDGET`
/// must equal this; `tests/spine_payload_budget.rs` pins them together.
pub const MAX_AUTH_PAYLOAD: usize = MAX_PAYLOAD - AUTH_OVERHEAD;
/// Tag bytes on the wire (`auth::TAG_LEN`, restated here so this module stays
/// free of the crypto crates and compiles in every harness).
pub const MAC_LEN: usize = 8;

/// A decoded authenticated frame. Decoding checks the *shape* only; the tag is
/// the caller's to verify (`auth::verify`) and the counter the
/// [`ReplayWindow`]'s to judge — this module holds no key.
pub struct AuthFrame<'a> {
    pub src: u8,
    pub seq: u8,
    pub ttl: u8,
    pub ctr: u32,
    pub payload: &'a [u8],
    pub mac: [u8; MAC_LEN],
}

impl<'a> AuthFrame<'a> {
    /// Serialize into `out` (cleared first). The payload is cut at
    /// [`MAX_AUTH_PAYLOAD`] — the caller is expected to have refused anything
    /// longer, since a cut payload would no longer match its tag.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.clear();
        out.push(self.src);
        out.push(self.seq);
        out.push(self.ttl);
        out.extend_from_slice(&self.ctr.to_be_bytes());
        let n = self.payload.len().min(MAX_AUTH_PAYLOAD);
        out.extend_from_slice(&self.payload[..n]);
        out.extend_from_slice(&self.mac);
    }

    /// Parse a received frame. `None` if too short to hold header, counter
    /// and tag — which is also what a v1 frame shorter than 15 bytes is.
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < HEADER + AUTH_OVERHEAD {
            return None;
        }
        let ctr = u32::from_be_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]);
        let end = bytes.len() - MAC_LEN;
        let mut mac = [0u8; MAC_LEN];
        mac.copy_from_slice(&bytes[end..]);
        Some(Self {
            src: bytes[0],
            seq: bytes[1],
            ttl: bytes[2],
            ctr,
            payload: &bytes[HEADER + 4..end],
            mac,
        })
    }
}

/// The receiver's anti-replay window for one source — RFC 4303 §3.4.3, as
/// `SPINE-REPLAY.md` §3 specifies: a highest-accepted counter `h` and a
/// 64-bit bitmap of the 64 below it. Duplicates from flood relay and ordinary
/// re-ordering are accepted exactly once; anything older than the window or
/// already marked is refused.
///
/// Persistence rounds the *opposite* way from the sender's: the receiver
/// persists a ceiling `h + M` and resumes *at* it, so a restart rejects up to
/// `M` legitimate frames while the sender catches up rather than accepting a
/// replay of anything in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayWindow {
    h: u32,
    bitmap: u64,
    /// The ceiling last persisted for this source; `h` never passes it
    /// without the store first being moved.
    persisted: u32,
}

/// What the window said about a counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// Fresh: newer than anything seen, or inside the window and unmarked.
    Accept,
    /// Older than the window can judge.
    TooOld,
    /// Inside the window and already accepted once.
    Seen,
}

impl ReplayWindow {
    /// Frames a receiver may reject after a restart while the sender catches
    /// up — the persisted ceiling is `h + M`. Chosen for a tolerable gap, not
    /// for symmetry with the sender's reserve: at one keepalive per 5 s this
    /// is under a minute of silence after a bridge reboot.
    pub const M: u32 = 8;
    const WIDTH: u32 = 64;

    /// Resume for one source from its persisted ceiling (0 if never heard).
    /// Everything at or below the ceiling is treated as already seen — the
    /// bitmap starts full, not empty — because the whole point of resuming
    /// *at* the ceiling is that a restart accepts nothing it might have
    /// accepted before.
    pub fn resume(persisted_ceiling: u32) -> Self {
        Self {
            h: persisted_ceiling,
            bitmap: u64::MAX,
            persisted: persisted_ceiling,
        }
    }

    /// Judge `ctr`, and mark it if accepted. `store_needed` is `Some(ceiling)`
    /// when the caller must persist a new ceiling for this source before the
    /// frame is acted on; a failed write must be treated as a rejection.
    pub fn accept(&mut self, ctr: u32) -> (Replay, Option<u32>) {
        if ctr > self.h {
            let shift = ctr - self.h;
            self.bitmap = if shift >= Self::WIDTH {
                0
            } else {
                self.bitmap << shift
            };
            self.bitmap |= 1;
            self.h = ctr;
            let need = if ctr >= self.persisted {
                self.persisted = ctr.saturating_add(Self::M);
                Some(self.persisted)
            } else {
                None
            };
            return (Replay::Accept, need);
        }
        let back = self.h - ctr;
        if back >= Self::WIDTH {
            return (Replay::TooOld, None);
        }
        if self.bitmap & (1u64 << back) != 0 {
            return (Replay::Seen, None);
        }
        self.bitmap |= 1u64 << back;
        (Replay::Accept, None)
    }
}

// ── The frame counter that survives a reboot ───────────────────────────────
//
// The counter is what makes the tag mean something over time: a receiver
// accepts each `(src, ctr)` once, so a captured frame verifies and is still
// refused. That only holds if the sender never reissues a counter — including
// across a reboot, which is where a RAM counter fails. Measured 2026-09-12,
// one layer down: the base station was reset (a serial port opened with DTR
// is enough), its 8-bit `seq` restarted at 0, and gw-40's de-dup ring dropped
// the next commands as duplicates; four recorded bench runs "sent" `descend`
// frames that never left the ring. The cure was built for `seq` first and the
// u32 counter now rides on it unchanged.
//
// `OBC-Prime/docs/SPINE-REPLAY.md` §2: persist a *ceiling*, not a position.
// On boot, start at the ceiling and authorise the next `RESERVE` before using
// any; extend before crossing. A crash costs at most `RESERVE` numbers and
// never repeats one. The receiver's window (§3, [`ReplayWindow`]) tolerates
// the gap by construction: a skipped counter is simply one that never
// arrives, and anything above its high-water mark is fresh.
//
// This is the pure half; the flash half is a [`CeilingStore`] the firmware
// binds to NVS. Everything here is testable on the host, and is.

/// Where ceilings live across a power cycle. NVS on the board; a map in tests.
/// Keyed, because one namespace holds the sender's counter (`seq_ceil`) and
/// one receiver window per source (`rx_<src>`). Errors are the store's own
/// (partition full, worn, absent); every user of this trait treats any of
/// them as a reason to stop — the sender stops issuing numbers, the receiver
/// rejects the frame.
pub trait CeilingStore {
    /// The persisted value under `key`, or 0 if none has been written yet.
    fn read(&mut self, key: &str) -> Result<u32, StoreError>;
    /// Persist `value` under `key`. Must not return until it is durable.
    fn write(&mut self, key: &str, value: u32) -> Result<(), StoreError>;
}

/// NVS key of the sender's counter ceiling.
pub const SEQ_CEILING_KEY: &str = "seq_ceil";

/// NVS key of the receiver ceiling for frames from `src` (`rx_40`, `rx_d8`).
/// NVS keys are at most 15 bytes; this is 5.
pub fn rx_ceiling_key(src: u8) -> String {
    format!("rx_{src:02x}")
}

/// The store could not be read or written. No payload: the response is the
/// same regardless of why (see [`SeqCounter::next`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreError;

/// This station's frame counter — the `ctr` in every frame it originates, with
/// its low byte as the wire `seq` — never reissued across reboots (see the
/// note above).
pub struct SeqCounter {
    /// Frames issued so far, ever, on this key. `seq` is its low byte.
    count: u32,
    /// Highest count persisted as authorised. `count` never reaches it
    /// without the store first being moved past it.
    ceiling: u32,
    /// Set once a store write has failed. Sticky: a counter that has lost its
    /// backing is a counter that might repeat, and must not be used again.
    failed: bool,
}

impl SeqCounter {
    /// Numbers authorised per store write: the most a crash can skip, and one
    /// NVS write per this many frames. Chosen when this counter was the 8-bit
    /// `seq` and had to clear a neighbour's 32-entry de-dup ring; that ring is
    /// gone, and 32 stays because SPINE-REPLAY §6 steps 1–3 were run on the
    /// bench at this value (gaps of 31 measured across seven resets) and the
    /// wear budget it implies — one write per 32 frames, under a write every
    /// 2.5 min at the keepalive rate — needs no improving.
    pub const RESERVE: u32 = 32;

    /// Resume from the persisted ceiling and authorise the next block. Fails
    /// if the store cannot be read or written — do not transmit in that case.
    pub fn boot(store: &mut impl CeilingStore) -> Result<Self, StoreError> {
        let hwm = store.read(SEQ_CEILING_KEY)?;
        let ceiling = hwm.checked_add(Self::RESERVE).ok_or(StoreError)?;
        store.write(SEQ_CEILING_KEY, ceiling)?;
        Ok(Self {
            count: hwm,
            ceiling,
            failed: false,
        })
    }

    /// The next `seq` to transmit. `Err` means the counter has no durable
    /// backing (a store write failed, now or earlier, or the count is
    /// exhausted) and the caller must not transmit: fail closed. A station
    /// that goes quiet is a condition the mesh supervisor already reports;
    /// a station that repeats numbers is not.
    pub fn next(&mut self, store: &mut impl CeilingStore) -> Result<u8, StoreError> {
        if self.failed {
            return Err(StoreError);
        }
        let next = self.count.checked_add(1).ok_or(StoreError)?;
        if next >= self.ceiling {
            // Extend BEFORE crossing. Persist-then-use loses numbers on a
            // crash; use-then-persist repeats them. Only one is survivable.
            let new_ceiling = self.ceiling.checked_add(Self::RESERVE).ok_or(StoreError)?;
            if store.write(SEQ_CEILING_KEY, new_ceiling).is_err() {
                self.failed = true;
                return Err(StoreError);
            }
            self.ceiling = new_ceiling;
        }
        self.count = next;
        Ok(next as u8)
    }

    /// Frames issued so far on this key.
    pub fn count(&self) -> u32 {
        self.count
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
        if f.is_ascii_whitespace() {
            s = rest
        } else {
            break;
        }
    }
    while let [rest @ .., l] = s {
        if l.is_ascii_whitespace() {
            s = rest
        } else {
            break;
        }
    }
    s
}

/// Accumulates bytes and yields complete newline-delimited lines.
///
/// Both origins on this board — the USB console and the UART1 compute uplink —
/// framed lines by hand, and both carried the same defect: a line that outgrew
/// the budget had its overflow silently dropped and its *prefix* sent on as
/// if it were the whole message. For JSON that produces a corrupt command which
/// still transmits, still costs airtime, and still has to be parsed and rejected
/// at the far end. Failing loudly and dropping the line is strictly better.
///
/// The budget is [`MAX_AUTH_PAYLOAD`]: every line this framer passes becomes
/// the payload of one authenticated frame.
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
    /// A line ended, but it exceeded [`MAX_AUTH_PAYLOAD`] and was discarded.
    Overflow,
}

impl LineFramer {
    pub const fn new() -> Self {
        Self {
            line: Vec::new(),
            overflowed: false,
            emitted: false,
        }
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
        if self.line.len() < MAX_AUTH_PAYLOAD {
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

    // ── SeqCounter: the properties SPINE-REPLAY.md §2 promises ─────────────

    /// Flash, as a map. `writes` counts durable writes; `fail_after` makes
    /// the Nth write fail, the way a full or worn partition would.
    struct Flash {
        cells: std::collections::BTreeMap<String, u32>,
        writes: usize,
        fail_after: Option<usize>,
    }

    impl Flash {
        fn new() -> Self {
            Self {
                cells: Default::default(),
                writes: 0,
                fail_after: None,
            }
        }
        /// The sender's persisted ceiling.
        fn ceiling(&self) -> u32 {
            self.cells.get(SEQ_CEILING_KEY).copied().unwrap_or(0)
        }
    }

    impl CeilingStore for Flash {
        fn read(&mut self, key: &str) -> Result<u32, StoreError> {
            Ok(self.cells.get(key).copied().unwrap_or(0))
        }
        fn write(&mut self, key: &str, value: u32) -> Result<(), StoreError> {
            if self.fail_after.is_some_and(|n| self.writes >= n) {
                return Err(StoreError);
            }
            self.cells.insert(key.to_string(), value);
            self.writes += 1;
            Ok(())
        }
    }

    // ── AuthFrame: the v2 wire shape (SPINE-AUTH.md §3.2) ──────────────────

    #[test]
    fn an_auth_frame_round_trips_and_carries_its_counter_and_tag() {
        let mut buf = Vec::new();
        let mac = [1, 2, 3, 4, 5, 6, 7, 8];
        AuthFrame {
            src: 0xd8,
            seq: 0x2b,
            ttl: 1,
            ctr: 0x0001_022b,
            payload: b"{\"t\":\"hb\"}",
            mac,
        }
        .encode(&mut buf);
        assert_eq!(buf.len(), HEADER + AUTH_OVERHEAD + 10);
        assert_eq!(&buf[..3], &[0xd8, 0x2b, 1]);
        assert_eq!(&buf[3..7], &[0, 1, 2, 0x2b], "counter is big-endian");
        let f = AuthFrame::decode(&buf).unwrap();
        assert_eq!((f.src, f.seq, f.ttl, f.ctr), (0xd8, 0x2b, 1, 0x0001_022b));
        assert_eq!(f.payload, b"{\"t\":\"hb\"}");
        assert_eq!(f.mac, mac);
    }

    #[test]
    fn a_v1_frame_or_a_runt_does_not_decode_as_v2() {
        // Shorter than header + counter + tag: refused outright. A retired v1
        // frame long enough to pass this check decodes to garbage that then
        // fails the tag — which is the receiver's job, not the decoder's.
        let v1: Vec<u8> = [0x40u8, 1, 0].iter().chain(b"{\"a\":1}").copied().collect();
        assert!(v1.len() < HEADER + AUTH_OVERHEAD);
        assert!(AuthFrame::decode(&v1).is_none());
        assert!(AuthFrame::decode(&[0u8; HEADER + AUTH_OVERHEAD - 1]).is_none());
        let empty = AuthFrame::decode(&[0u8; HEADER + AUTH_OVERHEAD]).unwrap();
        assert!(empty.payload.is_empty());
    }

    #[test]
    fn the_auth_budget_is_the_v1_budget_less_the_overhead() {
        assert_eq!(MAX_AUTH_PAYLOAD, 228);
        assert_eq!(MAC_LEN, 8);
        let mut buf = Vec::new();
        AuthFrame {
            src: 1,
            seq: 1,
            ttl: 0,
            ctr: 1,
            payload: &[b'x'; MAX_AUTH_PAYLOAD],
            mac: [0; MAC_LEN],
        }
        .encode(&mut buf);
        assert_eq!(
            buf.len(),
            HEADER + MAX_PAYLOAD,
            "a full v2 frame is exactly the v1 frame at its budget"
        );
        assert!(buf.len() <= 255, "and fits one SX1262 frame");
    }

    // ── ReplayWindow: RFC 4303 §3.4.3 as SPINE-REPLAY.md §3 specifies ─────

    #[test]
    fn a_fresh_counter_is_accepted_once_and_never_again() {
        let mut w = ReplayWindow::resume(0);
        assert_eq!(w.accept(1).0, Replay::Accept);
        assert_eq!(w.accept(1).0, Replay::Seen, "relay duplicate");
        assert_eq!(w.accept(2).0, Replay::Accept);
        assert_eq!(w.accept(2).0, Replay::Seen);
    }

    #[test]
    fn reordered_frames_inside_the_window_are_accepted_exactly_once() {
        // A two-hop frame arriving after the direct one that followed it.
        let mut w = ReplayWindow::resume(0);
        assert_eq!(w.accept(10).0, Replay::Accept);
        assert_eq!(w.accept(12).0, Replay::Accept);
        assert_eq!(w.accept(11).0, Replay::Accept, "late but unseen");
        assert_eq!(w.accept(11).0, Replay::Seen);
        assert_eq!(w.accept(10).0, Replay::Seen);
    }

    #[test]
    fn anything_older_than_the_window_is_refused_even_if_never_seen() {
        let mut w = ReplayWindow::resume(0);
        assert_eq!(w.accept(100).0, Replay::Accept);
        assert_eq!(w.accept(37).0, Replay::Accept, "63 back: inside");
        assert_eq!(w.accept(36).0, Replay::TooOld, "64 back: outside");
        assert_eq!(w.accept(1).0, Replay::TooOld);
    }

    #[test]
    fn a_large_jump_forgets_the_old_window() {
        let mut w = ReplayWindow::resume(0);
        assert_eq!(w.accept(5).0, Replay::Accept);
        assert_eq!(w.accept(5 + 200).0, Replay::Accept);
        // 5 is now far below the window; the bitmap must not have kept it as
        // a stale bit at some wrapped position.
        assert_eq!(w.accept(5).0, Replay::TooOld);
        assert_eq!(w.accept(5 + 200 - 63).0, Replay::Accept);
    }

    #[test]
    fn the_window_persists_a_ceiling_and_resumes_at_it() {
        // Receiver persistence rounds up: after a restart, everything at or
        // below the persisted ceiling is refused, so a replay of frames the
        // receiver accepted just before it died has nowhere to land.
        let mut flash = Flash::new();
        let key = rx_ceiling_key(0x40);
        let mut w = ReplayWindow::resume(flash.read(&key).unwrap());
        let mut accepted = Vec::new();
        for ctr in 1..=20u32 {
            let (v, need) = w.accept(ctr);
            assert_eq!(v, Replay::Accept);
            if let Some(c) = need {
                flash.write(&key, c).unwrap();
            }
            accepted.push(ctr);
        }
        let persisted = flash.read(&key).unwrap();
        assert!(
            persisted >= 20,
            "ceiling {persisted} is below the true high-water mark"
        );
        assert!(
            persisted <= 20 + ReplayWindow::M,
            "ceiling {persisted} rounds up by more than M"
        );
        // Restart.
        let mut w2 = ReplayWindow::resume(flash.read(&key).unwrap());
        for ctr in accepted {
            assert_ne!(
                w2.accept(ctr).0,
                Replay::Accept,
                "replayed {ctr} after restart"
            );
        }
        // The sender catches up: at most M legitimate frames are lost.
        let first_ok = (21..).find(|&c| w2.accept(c).0 == Replay::Accept).unwrap();
        assert!(
            first_ok - 21 <= ReplayWindow::M,
            "gap {} exceeds M",
            first_ok - 21
        );
    }

    #[test]
    fn the_window_writes_once_per_m_frames_not_once_per_frame() {
        let mut flash = Flash::new();
        let key = rx_ceiling_key(0x40);
        let mut w = ReplayWindow::resume(0);
        for ctr in 1..=(ReplayWindow::M * 10) {
            if let Some(c) = w.accept(ctr).1 {
                flash.write(&key, c).unwrap();
            }
        }
        assert_eq!(flash.writes as u32, 10);
    }

    #[test]
    fn sender_and_receiver_ceilings_live_under_different_keys() {
        // One namespace, two ceilings: they must not clobber each other.
        let mut flash = Flash::new();
        let mut c = SeqCounter::boot(&mut flash).unwrap();
        let mut w = ReplayWindow::resume(0);
        if let Some(x) = w.accept(1000).1 {
            flash.write(&rx_ceiling_key(0x40), x).unwrap();
        }
        c.next(&mut flash).unwrap();
        assert_eq!(flash.ceiling(), SeqCounter::RESERVE);
        assert_eq!(
            flash.read(&rx_ceiling_key(0x40)).unwrap(),
            1000 + ReplayWindow::M
        );
        assert_ne!(rx_ceiling_key(0x40), rx_ceiling_key(0xd8));
        assert!(rx_ceiling_key(0xd8).len() <= 15, "NVS key limit");
    }

    /// Issue `n` seqs and return their full counts.
    fn issue(c: &mut SeqCounter, store: &mut Flash, n: usize) -> Vec<u32> {
        (0..n)
            .map(|_| {
                c.next(store).expect("store healthy");
                c.count()
            })
            .collect()
    }

    #[test]
    fn a_clean_reboot_never_reissues_a_count_and_skips_at_most_the_reserve() {
        let mut flash = Flash::new();
        let mut before = Vec::new();
        for _ in 0..10 {
            let mut c = SeqCounter::boot(&mut flash).unwrap();
            let issued = issue(&mut c, &mut flash, 7); // fewer than RESERVE: no extension
            if let Some(&last) = before.last() {
                let first = issued[0];
                assert!(first > last, "after reboot {first} must exceed {last}");
                assert!(
                    first - last <= SeqCounter::RESERVE + 1,
                    "gap {} exceeds the reserve",
                    first - last
                );
            }
            before.extend(issued);
        }
        let mut sorted = before.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), before.len(), "a count was issued twice");
    }

    #[test]
    fn an_unclean_reboot_mid_block_repeats_nothing() {
        // "Unclean": the counter is dropped without any further write — the
        // ceiling on flash is whatever the last extension left. Numbers used
        // after it are skipped, not reused.
        let mut flash = Flash::new();
        let mut c = SeqCounter::boot(&mut flash).unwrap();
        let used = issue(&mut c, &mut flash, 40); // crosses one extension
        let last = *used.last().unwrap();
        // Power cut: `c` is abandoned with no further write.
        let mut c2 = SeqCounter::boot(&mut flash).unwrap();
        let first = issue(&mut c2, &mut flash, 1)[0];
        assert!(first > last, "{first} must exceed {last}");
        assert!(first - last <= SeqCounter::RESERVE + 1);
    }

    #[test]
    fn the_write_happens_before_the_numbers_are_used() {
        let mut flash = Flash::new();
        let c = SeqCounter::boot(&mut flash).unwrap();
        assert_eq!(flash.writes, 1, "boot authorises before issuing");
        assert!(flash.ceiling() >= c.count() + SeqCounter::RESERVE);
        let mut c = c;
        for _ in 0..(SeqCounter::RESERVE * 3) {
            c.next(&mut flash).unwrap();
            // The persisted ceiling is always strictly above the count in use.
            assert!(
                flash.ceiling() > c.count(),
                "count {} reached ceiling {}",
                c.count(),
                flash.ceiling()
            );
        }
        // One write per RESERVE numbers, plus the boot write.
        assert_eq!(flash.writes as u32, 1 + 3);
    }

    #[test]
    fn a_failed_write_stops_the_counter_for_good() {
        let mut flash = Flash::new();
        flash.fail_after = Some(1); // the boot write succeeds, the first extension fails
        let mut c = SeqCounter::boot(&mut flash).unwrap();
        for _ in 0..(SeqCounter::RESERVE - 1) {
            c.next(&mut flash).unwrap();
        }
        assert_eq!(c.next(&mut flash), Err(StoreError), "extension refused");
        flash.fail_after = None; // even after the store recovers…
        assert_eq!(
            c.next(&mut flash),
            Err(StoreError),
            "…the counter stays closed"
        );
        // A fresh boot from the intact ceiling is fine, and repeats nothing:
        // it resumes at the ceiling the failed extension never moved.
        let mut c2 = SeqCounter::boot(&mut flash).unwrap();
        c2.next(&mut flash).unwrap();
        assert!(c2.count() > c.count());
    }

    #[test]
    fn a_rebooted_station_is_fresh_to_its_neighbours_window() {
        // What the whole thing is for: a neighbour that has accepted this
        // station's frames up to some counter must accept every frame it
        // sends after a reboot — no counter is ever `Seen` or `TooOld`,
        // however many frames went before and however many reboots.
        for pre in [0usize, 5, 20, 40, 250, 256, 300, 510] {
            let mut flash = Flash::new();
            let mut c = SeqCounter::boot(&mut flash).unwrap();
            let mut w = ReplayWindow::resume(0);
            for _ in 0..pre {
                c.next(&mut flash).unwrap();
                assert_eq!(w.accept(c.count()).0, Replay::Accept);
            }
            for reboot in 0..3 {
                let mut c2 = SeqCounter::boot(&mut flash).unwrap();
                for i in 0..(SeqCounter::RESERVE * 2) {
                    c2.next(&mut flash).unwrap();
                    assert_eq!(
                        w.accept(c2.count()).0,
                        Replay::Accept,
                        "after {pre} frames and reboot {reboot}, frame {i} (ctr {}) was refused",
                        c2.count()
                    );
                }
            }
        }
    }

    #[test]
    fn the_wire_seq_is_the_low_byte_of_the_counter() {
        // The header's `seq` is what the log lines and the host's parser read;
        // it must be the counter the tag covers, not a second number.
        let mut flash = Flash::new();
        let mut c = SeqCounter::boot(&mut flash).unwrap();
        for _ in 0..300 {
            let s = c.next(&mut flash).unwrap();
            assert_eq!(s, c.count() as u8);
        }
    }

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
        assert_eq!(
            lines[0],
            b"{\"cmd\":\"capabilities\",\"to\":\"obc-esp32-s3-001\"}"
        );
        assert_eq!(lines[1], b"{\"cmd\":\"capabilities\",\"to\":\"gw-40\"}");
    }

    #[test]
    fn an_oversized_line_is_discarded_whole_not_truncated() {
        // The important half: no prefix escapes. A clipped JSON command is not a
        // shorter command, it is a corrupt one.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_AUTH_PAYLOAD + 50];
        burst.push(b'\n');
        let (lines, overflows) = run(&mut f, &burst);
        assert!(lines.is_empty(), "a truncated prefix must never be emitted");
        assert_eq!(overflows, 1);
    }

    #[test]
    fn the_framer_recovers_after_an_overflow() {
        // An oversized line must not poison the next one.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_AUTH_PAYLOAD + 1];
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
        // Off-by-one guard: MAX_AUTH_PAYLOAD bytes fit, +1 does not — and a
        // line at the budget encodes to a frame at exactly the radio budget.
        let mut f = LineFramer::new();
        let mut burst = vec![b'x'; MAX_AUTH_PAYLOAD];
        burst.push(b'\n');
        let (lines, overflows) = run(&mut f, &burst);
        assert_eq!(overflows, 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_AUTH_PAYLOAD);
        assert_eq!(MAX_AUTH_PAYLOAD + AUTH_OVERHEAD, MAX_PAYLOAD);
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
            assert!(
                !is_spine_payload(junk),
                "would have transmitted: {:?}",
                junk
            );
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
}

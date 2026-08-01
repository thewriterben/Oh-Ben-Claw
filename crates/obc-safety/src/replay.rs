//! Anti-replay: the half of `SPINE-REPLAY.md` that does not need a board.
//!
//! A tag proves a frame came from a holder of the node key. It says nothing
//! about *when*: an attacker who records one authenticated `gpio_write` can
//! replay it forever, and every replay verifies perfectly, because the frame is
//! genuine. It is just old. The counter inside the MAC is what makes a frame
//! old, and this is the receiver's half of that.
//!
//! ## Why a window rather than a high-water mark
//!
//! `SPINE-AUTH.md` §3.2 originally said *"the receiver rejects any counter at or
//! below the highest seen for that source"*. Strictly implemented that is wrong
//! for this mesh, and the evidence was already in the firmware: `spine.rs`
//! carries a de-duplication ring because **flood relay delivers the same frame
//! more than once, by design**. Re-ordering is normal too — a frame that takes
//! two hops arrives after one that went direct. Strict monotonicity drops all of
//! that as an attack, and the mesh appears to be under constant assault by
//! itself.
//!
//! So: IPsec's anti-replay window (RFC 4303 §3.4.3), unchanged. Accept anything
//! newer than the highest seen; accept anything inside the window that has not
//! been seen; refuse everything else.
//!
//! ## What is deliberately absent
//!
//! **Persistence.** `SPINE-REPLAY.md` §3 works out that a receiver must persist
//! a *ceiling* (`H + M`) rather than its position, because a receiver that
//! resumes below its true high-water mark accepts replays of everything in
//! between — the opposite rounding from the sender, and the classic form of this
//! bug. That belongs with the transport that needs it, and on the host the
//! answer is cheap (SQLite, `M = 1`) while on the node it is a flash question
//! that cannot be settled without hardware. This type stays in memory and says
//! so; a restart collapses the window, which fails closed.
//!
//! **The first frame.** A receiver that has never heard from a source has no
//! high-water mark and accepts what it hears, starting the window there. That is
//! a one-frame replay opportunity for a capture made before the receiver
//! existed, it is bounded, and the honest alternative is a provisioning
//! handshake rather than a cleverer window.

use std::collections::HashMap;

/// How many counters below the highest accepted one stay individually tracked.
///
/// 64 fits a `u64` bitmap exactly and absorbs the duplicate-and-reorder traffic
/// flood relay produces. Larger tolerates more re-ordering and widens the span
/// in which a replay of an unseen counter is still accepted.
pub const WINDOW: u32 = 64;

/// Why a frame was refused. Separate variants because they mean different
/// things to an operator: one is a duplicate the mesh produced, the other is a
/// frame old enough that this receiver can no longer tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayVerdict {
    /// Not seen before, and inside or ahead of the window.
    Fresh,
    /// Already accepted — a relay duplicate, or a replay.
    Duplicate,
    /// Older than the window; unjudgeable, so refused.
    TooOld,
}

/// Per-source replay state: the highest counter accepted, and a bitmap of the
/// [`WINDOW`] counters below it.
#[derive(Debug, Clone)]
struct Source {
    highest: u32,
    /// Bit *n* set means `highest - n - 1` has been accepted.
    seen: u64,
}

/// Anti-replay state for every source this receiver has heard from.
///
/// In memory only — see the module header.
#[derive(Debug, Clone, Default)]
pub struct ReplayWindow {
    sources: HashMap<String, Source>,
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Judge `ctr` from `source`, recording it when fresh.
    ///
    /// Takes `&mut self` on purpose: judging a counter *is* a state change, and
    /// an API that let a caller check without recording would let two frames
    /// with the same counter both pass a check and both be accepted.
    pub fn admit(&mut self, source: &str, ctr: u32) -> ReplayVerdict {
        match self.sources.get_mut(source) {
            None => {
                // First frame from this source. See the module header for why
                // this is accepted and what it costs.
                self.sources.insert(
                    source.to_string(),
                    Source {
                        highest: ctr,
                        seen: 0,
                    },
                );
                ReplayVerdict::Fresh
            }
            Some(state) => {
                if ctr > state.highest {
                    let advance = ctr - state.highest;
                    // Shift the old window along and record the old highest,
                    // which has been accepted, at its new offset.
                    //
                    // The boundary is `>`, not `>=`, and that took a test to get
                    // right. After advancing by exactly WINDOW the old highest
                    // sits at the far edge of the new window — still in range,
                    // still judgeable — so clearing the bitmap there would let it
                    // be accepted a second time. A replay of exactly one frame,
                    // reachable by an attacker who can make the counter jump 64,
                    // which on a lossy radio link happens by itself.
                    state.seen = if advance > WINDOW {
                        0
                    } else if advance == WINDOW {
                        // `seen << 64` is a shift overflow in Rust (and would be
                        // zero anyway); only the old highest survives.
                        1u64 << (WINDOW - 1)
                    } else {
                        (state.seen << advance) | (1u64 << (advance - 1))
                    };
                    state.highest = ctr;
                    ReplayVerdict::Fresh
                } else if ctr == state.highest {
                    ReplayVerdict::Duplicate
                } else {
                    let back = state.highest - ctr;
                    if back > WINDOW {
                        return ReplayVerdict::TooOld;
                    }
                    let bit = 1u64 << (back - 1);
                    if state.seen & bit != 0 {
                        ReplayVerdict::Duplicate
                    } else {
                        state.seen |= bit;
                        ReplayVerdict::Fresh
                    }
                }
            }
        }
    }

    /// The highest counter accepted from `source`, if any has been.
    pub fn highest(&self, source: &str) -> Option<u32> {
        self.sources.get(source).map(|s| s.highest)
    }

    /// Forget a source — used when a node is re-provisioned with a fresh key,
    /// which resets its counter legitimately.
    pub fn forget(&mut self, source: &str) {
        self.sources.remove(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = "obc-esp32-s3-001";

    #[test]
    fn a_rising_counter_is_always_fresh() {
        let mut w = ReplayWindow::new();
        for ctr in 1..=200 {
            assert_eq!(w.admit(N, ctr), ReplayVerdict::Fresh, "at {ctr}");
        }
        assert_eq!(w.highest(N), Some(200));
    }

    #[test]
    fn the_same_counter_twice_is_a_duplicate() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 10), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 10), ReplayVerdict::Duplicate);
    }

    /// The reason this is a window and not a high-water mark: flood relay
    /// re-orders, and a two-hop copy arriving after a direct one is ordinary
    /// traffic rather than an attack.
    #[test]
    fn a_reordered_frame_inside_the_window_is_accepted_once() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 100), ReplayVerdict::Fresh);
        assert_eq!(
            w.admit(N, 97),
            ReplayVerdict::Fresh,
            "arrived late, not old"
        );
        assert_eq!(
            w.admit(N, 97),
            ReplayVerdict::Duplicate,
            "the relay's second copy of the same late frame"
        );
        assert_eq!(w.highest(N), Some(100), "a late frame must not rewind H");
    }

    #[test]
    fn a_frame_older_than_the_window_is_refused() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 1000), ReplayVerdict::Fresh);
        assert_eq!(
            w.admit(N, 1000 - WINDOW),
            ReplayVerdict::Fresh,
            "at the edge"
        );
        assert_eq!(w.admit(N, 1000 - WINDOW - 1), ReplayVerdict::TooOld);
        assert_eq!(w.admit(N, 0), ReplayVerdict::TooOld);
    }

    /// The bug this shape invites: a jump forward must not leave stale bits
    /// behind, or a counter inside the *new* window is judged by whether an
    /// unrelated counter under the *old* one was seen.
    #[test]
    fn a_large_jump_clears_the_bitmap() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 10), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 11), ReplayVerdict::Fresh);
        // Jump well past the window: nothing below is still in range.
        assert_eq!(w.admit(N, 10_000), ReplayVerdict::Fresh);
        assert_eq!(
            w.admit(N, 10_000 - 1),
            ReplayVerdict::Fresh,
            "a counter under the new window must be judged on its own"
        );
        assert_eq!(w.admit(N, 10_000 - 1), ReplayVerdict::Duplicate);
    }

    /// A jump of exactly the window width is where the shift arithmetic is most
    /// likely to be wrong, and where the first version of this file *was*
    /// wrong: it cleared the bitmap at `advance >= WINDOW`, which forgot that
    /// the old highest had been accepted while it was still inside the new
    /// window — so replaying it a second time was accepted.
    ///
    /// A one-frame replay, reachable by anyone who can make the counter jump
    /// 64, which on a lossy radio link happens without help.
    #[test]
    fn the_old_highest_survives_a_jump_of_exactly_the_window_width() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 100), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 100 + WINDOW), ReplayVerdict::Fresh);
        assert_eq!(
            w.admit(N, 100),
            ReplayVerdict::Duplicate,
            "100 is still inside the window and was already accepted"
        );
        // One further out is beyond judgement, which is the safe answer.
        assert_eq!(w.admit(N, 99), ReplayVerdict::TooOld);
    }

    /// The complementary case: a jump of one *past* the window genuinely does
    /// put the old highest out of range, and clearing is then correct.
    #[test]
    fn a_jump_past_the_window_leaves_the_old_highest_unjudgeable() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 100), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 100 + WINDOW + 1), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 100), ReplayVerdict::TooOld);
    }

    #[test]
    fn sources_are_independent() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit("a", 50), ReplayVerdict::Fresh);
        assert_eq!(
            w.admit("b", 50),
            ReplayVerdict::Fresh,
            "one node's counter must not judge another's"
        );
        assert_eq!(w.admit("a", 50), ReplayVerdict::Duplicate);
    }

    /// Re-provisioning gives a node a fresh key and a counter that starts over.
    /// Without a way to forget, that node could never be heard from again.
    #[test]
    fn forgetting_a_source_lets_its_counter_restart() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, 5_000), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, 1), ReplayVerdict::TooOld);
        w.forget(N);
        assert_eq!(w.admit(N, 1), ReplayVerdict::Fresh);
    }

    /// Counters near `u32::MAX` must not panic on the arithmetic. Exhaustion is
    /// the sender's problem (`SPINE-REPLAY.md` §2 refuses to wrap); the receiver
    /// only has to not fall over.
    #[test]
    fn the_top_of_the_counter_space_does_not_overflow() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.admit(N, u32::MAX - 1), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, u32::MAX), ReplayVerdict::Fresh);
        assert_eq!(w.admit(N, u32::MAX), ReplayVerdict::Duplicate);
    }
}

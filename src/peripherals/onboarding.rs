//! Device-onboarding policy — a vendor allowlist for auto-trust (Track 0).
//!
//! When a USB device shows up, the brain shouldn't auto-onboard it as an
//! orchestrated node just because it enumerated. This is a small but real Track 0
//! hardening: only **known vendor IDs** are auto-trusted; an unrecognized VID is
//! **quarantined** — held back until an operator explicitly approves it. The
//! default allowlist is exactly the vendors already in the hardware registry, so
//! every board OBC knows about is auto-trusted and anything else is not.
//!
//! Adapted from a sibling project's `auto_discovery.filters.vendor_ids`; the
//! implementation here is original.

use super::registry::known_boards;
use std::collections::HashSet;

/// What to do with a freshly-discovered device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardDecision {
    /// Vendor is known — onboard automatically.
    AutoTrust,
    /// Vendor is unknown — hold for explicit operator approval.
    Quarantine,
}

/// An allowlist of USB vendor IDs permitted to auto-onboard.
#[derive(Debug, Clone, Default)]
pub struct VendorAllowlist {
    allowed: HashSet<u16>,
    allow_unknown: bool,
}

impl VendorAllowlist {
    /// Build an allowlist from an explicit set of vendor IDs.
    pub fn new(vids: impl IntoIterator<Item = u16>) -> Self {
        Self { allowed: vids.into_iter().collect(), allow_unknown: false }
    }

    /// The default policy: trust exactly the vendors present in the hardware
    /// registry (every board OBC knows). Unknown vendors are quarantined.
    pub fn from_known_boards() -> Self {
        Self::new(known_boards().iter().map(|b| b.vid))
    }

    /// Add a vendor ID to the allowlist (builder style).
    pub fn allow(mut self, vid: u16) -> Self {
        self.allowed.insert(vid);
        self
    }

    /// Permit *any* vendor (disables the gate — for trusted lab benches).
    pub fn allow_unknown(mut self, yes: bool) -> Self {
        self.allow_unknown = yes;
        self
    }

    /// Whether a vendor ID is permitted to auto-onboard.
    pub fn is_allowed(&self, vid: u16) -> bool {
        self.allow_unknown || self.allowed.contains(&vid)
    }

    /// The onboarding decision for a discovered device's vendor ID.
    pub fn decide(&self, vid: u16) -> OnboardDecision {
        if self.is_allowed(vid) {
            OnboardDecision::AutoTrust
        } else {
            OnboardDecision::Quarantine
        }
    }

    /// Number of explicitly-allowed vendors.
    pub fn len(&self) -> usize {
        self.allowed.len()
    }
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_registry_vendors_auto_trust() {
        let policy = VendorAllowlist::from_known_boards();
        // Espressif (0x303a) and STMicro (0x0483) are in the registry.
        assert_eq!(policy.decide(0x303a), OnboardDecision::AutoTrust);
        assert_eq!(policy.decide(0x0483), OnboardDecision::AutoTrust);
    }

    #[test]
    fn an_unknown_vendor_is_quarantined() {
        let policy = VendorAllowlist::from_known_boards();
        assert_eq!(policy.decide(0xdead), OnboardDecision::Quarantine);
    }

    #[test]
    fn explicit_allow_adds_a_vendor() {
        let policy = VendorAllowlist::new([0x1111u16]).allow(0x2222);
        assert!(policy.is_allowed(0x1111));
        assert_eq!(policy.decide(0x2222), OnboardDecision::AutoTrust);
        assert_eq!(policy.decide(0x3333), OnboardDecision::Quarantine);
    }

    #[test]
    fn allow_unknown_disables_the_gate() {
        let policy = VendorAllowlist::default().allow_unknown(true);
        assert_eq!(policy.decide(0xbeef), OnboardDecision::AutoTrust);
    }
}

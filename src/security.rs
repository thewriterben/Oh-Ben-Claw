//! Track 0 — re-exported from the [`obc_safety`] crate.
//!
//! The implementation moved to `crates/obc-safety` on 2026-07-30: the pin
//! allowlist and value ranges enforced at the actuator, the hash-chained and
//! signed action audit, the policy engine, taint tracking, node pairing and the
//! vault. It was extracted because it had exactly one outward dependency, and
//! that dependency was pointing the wrong way — see `obc_safety::risk`.
//!
//! Everything is re-exported, so `crate::security::limits::SafetyGate` and
//! friends resolve exactly as before. No call site changed.

pub use obc_safety::*;

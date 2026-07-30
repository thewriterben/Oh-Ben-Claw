//! Shim: the host crate's deployment planner, compiled verbatim.
//!
//! A real directory rather than an inline `mod` block — see
//! [`super::peripherals`] for why that distinction decides whether this crate
//! builds on Linux at all.

#[path = "../../../src/deployment/advisor.rs"]
pub mod advisor;
#[path = "../../../src/deployment/firmware_scaffold.rs"]
pub mod firmware_scaffold;
#[path = "../../../src/deployment/inventory.rs"]
pub mod inventory;
#[path = "../../../src/deployment/planner.rs"]
pub mod planner;
#[path = "../../../src/deployment/scheme.rs"]
pub mod scheme;

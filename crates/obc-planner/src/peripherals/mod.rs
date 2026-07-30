//! The board and accessory registry.
//!
//! Only `registry` lives here. The agent's other `peripherals` modules — the
//! transports, the bus tools, onboarding, the self-test — do real I/O and stay in
//! the host crate. The registry is a table plus lookups, and it is the input to
//! everything in [`crate::deployment`].
//!
//! It is also the single source of truth for two other repositories: the host
//! crate's `emit-registry` binary serialises these tables to `registry.json`,
//! which OBC-deployment-generator and Accelerapp both consume, and OBC-Prime
//! hashes. Adding a board here is a cross-repository event.

pub mod registry;

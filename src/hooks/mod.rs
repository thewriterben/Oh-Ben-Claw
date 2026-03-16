//! Event lifecycle hooks for Oh-Ben-Claw.
//!
//! Hooks let you intercept and react to agent lifecycle events such as session
//! start/end, message reception, tool invocation, and agent responses.
//!
//! # Quick Start
//!
//! 1. Implement [`HookHandler`] for your type.
//! 2. Register it with a [`HookRunner`].
//! 3. Call the `fire_*` methods at the appropriate points in your code.

pub mod runner;
pub mod traits;

pub use runner::HookRunner;
pub use traits::{HookHandler, HookResult};

//! Oh-Ben-Claw communication channels.
//!
//! Channels are the interfaces through which users interact with the agent.
//! Each channel adapter handles the specifics of a particular platform and
//! translates its messages into the common agent `process()` call.

pub mod cli;

pub use cli::CliChannel;

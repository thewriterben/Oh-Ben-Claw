//! Oh-Ben-Claw communication channels.
//!
//! Channels are the interfaces through which users interact with the agent.
//! Each channel adapter handles the specifics of a particular platform and
//! translates its messages into the common agent `process()` call.

pub mod cli;
pub mod discord;
pub mod imessage;
pub mod matrix;
pub mod slack;
pub mod telegram;
pub mod whatsapp;
mod utils;

pub use cli::CliChannel;
pub use discord::DiscordChannel;
pub use imessage::IMessageChannel;
pub use matrix::MatrixChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use whatsapp::WhatsAppChannel;

//! Oh-Ben-Claw communication channels.
//!
//! Channels are the interfaces through which users interact with the agent.
//! Each channel adapter handles the specifics of a particular platform and
//! translates its messages into the common agent `process()` call.
//!
//! ## New in Phase 10 (OpenClaw Parity)
//!
//! * **IRC** — raw-TCP IRC adapter with SASL PLAIN auth and channel support.
//! * **Signal** — Signal Messenger via the signal-cli JSON-RPC HTTP daemon.
//! * **Mattermost** — Mattermost WebSocket event API.
//! * **Typing indicators** — Telegram, Discord, and Slack now show a "typing…"
//!   status while the agent is processing, improving perceived responsiveness.
//!
//! ## New in Phase 11 (Pycoclaw/Mimiclaw Parity)
//!
//! * **Feishu/Lark** — Feishu enterprise messaging via webhook event subscription,
//!   inspired by [MimiClaw](https://github.com/memovai/mimiclaw).

pub mod cli;
pub mod discord;
pub mod feishu;
pub mod imessage;
pub mod irc;
pub mod matrix;
pub mod mattermost;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod typing;
mod utils;
pub mod whatsapp;

pub use cli::CliChannel;
pub use discord::DiscordChannel;
pub use feishu::FeishuChannel;
pub use imessage::IMessageChannel;
pub use irc::IrcChannel;
pub use matrix::MatrixChannel;
pub use mattermost::MattermostChannel;
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use typing::TypingTask;
pub use whatsapp::WhatsAppChannel;

//! Oh-Ben-Claw communication channels.
//!
//! Channels are the interfaces through which users interact with the agent.
//! Each channel adapter handles the specifics of a particular platform
//! (e.g., Telegram, Discord, CLI) and translates its messages into the
//! common `ChannelMessage` format.
//!
//! # Supported Channels
//!
//! | Channel   | Status      | Notes                                    |
//! |-----------|-------------|------------------------------------------|
//! | CLI       | Planned     | Interactive terminal interface           |
//! | Telegram  | Planned     | Bot API integration                      |
//! | Discord   | Planned     | Bot API + WebSocket gateway              |
//! | Slack     | Planned     | Events API integration                   |
//! | WhatsApp  | Planned     | Business API integration                 |
//! | iMessage  | Planned     | macOS only, via AppleScript              |
//! | Matrix    | Planned     | Matrix.org federation                    |
//! | IRC       | Planned     | Classic IRC protocol                     |
//! | GUI       | Planned     | Native desktop application               |

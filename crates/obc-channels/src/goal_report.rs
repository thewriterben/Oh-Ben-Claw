//! Goal reports to the chat a goal was set from (personal-agents item 4,
//! 2026-11-02).
//!
//! A standing goal's owner is the session it was set in: `telegram:<chat>` or
//! `slack:<channel>` (`obc_agent::goals::owner_for_session`). The report goes
//! back there as plain text, with the bot token the channel already uses. An
//! owner this reporter cannot reach is an error, and the tick then falls back
//! to the notifier's channels.

use crate::{slack, telegram};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use obc_agent::goals::GoalReporter;
use obc_config::{SlackConfig, TelegramConfig};
use serde_json::json;

pub struct OwnerReporter {
    http: reqwest::Client,
    /// `https://api.telegram.org/bot<token>` when Telegram has a token.
    telegram_api: Option<String>,
    slack_token: Option<String>,
}

impl OwnerReporter {
    /// `None` when neither channel has a token: every report then goes
    /// through the notifier.
    pub fn from_config(telegram: &TelegramConfig, slack: &SlackConfig) -> Option<Self> {
        let telegram_api = telegram
            .token
            .clone()
            .or_else(|| std::env::var("TELEGRAM_BOT_TOKEN").ok())
            .filter(|t| !t.is_empty())
            .map(|t| format!("https://api.telegram.org/bot{t}"));
        let slack_token = slack
            .bot_token
            .clone()
            .or_else(|| std::env::var("SLACK_BOT_TOKEN").ok())
            .filter(|t| !t.is_empty());
        (telegram_api.is_some() || slack_token.is_some()).then(|| Self {
            http: reqwest::Client::new(),
            telegram_api,
            slack_token,
        })
    }
}

#[async_trait]
impl GoalReporter for OwnerReporter {
    async fn report(&self, owner: &str, text: &str) -> Result<()> {
        match owner.split_once(':') {
            Some(("telegram", chat)) => {
                let Some(api) = &self.telegram_api else {
                    bail!("no Telegram token");
                };
                let chat_id: i64 = chat.parse().context("Telegram chat id")?;
                telegram::send_plain(&self.http, api, chat_id, text).await
            }
            Some(("slack", channel)) => {
                let Some(token) = &self.slack_token else {
                    bail!("no Slack token");
                };
                slack::slack_call(
                    &self.http,
                    token,
                    "chat.postMessage",
                    &json!({ "channel": channel, "text": text }),
                )
                .await
            }
            _ => bail!("no way to reach owner {owner:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_owner_on_a_channel_without_a_token_is_an_error_not_a_silent_drop() {
        let r = OwnerReporter {
            http: reqwest::Client::new(),
            telegram_api: None,
            slack_token: Some("xoxb-test".into()),
        };
        assert!(r.report("telegram:42", "x").await.is_err());
        assert!(r.report("discord:1", "x").await.is_err());
        assert!(r.report("", "x").await.is_err());
    }
}

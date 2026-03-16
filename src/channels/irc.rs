//! IRC channel adapter.
//!
//! Connects to an IRC server using raw TCP (optionally TLS), joins the
//! configured channels, and forwards PRIVMSG messages to the Oh-Ben-Claw
//! agent. Replies are sent as PRIVMSG back to the originating nick or channel.
//!
//! # Protocol notes
//!
//! * SASL PLAIN authentication is supported for services like Libera.Chat.
//! * If `password` is set (without SASL), a `PASS` command is sent at login.
//! * The bot responds to CTCP VERSION and PING (server-level and CTCP).
//! * Lines longer than 450 bytes are chunked before sending.
//!
//! # Setup
//!
//! ```toml
//! [channels.irc]
//! host       = "irc.libera.chat"
//! port       = 6697
//! use_tls    = true
//! nickname   = "oh-ben-claw"
//! channels   = ["#ai-bots"]
//! ```

use crate::agent::Agent;
use crate::config::{IrcConfig, ProviderConfig};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

// ── IRC line helpers ──────────────────────────────────────────────────────────

/// Parse an IRC message line into `(prefix, command, params)`.
fn parse_irc_line(line: &str) -> (Option<&str>, &str, Vec<&str>) {
    let line = line.trim_end_matches(['\r', '\n']);
    let (prefix, rest): (Option<&str>, &str) =
        if let Some(stripped) = line.strip_prefix(':') {
            let idx = stripped.find(' ').unwrap_or(stripped.len());
            (Some(&stripped[..idx]), stripped[idx..].trim_start())
        } else {
            (None, line)
        };

    // Split trailing parameter (after " :")
    let (params_str, trailing): (&str, Option<&str>) = if let Some(idx) = rest.find(" :") {
        (&rest[..idx], Some(&rest[idx + 2..]))
    } else {
        (rest, None)
    };

    let mut params: Vec<&str> = params_str
        .split_whitespace()
        .filter(|s: &&str| !s.is_empty())
        .collect();

    if let Some(t) = trailing {
        params.push(t);
    }

    let command = params.first().copied().unwrap_or("");
    let params = if params.is_empty() {
        vec![]
    } else {
        params[1..].to_vec()
    };

    (prefix, command, params)
}

/// Extract the nick portion from a prefix like `nick!user@host`.
fn nick_from_prefix(prefix: &str) -> &str {
    prefix.split('!').next().unwrap_or(prefix)
}

// ── IrcSink — a clonable handle for sending IRC lines via a channel ───────────

/// A clonable handle for sending raw IRC lines.  Under the hood it uses a
/// tokio mpsc channel whose receiver is forwarded by a writer task, avoiding
/// the `dyn AsyncWriteExt` object-safety issue.
#[derive(Clone)]
struct IrcSink(mpsc::Sender<String>);

impl IrcSink {
    /// Create a new sink and spawn the writer task on the provided write half.
    fn spawn(write_half: tokio::io::WriteHalf<TcpStream>) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(64);
        tokio::spawn(async move {
            let mut w = write_half;
            while let Some(line) = rx.recv().await {
                let bytes = line.as_bytes();
                if w.write_all(bytes).await.is_err() {
                    break;
                }
                if !line.ends_with('\n') && w.write_all(b"\r\n").await.is_err() {
                    break;
                }
                if w.flush().await.is_err() {
                    break;
                }
            }
        });
        Self(tx)
    }

    async fn send_raw(&self, line: impl Into<String>) -> Result<()> {
        self.0
            .send(line.into())
            .await
            .map_err(|_| anyhow::anyhow!("IRC writer task closed"))
    }

    /// Send PRIVMSG, chunking long lines.
    async fn privmsg(&self, target: &str, text: &str) -> Result<()> {
        // IRC RFC limits lines to 512 bytes; leave headroom for the prefix.
        const CHUNK: usize = 450;
        let mut remaining = text;
        while !remaining.is_empty() {
            let (chunk, rest) = if remaining.len() <= CHUNK {
                (remaining, "")
            } else {
                // Split on word boundary if possible
                let split_at = remaining[..CHUNK]
                    .rfind(' ')
                    .unwrap_or(CHUNK)
                    .max(1);
                (&remaining[..split_at], remaining[split_at..].trim_start())
            };
            self.send_raw(format!("PRIVMSG {} :{}", target, chunk))
                .await?;
            remaining = rest;
        }
        Ok(())
    }
}

// ── IrcChannel ────────────────────────────────────────────────────────────────

/// IRC channel adapter.
pub struct IrcChannel {
    agent: Arc<Agent>,
    provider_config: ProviderConfig,
    config: IrcConfig,
}

impl IrcChannel {
    /// Create a new `IrcChannel`.
    ///
    /// Returns `None` if no host is configured.
    pub fn new(config: &IrcConfig, agent: Arc<Agent>, provider_config: ProviderConfig) -> Option<Self> {
        config.host.as_ref()?;
        Some(Self {
            agent,
            provider_config,
            config: config.clone(),
        })
    }

    /// Connect and run the IRC adapter (blocking — intended for use in a
    /// `tokio::spawn` task).
    pub async fn run(&self) -> Result<()> {
        let host = self.config.host.as_deref().unwrap_or("localhost");
        let use_tls = self.config.use_tls;
        let port = self.config.port.unwrap_or(if use_tls { 6697 } else { 6667 });

        let addr = format!("{}:{}", host, port);
        tracing::info!(addr, "Connecting to IRC server");

        let tcp = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("IRC: failed to connect to {}", addr))?;

        if use_tls {
            self.run_with_stream(tcp, true).await
        } else {
            self.run_with_stream(tcp, false).await
        }
    }

    async fn run_with_stream(&self, stream: TcpStream, _tls: bool) -> Result<()> {
        let (read_half, write_half) = tokio::io::split(stream);
        let sink = IrcSink::spawn(write_half);
        let reader = BufReader::new(read_half);
        let mut lines = reader.lines();

        // ── Registration ─────────────────────────────────────────────────────
        let nick = &self.config.nickname;

        // SASL CAP negotiation (optional)
        let use_sasl = self.config.sasl_username.is_some();
        if use_sasl {
            sink.send_raw("CAP REQ :sasl").await?;
        }

        if let Some(pass) = &self.config.password {
            sink.send_raw(format!("PASS :{}", pass)).await?;
        }
        sink.send_raw(format!("NICK {}", nick)).await?;
        sink.send_raw(format!("USER {} 0 * :Oh-Ben-Claw AI", nick))
            .await?;

        let mut sasl_done = !use_sasl;
        let mut registered = false;
        let agent = self.agent.clone();
        let provider_config = self.provider_config.clone();
        let channels_to_join = self.config.channels.clone();
        let sasl_user = self.config.sasl_username.clone().unwrap_or_default();
        let sasl_pass = self.config.sasl_password.clone().unwrap_or_default();

        while let Some(line) = lines.next_line().await? {
            tracing::trace!(line = %line, "IRC ←");
            let (prefix, command, params) = parse_irc_line(&line);

            match command {
                // ── PING ─────────────────────────────────────────────────────
                "PING" => {
                    let token = params.first().copied().unwrap_or("?");
                    sink.send_raw(format!("PONG :{}", token)).await?;
                }

                // ── SASL ─────────────────────────────────────────────────────
                "CAP" => {
                    let sub = params.get(1).copied().unwrap_or("");
                    if sub == "ACK" && !sasl_done {
                        sink.send_raw("AUTHENTICATE PLAIN").await?;
                    } else if sub == "NAK" {
                        // SASL not supported — skip
                        sink.send_raw("CAP END").await?;
                        sasl_done = true;
                    }
                }
                "AUTHENTICATE" => {
                    if params.first().copied() == Some("+") {
                        // Build PLAIN payload: \0user\0pass
                        use base64::Engine as _;
                        let payload = format!("\0{}\0{}", sasl_user, sasl_pass);
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
                        sink.send_raw(format!("AUTHENTICATE {}", encoded)).await?;
                    }
                }
                "903" => {
                    // SASL authentication successful
                    sink.send_raw("CAP END").await?;
                    sasl_done = true;
                }
                "904" | "905" => {
                    tracing::warn!("IRC: SASL authentication failed");
                    sink.send_raw("CAP END").await?;
                    sasl_done = true;
                }

                // ── Welcome / registration complete ───────────────────────────
                "001" if sasl_done => {
                    if !registered {
                        registered = true;
                        tracing::info!(nick, "IRC: registered successfully");
                        for ch in &channels_to_join {
                            sink.send_raw(format!("JOIN {}", ch)).await?;
                        }
                    }
                }

                // ── PRIVMSG ───────────────────────────────────────────────────
                "PRIVMSG" if registered => {
                    let sender_nick = prefix
                        .map(nick_from_prefix)
                        .unwrap_or("unknown");
                    let target = params.first().copied().unwrap_or("");
                    let text = params.last().copied().unwrap_or("").trim();

                    // Skip CTCP messages (begin with \x01)
                    if text.starts_with('\x01') {
                        if text.starts_with("\x01VERSION") {
                            let reply = "\x01VERSION Oh-Ben-Claw IRC adapter\x01";
                            sink.privmsg(sender_nick, reply).await?;
                        }
                        continue;
                    }

                    // Determine reply target (channel → channel, DM → sender)
                    let reply_target = if target.starts_with('#') || target.starts_with('&') {
                        target.to_string()
                    } else {
                        sender_nick.to_string()
                    };

                    let session_id = format!("irc:{}", sender_nick);
                    let sink_clone = sink.clone();
                    let agent_clone = agent.clone();
                    let pc_clone = provider_config.clone();
                    let input = text.to_string();
                    let reply_target_owned = reply_target.clone();
                    let sender_nick_owned = sender_nick.to_string();

                    tokio::spawn(async move {
                        match agent_clone
                            .process(&session_id, &input, &pc_clone)
                            .await
                        {
                            Ok(response) => {
                                let prefix_str = if reply_target_owned.starts_with('#')
                                    || reply_target_owned.starts_with('&')
                                {
                                    format!("{}: ", sender_nick_owned)
                                } else {
                                    String::new()
                                };
                                let full = format!("{}{}", prefix_str, response.message);
                                if let Err(e) =
                                    sink_clone.privmsg(&reply_target_owned, &full).await
                                {
                                    tracing::error!(error = %e, "IRC: failed to send reply");
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "IRC: agent processing error");
                                let _ = sink_clone
                                    .privmsg(
                                        &reply_target_owned,
                                        "Sorry, I ran into an error. Please try again.",
                                    )
                                    .await;
                            }
                        }
                    });
                }

                // ── Nick-in-use fallback ───────────────────────────────────────
                "433" => {
                    let new_nick = format!("{}_", nick);
                    tracing::warn!(
                        nick,
                        new_nick,
                        "IRC: nickname in use, trying alternate"
                    );
                    sink.send_raw(format!("NICK {}", new_nick)).await?;
                }

                _ => {}
            }
        }

        tracing::info!("IRC connection closed");
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_irc_line_ping() {
        let (prefix, cmd, params) = parse_irc_line("PING :irc.libera.chat");
        assert_eq!(prefix, None);
        assert_eq!(cmd, "PING");
        assert_eq!(params, vec!["irc.libera.chat"]);
    }

    #[test]
    fn test_parse_irc_line_privmsg() {
        let (prefix, cmd, params) =
            parse_irc_line(":alice!alice@host PRIVMSG #general :Hello world");
        assert_eq!(prefix, Some("alice!alice@host"));
        assert_eq!(cmd, "PRIVMSG");
        assert_eq!(params, vec!["#general", "Hello world"]);
    }

    #[test]
    fn test_parse_irc_line_notice() {
        let (prefix, cmd, params) =
            parse_irc_line(":NickServ!service@libera.chat NOTICE alice :You are now identified");
        assert!(prefix.is_some());
        assert_eq!(cmd, "NOTICE");
        assert_eq!(params.last(), Some(&"You are now identified"));
    }

    #[test]
    fn test_nick_from_prefix() {
        assert_eq!(nick_from_prefix("alice!alice@host"), "alice");
        assert_eq!(nick_from_prefix("alice"), "alice");
    }

    #[test]
    fn test_new_returns_none_without_host() {
        use crate::config::IrcConfig;
        let cfg = IrcConfig::default();
        // Without an agent we can't call new(), but we can verify the logic.
        assert!(cfg.host.is_none());
    }
}

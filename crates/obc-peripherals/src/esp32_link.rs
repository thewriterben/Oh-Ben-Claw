//! A held-open USB-serial link to an `obc-esp32-s3` node, speaking its line
//! protocol.
//!
//! The firmware reads one JSON request per line, `{"id","cmd","args"}`, and
//! answers with one JSON line, `{"id","ok","result","refused"?,"error"?}`
//! (`firmware/obc-esp32-s3/src/main.rs`, `Response`). Three things make a
//! correct reader more than "write a line, read a line":
//!
//! - **The node talks on its own.** Announcements, beacons, reflex and safing
//!   reports share the same port. A reader has to skip every line that is not
//!   the answer it is waiting for.
//! - **A reply can be dropped, and says so.** A reply that does not fit the
//!   node's USB ring within its stall budget is cut off, followed by
//!   `{"type":"reply_dropped","sent","total","reason"}`. That notice carries no
//!   `id`. With one request outstanding at a time, which this link enforces by
//!   taking `&mut self`, it can only be about the request in flight. Reading it
//!   as "no reply" is the bug that cost three bench sessions (2026-09-16).
//! - **Opening the port resets the board.** On the ESP32-S3's USB-Serial-JTAG,
//!   opening the port reboots the node. `scripts/vision/capture_set.py` sleeps
//!   3.5 s and drains input before its first request. So this link is opened
//!   once and kept, unlike `arduino.rs`, which opens the port per command and
//!   would reboot a camera node on every frame.
//!
//! The protocol half ([`ReplyMatcher`], [`request_line`], [`decode_jpeg_result`])
//! does no I/O. [`Esp32Link`] runs it over any async byte stream, so tests can
//! drive it with an in-memory node and the `hardware` feature can hand it a real
//! serial port ([`Esp32Link::open_serial`]).

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// How long `capture_set.py` lets the node reboot after the port opens.
pub const OPEN_SETTLE: Duration = Duration::from_millis(3_500);
/// Its per-request wait. A QVGA JPEG is ~7 KB of base64 at 115200 baud
/// nominal; USB-Serial-JTAG is faster, and this is the measured-working value.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// What a node said to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeReply {
    /// `ok: true`, with the `result` string.
    Ok(String),
    /// `ok: false, refused: true`: Track 0 on the node declined, by policy.
    /// The node is working correctly.
    Refused(String),
    /// `ok: false` for any other reason.
    Failed(String),
    /// The node produced an answer and could not send all of it.
    Dropped {
        sent: u64,
        total: u64,
        reason: String,
    },
}

/// A request the node never answered in time. Typed, because a caller that
/// watches a node treats it as a failed attempt and keeps going, where a lost
/// link (an I/O error, the port closed) ends the watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTimeout {
    pub cmd: String,
    pub id: String,
    pub after: Duration,
}

impl std::fmt::Display for ReplyTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no reply to {} ({}) within {:?}",
            self.cmd, self.id, self.after
        )
    }
}

impl std::error::Error for ReplyTimeout {}

/// The request line for `cmd`, newline included.
pub fn request_line(id: &str, cmd: &str, args: Value) -> String {
    format!("{}\n", json!({ "id": id, "cmd": cmd, "args": args }))
}

/// Picks the answer to one request out of everything else on the line.
pub struct ReplyMatcher {
    id: String,
}

impl ReplyMatcher {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    /// Feed one received line. `Some` when it settles the request.
    pub fn feed(&self, line: &str) -> Option<NodeReply> {
        let line = line.trim();
        // A dropped reply leaves a partial line before the notice; it may fuse
        // with nothing parseable, so look for the notice by content too.
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return None,
        };
        if v.get("type").and_then(Value::as_str) == Some("reply_dropped") {
            return Some(NodeReply::Dropped {
                sent: v.get("sent").and_then(Value::as_u64).unwrap_or(0),
                total: v.get("total").and_then(Value::as_u64).unwrap_or(0),
                reason: v
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("reply dropped")
                    .to_string(),
            });
        }
        if v.get("id").and_then(Value::as_str) != Some(self.id.as_str()) {
            // Someone else's reply, or a report the node sent on its own.
            return None;
        }
        let ok = v.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Some(if ok {
            NodeReply::Ok(text("result"))
        } else if v.get("refused").and_then(Value::as_bool) == Some(true) {
            NodeReply::Refused(text("error"))
        } else {
            NodeReply::Failed(text("error"))
        })
    }
}

/// A `camera_capture` result: base64 that must decode to a JPEG.
pub fn decode_jpeg_result(result: &str) -> Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(result.trim())
        .context("camera_capture result is not base64")?;
    if bytes.len() < 4 || bytes[..2] != [0xFF, 0xD8] {
        bail!(
            "camera_capture result is not a JPEG ({} bytes, starts {:02X?})",
            bytes.len(),
            &bytes[..bytes.len().min(2)]
        );
    }
    Ok(bytes)
}

/// One node, one open stream, one request at a time.
pub struct Esp32Link<S> {
    io: BufReader<S>,
    next_id: u64,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Esp32Link<S> {
    /// Wrap a stream that is already past the node's open-reset (see
    /// [`Esp32Link::open_serial`] for a real port).
    pub fn new(stream: S) -> Self {
        Self {
            io: BufReader::new(stream),
            next_id: 0,
        }
    }

    /// Send `cmd` and wait up to `timeout` for its answer, skipping every
    /// other line. A timeout is an error. A node that answers is never one.
    pub async fn request(
        &mut self,
        cmd: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<NodeReply> {
        let id = format!("h{}", self.next_id);
        self.next_id += 1;
        let line = request_line(&id, cmd, args);
        let stream = self.io.get_mut();
        stream.write_all(line.as_bytes()).await?;
        stream.flush().await?;

        let matcher = ReplyMatcher::new(&id);
        let read = async {
            // Bytes, not `read_line`: one non-UTF-8 byte (boot noise after a
            // brownout reset) would be an InvalidData error, and the caller
            // would take it for a lost link instead of a line to skip.
            let mut buf = Vec::new();
            loop {
                buf.clear();
                if self.io.read_until(b'\n', &mut buf).await? == 0 {
                    bail!("node closed the link while {cmd} ({id}) was outstanding");
                }
                if let Some(reply) = matcher.feed(&String::from_utf8_lossy(&buf)) {
                    return Ok(reply);
                }
            }
        };
        match tokio::time::timeout(timeout, read).await {
            Ok(r) => r,
            Err(_) => Err(ReplyTimeout {
                cmd: cmd.to_string(),
                id,
                after: timeout,
            }
            .into()),
        }
    }

    /// `camera_capture`, returning the JPEG, or why there is none.
    ///
    /// The outer `Result` is the link (I/O, timeout). The inner one is the
    /// node's answer: `Err` carries a reason fit to record as a capture
    /// failure.
    pub async fn capture_jpeg(
        &mut self,
        timeout: Duration,
    ) -> Result<std::result::Result<Vec<u8>, String>> {
        Ok(
            match self.request("camera_capture", json!({}), timeout).await? {
                NodeReply::Ok(result) => decode_jpeg_result(&result).map_err(|e| format!("{e:#}")),
                NodeReply::Refused(e) => Err(format!("refused: {e}")),
                NodeReply::Failed(e) => Err(format!("failed: {e}")),
                NodeReply::Dropped {
                    sent,
                    total,
                    reason,
                } => Err(format!("reply dropped after {sent}/{total} B: {reason}")),
            },
        )
    }
}

#[cfg(feature = "hardware")]
impl Esp32Link<tokio_serial::SerialStream> {
    /// Open `port`, wait out the node's open-reset, and drop whatever it said
    /// while booting. Takes [`OPEN_SETTLE`].
    pub async fn open_serial(port: &str, baud: u32) -> Result<Self> {
        use tokio::io::AsyncReadExt;
        use tokio_serial::SerialPortBuilderExt;
        let mut stream = tokio_serial::new(port, baud)
            .open_native_async()
            .with_context(|| format!("failed to open serial port {port}"))?;
        tokio::time::sleep(OPEN_SETTLE).await;
        // Drain the boot chatter, so the first reply read is not preceded by a
        // half line from before the reset.
        let mut sink = [0u8; 4096];
        while let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(50), stream.read(&mut sink)).await
        {
            if n == 0 {
                break;
            }
        }
        Ok(Self::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn the_matcher_skips_reports_and_other_ids() {
        let m = ReplyMatcher::new("h3");
        assert_eq!(m.feed(r#"{"type":"beacon","node_id":"n"}"#), None);
        assert_eq!(m.feed(r#"{"id":"h2","ok":true,"result":"x"}"#), None);
        assert_eq!(m.feed("garbage {"), None);
        assert_eq!(
            m.feed(r#"{"id":"h3","ok":true,"result":"x"}"#),
            Some(NodeReply::Ok("x".into()))
        );
    }

    #[test]
    fn refusal_failure_and_drop_are_three_different_answers() {
        let m = ReplyMatcher::new("h0");
        assert_eq!(
            m.feed(
                r#"{"id":"h0","ok":false,"result":"","refused":true,"error":"pin 5 not allowed"}"#
            ),
            Some(NodeReply::Refused("pin 5 not allowed".into()))
        );
        assert_eq!(
            m.feed(r#"{"id":"h0","ok":false,"result":"","error":"camera not initialised"}"#),
            Some(NodeReply::Failed("camera not initialised".into()))
        );
        // The exact notice `send_line` writes; it has no id.
        assert_eq!(
            m.feed(r#"{"type":"reply_dropped","sent":4096,"total":6767,"reason":"ring stayed full - the answer exists but did not fit"}"#),
            Some(NodeReply::Dropped {
                sent: 4096,
                total: 6767,
                reason: "ring stayed full - the answer exists but did not fit".into()
            })
        );
    }

    #[test]
    fn a_capture_result_must_be_a_jpeg() {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        assert_eq!(
            decode_jpeg_result(&b64(&[0xFF, 0xD8, 0xFF, 0xE0, 1])).unwrap(),
            vec![0xFF, 0xD8, 0xFF, 0xE0, 1]
        );
        assert!(decode_jpeg_result(&b64(b"PNG...")).is_err());
        assert!(decode_jpeg_result("not base64!").is_err());
    }

    /// A node on the far end of an in-memory pipe: it chatters, answers one
    /// request after a beacon, and, for the second, drops the reply.
    #[tokio::test]
    async fn the_link_finds_its_answer_in_the_chatter_and_reports_a_drop() {
        let (host, node) = duplex(64 * 1024);
        let node_task = tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(node);
            let mut lines = BufReader::new(r).lines();
            // First request.
            let req: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(req["cmd"], "camera_capture");
            let id = req["id"].as_str().unwrap().to_string();
            let jpeg = base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xD9]);
            // Reset noise that is not UTF-8: a line to skip, not a lost link.
            w.write_all(b"\xff\xfe\x00ets boot\n").await.unwrap();
            w.write_all(b"{\"type\":\"beacon\"}\n").await.unwrap();
            w.write_all(
                format!("{{\"id\":\"{id}\",\"ok\":true,\"result\":\"{jpeg}\"}}\n").as_bytes(),
            )
            .await
            .unwrap();
            // Second request: a partial reply, then the notice, as send_line does.
            let _ = lines.next_line().await.unwrap().unwrap();
            w.write_all(b"{\"id\":\"h1\",\"ok\":true,\"result\":\"/9j/4AAQ")
                .await
                .unwrap();
            w.write_all(b"\n{\"type\":\"reply_dropped\",\"sent\":20,\"total\":6767,\"reason\":\"ring stayed full\"}\n")
                .await
                .unwrap();
        });

        let mut link = Esp32Link::new(host);
        let first = link.capture_jpeg(Duration::from_secs(5)).await.unwrap();
        assert_eq!(first, Ok(vec![0xFF, 0xD8, 0xFF, 0xD9]));
        let second = link.capture_jpeg(Duration::from_secs(5)).await.unwrap();
        let why = second.unwrap_err();
        assert!(why.contains("reply dropped after 20/6767"), "{why}");
        node_task.await.unwrap();
    }

    #[tokio::test]
    async fn silence_is_a_timeout_not_a_hang() {
        let (host, _node) = duplex(1024);
        let mut link = Esp32Link::new(host);
        let err = link
            .request("capabilities", json!({}), Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ReplyTimeout>().is_some(), "{err}");
    }
}

//! A Wyoming-protocol client, for on-device speech (plan P5).
//!
//! Wyoming is the Rhasspy / Home Assistant voice protocol: one JSON header line
//! per event, then `data_length` bytes of JSON data, then `payload_length`
//! bytes of payload. Moonshine and faster-whisper speak it for speech-to-text,
//! Piper and Kokoro for text-to-speech, each as its own small TCP server on the
//! SBC (10300 and 10200 by convention). That is what lets `audio_transcribe`
//! and `text_to_speech` stay on the device and keep the cloud as the fallback.
//!
//! Two exchanges, one connection each:
//!
//! - STT: `transcribe`, `audio-start`, `audio-chunk`..., `audio-stop`; the
//!   server answers `transcript {text}`.
//! - TTS: `synthesize {text, voice}`; the server answers `audio-start`,
//!   `audio-chunk`..., `audio-stop`.
//!
//! Either side may send `error {text}` instead. Events this client does not
//! know are skipped, which is how the protocol expects to grow.
//!
//! The `*_over` functions take any byte stream, so the exchanges are tested
//! against a fake server on an in-memory pipe; [`WyomingClient`] is the same
//! thing over TCP with one deadline around the whole exchange.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::TcpStream;

use crate::wav::Pcm;

/// The protocol version this client writes in its headers.
const VERSION: &str = "1.5.4";
/// Longest header line accepted, newline included.
pub const MAX_HEADER: u64 = 64 * 1024;
/// Largest `data` section accepted.
pub const MAX_DATA: u64 = 1024 * 1024;
/// Largest single payload accepted (one audio chunk is a few KiB).
pub const MAX_PAYLOAD: u64 = 8 * 1024 * 1024;
/// Most synthesized audio accepted in one answer: about 20 minutes of 16-bit
/// mono at 24 kHz, which is far past any sentence.
pub const MAX_AUDIO: usize = 64 * 1024 * 1024;
/// Samples per `audio-chunk` sent to an STT server.
const CHUNK_SAMPLES: usize = 1024;

/// One Wyoming event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The header's `type`.
    pub kind: String,
    pub data: Map<String, Value>,
    pub payload: Vec<u8>,
}

impl Event {
    /// An event with `data` (anything but a JSON object is taken as empty).
    pub fn new(kind: impl Into<String>, data: Value) -> Self {
        let data = match data {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        Self {
            kind: kind.into(),
            data,
            payload: Vec::new(),
        }
    }

    pub fn with_payload(mut self, payload: Vec<u8>) -> Self {
        self.payload = payload;
        self
    }

    /// The bytes on the wire: header line, data, payload.
    pub fn encode(&self) -> Vec<u8> {
        let data = if self.data.is_empty() {
            Vec::new()
        } else {
            serde_json::to_vec(&self.data).unwrap_or_default()
        };
        let mut header = json!({ "type": self.kind, "version": VERSION });
        if !data.is_empty() {
            header["data_length"] = data.len().into();
        }
        if !self.payload.is_empty() {
            header["payload_length"] = self.payload.len().into();
        }
        let mut out = serde_json::to_vec(&header).unwrap_or_default();
        out.push(b'\n');
        out.extend_from_slice(&data);
        out.extend_from_slice(&self.payload);
        out
    }
}

/// A length field from a header: absent or null is 0, anything over `max` is
/// refused before a byte of it is read.
fn length(header: &Value, key: &str, max: u64) -> Result<usize> {
    match header.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| anyhow!("Wyoming {key} is not a length: {v}"))?;
            if n > max {
                bail!("Wyoming {key} of {n} bytes is over the {max}-byte limit");
            }
            Ok(n as usize)
        }
    }
}

/// Read one event. `Ok(None)` when the peer closed the connection cleanly
/// between events.
///
/// Older peers put `data` inline in the header; newer ones send it after. Both
/// are read, and the two are merged.
pub async fn read_event<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Event>> {
    let mut line = Vec::new();
    let n = (&mut *reader)
        .take(MAX_HEADER + 1)
        .read_until(b'\n', &mut line)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        if line.len() as u64 > MAX_HEADER {
            bail!("Wyoming header longer than {MAX_HEADER} bytes");
        }
        bail!("Wyoming peer closed the connection mid-header");
    }
    let header: Value = serde_json::from_slice(&line).context("Wyoming header is not JSON")?;
    let kind = header
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Wyoming header has no type"))?
        .to_string();
    let mut data = match header.get("data") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    let data_len = length(&header, "data_length", MAX_DATA)?;
    if data_len > 0 {
        let mut buf = vec![0; data_len];
        reader.read_exact(&mut buf).await?;
        let more: Map<String, Value> =
            serde_json::from_slice(&buf).context("Wyoming event data is not a JSON object")?;
        data.extend(more);
    }
    let payload_len = length(&header, "payload_length", MAX_PAYLOAD)?;
    let mut payload = vec![0; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await?;
    }
    Ok(Some(Event {
        kind,
        data,
        payload,
    }))
}

/// Write one event. The caller flushes when the exchange is sent.
pub async fn write_event<W: AsyncWrite + Unpin>(writer: &mut W, event: &Event) -> Result<()> {
    writer.write_all(&event.encode()).await?;
    Ok(())
}

/// Normalise a configured endpoint to `host:port`. Takes `tcp://host:port` or
/// a bare `host:port`; Wyoming also runs over Unix sockets and stdio, which
/// this client does not.
pub fn parse_endpoint(endpoint: &str) -> Result<String> {
    let trimmed = endpoint.trim();
    let rest = match trimmed.split_once("://") {
        Some(("tcp", rest)) => rest,
        Some((scheme, _)) => {
            bail!("Wyoming endpoint {endpoint:?}: only tcp:// is supported, not {scheme}://")
        }
        None => trimmed,
    };
    let rest = rest.trim_end_matches('/');
    let Some((host, port)) = rest.rsplit_once(':') else {
        bail!("Wyoming endpoint {endpoint:?} has no port (expected host:port)");
    };
    if host.is_empty() || host.contains('/') {
        bail!("Wyoming endpoint {endpoint:?} has no usable host");
    }
    let port: u16 = port
        .parse()
        .map_err(|_| anyhow!("Wyoming endpoint {endpoint:?}: {port:?} is not a port"))?;
    if port == 0 {
        bail!("Wyoming endpoint {endpoint:?}: port 0 is not a server");
    }
    Ok(format!("{host}:{port}"))
}

fn error_text(event: &Event) -> &str {
    event
        .data
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
}

/// `(rate, width, channels)` from an `audio-start` or `audio-chunk`.
fn audio_format(event: &Event) -> Result<(u32, u16, u16)> {
    let get = |key: &str| event.data.get(key).and_then(Value::as_u64);
    let rate = get("rate").and_then(|v| u32::try_from(v).ok());
    let width = get("width").and_then(|v| u16::try_from(v).ok());
    let channels = get("channels").and_then(|v| u16::try_from(v).ok());
    match (rate, width, channels) {
        (Some(rate), Some(width), Some(channels))
            if rate > 0 && (1..=4).contains(&width) && channels > 0 =>
        {
            Ok((rate, width, channels))
        }
        _ => bail!("Wyoming {} has no usable rate/width/channels", event.kind),
    }
}

/// Run the STT exchange over `stream` and return the transcript, trimmed.
pub async fn transcribe_over<S>(stream: S, audio: &Pcm, language: Option<&str>) -> Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let ask = match language {
        Some(lang) => json!({ "language": lang }),
        None => json!({}),
    };
    write_event(&mut write, &Event::new("transcribe", ask)).await?;
    let format = json!({ "rate": audio.rate, "width": audio.width, "channels": audio.channels });
    write_event(&mut write, &Event::new("audio-start", format.clone())).await?;
    let chunk = CHUNK_SAMPLES * audio.frame_bytes().max(1);
    for piece in audio.data.chunks(chunk) {
        let event = Event::new("audio-chunk", format.clone()).with_payload(piece.to_vec());
        write_event(&mut write, &event).await?;
    }
    write_event(&mut write, &Event::new("audio-stop", json!({}))).await?;
    write.flush().await?;
    loop {
        let Some(event) = read_event(&mut read).await? else {
            bail!("Wyoming STT server closed the connection without a transcript");
        };
        match event.kind.as_str() {
            "transcript" => {
                let text = event.data.get("text").and_then(Value::as_str);
                return Ok(text.unwrap_or_default().trim().to_string());
            }
            "error" => bail!("Wyoming STT server: {}", error_text(&event)),
            _ => {}
        }
    }
}

/// Run the TTS exchange over `stream` and return the audio it synthesized.
pub async fn synthesize_over<S>(stream: S, text: &str, voice: Option<&str>) -> Result<Pcm>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut ask = json!({ "text": text });
    if let Some(name) = voice {
        ask["voice"] = json!({ "name": name });
    }
    write_event(&mut write, &Event::new("synthesize", ask)).await?;
    write.flush().await?;
    let mut format = None;
    let mut data = Vec::new();
    loop {
        let Some(event) = read_event(&mut read).await? else {
            bail!("Wyoming TTS server closed the connection before audio-stop");
        };
        match event.kind.as_str() {
            "audio-start" => format = Some(audio_format(&event)?),
            "audio-chunk" => {
                if format.is_none() {
                    format = Some(audio_format(&event)?);
                }
                if data.len() + event.payload.len() > MAX_AUDIO {
                    bail!("Wyoming TTS answer is over {MAX_AUDIO} bytes of audio");
                }
                data.extend_from_slice(&event.payload);
            }
            "audio-stop" => break,
            "error" => bail!("Wyoming TTS server: {}", error_text(&event)),
            _ => {}
        }
    }
    let Some((rate, width, channels)) = format else {
        bail!("Wyoming TTS server sent audio-stop without saying what the audio was");
    };
    Ok(Pcm {
        rate,
        width,
        channels,
        data,
    })
}

/// A Wyoming server at a TCP address, with a deadline on each exchange.
#[derive(Debug, Clone)]
pub struct WyomingClient {
    addr: String,
    timeout: Duration,
}

impl WyomingClient {
    /// `endpoint` as [`parse_endpoint`] takes it. Nothing is connected until
    /// the first exchange.
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self> {
        Ok(Self {
            addr: parse_endpoint(endpoint)?,
            timeout,
        })
    }

    /// `host:port`.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    async fn connect(&self, what: &str) -> Result<TcpStream> {
        TcpStream::connect(&self.addr)
            .await
            .with_context(|| format!("connecting to the Wyoming {what} server at {}", self.addr))
    }

    fn late(&self, what: &str) -> anyhow::Error {
        anyhow!(
            "Wyoming {what} server at {} did not answer within {} ms",
            self.addr,
            self.timeout.as_millis()
        )
    }

    /// Speech to text.
    pub async fn transcribe(&self, audio: &Pcm, language: Option<&str>) -> Result<String> {
        let work = async {
            let stream = self.connect("STT").await?;
            transcribe_over(stream, audio, language).await
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| self.late("STT"))?
    }

    /// Text to speech.
    pub async fn synthesize(&self, text: &str, voice: Option<&str>) -> Result<Pcm> {
        let work = async {
            let stream = self.connect("TTS").await?;
            synthesize_over(stream, text, voice).await
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| self.late("TTS"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

    type Fake = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);

    fn fake(server: DuplexStream) -> Fake {
        let (read, write) = tokio::io::split(server);
        (BufReader::new(read), write)
    }

    async fn reply(write: &mut WriteHalf<DuplexStream>, events: &[Event]) {
        for event in events {
            write_event(write, event).await.unwrap();
        }
        write.flush().await.unwrap();
    }

    fn mono16(samples: usize) -> Pcm {
        Pcm {
            rate: 16_000,
            width: 2,
            channels: 1,
            data: vec![1; samples * 2],
        }
    }

    #[test]
    fn encode_states_both_lengths() {
        let bytes = Event::new("audio-chunk", json!({ "rate": 16000 }))
            .with_payload(vec![9; 4])
            .encode();
        let newline = bytes.iter().position(|b| *b == b'\n').unwrap();
        let header: Value = serde_json::from_slice(&bytes[..newline]).unwrap();
        assert_eq!(header["type"], "audio-chunk");
        assert_eq!(header["data_length"], 14); // {"rate":16000}
        assert_eq!(header["payload_length"], 4);
        assert_eq!(&bytes[bytes.len() - 4..], &[9; 4]);
    }

    #[tokio::test]
    async fn read_event_round_trips_and_merges_inline_data() {
        let first = Event::new("transcript", json!({ "text": "hi" }));
        let mut wire = first.encode();
        // An older peer: data inline in the header, nothing after it.
        wire.extend_from_slice(b"{\"type\":\"error\",\"data\":{\"text\":\"old\"}}\n");
        let mut reader = BufReader::new(&wire[..]);
        assert_eq!(read_event(&mut reader).await.unwrap(), Some(first));
        let old = read_event(&mut reader).await.unwrap().unwrap();
        assert_eq!(old.kind, "error");
        assert_eq!(error_text(&old), "old");
        assert_eq!(read_event(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_event_refuses_oversized_lengths_and_cut_headers() {
        let big = format!(
            "{{\"type\":\"audio-chunk\",\"payload_length\":{}}}\n",
            MAX_PAYLOAD + 1
        );
        let err = read_event(&mut BufReader::new(big.as_bytes()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
        let cut = b"{\"type\":\"transcr";
        assert!(read_event(&mut BufReader::new(&cut[..])).await.is_err());
        let long = vec![b' '; MAX_HEADER as usize + 10];
        let err = read_event(&mut BufReader::new(&long[..]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("longer"), "{err}");
    }

    #[test]
    fn parse_endpoint_takes_tcp_and_bare_and_refuses_the_rest() {
        assert_eq!(
            parse_endpoint("tcp://127.0.0.1:10300").unwrap(),
            "127.0.0.1:10300"
        );
        assert_eq!(
            parse_endpoint(" localhost:10200/ ").unwrap(),
            "localhost:10200"
        );
        assert_eq!(parse_endpoint("tcp://[::1]:10300").unwrap(), "[::1]:10300");
        for bad in [
            "unix:///run/wyoming.sock",
            "http://127.0.0.1:10300",
            "127.0.0.1",
            ":10300",
            "tcp://host:0",
            "tcp://host:http",
            "tcp://host/path:10300",
        ] {
            assert!(parse_endpoint(bad).is_err(), "{bad} was accepted");
        }
    }

    #[tokio::test]
    async fn transcribe_streams_the_audio_and_returns_the_transcript() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (mut read, mut write) = fake(server);
            let mut kinds = Vec::new();
            let mut bytes = 0;
            let mut language = None;
            while let Some(event) = read_event(&mut read).await.unwrap() {
                if event.kind == "transcribe" {
                    language = event.data.get("language").cloned();
                }
                if event.kind == "audio-chunk" {
                    assert_eq!(event.data["rate"], 16000);
                }
                bytes += event.payload.len();
                kinds.push(event.kind.clone());
                if event.kind == "audio-stop" {
                    break;
                }
            }
            let answer = [
                Event::new("info", json!({})), // unknown to the client: skipped
                Event::new("transcript", json!({ "text": "  turn the lights on " })),
            ];
            reply(&mut write, &answer).await;
            (kinds, bytes, language)
        });
        let text = transcribe_over(client, &mono16(3000), Some("en"))
            .await
            .unwrap();
        assert_eq!(text, "turn the lights on");
        let (kinds, bytes, language) = server.await.unwrap();
        // 3000 samples in 1024-sample chunks: three chunks.
        let expected = [
            "transcribe",
            "audio-start",
            "audio-chunk",
            "audio-chunk",
            "audio-chunk",
            "audio-stop",
        ];
        assert_eq!(kinds, expected);
        assert_eq!(bytes, 6000);
        assert_eq!(language, Some(json!("en")));
    }

    #[tokio::test]
    async fn synthesize_collects_the_audio_until_stop() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (mut read, mut write) = fake(server);
            let ask = read_event(&mut read).await.unwrap().unwrap();
            let format = json!({ "rate": 22050, "width": 2, "channels": 1 });
            let answer = [
                Event::new("audio-start", format.clone()),
                Event::new("audio-chunk", format.clone()).with_payload(vec![1, 2]),
                Event::new("audio-chunk", format).with_payload(vec![3, 4]),
                Event::new("audio-stop", json!({})),
            ];
            reply(&mut write, &answer).await;
            ask
        });
        let pcm = synthesize_over(client, "hello", Some("en_US-lessac-medium"))
            .await
            .unwrap();
        assert_eq!((pcm.rate, pcm.width, pcm.channels), (22050, 2, 1));
        assert_eq!(pcm.data, vec![1, 2, 3, 4]);
        let ask = server.await.unwrap();
        assert_eq!(ask.kind, "synthesize");
        assert_eq!(ask.data["text"], "hello");
        assert_eq!(ask.data["voice"]["name"], "en_US-lessac-medium");
    }

    #[tokio::test]
    async fn a_server_error_or_a_missing_format_is_an_error() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let (mut read, mut write) = fake(server);
            let _ = read_event(&mut read).await;
            let answer = [Event::new("error", json!({ "text": "voice not loaded" }))];
            reply(&mut write, &answer).await;
        });
        let err = synthesize_over(client, "hello", None).await.unwrap_err();
        assert!(err.to_string().contains("voice not loaded"), "{err}");

        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let (mut read, mut write) = fake(server);
            let _ = read_event(&mut read).await;
            reply(&mut write, &[Event::new("audio-stop", json!({}))]).await;
        });
        assert!(synthesize_over(client, "hello", None).await.is_err());

        let (client, server) = tokio::io::duplex(64 * 1024);
        drop(server); // closed before answering
        assert!(transcribe_over(client, &mono16(10), None).await.is_err());
    }

    #[tokio::test]
    async fn the_tcp_client_answers_and_gives_up_at_its_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // First connection: a real answer. Second: accept and say nothing.
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(socket);
            let mut read = BufReader::new(read);
            while let Some(event) = read_event(&mut read).await.unwrap() {
                if event.kind == "audio-stop" {
                    break;
                }
            }
            let answer = Event::new("transcript", json!({ "text": "over tcp" }));
            write_event(&mut write, &answer).await.unwrap();
            write.flush().await.unwrap();
            let (_silent, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = WyomingClient::new(&format!("tcp://{addr}"), Duration::from_secs(5)).unwrap();
        assert_eq!(
            client.transcribe(&mono16(100), None).await.unwrap(),
            "over tcp"
        );
        let impatient = WyomingClient::new(&addr.to_string(), Duration::from_millis(200)).unwrap();
        let err = impatient.transcribe(&mono16(100), None).await.unwrap_err();
        assert!(err.to_string().contains("did not answer"), "{err}");
    }
}

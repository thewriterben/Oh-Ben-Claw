//! The edge host's on-device models: what is installed, and pulling one with
//! its digest checked (Phase 20 plan P4).
//!
//! `[[edge.models]]` lists them, as the deployment planner wrote them. A
//! source is one of two kinds:
//!
//! - `ollama://<tag>`, pulled through the local Ollama server. Ollama names a
//!   model by the sha256 of its manifest, and the manifest names every layer by
//!   its own sha256, which Ollama checks as it downloads. Pinning the manifest
//!   digest in `digest` therefore pins every byte. Unpinned, a pull reports the
//!   digest it got so the operator can pin it: trust on first use, said out
//!   loud rather than done silently.
//! - `https://…`, a GGUF file for a llama.cpp server, downloaded into a
//!   directory and hashed as it arrives. A URL is not content-addressed, so
//!   `digest` is required.
//!
//! A pull whose digest does not match is not kept. The Ollama tag is deleted
//! and the partial file removed, so the reflex tier cannot load a model that
//! nobody verified.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The Ollama server the reflex tier talks to (`EdgeConfig::reflex_provider`).
pub const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// How long a pull may take, download included.
pub const PULL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// One `[[edge.models]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeModelEntry {
    /// The model's name: the Ollama tag, or the name the llama.cpp server
    /// knows the file by. `[edge].local_model` refers to it.
    pub name: String,
    /// Download size in MB. Informational; the planner's RAM check used it.
    #[serde(default)]
    pub size_mb: Option<u32>,
    /// Estimated resident RAM in MB. Informational.
    #[serde(default)]
    pub ram_mb: Option<u32>,
    /// Quantisation, e.g. `Q4_K_M`. Informational.
    #[serde(default)]
    pub quantisation: Option<String>,
    /// `ollama://<tag>` or `https://<url of a .gguf>`.
    pub source: String,
    /// `sha256:<64 hex>`: for Ollama the manifest digest it lists the model
    /// under, for a file the file's own hash. Required for `https://`.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Where an entry comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// An Ollama tag.
    Ollama(String),
    /// A file URL.
    Https(String),
}

impl EdgeModelEntry {
    /// The parsed `source`.
    pub fn parsed_source(&self) -> Result<ModelSource> {
        let src = self.source.trim();
        if let Some(tag) = src.strip_prefix("ollama://") {
            if tag.trim().is_empty() {
                bail!("source {src:?} names no Ollama tag");
            }
            Ok(ModelSource::Ollama(tag.trim().to_string()))
        } else if src.starts_with("https://") {
            Ok(ModelSource::Https(src.to_string()))
        } else {
            bail!("source {src:?}: expected ollama://<tag> or https://<url of a .gguf file>")
        }
    }

    /// The pinned digest as 64 lowercase hex characters, if one is set.
    pub fn pinned_digest(&self) -> Result<Option<String>> {
        self.digest.as_deref().map(parse_digest).transpose()
    }
}

/// `sha256:<hex>` or bare hex, to 64 lowercase hex characters.
pub fn parse_digest(s: &str) -> Result<String> {
    let s = s.trim();
    let hex = s.strip_prefix("sha256:").unwrap_or(s).to_ascii_lowercase();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hex)
    } else {
        bail!("digest {s:?} is not sha256:<64 hex characters>")
    }
}

/// Check the entries as the config loader does. Errors only: a bad entry is
/// one a pull would refuse anyway, and refusing at load says so sooner.
pub fn validate(entries: &[EdgeModelEntry]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for e in entries {
        let name = e.name.trim();
        if name.is_empty() {
            bail!("edge.models has an entry with an empty name");
        }
        if !seen.insert(name) {
            bail!("edge.models lists {name:?} twice");
        }
        let source = e
            .parsed_source()
            .with_context(|| format!("edge.models {name:?}"))?;
        let pinned = e
            .pinned_digest()
            .with_context(|| format!("edge.models {name:?}"))?;
        if matches!(source, ModelSource::Https(_)) && pinned.is_none() {
            bail!(
                "edge.models {name:?}: an https source needs digest = \"sha256:...\", \
                 because a URL can serve different bytes tomorrow"
            );
        }
        if let ModelSource::Https(url) = &source {
            file_name(url).with_context(|| format!("edge.models {name:?}"))?;
        }
    }
    Ok(())
}

/// What is installed for one entry, against what the config pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// Not installed.
    Missing,
    /// Installed; the config pins no digest.
    Unpinned { digest: String },
    /// Installed with the pinned digest.
    Verified { digest: String },
    /// Installed with a different digest from the pinned one.
    Mismatch { pinned: String, found: String },
}

impl ModelState {
    fn of(pinned: Option<String>, found: Option<String>) -> Self {
        match (pinned, found) {
            (_, None) => Self::Missing,
            (None, Some(digest)) => Self::Unpinned { digest },
            (Some(p), Some(f)) if p == f => Self::Verified { digest: f },
            (Some(pinned), Some(found)) => Self::Mismatch { pinned, found },
        }
    }
}

impl std::fmt::Display for ModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "missing"),
            Self::Unpinned { digest } => {
                write!(f, "installed, unpinned (digest = \"sha256:{digest}\")")
            }
            Self::Verified { .. } => write!(f, "verified"),
            Self::Mismatch { pinned, found } => write!(
                f,
                "DIGEST MISMATCH: the config pins sha256:{pinned}, installed is sha256:{found}"
            ),
        }
    }
}

/// One entry's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    /// The entry's name.
    pub name: String,
    /// What is installed.
    pub state: ModelState,
}

/// The state of every entry. Asks Ollama once, and only when an entry needs
/// it; hashes each downloaded file that is present.
pub async fn status(
    entries: &[EdgeModelEntry],
    ollama_url: &str,
    gguf_dir: &Path,
) -> Result<Vec<ModelStatus>> {
    let client = client(Duration::from_secs(10))?;
    let mut tags: Option<Vec<Tag>> = None;
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let pinned = e.pinned_digest()?;
        let found = match e.parsed_source()? {
            ModelSource::Ollama(tag) => {
                if tags.is_none() {
                    tags = Some(list_tags(&client, ollama_url).await?);
                }
                find_tag(tags.as_deref().unwrap_or_default(), &tag)?
            }
            ModelSource::Https(url) => {
                let path = gguf_dir.join(file_name(&url)?);
                if path.exists() {
                    Some(sha256_file(&path).await?)
                } else {
                    None
                }
            }
        };
        out.push(ModelStatus {
            name: e.name.clone(),
            state: ModelState::of(pinned, found),
        });
    }
    Ok(out)
}

/// What a pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullReport {
    /// The entry's name.
    pub name: String,
    /// The digest now installed, 64 hex characters.
    pub digest: String,
    /// False when the pinned model was already installed and nothing was fetched.
    pub fetched: bool,
    /// Whether the digest was checked against a pinned one.
    pub verified: bool,
}

impl std::fmt::Display for PullReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.fetched, self.verified) {
            (false, _) => write!(f, "{}: already installed and verified", self.name),
            (true, true) => write!(f, "{}: pulled and verified", self.name),
            (true, false) => write!(
                f,
                "{}: pulled, not verified because no digest is pinned. To pin what \
                 you got, add to its [[edge.models]] entry: digest = \"sha256:{}\"",
                self.name, self.digest
            ),
        }
    }
}

/// Pull one entry and check its digest. A mismatch is removed, not kept.
pub async fn pull(
    entry: &EdgeModelEntry,
    ollama_url: &str,
    gguf_dir: &Path,
    timeout: Duration,
) -> Result<PullReport> {
    let client = client(timeout)?;
    let pinned = entry.pinned_digest()?;
    let report = |digest: String, fetched: bool| PullReport {
        name: entry.name.clone(),
        digest,
        fetched,
        verified: pinned.is_some(),
    };

    match entry.parsed_source()? {
        ModelSource::Ollama(tag) => {
            let before = find_tag(&list_tags(&client, ollama_url).await?, &tag)?;
            if let (Some(p), Some(b)) = (&pinned, &before) {
                if p == b {
                    return Ok(report(b.clone(), false));
                }
            }
            let resp = client
                .post(format!("{ollama_url}/api/pull"))
                .json(&json!({ "model": tag, "stream": false }))
                .send()
                .await
                .with_context(|| format!("Ollama at {ollama_url} did not answer"))?;
            let code = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(err) = body.get("error").and_then(|v| v.as_str()) {
                bail!("Ollama refused to pull {tag}: {err}");
            }
            if !code.is_success() {
                bail!("Ollama refused to pull {tag}: HTTP {code}");
            }
            let found = find_tag(&list_tags(&client, ollama_url).await?, &tag)?
                .ok_or_else(|| anyhow!("Ollama reported pulling {tag} but does not list it"))?;
            if let Some(p) = &pinned {
                if *p != found {
                    let removed = client
                        .delete(format!("{ollama_url}/api/delete"))
                        .json(&json!({ "model": tag }))
                        .send()
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|r| r.error_for_status().map_err(anyhow::Error::from));
                    let after = match removed {
                        Ok(_) => "It was removed rather than left for the reflex tier to load."
                            .to_string(),
                        Err(e) => format!("Removing it failed ({e}); run `ollama rm {tag}`."),
                    };
                    bail!(
                        "{tag}: Ollama fetched sha256:{found}, but the config pins \
                         sha256:{p}. {after} If the publisher re-tagged it on purpose, \
                         check the new digest and update [[edge.models]]."
                    );
                }
            }
            Ok(report(found, true))
        }
        ModelSource::Https(url) => {
            let Some(p) = pinned.clone() else {
                bail!(
                    "{}: an https source needs digest = \"sha256:...\"",
                    entry.name
                );
            };
            let dest = gguf_dir.join(file_name(&url)?);
            if dest.exists() && sha256_file(&dest).await? == p {
                return Ok(report(p, false));
            }
            download_verified(&client, &url, &dest, &p).await?;
            Ok(report(p, true))
        }
    }
}

/// Download `url` to `dest`, hashing as it arrives. The bytes go to
/// `<dest>.part` and are renamed into place only when the hash matches.
async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    pinned: &str,
) -> Result<()> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);

    let fetched = async {
        let mut resp = client.get(url).send().await?.error_for_status()?;
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = resp.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        anyhow::Ok(hex::encode(hasher.finalize()))
    }
    .await;

    match fetched {
        Ok(got) if got == pinned => {
            tokio::fs::rename(&part, dest)
                .await
                .with_context(|| format!("moving the download to {}", dest.display()))?;
            Ok(())
        }
        Ok(got) => {
            let _ = tokio::fs::remove_file(&part).await;
            bail!(
                "{url}: the download hashes to sha256:{got}, but the config pins \
                 sha256:{pinned}. It was deleted."
            )
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e.context(format!("downloading {url}")))
        }
    }
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    digest: String,
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(timeout).build()?)
}

async fn list_tags(client: &reqwest::Client, ollama_url: &str) -> Result<Vec<Tag>> {
    let tags: Tags = client
        .get(format!("{ollama_url}/api/tags"))
        .send()
        .await
        .with_context(|| format!("Ollama at {ollama_url} did not answer"))?
        .error_for_status()?
        .json()
        .await
        .context("Ollama's /api/tags answer was not the expected JSON")?;
    Ok(tags.models)
}

/// The listed digest for `tag`. `x` and `x:latest` are the same tag.
fn find_tag(tags: &[Tag], tag: &str) -> Result<Option<String>> {
    let want = tag.trim_end_matches(":latest");
    tags.iter()
        .find(|t| t.name.trim_end_matches(":latest") == want)
        .map(|t| {
            parse_digest(&t.digest)
                .with_context(|| format!("Ollama lists {} with an unusable digest", t.name))
        })
        .transpose()
}

/// The file name a URL's download is stored under: its last path segment.
fn file_name(url: &str) -> Result<String> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
        bail!("{url} does not end in a file name");
    }
    Ok(name.to_string())
}

async fn sha256_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use std::sync::{Arc, Mutex};

    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn ollama_entry(digest: Option<&str>) -> EdgeModelEntry {
        EdgeModelEntry {
            name: "gemma3:270m".into(),
            size_mb: Some(292),
            ram_mb: Some(621),
            quantisation: Some("Q8_0".into()),
            source: "ollama://gemma3:270m".into(),
            digest: digest.map(|d| format!("sha256:{d}")),
        }
    }

    /// A loopback HTTP server that answers each request from a script keyed by
    /// method and path, one body per call (the last repeats), and records the
    /// requests it saw.
    fn serve(script: Vec<(&'static str, Vec<String>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut calls = std::collections::HashMap::<&str, usize>::new();
            for mut conn in listener.incoming().flatten() {
                // Read the whole request, body included: closing a socket with
                // unread bytes resets it, and the client then never sees the answer.
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = conn.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&data).to_string();
                let line = head.lines().next().unwrap_or_default().to_string();
                let key = line
                    .rsplit_once(' ')
                    .map(|(k, _)| k)
                    .unwrap_or(&line)
                    .to_string();
                log.lock().unwrap().push(key.clone());
                let (status, body) = match script.iter().find(|(k, _)| *k == key) {
                    Some((k, bodies)) => {
                        let i = calls.entry(*k).or_default();
                        let body = bodies[(*i).min(bodies.len() - 1)].clone();
                        *i += 1;
                        ("200 OK", body)
                    }
                    None => ("404 Not Found", "{}".to_string()),
                };
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn tags(digest: &str) -> String {
        format!(r#"{{"models":[{{"name":"gemma3:270m","digest":"{digest}"}}]}}"#)
    }

    #[test]
    fn validation_refuses_what_a_pull_would_refuse() {
        assert!(validate(&[ollama_entry(None), ollama_entry(Some(D1))])
            .unwrap_err()
            .to_string()
            .contains("twice"));
        let mut e = ollama_entry(Some("sha256:abc"));
        assert!(format!("{:#}", validate(&[e.clone()]).unwrap_err()).contains("not sha256"));
        e.digest = None;
        e.source = "https://example.org/m/gemma.gguf".into();
        assert!(validate(&[e.clone()])
            .unwrap_err()
            .to_string()
            .contains("needs digest"));
        e.digest = Some(D1.to_uppercase());
        validate(&[e.clone()]).unwrap();
        e.source = "ftp://example.org/gemma.gguf".into();
        assert!(format!("{:#}", validate(&[e]).unwrap_err()).contains("expected ollama://"));
        assert_eq!(
            file_name("https://h/a/b.gguf?download=1").unwrap(),
            "b.gguf"
        );
        assert!(file_name("https://h/a/").is_err());
    }

    #[tokio::test]
    async fn status_reports_missing_unpinned_verified_and_mismatch() {
        let (url, _) = serve(vec![("GET /api/tags", vec![tags(D1)])]);
        let dir = std::env::temp_dir();
        let mut other = ollama_entry(None);
        other.name = "qwen3:0.6b".into();
        other.source = "ollama://qwen3:0.6b".into();
        let entries = [
            ollama_entry(None),
            ollama_entry(Some(D1)),
            ollama_entry(Some(D2)),
            other,
        ];
        let got: Vec<ModelState> = status(&entries, &url, &dir)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.state)
            .collect();
        assert_eq!(
            got,
            vec![
                ModelState::Unpinned { digest: D1.into() },
                ModelState::Verified { digest: D1.into() },
                ModelState::Mismatch {
                    pinned: D2.into(),
                    found: D1.into()
                },
                ModelState::Missing,
            ]
        );
    }

    #[tokio::test]
    async fn a_pinned_model_already_installed_is_not_fetched_again() {
        let (url, seen) = serve(vec![("GET /api/tags", vec![tags(D1)])]);
        let r = pull(&ollama_entry(Some(D1)), &url, Path::new("."), PULL_TIMEOUT)
            .await
            .unwrap();
        assert!(!r.fetched && r.verified);
        assert_eq!(*seen.lock().unwrap(), vec!["GET /api/tags"]);
    }

    #[tokio::test]
    async fn an_unpinned_pull_reports_the_digest_to_pin() {
        let (url, seen) = serve(vec![
            ("GET /api/tags", vec![r#"{"models":[]}"#.into(), tags(D1)]),
            ("POST /api/pull", vec![r#"{"status":"success"}"#.into()]),
        ]);
        let r = pull(&ollama_entry(None), &url, Path::new("."), PULL_TIMEOUT)
            .await
            .unwrap();
        assert!(r.fetched && !r.verified);
        assert!(r.to_string().contains(&format!("digest = \"sha256:{D1}\"")));
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["GET /api/tags", "POST /api/pull", "GET /api/tags"]
        );
    }

    #[tokio::test]
    async fn a_pull_that_does_not_match_the_pin_is_removed() {
        let (url, seen) = serve(vec![
            ("GET /api/tags", vec![r#"{"models":[]}"#.into(), tags(D2)]),
            ("POST /api/pull", vec![r#"{"status":"success"}"#.into()]),
            ("DELETE /api/delete", vec!["{}".into()]),
        ]);
        let err = pull(&ollama_entry(Some(D1)), &url, Path::new("."), PULL_TIMEOUT)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(&format!("fetched sha256:{D2}")), "{err}");
        assert!(err.contains("It was removed"), "{err}");
        assert_eq!(seen.lock().unwrap().last().unwrap(), "DELETE /api/delete");
    }

    #[tokio::test]
    async fn ollama_refusing_the_pull_is_an_error() {
        let (url, _) = serve(vec![
            ("GET /api/tags", vec![r#"{"models":[]}"#.into()]),
            (
                "POST /api/pull",
                vec![r#"{"error":"pull model manifest: file does not exist"}"#.into()],
            ),
        ]);
        let err = pull(&ollama_entry(None), &url, Path::new("."), PULL_TIMEOUT)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("file does not exist"), "{err}");
    }

    #[tokio::test]
    async fn a_download_is_kept_only_when_its_hash_matches() {
        let body = "not really a gguf".to_string();
        let good = hex::encode(Sha256::digest(body.as_bytes()));
        let (url, _) = serve(vec![("GET /m.gguf", vec![body])]);
        let dir = std::env::temp_dir().join(format!("obc-edge-models-{}", std::process::id()));
        let dest = dir.join("m.gguf");
        let client = client(Duration::from_secs(5)).unwrap();

        let err = download_verified(&client, &format!("{url}/m.gguf"), &dest, D1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("It was deleted"), "{err}");
        assert!(!dest.exists());
        assert!(!dir.join("m.gguf.part").exists());

        download_verified(&client, &format!("{url}/m.gguf"), &dest, &good)
            .await
            .unwrap();
        assert_eq!(sha256_file(&dest).await.unwrap(), good);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Browser automation tools — headless Chrome via the Chrome DevTools Protocol
//! (CDP) with an HTTP-only fallback for simple page fetching.
//!
//! Real since 2026-09-12: navigation, snapshots, clicks, typing and scrolling
//! go over a WebSocket to the tab ([`super::browser_cdp`]). Before that the
//! tools opened a tab over HTTP and then fetched the page with a plain GET,
//! and `browser_click` / `browser_type` / `browser_scroll` reported success
//! without touching anything. With no Chrome reachable, `navigate` and
//! `snapshot` still fall back to a plain fetch and say so; the three that
//! need a page refuse instead of pretending.
//!
//! Inspired by the browser automation overhaul shipped in OpenClaw 3.13
//! (March 2026): stable CDP attach mode, batched actions, CSS/XPath selector
//! targeting, delayed-click support, and per-tab session management.
//!
//! # Architecture
//!
//! Each `BrowserSession` connects to a running Chrome/Chromium instance with
//! remote debugging enabled (`--remote-debugging-port=9222`).  When no CDP
//! endpoint is reachable the tools fall back to a plain HTTP fetch so the
//! agent can still retrieve page content without a browser.
//!
//! # Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `browser_navigate` | Navigate to a URL and return the page title |
//! | `browser_snapshot` | Capture an accessibility snapshot of the current page |
//! | `browser_click` | Click an element matched by CSS selector |
//! | `browser_type` | Type text into a focused input / textarea |
//! | `browser_fill_secret` | Fill a field from the vault, on the secret's own host only |
//! | `browser_scroll` | Scroll the page (up / down / to a selector) |
//! | `browser_new_tab` | Open a new browser tab |
//! | `browser_close_tab` | Close the active tab |

use super::browser_cdp::{self, Cdp};
use crate::credentials::CredentialResolver;
use crate::{traits::BlastRadius, traits::RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_conscience::{ReachDecision, ReachGate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── Browser Session ───────────────────────────────────────────────────────────

/// The profile used when connecting to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserProfile {
    /// A sandboxed headless Chrome managed by Oh-Ben-Claw (default).
    #[default]
    Headless,
    /// Attach to the user's own signed-in Chrome session for auth-aware tasks.
    User,
}

/// Internal state held inside an `Arc<Mutex<_>>` so all tool structs can share it.
#[derive(Debug, Default)]
struct SessionState {
    /// Id of the active tab (CDP target).
    active_target_id: Option<String>,
    /// The active tab's `webSocketDebuggerUrl`; `None` means no browser is
    /// attached and the tools are in plain-HTTP mode.
    active_ws_url: Option<String>,
    /// URL currently loaded in the active tab.
    current_url: Option<String>,
    /// Title of the current page.
    current_title: Option<String>,
    /// Ordered list of open tab target IDs.
    open_tabs: Vec<String>,
}

/// Manages a connection to a Chrome DevTools Protocol endpoint.
///
/// All browser tools accept an `Arc<BrowserSession>` and operate on the
/// shared session state, meaning a single session can be used across many
/// tool calls within the same agent loop.
#[derive(Debug, Clone)]
pub struct BrowserSession {
    /// Base URL of the CDP HTTP endpoint, e.g. `http://localhost:9222`.
    pub cdp_url: String,
    /// How long to wait for page-load / selector operations.
    pub timeout: Duration,
    /// Browser profile in use.
    pub profile: BrowserProfile,
    client: reqwest::Client,
    state: Arc<Mutex<SessionState>>,
}

impl BrowserSession {
    /// Create a session targeting the default local CDP port (9222).
    pub fn new() -> Self {
        Self::with_cdp_url("http://localhost:9222")
    }

    /// Create a session targeting a custom CDP base URL.
    pub fn with_cdp_url(cdp_url: impl Into<String>) -> Self {
        Self {
            cdp_url: cdp_url.into(),
            timeout: Duration::from_secs(30),
            profile: BrowserProfile::default(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            state: Arc::new(Mutex::new(SessionState::default())),
        }
    }

    /// Configure the timeout for page-load and selector operations.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the browser profile.
    pub fn with_profile(mut self, profile: BrowserProfile) -> Self {
        self.profile = profile;
        self
    }

    // ── CDP helpers ───────────────────────────────────────────────────────────

    /// List all open targets (tabs) from the CDP endpoint.
    async fn list_targets(&self) -> anyhow::Result<Vec<Value>> {
        let url = format!("{}/json/list", self.cdp_url);
        let response = self
            .client
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await?
            .json::<Vec<Value>>()
            .await?;
        Ok(response)
    }

    /// Open a new tab via the CDP `/json/new` endpoint. `PUT`: Chrome has
    /// refused `GET` here since version 111 (`405 … supports only PUT verb`),
    /// which is what "error decoding response body" on every navigation was.
    async fn new_target(&self, url: Option<&str>) -> anyhow::Result<Value> {
        let endpoint = if let Some(u) = url {
            format!("{}/json/new?{}", self.cdp_url, url_encode(u))
        } else {
            format!("{}/json/new", self.cdp_url)
        };
        let response = self
            .client
            .put(&endpoint)
            .timeout(self.timeout)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("CDP /json/new returned {status}: {}", text.trim());
        }
        let target: Value = serde_json::from_str(&text)?;
        Ok(target)
    }

    /// The active tab as a CDP client, opening a tab when there is none.
    /// `Err` means no browser is reachable at `cdp_url`.
    async fn attach(&self) -> anyhow::Result<Cdp> {
        let existing = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.active_ws_url.clone()
        };
        if let Some(ws) = existing {
            return Ok(Cdp::new(ws, self.timeout));
        }
        let target = self.new_target(None).await?;
        let id = target["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("CDP new target returned no id"))?
            .to_string();
        let ws = target["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("CDP new target returned no webSocketDebuggerUrl"))?
            .to_string();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active_target_id = Some(id.clone());
        state.active_ws_url = Some(ws.clone());
        state.open_tabs.push(id);
        Ok(Cdp::new(ws, self.timeout))
    }

    /// Whether a real browser is attached (as opposed to plain-HTTP mode).
    pub fn attached(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active_ws_url
            .is_some()
    }

    /// Refresh the remembered URL and title from the page itself.
    async fn sync_location(&self, cdp: &Cdp) {
        if let Ok((href, title)) = cdp.location().await {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.current_url = Some(href);
            state.current_title = Some(title);
        }
    }

    /// Run a page script through the attached tab; a plain-HTTP session gets
    /// the explanation the model needs instead of a pretend success.
    async fn page_script(&self, script: &str) -> Result<Value, String> {
        let cdp = self.attach().await.map_err(|e| {
            format!(
                "No browser is attached (CDP at {} unreachable: {e}); this action needs a real page. \
                 browser_navigate and browser_snapshot still work as a plain fetch.",
                self.cdp_url
            )
        })?;
        cdp.eval(script).await.map_err(|e| e.to_string())
    }

    /// Close a tab by its target ID.
    async fn close_target(&self, target_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/json/close/{}", self.cdp_url, target_id);
        self.client.get(&url).timeout(self.timeout).send().await?;
        Ok(())
    }

    /// Navigate the active tab. Returns the page title and whether a real
    /// browser did it (`false` = plain HTTP fetch, no page to click on).
    async fn navigate(&self, url: &str) -> anyhow::Result<(String, bool)> {
        match self.attach().await {
            Ok(cdp) => {
                cdp.navigate(url).await?;
                let (href, title) = cdp.location().await.unwrap_or_default();
                let title = if title.trim().is_empty() {
                    url.to_string()
                } else {
                    title
                };
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.current_url = Some(if href.is_empty() {
                    url.to_string()
                } else {
                    href
                });
                state.current_title = Some(title.clone());
                return Ok((title, true));
            }
            Err(e) => {
                tracing::warn!(
                    cdp = %self.cdp_url,
                    error = %e,
                    "browser: no CDP endpoint, falling back to a plain HTTP fetch"
                );
            }
        }

        // Plain-HTTP fallback: fetch the page and read its title.
        let page_text = self
            .client
            .get(url)
            .timeout(self.timeout)
            .send()
            .await?
            .text()
            .await
            .unwrap_or_default();

        let title = extract_title(&page_text).unwrap_or_else(|| url.to_string());

        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.current_url = Some(url.to_string());
            state.current_title = Some(title.clone());
        }

        Ok((title, false))
    }

    /// The current page as text: from the live DOM (title, headings, inputs,
    /// buttons, links with selectors, visible text) when a browser is
    /// attached, else a plain fetch stripped of tags.
    async fn fetch_snapshot(&self, max_chars: usize) -> anyhow::Result<String> {
        let url = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state
                .current_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No page loaded; call browser_navigate first"))?
        };
        if self.attached() {
            let cdp = self.attach().await?;
            let v = cdp.eval(&browser_cdp::js_snapshot(max_chars)).await?;
            self.sync_location(&cdp).await;
            return Ok(v.as_str().unwrap_or_default().to_string());
        }

        let html = self
            .client
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await?
            .text()
            .await?;

        Ok(strip_html(&html))
    }

    /// Returns the current page URL, if any.
    pub fn current_url(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .current_url
            .clone()
    }

    /// Returns the current page title, if any.
    pub fn current_title(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .current_title
            .clone()
    }

    /// Returns the number of open tabs tracked by this session.
    pub fn open_tab_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .open_tabs
            .len()
    }
}

impl Default for BrowserSession {
    fn default() -> Self {
        Self::new()
    }
}

// ── HTML helpers ──────────────────────────────────────────────────────────────

/// Minimal `<title>` extractor that avoids a full HTML parser dependency.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?.checked_add("<title".len())?;
    let rest = &html[start..];
    let content_start = rest.find('>')?.checked_add(1)?;
    let content = &rest[content_start..];
    let end = content.to_lowercase().find("</title>")?;
    let title = content[..end].trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Strip HTML tags and collapse whitespace to produce plain text.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_tag {
            tag_buf.push(c);
            if c == '>' {
                let tag_lower = tag_buf.to_lowercase();
                // Determine whether this is an opening or closing tag by looking
                // for a leading '/'.  The tag_buf starts immediately after the
                // opening '<', so a closing </script> produces "/script>".
                let is_closing = tag_lower.starts_with('/');
                // Match on the element name (skip the leading '/' for closing tags).
                let name = if is_closing {
                    &tag_lower[1..]
                } else {
                    &tag_lower[..]
                };
                if name.starts_with("script") || name.starts_with("script ") {
                    in_script = !is_closing;
                }
                if name.starts_with("style") || name.starts_with("style ") {
                    in_style = !is_closing;
                }
                in_tag = false;
                tag_buf.clear();
                result.push(' ');
            }
        } else if c == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if !in_script && !in_style {
            result.push(c);
        }
        i += 1;
    }

    // Collapse whitespace
    result
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(8000)
        .collect()
}

/// Percent-encode a URL for use as a query-string value in CDP `/json/new?{url}`.
///
/// Safe characters are those that are valid unencoded in a URL *path* but
/// should still be encoded when the whole URL appears as a *value* inside
/// another URL's query string.  We keep `:/` so the scheme and path are
/// preserved but encode `?` and `&` to avoid breaking the outer query.
fn url_encode(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | ':' | '/') {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect::<Vec<_>>()
            }
        })
        .collect()
}

// ── BrowserNavigateTool ───────────────────────────────────────────────────────

/// Navigate the browser to a URL and return the page title.
pub struct BrowserNavigateTool {
    session: Arc<BrowserSession>,
    /// Optional conscience reach gate. When present, the destination host must be
    /// on the egress allowlist for the `browser` tool, or navigation is refused
    /// before the browser is pointed anywhere — arbitrary browsing is an egress
    /// path exactly like the HTTP tool (the breach lesson applies to both).
    reach: Option<ReachGate>,
    /// Optional Track 0 auditor: a refused navigation writes a `conscience.reach`
    /// denial to the tamper-evident log, same as the HTTP tool and perception.
    auditor: Option<Arc<Mutex<obc_safety::ActionAuditor>>>,
}

impl BrowserNavigateTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self {
            session,
            reach: None,
            auditor: None,
        }
    }

    /// Attach a conscience reach gate. Every destination host must then be
    /// allowlisted for the `browser` tool, or navigation is refused.
    pub fn with_reach_gate(mut self, gate: ReachGate) -> Self {
        self.reach = Some(gate);
        self
    }

    /// Attach a Track 0 auditor so refused navigations become tamper-evident records.
    pub fn with_auditor(mut self, auditor: Arc<Mutex<obc_safety::ActionAuditor>>) -> Self {
        self.auditor = Some(auditor);
        self
    }

    /// Egress check for a URL. `Ok(())` if allowed (or no gate); `Err(reason)`
    /// if the reach gate refuses. Mirrors `HttpTool::check_reach` (tool `browser`).
    fn check_reach(&self, url: &str) -> Result<(), String> {
        let Some(gate) = &self.reach else {
            return Ok(());
        };
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        match gate.check("browser", &host) {
            ReachDecision::Allow { .. } => Ok(()),
            ReachDecision::Refuse(reason) => Err(reason.to_string()),
        }
    }
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str {
        "browser_navigate"
    }

    fn output_trust(&self) -> crate::traits::OutputTrust {
        // Returns the page title — attacker-controllable content (Track 0).
        crate::traits::OutputTrust::External
    }

    fn description(&self) -> &str {
        "Navigate the browser to a URL and return the page title. \
         Use this before taking a snapshot or interacting with page elements."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to navigate to (must start with http:// or https://)."
                },
                "wait_ms": {
                    "type": "integer",
                    "description": "Milliseconds to wait after navigation before returning (default: 0).",
                    "default": 0,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => return Ok(ToolResult::err("Missing required parameter 'url'")),
        };

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolResult::err(format!(
                "URL must start with http:// or https://, got: {url}"
            )));
        }

        // Conscience reach gate: the destination host must be on the egress
        // allowlist BEFORE the browser is pointed anywhere. Logged, not silent —
        // navigation is an egress path exactly like the HTTP tool.
        if let Err(reason) = self.check_reach(&url) {
            tracing::warn!(url = %url, refusal = %reason, "conscience: browser egress refused");
            if let Some(auditor) = &self.auditor {
                let host = reqwest::Url::parse(&url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_default();
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let mut g = auditor.lock().unwrap_or_else(|e| e.into_inner());
                let _ = g.record_conscience_refusal(ts, "conscience.reach", &host, &reason);
            }
            return Ok(ToolResult::err(format!("conscience: {reason}")));
        }

        let wait_ms = args
            .get("wait_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(10_000);

        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }

        match self.session.navigate(&url).await {
            Ok((title, true)) => Ok(ToolResult::ok(format!(
                "Navigated to {url}\nPage title: {title}\n(live browser tab; browser_snapshot, browser_click and browser_type act on it)"
            ))),
            Ok((title, false)) => Ok(ToolResult::ok(format!(
                "Fetched {url} over plain HTTP (no browser attached at {})\nPage title: {title}\n(browser_snapshot returns the fetched text; clicking and typing are not possible)",
                self.session.cdp_url
            ))),
            Err(e) => Ok(ToolResult::err(format!("Navigation failed for {url}: {e}"))),
        }
    }
}

// ── BrowserSnapshotTool ───────────────────────────────────────────────────────

/// Capture an accessibility / text snapshot of the current page.
pub struct BrowserSnapshotTool {
    session: Arc<BrowserSession>,
}

impl BrowserSnapshotTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn name(&self) -> &str {
        "browser_snapshot"
    }

    fn output_trust(&self) -> crate::traits::OutputTrust {
        // Returns rendered page content — attacker-controllable (Track 0).
        crate::traits::OutputTrust::External
    }

    fn description(&self) -> &str {
        "Capture a text snapshot of the current browser page, stripping HTML tags \
         to return readable content. Call browser_navigate first to load a page."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to return (default: 4000, max: 8000).",
                    "default": 4000,
                    "minimum": 100,
                    "maximum": 8000
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(4000)
            .clamp(100, 8000) as usize;

        let live = self.session.attached();
        match self.session.fetch_snapshot(max_chars).await {
            Ok(text) if live => Ok(ToolResult::ok(text)),
            Ok(text) => {
                let truncated: String = text.chars().take(max_chars).collect();
                let url = self
                    .session
                    .current_url()
                    .unwrap_or_else(|| "(unknown)".to_string());
                let title = self
                    .session
                    .current_title()
                    .unwrap_or_else(|| "(unknown)".to_string());
                Ok(ToolResult::ok(format!(
                    "URL: {url}\nTitle: {title}\n(plain HTTP fetch, no browser attached)\n\n{truncated}"
                )))
            }
            Err(e) => Ok(ToolResult::err(format!("Snapshot failed: {e}"))),
        }
    }
}

// ── BrowserClickTool ──────────────────────────────────────────────────────────

/// Click an element on the current page identified by a CSS selector.
pub struct BrowserClickTool {
    session: Arc<BrowserSession>,
}

impl BrowserClickTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str {
        "browser_click"
    }

    fn risk_class(&self) -> RiskClass {
        // A click acts on a live page; replaying it acts again (2026-09-26).
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn description(&self) -> &str {
        "Click an element on the current browser page using a CSS selector. \
         Use browser_snapshot first to identify the selectors available on the page."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector of the element to click (e.g. 'button#submit', 'a.nav-link[href=\"/home\"]')."
                },
                "delay_ms": {
                    "type": "integer",
                    "description": "Delay in milliseconds before clicking, to mimic human interaction (default: 0, max: 2000).",
                    "default": 0,
                    "minimum": 0,
                    "maximum": 2000
                }
            },
            "required": ["selector"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let selector = match args.get("selector").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("Missing required parameter 'selector'")),
        };

        let delay_ms = args
            .get("delay_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(2000);

        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        let url = match self.session.current_url() {
            Some(u) => u,
            None => {
                return Ok(ToolResult::err(
                    "No page loaded; call browser_navigate first",
                ))
            }
        };

        match self.session.page_script(&browser_cdp::js_click(&selector)).await {
            Ok(v) if v.as_str() == Some("NOT_FOUND") => Ok(ToolResult::err(format!(
                "No element matches '{selector}' on {url}; take a browser_snapshot for the selectors that exist"
            ))),
            Ok(v) => {
                tokio::time::sleep(Duration::from_millis(400)).await;
                if let Ok(cdp) = self.session.attach().await {
                    self.session.sync_location(&cdp).await;
                }
                let now = self.session.current_url().unwrap_or(url);
                let title = self.session.current_title().unwrap_or_default();
                Ok(ToolResult::ok(format!(
                    "Clicked {} '{selector}'. Page is now {now} — {title}",
                    v.as_str().unwrap_or("element").trim_start_matches("OK ")
                )))
            }
            Err(e) => Ok(ToolResult::err(format!("Click failed: {e}"))),
        }
    }
}

// ── BrowserTypeTool ───────────────────────────────────────────────────────────

/// Type text into the focused element or into an element matched by a selector.
pub struct BrowserTypeTool {
    session: Arc<BrowserSession>,
}

impl BrowserTypeTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn name(&self) -> &str {
        "browser_type"
    }

    fn risk_class(&self) -> RiskClass {
        // Typing into a live page is a side effect; replaying it types again.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn description(&self) -> &str {
        "Type text into a browser input field. Optionally specify a CSS selector to \
         focus the target element first. Use browser_click to focus an element without typing."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to type into the element."
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector to focus before typing."
                },
                "submit": {
                    "type": "boolean",
                    "description": "If true, press Enter after typing (default: false).",
                    "default": false
                },
                "delay_ms": {
                    "type": "integer",
                    "description": "Delay between keystrokes in milliseconds for human-like input (default: 0, max: 200).",
                    "default": 0,
                    "minimum": 0,
                    "maximum": 200
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return Ok(ToolResult::err("Missing required parameter 'text'")),
        };

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(String::from);
        let submit = args
            .get("submit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let url = match self.session.current_url() {
            Some(u) => u,
            None => {
                return Ok(ToolResult::err(
                    "No page loaded; call browser_navigate first",
                ))
            }
        };

        let target = selector.as_deref().unwrap_or("focused element");

        let cdp = match self.session.attach().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::err(format!(
                "No browser is attached (CDP at {} unreachable: {e}); typing needs a real page.",
                self.session.cdp_url
            )))
            }
        };
        if let Some(sel) = &selector {
            match cdp.eval(&browser_cdp::js_focus(sel)).await {
                Ok(v) if v.as_str() == Some("NOT_FOUND") => {
                    return Ok(ToolResult::err(format!(
                        "No element matches '{sel}' on {url}; take a browser_snapshot for the selectors that exist"
                    )))
                }
                Ok(_) => {}
                Err(e) => return Ok(ToolResult::err(format!("Focus failed: {e}"))),
            }
        }
        if let Err(e) = cdp.insert_text(&text).await {
            return Ok(ToolResult::err(format!("Typing failed: {e}")));
        }
        if submit {
            if let Err(e) = cdp.press_enter().await {
                return Ok(ToolResult::err(format!("Typed, but Enter failed: {e}")));
            }
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        self.session.sync_location(&cdp).await;
        let suffix = if submit { " + Enter" } else { "" };
        let now = self.session.current_url().unwrap_or(url);
        Ok(ToolResult::ok(format!(
            "Typed '{text}'{suffix} into {target}. Page is now {now}"
        )))
    }
}

// ── BrowserFillSecretTool ─────────────────────────────────────────────────────

/// Fill a field from the secrets vault without the model ever seeing the value
/// (docs/PERSONAL-AGENTS-2026-11.md, item 7).
///
/// The model names the secret (`vault:github`); the value is resolved here, at
/// the moment of the fill, through the same [`CredentialResolver`] that injects
/// HTTP credentials, and typed into the field. It never appears in the tool
/// result, and a snapshot of the page shows the field as `[hidden]`.
///
/// **A secret fills only on its own domain.** The binding is the conscience
/// egress allowlist: a fill is allowed only when the page's live host is
/// allowlisted for the `browser` tool *and* that host's rule names this secret
/// as its credential. The host is read from the tab after the field is
/// focused, not from the session's remembered URL, so a redirect cannot carry
/// the fill to another site. Only `https` pages qualify, and only a field in
/// the top document (focus that lands in a frame is refused).
///
/// Every outcome is audited when an auditor is attached: a fill as
/// [`obc_safety::ActionAuditor::record_secret_use`], a refusal as a
/// `conscience.secret` denial.
pub struct BrowserFillSecretTool {
    session: Arc<BrowserSession>,
    reach: ReachGate,
    resolver: Arc<dyn CredentialResolver>,
    auditor: Option<Arc<Mutex<obc_safety::ActionAuditor>>>,
}

impl BrowserFillSecretTool {
    /// Both the reach gate and the resolver are required: without the gate no
    /// secret is bound to any host, so nothing could ever be filled.
    pub fn new(
        session: Arc<BrowserSession>,
        reach: ReachGate,
        resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self {
            session,
            reach,
            resolver,
            auditor: None,
        }
    }

    /// Attach a Track 0 auditor so each fill and each refusal is a
    /// tamper-evident record.
    pub fn with_auditor(mut self, auditor: Arc<Mutex<obc_safety::ActionAuditor>>) -> Self {
        self.auditor = Some(auditor);
        self
    }

    /// The secret name from `vault:<name>`, or why the reference is unusable.
    fn secret_name(reference: &str) -> Result<&str, String> {
        match reference.strip_prefix("vault:") {
            Some(name) if !name.trim().is_empty() => Ok(name.trim()),
            _ => Err(format!(
                "secret must be a vault reference like \"vault:github\", got \"{reference}\""
            )),
        }
    }

    /// May the secret `name` fill a field on the page at `href`? `Ok(host)` if
    /// the page is `https` and its host's `browser` reach rule names `name` as
    /// its credential; `Err(reason)` otherwise. Pure, so it is tested offline.
    fn authorize(&self, name: &str, href: &str) -> Result<String, String> {
        let url = reqwest::Url::parse(href)
            .map_err(|_| format!("the page has no usable address ({href})"))?;
        let host = url.host_str().unwrap_or_default().to_string();
        if url.scheme() != "https" {
            return Err(format!(
                "secrets fill only on https pages; this page is {}",
                url.scheme()
            ));
        }
        match self.reach.check("browser", &host) {
            ReachDecision::Refuse(reason) => Err(reason.to_string()),
            ReachDecision::Allow { credential } if credential.as_deref() == Some(name) => Ok(host),
            ReachDecision::Allow { .. } => Err(format!(
                "conscience: vault:{name} is not bound to '{host}' (its reach rule names \
                 a different credential, or none)"
            )),
        }
    }

    /// Write a `conscience.secret` refusal to the chain, if an auditor is attached.
    fn audit_refusal(&self, name: &str, host: &str, reason: &str) {
        if let Some(auditor) = &self.auditor {
            let mut g = auditor.lock().unwrap_or_else(|e| e.into_inner());
            let subject = format!("vault:{name} @ {host}");
            let _ = g.record_conscience_refusal(now_ms(), "conscience.secret", &subject, reason);
        }
    }

    /// Log and audit a refusal, and turn it into the tool's error.
    fn refuse(&self, name: &str, host: &str, reason: String) -> ToolResult {
        tracing::warn!(secret = %name, host = %host, refusal = %reason, "browser_fill_secret refused");
        self.audit_refusal(name, host, &reason);
        ToolResult::err(reason)
    }
}

/// Milliseconds since the Unix epoch, for audit records.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[async_trait]
impl Tool for BrowserFillSecretTool {
    fn name(&self) -> &str {
        "browser_fill_secret"
    }

    fn risk_class(&self) -> RiskClass {
        // Typing a credential into a live page; replaying it types it again.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn description(&self) -> &str {
        "Fill a browser input field (a password, API key or other secret) from the \
         secrets vault. Give the field's CSS selector and the secret as \"vault:<name>\". \
         The value is never shown to you. It fills only on the https site the secret \
         belongs to; anywhere else it is refused."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "field": {
                    "type": "string",
                    "description": "CSS selector of the input or textarea to fill (from browser_snapshot)."
                },
                "secret": {
                    "type": "string",
                    "description": "The secret to fill, as \"vault:<name>\"."
                },
                "submit": {
                    "type": "boolean",
                    "description": "If true, press Enter after filling (default: false).",
                    "default": false
                }
            },
            "required": ["field", "secret"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(field) = args.get("field").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("Missing required parameter 'field'"));
        };
        let Some(reference) = args.get("secret").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("Missing required parameter 'secret'"));
        };
        let name = match Self::secret_name(reference) {
            Ok(n) => n,
            Err(e) => return Ok(ToolResult::err(e)),
        };
        let submit = args
            .get("submit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let cdp = match self.session.attach().await {
            Ok(c) => c,
            Err(e) => {
                let url = &self.session.cdp_url;
                let msg = format!("No browser is attached (CDP at {url} unreachable: {e}); filling needs a real page.");
                return Ok(ToolResult::err(msg));
            }
        };

        // Focus the field and read the page's address in one script, so the
        // host checked is the host of the page the field is on.
        let v = match cdp.eval(&browser_cdp::js_focus_secret_field(field)).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::err(format!("Focus failed: {e}"))),
        };
        let status = v.get(0).and_then(|s| s.as_str()).unwrap_or_default();
        let href = v.get(1).and_then(|s| s.as_str()).unwrap_or_default();
        match status {
            "OK" => {}
            "NOT_FOUND" => {
                return Ok(ToolResult::err(format!(
                    "No element matches '{field}' on {href}; take a browser_snapshot for the selectors that exist"
                )))
            }
            "NOT_A_FIELD" => {
                return Ok(ToolResult::err(format!(
                    "'{field}' is not an editable text field; secrets fill only inputs and textareas"
                )))
            }
            _ => {
                return Ok(ToolResult::err(format!(
                    "'{field}' could not be focused in the page itself (a field inside a frame is not filled)"
                )))
            }
        }

        // The domain binding comes before the vault is touched: a refused fill
        // never resolves the secret.
        let host = match self.authorize(name, href) {
            Ok(h) => h,
            Err(reason) => {
                let host = reqwest::Url::parse(href)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_default();
                return Ok(self.refuse(name, &host, reason));
            }
        };
        let Some(secret) = self.resolver.resolve(name) else {
            let reason =
                format!("vault:{name} could not be resolved (not in the vault or environment)");
            return Ok(self.refuse(name, &host, reason));
        };

        // Errors from here on carry the CDP method, never the typed text.
        let typed = cdp.insert_text(&secret).await;
        drop(secret);
        if let Err(e) = typed {
            return Ok(ToolResult::err(format!("Filling failed: {e}")));
        }
        tracing::info!(secret = %name, host = %host, field = %field, "browser_fill_secret: filled");
        if let Some(auditor) = &self.auditor {
            let mut g = auditor.lock().unwrap_or_else(|e| e.into_inner());
            let _ = g.record_secret_use(now_ms(), name, &host, field);
        }
        if submit {
            if let Err(e) = cdp.press_enter().await {
                return Ok(ToolResult::err(format!("Filled, but Enter failed: {e}")));
            }
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        self.session.sync_location(&cdp).await;
        let suffix = if submit { " + Enter" } else { "" };
        let now = self
            .session
            .current_url()
            .unwrap_or_else(|| href.to_string());
        Ok(ToolResult::ok(format!(
            "Filled {field} on {host} from vault:{name}{suffix} (value not shown). Page is now {now}"
        )))
    }
}

// ── BrowserScrollTool ─────────────────────────────────────────────────────────

/// Scroll the current page.
pub struct BrowserScrollTool {
    session: Arc<BrowserSession>,
}

impl BrowserScrollTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserScrollTool {
    fn name(&self) -> &str {
        "browser_scroll"
    }

    fn description(&self) -> &str {
        "Scroll the current browser page. Direction can be 'up', 'down', 'top', or 'bottom'. \
         Optionally scroll to an element matched by a CSS selector."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "description": "Scroll direction.",
                    "enum": ["up", "down", "top", "bottom"],
                    "default": "down"
                },
                "amount_px": {
                    "type": "integer",
                    "description": "Pixels to scroll for 'up'/'down' directions (default: 500).",
                    "default": 500,
                    "minimum": 1,
                    "maximum": 10000
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector — scroll to element instead of using direction."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let url = match self.session.current_url() {
            Some(u) => u,
            None => {
                return Ok(ToolResult::err(
                    "No page loaded; call browser_navigate first",
                ))
            }
        };

        if let Some(selector) = args.get("selector").and_then(|v| v.as_str()) {
            return Ok(
                match self
                    .session
                    .page_script(&browser_cdp::js_scroll_to(selector))
                    .await
                {
                    Ok(v) if v.as_str() == Some("NOT_FOUND") => {
                        ToolResult::err(format!("No element matches '{selector}' on {url}"))
                    }
                    Ok(_) => ToolResult::ok(format!("Scrolled to '{selector}' on {url}")),
                    Err(e) => ToolResult::err(format!("Scroll failed: {e}")),
                },
            );
        }

        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("down");

        let amount_px = args
            .get("amount_px")
            .and_then(|v| v.as_u64())
            .unwrap_or(500)
            .clamp(1, 10_000);

        match self
            .session
            .page_script(&browser_cdp::js_scroll(direction, amount_px))
            .await
        {
            Ok(v) => Ok(ToolResult::ok(format!(
                "Scrolled {direction} on {url} (scrollY now {})",
                v.as_str().unwrap_or("?").trim_start_matches("OK ")
            ))),
            Err(e) => Ok(ToolResult::err(format!("Scroll failed: {e}"))),
        }
    }
}

// ── BrowserNewTabTool ─────────────────────────────────────────────────────────

/// Open a new browser tab, optionally navigating immediately.
pub struct BrowserNewTabTool {
    session: Arc<BrowserSession>,
}

impl BrowserNewTabTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserNewTabTool {
    fn name(&self) -> &str {
        "browser_new_tab"
    }

    fn risk_class(&self) -> RiskClass {
        // Opens a tab in the shared headless Chrome; replaying it leaks tabs.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn description(&self) -> &str {
        "Open a new browser tab. Optionally navigate to a URL in the new tab immediately."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Optional URL to load in the new tab."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let url = args.get("url").and_then(|v| v.as_str()).map(String::from);

        match self.session.new_target(url.as_deref()).await {
            Ok(target) => {
                let id = target["id"].as_str().unwrap_or("unknown").to_string();
                {
                    let mut state = self.session.state.lock().unwrap_or_else(|p| p.into_inner());
                    state.active_target_id = Some(id.clone());
                    state.active_ws_url = target["webSocketDebuggerUrl"].as_str().map(String::from);
                    state.open_tabs.push(id.clone());
                    state.current_url = url.clone();
                    state.current_title = None;
                }
                let msg = if let Some(ref u) = url {
                    format!("Opened new tab (id={id}) and navigated to {u}")
                } else {
                    format!("Opened new tab (id={id})")
                };
                Ok(ToolResult::ok(msg))
            }
            Err(e) => Ok(ToolResult::err(format!("Failed to open new tab: {e}"))),
        }
    }
}

// ── BrowserCloseTabTool ───────────────────────────────────────────────────────

/// Close the active browser tab.
pub struct BrowserCloseTabTool {
    session: Arc<BrowserSession>,
}

impl BrowserCloseTabTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserCloseTabTool {
    fn name(&self) -> &str {
        "browser_close_tab"
    }

    fn risk_class(&self) -> RiskClass {
        // Closes a tab someone else may be using; replaying it closes another.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    fn description(&self) -> &str {
        "Close the currently active browser tab."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let target_id = {
            let state = self.session.state.lock().unwrap_or_else(|p| p.into_inner());
            state.active_target_id.clone()
        };

        // The session's idea of which tab is active is a cached belief, and the user
        // can invalidate it at any moment by closing a tab by hand. When it is empty,
        // ask the browser rather than reporting "no tabs" at a window full of them —
        // this is what `list_targets` was written for and never wired to.
        let target_id = match target_id {
            Some(id) => id,
            None => match self.session.list_targets().await {
                Ok(targets) => match targets
                    .iter()
                    .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                    .find_map(|t| t.get("id").and_then(|v| v.as_str()))
                {
                    Some(id) => id.to_string(),
                    None => return Ok(ToolResult::err("No open tab to close")),
                },
                Err(e) => {
                    return Ok(ToolResult::err(format!(
                        "No active tab, and could not list tabs: {e}"
                    )))
                }
            },
        };

        match self.session.close_target(&target_id).await {
            Ok(()) => {
                let mut state = self.session.state.lock().unwrap_or_else(|p| p.into_inner());
                state.open_tabs.retain(|id| id != &target_id);
                state.active_target_id = state.open_tabs.last().cloned();
                state.active_ws_url = None; // re-attached on the next action
                state.current_url = None;
                state.current_title = None;
                Ok(ToolResult::ok(format!("Closed tab {target_id}")))
            }
            Err(e) => Ok(ToolResult::err(format!("Failed to close tab: {e}"))),
        }
    }
}

// ── Convenience constructor ───────────────────────────────────────────────────

/// Build all browser tools sharing the same `BrowserSession`.
///
/// ```
/// use obc_tools::builtin::browser::all_browser_tools;
///
/// let tools = all_browser_tools(None);
/// assert_eq!(tools.len(), 7);
/// ```
pub fn all_browser_tools(cdp_url: Option<&str>) -> Vec<Box<dyn Tool>> {
    all_browser_tools_with_reach(cdp_url, None, None, None)
}

/// Like [`all_browser_tools`], but gates the navigate tool's destination host
/// through a conscience reach gate (tool name `browser`) and audits refusals.
/// Navigation is an egress path exactly like the HTTP tool, so it gets the same
/// deterministic allowlist the model cannot override. `None, None, None`
/// reproduces [`all_browser_tools`] exactly.
///
/// With both a gate and a `resolver`, `browser_fill_secret` is registered too:
/// the gate's host rules are what bind a secret to its domain, so without one
/// the tool could never fill anything and is left out of the prompt.
pub fn all_browser_tools_with_reach(
    cdp_url: Option<&str>,
    reach: Option<ReachGate>,
    auditor: Option<Arc<Mutex<obc_safety::ActionAuditor>>>,
    resolver: Option<Arc<dyn CredentialResolver>>,
) -> Vec<Box<dyn Tool>> {
    let session = Arc::new(if let Some(url) = cdp_url {
        BrowserSession::with_cdp_url(url)
    } else {
        BrowserSession::new()
    });

    let fill_secret = match (&reach, resolver) {
        (Some(g), Some(r)) => {
            let mut t = BrowserFillSecretTool::new(session.clone(), g.clone(), r);
            if let Some(a) = auditor.clone() {
                t = t.with_auditor(a);
            }
            Some(t)
        }
        _ => None,
    };

    let mut navigate = BrowserNavigateTool::new(session.clone());
    if let Some(g) = reach {
        navigate = navigate.with_reach_gate(g);
    }
    if let Some(a) = auditor {
        navigate = navigate.with_auditor(a);
    }

    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(navigate),
        Box::new(BrowserSnapshotTool::new(session.clone())),
        Box::new(BrowserClickTool::new(session.clone())),
        Box::new(BrowserTypeTool::new(session.clone())),
        Box::new(BrowserScrollTool::new(session.clone())),
        Box::new(BrowserNewTabTool::new(session.clone())),
        Box::new(BrowserCloseTabTool::new(session)),
    ];
    if let Some(t) = fill_secret {
        tools.push(Box::new(t));
    }
    tools
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session() -> Arc<BrowserSession> {
        Arc::new(BrowserSession::with_cdp_url("http://localhost:19222"))
    }

    // ── BrowserSession ────────────────────────────────────────────────────────

    #[test]
    fn session_default_state() {
        let s = BrowserSession::new();
        assert_eq!(s.cdp_url, "http://localhost:9222");
        assert_eq!(s.profile, BrowserProfile::Headless);
        assert!(s.current_url().is_none());
        assert!(s.current_title().is_none());
        assert_eq!(s.open_tab_count(), 0);
    }

    #[test]
    fn session_with_cdp_url() {
        let s = BrowserSession::with_cdp_url("http://192.168.1.100:9222");
        assert_eq!(s.cdp_url, "http://192.168.1.100:9222");
    }

    #[test]
    fn session_with_profile() {
        let s = BrowserSession::new().with_profile(BrowserProfile::User);
        assert_eq!(s.profile, BrowserProfile::User);
    }

    // ── HTML helpers ──────────────────────────────────────────────────────────

    #[test]
    fn extract_title_finds_title() {
        let html = "<html><head><title>Hello World</title></head><body></body></html>";
        assert_eq!(extract_title(html), Some("Hello World".to_string()));
    }

    #[test]
    fn extract_title_handles_whitespace() {
        let html = "<html><head><title>  Spaced Title  </title></head></html>";
        assert_eq!(extract_title(html), Some("Spaced Title".to_string()));
    }

    #[test]
    fn extract_title_returns_none_when_absent() {
        let html = "<html><body>no title here</body></html>";
        assert_eq!(extract_title(html), None);
    }

    #[test]
    fn strip_html_removes_tags() {
        let html = "<p>Hello <b>World</b>!</p>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<p>"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn strip_html_removes_script_content() {
        let html = "<p>Text</p><script>alert('xss')</script><p>More</p>";
        let text = strip_html(html);
        assert!(text.contains("Text"));
        assert!(!text.contains("alert"));
        // Content after the closing </script> tag should still appear
        assert!(text.contains("More"));
    }

    #[test]
    fn strip_html_content_after_closing_script_is_visible() {
        // Verifies the closing-tag detection works: text after </script> appears.
        let html = "<script>var x = 1;</script><p>After script</p>";
        let text = strip_html(html);
        assert!(!text.contains("var x"), "script content should be stripped");
        assert!(
            text.contains("After script"),
            "text after </script> should be visible"
        );
    }

    #[test]
    fn strip_html_removes_style_content() {
        let html = "<style>.btn { color: red; }</style><p>Visible</p>";
        let text = strip_html(html);
        assert!(!text.contains(".btn"));
        assert!(text.contains("Visible"));
    }

    #[test]
    fn strip_html_content_after_closing_style_is_visible() {
        let html = "<style>body { margin: 0; }</style><p>After style</p>";
        let text = strip_html(html);
        assert!(!text.contains("margin"), "style content should be stripped");
        assert!(
            text.contains("After style"),
            "text after </style> should be visible"
        );
    }

    #[test]
    fn url_encode_passes_safe_chars() {
        let s = url_encode("https://example.com/path");
        assert!(s.contains("https://example.com/path"));
    }

    #[test]
    fn url_encode_encodes_question_mark() {
        // '?' must be encoded so it doesn't break the outer query string
        let s = url_encode("https://example.com/search?q=1");
        assert!(!s.contains('?'), "url_encode should encode '?'");
        assert!(s.contains("%3F"));
    }

    #[test]
    fn url_encode_encodes_space() {
        let s = url_encode("hello world");
        assert!(!s.contains(' '));
        assert!(s.contains("%20"));
    }

    // ── Tool names and schemas ────────────────────────────────────────────────

    #[test]
    fn all_browser_tools_returns_seven_tools() {
        let tools = all_browser_tools(None);
        assert_eq!(tools.len(), 7);
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_snapshot"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_type"));
        assert!(names.contains(&"browser_scroll"));
        assert!(names.contains(&"browser_new_tab"));
        assert!(names.contains(&"browser_close_tab"));
    }

    #[test]
    fn navigate_tool_schema_requires_url() {
        let s = make_session();
        let tool = BrowserNavigateTool::new(s);
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("url")));
    }

    #[test]
    fn click_tool_schema_requires_selector() {
        let s = make_session();
        let tool = BrowserClickTool::new(s);
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("selector")));
    }

    #[test]
    fn type_tool_schema_requires_text() {
        let s = make_session();
        let tool = BrowserTypeTool::new(s);
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("text")));
    }

    // ── Execute — synchronous / offline paths ─────────────────────────────────

    #[tokio::test]
    async fn navigate_rejects_non_http_urls() {
        let s = make_session();
        let tool = BrowserNavigateTool::new(s);
        let result = tool
            .execute(json!({"url": "ftp://example.com"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("http"));
    }

    #[tokio::test]
    async fn navigate_missing_url_returns_error() {
        let s = make_session();
        let tool = BrowserNavigateTool::new(s);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("url"));
    }

    #[tokio::test]
    async fn snapshot_no_page_loaded_returns_error() {
        let s = make_session();
        let tool = BrowserSnapshotTool::new(s);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("browser_navigate"));
    }

    #[tokio::test]
    async fn click_no_page_loaded_returns_error() {
        let s = make_session();
        let tool = BrowserClickTool::new(s);
        let result = tool.execute(json!({"selector": "button"})).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn type_no_page_loaded_returns_error() {
        let s = make_session();
        let tool = BrowserTypeTool::new(s);
        let result = tool.execute(json!({"text": "hello"})).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn scroll_no_page_loaded_returns_error() {
        let s = make_session();
        let tool = BrowserScrollTool::new(s);
        let result = tool.execute(json!({"direction": "down"})).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn close_tab_no_active_tab_returns_error() {
        let s = make_session();
        let tool = BrowserCloseTabTool::new(s);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("No active tab"));
    }

    #[tokio::test]
    async fn click_missing_selector_returns_error() {
        let s = make_session();
        let tool = BrowserClickTool::new(s);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("selector"));
    }

    #[tokio::test]
    async fn type_missing_text_returns_error() {
        let s = make_session();
        let tool = BrowserTypeTool::new(s);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("text"));
    }

    #[test]
    fn browser_profile_default_is_headless() {
        assert_eq!(BrowserProfile::default(), BrowserProfile::Headless);
    }

    #[test]
    fn session_state_open_tab_count_starts_zero() {
        let s = BrowserSession::new();
        assert_eq!(s.open_tab_count(), 0);
    }

    // ── BrowserFillSecretTool ─────────────────────────────────────────────────

    /// `github.com` is bound to `vault:github`, `docs.example.com` is
    /// allowlisted with no credential, and `other.example.com` is bound to a
    /// different secret.
    fn secret_gate() -> ReachGate {
        use obc_conscience::{HostRule, ReachScope, ToolReach};
        let rule = |host: &str, credential: Option<&str>| HostRule {
            host: host.into(),
            purpose: "test".into(),
            credential: credential.map(str::to_string),
        };
        ReachGate::new(
            vec![
                rule("github.com", Some("github")),
                rule("docs.example.com", None),
                rule("other.example.com", Some("other")),
            ],
            vec![ToolReach {
                tool: "browser".into(),
                scope: ReachScope::Egress,
            }],
        )
    }

    /// Counts lookups, so a test can show a refused fill never reads the vault.
    struct CountingResolver(std::sync::atomic::AtomicUsize);
    impl CredentialResolver for CountingResolver {
        fn resolve(&self, name: &str) -> Option<String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (name == "github").then(|| "ghp_secret_value".to_string())
        }
    }

    fn fill_tool(resolver: Arc<CountingResolver>) -> BrowserFillSecretTool {
        BrowserFillSecretTool::new(make_session(), secret_gate(), resolver)
    }

    fn counting() -> Arc<CountingResolver> {
        Arc::new(CountingResolver(std::sync::atomic::AtomicUsize::new(0)))
    }

    #[test]
    fn secret_reference_must_be_a_vault_name() {
        assert_eq!(
            BrowserFillSecretTool::secret_name("vault:github"),
            Ok("github")
        );
        assert!(BrowserFillSecretTool::secret_name("github").is_err());
        assert!(BrowserFillSecretTool::secret_name("vault:").is_err());
        assert!(BrowserFillSecretTool::secret_name("vault:  ").is_err());
    }

    #[test]
    fn secret_fills_on_its_own_host() {
        let t = fill_tool(counting());
        assert_eq!(
            t.authorize("github", "https://github.com/login"),
            Ok("github.com".to_string())
        );
    }

    #[test]
    fn secret_is_refused_on_any_other_host() {
        let t = fill_tool(counting());
        // not allowlisted at all
        assert!(t
            .authorize("github", "https://github.com.evil.example/login")
            .is_err());
        // a subdomain is a different host
        assert!(t
            .authorize("github", "https://gist.github.com/login")
            .is_err());
        // allowlisted, but bound to no secret
        assert!(t.authorize("github", "https://docs.example.com/").is_err());
        // allowlisted, but bound to a different secret
        let e = t
            .authorize("github", "https://other.example.com/")
            .unwrap_err();
        assert!(e.contains("not bound"), "{e}");
    }

    #[test]
    fn secret_is_refused_off_https() {
        let t = fill_tool(counting());
        assert!(t.authorize("github", "http://github.com/login").is_err());
        assert!(t.authorize("github", "data:text/html,<input>").is_err());
        assert!(t.authorize("github", "about:blank").is_err());
    }

    #[test]
    fn secret_is_refused_when_the_browser_has_no_egress() {
        use obc_conscience::HostRule;
        let gate = ReachGate::new(
            vec![HostRule {
                host: "github.com".into(),
                purpose: "test".into(),
                credential: Some("github".into()),
            }],
            vec![], // `browser` unlisted: no reach
        );
        let t = BrowserFillSecretTool::new(make_session(), gate, counting());
        assert!(t.authorize("github", "https://github.com/login").is_err());
    }

    #[tokio::test]
    async fn fill_without_a_browser_never_reads_the_vault() {
        let r = counting();
        let t = fill_tool(Arc::clone(&r));
        let result = t
            .execute(json!({"field": "#password", "secret": "vault:github"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(r.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        let text = format!("{:?}", result);
        assert!(!text.contains("ghp_secret_value"));
    }

    #[tokio::test]
    async fn fill_rejects_a_literal_secret() {
        let r = counting();
        let t = fill_tool(Arc::clone(&r));
        let result = t
            .execute(json!({"field": "#password", "secret": "hunter2"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("vault:"));
        assert_eq!(r.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fill_requires_field_and_secret() {
        let t = fill_tool(counting());
        let r = t.execute(json!({"secret": "vault:github"})).await.unwrap();
        assert!(r.error.as_deref().unwrap_or("").contains("field"));
        let r = t.execute(json!({"field": "#p"})).await.unwrap();
        assert!(r.error.as_deref().unwrap_or("").contains("secret"));
    }

    #[test]
    fn fill_secret_is_registered_only_with_a_gate_and_a_resolver() {
        let has = |tools: &[Box<dyn Tool>]| tools.iter().any(|t| t.name() == "browser_fill_secret");
        let resolver = || Some(counting() as Arc<dyn CredentialResolver>);
        assert!(!has(&all_browser_tools(None)));
        assert!(!has(&all_browser_tools_with_reach(
            None,
            Some(secret_gate()),
            None,
            None
        )));
        assert!(!has(&all_browser_tools_with_reach(
            None,
            None,
            None,
            resolver()
        )));
        assert!(has(&all_browser_tools_with_reach(
            None,
            Some(secret_gate()),
            None,
            resolver()
        )));
    }

    #[test]
    fn fill_secret_is_not_replayable() {
        let t = fill_tool(counting());
        assert!(!t.risk_class().reversible);
        assert!(!t.risk_class().physical);
    }
}

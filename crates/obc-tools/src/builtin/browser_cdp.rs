//! A small Chrome DevTools Protocol client (parity plan Stage 3, item 8,
//! 2026-09-12).
//!
//! Until now the browser tools had no protocol behind them: `browser_navigate`
//! opened a tab over the HTTP endpoint and then fetched the page with a plain
//! HTTP GET, `browser_snapshot` stripped that HTML, and `browser_click` /
//! `browser_type` / `browser_scroll` logged the request and reported success
//! without touching a page. A model that "clicked" got told it had.
//!
//! This module is the missing half: one WebSocket command at a time against a
//! tab's `webSocketDebuggerUrl` — `Page.navigate`, `Runtime.evaluate`,
//! `Input.insertText`, `Input.dispatchKeyEvent` — plus the JavaScript the
//! tools evaluate in the page, kept as pure string builders so they are
//! unit-tested without a browser. A connection per command is deliberate: the
//! tools are called a few times per turn, Chrome is on loopback, and it keeps
//! the client free of session state that goes stale when a tab is closed by
//! hand.

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// One tab's DevTools endpoint.
#[derive(Debug, Clone)]
pub struct Cdp {
    pub ws_url: String,
    pub timeout: Duration,
}

impl Cdp {
    pub fn new(ws_url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            ws_url: ws_url.into(),
            timeout,
        }
    }

    /// Send one CDP command and return its `result`.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let (mut ws, _) = tokio::time::timeout(self.timeout, connect_async(&self.ws_url))
            .await
            .with_context(|| format!("CDP connect to {} timed out", self.ws_url))?
            .with_context(|| format!("CDP connect to {} failed", self.ws_url))?;
        let id = 1;
        ws.send(Message::Text(
            json!({"id": id, "method": method, "params": params}).to_string(),
        ))
        .await
        .context("CDP send")?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("CDP {method}: no reply within {:?}", self.timeout);
            }
            let msg = tokio::time::timeout(remaining, ws.next())
                .await
                .with_context(|| format!("CDP {method}: no reply within {:?}", self.timeout))?
                .ok_or_else(|| anyhow::anyhow!("CDP {method}: connection closed"))?
                .context("CDP receive")?;
            let Message::Text(text) = msg else { continue };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(|i| i.as_i64()) != Some(id) {
                continue; // an event, not our reply
            }
            let _ = ws.close(None).await;
            if let Some(err) = v.get("error") {
                bail!(
                    "CDP {method}: {}",
                    err.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("error")
                );
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Evaluate JavaScript in the page and return its value. A thrown
    /// exception becomes an error carrying the page's description of it.
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let why = ex
                .pointer("/exception/description")
                .and_then(|d| d.as_str())
                .or_else(|| ex.get("text").and_then(|t| t.as_str()))
                .unwrap_or("exception");
            bail!("page script failed: {why}");
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    /// `Page.navigate`, then wait for `document.readyState == "complete"`.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let r = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(e) = r.get("errorText").and_then(|e| e.as_str()) {
            if !e.is_empty() {
                bail!("navigation failed: {e}");
            }
        }
        self.wait_loaded().await
    }

    /// Poll `document.readyState` until `complete` or the timeout.
    pub async fn wait_loaded(&self) -> Result<()> {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.eval("document.readyState").await {
                Ok(v) if v.as_str() == Some("complete") => return Ok(()),
                Ok(_) => {}
                Err(_) if Instant::now() < deadline => {} // mid-navigation, context gone
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                bail!("page did not finish loading within {:?}", self.timeout);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// `[href, title]` of the page as it is now.
    pub async fn location(&self) -> Result<(String, String)> {
        let v = self.eval(JS_LOCATION).await?;
        let href = v
            .get(0)
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let title = v
            .get(1)
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        Ok((href, title))
    }

    /// Type text into the focused element the way a keyboard would.
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    /// Press Enter in the focused element.
    pub async fn press_enter(&self) -> Result<()> {
        for kind in ["keyDown", "keyUp"] {
            self.call(
                "Input.dispatchKeyEvent",
                json!({
                    "type": kind, "key": "Enter", "code": "Enter",
                    "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13,
                    "text": if kind == "keyDown" { "\r" } else { "" }
                }),
            )
            .await?;
        }
        Ok(())
    }
}

// ── The JavaScript the tools run in the page ─────────────────────────────────

/// `[location.href, document.title]`.
pub const JS_LOCATION: &str = "[location.href, document.title]";

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Click the first element matching `selector`; yields `"OK <tag>"` or `"NOT_FOUND"`.
pub fn js_click(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'NOT_FOUND'; \
         el.scrollIntoView({{block: 'center'}}); el.click(); return 'OK ' + el.tagName.toLowerCase(); }})()",
        sel = js_str(selector)
    )
}

/// Focus the first element matching `selector`; yields `"OK <tag>"` or `"NOT_FOUND"`.
pub fn js_focus(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'NOT_FOUND'; \
         el.scrollIntoView({{block: 'center'}}); el.focus(); \
         if ('select' in el && typeof el.select === 'function') el.select(); \
         return 'OK ' + el.tagName.toLowerCase(); }})()",
        sel = js_str(selector)
    )
}

/// Scroll the first element matching `selector` into view.
pub fn js_scroll_to(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'NOT_FOUND'; \
         el.scrollIntoView({{block: 'center'}}); return 'OK'; }})()",
        sel = js_str(selector)
    )
}

/// Scroll the window: `up`/`down` by `amount_px`, or to `top`/`bottom`.
pub fn js_scroll(direction: &str, amount_px: u64) -> String {
    match direction {
        "top" => "(() => { window.scrollTo(0, 0); return 'OK ' + window.scrollY; })()".to_string(),
        "bottom" => "(() => { window.scrollTo(0, document.body.scrollHeight); return 'OK ' + window.scrollY; })()"
            .to_string(),
        "up" => format!("(() => {{ window.scrollBy(0, -{amount_px}); return 'OK ' + window.scrollY; }})()"),
        _ => format!("(() => {{ window.scrollBy(0, {amount_px}); return 'OK ' + window.scrollY; }})()"),
    }
}

/// A compact, model-readable snapshot: title, URL, headings, the interactive
/// elements with a selector each (`#id`, `[name=…]`, or `tag:nth-of-type`),
/// then the visible text, cut at `max_chars`.
pub fn js_snapshot(max_chars: usize) -> String {
    format!(
        r#"(() => {{
  const max = {max_chars};
  const clip = (s, n) => (s || '').replace(/\s+/g, ' ').trim().slice(0, n);
  const sel = (el) => {{
    if (el.id) return '#' + CSS.escape(el.id);
    const name = el.getAttribute('name');
    if (name) return el.tagName.toLowerCase() + '[name="' + name.replace(/"/g, '\\"') + '"]';
    const p = el.parentElement; if (!p) return el.tagName.toLowerCase();
    const same = [...p.children].filter(c => c.tagName === el.tagName);
    return el.tagName.toLowerCase() + (same.length > 1 ? ':nth-of-type(' + (same.indexOf(el) + 1) + ')' : '');
  }};
  const visible = (el) => {{ const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }};
  const out = [];
  out.push('Title: ' + clip(document.title, 200));
  out.push('URL: ' + location.href);
  const hs = [...document.querySelectorAll('h1, h2, h3')].filter(visible).slice(0, 20);
  if (hs.length) out.push('', 'Headings:', ...hs.map(h => '  ' + h.tagName.toLowerCase() + ': ' + clip(h.innerText, 120)));
  const inputs = [...document.querySelectorAll('input, textarea, select')].filter(visible).slice(0, 30);
  if (inputs.length) out.push('', 'Inputs:', ...inputs.map(i => '  ' + sel(i) + '  type=' + (i.type || i.tagName.toLowerCase())
    + (i.placeholder ? ' placeholder="' + clip(i.placeholder, 60) + '"' : '')
    + (i.getAttribute('aria-label') ? ' label="' + clip(i.getAttribute('aria-label'), 60) + '"' : '')
    + (i.value ? ' value="' + clip(i.value, 40) + '"' : '')));
  const buttons = [...document.querySelectorAll('button, input[type=submit], [role=button]')].filter(visible).slice(0, 30);
  if (buttons.length) out.push('', 'Buttons:', ...buttons.map(b => '  ' + sel(b) + '  "' + clip(b.innerText || b.value || b.getAttribute('aria-label'), 60) + '"'));
  const links = [...document.querySelectorAll('a[href]')].filter(visible).slice(0, 40);
  if (links.length) out.push('', 'Links:', ...links.map(a => '  ' + sel(a) + '  "' + clip(a.innerText, 60) + '" -> ' + a.href));
  out.push('', 'Text:', clip(document.body ? document.body.innerText : '', max));
  return out.join('\n').slice(0, max + 4000);
}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_json_escaped_into_the_scripts() {
        let s = js_click("a[href=\"x\"] > span");
        assert!(
            s.contains(r#"document.querySelector("a[href=\"x\"] > span")"#),
            "{s}"
        );
        assert!(s.contains("el.click()"));
        assert!(js_focus("#q").contains("el.focus()"));
        assert!(js_scroll_to("#q").contains("scrollIntoView"));
    }

    #[test]
    fn scroll_scripts_cover_the_four_directions() {
        assert!(js_scroll("top", 0).contains("scrollTo(0, 0)"));
        assert!(js_scroll("bottom", 0).contains("scrollHeight"));
        assert!(js_scroll("up", 300).contains("scrollBy(0, -300)"));
        assert!(js_scroll("down", 500).contains("scrollBy(0, 500)"));
    }

    #[test]
    fn the_snapshot_script_lists_what_a_model_needs() {
        let s = js_snapshot(4000);
        for needle in [
            "const max = 4000",
            "Headings:",
            "Inputs:",
            "Buttons:",
            "Links:",
            "innerText",
        ] {
            assert!(s.contains(needle), "{needle}");
        }
    }

    /// Needs a Chrome with remote debugging; run with
    /// `OBC_TEST_CDP=http://127.0.0.1:9333 cargo test -p obc-tools -- --ignored cdp`.
    #[tokio::test]
    #[ignore]
    async fn drives_a_real_tab_end_to_end() {
        let Some(base) = std::env::var("OBC_TEST_CDP").ok() else {
            return;
        };
        let client = reqwest::Client::new();
        let tab: Value = client
            .put(format!("{base}/json/new?about:blank"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let ws = tab["webSocketDebuggerUrl"].as_str().unwrap().to_string();
        let id = tab["id"].as_str().unwrap().to_string();
        let cdp = Cdp::new(ws, Duration::from_secs(20));

        cdp.navigate("data:text/html,<title>Probe</title><h1>Hello</h1><input id=q placeholder=ask><button id=b onclick=\"document.title='clicked '+document.getElementById('q').value\">Go</button>")
            .await
            .unwrap();
        let (_, title) = cdp.location().await.unwrap();
        assert_eq!(title, "Probe");
        let snap = cdp.eval(&js_snapshot(2000)).await.unwrap();
        let snap = snap.as_str().unwrap();
        assert!(
            snap.contains("h1: Hello") && snap.contains("#q") && snap.contains("#b"),
            "{snap}"
        );
        assert_eq!(cdp.eval(&js_focus("#q")).await.unwrap(), "OK input");
        cdp.insert_text("hello").await.unwrap();
        assert_eq!(cdp.eval(&js_click("#b")).await.unwrap(), "OK button");
        let (_, title) = cdp.location().await.unwrap();
        assert_eq!(title, "clicked hello");
        assert_eq!(cdp.eval(&js_click("#nope")).await.unwrap(), "NOT_FOUND");
        let _ = client.get(format!("{base}/json/close/{id}")).send().await;
    }
}

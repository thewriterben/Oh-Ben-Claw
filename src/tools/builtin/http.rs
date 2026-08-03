//! HTTP request tool — make HTTP requests and return responses.

use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_conscience::{ReachDecision, ReachGate};
use reqwest::Client;
use serde_json::{json, Value};

/// Tool: make HTTP requests (GET, POST, PUT, DELETE, PATCH).
pub struct HttpTool {
    client: Client,
    /// Optional conscience reach gate. When present, the request host must be on
    /// the egress allowlist for the `http` tool or the request is refused before
    /// any connection is made (the breach lesson: containment must not rely on
    /// the model's own refusals). `None` = no egress gate (until config wires one).
    reach: Option<ReachGate>,
}

impl HttpTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            reach: None,
        }
    }

    /// Attach a conscience reach gate. Every request host must then be
    /// allowlisted for the `http` tool, or the request is refused pre-connection.
    pub fn with_reach_gate(mut self, gate: ReachGate) -> Self {
        self.reach = Some(gate);
        self
    }

    /// Egress check for a URL. `Ok(())` if allowed (or no gate); `Err(reason)`
    /// if the reach gate refuses. Extracted for testability without a network.
    fn check_reach(&self, url: &str) -> Result<(), String> {
        let Some(gate) = &self.reach else {
            return Ok(()); // no gate configured
        };
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        match gate.check("http", &host) {
            ReachDecision::Allow { .. } => Ok(()),
            ReachDecision::Refuse(reason) => Err(reason.to_string()),
        }
    }
}

impl Default for HttpTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpTool {
    fn name(&self) -> &str {
        "http"
    }

    fn output_trust(&self) -> crate::tools::traits::OutputTrust {
        // Fetches arbitrary web content — a prompt-injection vector (Track 0).
        crate::tools::traits::OutputTrust::External
    }

    fn description(&self) -> &str {
        "Make an HTTP request to a URL and return the response body. \
         Supports GET, POST, PUT, DELETE, and PATCH methods. \
         Can send JSON bodies and custom headers. \
         Use this to call APIs, fetch web pages, or interact with web services."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "description": "HTTP method (default: GET).",
                    "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"],
                    "default": "GET"
                },
                "url": {
                    "type": "string",
                    "description": "The URL to request."
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs.",
                    "additionalProperties": {"type": "string"}
                },
                "body": {
                    "description": "Optional request body (string or JSON object)."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds (default: 30, max: 120).",
                    "default": 30,
                    "minimum": 1,
                    "maximum": 120
                }
            },
            "required": ["url"]
        })
    }

    fn risk_class(&self) -> RiskClass {
        // The HTTP tool supports POST/PUT/DELETE/PATCH, so it can mutate remote
        // state and isn't safely re-runnable. The self-improvement loop must
        // quarantine learned skills that use it rather than verify by replay.
        // Not `physical`, so the Track 0 agent gate is unaffected.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))?
            .to_string();

        // Conscience reach gate (Track 0 for egress): refuse a host that is not
        // allowlisted BEFORE any connection is made. Logged, not silent.
        if let Err(reason) = self.check_reach(&url) {
            tracing::warn!(url = %url, refusal = %reason, "conscience: egress refused");
            return Ok(ToolResult::err(format!("conscience: {reason}")));
        }

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .clamp(1, 120);

        tracing::debug!(method = %method, url = %url, "Making HTTP request");

        let mut request = match method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "DELETE" => self.client.delete(&url),
            "PATCH" => self.client.patch(&url),
            other => {
                return Ok(ToolResult::err(format!(
                    "Unsupported HTTP method: {}",
                    other
                )))
            }
        };

        // Add custom headers
        if let Some(headers) = args.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(val_str) = value.as_str() {
                    request = request.header(key, val_str);
                }
            }
        }

        // Add body
        if let Some(body) = args.get("body") {
            match body {
                Value::String(s) => {
                    request = request.body(s.clone());
                }
                Value::Object(_) | Value::Array(_) => {
                    request = request.json(body);
                }
                _ => {}
            }
        }

        // Set timeout
        request = request.timeout(std::time::Duration::from_secs(timeout_secs));

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("Request failed: {}", e))),
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status.is_success() {
            Ok(ToolResult::ok(format!(
                "HTTP {} {}\n{}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                body
            )))
        } else {
            Ok(ToolResult {
                success: false,
                output: format!(
                    "HTTP {} {}\n{}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or(""),
                    body
                ),
                error: Some(format!("HTTP error: {}", status)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_tool_has_correct_name() {
        let tool = HttpTool::new();
        assert_eq!(tool.name(), "http");
    }

    #[tokio::test]
    async fn http_missing_url_returns_error() {
        let tool = HttpTool::new();
        let result = tool.execute(json!({"method": "GET"})).await;
        assert!(result.is_err());
    }

    // -- conscience reach gate at the egress boundary --

    fn gate() -> ReachGate {
        use obc_conscience::{HostRule, ReachScope, ToolReach};
        ReachGate::new(
            vec![HostRule {
                host: "api.allowed.example".into(),
                purpose: "test".into(),
                credential: None,
            }],
            vec![ToolReach { tool: "http".into(), scope: ReachScope::Egress }],
        )
    }

    #[test]
    fn reach_allows_allowlisted_host() {
        let tool = HttpTool::new().with_reach_gate(gate());
        assert!(tool.check_reach("https://api.allowed.example/v1/thing").is_ok());
    }

    #[test]
    fn reach_refuses_unlisted_host() {
        let tool = HttpTool::new().with_reach_gate(gate());
        assert!(tool.check_reach("https://evil.example.com/exfil").is_err());
    }

    #[tokio::test]
    async fn execute_refuses_unlisted_host_without_connecting() {
        // No network is touched: the gate refuses before send().
        let tool = HttpTool::new().with_reach_gate(gate());
        let result = tool
            .execute(json!({"url": "https://evil.example.com/exfil"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("conscience"));
    }

    #[test]
    fn no_gate_allows_any_host() {
        let tool = HttpTool::new(); // ungated
        assert!(tool.check_reach("https://anywhere.example").is_ok());
    }
}

//! HTTP request tool — make HTTP requests and return responses.

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Tool: make HTTP requests (GET, POST, PUT, DELETE, PATCH).
pub struct HttpTool {
    client: Client,
}

impl HttpTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
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
            other => return Ok(ToolResult::err(format!("Unsupported HTTP method: {}", other))),
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
            Ok(ToolResult::ok(format!("HTTP {} {}\n{}", status.as_u16(), status.canonical_reason().unwrap_or(""), body)))
        } else {
            Ok(ToolResult {
                success: false,
                output: format!("HTTP {} {}\n{}", status.as_u16(), status.canonical_reason().unwrap_or(""), body),
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
}

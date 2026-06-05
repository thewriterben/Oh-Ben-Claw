//! MCP client — connects to external MCP servers via stdio or HTTP+SSE.
//!
//! Supports both protocol lifecycles (Phase 15, WS2):
//! - **Legacy 2024-11-05**: `initialize`/`initialized` handshake on connect.
//! - **Stateless 2026-07-28**: no handshake; `clientInfo` rides in `_meta` on
//!   every request, capabilities come from `server/discover`, and HTTP
//!   requests carry `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` headers.

use super::{
    client_info_meta, JsonRpcRequest, JsonRpcResponse, McpServerConfig, McpToolDef, ProtocolMode,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

// ── Transport ─────────────────────────────────────────────────────────────────

enum Transport {
    Stdio(Box<StdioTransport>),
    Http(HttpTransport),
}

struct StdioTransport {
    stdin: ChildStdin,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    _child: Child,
}

struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

// ── MCP Client ────────────────────────────────────────────────────────────────

/// A client for communicating with an MCP server.
pub struct McpClient {
    transport: Transport,
    next_id: u64,
    mode: ProtocolMode,
    server_name: String,
    server_version: String,
    /// `ttlMs` from the most recent `tools/list` response (2026 spec, SEP-2549).
    tools_ttl_ms: Option<u64>,
}

impl McpClient {
    /// Connect to an MCP server using the given configuration.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        match config.transport.as_str() {
            "stdio" => Self::connect_stdio(config).await,
            "http" => Self::connect_http(config).await,
            t => anyhow::bail!("Unknown MCP transport: {t}"),
        }
    }

    async fn connect_stdio(config: &McpServerConfig) -> Result<Self> {
        let command = config
            .command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("stdio transport requires 'command'"))?;

        let args = config.args.as_deref().unwrap_or(&[]);

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(env) = &config.env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("No stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("No stdout"))?;

        let mut client = Self {
            transport: Transport::Stdio(Box::new(StdioTransport {
                stdin,
                stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
                _child: child,
            })),
            next_id: 1,
            mode: config.protocol_mode,
            server_name: String::new(),
            server_version: String::new(),
            tools_ttl_ms: None,
        };

        client.establish().await?;
        Ok(client)
    }

    async fn connect_http(config: &McpServerConfig) -> Result<Self> {
        let base_url = config
            .url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("http transport requires 'url'"))?;

        let mut client = Self {
            transport: Transport::Http(HttpTransport {
                client: reqwest::Client::new(),
                base_url,
                token: config.token.clone(),
            }),
            next_id: 1,
            mode: config.protocol_mode,
            server_name: String::new(),
            server_version: String::new(),
            tools_ttl_ms: None,
        };

        client.establish().await?;
        Ok(client)
    }

    /// Establish the connection according to the protocol mode.
    async fn establish(&mut self) -> Result<()> {
        match self.mode {
            ProtocolMode::Legacy2024 => self.initialize().await,
            ProtocolMode::Stateless2026 => self.discover().await,
        }
    }

    /// Perform the legacy (2024-11-05) MCP initialize handshake.
    ///
    /// Note: `roots` and `sampling` capability declarations were dropped —
    /// both features are deprecated in 2026-07-28 (SEP-2577) and this client
    /// never implemented either.
    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": super::PROTOCOL_VERSION_LEGACY,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "oh-ben-claw",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;

        self.record_server_info(&result);

        // Send initialized notification
        self.notify("notifications/initialized", json!({})).await?;

        tracing::debug!(
            "MCP handshake complete: {} v{}",
            self.server_name,
            self.server_version
        );
        Ok(())
    }

    /// 2026-07-28 mode: no handshake. Optionally fetch server capabilities
    /// via `server/discover`; tolerate servers that don't implement it,
    /// since discovery is on-demand rather than a lifecycle requirement.
    async fn discover(&mut self) -> Result<()> {
        match self.request("server/discover", json!({})).await {
            Ok(result) => {
                self.record_server_info(&result);
                tracing::debug!(
                    "MCP server/discover: {} v{}",
                    self.server_name,
                    self.server_version
                );
            }
            Err(e) => {
                self.server_name = "unknown".to_string();
                self.server_version = "0.0.0".to_string();
                tracing::debug!("MCP server/discover unavailable (continuing): {e}");
            }
        }
        Ok(())
    }

    fn record_server_info(&mut self, result: &Value) {
        self.server_name = result["serverInfo"]["name"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        self.server_version = result["serverInfo"]["version"]
            .as_str()
            .unwrap_or("0.0.0")
            .to_string();
    }

    /// List all tools available on the connected server.
    ///
    /// In 2026 mode the response may carry `ttlMs` (SEP-2549); it is recorded
    /// and exposed via [`Self::tools_ttl_ms`] so callers can cache the list.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>> {
        let result = self.request("tools/list", json!({})).await?;
        self.tools_ttl_ms = result["ttlMs"].as_u64();
        let tools: Vec<McpToolDef> =
            serde_json::from_value(result["tools"].clone()).unwrap_or_default();
        Ok(tools)
    }

    /// `ttlMs` from the most recent `tools/list` response, if the server
    /// provided one (2026-07-28 spec). `None` means "do not cache".
    pub fn tools_ttl_ms(&self) -> Option<u64> {
        self.tools_ttl_ms
    }

    /// Call a tool on the connected server.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments
                }),
            )
            .await?;

        // Extract text content from the MCP result
        let content = result["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["text"].as_str())
            .unwrap_or("")
            .to_string();

        let is_error = result["isError"].as_bool().unwrap_or(false);
        if is_error {
            anyhow::bail!("MCP tool returned error: {content}");
        }

        Ok(content)
    }

    /// Send a JSON-RPC request and await the response.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        // 2026 mode: clientInfo travels in `_meta` on every request (SEP-2575).
        let params = if self.mode == ProtocolMode::Stateless2026 {
            Self::with_client_meta(params)
        } else {
            params
        };

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: method.to_string(),
            params,
        };

        match &mut self.transport {
            Transport::Stdio(t) => {
                let mut line = serde_json::to_string(&req)?;
                line.push('\n');
                t.stdin.write_all(line.as_bytes()).await?;
                t.stdin.flush().await?;

                let stdout = t.stdout.clone();
                let mut guard = stdout.lock().await;
                let mut response_line = String::new();
                guard.read_line(&mut response_line).await?;

                let resp: JsonRpcResponse = serde_json::from_str(response_line.trim())?;
                if let Some(err) = resp.error {
                    anyhow::bail!("MCP error {}: {}", err.code, err.message);
                }
                Ok(resp.result.unwrap_or(Value::Null))
            }
            Transport::Http(t) => {
                let url = format!("{}/mcp", t.base_url);
                let mut builder = t.client.post(&url).json(&req);
                if let Some(token) = &t.token {
                    builder = builder.bearer_auth(token);
                }
                // 2026 Streamable HTTP requires routing headers (SEP-2243).
                if self.mode == ProtocolMode::Stateless2026 {
                    builder = builder
                        .header("MCP-Protocol-Version", self.mode.version())
                        .header("Mcp-Method", &req.method);
                    if let Some(name) = req.params.get("name").and_then(|n| n.as_str()) {
                        builder = builder.header("Mcp-Name", name);
                    }
                }
                let resp: JsonRpcResponse = builder.send().await?.json().await?;
                if let Some(err) = resp.error {
                    anyhow::bail!("MCP error {}: {}", err.code, err.message);
                }
                Ok(resp.result.unwrap_or(Value::Null))
            }
        }
    }

    /// Merge the spec-defined clientInfo `_meta` entry into request params,
    /// preserving any `_meta` keys the caller already set.
    fn with_client_meta(mut params: Value) -> Value {
        if !params.is_object() {
            return params;
        }
        let meta_addition = client_info_meta();
        let obj = params.as_object_mut().expect("checked is_object above");
        match obj.get_mut("_meta") {
            Some(Value::Object(existing)) => {
                if let Value::Object(add) = meta_addition {
                    for (k, v) in add {
                        existing.entry(k).or_insert(v);
                    }
                }
            }
            _ => {
                obj.insert("_meta".to_string(), meta_addition);
            }
        }
        params
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params,
        };

        match &mut self.transport {
            Transport::Stdio(t) => {
                let mut line = serde_json::to_string(&req)?;
                line.push('\n');
                t.stdin.write_all(line.as_bytes()).await?;
                t.stdin.flush().await?;
            }
            Transport::Http(_) => {
                // HTTP transport doesn't need notifications
            }
        }
        Ok(())
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// The protocol mode this client speaks.
    pub fn mode(&self) -> ProtocolMode {
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_client_meta_adds_meta_to_plain_params() {
        let params = json!({"name": "search", "arguments": {"q": "otters"}});
        let out = McpClient::with_client_meta(params);
        let info = &out["_meta"]["io.modelcontextprotocol/clientInfo"];
        assert_eq!(info["name"], "oh-ben-claw");
        assert!(info["version"].is_string());
        // Original params preserved.
        assert_eq!(out["name"], "search");
    }

    #[test]
    fn with_client_meta_preserves_existing_meta_keys() {
        let params = json!({
            "_meta": {"traceparent": "00-abc-def-01", "io.modelcontextprotocol/clientInfo": {"name": "custom"}}
        });
        let out = McpClient::with_client_meta(params);
        // Caller-set keys win; we only fill in what's missing.
        assert_eq!(out["_meta"]["traceparent"], "00-abc-def-01");
        assert_eq!(
            out["_meta"]["io.modelcontextprotocol/clientInfo"]["name"],
            "custom"
        );
    }

    #[test]
    fn with_client_meta_passes_non_object_params_through() {
        let params = json!([1, 2, 3]);
        let out = McpClient::with_client_meta(params.clone());
        assert_eq!(out, params);
    }

    #[test]
    fn protocol_mode_versions() {
        assert_eq!(ProtocolMode::Legacy2024.version(), "2024-11-05");
        assert_eq!(ProtocolMode::Stateless2026.version(), "2026-07-28");
        assert_eq!(ProtocolMode::default(), ProtocolMode::Legacy2024);
    }

    #[test]
    fn protocol_mode_serde_kebab_case() {
        let m: ProtocolMode = serde_json::from_str("\"stateless-2026\"").unwrap();
        assert_eq!(m, ProtocolMode::Stateless2026);
        let s = serde_json::to_string(&ProtocolMode::Legacy2024).unwrap();
        assert_eq!(s, "\"legacy-2024\"");
    }
}

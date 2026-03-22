//! MCP client — connects to external MCP servers via stdio or HTTP+SSE.

use super::{JsonRpcRequest, JsonRpcResponse, McpServerConfig, McpToolDef};
use anyhow::Result;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

// ── Transport ─────────────────────────────────────────────────────────────────

enum Transport {
    Stdio(StdioTransport),
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
    server_name: String,
    server_version: String,
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
            transport: Transport::Stdio(StdioTransport {
                stdin,
                stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
                _child: child,
            }),
            next_id: 1,
            server_name: String::new(),
            server_version: String::new(),
        };

        // Perform MCP handshake
        client.initialize().await?;
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
            server_name: String::new(),
            server_version: String::new(),
        };

        client.initialize().await?;
        Ok(client)
    }

    /// Perform the MCP initialize handshake.
    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "roots": {"listChanged": false},
                        "sampling": {}
                    },
                    "clientInfo": {
                        "name": "oh-ben-claw",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;

        self.server_name = result["serverInfo"]["name"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        self.server_version = result["serverInfo"]["version"]
            .as_str()
            .unwrap_or("0.0.0")
            .to_string();

        // Send initialized notification
        self.notify("notifications/initialized", json!({})).await?;

        tracing::debug!(
            "MCP handshake complete: {} v{}",
            self.server_name,
            self.server_version
        );
        Ok(())
    }

    /// List all tools available on the connected server.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools: Vec<McpToolDef> =
            serde_json::from_value(result["tools"].clone()).unwrap_or_default();
        Ok(tools)
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
                let resp: JsonRpcResponse = builder.send().await?.json().await?;
                if let Some(err) = resp.error {
                    anyhow::bail!("MCP error {}: {}", err.code, err.message);
                }
                Ok(resp.result.unwrap_or(Value::Null))
            }
        }
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
}

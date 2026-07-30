//! Model Context Protocol (MCP) server and client.
//!
//! Oh-Ben-Claw can act as both an MCP **server** (exposing its tools to any
//! MCP-compatible host like Claude Desktop, Cursor, or VS Code) and an MCP
//! **client** (connecting to external MCP servers to consume their tools).
//!
//! ## Protocol
//! MCP uses JSON-RPC 2.0 over stdio (for local processes) or HTTP+SSE
//! (for remote servers). This implementation supports both transports.
//!
//! ## References
//! - MCP Spec: <https://spec.modelcontextprotocol.io>
//! - Rust SDK: `rmcp` crate (Linux Foundation project, v0.16+)

use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub mod client;
pub mod server;

// ── Protocol Mode (Phase 15, WS2) ─────────────────────────────────────────────

/// MCP protocol version string for the legacy (2024-11-05) lifecycle.
pub const PROTOCOL_VERSION_LEGACY: &str = "2024-11-05";
/// MCP protocol version string for the stateless 2026-07-28 specification.
pub const PROTOCOL_VERSION_2026: &str = "2026-07-28";
/// Every published protocol revision this implementation accepts on the wire.
/// All pre-2026 revisions share the handshake lifecycle the bilingual server
/// speaks, so they are all valid values for `MCP-Protocol-Version` and for a
/// legacy client's `initialize.params.protocolVersion`.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    PROTOCOL_VERSION_2026,
];

/// Which MCP protocol lifecycle to speak.
///
/// The 2026-07-28 specification removes the `initialize`/`initialized`
/// handshake and the protocol-level session: `protocolVersion` and
/// `clientInfo` travel in `_meta` on every request, capabilities are fetched
/// on demand via `server/discover`, and Streamable HTTP requests must carry
/// `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` headers.
///
/// Servers built from this module are **bilingual** regardless of mode: they
/// answer `initialize` for legacy clients and `server/discover` for 2026
/// clients. The mode primarily drives client behaviour and HTTP strictness.
///
/// `Default` is the mode a *negotiating* client tries **first**, not a mode it
/// commits to. It became `Stateless2026` on 2026-07-30 — the Phase 15 flip
/// scheduled for July 28 — which was only safe once
/// [`McpClient`](client::McpClient) could fall back. Flipping it without a
/// fallback would have connected successfully to every legacy server and failed
/// at the first `tools/call`, because `server/discover` is optional and its
/// absence proves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolMode {
    /// Pre-2026 lifecycle: initialize handshake, no required HTTP headers.
    #[serde(rename = "legacy-2024")]
    Legacy2024,
    /// 2026-07-28 stateless lifecycle.
    #[default]
    #[serde(rename = "stateless-2026")]
    Stateless2026,
}

/// How a connected client arrived at the protocol lifecycle it is speaking.
///
/// Exposed because "which protocol is this connection using" stopped being a
/// property of the config the moment negotiation existed, and an operator
/// debugging a server needs to know whether they are looking at their own
/// choice or at a fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSource {
    /// The config named a mode. No probe was sent and no fallback was tried —
    /// pinning is a statement of intent and quietly doing something else would
    /// be the same class of bug as the flip this replaced.
    Pinned,
    /// Negotiated, and the preferred mode answered.
    Preferred,
    /// Negotiated, the preferred mode did not answer, and this is the fallback.
    Fallback,
}

impl ProtocolMode {
    /// The protocol version string this mode advertises.
    pub fn version(&self) -> &'static str {
        match self {
            Self::Legacy2024 => PROTOCOL_VERSION_LEGACY,
            Self::Stateless2026 => PROTOCOL_VERSION_2026,
        }
    }
}

/// Build the `_meta` object that 2026-mode clients attach to every request.
///
/// Key name is fixed by the specification: `io.modelcontextprotocol/clientInfo`.
pub fn client_info_meta() -> Value {
    json!({
        "io.modelcontextprotocol/clientInfo": {
            "name": "oh-ben-claw",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

// ── MCP Data Types ────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// Absent on notifications, and **omitted from the wire** when absent.
    ///
    /// Without `skip_serializing_if` this serialised as `"id": null`, which
    /// JSON-RPC 2.0 does not treat as a notification — a notification is defined
    /// by the *absence* of the member, and a request with a null id is malformed
    /// rather than fire-and-forget. Servers strict enough to say so replied with
    /// an error to `notifications/initialized`, and since the client does not
    /// read after a notify, that error sat in the pipe and was consumed as the
    /// response to the *next* request. Every subsequent call was then answering
    /// the previous question.
    ///
    /// Found on 2026-07-30 by `tests/mcp_protocol_negotiation.rs`, which fails
    /// this exact way: the tool call after a legacy fallback returned
    /// "no such method: notifications/initialized". It had been latent since the
    /// handshake was written, invisible because every server tested against
    /// happened to ignore the malformed id.
    /// `default` is not decoration: serde does **not** treat a missing
    /// `Option<T>` as `None` on the way in, so omitting the member on the way
    /// out without this makes every notification we send un-parseable by our own
    /// server — which is exactly what happened, and what
    /// `test_http_notification_gets_202_and_no_body` caught by turning 202 into
    /// 400. The two attributes are one change; either alone is a bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// MCP tool definition (as returned by `tools/list`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

impl McpToolDef {
    /// Build an MCP tool definition from a `Tool` trait object.
    pub fn from_tool(tool: &dyn Tool) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.parameters_schema(),
        }
    }
}

/// MCP tool call result content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

// ── MCP Remote Tool ───────────────────────────────────────────────────────────

/// A `Tool` implementation that proxies calls to a remote MCP server.
pub struct McpRemoteTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
    /// Shared MCP client connection.
    pub client: Arc<Mutex<client::McpClient>>,
}

#[async_trait]
impl Tool for McpRemoteTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    fn output_trust(&self) -> crate::tools::traits::OutputTrust {
        // Output comes from a remote MCP server outside the trust boundary
        // (Track 0 taint tracking).
        crate::tools::traits::OutputTrust::External
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let mut client = self.client.lock().await;
        match client.call_tool(&self.name, args).await {
            Ok(result) => Ok(ToolResult::ok(result)),
            Err(e) => Ok(ToolResult::err(format!("MCP tool call failed: {e}"))),
        }
    }
}

// ── MCP Tool Registry ─────────────────────────────────────────────────────────

/// Registry of all MCP server connections and their tools.
pub struct McpRegistry {
    /// Map from server name → client
    clients: HashMap<String, Arc<Mutex<client::McpClient>>>,
    /// Map from tool name → (server name, tool def)
    tools: HashMap<String, (String, McpToolDef)>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            tools: HashMap::new(),
        }
    }

    /// Connect to an MCP server and register all its tools.
    pub async fn connect(&mut self, name: &str, config: &McpServerConfig) -> Result<usize> {
        let mut client = client::McpClient::connect(config).await?;
        let tool_defs = client.list_tools().await?;
        let count = tool_defs.len();

        let client_arc = Arc::new(Mutex::new(client));
        self.clients.insert(name.to_string(), client_arc.clone());

        for tool_def in tool_defs {
            self.tools
                .insert(tool_def.name.clone(), (name.to_string(), tool_def));
        }

        tracing::info!("Connected to MCP server '{}' with {} tools", name, count);
        Ok(count)
    }

    /// Build `Box<dyn Tool>` instances for all registered MCP tools.
    pub fn build_tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .filter_map(|(tool_name, (server_name, tool_def))| {
                self.clients.get(server_name).map(|client| {
                    Box::new(McpRemoteTool {
                        name: tool_name.clone(),
                        description: tool_def.description.clone(),
                        schema: tool_def.input_schema.clone(),
                        client: client.clone(),
                    }) as Box<dyn Tool>
                })
            })
            .collect()
    }

    /// Return a shared handle to a connected server's client, if present.
    ///
    /// Lets callers reuse the live connection for out-of-band polling — e.g. a
    /// perception loop that pulls ClawCam detections into world memory via
    /// [`crate::vision::clawcam_ingest::poll_clawcam_into_world`].
    pub fn client(&self, server_name: &str) -> Option<Arc<Mutex<client::McpClient>>> {
        self.clients.get(server_name).cloned()
    }

    /// List all registered tools with their server names.
    pub fn list_tools(&self) -> Vec<(String, String, String)> {
        self.tools
            .iter()
            .map(|(name, (server, def))| (name.clone(), server.clone(), def.description.clone()))
            .collect()
    }

    /// Disconnect from all servers.
    pub async fn disconnect_all(&mut self) {
        self.clients.clear();
        self.tools.clear();
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Transport type: "stdio" or "http"
    pub transport: String,
    /// For stdio: command to spawn (e.g. "npx @modelcontextprotocol/server-filesystem")
    pub command: Option<String>,
    /// For stdio: arguments to pass to the command
    pub args: Option<Vec<String>>,
    /// For http: the base URL of the MCP server
    pub url: Option<String>,
    /// Optional bearer token for HTTP transport
    pub token: Option<String>,
    /// Environment variables to set for stdio processes
    pub env: Option<HashMap<String, String>>,
    /// Protocol lifecycle to speak: `"legacy-2024"`, `"stateless-2026"`, or
    /// **omitted to negotiate**, which is the default and the right answer for
    /// almost everyone.
    ///
    /// Omitted, the client probes for the 2026-07-28 lifecycle and falls back to
    /// the legacy handshake when the server does not answer it. Set explicitly,
    /// the client speaks exactly that and does not fall back — pinning exists so
    /// that a server known to misbehave under negotiation can be nailed down,
    /// and a pin that silently did something else would defeat the purpose.
    #[serde(default)]
    pub protocol_mode: Option<ProtocolMode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonrpc_response_ok() {
        let resp = JsonRpcResponse::ok(Some(json!(1)), json!({"result": "ok"}));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert_eq!(resp.jsonrpc, "2.0");
    }

    #[test]
    fn test_jsonrpc_response_err() {
        let resp = JsonRpcResponse::err(Some(json!(1)), -32600, "Invalid Request");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[test]
    fn test_mcp_registry_new() {
        let registry = McpRegistry::new();
        assert!(registry.list_tools().is_empty());
        assert!(registry.build_tools().is_empty());
    }
}

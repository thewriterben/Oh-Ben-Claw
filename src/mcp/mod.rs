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

// ── MCP Data Types ────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
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
            self.tools.insert(
                tool_def.name.clone(),
                (name.to_string(), tool_def),
            );
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

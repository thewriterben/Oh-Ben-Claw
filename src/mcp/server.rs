//! MCP server — exposes Oh-Ben-Claw tools to external MCP hosts.
//!
//! Supports both stdio (for local process integration with Claude Desktop,
//! Cursor, VS Code, etc.) and HTTP+SSE transports.

use super::{JsonRpcRequest, JsonRpcResponse, McpContent, McpToolDef};
use crate::tools::Tool;
use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

// ── MCP Server ────────────────────────────────────────────────────────────────

/// An MCP server that exposes Oh-Ben-Claw tools.
pub struct McpServer {
    tools: HashMap<String, Arc<dyn Tool>>,
    server_name: String,
    server_version: String,
}

impl McpServer {
    /// Create a new MCP server with the given tools.
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        let mut tool_map = HashMap::new();
        for tool in tools {
            tool_map.insert(tool.name().to_string(), Arc::from(tool));
        }
        Self {
            tools: tool_map,
            server_name: "oh-ben-claw".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Run the MCP server over stdio (for local process integration).
    pub async fn run_stdio(self) -> Result<()> {
        let server = Arc::new(Mutex::new(self));
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);
        let mut writer = tokio::io::BufWriter::new(stdout);

        tracing::info!("MCP server running on stdio");

        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break; // EOF
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(e) => {
                    let resp = JsonRpcResponse::err(None, -32700, &format!("Parse error: {e}"));
                    let mut out = serde_json::to_string(&resp)?;
                    out.push('\n');
                    writer.write_all(out.as_bytes()).await?;
                    writer.flush().await?;
                    continue;
                }
            };

            let id = request.id.clone();
            let server_clone = server.clone();
            let response = {
                let mut srv = server_clone.lock().await;
                srv.handle_request(request).await
            };

            let mut out = serde_json::to_string(&response)?;
            out.push('\n');
            writer.write_all(out.as_bytes()).await?;
            writer.flush().await?;
        }

        Ok(())
    }

    /// Build an Axum router for HTTP transport.
    pub fn http_router(self) -> Router {
        let server = Arc::new(Mutex::new(self));
        Router::new()
            .route("/mcp", post(http_handler))
            .with_state(server)
    }

    /// Handle a single JSON-RPC request.
    async fn handle_request(&mut self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();
        match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "notifications/initialized" => {
                // No response for notifications
                JsonRpcResponse::ok(None, Value::Null)
            }
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            "ping" => JsonRpcResponse::ok(id, json!({})),
            method => {
                JsonRpcResponse::err(id, -32601, &format!("Method not found: {method}"))
            }
        }
    }

    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {"listChanged": false}
                },
                "serverInfo": {
                    "name": self.server_name,
                    "version": self.server_version
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools: Vec<McpToolDef> = self
            .tools
            .values()
            .map(|t| McpToolDef::from_tool(t.as_ref()))
            .collect();
        JsonRpcResponse::ok(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, id: Option<Value>, params: Value) -> JsonRpcResponse {
        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return JsonRpcResponse::err(id, -32602, "Missing 'name' parameter");
            }
        };

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let tool = match self.tools.get(&name) {
            Some(t) => t.clone(),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    &format!("Tool not found: {name}"),
                );
            }
        };

        let tool_result = match tool.execute(arguments).await {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::err(id, -32603, &format!("Tool execution error: {e}"));
            }
        };
        let is_error = !tool_result.success;
        let text = if tool_result.success {
            tool_result.output.clone()
        } else {
            tool_result.error.clone().unwrap_or_else(|| tool_result.output.clone())
        };
        let content = vec![McpContent {
            content_type: "text".to_string(),
            text,
        }];

        JsonRpcResponse::ok(
            id,
            json!({
                "content": content,
                "isError": is_error
            }),
        )
    }

    /// Get the number of tools registered.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

// ── HTTP Handler ──────────────────────────────────────────────────────────────

async fn http_handler(
    State(server): State<Arc<Mutex<McpServer>>>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let mut srv = server.lock().await;
    let response = srv.handle_request(request).await;
    (StatusCode::OK, Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::builtin::shell::ShellTool;

    fn make_server() -> McpServer {
        McpServer::new(vec![Box::new(ShellTool::new())])
    }

    #[tokio::test]
    async fn test_initialize() {
        let mut server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: json!({
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "test", "version": "1.0"}
            }),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "oh-ben-claw");
        assert_eq!(result["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn test_tools_list() {
        let mut server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/list".to_string(),
            params: json!({}),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let tools = resp.result.unwrap()["tools"].clone();
        assert!(tools.as_array().unwrap().len() >= 1);
    }

    #[tokio::test]
    async fn test_tools_call_not_found() {
        let mut server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: json!({"name": "nonexistent_tool", "arguments": {}}),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn test_ping() {
        let mut server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "ping".to_string(),
            params: json!({}),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let mut server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "unknown/method".to_string(),
            params: json!({}),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_server_tool_count() {
        let server = make_server();
        assert_eq!(server.tool_count(), 1);
    }
}

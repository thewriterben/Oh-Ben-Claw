//! MCP server — exposes Oh-Ben-Claw tools to external MCP hosts.
//!
//! Supports both stdio (for local process integration with Claude Desktop,
//! Cursor, VS Code, etc.) and HTTP+SSE transports.

use super::{JsonRpcRequest, JsonRpcResponse, McpContent, McpToolDef, ProtocolMode};
use crate::tools::Tool;
use anyhow::Result;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
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

/// How long clients may cache `tools/list` responses (SEP-2549).
const TOOLS_LIST_TTL_MS: u64 = 60_000;

/// An MCP server that exposes Oh-Ben-Claw tools.
///
/// The server is **bilingual** (Phase 15, WS2): it answers the legacy
/// `initialize` handshake for 2024-11-05 clients and `server/discover` for
/// 2026-07-28 clients, and accepts handshake-less requests (it never held
/// per-connection state). The configured [`ProtocolMode`] controls HTTP
/// strictness: in `Stateless2026` mode, HTTP requests must carry the
/// `MCP-Protocol-Version` and `Mcp-Method` routing headers (SEP-2243).
pub struct McpServer {
    tools: HashMap<String, Arc<dyn Tool>>,
    server_name: String,
    server_version: String,
    mode: ProtocolMode,
}

impl McpServer {
    /// Create a new MCP server with the given tools (legacy-compatible mode).
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self::with_mode(tools, ProtocolMode::default())
    }

    /// Create a new MCP server with an explicit protocol mode.
    pub fn with_mode(tools: Vec<Box<dyn Tool>>, mode: ProtocolMode) -> Self {
        let mut tool_map = HashMap::new();
        for tool in tools {
            tool_map.insert(tool.name().to_string(), Arc::from(tool));
        }
        Self {
            tools: tool_map,
            server_name: "oh-ben-claw".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            mode,
        }
    }

    /// The protocol mode this server enforces on HTTP transport.
    pub fn mode(&self) -> ProtocolMode {
        self.mode
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
    ///
    /// Bilingual dispatch: `initialize` (legacy clients) and `server/discover`
    /// (2026 clients) are both always available, and no method requires a
    /// prior handshake. Public so embedders and the eval harness can drive
    /// the server without a transport.
    pub async fn handle_request(&mut self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();
        match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "server/discover" => self.handle_discover(id),
            "notifications/initialized" => {
                // No response for notifications
                JsonRpcResponse::ok(None, Value::Null)
            }
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            "ping" => JsonRpcResponse::ok(id, json!({})),
            method => JsonRpcResponse::err(id, -32601, &format!("Method not found: {method}")),
        }
    }

    /// Legacy 2024-11-05 handshake response.
    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": super::PROTOCOL_VERSION_LEGACY,
                "capabilities": self.capabilities(),
                "serverInfo": self.server_info()
            }),
        )
    }

    /// 2026-07-28 on-demand capability discovery (SEP-2575).
    ///
    /// Shape mirrors the initialize result; revalidate against the final
    /// specification when it ships on 2026-07-28 (Phase 15 work item 8).
    fn handle_discover(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": super::PROTOCOL_VERSION_2026,
                "capabilities": self.capabilities(),
                "serverInfo": self.server_info()
            }),
        )
    }

    fn capabilities(&self) -> Value {
        json!({
            "tools": {"listChanged": false}
        })
    }

    fn server_info(&self) -> Value {
        json!({
            "name": self.server_name,
            "version": self.server_version
        })
    }

    fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools: Vec<McpToolDef> = self
            .tools
            .values()
            .map(|t| McpToolDef::from_tool(t.as_ref()))
            .collect();
        // ttlMs/cacheScope are additive (SEP-2549); legacy clients ignore them.
        JsonRpcResponse::ok(
            id,
            json!({
                "tools": tools,
                "ttlMs": TOOLS_LIST_TTL_MS,
                "cacheScope": "private"
            }),
        )
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
                return JsonRpcResponse::err(id, -32602, &format!("Tool not found: {name}"));
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
            tool_result
                .error
                .clone()
                .unwrap_or_else(|| tool_result.output.clone())
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

/// Validate 2026-07-28 Streamable HTTP routing headers (SEP-2243).
///
/// Rules:
/// - If `Mcp-Method` is present it must match the JSON-RPC body method.
/// - If `Mcp-Name` is present it must match `params.name`.
/// - If `MCP-Protocol-Version` is present it must be a version we speak.
/// - In `Stateless2026` mode, `MCP-Protocol-Version` and `Mcp-Method` are
///   **required**; in legacy mode they are validated only when present.
fn validate_http_headers(
    headers: &HeaderMap,
    request: &JsonRpcRequest,
    mode: ProtocolMode,
) -> Result<(), String> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };

    let version = get("mcp-protocol-version");
    let method = get("mcp-method");
    let name = get("mcp-name");

    if let Some(v) = &version {
        if v != super::PROTOCOL_VERSION_LEGACY && v != super::PROTOCOL_VERSION_2026 {
            return Err(format!("Unsupported MCP-Protocol-Version: {v}"));
        }
    }
    if let Some(m) = &method {
        if *m != request.method {
            return Err(format!(
                "Mcp-Method header '{m}' does not match body method '{}'",
                request.method
            ));
        }
    }
    if let Some(n) = &name {
        let body_name = request.params.get("name").and_then(|v| v.as_str());
        if body_name != Some(n.as_str()) {
            return Err(format!(
                "Mcp-Name header '{n}' does not match body params.name '{}'",
                body_name.unwrap_or("<absent>")
            ));
        }
    }
    if mode == ProtocolMode::Stateless2026 {
        if version.is_none() {
            return Err("MCP-Protocol-Version header is required".to_string());
        }
        if method.is_none() {
            return Err("Mcp-Method header is required".to_string());
        }
    }
    Ok(())
}

async fn http_handler(
    State(server): State<Arc<Mutex<McpServer>>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let mut srv = server.lock().await;

    if let Err(msg) = validate_http_headers(&headers, &request, srv.mode()) {
        let resp = JsonRpcResponse::err(request.id.clone(), -32600, &msg);
        return (StatusCode::BAD_REQUEST, Json(resp));
    }

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

    // ── Phase 15 WS2: 2026-07-28 dual-mode ────────────────────────────────

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn test_server_discover() {
        let mut server = make_server();
        let resp = server.handle_request(req("server/discover", json!({}))).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2026-07-28");
        assert_eq!(result["serverInfo"]["name"], "oh-ben-claw");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_handshakeless_tools_call_with_meta() {
        // 2026 clients send no initialize and carry _meta on every request;
        // the server must accept both without complaint.
        let mut server = make_server();
        let params = json!({
            "name": "nonexistent_tool",
            "arguments": {},
            "_meta": {"io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1.0"}}
        });
        let resp = server.handle_request(req("tools/call", params)).await;
        // Tool doesn't exist → -32602, but the _meta and missing handshake
        // must not produce a protocol-level failure.
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn test_tools_list_carries_ttl() {
        let mut server = make_server();
        let resp = server.handle_request(req("tools/list", json!({}))).await;
        let result = resp.result.unwrap();
        assert_eq!(result["ttlMs"], 60_000);
        assert_eq!(result["cacheScope"], "private");
    }

    #[test]
    fn test_http_headers_mismatch_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-method", "tools/list".parse().unwrap());
        let r = req("tools/call", json!({"name": "x"}));
        let err = validate_http_headers(&headers, &r, ProtocolMode::Legacy2024);
        assert!(err.is_err());
    }

    #[test]
    fn test_http_name_header_mismatch_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-method", "tools/call".parse().unwrap());
        headers.insert("mcp-name", "other_tool".parse().unwrap());
        let r = req("tools/call", json!({"name": "search"}));
        let err = validate_http_headers(&headers, &r, ProtocolMode::Legacy2024);
        assert!(err.is_err());
    }

    #[test]
    fn test_http_headers_required_in_2026_mode() {
        let headers = HeaderMap::new();
        let r = req("tools/list", json!({}));
        let err = validate_http_headers(&headers, &r, ProtocolMode::Stateless2026);
        assert!(err.is_err());
    }

    #[test]
    fn test_http_headers_valid_2026_request() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        headers.insert("mcp-method", "tools/call".parse().unwrap());
        headers.insert("mcp-name", "search".parse().unwrap());
        let r = req("tools/call", json!({"name": "search", "arguments": {}}));
        assert!(validate_http_headers(&headers, &r, ProtocolMode::Stateless2026).is_ok());
    }

    #[test]
    fn test_http_legacy_mode_headers_optional() {
        let headers = HeaderMap::new();
        let r = req("tools/list", json!({}));
        assert!(validate_http_headers(&headers, &r, ProtocolMode::Legacy2024).is_ok());
    }

    #[test]
    fn test_http_unsupported_version_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "2030-01-01".parse().unwrap());
        headers.insert("mcp-method", "tools/list".parse().unwrap());
        let r = req("tools/list", json!({}));
        let err = validate_http_headers(&headers, &r, ProtocolMode::Stateless2026);
        assert!(err.is_err());
    }
}

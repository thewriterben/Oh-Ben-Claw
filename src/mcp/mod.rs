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
///
/// Every call forwards its arguments to a server outside the trust boundary, so
/// this is an egress path in exactly the sense the `http` and `browser` tools
/// are. When a conscience [`obc_conscience::ReachGate`] is attached the
/// destination server must be allowlisted for the `mcp` tool or the call is
/// refused before any argument leaves — the same containment rule (the breach
/// lesson: don't rely on the model's own refusals), applied to the MCP-client
/// surface.
pub struct McpRemoteTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
    /// Logical name of the server this tool lives on — the reach host key.
    pub server: String,
    /// Shared MCP client connection.
    pub client: Arc<Mutex<client::McpClient>>,
    /// Optional conscience reach gate. `None` = ungated (until config wires one).
    pub reach: Option<obc_conscience::ReachGate>,
    /// Optional Track 0 auditor so a reach refusal becomes a tamper-evident record.
    pub auditor: Option<Arc<std::sync::Mutex<crate::security::ActionAuditor>>>,
}

impl McpRemoteTool {
    /// Core egress decision, independent of a live client so it is unit-testable.
    /// Keyed on the server name so an operator's allowlist is transport-agnostic
    /// (a stdio subprocess and an HTTP endpoint are gated the same way).
    /// `Ok(())` = allowed (or no gate); `Err(reason)` = the gate refuses.
    fn reach_decision(
        reach: Option<&obc_conscience::ReachGate>,
        server: &str,
    ) -> Result<(), String> {
        let Some(gate) = reach else {
            return Ok(());
        };
        match gate.check("mcp", server) {
            obc_conscience::ReachDecision::Allow { .. } => Ok(()),
            obc_conscience::ReachDecision::Refuse(reason) => Err(reason.to_string()),
        }
    }

    fn check_reach(&self) -> Result<(), String> {
        Self::reach_decision(self.reach.as_ref(), &self.server)
    }
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

    fn output_trust(&self) -> obc_tool_api::OutputTrust {
        // Output comes from a remote MCP server outside the trust boundary
        // (Track 0 taint tracking).
        obc_tool_api::OutputTrust::External
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        // Conscience reach gate (Track 0 for egress): refuse a server that is not
        // allowlisted BEFORE any argument is forwarded. Logged, not silent.
        if let Err(reason) = self.check_reach() {
            tracing::warn!(server = %self.server, tool = %self.name, refusal = %reason,
                "conscience: mcp egress refused");
            if let Some(auditor) = &self.auditor {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let mut g = auditor.lock().unwrap_or_else(|e| e.into_inner());
                let _ = g.record_conscience_refusal(ts, "conscience.reach", &self.server, &reason);
            }
            return Ok(ToolResult::err(format!("conscience: {reason}")));
        }
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

    /// Connect to an MCP server, enforcing the conscience egress rules at the
    /// connection boundary — reach (item (a)) and credential injection (item (b)).
    ///
    /// Before opening the connection: if `reach` refuses the server (not
    /// allowlisted for the `mcp` tool) no connection is made; if the allow names
    /// a credential, it is resolved by name through `resolver` and bound to the
    /// connection — the HTTP bearer token for an `http` server, or an environment
    /// variable of that name for a `stdio` subprocess — so the secret reaches the
    /// server without passing through the model. A named-but-unresolvable
    /// credential **fails closed**: the connection is refused, not opened
    /// unauthenticated. With `reach == None` this is exactly [`Self::connect`].
    pub async fn connect_with_conscience(
        &mut self,
        name: &str,
        config: &McpServerConfig,
        reach: Option<&obc_conscience::ReachGate>,
        resolver: Option<&dyn crate::tools::credentials::CredentialResolver>,
    ) -> Result<usize> {
        let effective = match reach {
            Some(gate) => Self::apply_conscience_to_config(name, config, gate, resolver)?,
            None => config.clone(),
        };
        self.connect(name, &effective).await
    }

    /// Compute the effective connection config with a reach-named credential
    /// bound, or an error if the server is refused / the credential is required
    /// but unresolvable. Pure (no I/O), so the decision is unit-testable without
    /// a live server.
    fn apply_conscience_to_config(
        name: &str,
        config: &McpServerConfig,
        reach: &obc_conscience::ReachGate,
        resolver: Option<&dyn crate::tools::credentials::CredentialResolver>,
    ) -> Result<McpServerConfig> {
        let credential = match reach.check("mcp", name) {
            obc_conscience::ReachDecision::Allow { credential } => credential,
            obc_conscience::ReachDecision::Refuse(reason) => anyhow::bail!("conscience: {reason}"),
        };
        let Some(cred_name) = credential else {
            return Ok(config.clone()); // allowed, no credential required
        };
        let secret = resolver
            .and_then(|r| r.resolve(&cred_name))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "conscience: MCP server '{name}' requires credential '{cred_name}' but it \
                 could not be resolved (not in the vault or environment)"
                )
            })?;
        let mut effective = config.clone();
        match effective.transport.as_str() {
            "http" => effective.token = Some(secret),
            "stdio" => {
                effective
                    .env
                    .get_or_insert_with(Default::default)
                    .insert(cred_name, secret);
            }
            _ => {}
        }
        Ok(effective)
    }

    /// Build `Box<dyn Tool>` instances for all registered MCP tools, ungated.
    pub fn build_tools(&self) -> Vec<Box<dyn Tool>> {
        self.build_tools_with_reach(None, None)
    }

    /// Build tool instances with an optional conscience reach gate attached.
    ///
    /// Every MCP remote tool is an egress path (it forwards arguments to a
    /// server outside the trust boundary), so when a gate is supplied each tool
    /// is constructed already carrying it — gated by construction, the same
    /// discipline as `default_tools_with_reach`. The reach host is the server
    /// name, so an allowlist reads `{ host: "clawcam", ... }` regardless of
    /// whether that server is a stdio subprocess or an HTTP endpoint.
    pub fn build_tools_with_reach(
        &self,
        reach: Option<obc_conscience::ReachGate>,
        auditor: Option<Arc<std::sync::Mutex<crate::security::ActionAuditor>>>,
    ) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .filter_map(|(tool_name, (server_name, tool_def))| {
                self.clients.get(server_name).map(|client| {
                    Box::new(McpRemoteTool {
                        name: tool_name.clone(),
                        description: tool_def.description.clone(),
                        schema: tool_def.input_schema.clone(),
                        server: server_name.clone(),
                        client: client.clone(),
                        reach: reach.clone(),
                        auditor: auditor.clone(),
                    }) as Box<dyn Tool>
                })
            })
            .collect()
    }

    /// Return a shared handle to a connected server's client, if present.
    ///
    /// Lets callers reuse the live connection for out-of-band polling — e.g. a
    /// perception loop that pulls camera detections into world memory.
    ///
    /// The vision module's ingest function is the caller this was written for.
    /// It is described rather than linked: this was `mcp`'s only reference to
    /// `vision` of any kind, so the rustdoc link was the entire `mcp -> vision`
    /// edge, and `vision -> mcp` is three real `use` lines. One doc link, one
    /// cycle. See docs/ENDGAME.md — the script counts text, not calls.
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

    // -- conscience reach gate on the MCP-client egress surface --

    fn egress_gate() -> obc_conscience::ReachGate {
        use obc_conscience::{HostRule, ReachScope, ToolReach};
        obc_conscience::ReachGate::new(
            vec![HostRule {
                host: "clawcam".into(),
                purpose: "perception".into(),
                credential: None,
            }],
            vec![ToolReach {
                tool: "mcp".into(),
                scope: ReachScope::Egress,
            }],
        )
    }

    #[test]
    fn mcp_reach_allows_allowlisted_server() {
        // The server an operator listed can be reached.
        assert!(McpRemoteTool::reach_decision(Some(&egress_gate()), "clawcam").is_ok());
    }

    #[test]
    fn mcp_reach_refuses_unlisted_server() {
        // A server that is not on the allowlist is refused before any argument
        // leaves — default-deny, the same as http/browser egress.
        assert!(McpRemoteTool::reach_decision(Some(&egress_gate()), "exfil-corp").is_err());
    }

    #[test]
    fn mcp_no_gate_allows_any_server() {
        // No gate configured → no egress restriction (until config wires one).
        assert!(McpRemoteTool::reach_decision(None, "anything").is_ok());
    }

    // -- connection-level credential injection (item (b)) --

    /// Allows "github" for the mcp tool AND names a credential to bind.
    fn egress_gate_with_cred() -> obc_conscience::ReachGate {
        use obc_conscience::{HostRule, ReachScope, ToolReach};
        obc_conscience::ReachGate::new(
            vec![HostRule {
                host: "github".into(),
                purpose: "code".into(),
                credential: Some("gh-token".into()),
            }],
            vec![ToolReach {
                tool: "mcp".into(),
                scope: ReachScope::Egress,
            }],
        )
    }

    struct OneKey(&'static str, &'static str);
    impl crate::tools::credentials::CredentialResolver for OneKey {
        fn resolve(&self, name: &str) -> Option<String> {
            (name == self.0).then(|| self.1.to_string())
        }
    }

    fn http_cfg() -> McpServerConfig {
        McpServerConfig {
            transport: "http".into(),
            command: None,
            args: None,
            url: Some("https://mcp.github.example".into()),
            token: None,
            env: None,
            protocol_mode: None,
        }
    }

    fn stdio_cfg() -> McpServerConfig {
        McpServerConfig {
            transport: "stdio".into(),
            command: Some("mcp-github".into()),
            args: None,
            url: None,
            token: None,
            env: None,
            protocol_mode: None,
        }
    }

    #[test]
    fn conscience_config_refuses_unlisted_server() {
        // "evil" isn't allowlisted → refuse before any connection.
        let r = McpRegistry::apply_conscience_to_config(
            "evil",
            &http_cfg(),
            &egress_gate_with_cred(),
            Some(&OneKey("gh-token", "ghp_secret")),
        );
        assert!(r.is_err());
    }

    #[test]
    fn conscience_config_binds_bearer_for_http() {
        // Allowed + credential resolvable → bound as the HTTP bearer token.
        let cfg = McpRegistry::apply_conscience_to_config(
            "github",
            &http_cfg(),
            &egress_gate_with_cred(),
            Some(&OneKey("gh-token", "ghp_secret")),
        )
        .unwrap();
        assert_eq!(cfg.token.as_deref(), Some("ghp_secret"));
    }

    #[test]
    fn conscience_config_binds_env_for_stdio() {
        // Allowed + credential resolvable → bound as an env var of that name.
        let cfg = McpRegistry::apply_conscience_to_config(
            "github",
            &stdio_cfg(),
            &egress_gate_with_cred(),
            Some(&OneKey("gh-token", "ghp_secret")),
        )
        .unwrap();
        assert_eq!(
            cfg.env
                .as_ref()
                .and_then(|e| e.get("gh-token"))
                .map(String::as_str),
            Some("ghp_secret")
        );
    }

    #[test]
    fn conscience_config_fails_closed_when_credential_unresolvable() {
        // Named credential, but the resolver doesn't have it → refuse (no connect).
        let r = McpRegistry::apply_conscience_to_config(
            "github",
            &http_cfg(),
            &egress_gate_with_cred(),
            Some(&OneKey("other", "x")),
        );
        assert!(r.is_err());
        // Same with no resolver at all.
        assert!(McpRegistry::apply_conscience_to_config(
            "github",
            &http_cfg(),
            &egress_gate_with_cred(),
            None,
        )
        .is_err());
    }
}

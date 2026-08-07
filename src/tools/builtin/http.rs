//! HTTP request tool — make HTTP requests and return responses.

use crate::tools::credentials::CredentialResolver;
use crate::tools::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use obc_conscience::{ReachDecision, ReachGate};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool: make HTTP requests (GET, POST, PUT, DELETE, PATCH).
pub struct HttpTool {
    client: Client,
    /// Optional conscience reach gate. When present, the request host must be on
    /// the egress allowlist for the `http` tool or the request is refused before
    /// any connection is made (the breach lesson: containment must not rely on
    /// the model's own refusals). `None` = no egress gate (until config wires one).
    reach: Option<ReachGate>,
    /// Optional Track 0 auditor. When present, a reach refusal is written to the
    /// tamper-evident chain as a `conscience.reach` denial — same as perception.
    auditor: Option<Arc<std::sync::Mutex<crate::security::ActionAuditor>>>,
    /// Optional credential resolver (conscience item (b)). When the reach gate
    /// allows a host *and* names a credential, the secret is resolved through
    /// this and injected as a bearer token at the boundary. If a credential is
    /// named but cannot be resolved, the request is refused — fail-closed.
    resolver: Option<Arc<dyn CredentialResolver>>,
}

impl HttpTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            reach: None,
            auditor: None,
            resolver: None,
        }
    }

    /// Attach a conscience reach gate. Every request host must then be
    /// allowlisted for the `http` tool, or the request is refused pre-connection.
    pub fn with_reach_gate(mut self, gate: ReachGate) -> Self {
        self.reach = Some(gate);
        self
    }

    /// Attach a Track 0 auditor so reach refusals become tamper-evident records.
    pub fn with_auditor(
        mut self,
        auditor: Arc<std::sync::Mutex<crate::security::ActionAuditor>>,
    ) -> Self {
        self.auditor = Some(auditor);
        self
    }

    /// Attach a credential resolver (conscience item (b)). When the reach gate
    /// allows a host that names a credential, its secret is resolved through this
    /// and injected as a bearer token; a named-but-unresolvable credential
    /// refuses the request (fail-closed).
    pub fn with_resolver(mut self, resolver: Arc<dyn CredentialResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Egress check for a URL. On allow, returns the *name* of the credential the
    /// host rule wants injected (if any); `Ok(None)` = allowed, no credential.
    /// `Err(reason)` = the reach gate refuses. Extracted for testability without
    /// a network.
    fn check_reach(&self, url: &str) -> Result<Option<String>, String> {
        let Some(gate) = &self.reach else {
            return Ok(None); // no gate configured
        };
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        match gate.check("http", &host) {
            ReachDecision::Allow { credential } => Ok(credential),
            ReachDecision::Refuse(reason) => Err(reason.to_string()),
        }
    }

    /// Whether the caller already set an `Authorization` header (case-insensitive).
    /// An explicit header is respected — the conscience-injected credential never
    /// silently overrides what the caller asked for.
    fn caller_set_authorization(args: &Value) -> bool {
        args.get("headers")
            .and_then(|v| v.as_object())
            .map(|h| h.keys().any(|k| k.eq_ignore_ascii_case("authorization")))
            .unwrap_or(false)
    }

    /// The host of a URL, for audit records ("" if it won't parse).
    fn host_of(url: &str) -> String {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default()
    }

    /// Write a `conscience.reach` refusal to the tamper-evident chain, if an
    /// auditor is attached. Shared by the host-not-allowed and credential-
    /// unavailable refusal paths so both are recorded identically.
    fn audit_refusal(&self, host: &str, reason: &str) {
        if let Some(auditor) = &self.auditor {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let mut g = auditor.lock().unwrap_or_else(|e| e.into_inner());
            let _ = g.record_conscience_refusal(ts, "conscience.reach", host, reason);
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
        // allowlisted BEFORE any connection is made. Logged, not silent. On
        // allow, `cred_name` is the credential the host rule wants injected.
        let cred_name = match self.check_reach(&url) {
            Ok(c) => c,
            Err(reason) => {
                tracing::warn!(url = %url, refusal = %reason, "conscience: egress refused");
                self.audit_refusal(&Self::host_of(&url), &reason);
                return Ok(ToolResult::err(format!("conscience: {reason}")));
            }
        };

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

        // Conscience credential injection (item (b)): the reach gate allowed this
        // host and named a credential. Resolve it through the vault-backed
        // resolver and inject as a bearer token — the model never sees the
        // secret, only the name. Fail closed: a credential that is named but
        // cannot be resolved refuses the call rather than sending it
        // unauthenticated (a request the operator expected to be authenticated
        // must not silently downgrade). An Authorization header the caller set
        // explicitly is respected and not overridden.
        if let Some(name) = &cred_name {
            if Self::caller_set_authorization(&args) {
                tracing::debug!(credential = %name,
                    "conscience: caller set Authorization; not injecting");
            } else {
                match self.resolver.as_ref().and_then(|r| r.resolve(name)) {
                    Some(token) => {
                        request = request.bearer_auth(token);
                    }
                    None => {
                        let reason = format!(
                            "credential '{name}' is required for this host but could \
                             not be resolved (not in the vault or environment)"
                        );
                        tracing::warn!(url = %url, credential = %name,
                            "conscience: egress refused (credential unavailable)");
                        self.audit_refusal(&Self::host_of(&url), &reason);
                        return Ok(ToolResult::err(format!("conscience: {reason}")));
                    }
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
            vec![ToolReach {
                tool: "http".into(),
                scope: ReachScope::Egress,
            }],
        )
    }

    #[test]
    fn reach_allows_allowlisted_host() {
        let tool = HttpTool::new().with_reach_gate(gate());
        assert!(tool
            .check_reach("https://api.allowed.example/v1/thing")
            .is_ok());
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

    #[tokio::test]
    async fn refused_reach_is_written_to_the_audit_chain() {
        use crate::security::ActionAuditor;
        use std::sync::{Arc, Mutex};
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("obc-http-conscience-{nanos}.jsonl"));
        let key = b"test-key".to_vec();
        let auditor = Arc::new(Mutex::new(
            ActionAuditor::open(key.clone(), path.clone()).unwrap(),
        ));
        let tool = HttpTool::new()
            .with_reach_gate(gate())
            .with_auditor(Arc::clone(&auditor));

        let result = tool
            .execute(json!({"url": "https://evil.example.com/exfil"}))
            .await
            .unwrap();
        assert!(!result.success); // refused, no connection

        // the refusal is a tamper-evident record in the same chain as actions
        assert_eq!(crate::security::audit::verify(&path, &key).unwrap(), 1);
        let _ = std::fs::remove_file(&path);
    }

    // -- conscience credential injection (item (b)) --

    /// Allows the host, and the rule names a credential to inject.
    fn gate_with_cred() -> ReachGate {
        use obc_conscience::{HostRule, ReachScope, ToolReach};
        ReachGate::new(
            vec![HostRule {
                host: "api.allowed.example".into(),
                purpose: "test".into(),
                credential: Some("api-key".into()),
            }],
            vec![ToolReach {
                tool: "http".into(),
                scope: ReachScope::Egress,
            }],
        )
    }

    /// A resolver that knows exactly one name — no vault, no disk.
    struct OneKey(&'static str, &'static str);
    impl CredentialResolver for OneKey {
        fn resolve(&self, name: &str) -> Option<String> {
            (name == self.0).then(|| self.1.to_string())
        }
    }

    #[test]
    fn reach_surfaces_the_named_credential_on_allow() {
        let tool = HttpTool::new().with_reach_gate(gate_with_cred());
        assert_eq!(
            tool.check_reach("https://api.allowed.example/x")
                .unwrap()
                .as_deref(),
            Some("api-key")
        );
    }

    #[test]
    fn detects_caller_set_authorization_case_insensitively() {
        assert!(HttpTool::caller_set_authorization(
            &json!({"headers": {"AuThOrIzAtIoN": "Bearer x"}})
        ));
        assert!(!HttpTool::caller_set_authorization(
            &json!({"headers": {"X-Other": "y"}})
        ));
        assert!(!HttpTool::caller_set_authorization(&json!({})));
    }

    #[tokio::test]
    async fn execute_fails_closed_when_named_credential_has_no_resolver() {
        // Host is allowed, but the rule requires a credential and no resolver is
        // wired. Fail closed BEFORE any connection — never send unauthenticated.
        let tool = HttpTool::new().with_reach_gate(gate_with_cred());
        let result = tool
            .execute(json!({"url": "https://api.allowed.example/x"}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap_or_default();
        assert!(err.contains("conscience"), "{err}");
        assert!(err.contains("credential"), "{err}");
    }

    #[tokio::test]
    async fn execute_fails_closed_when_resolver_lacks_the_name() {
        // A resolver is present but doesn't know "api-key" → still fail closed.
        let tool = HttpTool::new()
            .with_reach_gate(gate_with_cred())
            .with_resolver(Arc::new(OneKey("some-other-key", "sekret")));
        let result = tool
            .execute(json!({"url": "https://api.allowed.example/x"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("credential"));
    }

    #[tokio::test]
    async fn credential_refusal_is_written_to_the_audit_chain() {
        use crate::security::ActionAuditor;
        use std::sync::Mutex;
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("obc-http-cred-{nanos}.jsonl"));
        let key = b"test-key".to_vec();
        let auditor = Arc::new(Mutex::new(
            ActionAuditor::open(key.clone(), path.clone()).unwrap(),
        ));
        // Allowed host, required-but-unresolvable credential → audited refusal.
        let tool = HttpTool::new()
            .with_reach_gate(gate_with_cred())
            .with_auditor(Arc::clone(&auditor));
        let result = tool
            .execute(json!({"url": "https://api.allowed.example/x"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(crate::security::audit::verify(&path, &key).unwrap(), 1);
        let _ = std::fs::remove_file(&path);
    }
}

//! Sub-agent scopes — the tools and vault credentials a spawned sub-agent may
//! use, never wider than its parent's (PERSONAL-AGENTS-2026-11 item 9).
//!
//! A scope is a stack of layers. Each layer is a list of globs (the same `*`
//! and `?` the tool policy uses); a name is in scope only when **every** layer
//! has a glob matching it. The pool's ceiling (`[orchestrator.sub_agent_scope]`)
//! is the first layer and a spawn request adds its own, so a child can only
//! narrow what its parent had: an intersection needs no glob-subset proof.
//!
//! Enforced at two points. Tools outside the scope are never registered, and
//! the execution chokepoint refuses a call to a name outside it (every delegate
//! hop included). Vault names are enforced by [`ScopedResolver`] at the HTTP
//! tool's credential injection, which fails closed on a name it cannot resolve,
//! and by [`VaultScopedFill`] on the parent's `browser_fill_secret`.
//!
//! What this is not: an OS boundary. A sub-agent given `shell` or `file` can
//! read the environment and the files the process can read, credentials
//! included. Keep those two out of a sub-agent's tools when its vault scope
//! has to mean something.

use anyhow::{bail, Result};
use async_trait::async_trait;
use obc_safety::glob_match;
use obc_tool_api::{OutputTrust, RiskClass, RolloutStage};
use obc_tools::credentials::CredentialResolver;
use obc_tools::{Tool, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// `[orchestrator.sub_agent_scope]` — the ceiling every sub-agent is cut from.
/// The default (`"*"` for both) keeps the behaviour before scopes existed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopeConfig {
    /// Tool-name globs a sub-agent may be given.
    #[serde(default = "everything")]
    pub tools: Vec<String>,
    /// Vault credential names (globs) a sub-agent's egress may inject.
    #[serde(default = "everything")]
    pub vaults: Vec<String>,
}

fn everything() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            tools: everything(),
            vaults: everything(),
        }
    }
}

/// A resolved scope: one glob list per layer, for tools and for vaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    tools: Vec<Vec<String>>,
    vaults: Vec<Vec<String>>,
}

impl Scope {
    /// No layers: every tool and every vault name is in scope.
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// The pool ceiling from config, as the first layer.
    pub fn from_config(config: &ScopeConfig) -> Self {
        Self {
            tools: vec![config.tools.clone()],
            vaults: vec![config.vaults.clone()],
        }
    }

    pub fn allows_tool(&self, name: &str) -> bool {
        allowed(&self.tools, name)
    }

    pub fn allows_vault(&self, name: &str) -> bool {
        allowed(&self.vaults, name)
    }

    /// Cut a child's scope from this one. For each list: empty inherits this
    /// scope unchanged, `["none"]` grants nothing, and anything else adds a
    /// layer. A literal name (no glob characters) outside this scope is
    /// refused, so a request for something the parent lacks fails loudly
    /// instead of quietly spawning an agent without it; a glob is intersected.
    pub fn narrow(&self, tools: &[String], vaults: &[String]) -> Result<Scope> {
        let mut child = self.clone();
        if let Some(layer) = layer(tools, |n| self.allows_tool(n), "tool")? {
            child.tools.push(layer);
        }
        if let Some(layer) = layer(vaults, |n| self.allows_vault(n), "vault")? {
            child.vaults.push(layer);
        }
        Ok(child)
    }
}

fn allowed(layers: &[Vec<String>], name: &str) -> bool {
    layers
        .iter()
        .all(|globs| globs.iter().any(|g| glob_match(g, name)))
}

fn layer(
    requested: &[String],
    parent_allows: impl Fn(&str) -> bool,
    kind: &str,
) -> Result<Option<Vec<String>>> {
    if requested.is_empty() {
        return Ok(None);
    }
    if requested == ["none"] {
        return Ok(Some(Vec::new()));
    }
    for name in requested {
        let literal = !name.contains(['*', '?']);
        if literal && !parent_allows(name) {
            bail!("{kind} '{name}' is outside the parent's scope");
        }
    }
    Ok(Some(requested.to_vec()))
}

/// A credential resolver that only resolves names in a scope's vault list.
/// Out of scope reads as "no such credential", which the HTTP tool treats as
/// a refusal (fail closed), never as a request sent without it.
pub struct ScopedResolver {
    inner: Arc<dyn CredentialResolver>,
    scope: Scope,
    agent: String,
}

impl ScopedResolver {
    pub fn new(inner: Arc<dyn CredentialResolver>, scope: Scope, agent: &str) -> Self {
        Self {
            inner,
            scope,
            agent: agent.to_string(),
        }
    }
}

impl CredentialResolver for ScopedResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        if !self.scope.allows_vault(name) {
            tracing::warn!(agent = %self.agent, credential = %name, "credential outside the sub-agent's vault scope");
            return None;
        }
        self.inner.resolve(name)
    }
}

/// The parent's `browser_fill_secret`, limited to a scope's vault list. That
/// tool holds the parent's own browser session and unscoped resolver, so it is
/// wrapped rather than rebuilt: a `secret` outside the scope is refused before
/// the inner tool runs, and every other trait method is the inner tool's.
pub struct VaultScopedFill {
    inner: Arc<dyn Tool>,
    scope: Scope,
    agent: String,
}

impl VaultScopedFill {
    /// The tool this wraps.
    pub const TOOL: &'static str = "browser_fill_secret";

    pub fn new(inner: Arc<dyn Tool>, scope: Scope, agent: &str) -> Self {
        Self {
            inner,
            scope,
            agent: agent.to_string(),
        }
    }
}

#[async_trait]
impl Tool for VaultScopedFill {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    fn risk_class(&self) -> RiskClass {
        self.inner.risk_class()
    }

    fn rollout_stage(&self) -> RolloutStage {
        self.inner.rollout_stage()
    }

    fn output_trust(&self) -> OutputTrust {
        self.inner.output_trust()
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        // The same parse as the tool's own: `vault:<name>`, trimmed. A
        // malformed reference is left for the inner tool to refuse.
        let name = args
            .get("secret")
            .and_then(|v| v.as_str())
            .and_then(|r| r.strip_prefix("vault:"))
            .map(str::trim);
        if let Some(name) = name.filter(|n| !self.scope.allows_vault(n)) {
            tracing::warn!(agent = %self.agent, credential = %name, "secret outside the sub-agent's vault scope");
            return Ok(ToolResult::err(format!(
                "secret 'vault:{name}' is outside this sub-agent's vault scope"
            )));
        }
        self.inner.execute(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn ceiling(tools: &[&str], vaults: &[&str]) -> Scope {
        Scope::from_config(&ScopeConfig {
            tools: s(tools),
            vaults: s(vaults),
        })
    }

    #[test]
    fn unrestricted_allows_everything() {
        let scope = Scope::unrestricted();
        assert!(scope.allows_tool("shell"));
        assert!(scope.allows_vault("github"));
    }

    #[test]
    fn default_config_keeps_the_old_behaviour() {
        let scope = Scope::from_config(&ScopeConfig::default());
        assert!(scope.allows_tool("shell"));
        assert!(scope.allows_vault("anything"));
    }

    #[test]
    fn empty_request_inherits_the_parent() {
        let parent = ceiling(&["http", "browser_*"], &["github"]);
        let child = parent.narrow(&[], &[]).unwrap();
        assert_eq!(child, parent);
    }

    #[test]
    fn none_grants_nothing() {
        let child = Scope::unrestricted()
            .narrow(&s(&["none"]), &s(&["none"]))
            .unwrap();
        assert!(!child.allows_tool("http"));
        assert!(!child.allows_vault("github"));
    }

    #[test]
    fn a_child_glob_is_intersected_with_the_parent() {
        let parent = ceiling(&["browser_*", "http"], &["*"]);
        let child = parent.narrow(&s(&["*"]), &[]).unwrap();
        // The child asked for everything and got only what the parent had.
        assert!(child.allows_tool("browser_navigate"));
        assert!(child.allows_tool("http"));
        assert!(!child.allows_tool("shell"));
    }

    #[test]
    fn a_child_can_narrow() {
        let parent = ceiling(&["*"], &["github", "slack_*"]);
        let child = parent.narrow(&s(&["http"]), &s(&["slack_*"])).unwrap();
        assert!(child.allows_tool("http"));
        assert!(!child.allows_tool("shell"));
        assert!(child.allows_vault("slack_bot"));
        assert!(!child.allows_vault("github"));
    }

    #[test]
    fn a_literal_outside_the_parent_is_refused() {
        let parent = ceiling(&["http"], &["github"]);
        let err = parent.narrow(&s(&["shell"]), &[]).unwrap_err();
        assert!(err.to_string().contains("tool 'shell'"), "{err}");
        let err = parent.narrow(&[], &s(&["aws_root"])).unwrap_err();
        assert!(err.to_string().contains("vault 'aws_root'"), "{err}");
    }

    #[test]
    fn a_grandchild_cannot_widen_past_the_middle_layer() {
        let child = ceiling(&["*"], &["*"])
            .narrow(&s(&["http", "file"]), &[])
            .unwrap();
        assert!(child.narrow(&s(&["shell"]), &[]).is_err());
        let grandchild = child.narrow(&s(&["*"]), &[]).unwrap();
        assert!(grandchild.allows_tool("file"));
        assert!(!grandchild.allows_tool("shell"));
    }

    struct Fixed;
    impl CredentialResolver for Fixed {
        fn resolve(&self, name: &str) -> Option<String> {
            Some(format!("secret-{name}"))
        }
    }

    #[test]
    fn scoped_resolver_withholds_out_of_scope_names() {
        let scope = Scope::unrestricted().narrow(&[], &s(&["github"])).unwrap();
        let r = ScopedResolver::new(Arc::new(Fixed), scope, "researcher");
        assert_eq!(r.resolve("github").as_deref(), Some("secret-github"));
        assert_eq!(r.resolve("aws_root"), None);
    }

    /// Stands in for `browser_fill_secret`: succeeds whenever it is reached.
    struct Fill;
    #[async_trait]
    impl Tool for Fill {
        fn name(&self) -> &str {
            VaultScopedFill::TOOL
        }
        fn description(&self) -> &str {
            "fill"
        }
        fn risk_class(&self) -> RiskClass {
            RiskClass {
                physical: false,
                ..RiskClass::default()
            }
        }
        async fn execute(&self, _args: Value) -> Result<ToolResult> {
            Ok(ToolResult::ok("filled"))
        }
    }

    #[tokio::test]
    async fn vault_scoped_fill_refuses_out_of_scope_secrets() {
        let scope = Scope::unrestricted().narrow(&[], &s(&["github"])).unwrap();
        let fill = VaultScopedFill::new(Arc::new(Fill), scope, "researcher");
        let ok = fill
            .execute(serde_json::json!({"field": "#p", "secret": "vault:github"}))
            .await
            .unwrap();
        assert!(ok.is_ok());
        let refused = fill
            .execute(serde_json::json!({"field": "#p", "secret": "vault: aws_root"}))
            .await
            .unwrap();
        assert!(!refused.is_ok());
        assert!(refused.error.unwrap_or_default().contains("aws_root"));
    }
}

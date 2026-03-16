//! Tool Policy Engine
//!
//! Policies are evaluated in order. The first matching policy wins.
//! If no policy matches, the default action is `Allow`.
//!
//! # Policy Matching
//!
//! Each policy has a `tool_pattern` (a glob-style pattern matched against the tool name)
//! and an optional `arg_contains` string matched against the serialized tool arguments.
//!
//! # Actions
//!
//! - `Allow` — permit the tool call (default)
//! - `Deny`  — block the tool call and return an error to the agent
//! - `Audit` — allow the tool call but log it with a warning

use serde::{Deserialize, Serialize};

/// What to do when a policy matches.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPolicyAction {
    #[default]
    Allow,
    Deny,
    Audit,
}

/// A single tool execution policy rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Human-readable name for this policy rule.
    pub name: String,

    /// Glob pattern matched against the tool name (e.g. `"shell"`, `"file_*"`, `"*"`).
    pub tool_pattern: String,

    /// Optional substring matched against the serialized JSON arguments.
    /// If set, the policy only matches when the arguments contain this string.
    #[serde(default)]
    pub arg_contains: Option<String>,

    /// The action to take when this policy matches.
    #[serde(default)]
    pub action: ToolPolicyAction,

    /// Optional human-readable reason shown in logs and error messages.
    #[serde(default)]
    pub reason: Option<String>,
}

impl ToolPolicy {
    /// Check whether this policy matches the given tool name and argument string.
    pub fn matches(&self, tool_name: &str, args_json: &str) -> bool {
        if !glob_match(&self.tool_pattern, tool_name) {
            return false;
        }
        if let Some(ref needle) = self.arg_contains {
            if !args_json.contains(needle.as_str()) {
                return false;
            }
        }
        true
    }
}

/// The result of a policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyVerdict {
    pub action: ToolPolicyAction,
    pub policy_name: Option<String>,
    pub reason: Option<String>,
}

impl PolicyVerdict {
    pub fn allow() -> Self {
        Self {
            action: ToolPolicyAction::Allow,
            policy_name: None,
            reason: None,
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.action != ToolPolicyAction::Deny
    }
}

/// The policy engine — evaluates a list of `ToolPolicy` rules in order.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    policies: Vec<ToolPolicy>,
}

impl PolicyEngine {
    /// Create a new policy engine with the given rules.
    pub fn new(policies: Vec<ToolPolicy>) -> Self {
        Self { policies }
    }

    /// Evaluate policies for a given tool call.
    ///
    /// Returns the first matching policy verdict, or `Allow` if no policy matches.
    pub fn evaluate(&self, tool_name: &str, args_json: &str) -> PolicyVerdict {
        for policy in &self.policies {
            if policy.matches(tool_name, args_json) {
                let verdict = PolicyVerdict {
                    action: policy.action.clone(),
                    policy_name: Some(policy.name.clone()),
                    reason: policy.reason.clone(),
                };

                match &verdict.action {
                    ToolPolicyAction::Allow => {
                        tracing::debug!(
                            tool = tool_name,
                            policy = %policy.name,
                            "Tool call allowed by policy"
                        );
                    }
                    ToolPolicyAction::Deny => {
                        tracing::warn!(
                            tool = tool_name,
                            policy = %policy.name,
                            reason = ?policy.reason,
                            "Tool call DENIED by policy"
                        );
                    }
                    ToolPolicyAction::Audit => {
                        tracing::warn!(
                            tool = tool_name,
                            policy = %policy.name,
                            args = args_json,
                            "Tool call AUDITED by policy (allowed)"
                        );
                    }
                }

                return verdict;
            }
        }

        // No policy matched — default allow
        PolicyVerdict::allow()
    }

    /// Return the number of configured policies.
    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }
}

// ── Glob Matching ─────────────────────────────────────────────────────────────

/// Maximum recursion depth for glob matching to prevent ReDoS on pathological
/// patterns like `*a*a*a*a*` against long input strings.
const GLOB_MAX_DEPTH: usize = 64;

/// Minimal glob matcher supporting `*` (any sequence) and `?` (any single char).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t, 0)
}

fn glob_match_inner(p: &[char], t: &[char], depth: usize) -> bool {
    if depth > GLOB_MAX_DEPTH {
        return false;
    }
    match (p.first(), t.first()) {
        (None, None) => true,
        (Some(&'*'), _) => {
            // '*' matches zero or more characters
            glob_match_inner(&p[1..], t, depth + 1)
                || (!t.is_empty() && glob_match_inner(p, &t[1..], depth + 1))
        }
        (Some(&'?'), Some(_)) => glob_match_inner(&p[1..], &t[1..], depth + 1),
        (Some(pc), Some(tc)) if pc == tc => glob_match_inner(&p[1..], &t[1..], depth + 1),
        _ => false,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn deny_policy(pattern: &str) -> ToolPolicy {
        ToolPolicy {
            name: format!("deny-{}", pattern),
            tool_pattern: pattern.to_string(),
            arg_contains: None,
            action: ToolPolicyAction::Deny,
            reason: Some("test deny".to_string()),
        }
    }

    fn audit_policy(pattern: &str, arg_contains: Option<&str>) -> ToolPolicy {
        ToolPolicy {
            name: format!("audit-{}", pattern),
            tool_pattern: pattern.to_string(),
            arg_contains: arg_contains.map(|s| s.to_string()),
            action: ToolPolicyAction::Audit,
            reason: None,
        }
    }

    #[test]
    fn no_policies_allows_everything() {
        let engine = PolicyEngine::new(vec![]);
        let verdict = engine.evaluate("shell", r#"{"command":"ls"}"#);
        assert!(verdict.is_allowed());
        assert_eq!(verdict.action, ToolPolicyAction::Allow);
    }

    #[test]
    fn exact_name_deny_blocks_tool() {
        let engine = PolicyEngine::new(vec![deny_policy("shell")]);
        let verdict = engine.evaluate("shell", "{}");
        assert!(!verdict.is_allowed());
        assert_eq!(verdict.action, ToolPolicyAction::Deny);
    }

    #[test]
    fn wildcard_deny_blocks_matching_tools() {
        let engine = PolicyEngine::new(vec![deny_policy("file_*")]);
        assert!(!engine.evaluate("file_read", "{}").is_allowed());
        assert!(!engine.evaluate("file_write", "{}").is_allowed());
        assert!(engine.evaluate("shell", "{}").is_allowed());
    }

    #[test]
    fn arg_contains_only_matches_when_args_match() {
        let engine = PolicyEngine::new(vec![audit_policy("shell", Some("/etc/passwd"))]);
        // Should audit when args contain /etc/passwd
        let v1 = engine.evaluate("shell", r#"{"command":"cat /etc/passwd"}"#);
        assert_eq!(v1.action, ToolPolicyAction::Audit);
        // Should allow when args don't contain it
        let v2 = engine.evaluate("shell", r#"{"command":"ls"}"#);
        assert_eq!(v2.action, ToolPolicyAction::Allow);
    }

    #[test]
    fn first_matching_policy_wins() {
        let engine = PolicyEngine::new(vec![audit_policy("shell", None), deny_policy("shell")]);
        // Audit comes first — should audit, not deny
        let v = engine.evaluate("shell", "{}");
        assert_eq!(v.action, ToolPolicyAction::Audit);
    }

    #[test]
    fn glob_star_matches_anything() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("file_*", "file_read"));
        assert!(!glob_match("file_*", "shell"));
    }

    #[test]
    fn glob_question_matches_single_char() {
        assert!(glob_match("sh?ll", "shell"));
        assert!(!glob_match("sh?ll", "shill_extra"));
    }

    #[test]
    fn glob_deep_recursion_is_bounded() {
        // Pathological pattern that would cause exponential backtracking
        // without the depth limit. With the limit, this completes instantly.
        let pattern = "*a*a*a*a*a*a*a*a*a*a*";
        let text = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        // Should return false (no match) rather than hang
        assert!(!glob_match(pattern, text));
    }
}

//! Human-in-the-loop approval workflow.
//!
//! The `ApprovalManager` checks whether a tool call needs explicit user approval
//! before execution, prompts the user interactively via stdin, and records all
//! decisions in an in-memory audit log.

use crate::config::{AutonomyConfig, AutonomyLevel};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::Arc;

/// A pending tool approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// The name of the tool being requested.
    pub tool_name: String,
    /// The arguments that will be passed to the tool.
    pub arguments: serde_json::Value,
}

/// The user's response to an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalResponse {
    /// Allow this single invocation.
    Yes,
    /// Deny this invocation.
    No,
    /// Allow all future invocations of this tool in the current session.
    Always,
}

/// A single entry in the approval audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalLogEntry {
    /// ISO-8601 timestamp of the decision.
    pub timestamp: String,
    /// Name of the tool that was reviewed.
    pub tool_name: String,
    /// Truncated summary of the arguments.
    pub arguments_summary: String,
    /// The decision that was made.
    pub decision: ApprovalResponse,
    /// The channel or context in which the approval occurred.
    pub channel: String,
}

/// Manages the human-in-the-loop approval flow for tool execution.
pub struct ApprovalManager {
    config: AutonomyConfig,
    /// Tools approved with `Always` during this session.
    session_allowlist: Arc<Mutex<HashSet<String>>>,
    /// Audit log of all approval decisions this session.
    audit_log: Arc<Mutex<Vec<ApprovalLogEntry>>>,
    /// When true, all approval requests are auto-denied (non-interactive mode).
    non_interactive: bool,
}

impl ApprovalManager {
    /// Create an `ApprovalManager` from the given autonomy configuration.
    pub fn from_config(config: &AutonomyConfig) -> Self {
        Self {
            config: config.clone(),
            session_allowlist: Arc::new(Mutex::new(HashSet::new())),
            audit_log: Arc::new(Mutex::new(Vec::new())),
            non_interactive: false,
        }
    }

    /// Create an `ApprovalManager` for non-interactive contexts (bots, API, etc.).
    ///
    /// In non-interactive mode every request that would normally prompt the user
    /// is automatically denied so that the system never blocks waiting for input.
    pub fn for_non_interactive(config: &AutonomyConfig) -> Self {
        Self {
            config: config.clone(),
            session_allowlist: Arc::new(Mutex::new(HashSet::new())),
            audit_log: Arc::new(Mutex::new(Vec::new())),
            non_interactive: true,
        }
    }

    /// Returns `true` if this tool requires explicit user approval before execution.
    pub fn needs_approval(&self, tool_name: &str) -> bool {
        // always_ask overrides everything
        if self.config.always_ask.iter().any(|t| t == tool_name) {
            return true;
        }
        // auto_approve short-circuits any level check
        if self.config.auto_approve.iter().any(|t| t == tool_name) {
            return false;
        }
        // session allowlist from previous Always decisions
        if self.session_allowlist.lock().contains(tool_name) {
            return false;
        }
        match self.config.level {
            AutonomyLevel::Full => false,
            AutonomyLevel::Supervised => true,
            AutonomyLevel::Manual => true,
        }
    }

    /// Prompt the user for approval of a tool call.
    ///
    /// In non-interactive mode this always returns [`ApprovalResponse::No`].
    pub fn request_approval(&self, req: &ApprovalRequest) -> ApprovalResponse {
        let args_summary = {
            let s = req.arguments.to_string();
            if s.len() > 120 { format!("{}…", &s[..120]) } else { s }
        };

        let decision = if self.non_interactive {
            ApprovalResponse::No
        } else {
            println!(
                "\n⚠️  Tool approval required\n   Tool : {}\n   Args : {}\n",
                req.tool_name, args_summary
            );
            println!("Allow? [y]es / [n]o / [a]lways: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                ApprovalResponse::No
            } else {
                match input.trim().to_lowercase().as_str() {
                    "y" | "yes" => ApprovalResponse::Yes,
                    "a" | "always" => ApprovalResponse::Always,
                    _ => ApprovalResponse::No,
                }
            }
        };

        if decision == ApprovalResponse::Always {
            self.session_allowlist.lock().insert(req.tool_name.clone());
        }

        let entry = ApprovalLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_name: req.tool_name.clone(),
            arguments_summary: args_summary,
            decision: decision.clone(),
            channel: if self.non_interactive { "non-interactive".to_string() } else { "cli".to_string() },
        };
        self.audit_log.lock().push(entry);

        decision
    }

    /// Returns a snapshot of the full approval audit log for this session.
    pub fn audit_log(&self) -> Vec<ApprovalLogEntry> {
        self.audit_log.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyLevel;

    fn config_full() -> AutonomyConfig {
        AutonomyConfig { level: AutonomyLevel::Full, auto_approve: vec![], always_ask: vec![] }
    }
    fn config_supervised() -> AutonomyConfig {
        AutonomyConfig { level: AutonomyLevel::Supervised, auto_approve: vec![], always_ask: vec![] }
    }
    fn config_manual() -> AutonomyConfig {
        AutonomyConfig { level: AutonomyLevel::Manual, auto_approve: vec![], always_ask: vec![] }
    }

    #[test]
    fn full_autonomy_no_approval_needed() {
        let mgr = ApprovalManager::from_config(&config_full());
        assert!(!mgr.needs_approval("shell"));
    }

    #[test]
    fn supervised_needs_approval() {
        let mgr = ApprovalManager::from_config(&config_supervised());
        assert!(mgr.needs_approval("shell"));
    }

    #[test]
    fn auto_approve_bypasses_supervised() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["read_file".to_string()],
            always_ask: vec![],
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(!mgr.needs_approval("read_file"));
        assert!(mgr.needs_approval("shell"));
    }

    #[test]
    fn always_ask_overrides_full() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Full,
            auto_approve: vec![],
            always_ask: vec!["delete_file".to_string()],
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(mgr.needs_approval("delete_file"));
        assert!(!mgr.needs_approval("shell"));
    }

    #[test]
    fn non_interactive_auto_denies() {
        let mgr = ApprovalManager::for_non_interactive(&config_supervised());
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: serde_json::json!({"cmd": "ls"}),
        };
        assert_eq!(mgr.request_approval(&req), ApprovalResponse::No);
    }

    #[test]
    fn session_allowlist_populated_by_always() {
        let mgr = ApprovalManager::for_non_interactive(&config_supervised());
        // manually insert into allowlist
        mgr.session_allowlist.lock().insert("read_file".to_string());
        assert!(!mgr.needs_approval("read_file"));
    }

    #[test]
    fn audit_log_records_decisions() {
        let mgr = ApprovalManager::for_non_interactive(&config_supervised());
        let req = ApprovalRequest {
            tool_name: "shell".to_string(),
            arguments: serde_json::json!({}),
        };
        mgr.request_approval(&req);
        let log = mgr.audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tool_name, "shell");
        assert_eq!(log[0].decision, ApprovalResponse::No);
    }

    #[test]
    fn manual_needs_approval() {
        let mgr = ApprovalManager::from_config(&config_manual());
        assert!(mgr.needs_approval("shell"));
    }
}

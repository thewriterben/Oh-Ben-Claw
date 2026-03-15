//! Node Pairing and Authentication
//!
//! Peripheral nodes must prove their identity before their tools are accepted
//! into the agent's tool registry. The pairing protocol is:
//!
//! 1. The node generates a `PairingToken` = HMAC-SHA256(secret, node_id + timestamp)
//! 2. The node includes the token in its `NodeAnnouncement` metadata
//! 3. The brain verifies the token using the shared secret
//! 4. If valid (and not expired), the node is marked `Paired`
//! 5. If invalid or missing, the node is marked `Quarantined`
//!
//! Tokens are valid for 5 minutes to prevent replay attacks.

use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// The pairing status of a peripheral node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingStatus {
    /// The node has been verified and its tools are trusted.
    Paired,
    /// The node has not yet presented a valid token.
    Unpaired,
    /// The node presented an invalid or expired token.
    Quarantined { reason: String },
}

impl std::fmt::Display for PairingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paired => write!(f, "Paired"),
            Self::Unpaired => write!(f, "Unpaired"),
            Self::Quarantined { reason } => write!(f, "Quarantined ({})", reason),
        }
    }
}

/// A pairing token presented by a peripheral node in its `NodeAnnouncement`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairingToken {
    /// The node ID this token was generated for.
    pub node_id: String,
    /// Unix timestamp (seconds) when the token was generated.
    pub timestamp: u64,
    /// HMAC-SHA256(secret, node_id + ":" + timestamp) as a hex string.
    pub hmac: String,
}

impl PairingToken {
    /// Generate a new pairing token for the given node using the shared secret.
    pub fn generate(secret: &str, node_id: &str) -> Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();

        let message = format!("{}:{}", node_id, timestamp);
        let hmac = compute_hmac(secret, &message)?;

        Ok(Self {
            node_id: node_id.to_string(),
            timestamp,
            hmac,
        })
    }

    /// Verify this token against the shared secret.
    ///
    /// Returns an error if the HMAC is invalid or the token is older than `max_age_secs`.
    pub fn verify(&self, secret: &str, max_age_secs: u64) -> Result<()> {
        // Check timestamp freshness
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();

        if now.saturating_sub(self.timestamp) > max_age_secs {
            anyhow::bail!(
                "Pairing token expired (age={}s, max={}s)",
                now.saturating_sub(self.timestamp),
                max_age_secs
            );
        }

        // Verify HMAC
        let message = format!("{}:{}", self.node_id, self.timestamp);
        let expected = compute_hmac(secret, &message)?;

        if expected != self.hmac {
            anyhow::bail!("Pairing token HMAC mismatch for node '{}'", self.node_id);
        }

        Ok(())
    }
}

fn compute_hmac(secret: &str, message: &str) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| anyhow::anyhow!("HMAC key error: {}", e))?;
    mac.update(message.as_bytes());
    let result = mac.finalize();
    Ok(hex::encode(result.into_bytes()))
}

/// Manages the pairing state of all known peripheral nodes.
#[derive(Debug, Clone)]
pub struct NodePairingManager {
    secret: Option<String>,
    /// node_id → PairingStatus
    state: Arc<Mutex<HashMap<String, PairingStatus>>>,
    /// Token validity window in seconds (default: 300 = 5 minutes).
    max_token_age_secs: u64,
}

impl NodePairingManager {
    /// Create a new pairing manager.
    ///
    /// If `secret` is `None`, pairing is disabled and all nodes are treated as trusted.
    pub fn new(secret: Option<String>) -> Self {
        Self {
            secret,
            state: Arc::new(Mutex::new(HashMap::new())),
            max_token_age_secs: 300,
        }
    }

    /// Check whether pairing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.secret.is_some()
    }

    /// Attempt to pair a node using the token embedded in its announcement metadata.
    ///
    /// Returns the resulting `PairingStatus`.
    pub fn pair_node(&self, node_id: &str, token_json: Option<&serde_json::Value>) -> PairingStatus {
        if !self.is_enabled() {
            // Pairing disabled — all nodes are trusted
            let status = PairingStatus::Paired;
            self.set_status(node_id, status.clone());
            return status;
        }

        let secret = self.secret.as_deref().unwrap();

        let status = match token_json.and_then(|v| v.get("pairing_token")) {
            None => PairingStatus::Quarantined {
                reason: "no pairing token in announcement".to_string(),
            },
            Some(token_val) => {
                match serde_json::from_value::<PairingToken>(token_val.clone()) {
                    Err(e) => PairingStatus::Quarantined {
                        reason: format!("invalid token format: {}", e),
                    },
                    Ok(token) => {
                        if token.node_id != node_id {
                            PairingStatus::Quarantined {
                                reason: format!(
                                    "token node_id mismatch: expected '{}', got '{}'",
                                    node_id, token.node_id
                                ),
                            }
                        } else {
                            match token.verify(secret, self.max_token_age_secs) {
                                Ok(()) => {
                                    tracing::info!(node_id = %node_id, "Node paired successfully");
                                    PairingStatus::Paired
                                }
                                Err(e) => PairingStatus::Quarantined {
                                    reason: e.to_string(),
                                },
                            }
                        }
                    }
                }
            }
        };

        if let PairingStatus::Quarantined { ref reason } = status {
            tracing::warn!(node_id = %node_id, reason = %reason, "Node quarantined");
        }

        self.set_status(node_id, status.clone());
        status
    }

    /// Get the current pairing status of a node.
    pub fn status(&self, node_id: &str) -> PairingStatus {
        self.state
            .lock()
            .unwrap()
            .get(node_id)
            .cloned()
            .unwrap_or(PairingStatus::Unpaired)
    }

    /// Check whether a node is trusted (Paired or pairing disabled).
    pub fn is_trusted(&self, node_id: &str) -> bool {
        if !self.is_enabled() {
            return true;
        }
        self.status(node_id) == PairingStatus::Paired
    }

    /// Manually revoke a node's pairing (e.g., on disconnect or security event).
    pub fn revoke(&self, node_id: &str) {
        self.set_status(
            node_id,
            PairingStatus::Quarantined {
                reason: "manually revoked".to_string(),
            },
        );
        tracing::info!(node_id = %node_id, "Node pairing revoked");
    }

    /// Return a snapshot of all node pairing states.
    pub fn all_statuses(&self) -> HashMap<String, PairingStatus> {
        self.state.lock().unwrap().clone()
    }

    fn set_status(&self, node_id: &str, status: PairingStatus) {
        self.state
            .lock()
            .unwrap()
            .insert(node_id.to_string(), status);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-32-bytes-long-padding!";

    #[test]
    fn generate_and_verify_token() {
        let token = PairingToken::generate(SECRET, "esp32-s3-node-1").unwrap();
        assert_eq!(token.node_id, "esp32-s3-node-1");
        token.verify(SECRET, 300).unwrap();
    }

    #[test]
    fn wrong_secret_fails_verification() {
        let token = PairingToken::generate(SECRET, "node-1").unwrap();
        let result = token.verify("wrong-secret", 300);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("HMAC mismatch"));
    }

    #[test]
    fn expired_token_fails_verification() {
        let mut token = PairingToken::generate(SECRET, "node-1").unwrap();
        // Backdate the timestamp by 10 minutes
        token.timestamp -= 600;
        // Recompute HMAC with the backdated timestamp so HMAC itself is valid
        let message = format!("{}:{}", token.node_id, token.timestamp);
        token.hmac = {
            let mut mac = HmacSha256::new_from_slice(SECRET.as_bytes()).unwrap();
            mac.update(message.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };
        let result = token.verify(SECRET, 300);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[test]
    fn pairing_disabled_trusts_all_nodes() {
        let mgr = NodePairingManager::new(None);
        assert!(!mgr.is_enabled());
        let status = mgr.pair_node("any-node", None);
        assert_eq!(status, PairingStatus::Paired);
        assert!(mgr.is_trusted("any-node"));
    }

    #[test]
    fn pairing_enabled_quarantines_node_without_token() {
        let mgr = NodePairingManager::new(Some(SECRET.to_string()));
        let status = mgr.pair_node("node-1", None);
        assert!(matches!(status, PairingStatus::Quarantined { .. }));
        assert!(!mgr.is_trusted("node-1"));
    }

    #[test]
    fn pairing_enabled_accepts_valid_token() {
        let mgr = NodePairingManager::new(Some(SECRET.to_string()));
        let token = PairingToken::generate(SECRET, "node-1").unwrap();
        let metadata = serde_json::json!({ "pairing_token": token });
        let status = mgr.pair_node("node-1", Some(&metadata));
        assert_eq!(status, PairingStatus::Paired);
        assert!(mgr.is_trusted("node-1"));
    }

    #[test]
    fn revoke_removes_trust() {
        let mgr = NodePairingManager::new(Some(SECRET.to_string()));
        let token = PairingToken::generate(SECRET, "node-1").unwrap();
        let metadata = serde_json::json!({ "pairing_token": token });
        mgr.pair_node("node-1", Some(&metadata));
        assert!(mgr.is_trusted("node-1"));
        mgr.revoke("node-1");
        assert!(!mgr.is_trusted("node-1"));
    }

    #[test]
    fn unpaired_node_is_not_trusted() {
        let mgr = NodePairingManager::new(Some(SECRET.to_string()));
        assert!(!mgr.is_trusted("unknown-node"));
        assert_eq!(mgr.status("unknown-node"), PairingStatus::Unpaired);
    }
}

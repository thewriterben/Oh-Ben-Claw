//! Oh-Ben-Claw Security Subsystem
//!
//! The security subsystem has five components:
//!
//! ## 1. Tool Policy Engine (`policy`)
//! Controls which tools the agent is allowed to invoke and under what conditions.
//! Policies are defined in the configuration file and evaluated before every tool call.
//! A tool call that violates a policy is blocked and logged.
//!
//! ## 2. Node Pairing (`pairing`)
//! Authenticates peripheral nodes before accepting their tool announcements.
//! Each node must present a valid HMAC-SHA256 token derived from a shared secret.
//! Unpaired nodes are quarantined — their announcements are logged but their tools
//! are not registered into the agent's tool registry.
//!
//! ## 3. Secrets Vault (`vault`)
//! Provides encrypted at-rest storage for API keys and other sensitive credentials.
//! Secrets are stored in a SQLite database with AES-256-GCM encryption.
//! The vault is unlocked with a master password derived via Argon2id.
//!
//! ## 4. Safety Limits (`limits`) — Track 0
//! Deterministic, model-independent limits (pin allow-list, value range, rate)
//! for physical actions, enforced before an actuator tool runs. Mirrors the
//! on-MCU `SafetyGate` in the ESP32-S3 firmware.
//!
//! ## 5. Action Audit (`audit`) — Track 0
//! Tamper-evident, hash-chained + HMAC'd append-only log of physical-action
//! decisions; `audit::verify` detects any edit, insertion, deletion, or reorder.

pub mod audit;
pub mod audit_sign;
pub mod limits;
pub mod pairing;
pub mod policy;
pub mod redteam;
pub mod taint;
pub mod trust;
pub mod vault;

#[allow(unused_imports)]
pub use audit::{ActionAuditor, ActionRecord, AuditError, Decision};
#[allow(unused_imports)]
pub use limits::{SafetyConfig, SafetyGate, SafetyLimit, SafetyViolation};
#[allow(unused_imports)]
pub use pairing::{NodePairingManager, PairingStatus};
#[allow(unused_imports)]
pub use policy::{PolicyEngine, ToolPolicy, ToolPolicyAction};
#[allow(unused_imports)]
pub use vault::SecretsVault;

use anyhow::Result;

/// Initialize the security subsystem.
///
/// Individual security components (`PolicyEngine`, `NodePairingManager`, `SecretsVault`)
/// are initialized on demand via `SecurityContext::new`. This function is a no-op and
/// exists for forward-compatibility with future global initialization steps.
pub fn init() {}

/// A security context passed to the agent loop for enforcement.
#[derive(Clone)]
pub struct SecurityContext {
    pub policy: PolicyEngine,
    pub pairing: NodePairingManager,
}

impl SecurityContext {
    /// Create a new security context with the given configuration.
    pub fn new(config: &SecurityConfig) -> Result<Self> {
        Ok(Self {
            policy: PolicyEngine::new(config.policies.clone()),
            pairing: NodePairingManager::new(config.pairing_secret.clone()),
        })
    }
}

/// Security configuration (embedded in the root `Config`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SecurityConfig {
    /// Whether to require node pairing before accepting tool announcements.
    #[serde(default)]
    pub require_pairing: bool,

    /// The shared secret used for HMAC-based node pairing tokens.
    /// Should be a random 32-byte hex string. If not set, pairing is disabled.
    #[serde(default)]
    pub pairing_secret: Option<String>,

    /// Tool execution policies.
    #[serde(default)]
    pub policies: Vec<ToolPolicy>,

    /// Whether to enable the secrets vault.
    #[serde(default)]
    pub vault_enabled: bool,

    /// Path to the vault database file.
    /// Defaults to `~/.oh-ben-claw/vault.db`.
    #[serde(default)]
    pub vault_path: Option<String>,
}

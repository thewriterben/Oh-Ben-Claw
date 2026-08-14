//! Credential resolution at the egress boundary — conscience reach item (b).
//!
//! The reach gate ([`obc_conscience::ReachGate`]) deals only in credential
//! *names*: an allow for a host carries `Some("github")`, never the secret. The
//! secret is resolved here, at the moment of the outbound call, and injected —
//! so a poisoned skill that reads the config exfiltrates the name `"github"`,
//! not the token behind it (the design note in `reach.rs`: "a poisoned skill
//! exfiltrates a name, not a secret").
//!
//! [`CredentialResolver`] is the seam between the name and the secret. The
//! production implementation is the encrypted [`SecretsVault`] via `get_or_env`
//! (vault value first, environment variable as fallback), so an operator can
//! keep credentials in the encrypted store *or* supply them by environment,
//! whichever fits the deployment. Tests use a plain map so the injection logic
//! can be exercised without a vault on disk.
//!
//! **Fail-closed.** A host rule that *names* a credential is declaring that the
//! call needs it. If the name cannot be resolved, the egress tool refuses the
//! call rather than making it unauthenticated — the same posture as the reach
//! gate itself. Sending a request the operator expected to be authenticated,
//! without the credential, would leak the request to the host and is exactly the
//! silent-downgrade failure the conscience layer exists to prevent.

/// Resolves a credential *name* (as carried by
/// [`obc_conscience::ReachDecision::Allow`]) to its secret value.
///
/// `None` means the named credential does not exist — the caller must fail
/// closed (refuse the egress), never proceed unauthenticated.
pub trait CredentialResolver: Send + Sync {
    /// Resolve `name` to its secret, or `None` if there is no such credential.
    fn resolve(&self, name: &str) -> Option<String>;
}

/// The production resolver: the encrypted secrets vault, falling back to the
/// environment variable of the same name (`get_or_env`). A locked vault simply
/// yields the environment value, so env-only deployments work with no unlock.
impl CredentialResolver for obc_safety::SecretsVault {
    fn resolve(&self, name: &str) -> Option<String> {
        // `get_or_env` only errors on a decryption failure (wrong master
        // password); treat that as "unresolved" so a broken vault fails closed
        // rather than panicking at the egress boundary.
        self.get_or_env(name).ok().flatten()
    }
}

/// Environment-only resolver: `std::env::var(name)`. The fallback when no vault
/// is unlocked, so env-backed credentials resolve with no master password —
/// this is the plain form of the env-backed resolution the operator chose.
pub struct EnvResolver;

impl CredentialResolver for EnvResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A map-backed resolver for tests — no vault, no disk, no env.
    struct MapResolver(HashMap<String, String>);

    impl CredentialResolver for MapResolver {
        fn resolve(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    #[test]
    fn resolves_a_known_name() {
        let r = MapResolver(HashMap::from([(
            "github".to_string(),
            "ghp_secret".to_string(),
        )]));
        assert_eq!(r.resolve("github").as_deref(), Some("ghp_secret"));
    }

    #[test]
    fn unknown_name_is_none() {
        let r = MapResolver(HashMap::new());
        assert!(r.resolve("nope").is_none());
    }
}

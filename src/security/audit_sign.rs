//! Ed25519 detached signatures for the Track 0 audit.
//!
//! The existing action audit (`src/security/audit.rs`) is hash-chained and
//! HMAC-SHA256-MAC'd — tamper-evident, but only a *holder of the shared key* can
//! verify it. This module adds the asymmetric half the audit's header pre-scoped:
//! an [`AuditSigner`] (Ed25519 keypair) produces **detached signatures** over
//! arbitrary bytes, and [`verify_hex`] checks them against the **public** key — so
//! any third party can verify the audit's integrity without ever holding the
//! secret (non-repudiation).
//!
//! This is the vetted-crate primitive; wiring it into each `ActionRecord` (an
//! optional `sig` field + a signature-verify pass) is a small follow-up that
//! leaves the chain logic untouched. Uses `ed25519-dalek` v2 — a real, audited
//! implementation, deliberately *not* the SHA3-labelled-as-Kyber stub the
//! cross-pollination analysis flagged.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

/// An Ed25519 signing keypair for the audit. The secret must be stored in the
/// secrets vault; the public key is safe to publish for third-party verification.
pub struct AuditSigner {
    key: SigningKey,
}

impl AuditSigner {
    /// Generate a fresh keypair from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut csprng = OsRng;
        Self { key: SigningKey::generate(&mut csprng) }
    }

    /// Reconstruct a signer from its 32-byte secret key (hex).
    pub fn from_hex(secret_hex: &str) -> anyhow::Result<Self> {
        let bytes = hex::decode(secret_hex)?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("ed25519 secret key must be 32 bytes"))?;
        Ok(Self { key: SigningKey::from_bytes(&arr) })
    }

    /// The 32-byte secret key as hex — store securely (vault).
    pub fn secret_hex(&self) -> String {
        hex::encode(self.key.to_bytes())
    }

    /// The 32-byte public verifying key as hex — safe to publish.
    pub fn public_hex(&self) -> String {
        hex::encode(self.key.verifying_key().to_bytes())
    }

    /// Sign `message`; returns the 64-byte detached signature as hex.
    pub fn sign_hex(&self, message: &[u8]) -> String {
        let sig: Signature = self.key.sign(message);
        hex::encode(sig.to_bytes())
    }
}

/// Verify a detached signature against a public key. No secret is needed, so any
/// third party holding the public key can independently check the audit. Returns
/// `false` for any malformed input rather than erroring.
pub fn verify_hex(public_hex: &str, message: &[u8], signature_hex: &str) -> bool {
    let Ok(pk_bytes) = hex::decode(public_hex) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    let pk_arr: [u8; 32] = match pk_bytes.as_slice().try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let sig_arr: [u8; 64] = match sig_bytes.as_slice().try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let sig = Signature::from_bytes(&sig_arr);
    vk.verify(message, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_round_trip() {
        let signer = AuditSigner::generate();
        let msg = b"audit record 42";
        let sig = signer.sign_hex(msg);
        assert!(verify_hex(&signer.public_hex(), msg, &sig));
    }

    #[test]
    fn a_tampered_message_fails_verification() {
        let signer = AuditSigner::generate();
        let sig = signer.sign_hex(b"original");
        assert!(!verify_hex(&signer.public_hex(), b"tampered", &sig));
    }

    #[test]
    fn a_different_key_fails_verification() {
        let a = AuditSigner::generate();
        let b = AuditSigner::generate();
        let sig = a.sign_hex(b"x");
        assert!(!verify_hex(&b.public_hex(), b"x", &sig));
    }

    #[test]
    fn secret_key_round_trips_through_hex() {
        let signer = AuditSigner::generate();
        let restored = AuditSigner::from_hex(&signer.secret_hex()).unwrap();
        assert_eq!(signer.public_hex(), restored.public_hex());
        // a signature from the restored key verifies against the original public key
        let msg = b"m";
        assert!(verify_hex(&signer.public_hex(), msg, &restored.sign_hex(msg)));
    }

    #[test]
    fn malformed_inputs_do_not_verify() {
        let signer = AuditSigner::generate();
        assert!(!verify_hex("not-hex", b"m", &signer.sign_hex(b"m")));
        assert!(!verify_hex(&signer.public_hex(), b"m", "deadbeef")); // wrong sig length
        assert!(AuditSigner::from_hex("zz").is_err());
    }
}

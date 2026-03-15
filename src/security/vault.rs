//! Encrypted Secrets Vault
//!
//! Provides secure at-rest storage for API keys and other sensitive credentials.
//!
//! # Design
//!
//! - Secrets are stored in a SQLite database at `~/.oh-ben-claw/vault.db`
//! - Each secret is encrypted with AES-256-GCM using a unique random nonce
//! - The encryption key is derived from a master password using Argon2id
//! - The Argon2id salt is stored in the database alongside the encrypted secrets
//! - The vault must be unlocked with the master password before use
//!
//! # Usage
//!
//! ```rust,ignore
//! let vault = SecretsVault::open("/path/to/vault.db")?;
//! vault.unlock("my-master-password")?;
//! vault.set("OPENAI_API_KEY", "sk-...")?;
//! let key = vault.get("OPENAI_API_KEY")?;
//! ```

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{Context, Result};
use argon2::{Argon2, PasswordHasher};
use argon2::password_hash::{rand_core::RngCore, SaltString};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The secrets vault.
#[derive(Clone)]
pub struct SecretsVault {
    db_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
    /// The derived AES-256-GCM key (set after `unlock()`).
    key: Arc<Mutex<Option<[u8; 32]>>>,
}

impl std::fmt::Debug for SecretsVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsVault")
            .field("db_path", &self.db_path)
            .field("locked", &self.key.lock().unwrap().is_none())
            .finish()
    }
}

impl SecretsVault {
    /// Open (or create) the vault database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create vault directory: {:?}", parent))?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open vault database: {:?}", db_path))?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        // Create tables
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vault_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS secrets (
                name       TEXT PRIMARY KEY,
                ciphertext BLOB NOT NULL,
                nonce      BLOB NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );",
        )?;

        Ok(Self {
            db_path,
            conn: Arc::new(Mutex::new(conn)),
            key: Arc::new(Mutex::new(None)),
        })
    }

    /// Unlock the vault with the given master password.
    ///
    /// If the vault is new (no salt stored), a new Argon2id salt is generated
    /// and stored. On subsequent unlocks, the stored salt is used.
    pub fn unlock(&self, master_password: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Get or create the Argon2id salt
        let salt_b64: String = {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT value FROM vault_meta WHERE key = 'argon2_salt'",
                    [],
                    |row| row.get(0),
                )
                .ok();

            match existing {
                Some(s) => s,
                None => {
                    // Generate a new random salt
                    let salt = SaltString::generate(&mut OsRng);
                    let salt_str = salt.as_str().to_string();
                    conn.execute(
                        "INSERT INTO vault_meta (key, value) VALUES ('argon2_salt', ?1)",
                        params![salt_str],
                    )?;
                    salt_str
                }
            }
        };

        // Derive 32-byte key using Argon2id
        let salt = argon2::password_hash::SaltString::from_b64(&salt_b64)
            .map_err(|e| anyhow::anyhow!("Invalid Argon2 salt: {}", e))?;

        let argon2 = Argon2::default();
        let mut key_bytes = [0u8; 32];
        argon2
            .hash_password_into(
                master_password.as_bytes(),
                salt.as_str().as_bytes(),
                &mut key_bytes,
            )
            .map_err(|e| anyhow::anyhow!("Argon2 key derivation failed: {}", e))?;

        *self.key.lock().unwrap() = Some(key_bytes);
        tracing::info!(db = ?self.db_path, "Vault unlocked");
        Ok(())
    }

    /// Lock the vault (clears the in-memory key).
    pub fn lock(&self) {
        *self.key.lock().unwrap() = None;
        tracing::info!(db = ?self.db_path, "Vault locked");
    }

    /// Check whether the vault is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.key.lock().unwrap().is_some()
    }

    /// Store a secret. Overwrites any existing secret with the same name.
    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        let key_bytes = self.require_key()?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, value.as_bytes())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO secrets (name, ciphertext, nonce, updated_at)
             VALUES (?1, ?2, ?3, strftime('%s','now'))
             ON CONFLICT(name) DO UPDATE SET
               ciphertext = excluded.ciphertext,
               nonce      = excluded.nonce,
               updated_at = excluded.updated_at",
            params![name, ciphertext, nonce.as_slice()],
        )?;

        tracing::debug!(name = %name, "Secret stored in vault");
        Ok(())
    }

    /// Retrieve a secret by name. Returns `None` if not found.
    pub fn get(&self, name: &str) -> Result<Option<String>> {
        let key_bytes = self.require_key()?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

        let conn = self.conn.lock().unwrap();
        let result: Option<(Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT ciphertext, nonce FROM secrets WHERE name = ?1",
                params![name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        match result {
            None => Ok(None),
            Some((ciphertext, nonce_bytes)) => {
                let nonce = Nonce::from_slice(&nonce_bytes);
                let plaintext = cipher
                    .decrypt(nonce, ciphertext.as_ref())
                    .map_err(|e| anyhow::anyhow!("Decryption failed for '{}': {}", name, e))?;
                Ok(Some(String::from_utf8(plaintext)?))
            }
        }
    }

    /// Delete a secret by name. Returns `true` if the secret existed.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM secrets WHERE name = ?1", params![name])?;
        Ok(rows > 0)
    }

    /// List all secret names (not their values).
    pub fn list(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM secrets ORDER BY name")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    /// Get a secret, falling back to the environment variable with the same name.
    ///
    /// This is the primary method used by provider adapters to resolve API keys:
    /// vault takes precedence over environment variables.
    pub fn get_or_env(&self, name: &str) -> Result<Option<String>> {
        if self.is_unlocked() {
            if let Some(val) = self.get(name)? {
                return Ok(Some(val));
            }
        }
        Ok(std::env::var(name).ok())
    }

    fn require_key(&self) -> Result<[u8; 32]> {
        self.key
            .lock()
            .unwrap()
            .ok_or_else(|| anyhow::anyhow!("Vault is locked — call unlock() first"))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_vault() -> (SecretsVault, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_vault.db");
        let vault = SecretsVault::open(&path).unwrap();
        (vault, dir)
    }

    #[test]
    fn vault_starts_locked() {
        let (vault, _dir) = open_test_vault();
        assert!(!vault.is_unlocked());
    }

    #[test]
    fn unlock_and_lock_cycle() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        assert!(vault.is_unlocked());
        vault.lock();
        assert!(!vault.is_unlocked());
    }

    #[test]
    fn set_and_get_secret() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        vault.set("OPENAI_API_KEY", "sk-test-1234").unwrap();
        let val = vault.get("OPENAI_API_KEY").unwrap();
        assert_eq!(val, Some("sk-test-1234".to_string()));
    }

    #[test]
    fn get_missing_secret_returns_none() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        let val = vault.get("NONEXISTENT").unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn overwrite_secret() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        vault.set("KEY", "old-value").unwrap();
        vault.set("KEY", "new-value").unwrap();
        assert_eq!(vault.get("KEY").unwrap(), Some("new-value".to_string()));
    }

    #[test]
    fn delete_secret() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        vault.set("KEY", "value").unwrap();
        assert!(vault.delete("KEY").unwrap());
        assert!(vault.get("KEY").unwrap().is_none());
        // Deleting again returns false
        assert!(!vault.delete("KEY").unwrap());
    }

    #[test]
    fn list_secrets() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        vault.set("B_KEY", "b").unwrap();
        vault.set("A_KEY", "a").unwrap();
        let names = vault.list().unwrap();
        assert_eq!(names, vec!["A_KEY", "B_KEY"]);
    }

    #[test]
    fn wrong_password_produces_different_key() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("correct-password").unwrap();
        vault.set("SECRET", "hello").unwrap();
        vault.lock();

        // Unlock with wrong password — key derivation succeeds but decryption fails
        vault.unlock("wrong-password").unwrap();
        let result = vault.get("SECRET");
        assert!(result.is_err());
    }

    #[test]
    fn get_or_env_falls_back_to_env() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        std::env::set_var("TEST_OBC_SECRET_VAR", "env-value");
        let val = vault.get_or_env("TEST_OBC_SECRET_VAR").unwrap();
        assert_eq!(val, Some("env-value".to_string()));
        std::env::remove_var("TEST_OBC_SECRET_VAR");
    }

    #[test]
    fn vault_secret_takes_precedence_over_env() {
        let (vault, _dir) = open_test_vault();
        vault.unlock("master-password").unwrap();
        std::env::set_var("TEST_OBC_SECRET_PRIO", "env-value");
        vault.set("TEST_OBC_SECRET_PRIO", "vault-value").unwrap();
        let val = vault.get_or_env("TEST_OBC_SECRET_PRIO").unwrap();
        assert_eq!(val, Some("vault-value".to_string()));
        std::env::remove_var("TEST_OBC_SECRET_PRIO");
    }
}

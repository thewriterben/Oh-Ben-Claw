//! Personality store — editable SOUL.md and USER.md personality files.
//!
//! Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s approach of
//! storing the agent's personality and user profile as plain Markdown files that
//! anyone can read and edit without touching `config.toml`.
//!
//! # Files
//!
//! | File | Purpose |
//! |------|---------|
//! | `SOUL.md` | Agent personality — overrides `[agent].system_prompt` if present |
//! | `USER.md` | User profile — preferences, name, language, etc. |
//!
//! Both files live in the Oh-Ben-Claw data directory (`~/.oh-ben-claw/`).
//! If a file does not exist the relevant method returns `None`, allowing the
//! caller to fall back to config values.
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::memory::PersonalityStore;
//!
//! let store = PersonalityStore::new();
//!
//! // Use SOUL.md as system prompt if it exists, otherwise use config default.
//! let system_prompt = store.soul()
//!     .unwrap_or_else(|| "You are Oh-Ben-Claw, an AI assistant.".to_string());
//!
//! if let Some(user_profile) = store.user() {
//!     println!("User profile: {user_profile}");
//! }
//! ```

use anyhow::Result;
use directories::ProjectDirs;
use std::path::PathBuf;

// ── PersonalityStore ──────────────────────────────────────────────────────────

/// Reads and writes SOUL.md and USER.md personality files.
pub struct PersonalityStore {
    data_dir: PathBuf,
}

impl PersonalityStore {
    /// Create a `PersonalityStore` that uses the default Oh-Ben-Claw data
    /// directory (`~/.oh-ben-claw/`).
    pub fn new() -> Self {
        let data_dir = Self::default_data_dir().unwrap_or_else(|_| PathBuf::from(".oh-ben-claw"));
        Self { data_dir }
    }

    /// Create a `PersonalityStore` with an explicit base directory (for tests).
    pub fn with_dir(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    // ── Paths ─────────────────────────────────────────────────────────────────

    fn soul_path(&self) -> PathBuf {
        self.data_dir.join("SOUL.md")
    }

    fn user_path(&self) -> PathBuf {
        self.data_dir.join("USER.md")
    }

    fn default_data_dir() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
        Ok(dirs.data_dir().to_path_buf())
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Read the contents of `SOUL.md`, if it exists.
    ///
    /// Returns `Some(content)` when the file is present and readable,
    /// `None` otherwise.
    pub fn soul(&self) -> Option<String> {
        std::fs::read_to_string(self.soul_path()).ok()
    }

    /// Read the contents of `USER.md`, if it exists.
    ///
    /// Returns `Some(content)` when the file is present and readable,
    /// `None` otherwise.
    pub fn user(&self) -> Option<String> {
        std::fs::read_to_string(self.user_path()).ok()
    }

    /// Build a complete system prompt.
    ///
    /// Combines `SOUL.md` (agent personality) with `USER.md` (user profile),
    /// with `fallback_prompt` used when `SOUL.md` does not exist.
    ///
    /// ```text
    /// {SOUL.md or fallback_prompt}
    ///
    /// ## User Profile
    ///
    /// {USER.md}   ← appended only when USER.md exists
    /// ```
    pub fn build_system_prompt(&self, fallback_prompt: &str) -> String {
        let soul = self.soul().unwrap_or_else(|| fallback_prompt.to_string());

        match self.user() {
            Some(user) if !user.trim().is_empty() => {
                format!("{soul}\n\n## User Profile\n\n{user}")
            }
            _ => soul,
        }
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Write new content to `SOUL.md`.  Creates the data directory if needed.
    pub fn write_soul(&self, content: &str) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(self.soul_path(), content)?;
        tracing::info!("SOUL.md updated ({} bytes)", content.len());
        Ok(())
    }

    /// Write new content to `USER.md`.  Creates the data directory if needed.
    pub fn write_user(&self, content: &str) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(self.user_path(), content)?;
        tracing::info!("USER.md updated ({} bytes)", content.len());
        Ok(())
    }
}

impl Default for PersonalityStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (PersonalityStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = PersonalityStore::with_dir(dir.path().to_path_buf());
        (store, dir)
    }

    #[test]
    fn soul_returns_none_when_missing() {
        let (store, _dir) = make_store();
        assert!(store.soul().is_none());
    }

    #[test]
    fn user_returns_none_when_missing() {
        let (store, _dir) = make_store();
        assert!(store.user().is_none());
    }

    #[test]
    fn write_and_read_soul() {
        let (store, _dir) = make_store();
        store.write_soul("You are a friendly robot.").unwrap();
        assert_eq!(store.soul().unwrap(), "You are a friendly robot.");
    }

    #[test]
    fn write_and_read_user() {
        let (store, _dir) = make_store();
        store.write_user("Name: Alice\nLanguage: English").unwrap();
        assert_eq!(store.user().unwrap(), "Name: Alice\nLanguage: English");
    }

    #[test]
    fn build_system_prompt_uses_fallback_when_no_soul() {
        let (store, _dir) = make_store();
        let prompt = store.build_system_prompt("Default prompt.");
        assert_eq!(prompt, "Default prompt.");
    }

    #[test]
    fn build_system_prompt_uses_soul_when_present() {
        let (store, _dir) = make_store();
        store.write_soul("Custom soul.").unwrap();
        let prompt = store.build_system_prompt("Default prompt.");
        assert_eq!(prompt, "Custom soul.");
    }

    #[test]
    fn build_system_prompt_appends_user_profile() {
        let (store, _dir) = make_store();
        store.write_soul("Agent soul.").unwrap();
        store.write_user("Name: Bob").unwrap();
        let prompt = store.build_system_prompt("Default.");
        assert!(prompt.starts_with("Agent soul."));
        assert!(prompt.contains("## User Profile"));
        assert!(prompt.contains("Name: Bob"));
    }

    #[test]
    fn build_system_prompt_skips_empty_user_file() {
        let (store, _dir) = make_store();
        store.write_soul("Soul.").unwrap();
        store.write_user("   \n  ").unwrap();
        let prompt = store.build_system_prompt("Default.");
        assert!(!prompt.contains("## User Profile"));
    }
}

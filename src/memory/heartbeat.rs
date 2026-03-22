//! Heartbeat store — proactive task dispatch from `HEARTBEAT.md`.
//!
//! Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s heartbeat
//! service, which periodically reads a Markdown task file and triggers the
//! agent when actionable items are found.
//!
//! # Concept
//!
//! Place uncompleted tasks in `~/.oh-ben-claw/HEARTBEAT.md`.  The heartbeat
//! service wakes up on a configurable interval (default: 30 minutes), parses
//! the file, and — if any actionable lines are found — injects a prompt into
//! the agent loop so the AI can act on them autonomously.
//!
//! An **actionable line** is any line that is *not*:
//! - Empty or whitespace-only
//! - A Markdown header (`#…`)
//! - A completed checkbox (`- [x]` or `* [x]`)
//!
//! # File format example
//!
//! ```markdown
//! # My Tasks
//!
//! - [ ] Send the weekly report to the team          ← actionable
//! - [x] Order replacement fan for the server room   ← completed, skipped
//! - [ ] Book dentist appointment                    ← actionable
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::memory::HeartbeatStore;
//!
//! let store = HeartbeatStore::new();
//!
//! if store.has_tasks() {
//!     let prompt = store.build_prompt();
//!     // inject `prompt` into the agent loop …
//! }
//! ```

use anyhow::Result;
use directories::ProjectDirs;
use std::path::PathBuf;

/// Default path within the data directory.
const HEARTBEAT_FILE: &str = "HEARTBEAT.md";

/// The prompt injected into the agent when actionable tasks are found.
const HEARTBEAT_PROMPT_TEMPLATE: &str =
    "Read your HEARTBEAT.md task list and follow any instructions or tasks listed there. \
     If nothing needs attention, reply with just: HEARTBEAT_OK";

// ── HeartbeatStore ────────────────────────────────────────────────────────────

/// Reads `HEARTBEAT.md` and detects actionable tasks.
pub struct HeartbeatStore {
    path: PathBuf,
}

impl HeartbeatStore {
    /// Create a `HeartbeatStore` that uses the default Oh-Ben-Claw data
    /// directory (`~/.oh-ben-claw/HEARTBEAT.md`).
    pub fn new() -> Self {
        let path =
            Self::default_path().unwrap_or_else(|_| PathBuf::from(".oh-ben-claw/HEARTBEAT.md"));
        Self { path }
    }

    /// Create a `HeartbeatStore` with an explicit file path (for tests).
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    fn default_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
        Ok(dirs.data_dir().join(HEARTBEAT_FILE))
    }

    // ── Task detection ────────────────────────────────────────────────────────

    /// Return `true` if `HEARTBEAT.md` contains at least one actionable line.
    ///
    /// Returns `false` when the file is missing, empty, or contains only
    /// headers and completed checkboxes.
    pub fn has_tasks(&self) -> bool {
        self.actionable_tasks()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    }

    /// Collect all actionable lines from `HEARTBEAT.md`.
    ///
    /// Returns `Ok(vec![…])` with the stripped line texts, or an empty vec
    /// if there are no actionable tasks.  Returns `Err` only on I/O error.
    pub fn actionable_tasks(&self) -> Result<Vec<String>> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };

        let tasks = content
            .lines()
            .filter(|line| Self::is_actionable(line))
            .map(|line| line.trim().to_string())
            .collect();

        Ok(tasks)
    }

    /// Classify a single line as actionable or not.
    fn is_actionable(line: &str) -> bool {
        let trimmed = line.trim();

        // Empty lines
        if trimmed.is_empty() {
            return false;
        }

        // Markdown headers
        if trimmed.starts_with('#') {
            return false;
        }

        // Completed checkboxes: "- [x]…" or "* [x]…"
        if Self::is_completed_checkbox(trimmed) {
            return false;
        }

        true
    }

    /// Return `true` for `- [x] …` or `* [x] …` (case-insensitive marker).
    fn is_completed_checkbox(trimmed: &str) -> bool {
        let rest = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));

        match rest {
            Some(r) => {
                let inner = r.trim_start();
                inner.starts_with("[x]") || inner.starts_with("[X]")
            }
            None => false,
        }
    }

    // ── Prompt ────────────────────────────────────────────────────────────────

    /// Build the prompt that is injected into the agent loop.
    ///
    /// Returns `None` when there are no actionable tasks.
    pub fn build_prompt(&self) -> Option<String> {
        if self.has_tasks() {
            Some(HEARTBEAT_PROMPT_TEMPLATE.to_string())
        } else {
            None
        }
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Write new content to `HEARTBEAT.md`. Creates parent directories as needed.
    pub fn write(&self, content: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, content)?;
        tracing::info!("HEARTBEAT.md updated ({} bytes)", content.len());
        Ok(())
    }

    /// Append a new uncompleted task to `HEARTBEAT.md`.
    pub fn append_task(&self, task: &str) -> Result<()> {
        use std::io::Write;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "- [ ] {task}")?;
        Ok(())
    }
}

impl Default for HeartbeatStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store_with_content(content: &str) -> (HeartbeatStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("HEARTBEAT.md");
        std::fs::write(&path, content).unwrap();
        (HeartbeatStore::with_path(path), dir)
    }

    fn empty_store() -> (HeartbeatStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("HEARTBEAT.md");
        (HeartbeatStore::with_path(path), dir)
    }

    #[test]
    fn no_file_means_no_tasks() {
        let (store, _dir) = empty_store();
        assert!(!store.has_tasks());
    }

    #[test]
    fn empty_file_means_no_tasks() {
        let (store, _dir) = store_with_content("");
        assert!(!store.has_tasks());
    }

    #[test]
    fn headers_only_means_no_tasks() {
        let (store, _dir) = store_with_content("# My Tasks\n## Sub-section\n");
        assert!(!store.has_tasks());
    }

    #[test]
    fn completed_checkboxes_only_means_no_tasks() {
        let (store, _dir) = store_with_content("- [x] Done\n* [X] Also done\n");
        assert!(!store.has_tasks());
    }

    #[test]
    fn uncompleted_checkbox_is_actionable() {
        let (store, _dir) = store_with_content("- [ ] Buy groceries\n");
        assert!(store.has_tasks());
    }

    #[test]
    fn plain_text_line_is_actionable() {
        let (store, _dir) = store_with_content("Call the dentist\n");
        assert!(store.has_tasks());
    }

    #[test]
    fn mixed_content_detects_actionable() {
        let content = "# Tasks\n\n- [x] Done\n- [ ] Pending\n";
        let (store, _dir) = store_with_content(content);
        let tasks = store.actionable_tasks().unwrap();
        assert_eq!(tasks, vec!["- [ ] Pending"]);
    }

    #[test]
    fn build_prompt_returns_none_when_no_tasks() {
        let (store, _dir) = empty_store();
        assert!(store.build_prompt().is_none());
    }

    #[test]
    fn build_prompt_returns_some_when_tasks_exist() {
        let (store, _dir) = store_with_content("- [ ] Do the thing\n");
        assert!(store.build_prompt().is_some());
    }

    #[test]
    fn append_task_creates_file_and_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("HEARTBEAT.md");
        let store = HeartbeatStore::with_path(path.clone());
        store.append_task("Feed the cat").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("- [ ] Feed the cat"));
    }

    #[test]
    fn write_replaces_file_content() {
        let (store, _dir) = store_with_content("old content");
        store.write("new content").unwrap();
        assert_eq!(store.actionable_tasks().unwrap(), vec!["new content"]);
    }
}

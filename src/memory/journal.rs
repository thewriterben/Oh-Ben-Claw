//! Daily journal — human-readable YYYY-MM-DD.md day notes.
//!
//! Inspired by [MimiClaw](https://github.com/memovai/mimiclaw)'s
//! `memory_append_today` / `memory_read_recent` functions, which complement
//! the SQLite conversation history with plain-text daily notes that users and
//! developers can read and edit directly.
//!
//! # Files
//!
//! All journal files are stored in `~/.oh-ben-claw/journal/` as individual
//! `YYYY-MM-DD.md` files — one per day.  Each file starts with a `# YYYY-MM-DD`
//! header the first time a note is appended on that day.
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::memory::DailyJournal;
//!
//! let journal = DailyJournal::new();
//!
//! // Append a note to today's journal
//! journal.append_today("Deployed firmware v0.3.2 to the kitchen sensor.").unwrap();
//!
//! // Read the last 3 days of notes
//! let recent = journal.read_recent(3).unwrap();
//! println!("{recent}");
//! ```

use anyhow::Result;
use chrono::{Days, Local, NaiveDate};
use directories::ProjectDirs;
use std::path::PathBuf;

// ── DailyJournal ─────────────────────────────────────────────────────────────

/// Writes and reads daily Markdown journal files.
pub struct DailyJournal {
    journal_dir: PathBuf,
}

impl DailyJournal {
    /// Create a `DailyJournal` using the default Oh-Ben-Claw data directory
    /// (`~/.oh-ben-claw/journal/`).
    pub fn new() -> Self {
        let journal_dir =
            Self::default_journal_dir().unwrap_or_else(|_| PathBuf::from(".oh-ben-claw/journal"));
        Self { journal_dir }
    }

    /// Create a `DailyJournal` with an explicit directory (for tests).
    pub fn with_dir(journal_dir: PathBuf) -> Self {
        Self { journal_dir }
    }

    fn default_journal_dir() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
        Ok(dirs.data_dir().join("journal"))
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    fn path_for_date(&self, date: NaiveDate) -> PathBuf {
        self.journal_dir
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    // ── Write ─────────────────────────────────────────────────────────────────

    /// Append a note to today's journal file.
    ///
    /// Creates the journal directory and file if they do not exist.  Adds a
    /// `# YYYY-MM-DD` header automatically when creating a new file.
    pub fn append_today(&self, note: &str) -> Result<()> {
        self.append_on(Self::today(), note)
    }

    /// Append a note to the journal file for a specific date (for testing).
    pub fn append_on(&self, date: NaiveDate, note: &str) -> Result<()> {
        use std::io::Write;

        std::fs::create_dir_all(&self.journal_dir)?;
        let path = self.path_for_date(date);
        let is_new = !path.exists();

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        if is_new {
            writeln!(file, "# {}\n", date.format("%Y-%m-%d"))?;
        }

        writeln!(file, "{note}")?;
        Ok(())
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    /// Read all notes from today's journal file.
    ///
    /// Returns an empty string if today's file does not exist yet.
    pub fn read_today(&self) -> Result<String> {
        self.read_date(Self::today())
    }

    /// Read all notes from a specific date's journal file.
    ///
    /// Returns an empty string if the file does not exist.
    pub fn read_date(&self, date: NaiveDate) -> Result<String> {
        let path = self.path_for_date(date);
        match std::fs::read_to_string(&path) {
            Ok(c) => Ok(c),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Read the last `days` days of journal notes, most recent first.
    ///
    /// Days with no notes are skipped.  Sections are separated by `\n---\n`.
    pub fn read_recent(&self, days: u32) -> Result<String> {
        let today = Self::today();
        let mut sections = Vec::new();

        for i in 0..days {
            let date = today.checked_sub_days(Days::new(i as u64)).unwrap_or(today);
            let content = self.read_date(date)?;
            if !content.trim().is_empty() {
                sections.push(content);
            }
        }

        Ok(sections.join("\n---\n"))
    }

    /// List all journal dates that have notes, in descending order.
    pub fn list_dates(&self) -> Result<Vec<NaiveDate>> {
        if !self.journal_dir.exists() {
            return Ok(vec![]);
        }

        let mut dates = Vec::new();
        for entry in std::fs::read_dir(&self.journal_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(stem) = name_str.strip_suffix(".md") {
                if let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
                    dates.push(date);
                }
            }
        }
        dates.sort_unstable_by(|a, b| b.cmp(a)); // most recent first
        Ok(dates)
    }
}

impl Default for DailyJournal {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_journal() -> (DailyJournal, TempDir) {
        let dir = TempDir::new().unwrap();
        let journal = DailyJournal::with_dir(dir.path().join("journal"));
        (journal, dir)
    }

    fn fixed_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 19).unwrap()
    }

    #[test]
    fn read_date_returns_empty_for_missing_file() {
        let (journal, _dir) = make_journal();
        let content = journal.read_date(fixed_date()).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn append_on_creates_file_with_header() {
        let (journal, _dir) = make_journal();
        journal.append_on(fixed_date(), "First note").unwrap();
        let content = journal.read_date(fixed_date()).unwrap();
        assert!(content.contains("# 2026-03-19"));
        assert!(content.contains("First note"));
    }

    #[test]
    fn append_on_same_day_does_not_duplicate_header() {
        let (journal, _dir) = make_journal();
        journal.append_on(fixed_date(), "Note A").unwrap();
        journal.append_on(fixed_date(), "Note B").unwrap();
        let content = journal.read_date(fixed_date()).unwrap();
        assert_eq!(content.matches("# 2026-03-19").count(), 1);
        assert!(content.contains("Note A"));
        assert!(content.contains("Note B"));
    }

    #[test]
    fn read_recent_combines_multiple_days() {
        let (journal, _dir) = make_journal();
        let today = NaiveDate::from_ymd_opt(2026, 3, 19).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 3, 18).unwrap();
        journal.append_on(today, "Today's note").unwrap();
        journal.append_on(yesterday, "Yesterday's note").unwrap();

        // Read starting from today, look back 2 days
        // We use read_date instead of read_recent to avoid depending on Local::now()
        let t = journal.read_date(today).unwrap();
        let y = journal.read_date(yesterday).unwrap();
        let combined = format!("{t}\n---\n{y}");
        assert!(combined.contains("Today's note"));
        assert!(combined.contains("Yesterday's note"));
    }

    #[test]
    fn list_dates_returns_all_dates_descending() {
        let (journal, _dir) = make_journal();
        let d1 = NaiveDate::from_ymd_opt(2026, 3, 19).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 3, 17).unwrap();
        journal.append_on(d1, "Note").unwrap();
        journal.append_on(d2, "Note").unwrap();
        let dates = journal.list_dates().unwrap();
        assert_eq!(dates, vec![d1, d2]);
    }

    #[test]
    fn list_dates_empty_when_no_journal_dir() {
        let dir = TempDir::new().unwrap();
        let journal = DailyJournal::with_dir(dir.path().join("nonexistent"));
        let dates = journal.list_dates().unwrap();
        assert!(dates.is_empty());
    }
}

//! Bounded, agent-curated notes (parity plan item 4, 2026-09-11).
//!
//! Two small Markdown files the agent maintains through the `memory` tool and
//! is shown at the start of every conversation, the way Hermes keeps
//! `MEMORY.md` and `USER.md`:
//!
//! - `MEMORY.md` — working notes about the machine, the bench, ongoing work
//!   (limit [`MEMORY_LIMIT`] characters);
//! - `USER.md` — who the operator is and how they want things done (limit
//!   [`USER_LIMIT`] characters).
//!
//! The limits are the point. World memory holds facts with provenance and
//! `memory.db` holds every message; these files hold the few sentences worth
//! reading before anything else, and a hard cap forces the agent to curate
//! rather than accumulate. An entry is one line (`- …`); `add` refuses when
//! the file would exceed its limit and says so, and the agent must `replace`
//! or `remove` first. Writes are atomic (temp file + rename), so a reader never
//! sees half a file.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Character cap on `MEMORY.md` (Hermes's figure).
pub const MEMORY_LIMIT: usize = 2_200;
/// Character cap on `USER.md` (Hermes's figure).
pub const USER_LIMIT: usize = 1_375;

/// Which of the two files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Memory,
    User,
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "memory" | "memory.md" | "notes" => Some(Target::Memory),
            "user" | "user.md" | "operator" => Some(Target::User),
            _ => None,
        }
    }
    pub fn file_name(self) -> &'static str {
        match self {
            Target::Memory => "MEMORY.md",
            Target::User => "USER.md",
        }
    }
    pub fn limit(self) -> usize {
        match self {
            Target::Memory => MEMORY_LIMIT,
            Target::User => USER_LIMIT,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Memory => "memory",
            Target::User => "user",
        }
    }
}

/// The two files, under one directory.
pub struct Notes {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl Notes {
    /// The default location: `<data dir>/notes/`.
    pub fn default_dir() -> PathBuf {
        obc_paths::in_data_dir("notes")
    }

    /// Open the notes at `dir`, creating it.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir,
            lock: Mutex::new(()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, t: Target) -> PathBuf {
        self.dir.join(t.file_name())
    }

    /// The file's text, empty when it does not exist yet.
    pub fn read(&self, t: Target) -> String {
        std::fs::read_to_string(self.path(t)).unwrap_or_default()
    }

    /// The entries, in order, without their `- ` marker.
    pub fn entries(&self, t: Target) -> Vec<String> {
        parse_entries(&self.read(t))
    }

    /// Append an entry. Refuses, naming the numbers, when the file would exceed
    /// its limit.
    pub fn add(&self, t: Target, text: &str) -> Result<Usage> {
        let entry = one_line(text)?;
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = self.entries(t);
        entries.push(entry);
        self.write(t, &entries)
    }

    /// Replace entry `index` (1-based).
    pub fn replace(&self, t: Target, index: usize, text: &str) -> Result<Usage> {
        let entry = one_line(text)?;
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = self.entries(t);
        let slot = entries.get_mut(index.wrapping_sub(1)).ok_or_else(|| {
            anyhow::anyhow!("{} has no entry {index} (it has {})", t.file_name(), 0)
        })?;
        *slot = entry;
        self.write(t, &entries)
    }

    /// Remove entry `index` (1-based).
    pub fn remove(&self, t: Target, index: usize) -> Result<Usage> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = self.entries(t);
        if index == 0 || index > entries.len() {
            bail!(
                "{} has no entry {index} (it has {})",
                t.file_name(),
                entries.len()
            );
        }
        entries.remove(index - 1);
        self.write(t, &entries)
    }

    fn write(&self, t: Target, entries: &[String]) -> Result<Usage> {
        let body = render_entries(entries);
        let used = body.chars().count();
        let limit = t.limit();
        if used > limit {
            bail!(
                "{} would be {used}/{limit} characters; replace or remove an entry first",
                t.file_name()
            );
        }
        let path = self.path(t);
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(Usage {
            used,
            limit,
            entries: entries.len(),
        })
    }

    /// What every prompt carries: both files, labelled, or `None` when both are
    /// empty so an unused feature costs nothing.
    pub fn render(&self) -> Option<String> {
        let user = self.entries(Target::User);
        let memory = self.entries(Target::Memory);
        if user.is_empty() && memory.is_empty() {
            return None;
        }
        let mut out = String::from(
            "## Notes\n\nYour own curated notes, kept with the `memory` tool. They are short on purpose.\n",
        );
        if !user.is_empty() {
            out.push_str("\n### About the operator\n");
            out.push_str(&render_entries(&user));
        }
        if !memory.is_empty() {
            out.push_str("\n### Working notes\n");
            out.push_str(&render_entries(&memory));
        }
        Some(out)
    }
}

/// How full a file is after a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: usize,
    pub limit: usize,
    pub entries: usize,
}

fn one_line(text: &str) -> Result<String> {
    let s: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        bail!("an entry needs some text");
    }
    Ok(s)
}

fn parse_entries(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| {
            let l = l.trim_end();
            l.strip_prefix("- ")
                .or_else(|| l.strip_prefix("* "))
                .map(|e| e.trim().to_string())
        })
        .filter(|e| !e.is_empty())
        .collect()
}

fn render_entries(entries: &[String]) -> String {
    let mut s = String::new();
    for e in entries {
        s.push_str("- ");
        s.push_str(e);
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Notes {
        let dir = std::env::temp_dir().join(format!("obc-notes-{}", uuid::Uuid::new_v4()));
        Notes::open(dir).unwrap()
    }

    #[test]
    fn add_replace_remove_round_trip_through_the_file() {
        let n = notes();
        assert!(n.render().is_none(), "nothing yet, nothing rendered");
        n.add(
            Target::User,
            "Name: Benji. Prefers evidence before conclusions.",
        )
        .unwrap();
        n.add(Target::Memory, "The bench GPU is an RTX 5070, 12 GB.")
            .unwrap();
        n.add(
            Target::Memory,
            "Ollama hoists\nsystem messages\tto the top.",
        )
        .unwrap();
        assert_eq!(
            n.entries(Target::Memory)[1],
            "Ollama hoists system messages to the top."
        );
        n.replace(
            Target::Memory,
            1,
            "The bench GPU is an RTX 5070 with 12 GB of VRAM.",
        )
        .unwrap();
        n.remove(Target::Memory, 2).unwrap();
        let reopened = Notes::open(n.dir().to_path_buf()).unwrap();
        assert_eq!(
            reopened.entries(Target::Memory),
            ["The bench GPU is an RTX 5070 with 12 GB of VRAM."]
        );
        let r = reopened.render().unwrap();
        assert!(r.contains("### About the operator\n- Name: Benji."));
        assert!(r.contains("### Working notes\n- The bench GPU"));
        assert!(reopened.remove(Target::Memory, 2).is_err());
        assert!(reopened.replace(Target::User, 0, "x").is_err());
    }

    #[test]
    fn the_limit_is_enforced_and_named() {
        let n = notes();
        let big = "x".repeat(600);
        n.add(Target::User, &big).unwrap();
        let u = n.add(Target::User, &big).unwrap();
        assert_eq!(u.used, 2 * 603);
        let err = n.add(Target::User, &big).unwrap_err().to_string();
        assert!(err.contains("USER.md would be 1809/1375"), "{err}");
        assert_eq!(
            n.entries(Target::User).len(),
            2,
            "the refused entry was not written"
        );
        assert!(
            n.add(Target::Memory, "   ").is_err(),
            "blank entries are refused"
        );
    }

    #[test]
    fn targets_parse_leniently() {
        assert_eq!(Target::parse("USER.md"), Some(Target::User));
        assert_eq!(Target::parse(" memory "), Some(Target::Memory));
        assert_eq!(Target::parse("world"), None);
    }
}

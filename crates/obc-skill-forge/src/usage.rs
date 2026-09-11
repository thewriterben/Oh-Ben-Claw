//! Per-skill usage ledger (parity item 4, 2026-09-11).
//!
//! Until now nothing persistent said whether a learned skill had ever been
//! used: `learned_skill_invocations_total` was one in-memory aggregate, and the
//! rollout tracker only counts staged runs. The curator needs to know which
//! skills earn their context rent, so the agent records every invocation of a
//! forge-managed skill here — count, first seen, last used — in one JSON file
//! beside the rollout record, rewritten on each write like that one.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What is known about one skill's use.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageRecord {
    pub count: u64,
    pub first_seen_ms: u64,
    pub last_used_ms: u64,
}

/// The ledger: one record per skill name.
pub struct UsageLedger {
    path: PathBuf,
    map: Mutex<HashMap<String, UsageRecord>>,
}

impl UsageLedger {
    /// `<data dir>/skill_usage.json`.
    pub fn default_path() -> PathBuf {
        obc_paths::in_data_dir("skill_usage.json")
    }

    /// Load the ledger at `path`; a missing or unreadable file is an empty ledger.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Note one invocation now.
    pub fn record(&self, skill: &str) {
        self.record_at(skill, now_ms());
    }

    /// Note one invocation at `now_ms`.
    pub fn record_at(&self, skill: &str, now_ms: u64) {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        let rec = map.entry(skill.to_string()).or_insert(UsageRecord {
            count: 0,
            first_seen_ms: now_ms,
            last_used_ms: now_ms,
        });
        rec.count += 1;
        rec.last_used_ms = now_ms;
        let snapshot = map.clone();
        drop(map);
        self.persist(&snapshot);
    }

    pub fn get(&self, skill: &str) -> Option<UsageRecord> {
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(skill)
            .copied()
    }

    pub fn all(&self) -> HashMap<String, UsageRecord> {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Forget a skill (after it is removed from the forge).
    pub fn forget(&self, skill: &str) {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if map.remove(skill).is_some() {
            let snapshot = map.clone();
            drop(map);
            self.persist(&snapshot);
        }
    }

    fn persist(&self, map: &HashMap<String, UsageRecord>) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    tracing::warn!(path = ?self.path, error = %e, "skill usage ledger not written");
                }
            }
            Err(e) => tracing::warn!(error = %e, "skill usage ledger not serialised"),
        }
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill_usage.json");
        let ledger = UsageLedger::load(&path);
        ledger.record_at("learned_a", 1_000);
        ledger.record_at("learned_a", 5_000);
        ledger.record_at("learned_b", 2_000);
        assert_eq!(
            ledger.get("learned_a"),
            Some(UsageRecord {
                count: 2,
                first_seen_ms: 1_000,
                last_used_ms: 5_000
            })
        );
        let again = UsageLedger::load(&path);
        assert_eq!(again.all().len(), 2);
        again.forget("learned_b");
        assert!(UsageLedger::load(&path).get("learned_b").is_none());
        assert!(UsageLedger::load(dir.path().join("absent.json"))
            .all()
            .is_empty());
    }
}

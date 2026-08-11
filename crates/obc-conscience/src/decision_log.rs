//! Persistent conscience decision log — the real history `replay` runs on.
//!
//! `obc_conscience::replay` can re-run any recorded decision, but the audit chain
//! records *refusals only* and without the perception confidence, so it isn't
//! enough to replay a perception decision. This is the missing sink: an
//! append-only JSONL file of full [`DecisionRecord`]s (allows and refusals alike,
//! with confidence), stamped with the config fingerprint in effect when the log
//! was opened.
//!
//! Off by default. The runtime opens one only when `OBC_DECISION_LOG` names a
//! path; every perception decision the gate makes is then appended. Read it back
//! with `oh-ben-claw replay-decisions --log <path>` to prove determinism or
//! attribute drift.
//!
//! The format is JSON Lines (one record per line) because that is what an
//! append-only log wants; the replay reader accepts both a JSON array and JSONL.

use crate::{Conscience, ConscienceConfig, DecisionInput, DecisionRecord};
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// An append-only sink for conscience decisions.
pub struct DecisionLog {
    file: Mutex<File>,
    /// Fingerprint of the config in effect when the log was opened — stamped on
    /// every record so replay can tell a policy change from a determinism bug.
    fingerprint: String,
}

impl DecisionLog {
    /// Open (creating/appending) a decision log at `path`, stamping records with
    /// the fingerprint of `config`.
    pub fn open(path: impl AsRef<Path>, config: &ConscienceConfig) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating decision-log dir {parent:?}"))?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening decision log {path:?}"))?;
        Ok(Self {
            file: Mutex::new(file),
            fingerprint: crate::config_fingerprint(config),
        })
    }

    /// Evaluate `input` under `conscience` — the *same* pure call the gate makes —
    /// stamp it with `ts` and the config fingerprint, and append it as one JSONL
    /// record. Best-effort: a write failure is logged, never propagated, because
    /// a full disk must not take down perception (the gate already decided; this
    /// only records it).
    pub fn record(&self, ts: u64, input: DecisionInput, conscience: &Conscience) {
        let verdict = crate::replay::evaluate(&input, conscience);
        let rec = DecisionRecord {
            ts,
            input,
            verdict,
            config_fingerprint: Some(self.fingerprint.clone()),
        };
        match serde_json::to_string(&rec) {
            Ok(line) => {
                let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = writeln!(f, "{line}") {
                    tracing::warn!("decision log write failed: {e}");
                }
            }
            Err(e) => tracing::warn!("decision log serialize failed: {e}"),
        }
    }
}

/// Parse a decision log that is either a JSON array or JSON Lines. The persisted
/// log is JSONL (append-only); a hand-written test log may be a single array.
pub fn parse_log(text: &str) -> Result<Vec<DecisionRecord>> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('[') {
        return serde_json::from_str(text).context("parsing decision log as a JSON array");
    }
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rec: DecisionRecord = serde_json::from_str(line)
            .with_context(|| format!("parsing decision-log line {}", i + 1))?;
        out.push(rec);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConsentRule, Transmit};

    fn armed() -> ConscienceConfig {
        ConscienceConfig {
            enabled: true,
            subjects: vec![ConsentRule::allow("wildlife", 30, Transmit::WeightsOnly)],
            ..Default::default()
        }
    }

    #[test]
    fn logs_a_perception_decision_and_replays_it() {
        let cfg = armed();
        let conscience = Conscience::new(&cfg);
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("obc-declog-{nanos}.jsonl"));

        {
            let log = DecisionLog::open(&path, &cfg).unwrap();
            log.record(
                1,
                DecisionInput::Perception {
                    label: "deer".into(),
                    confidence: 0.9,
                },
                &conscience,
            );
            log.record(
                2,
                DecisionInput::Perception {
                    label: "person".into(),
                    confidence: 0.9,
                },
                &conscience,
            );
        } // drop → flush

        let text = std::fs::read_to_string(&path).unwrap();
        let records = parse_log(&text).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records[0].verdict.allowed, "deer allowed");
        assert!(!records[1].verdict.allowed, "person refused");

        // The persisted log replays cleanly under the same config.
        let report = crate::replay_decisions(&records, &conscience, &cfg);
        assert!(report.all_matched());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_log_accepts_both_array_and_jsonl() {
        let array = r#"[{"ts":1,"input":{"kind":"perception","label":"deer","confidence":0.9},"verdict":{"allowed":true}}]"#;
        assert_eq!(parse_log(array).unwrap().len(), 1);
        let jsonl = "{\"ts\":1,\"input\":{\"kind\":\"reach\",\"tool\":\"http\",\"host\":\"h\"},\"verdict\":{\"allowed\":false}}\n\n{\"ts\":2,\"input\":{\"kind\":\"perception\",\"label\":\"deer\",\"confidence\":0.5},\"verdict\":{\"allowed\":true}}\n";
        assert_eq!(parse_log(jsonl).unwrap().len(), 2);
    }
}

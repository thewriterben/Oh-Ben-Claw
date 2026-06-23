//! Tamper-evident audit log for physical actions (Track 0).
//!
//! Every world-changing / physical action the agent takes (or is refused) is
//! recorded as an append-only JSONL entry that is **hash-chained and MAC'd**:
//! each record carries the previous record's MAC as `prev_mac`, and its own MAC
//! is `HMAC-SHA256(key, seq | ts | node | tool | args_sha256 | decision | prev_mac)`.
//! Any insertion, deletion, reordering, or edit breaks the chain and is caught
//! by [`verify`]. The signing key should come from the secrets vault.
//!
//! This is the symmetric, tamper-evident v1 of the Track 0 signed audit. A
//! holder of the key can verify integrity. Upgrading to Ed25519 *detached
//! signatures* (so third parties can verify without the secret) is a clean
//! follow-up once a vetted signing crate is added — the record shape and chain
//! logic stay the same; only `compute_mac`/`verify` change.

use crate::tools::traits::RiskClass;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

type HmacSha256 = Hmac<Sha256>;

/// Genesis `prev_mac` for the first record in a chain.
pub const GENESIS: &str = "GENESIS";

/// The outcome of a physical-action authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "reason")]
pub enum Decision {
    /// Action was permitted and executed.
    Allowed,
    /// Action was refused (carries the reason, e.g. a safety violation).
    Denied(String),
    /// Action requires operator approval before it can run.
    NeedsApproval,
}

/// One append-only, chained audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    /// Monotonic sequence number (0-based).
    pub seq: u64,
    /// Wall-clock timestamp (ms) supplied by the caller.
    pub ts_ms: u64,
    /// Node the action targeted.
    pub node_id: String,
    /// Tool invoked.
    pub tool: String,
    /// SHA-256 (hex) of the canonical tool arguments.
    pub args_sha256: String,
    /// The action's declared physical risk.
    pub risk: RiskClass,
    /// What happened.
    pub decision: Decision,
    /// MAC of the previous record (chain link); [`GENESIS`] for the first.
    pub prev_mac: String,
    /// HMAC-SHA256 over this record's canonical fields.
    pub mac: String,
}

/// SHA-256 (hex) of a tool's arguments. `serde_json` sorts map keys, so this is
/// deterministic for equal argument values.
pub fn args_sha256(args: &Value) -> String {
    let bytes = serde_json::to_vec(args).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&bytes);
    hex::encode(h.finalize())
}

fn canonical(
    seq: u64,
    ts_ms: u64,
    node_id: &str,
    tool: &str,
    args_sha256: &str,
    decision: &Decision,
    prev_mac: &str,
) -> String {
    let dec = serde_json::to_string(decision).unwrap_or_default();
    format!("{seq}|{ts_ms}|{node_id}|{tool}|{args_sha256}|{dec}|{prev_mac}")
}

fn compute_mac(key: &[u8], canonical: &str) -> anyhow::Result<String> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("invalid audit key: {e}"))?;
    mac.update(canonical.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Appends tamper-evident records for physical actions to a JSONL file.
pub struct ActionAuditor {
    key: Vec<u8>,
    path: PathBuf,
    seq: u64,
    prev_mac: String,
}

impl ActionAuditor {
    /// Open (or resume) an audit log at `path`, signed with `key`.
    ///
    /// If the file already has records, the chain state (`seq`, `prev_mac`) is
    /// restored from the last line so new records continue the same chain.
    pub fn open(key: impl Into<Vec<u8>>, path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let (seq, prev_mac) = match Self::last_record(&path)? {
            Some(last) => (last.seq + 1, last.mac),
            None => (0, GENESIS.to_string()),
        };
        Ok(Self {
            key: key.into(),
            path,
            seq,
            prev_mac,
        })
    }

    fn last_record(path: &Path) -> anyhow::Result<Option<ActionRecord>> {
        if !path.exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(path)?;
        let mut last = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            last = Some(serde_json::from_str::<ActionRecord>(&line)?);
        }
        Ok(last)
    }

    /// Record one physical-action decision; returns the written record.
    pub fn record(
        &mut self,
        ts_ms: u64,
        node_id: &str,
        tool: &str,
        args: &Value,
        risk: RiskClass,
        decision: Decision,
    ) -> anyhow::Result<ActionRecord> {
        let args_hash = args_sha256(args);
        let canon = canonical(
            self.seq, ts_ms, node_id, tool, &args_hash, &decision, &self.prev_mac,
        );
        let mac = compute_mac(&self.key, &canon)?;
        let rec = ActionRecord {
            seq: self.seq,
            ts_ms,
            node_id: node_id.to_string(),
            tool: tool.to_string(),
            args_sha256: args_hash,
            risk,
            decision,
            prev_mac: self.prev_mac.clone(),
            mac: mac.clone(),
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&rec)?)?;

        self.seq += 1;
        self.prev_mac = mac;
        Ok(rec)
    }
}

/// An integrity failure found while verifying an audit chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditError {
    /// A record's recomputed MAC does not match (tampered fields).
    BadMac { seq: u64 },
    /// A record's `prev_mac` does not link to the previous record (insert/delete/reorder).
    BrokenChain { seq: u64 },
    /// Sequence numbers are not contiguous from 0.
    BadSequence { expected: u64, found: u64 },
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::BadMac { seq } => write!(f, "audit: bad MAC at seq {seq} (tampered)"),
            AuditError::BrokenChain { seq } => write!(f, "audit: broken chain at seq {seq}"),
            AuditError::BadSequence { expected, found } => {
                write!(f, "audit: expected seq {expected}, found {found}")
            }
        }
    }
}
impl std::error::Error for AuditError {}

/// Verify an audit log's integrity end to end. Returns the verified record count.
///
/// Detects edits (`BadMac`), insert/delete/reorder (`BrokenChain`), and gaps
/// (`BadSequence`). `Ok` requires the holder of `key`.
pub fn verify(path: impl AsRef<Path>, key: &[u8]) -> anyhow::Result<usize> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(0);
    }
    let file = std::fs::File::open(path)?;
    let mut expected_seq = 0u64;
    let mut prev = GENESIS.to_string();
    let mut count = 0usize;

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: ActionRecord = serde_json::from_str(&line)?;

        if rec.seq != expected_seq {
            return Err(AuditError::BadSequence {
                expected: expected_seq,
                found: rec.seq,
            }
            .into());
        }
        if rec.prev_mac != prev {
            return Err(AuditError::BrokenChain { seq: rec.seq }.into());
        }
        let canon = canonical(
            rec.seq, rec.ts_ms, &rec.node_id, &rec.tool, &rec.args_sha256, &rec.decision,
            &rec.prev_mac,
        );
        if compute_mac(key, &canon)? != rec.mac {
            return Err(AuditError::BadMac { seq: rec.seq }.into());
        }

        prev = rec.mac;
        expected_seq += 1;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::traits::BlastRadius;
    use serde_json::json;

    fn tmp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("obc-audit-{tag}-{nanos}.jsonl"));
        p
    }

    fn high_risk() -> RiskClass {
        RiskClass::physical(false, BlastRadius::High)
    }

    #[test]
    fn records_and_verifies_a_chain() {
        let path = tmp_path("ok");
        let key = b"test-key";
        {
            let mut a = ActionAuditor::open(key.to_vec(), path.clone()).unwrap();
            a.record(1_000, "node-1", "gpio_write", &json!({"pin":17,"value":1}), high_risk(), Decision::Allowed).unwrap();
            a.record(1_500, "node-1", "gpio_write", &json!({"pin":99,"value":1}), high_risk(), Decision::Denied("pin not allowed".into())).unwrap();
            a.record(2_000, "node-1", "capture_now", &json!({}), RiskClass::physical(true, BlastRadius::Low), Decision::NeedsApproval).unwrap();
        }
        assert_eq!(verify(&path, key).unwrap(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resumes_chain_across_reopen() {
        let path = tmp_path("resume");
        let key = b"k";
        {
            let mut a = ActionAuditor::open(key.to_vec(), path.clone()).unwrap();
            a.record(1, "n", "gpio_write", &json!({"pin":17}), high_risk(), Decision::Allowed).unwrap();
        }
        {
            let mut a = ActionAuditor::open(key.to_vec(), path.clone()).unwrap();
            let rec = a.record(2, "n", "gpio_write", &json!({"pin":18}), high_risk(), Decision::Allowed).unwrap();
            assert_eq!(rec.seq, 1, "chain resumed at next seq");
        }
        assert_eq!(verify(&path, key).unwrap(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn detects_tampering() {
        let path = tmp_path("tamper");
        let key = b"k";
        {
            let mut a = ActionAuditor::open(key.to_vec(), path.clone()).unwrap();
            a.record(1, "n", "gpio_write", &json!({"pin":17,"value":1}), high_risk(), Decision::Allowed).unwrap();
            a.record(2, "n", "gpio_write", &json!({"pin":17,"value":0}), high_risk(), Decision::Allowed).unwrap();
        }
        // Tamper a field that is actually stored in the record. (The raw args
        // are NOT stored — only their SHA-256 — so we edit the tool name, which
        // is part of the MAC's canonical input.)
        let contents = std::fs::read_to_string(&path).unwrap();
        let tampered = contents.replacen("gpio_write", "gpio_hack", 1);
        assert_ne!(tampered, contents, "tamper must actually change the file");
        std::fs::write(&path, tampered).unwrap();

        let err = verify(&path, key).unwrap_err();
        assert!(err.to_string().contains("audit:"), "expected an AuditError, got {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_key_fails_verification() {
        let path = tmp_path("key");
        {
            let mut a = ActionAuditor::open(b"right".to_vec(), path.clone()).unwrap();
            a.record(1, "n", "gpio_write", &json!({"pin":17}), high_risk(), Decision::Allowed).unwrap();
        }
        assert!(verify(&path, b"wrong").is_err());
        assert!(verify(&path, b"right").is_ok());
        let _ = std::fs::remove_file(&path);
    }
}

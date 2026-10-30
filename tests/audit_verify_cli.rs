//! `oh-ben-claw audit-verify`, driven as an operator would: the real binary,
//! a real config file, a log written by the real auditor.
//!
//! Until 2026-10-13 nothing outside tests called `audit::verify` or
//! `verify_signatures`, and the signer was never attached, so "signed,
//! tamper-evident audit" had no way to be checked by the person it was for.

use oh_ben_claw::security::audit::{ActionAuditor, Decision};
use oh_ben_claw::security::audit_sign::AuditSigner;
use oh_ben_claw::security::risk::{BlastRadius, RiskClass};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

const KEY: &str = "test-audit-key";

fn scratch(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("obc-audit-verify-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A config whose audit key is explicit, so the binary resolves the same key
/// that wrote the log and touches nothing else on disk.
fn config(dir: &Path, log: &Path) -> PathBuf {
    let cfg = dir.join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[safety]\nenabled = true\naudit_key = \"{KEY}\"\naudit_log_path = \"{}\"\n",
            log.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    cfg
}

fn write_signed_log(log: &Path, signer: AuditSigner) {
    let mut a = ActionAuditor::open(KEY.as_bytes().to_vec(), log)
        .unwrap()
        .with_signer(signer);
    for pin in [17, 18] {
        a.record(
            1,
            "node-1",
            "gpio_write",
            &json!({ "pin": pin }),
            RiskClass::physical(true, BlastRadius::Low),
            Decision::Allowed,
        )
        .unwrap();
    }
}

fn audit_verify(dir: &Path, cfg: &Path, public: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_oh-ben-claw"))
        .args(["--config", cfg.to_str().unwrap(), "audit-verify"])
        .args(["--public-key", public])
        .env("OBC_DATA_DIR", dir)
        .env_remove("OBC_CONFIG")
        .output()
        .unwrap()
}

#[test]
fn a_clean_signed_log_verifies_and_both_kinds_of_tampering_fail() {
    let dir = scratch("cli");
    let log = dir.join("action_audit.jsonl");
    let cfg = config(&dir, &log);
    let signer = AuditSigner::generate();
    let public = signer.public_hex();
    write_signed_log(&log, signer);

    let out = audit_verify(&dir, &cfg, &public);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("chain:      2 record(s)"), "{stdout}");
    assert!(stdout.contains("signatures: 2 record(s)"), "{stdout}");

    // Edit a record's content: the HMAC chain catches it.
    let original = std::fs::read_to_string(&log).unwrap();
    std::fs::write(&log, original.replacen("node-1", "node-9", 1)).unwrap();
    assert!(!audit_verify(&dir, &cfg, &public).status.success());

    // Right log, wrong public key: the signature check catches it.
    std::fs::write(&log, &original).unwrap();
    let stranger = AuditSigner::generate().public_hex();
    let out = audit_verify(&dir, &cfg, &stranger);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("signature"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_log_with_every_signature_stripped_fails() {
    // The signature is outside the MAC'd form, so the chain still passes; only
    // the count of signed records can show that signing was undone.
    let dir = scratch("stripped");
    let log = dir.join("action_audit.jsonl");
    let cfg = config(&dir, &log);
    let signer = AuditSigner::generate();
    let public = signer.public_hex();
    write_signed_log(&log, signer);
    let stripped: String = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            v.as_object_mut().unwrap().remove("sig");
            format!("{v}\n")
        })
        .collect();
    std::fs::write(&log, stripped).unwrap();

    let out = audit_verify(&dir, &cfg, &public);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("none of 2 record(s) is signed"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verifying_without_a_key_mints_none() {
    // No audit_key, no vault, no pairing secret, no key file: `start` would
    // generate one, a verifier must not.
    let dir = scratch("nokey");
    let log = dir.join("action_audit.jsonl");
    write_signed_log(&log, AuditSigner::generate());
    let cfg = dir.join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[safety]\nenabled = true\naudit_log_path = \"{}\"\n",
            log.to_str().unwrap().replace('\\', "/")
        ),
    )
    .unwrap();

    let out = audit_verify(&dir, &cfg, &AuditSigner::generate().public_hex());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("no audit key found"), "{stderr}");
    assert!(!dir.join("action_audit.key").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

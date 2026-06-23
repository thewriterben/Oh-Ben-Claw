//! Emit the canonical Oh-Ben-Claw hardware registry as JSON.
//!
//! This is the single-source-of-truth exporter: it serializes the Rust
//! `peripherals::registry` tables to a stable JSON document that sibling
//! projects (the OBC deployment generator, Accelerapp) consume instead of
//! maintaining their own copies of the hardware catalog.
//!
//! Usage:
//! ```bash
//! # Write directly to a file as UTF-8 (recommended — avoids shell encoding issues):
//! cargo run --bin emit-registry -- registry/registry.json
//!
//! # Or print to stdout (note: PowerShell `>` redirection re-encodes to UTF-16!):
//! cargo run --bin emit-registry
//! ```
//!
//! Passing an output path makes the binary write the bytes itself as UTF-8
//! (no BOM), regardless of the shell — PowerShell's `>` operator otherwise
//! writes UTF-16, which breaks JSON consumers.
//!
//! The JSON shape is `{ schema_version, boards[], accessories[] }`; bump
//! `REGISTRY_SCHEMA_VERSION` in `peripherals::registry` on any breaking change.

use oh_ben_claw::peripherals::registry;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let json = registry::registry_json()?;
    match std::env::args().nth(1) {
        // Write the file ourselves as UTF-8 — never trust the shell's redirection encoding.
        Some(path) => {
            std::fs::write(&path, json.as_bytes())?;
            eprintln!("wrote {} ({} bytes, UTF-8)", path, json.len());
        }
        // No path: emit raw UTF-8 bytes to stdout.
        None => {
            std::io::stdout().write_all(json.as_bytes())?;
            std::io::stdout().write_all(b"\n")?;
        }
    }
    Ok(())
}

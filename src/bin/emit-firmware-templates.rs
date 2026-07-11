//! Emit the canonical Oh-Ben-Claw firmware template set as JSON.
//!
//! Ecosystem Integration I6: one starter sketch per flashable registry board,
//! generated from `deployment::firmware_scaffold` — the single source of
//! truth that the OBC deployment generator and Accelerapp bundle instead of
//! maintaining their own template copies.
//!
//! Usage (same encoding rules as `emit-registry` — pass a path so the binary
//! writes UTF-8 itself; PowerShell `>` redirection would produce UTF-16):
//! ```bash
//! cargo run --bin emit-firmware-templates -- firmware-templates/templates.json
//! ```

use oh_ben_claw::deployment::firmware_scaffold;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let json = firmware_scaffold::templates_json()?;
    match std::env::args().nth(1) {
        Some(path) => {
            std::fs::write(&path, json.as_bytes())?;
            eprintln!("wrote {} ({} bytes, UTF-8)", path, json.len());
        }
        None => {
            std::io::stdout().write_all(json.as_bytes())?;
            std::io::stdout().write_all(b"\n")?;
        }
    }
    Ok(())
}

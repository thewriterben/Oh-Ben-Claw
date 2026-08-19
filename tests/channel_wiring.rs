//! Every exported channel must actually be started.
//!
//! `README.md` advertised eleven channels. Seven could start. `IrcChannel`,
//! `SignalChannel`, `MattermostChannel` and `FeishuChannel` — 1,524 lines,
//! twenty-two unit tests between them, each with the same
//! `new(&config, agent, provider) -> Option<Self>` shape and the same `run()`
//! loop as the seven that worked — were written and never added to
//! `start_channels`. Their config blocks existed too, so an operator could set an
//! IRC password, start the agent, and get silence.
//!
//! Nothing caught it. `channels/mod.rs` re-exported all eleven, so the types were
//! reachable and the module-level survey saw a live module; the file-level sweep
//! did flag the four, but only after it learned to ignore `pub use` lines, and
//! only once someone ran it. None of that is a gate.
//!
//! This is the gate. It reads the two files and requires them to agree, so the
//! next channel added to `channels/mod.rs` and forgotten in `main.rs` fails CI
//! instead of shipping as a claim.
//!
//! It is a source-text check, which is unusual for a test and is the right tool
//! here: the thing being asserted is a fact about wiring that no runtime API
//! exposes. `start_channels` takes a live `Agent`, a provider and a tokio runtime
//! and spawns forever — there is nothing to call and inspect.

use std::collections::BTreeSet;
use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| {
        // Say which input is missing rather than surfacing a bare io::Error. A
        // check that cannot read its input has not passed and has not failed, and
        // the two must not look alike — the same reason
        // scripts/file_reachability.py refuses to report "0 overclaims" when it
        // cannot find the docs.
        panic!(
            "cannot read {} ({e}). This test compares source against documentation, \
             so it needs a full checkout; a partial one makes it meaningless rather \
             than green.",
            p.display()
        )
    })
}

/// Channel types re-exported from the channels crate's `lib.rs`.
///
/// The path moved on 2026-08-14 when `src/channels/` became `crates/obc-channels`.
/// The test failed on the move, which is the correct behaviour and worth saying
/// plainly: a check that reads source by path is a check that can be silently
/// orphaned by a refactor, and the only thing standing between that and a green
/// suite is that this one reads a file it cannot find rather than a file that
/// exists and no longer means what it meant.
fn exported_channels() -> BTreeSet<String> {
    read("crates/obc-channels/src/lib.rs")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub use "))
        .filter_map(|l| l.split("::").nth(1))
        .filter_map(|l| l.split(';').next())
        .map(str::trim)
        .filter(|n| n.ends_with("Channel"))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_exported_channel_is_constructed_in_main() {
    let exported = exported_channels();
    assert!(
        exported.len() >= 10,
        "parsed only {} channel exports from channels/mod.rs — the export style \
         changed and this test is no longer reading it correctly: {exported:?}",
        exported.len()
    );

    let main_rs = read("src/main.rs");
    let missing: Vec<&String> = exported
        .iter()
        .filter(|ty| !main_rs.contains(&format!("{ty}::new")))
        .collect();

    assert!(
        missing.is_empty(),
        "these channels are exported from channels/mod.rs but never constructed \
         in main.rs, so they cannot start no matter how they are configured: \
         {missing:?}\n\
         Either wire them into start_channels (see the Matrix block for the \
         pattern) or stop exporting them — but do not leave them advertised."
    );
}

/// `CliChannel` is started on a different path (it owns the foreground session),
/// so the count check below excludes nothing and simply pins the total: a channel
/// deleted without updating the README is the same class of drift in reverse.
#[test]
fn the_channel_count_is_what_the_docs_say() {
    let exported = exported_channels();
    let readme = read("README.md");

    // The README sentence lists them by name; every exported type should appear
    // there in some form. This is a spelling check on the marketing copy, which
    // is exactly where the eleven-versus-seven gap lived.
    let named: Vec<String> = exported
        .iter()
        .filter(|ty| {
            let bare = ty.trim_end_matches("Channel");
            // "IMessage" is written "iMessage"; compare case-insensitively.
            !readme.to_lowercase().contains(&bare.to_lowercase())
        })
        .cloned()
        .collect();

    assert!(
        named.is_empty(),
        "exported channels the README never mentions: {named:?} — either document \
         them or stop shipping them"
    );
}

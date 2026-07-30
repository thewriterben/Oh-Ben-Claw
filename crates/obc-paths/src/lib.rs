//! Where this instance keeps its data — resolved in exactly one place.
//!
//! # The problem this replaces
//!
//! There were two conventions, running at the same time, disagreeing.
//!
//! `memory.db`, `world.db`, `scheduler.db`, the journal and `HEARTBEAT.md` used
//! `ProjectDirs::data_dir()` — platform-correct, and on Linux that is
//! `~/.local/share/oh-ben-claw/`. Meanwhile the approval grants, the harness records,
//! the skill-evolution log, the skill install audit, the rollout record and the
//! gateway's `incoming/` directory hardcoded `$HOME/.oh-ben-claw`. Every doc comment
//! in the crate said `~/.oh-ben-claw/...`, which described neither arrangement on
//! Windows and only half of it on Linux.
//!
//! So a user's data was split across two directories, the documentation named a
//! third, and there was **no way to move any of it**. `OBC_CONFIG` could relocate the
//! config file; nothing could relocate the data. Two agents on one host silently
//! shared one `memory.db`, one audit chain and one set of approval grants — which is
//! not a multi-tenancy limitation so much as data corruption waiting for a second
//! process.
//!
//! # The rule
//!
//! One root, resolved once, in this order:
//!
//! 1. `OBC_DATA_DIR` — an environment variable, because that is what a service
//!    manager, a container and a second instance on the same laptop can all set.
//! 2. `[paths].data_dir` in the config file — a stated intention, honoured over the
//!    default but not over an explicit environment override for this run.
//! 3. `ProjectDirs` for the platform. Not `~/.oh-ben-claw`: a dotdir in `$HOME` is a
//!    Linux habit that is wrong on Windows and macOS, and this project has users on
//!    all three.
//!
//! Everything else is a subpath of that root, so "where is my data" has one answer
//! and moving an instance is one variable.
//!
//! This does not make the agent multi-tenant, and nothing here pretends it does.
//! What it does is stop the single-tenant assumption from being welded into a dozen
//! call sites, so that when a hosted deployment needs per-tenant isolation the change
//! is a resolver, not an excavation.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variable that relocates the entire data root for one run.
pub const DATA_DIR_VAR: &str = "OBC_DATA_DIR";

/// `[paths].data_dir`, published once by the process that loaded the config.
///
/// A process-wide cell rather than a parameter threaded through twelve call sites,
/// because most of those sites are `default_*()` associated functions that never see
/// a `Config` and never will. The alternative — half the paths honouring the config
/// key and half not — is the exact split this module exists to end.
static CONFIGURED: OnceLock<PathBuf> = OnceLock::new();

/// Publish `[paths].data_dir`. Called once, early, by whoever loaded the config.
///
/// Returns `false` if a value was already published, in which case the existing one
/// stands. Silently taking the last writer would make the data location depend on
/// startup ordering, which is a genuinely horrible thing to debug.
pub fn set_configured(dir: impl Into<PathBuf>) -> bool {
    let dir = dir.into();
    if dir.as_os_str().is_empty() {
        return false;
    }
    CONFIGURED.set(dir).is_ok()
}

/// The platform default, used when neither the environment nor the config says
/// otherwise.
///
/// `None` only when the platform cannot say where a home directory is. Callers treat
/// that as "write beside me", not as fatal: an agent that refuses to start because it
/// cannot find `$HOME` is less useful than one that writes to the working directory
/// and says where.
pub fn platform_default() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "thewriterben", "oh-ben-claw")
        .map(|d| d.data_dir().to_path_buf())
}

/// The data root for this instance.
pub fn data_dir() -> PathBuf {
    resolve(CONFIGURED.get().map(|p| p.as_path()))
}

/// The resolution itself, with the configured value passed in — the testable half.
///
/// The environment wins because it is the per-run knob: a config file gets checked
/// into a repository and copied between machines, and an operator starting a second
/// instance should not have to edit one to do it.
pub fn resolve(configured: Option<&Path>) -> PathBuf {
    if let Ok(v) = std::env::var(DATA_DIR_VAR) {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = configured.filter(|p| !p.as_os_str().is_empty()) {
        return v.to_path_buf();
    }
    platform_default().unwrap_or_else(|| PathBuf::from("."))
}

/// A named file or directory inside the data root, with the root created.
///
/// A failure to create the directory is not reported here: the caller is about to
/// open the file and will produce a better error than "could not create directory"
/// ever could, because it knows what it was trying to do.
pub fn in_data_dir(name: impl AsRef<Path>) -> PathBuf {
    let root = data_dir();
    let _ = std::fs::create_dir_all(&root);
    root.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate process environment, so they serialise behind one mutex
    // rather than running in parallel like everything else. `cargo test` uses
    // threads, and a sibling test clearing OBC_DATA_DIR mid-assertion is exactly the
    // kind of flake that gets a test marked #[ignore] a year later.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_var<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(DATA_DIR_VAR).ok();
        match value {
            Some(v) => std::env::set_var(DATA_DIR_VAR, v),
            None => std::env::remove_var(DATA_DIR_VAR),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var(DATA_DIR_VAR, v),
            None => std::env::remove_var(DATA_DIR_VAR),
        }
        out
    }

    #[test]
    fn the_environment_wins_over_the_config_file() {
        // The case this exists for: a second instance on a machine that already has a
        // config, started without editing it.
        with_var(Some("/srv/obc/tenant-b"), || {
            let got = resolve(Some(Path::new("/srv/obc/tenant-a")));
            assert_eq!(got, PathBuf::from("/srv/obc/tenant-b"));
        });
    }

    #[test]
    fn the_config_file_wins_over_the_platform_default() {
        with_var(None, || {
            assert_eq!(
                resolve(Some(Path::new("/srv/obc/one"))),
                PathBuf::from("/srv/obc/one")
            );
        });
    }

    #[test]
    fn an_empty_setting_is_not_a_setting() {
        // `export OBC_DATA_DIR=` is a common way to end up with a set-but-empty
        // variable, and honouring it would put the databases in the filesystem root.
        with_var(Some("   "), || {
            assert_eq!(
                resolve(Some(Path::new("/srv/obc/one"))),
                PathBuf::from("/srv/obc/one")
            );
        });
        with_var(None, || {
            let fallback = platform_default().unwrap_or_else(|| PathBuf::from("."));
            assert_eq!(resolve(Some(Path::new(""))), fallback);
        });
    }

    #[test]
    fn the_default_is_the_platform_convention_not_a_home_dotdir() {
        // `~/.oh-ben-claw` is a Linux habit. It was hardcoded in six places and was
        // wrong on Windows and macOS in all six.
        with_var(None, || {
            let d = resolve(None);
            assert!(
                !d.ends_with(".oh-ben-claw"),
                "resolved to a home dotdir: {d:?}"
            );
            assert_eq!(Some(d), platform_default());
        });
    }

    #[test]
    fn everything_lands_under_one_root() {
        // A real temp directory, because `in_data_dir` creates the root — a test that
        // quietly makes `C:\srv` is a bad neighbour even when it passes.
        let root = std::env::temp_dir().join("obc-paths-root-test");
        with_var(Some(&root.to_string_lossy()), || {
            for name in [
                "memory.db",
                "world.db",
                "vault.db",
                "journal",
                "approval_grants.json",
            ] {
                assert!(
                    in_data_dir(name).starts_with(&root),
                    "{name} escaped the data root"
                );
            }
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_first_published_config_value_stands() {
        // Startup ordering must not decide where the database lives.
        //
        // This writes a process-global that no later test can undo, so it is the only
        // test that touches it and it points at a temp path: if some other test in
        // this binary later resolves a default, it lands somewhere harmless.
        let first = std::env::temp_dir().join("obc-paths-published");
        assert!(set_configured(first.clone()));
        assert!(!set_configured(
            std::env::temp_dir().join("obc-paths-second")
        ));
        assert!(!set_configured(""));
        assert_eq!(CONFIGURED.get(), Some(&first));
    }
}

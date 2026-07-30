//! The hardware planner: registry → inventory → deployment scheme, plus the
//! geometry and site optimizer the scheme is placed with.
//!
//! # What this crate is for
//!
//! Given a pile of boards and a list of things an operator wants the system to
//! do, produce a deployment: which board takes which role, what capabilities are
//! satisfied, what hardware is missing, where the nodes physically go, and a
//! paste-ready configuration file. It is pure — no I/O, no async, no providers —
//! which is what lets the same source compile to WebAssembly and run in a phone
//! app.
//!
//! # Why it is a crate
//!
//! `planner-wasm` used to compile these files verbatim out of the agent's `src/`
//! tree with `#[path]` attributes. That worked, and it proved something valuable:
//! rustc will not link a crate whose sources reach outside it, so the WASM build
//! succeeding *was* a proof that this closure was self-contained — a stronger
//! statement than the name-based reachability surveys in `scripts/` can make.
//!
//! It also broke. `#[path]` inside an inline `mod` block resolves against a
//! directory named for the module; Windows normalises `..` components lexically
//! and resolved it anyway, Linux does not and did not. The build was green on the
//! author's machine and red on every CI push — the worst shape a build failure
//! can have, because the one person who can fix it is the one person who cannot
//! see it. A real crate with a real dependency removes the entire class.
//!
//! # The one edge
//!
//! Measured at extraction, the whole closure had exactly one reference to the
//! rest of the agent: `geo::anchor` reaching for world memory. It is behind the
//! `world-anchor` feature and resolves to the `obc-memory` crate, which was
//! migrated first. Everything else here refers only to itself.
//!
//! # The cross-repository contract
//!
//! [`deployment::HardwareInventory::to_deployment_toml`] and its inverse
//! [`deployment::HardwareInventory::from_deployment_config`] define the
//! `[deployment]` schema. OBC-deployment-generator's TypeScript emitter must
//! produce byte-identical output for the same inventory, and OBC-Prime hashes the
//! shared golden fixtures across three repositories. Changing the emitted text is
//! a cross-repository event, not a local edit.

pub mod config;
pub mod deployment;
pub mod geo;
pub mod peripherals;
pub mod siteplan;

/// Read a repository-root file by walking up from this crate's manifest
/// directory. Test-only.
///
/// Two drift guards in this crate — `committed_registry_json_is_current` and
/// `committed_templates_json_is_current` — compare the live tables against the
/// exports committed at the repository root, and those exports are consumed by
/// two other repositories. They are the reason the registry cannot silently fork
/// from `registry.json`, and they caught genuine drift the first time they ran.
///
/// They used to locate the file with a hardcoded list of two candidate paths:
/// the manifest dir, and one level up for when these sources were compiled into
/// `planner-wasm`. Extracting this crate put them two levels down and broke
/// both. A list of literal `..` prefixes is a thing that breaks every time the
/// file moves, and it had already been extended once.
///
/// So: walk up. The bound is small and the miss is loud — `None` reaches an
/// `expect` that names the generator command, so a guard that cannot find its
/// input fails rather than passing on an empty comparison. That distinction is
/// the whole point of these two tests.
#[cfg(test)]
pub(crate) fn find_up(rel: &str) -> Option<String> {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..6 {
        if let Ok(s) = std::fs::read_to_string(dir.join(rel)) {
            return Some(s);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

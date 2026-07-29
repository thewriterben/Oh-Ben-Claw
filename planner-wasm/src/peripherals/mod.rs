//! Shim: the host crate's board registry, compiled verbatim.
//!
//! This is a **real directory with a real `mod.rs`**, and that is load-bearing.
//!
//! When `#[path]` sits inside an *inline* `mod peripherals { … }` block, rustc
//! resolves it against `<dir of the enclosing file>/peripherals/` — a directory that
//! did not exist. Windows normalises the `..` components lexically before touching
//! the filesystem, so the path resolved anyway and the crate built. Linux requires
//! every component of a path to exist, so `cargo build --workspace` failed there
//! with:
//!
//! ```text
//! error: couldn't read `planner-wasm/src/peripherals/../../../src/peripherals/registry.rs`
//! ```
//!
//! CI runs on `ubuntu-latest`. This was failing on every push while the same command
//! was green on the author's machine — the worst shape a build failure can have,
//! because the person who can fix it is the one person who cannot see it.
//!
//! With the directory real, the identical `#[path]` string resolves on both.

#[path = "../../../src/peripherals/registry.rs"]
pub mod registry;

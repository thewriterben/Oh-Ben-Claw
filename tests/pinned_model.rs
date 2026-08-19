//! One pinned model identifier, in one place.
//!
//! This is the coupling that keeps going wrong. The same model string has to
//! appear in `config.example.toml`, in the config the deployment planner emits,
//! and in the reference bodies — and every previous update changed some of
//! them. A stale copy is not a cosmetic problem: it is the value a new user is
//! told to uncomment.
//!
//! It lived in `src/config/first_run.rs`'s test module until 2026-08-13, where
//! it named `crate::deployment` twice. Those two lines were the entire
//! `config -> deployment` edge, and `deployment -> config` is real, so a mutual
//! pair in the dependency graph — one of three the core had left — was being
//! held open by a test asserting that two strings match.
//!
//! Sixth time today that a cross-layer claim turned out to be filed under one
//! of the layers it spans. The claim is unchanged; only the file is.

use oh_ben_claw::config::first_run::pinned_model;
use oh_ben_claw::deployment::inventory::HardwareInventory;
use oh_ben_claw::deployment::planner::DeploymentPlanner;

#[test]
fn the_pinned_model_appears_everywhere_it_should() {
    let pinned = pinned_model("anthropic").expect("an anthropic candidate");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let example =
        std::fs::read_to_string(root.join("config.example.toml")).expect("config.example.toml");
    assert!(
        example.contains(&format!("model = \"{pinned}\"")),
        "config.example.toml pins a different model than first_run"
    );

    // The planner's emitted config offers the same identifier to uncomment.
    let inv = HardwareInventory::nanopi_scenario();
    let emitted = DeploymentPlanner::plan(&inv).config_toml;
    assert!(
        emitted.contains(&format!("# model = \"{pinned}\"")),
        "the generated config suggests a different model than first_run"
    );
}

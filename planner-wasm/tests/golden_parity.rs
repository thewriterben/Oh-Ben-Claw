//! The JSON API this crate exports to JavaScript, compared to the goldens whole.
//!
//! # Why this file exists
//!
//! The four tests inside `src/lib.rs` check the API's *shape*: the TOML
//! `starts_with("[deployment]\n")`, the registry JSON `contains("esp32-s3")`,
//! the assignment list has at least two entries. Every one of them would pass on
//! output that was subtly, ruinously wrong — which is not a hypothetical here.
//! `OBC-deployment-generator/tests/wasm-planner.test.ts` asserted
//! `toContain("[peripherals]")` while its sibling compared the whole file, and
//! the narrow assertion sat directly beneath a comment explaining why narrow
//! assertions are dangerous. The vendored WASM bundle it was guarding had been
//! emitting a 79-line config against a 119-line golden for six weeks.
//!
//! So this compares the whole artifact, byte for byte, against the same fixtures
//! the Rust and TypeScript suites use. The goldens are the contract; anything
//! less than the whole file is an opinion about which parts of the contract
//! matter.
//!
//! # Where it came from
//!
//! An earlier version of this test existed as `shim_golden.rs` and was **never
//! committed** — it had hardcoded paths from the container it was written in and
//! ran nowhere else. It passed in every local verification run for a day and had
//! never once run in CI, which is the same failure as a `cargo fmt --all`
//! immediately followed by `cargo fmt --check`: a green result that reached
//! nobody. Committed properly now, with paths that resolve from the manifest
//! directory.
//!
//! # What this does not replace
//!
//! `parity/verify_wasm.cjs` in OBC-Prime executes the actual `.wasm` bundle
//! under Node. This runs the same Rust natively. Both are needed: this catches a
//! logic change, that one catches a bundle built from stale sources — which
//! hashes structurally cannot see.

use obc_planner_wasm::api;

fn fixture(rel: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| {
        // A missing fixture must fail, never silently skip. This suite exists to
        // compare against goldens; with no goldens it is not passing, it is not
        // running.
        panic!("fixture {} unreadable ({e})", p.display())
    })
}

/// Trailing-whitespace- and line-ending-insensitive, nothing else. The repo
/// carries `.gitattributes` precisely so CRLF is not a real difference.
fn norm(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

#[test]
fn plan_deployment_reproduces_the_config_golden_whole() {
    let out = api::plan_deployment(&fixture("deployment/nanopi/inventory.json"))
        .expect("plan_deployment on the shared inventory fixture");
    let v: serde_json::Value = serde_json::from_str(&out).expect("api returns JSON");
    let got = v["config_toml"].as_str().expect("config_toml is a string");
    let want = fixture("deployment/nanopi/expected-config.toml");

    assert_eq!(
        norm(got),
        norm(&want),
        "the WASM crate's planner output no longer matches expected-config.toml. \
         That golden is mirrored into OBC-deployment-generator and hashed by \
         OBC-Prime, so a deliberate change here is a three-repository change; \
         re-bless with OBC_BLESS=1 only after deciding it is one."
    );
}

#[test]
fn deployment_toml_reproduces_its_golden_whole() {
    let got = api::deployment_toml(&fixture("deployment/nanopi/inventory.json"))
        .expect("deployment_toml on the shared inventory fixture");
    let want = fixture("deployment/nanopi/expected-deployment.toml");
    assert_eq!(norm(&got), norm(&want), "[deployment] block drifted");
}

#[test]
fn plan_site_reproduces_the_siteplan_golden_whole() {
    let out = api::plan_site(&fixture("siteplan/square/case.json")).expect("plan_site");
    let v: serde_json::Value = serde_json::from_str(&out).expect("api returns JSON");
    let got = v["toml"].as_str().expect("toml is a string");
    let want = fixture("siteplan/square/expected-site.toml");
    assert_eq!(norm(got), norm(&want), "[site] layout drifted");
}

/// The control. Every assertion above is an equality against a file, and a
/// fixture that silently read as empty would make three of them compare "" to ""
/// and pass. This one fails if the fixtures are not what they should be.
#[test]
fn proof_the_goldens_are_hot() {
    let cfg = fixture("deployment/nanopi/expected-config.toml");
    let dep = fixture("deployment/nanopi/expected-deployment.toml");
    let site = fixture("siteplan/square/expected-site.toml");

    assert!(
        cfg.lines().count() > 80,
        "expected-config.toml is {} lines; it was 119 when this was written, and \
         the bug that prompted all of this produced 79",
        cfg.lines().count()
    );
    assert!(
        dep.contains("[[deployment.hardware]]"),
        "deployment golden is not a deployment block"
    );
    assert!(
        site.contains("[[site.node]]"),
        "siteplan golden has no nodes"
    );
}

//! Every config this repository ships must parse and pass validation.
//!
//! `examples/config-cutting-edge.toml` shipped `[gateway] enabled = true`,
//! `host = "0.0.0.0"` and `api_token` commented out. As documentation that was
//! already advice to publish an endpoint that can run `shell` to the whole
//! network. On 2026-08-18 the config layer started refusing exactly that
//! combination, which turned a bad example into a broken one — and nothing
//! would have noticed, because no test had ever loaded it.
//!
//! Two tests in this repository read `config.example.toml`, and both read it as
//! *text*, to check that a pinned model string and a commented policy block
//! match code. Neither parses it. A file can be grepped successfully and still
//! not load.
//!
//! This is the cheap general version: parse it, validate it, fail loudly.
//! Warnings are allowed — an example may legitimately demonstrate a setting
//! that warns — but a hard validation error means the example cannot be used
//! for the thing it is an example of.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn shipped_configs() -> Vec<PathBuf> {
    let mut out = vec![root().join("config.example.toml")];
    let dir = root().join("examples");
    let mut examples: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("examples/ must exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    examples.sort();
    out.extend(examples);
    out
}

fn load(path: &Path) -> oh_ben_claw::config::Config {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()))
}

#[test]
fn every_shipped_config_parses_and_validates() {
    let configs = shipped_configs();
    // Discovery is by directory walk, so an empty list would pass this test
    // while checking nothing — the failure mode these surveys keep finding.
    assert!(
        configs.len() >= 4,
        "expected config.example.toml plus the examples/, found {configs:?}"
    );

    for path in configs {
        let config = load(&path);
        match config.validate() {
            Ok(warnings) => {
                for w in warnings {
                    println!("{}: warning: {w}", path.display());
                }
            }
            Err(e) => panic!(
                "{} is shipped as an example and does not load: {e}",
                path.display()
            ),
        }
    }
}

#[test]
fn no_shipped_config_publishes_an_unauthenticated_gateway() {
    // The narrower claim, stated separately so it survives someone deciding
    // that validation should be lenient. An example is a recommendation.
    for path in shipped_configs() {
        let config = load(&path);
        if config.gateway.enabled && !oh_ben_claw::config::is_loopback_host(&config.gateway.host) {
            assert!(
                config.gateway.api_token.is_some(),
                "{} enables the gateway on {} with no api_token",
                path.display(),
                config.gateway.host
            );
        }
    }
}

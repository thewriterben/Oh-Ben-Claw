//! The config the deployment planner tells you to paste must actually load.
//!
//! `DeploymentScheme::config_toml` is documented as "Ready-to-use TOML
//! configuration for `~/.oh-ben-claw/config.toml`", and the generator app hands
//! it to a user to paste. The only test on it asserted that the string
//! `contains("[peripherals]")`.
//!
//! `contains` is not parsing. Three of the four configs shipped in this
//! repository turned out not to load — one invalid TOML since it was written,
//! two failing validation — and every one of them would have passed a
//! `contains` check for its own section headers. This is the same question
//! asked of generated output rather than committed files, and it matters more:
//! a bad example wastes someone's afternoon, a bad generated config is what the
//! planner told them to run.

use oh_ben_claw::config::Config;
use oh_ben_claw::obc_planner::deployment::{DeploymentPlanner, HardwareInventory};

fn parse(toml_text: &str, label: &str) -> Config {
    toml::from_str(toml_text)
        .unwrap_or_else(|e| panic!("planner output for {label} is not valid TOML: {e}"))
}

fn scenarios() -> Vec<(&'static str, HardwareInventory)> {
    vec![
        ("nanopi_scenario", HardwareInventory::nanopi_scenario()),
        // The degenerate case, included on purpose: the planner has a documented
        // "empty inventory produces warnings" path, and a config emitted down
        // that path is still a config someone could be handed.
        ("empty", HardwareInventory::new("empty")),
    ]
}

#[test]
fn planner_output_parses_and_validates() {
    for (label, inv) in scenarios() {
        let scheme = DeploymentPlanner::plan(&inv);
        assert!(
            !scheme.config_toml.trim().is_empty(),
            "{label}: planner emitted an empty config, which would pass every \
             assertion below by vacuum"
        );

        let config = parse(&scheme.config_toml, label);
        match config.validate() {
            Ok(warnings) => {
                for w in warnings {
                    println!("{label}: warning: {w}");
                }
            }
            Err(e) => panic!(
                "planner output for {label} parses but does not validate: {e}\n\
                 --- emitted config ---\n{}",
                scheme.config_toml
            ),
        }
    }
}

#[test]
fn planner_output_does_not_publish_an_unauthenticated_gateway() {
    // The planner writes a config for a real device on a real network. If it
    // ever starts enabling the gateway, it has to bring a token or stay on
    // loopback — the rule the config layer now enforces, asserted at the point
    // that generates configs rather than only at the point that loads them.
    for (label, inv) in scenarios() {
        let scheme = DeploymentPlanner::plan(&inv);
        let config = parse(&scheme.config_toml, label);
        if config.gateway.enabled && !oh_ben_claw::config::is_loopback_host(&config.gateway.host) {
            assert!(
                config.gateway.api_token.is_some(),
                "{label}: planner emitted an enabled gateway on {} with no api_token",
                config.gateway.host
            );
        }
    }
}

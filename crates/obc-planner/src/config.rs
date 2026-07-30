//! The `[deployment]` configuration block — the planner's own output schema.
//!
//! These types live beside [`crate::deployment::HardwareInventory`] rather than
//! in the agent's `config` module, because they are one half of a round trip
//! whose other half is `to_deployment_toml`. Keeping the writer and the reader in
//! one crate is what lets the contract be stated as a fixed point:
//!
//! ```text
//! inventory → emit → parse → rebuild → emit   ==   inventory → emit
//! ```
//!
//! Before the extraction they were in `src/config/mod.rs`, and
//! `from_deployment_config` had to sit in `deployment/mod.rs` rather than next to
//! its inverse, because `inventory.rs` was compiled verbatim into a crate that
//! had no `config` module and a `crate::config::` reference there broke the WASM
//! build. That awkwardness was a symptom of the split this crate removes.
//!
//! The agent re-exports both types from `oh_ben_claw::config`, so no call site
//! changed.

use serde::{Deserialize, Serialize};

/// Configuration for a single hardware item in a deployment scenario.
///
/// Used inside `DeploymentConfig.hardware` to describe every board or
/// accessory that is part of the deployment.
///
/// ```toml
/// [[deployment.hardware]]
/// name       = "nanopi-neo3"
/// board_name = "nanopi-neo3"
/// transport  = "native"
/// role       = "host"
/// accessories = ["dht22"]
///
/// [[deployment.hardware]]
/// name       = "xiao-esp32s3-sense"
/// board_name = "xiao-esp32s3-sense"
/// transport  = "serial"
/// role       = "vision"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentHardwareConfig {
    /// Human-readable label for this item.
    pub name: String,
    /// Board registry name (e.g. `"nanopi-neo3"`, `"xiao-esp32s3-sense"`).
    pub board_name: String,
    /// Transport type: `"native"`, `"serial"`, `"mqtt"`.
    pub transport: String,
    /// Serial device path (for serial transport).
    #[serde(default)]
    pub path: Option<String>,
    /// MQTT node ID (for mqtt transport).
    #[serde(default)]
    pub node_id: Option<String>,
    /// Operator-assigned role: `"host"`, `"display"`, `"vision"`, `"listening"`,
    /// `"sensing"`, `"peripheral"`, `"console"`.  Leave empty for
    /// auto-assignment. Unrecognised text is treated as empty rather than
    /// rejected — see `ItemRole::from_str`.
    #[serde(default)]
    pub role: String,
    /// Accessory names attached to this board (e.g. `["dht22"]`).
    #[serde(default)]
    pub accessories: Vec<String>,
}

/// Configuration for the deployment scheme generator (Phase 13).
///
/// This block is **written by a planner and read by the agent**, which is
/// unusual for a config section and is the thing to hold on to when reading it.
/// `DeploymentPlanner` (and the TypeScript emitter in
/// OBC-deployment-generator, byte-for-byte) produces it from a hardware
/// inventory; `HardwareInventory::from_deployment_config` turns it back into
/// that inventory. The two directions are pinned as a fixed point by
/// `tests/deployment_config_roundtrip.rs` and by the shared golden fixtures in
/// `tests/fixtures/deployment/`.
///
/// ```toml
/// [deployment]
/// enabled = true
/// scenario = "NanoPi Home Assistant"
/// auto_plan = true
/// feature_desires = ["vision", "listening", "speech", "environmental_sensing"]
///
/// [[deployment.hardware]]
/// name = "nanopi-neo3"
/// board_name = "nanopi-neo3"
/// transport = "native"
/// role = "host"
/// accessories = ["dht22"]
/// ```
///
/// The example above is not decorative: `deployment_config_roundtrip.rs`
/// extracts it from this source file and parses it. An earlier version wrote
/// three keys per line separated by semicolons, which is not TOML and had sat
/// here uncaught since Phase 13 — the same failure as the `[[safety.limit]]`
/// that silently did nothing because the real key was `[[safety.limits]]`.
/// Documented configuration is prose until something executes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentConfig {
    /// Whether the deployment subsystem is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Human-readable name for this deployment scenario.
    #[serde(default = "default_scenario_name")]
    pub scenario: String,
    /// When true, re-derive the deployment scheme from `hardware` at startup,
    /// print it, and warn if it no longer matches the block that generated this
    /// file.
    ///
    /// The planner emits `auto_plan = true` into every config it writes, so this
    /// is on by default in practice. What it buys is drift detection: the config
    /// was planned once, by whichever version of the planner was current then,
    /// and the agent reading it may be several versions later. Re-planning at
    /// startup and comparing is cheap — the planner is pure — and it is the same
    /// idea as `parity/verify_wasm.cjs` executing the bundle rather than
    /// trusting its hash.
    #[serde(default)]
    pub auto_plan: bool,
    /// The hardware items in the deployment.
    #[serde(default)]
    pub hardware: Vec<DeploymentHardwareConfig>,
    /// High-level features the operator wants (see `FeatureDesire` variants).
    ///
    /// Recognised values: `"vision"`, `"listening"`, `"speech"`,
    /// `"environmental_sensing"`, `"display_output"`, `"touch_input"`,
    /// `"edge_inference"`, `"wireless_mesh"`, `"persistent_memory"`.
    #[serde(default)]
    pub feature_desires: Vec<String>,
}

fn default_scenario_name() -> String {
    "Oh-Ben-Claw Deployment".to_string()
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scenario: default_scenario_name(),
            auto_plan: false,
            hardware: Vec::new(),
            feature_desires: Vec::new(),
        }
    }
}

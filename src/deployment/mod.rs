//! Oh-Ben-Claw Deployment Subsystem — Phase 13
//!
//! Provides an advanced multi-agent swarm system for generating custom
//! deployment schemes based on available hardware and desired features.
//!
//! # Workflow
//!
//! ```text
//! HardwareInventory  →  DeploymentPlanner  →  DeploymentScheme
//!       │                                           │
//!       └─────────── DeploymentSwarm ───────────────┘
//!                    (LLM sub-agents refine)
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use oh_ben_claw::deployment::{HardwareInventory, DeploymentPlanner, DeploymentSwarm};
//!
//! // Build inventory for the NanoPi-Neo3 reference scenario
//! let inv = HardwareInventory::nanopi_scenario();
//!
//! // Generate a deployment scheme using the rule-based planner (no LLM needed)
//! let result = DeploymentSwarm::plan_static(&inv);
//! println!("{}", result.scheme.report());
//! ```
//!
//! # Supported Hardware (Phase 13)
//!
//! | Board | Role | Capabilities |
//! |---|---|---|
//! | NanoPi Neo3 | Host | gpio, i2c, spi, pwm |
//! | Waveshare ESP32-S3-Touch-LCD-2.1 | Display/Sound | display, touch, audio_sample, wifi |
//! | Seeed XIAO ESP32S3-Sense | Vision | camera_capture, audio_sample, wifi, ble |
//! | Sipeed 6+1 Mic Array | Listening | audio_sample |
//! | DHT22 (GPIO accessory) | Sensing | sensor_read |

pub mod advisor;
pub mod firmware_scaffold;
pub mod inventory;
pub mod planner;
pub mod saga;
pub mod scheme;
pub mod swarm;

pub use advisor::HardwareAdvisor;
pub use inventory::{FeatureDesire, HardwareInventory, HardwareItem, ItemRole};
pub use planner::DeploymentPlanner;
pub use scheme::{AgentAssignment, DeploymentScheme, NodeRole, SuggestedHardware};
pub use swarm::{DeploymentSwarm, SwarmConfig, SwarmResult};

/// Reading a `[deployment]` block back into an inventory.
///
/// **Why this is here and not in `inventory.rs`.** That file is compiled
/// *verbatim* into `planner-wasm` (see `planner-wasm/src/deployment/mod.rs`,
/// which `#[path]`-includes it), and that crate has no `config` module. A
/// `crate::config::` reference inside `inventory.rs` builds fine here and breaks
/// the WASM crate — which is how this was found. `mod.rs` is the host-only half:
/// `planner-wasm` supplies its own shim, so anything placed here is invisible to
/// it. Keep host-only code on this side of that line.
impl HardwareInventory {
    /// Rebuild an inventory from a parsed `[deployment]` block — the inverse of
    /// [`HardwareInventory::to_deployment_toml`].
    ///
    /// Until this existed the `[deployment]` schema was write-only. The planner
    /// emitted it, the TypeScript emitter in OBC-deployment-generator emitted a
    /// byte-identical copy, golden fixtures pinned both, and OBC-Prime hashed
    /// those fixtures across three repositories — and nothing ever read one
    /// back. `config.deployment` was consulted by exactly one thing in the tree:
    /// a test asserting the emitted TOML deserialises. That checks serde, not
    /// the contract.
    ///
    /// With the inverse in hand the contract becomes a fixed point —
    /// `emit → parse → rebuild → emit` reproduces the original text — which is a
    /// much stronger statement than "it parses", and it is the property the
    /// cross-repo goldens are reaching for. See
    /// `tests/deployment_config_roundtrip.rs`.
    ///
    /// **`capabilities` is deliberately not round-tripped.** It is absent from
    /// the emitted schema by design: capabilities come from the board registry,
    /// so carrying them in the config would let a stale file override it. A
    /// rebuilt item has empty `capabilities` and `resolved_capabilities()` fills
    /// them from the registry, exactly as for a hand-built inventory.
    pub fn from_deployment_config(cfg: &crate::config::DeploymentConfig) -> Self {
        let mut inv = Self::new(cfg.scenario.clone());

        for want in &cfg.feature_desires {
            // The exact inverse of `desire_token` in `to_deployment_toml`: unit
            // variants round-trip through their snake_case token, and anything
            // the enum does not know becomes Custom — which is what Custom is
            // for, and keeps an operator's own desire from being silently
            // dropped on the way in.
            let desire =
                serde_json::from_value::<FeatureDesire>(serde_json::Value::String(want.clone()))
                    .unwrap_or_else(|_| FeatureDesire::Custom(want.clone()));
            inv.add_desire(desire);
        }

        for hw in &cfg.hardware {
            inv.add_item(HardwareItem {
                name: hw.name.clone(),
                board_name: hw.board_name.clone(),
                transport: hw.transport.clone(),
                path: hw.path.clone(),
                node_id: hw.node_id.clone(),
                role: hw.role.parse().unwrap_or_default(),
                accessories: hw.accessories.clone(),
                capabilities: Vec::new(),
            });
        }

        inv
    }
}

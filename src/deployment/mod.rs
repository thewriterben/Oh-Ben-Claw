//! Deployment — the host-side half.
//!
//! The planner itself now lives in the [`obc_planner`] crate: the board
//! registry, the geometry, the site optimizer, and
//! `HardwareInventory → DeploymentPlanner → DeploymentScheme`. It was extracted
//! on 2026-07-30 because `planner-wasm` had already been compiling those files
//! standalone via `#[path]`, which is a compiler-grade proof that the closure is
//! self-contained.
//!
//! What stays here is what could not go: [`swarm`] drives LLM sub-agents to
//! refine a scheme, and [`saga`] is a multi-step rollout mechanism for an async
//! runtime — compensating actions, unwound in reverse on failure. Nothing calls
//! it yet: no code here applies a scheme across nodes, so there is no rollout
//! for it to compensate. It is tested and available, not wired.
//! Both need providers and a reactor; neither is needed to plan.
//!
//! Everything the crate exports is re-exported below, so `crate::deployment::X`
//! means what it always did and no call site changed.
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
//! let inv = HardwareInventory::nanopi_scenario();
//! let result = DeploymentSwarm::plan_static(&inv);
//! println!("{}", result.scheme.report());
//! ```

pub mod saga;
pub mod swarm;

pub use obc_planner::deployment::{advisor, firmware_scaffold, inventory, planner, scheme};
pub use obc_planner::deployment::{
    AgentAssignment, DeploymentPlanner, DeploymentScheme, FeatureDesire, HardwareAdvisor,
    HardwareInventory, HardwareItem, ItemRole, NodeRole, SuggestedHardware,
};
pub use swarm::{DeploymentSwarm, SwarmConfig, SwarmResult};

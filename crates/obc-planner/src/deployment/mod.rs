//! Hardware inventory → deployment scheme.
//!
//! ```text
//! HardwareInventory  →  DeploymentPlanner  →  DeploymentScheme
//! ```
//!
//! The pure half of the agent's deployment subsystem. `swarm` (LLM sub-agents
//! that refine a scheme) and `saga` (a rollout mechanism: compensating actions
//! unwound in reverse on failure, tested but not yet called by anything) stay in
//! the host crate: both need a runtime and providers, and neither is needed to
//! plan.

pub mod advisor;
pub mod firmware_scaffold;
pub mod inventory;
pub mod planner;
pub mod scheme;

pub use advisor::HardwareAdvisor;
pub use inventory::{FeatureDesire, HardwareInventory, HardwareItem, ItemRole};
pub use planner::DeploymentPlanner;
pub use scheme::{AgentAssignment, DeploymentScheme, NodeRole, SuggestedHardware};

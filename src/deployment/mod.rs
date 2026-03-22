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
pub mod inventory;
pub mod planner;
pub mod scheme;
pub mod swarm;

pub use advisor::HardwareAdvisor;
pub use inventory::{FeatureDesire, HardwareInventory, HardwareItem, ItemRole};
pub use planner::DeploymentPlanner;
pub use scheme::{AgentAssignment, DeploymentScheme, NodeRole, SuggestedHardware};
pub use swarm::{DeploymentSwarm, SwarmConfig, SwarmResult};

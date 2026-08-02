//! Oh-Ben-Claw — Advanced multi-device AI assistant.
//!
//! This crate provides the core library for the Oh-Ben-Claw system.
//! It extends the ZeroClaw architecture with a distributed, multi-device
//! coordination layer built on MQTT.
//!
//! # Architecture
//!
//! The system is organized around three layers:
//!
//! - **Brain** (`agent`): The central LLM-powered reasoning engine.
//! - **Spine** (`spine`): The MQTT-based communication backbone.
//! - **Appendages** (`peripherals`): Firmware and drivers for hardware nodes.
//!
//! # Feature Flags
//!
//! - `hardware`: Enable USB device discovery and serial port communication.
//! - `mqtt-spine`: Enable the MQTT communication spine.
//! - `peripheral-rpi`: Enable Raspberry Pi GPIO via rppal (Linux only).
//! - `peripheral-nanopi`: Enable NanoPi Neo3 GPIO via sysfs (Linux only).
//! - `gui`: Enable the native GUI application.

// NOTE: this crate deliberately does NOT carry a blanket
// `#![allow(dead_code, unused_imports, unused_variables)]`. It used to, and that
// is how five unreachable modules and a documented-but-never-called
// PersonalityStore sat here without a single warning. Suppress narrowly, at the
// item, with a reason — never at the crate root.
//
// Public library API — items are exported for use by external consumers (CLI, GUI,
// tests, and future integrations). A library legitimately exposes more surface than
// its own binary uses; that is an argument for `pub`, not for silencing the lint.

/// Agent-to-agent protocol. No `src/` consumer, but `tests/evals.rs` pins its wire
/// shape as a release gate — which is exactly the kind of consumer a source-only
/// survey misses. Kept.
pub mod a2a;
pub mod aerial;
pub mod agent;
pub mod approval;
pub mod audio;
pub mod channels;
// The three body-telemetry suites live in their own crate (2026-08-01), the
// first piece picked by `scripts/extractability.py` rather than by hand. They
// are one crate because they are one pattern: reading -> classified world-memory
// fact -> derived mode a reflex watches. Re-exported under the old names, so
// `crate::comms::…`, `crate::power::…` and `crate::sensing::…` are unchanged at
// every call site.
pub use obc_telemetry::comms;
pub mod config;
pub mod cost;
pub mod deployment;
pub mod doctor;
pub mod fleet;
pub mod foresight;
pub mod gateway;
/// Local-tangent-plane geometry and site models — re-exported from the
/// [`obc_planner`] crate, where it sits next to the site optimizer that consumes
/// it. `oh_ben_claw::geo::X` is unchanged.
pub use obc_planner::geo;
pub mod gnss;
pub mod harness;
pub mod learning;
pub mod mcp;
// The memory substrate lives in its own crate (2026-07-30). Re-exported under the
// old name so all twenty-three consumers keep compiling against `crate::memory::…`
// — a crate split that renamed every call site would be two changes at once, and
// only one of them is the point.
pub use obc_memory as memory;
pub use obc_planner;
pub mod mission;
pub mod movement;
pub mod navigation;
// The agent's self-instrumentation lives in its own crate (2026-08-02) — spans,
// the span ring buffer, and the counters `/api/v1/metrics` serves. Re-exported
// under the old name so `crate::observability::…` is unchanged at every call
// site in reflex, system2, orchestrator, skill_forge and the gateway.
//
// `obc_telemetry` is the agent watching its body; this is the agent watching
// itself. The two were named apart on purpose and are easy to confuse.
pub use obc_observability as observability;
pub mod peripherals;
pub use obc_telemetry::power;
pub mod providers;
// Cron, interval and one-shot tasks live in their own crate (2026-08-02) — the
// last module the extractability survey listed with zero blocking edges.
// Re-exported under the old name so `crate::scheduler::…` is unchanged in
// main.rs and the four gateway routes that drive it.
pub use obc_scheduler as scheduler;
pub mod security;
/// The site coverage optimizer — re-exported from the [`obc_planner`] crate.
pub use obc_planner::siteplan;
pub use obc_telemetry::sensing;
pub mod skill_forge;
pub mod spine;
pub mod tools;
pub mod tunnel;
pub mod vision;
pub use config::Config;

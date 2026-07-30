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
pub mod comms;
pub mod config;
pub mod cost;
pub mod deployment;
pub mod doctor;
pub mod fleet;
pub mod foresight;
pub mod gateway;
pub mod geo;
pub mod gnss;
pub mod harness;
pub mod learning;
pub mod mcp;
pub mod memory;
pub mod mission;
pub mod movement;
pub mod multimodal;
pub mod navigation;
pub mod observability;
pub mod peripherals;
pub mod power;
pub mod providers;
pub mod scheduler;
pub mod security;
pub mod sensing;
pub mod siteplan;
pub mod skill_forge;
pub mod spine;
pub mod tools;
pub mod tunnel;
pub mod vision;
pub use config::Config;

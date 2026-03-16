#![allow(dead_code, unused_imports, unused_variables)]
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

// Public library API — items are exported for use by external consumers (CLI, GUI,
// tests, and future integrations). Dead-code lint is suppressed at the crate level
// because the library intentionally exposes a broader surface than the binary uses.
#![allow(dead_code)]

pub mod agent;
pub mod audio;
pub mod channels;
pub mod config;
pub mod dashboard;
pub mod gateway;
pub mod memory;
pub mod observability;
pub mod peripherals;
pub mod providers;
pub mod scheduler;
pub mod security;
pub mod skill_forge;
pub mod spine;
pub mod tools;
pub mod tunnel;
pub mod mcp;
pub mod vision;
pub use config::Config;

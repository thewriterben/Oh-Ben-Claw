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

pub mod agent;
pub mod spine;
pub mod channels;
pub mod config;
pub mod memory;
pub mod observability;
pub mod peripherals;
pub mod providers;
pub mod security;
pub mod tools;
pub mod tunnel;

pub use config::Config;

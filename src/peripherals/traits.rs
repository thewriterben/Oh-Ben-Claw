//! Peripheral trait — the interface all hardware peripheral drivers must implement.

use crate::tools::traits::Tool;
use async_trait::async_trait;

/// A hardware peripheral that exposes a set of tools to the agent.
///
/// Each peripheral represents a connected hardware device (e.g., an ESP32-S3,
/// a NanoPi Neo3, or a Raspberry Pi). It provides a list of tools that the
/// agent can use to interact with the device.
#[async_trait]
pub trait Peripheral: Send + Sync {
    /// The human-readable name of this peripheral.
    fn name(&self) -> &str;

    /// The board type (e.g., "esp32-s3", "nanopi-neo3").
    fn board_type(&self) -> &str;

    /// Connect to the peripheral.
    async fn connect(&mut self) -> anyhow::Result<()>;

    /// Disconnect from the peripheral.
    async fn disconnect(&mut self) -> anyhow::Result<()>;

    /// Check if the peripheral is healthy and responsive.
    async fn health_check(&self) -> bool;

    /// Return the list of tools this peripheral exposes.
    fn tools(&self) -> Vec<Box<dyn Tool>>;
}

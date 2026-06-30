//! Node bring-up self-test contract + a host-side simulated node for CI.
//!
//! Before the brain trusts a freshly-flashed node, the node should prove it works:
//! its GPIO loops back, a sensor reads, its link is up. This module defines that
//! **bring-up self-test contract** ([`NodeSelfTest`]) — the same shape a real
//! serial/MQTT device adapter implements and the host-side [`SimulatedNode`]
//! implements for CI. Onboarding runs the suite and refuses a node that fails a
//! check; CI runs the simulator so the onboarding/spine/fleet paths can be
//! exercised **with no hardware attached**.
//!
//! Adapted from a sibling project's `DeviceAdapter.test_*` contract +
//! `SimulatedHardware`; the implementation here is original and self-contained.

use crate::spine::NodeAnnouncement;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;

/// The canonical bring-up checks every node should pass.
pub const STANDARD_CHECKS: &[&str] = &["gpio_loopback", "sensor_read", "link_up"];

/// One self-test outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfTestResult {
    pub name: String,
    pub passed: bool,
    pub detail: Option<String>,
}

impl SelfTestResult {
    pub fn pass(name: impl Into<String>) -> Self {
        Self { name: name.into(), passed: true, detail: None }
    }
    pub fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { name: name.into(), passed: false, detail: Some(detail.into()) }
    }
}

/// The result of a node's full bring-up suite.
#[derive(Debug, Clone)]
pub struct BringupReport {
    pub node_id: String,
    pub board: String,
    pub results: Vec<SelfTestResult>,
}

impl BringupReport {
    /// Whether every check passed (an empty suite vacuously passes).
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|r| r.passed)
    }
    /// The checks that failed.
    pub fn failures(&self) -> Vec<&SelfTestResult> {
        self.results.iter().filter(|r| !r.passed).collect()
    }
    /// A one-line human summary (`"rover-1 (esp32-s3): 2/3 checks passed"`).
    pub fn summary(&self) -> String {
        let passed = self.results.iter().filter(|r| r.passed).count();
        format!("{} ({}): {}/{} checks passed", self.node_id, self.board, passed, self.results.len())
    }
}

/// The bring-up self-test contract. A real device adapter drives the checks over
/// its transport; the simulator scripts them. Run on onboarding and in CI.
#[async_trait]
pub trait NodeSelfTest: Send + Sync {
    fn node_id(&self) -> &str;
    fn board(&self) -> &str;
    /// Run the standard bring-up checks and report.
    async fn run_bringup(&self) -> BringupReport;
}

/// A host-side simulated node for CI: implements the self-test contract with
/// scriptable outcomes and announces on the spine like a real node — so the
/// onboarding/fleet paths can be tested with no hardware.
#[derive(Debug, Clone)]
pub struct SimulatedNode {
    node_id: String,
    board: String,
    firmware_version: String,
    failing: HashSet<String>,
}

impl SimulatedNode {
    /// A healthy node that passes every standard check.
    pub fn healthy(node_id: impl Into<String>, board: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            board: board.into(),
            firmware_version: "sim-0.1.0".to_string(),
            failing: HashSet::new(),
        }
    }

    /// Mark a standard check as failing (to exercise the rejection path).
    pub fn with_failing_check(mut self, name: impl Into<String>) -> Self {
        self.failing.insert(name.into());
        self
    }

    /// The spine announcement this node would broadcast (no tools — a bring-up
    /// stand-in; real nodes announce their tool set).
    pub fn announcement(&self) -> NodeAnnouncement {
        NodeAnnouncement {
            node_id: self.node_id.clone(),
            board: self.board.clone(),
            firmware_version: self.firmware_version.clone(),
            tools: Vec::new(),
            metadata: Value::Null,
        }
    }
}

#[async_trait]
impl NodeSelfTest for SimulatedNode {
    fn node_id(&self) -> &str {
        &self.node_id
    }
    fn board(&self) -> &str {
        &self.board
    }
    async fn run_bringup(&self) -> BringupReport {
        let results = STANDARD_CHECKS
            .iter()
            .map(|&c| {
                if self.failing.contains(c) {
                    SelfTestResult::fail(c, "simulated failure")
                } else {
                    SelfTestResult::pass(c)
                }
            })
            .collect();
        BringupReport { node_id: self.node_id.clone(), board: self.board.clone(), results }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_healthy_node_passes_every_standard_check() {
        let node = SimulatedNode::healthy("rover-1", "esp32-s3");
        let report = node.run_bringup().await;
        assert!(report.all_passed());
        assert_eq!(report.results.len(), STANDARD_CHECKS.len());
        assert!(report.failures().is_empty());
    }

    #[tokio::test]
    async fn a_failing_check_fails_the_bringup() {
        let node = SimulatedNode::healthy("rover-2", "esp32-s3").with_failing_check("sensor_read");
        let report = node.run_bringup().await;
        assert!(!report.all_passed(), "one failed check fails bring-up");
        let failures = report.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "sensor_read");
        assert_eq!(failures[0].detail.as_deref(), Some("simulated failure"));
    }

    #[tokio::test]
    async fn summary_reports_the_pass_ratio() {
        let node = SimulatedNode::healthy("n", "rp2040").with_failing_check("link_up");
        let s = node.run_bringup().await.summary();
        assert!(s.contains("n (rp2040)"), "summary names the node + board: {s}");
        assert!(s.contains("2/3"), "summary shows the pass ratio: {s}");
    }

    #[test]
    fn simulated_node_announces_on_the_spine() {
        let node = SimulatedNode::healthy("cam-7", "xiao-esp32s3-sense");
        let ann = node.announcement();
        assert_eq!(ann.node_id, "cam-7");
        assert_eq!(ann.board, "xiao-esp32s3-sense");
        assert!(ann.tools.is_empty());
    }
}

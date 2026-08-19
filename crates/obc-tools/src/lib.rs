//! Oh-Ben-Claw tool registry.
//!
//! This module provides the core tool infrastructure and all built-in tools.
//! The agent's tool registry is assembled by combining the built-in tools
//! with any tools discovered from connected peripheral nodes.

pub mod builtin;
pub mod credentials;
/// The tool contract — the `Tool` trait, `ToolResult`, and the Track 0
/// vocabulary — extracted to [`obc_tool_api`] on 2026-08-08.
///
/// It left because it was the reason three other modules had an edge into this
/// one. `agent` named `tools::traits` 23 times, `gateway` 5, `spine` 2: 30 of
/// the core's 117 crossings were the contract, not the 9052 lines of tools
/// implementing it. See docs/ENDGAME.md.
///
/// `crate::traits::…` is unchanged at every call site.
pub use obc_tool_api as traits;

pub use builtin::{
    audio::{AudioTranscribeTool, TextToSpeechTool},
    browser::{
        BrowserClickTool, BrowserCloseTabTool, BrowserNavigateTool, BrowserNewTabTool,
        BrowserScrollTool, BrowserSession, BrowserSnapshotTool, BrowserTypeTool,
    },
    file::FileTool,
    http::HttpTool,
    memory::MemoryTool,
    ota::{DeviceHealthTool, OtaUpdateTool},
    shell::ShellTool,
    vision::VisionTool,
};
pub use traits::{Tool, ToolResult};
/// Build the default set of built-in tools.
///
/// Vision and audio tools read their API keys from environment variables
/// (`OPENAI_API_KEY`, `OPENAI_API_BASE`) at construction time.
/// Browser tools are always registered; they connect to a local CDP endpoint
/// if Chrome/Chromium is running with `--remote-debugging-port=9222`.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    default_tools_with_reach(None, None, None)
}

/// Like [`default_tools`], but attaches a conscience egress **reach gate** to
/// tools that make outbound network calls (the HTTP tool today). When a gate is
/// supplied, a request to a host not on the egress allowlist is refused before
/// any connection — the breach lesson, enforced by deterministic code the model
/// cannot override. `None` reproduces `default_tools()` exactly.
///
/// `auditor`, when supplied, is attached to the egress-gated HTTP tool so a
/// reach refusal is written to the tamper-evident audit log as a
/// `conscience.reach` denial — the same first-class record as a perception
/// refusal. A conscience that isn't audited is just a promise.
///
/// `resolver`, when supplied (conscience item (b)), lets the HTTP tool inject a
/// credential the reach gate names on an allow — resolved by name from the
/// vault/environment, never seen by the model. A named-but-unresolvable
/// credential fails closed.
pub fn default_tools_with_reach(
    reach: Option<obc_conscience::ReachGate>,
    auditor: Option<std::sync::Arc<std::sync::Mutex<obc_safety::ActionAuditor>>>,
    resolver: Option<std::sync::Arc<dyn crate::credentials::CredentialResolver>>,
) -> Vec<Box<dyn Tool>> {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let cdp_url = std::env::var("OBC_BROWSER_CDP_URL").ok();

    let http_tool = {
        let mut t = HttpTool::new();
        if let Some(gate) = reach.clone() {
            t = t.with_reach_gate(gate);
        }
        if let Some(a) = auditor.clone() {
            t = t.with_auditor(a);
        }
        if let Some(r) = resolver.clone() {
            t = t.with_resolver(r);
        }
        t
    };

    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ShellTool::new()),
        Box::new(FileTool::new()),
        Box::new(http_tool),
        Box::new(MemoryTool::new()),
        Box::new(AudioTranscribeTool::default()),
        Box::new(TextToSpeechTool::default()),
        Box::new(OtaUpdateTool),
        Box::new(DeviceHealthTool),
        Box::new(builtin::siteplan::SitePlanTool::new()),
        Box::new(builtin::aerial::AerialStatusTool::new()),
        Box::new(builtin::gnss::GnssFixTool::new()),
    ];

    // Vision tool requires an API key; only add if one is available
    if !api_key.is_empty() {
        tools.push(Box::new(VisionTool::new(api_key)));
    }

    // Browser tools share a single session; CDP URL from env or default port.
    // Navigation is an egress path, so it gets the same reach gate + auditor as
    // the HTTP tool — arbitrary browsing can't reach a non-allowlisted host.
    tools.extend(builtin::browser::all_browser_tools_with_reach(
        cdp_url.as_deref(),
        reach.clone(),
        auditor.clone(),
    ));

    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// No two tools may claim the same name.
    ///
    /// Added 2026-07-29 after an audit found **two** `impl Tool` types both
    /// returning `"audio_transcribe"`, with different required parameters:
    /// `AudioTranscribeTool` (`file_path`, registered) and
    /// `AudioTranscriptionTool` in `builtin::vision` (`path`, never registered).
    /// Only the first is exported here, so nothing is broken today — but the
    /// second is a live landmine, because the day someone registers it the
    /// registry silently holds two different tools under one name and which one
    /// answers depends on iteration order.
    ///
    /// A model that calls the wrong one gets an argument-name error, and the
    /// observed failure mode for local models given a tool error is to
    /// *confabulate a plausible result rather than report the failure*. So the
    /// cost of this collision is not an exception, it is a fabricated answer.
    ///
    /// This test guards the registered set. It does not resolve the duplicate —
    /// see the collision test below, which pins the hazard so it cannot be
    /// forgotten, and fails the moment someone "fixes" it by renaming one side
    /// without deleting the dead copy.
    #[test]
    fn default_tools_have_unique_names() {
        let tools = default_tools();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &tools {
            *counts.entry(t.name()).or_insert(0) += 1;
        }
        let dupes: Vec<String> = counts
            .iter()
            .filter(|(_, &n)| n > 1)
            .map(|(name, n)| format!("{name} (x{n})"))
            .collect();
        assert!(
            dupes.is_empty(),
            "two tools registered under one name: {}",
            dupes.join(", ")
        );
        assert!(
            tools.len() >= 11,
            "default_tools() shrank unexpectedly: {} tools",
            tools.len()
        );
    }

    /// Every registered tool declares a non-empty name, description and an
    /// object-shaped parameter schema. A tool the model cannot understand the
    /// call shape of is worse than an absent one.
    #[test]
    fn default_tools_are_describable() {
        for t in default_tools() {
            let name = t.name();
            assert!(!name.is_empty(), "a tool has an empty name");
            assert!(
                !t.description().trim().is_empty(),
                "{name}: empty description"
            );
            let schema = t.parameters_schema();
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{name}: parameters_schema is not an object schema"
            );
            assert!(
                schema.get("properties").is_some(),
                "{name}: parameters_schema has no properties"
            );
        }
    }

    /// The unresolved `audio_transcribe` collision, written down.
    ///
    /// This test *asserts the bug exists*, which is deliberate: it is the record
    /// that the two types disagree, and it will fail — telling you to update or
    /// delete it — the moment either side is renamed or removed. Deleting
    /// `AudioTranscriptionTool` (and its two tests in `builtin::vision`) is the
    /// intended resolution; the alternative is to register it under a distinct
    /// name. Either way, this test is the thing that notices.
    #[test]
    fn audio_transcribe_name_is_still_claimed_twice() {
        use builtin::vision::AudioTranscriptionTool;

        let registered = AudioTranscribeTool::default();
        let orphan = AudioTranscriptionTool::new("not-a-real-key");

        assert_eq!(
            registered.name(),
            orphan.name(),
            "the collision is gone — good. Delete this test."
        );

        let req = |t: &dyn Tool| -> Vec<String> {
            t.parameters_schema()
                .get("required")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };

        assert_eq!(req(&registered), vec!["file_path".to_string()]);
        assert_eq!(req(&orphan), vec!["path".to_string()]);
        assert_ne!(
            req(&registered),
            req(&orphan),
            "same name, same parameters — the collision is now harmless, but still \
             delete one of them"
        );
    }
}

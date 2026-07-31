//! The Track 0 vocabulary: how dangerous an action is, and how far it is trusted.
//!
//! These four types are the contract between the tool layer, the approval
//! layer, and the safety gate. Six modules use them — agent, approval, gateway,
//! mcp, skill_forge and tools — which is what makes them a contract rather than
//! an implementation detail of any one of them.
//!
//! They lived in `src/tools/traits.rs` until 2026-07-30, beside the `Tool` trait
//! whose methods return them. That put the safety layer downstream of the tool
//! registry: `security/audit.rs`, `security/taint.rs` and `security/trust.rs`
//! all reached into `crate::tools` for a risk classification. It was the only
//! outward dependency the entire security subsystem had, and it pointed at the
//! least self-contained module in the tree.
//!
//! Moving them here inverts that. The tool layer now depends on the safety
//! vocabulary, which is the direction every doc comment below already describes
//! — "drives Track 0 approval defaults", "used by the Track 0 safety layer",
//! "the safety gate uses it to require deterministic limits". `tools::traits`
//! re-exports all four, so no call site changed.

use serde::{Deserialize, Serialize};

/// How large a real-world effect a tool can have if it goes wrong.
///
/// Drives Track 0 approval defaults: higher blast radius ⇒ stricter gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlastRadius {
    /// No physical/real-world effect (pure compute, reads).
    None,
    /// A small, contained physical effect (e.g. toggle an LED).
    Low,
    /// A large or hazardous physical effect (e.g. unlock a door, drive a motor).
    High,
}

/// The physical risk profile of a tool, used by the Track 0 safety layer.
///
/// Non-physical, reversible tools (the default) are unaffected. Tools that
/// actuate the real world override `Tool::risk_class` to declare their risk;
/// the approval layer uses this to set default scopes (irreversible/high-blast
/// actions default to per-call approval and are never auto-grantable to
/// `forever`), and the safety gate uses it to require deterministic limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskClass {
    /// Whether the action can be cleanly undone.
    pub reversible: bool,
    /// The real-world blast radius.
    pub blast: BlastRadius,
    /// Whether the tool drives a physical actuator / real-world effect.
    pub physical: bool,
}

impl RiskClass {
    /// A non-physical, reversible action (the default for ordinary tools).
    pub const fn safe() -> Self {
        Self {
            reversible: true,
            blast: BlastRadius::None,
            physical: false,
        }
    }

    /// A physical actuator action with the given reversibility and blast radius.
    pub const fn physical(reversible: bool, blast: BlastRadius) -> Self {
        Self {
            reversible,
            blast,
            physical: true,
        }
    }

    /// Whether this action must default to per-call approval (irreversible or
    /// high-blast physical actions).
    pub const fn requires_per_call_approval(&self) -> bool {
        self.physical && (!self.reversible || matches!(self.blast, BlastRadius::High))
    }
}

impl Default for RiskClass {
    fn default() -> Self {
        Self::safe()
    }
}

/// Track 0 staged-rollout stage of a tool (relevant to learned/installed
/// skills; ordinary built-in tools are always `Autonomous`).
///
/// A skill climbs `Simulate → Supervised → Autonomous`, each promotion gated
/// on a clean run record and performed by an operator:
/// - `Simulate` — the agent may *invoke* the skill, but the execution
///   chokepoint only reports what **would** run; nothing executes.
/// - `Supervised` — executes only with an explicit operator grant
///   (auto-approve list, session, or forever grant); a failure demotes.
/// - `Autonomous` — runs like any other tool (still subject to policy,
///   Track 0 limits, trust, and approval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStage {
    Simulate,
    Supervised,
    #[default]
    Autonomous,
}

impl RolloutStage {
    /// The next stage up, if any.
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Simulate => Some(Self::Supervised),
            Self::Supervised => Some(Self::Autonomous),
            Self::Autonomous => None,
        }
    }

    /// The next stage down, if any.
    pub fn prev(self) -> Option<Self> {
        match self {
            Self::Autonomous => Some(Self::Supervised),
            Self::Supervised => Some(Self::Simulate),
            Self::Simulate => None,
        }
    }

    /// Stable string form (matches the serde wire format).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulate => "simulate",
            Self::Supervised => "supervised",
            Self::Autonomous => "autonomous",
        }
    }
}

/// How trustworthy a tool's *output* is as a data source (Track 0 taint
/// tracking). Tools that surface content from outside the trust boundary —
/// web pages, remote MCP servers, arbitrary inbound messages — return
/// [`OutputTrust::External`]; their output is pooled and any privileged
/// action whose arguments echo that content is flagged/refused. The default
/// is `Trusted` (local computation, the operator's own files, sensors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputTrust {
    /// Local/first-party output — not a prompt-injection vector.
    #[default]
    Trusted,
    /// Output may contain attacker-controlled content from outside the
    /// trust boundary (web, remote MCP, untrusted inbound text).
    External,
}

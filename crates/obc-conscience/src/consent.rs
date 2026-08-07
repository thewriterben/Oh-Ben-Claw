//! The perception gate (consent registry) — Track 0 for what the agent may SEE.
//!
//! A camera-bearing, always-on agent is a surveillance apparatus by default.
//! This gate decides — with fixed rules the model cannot influence — whether a
//! detected subject may be captured, how long it may be retained, and how it
//! may be transmitted. It runs BEFORE any frame reaches the reasoner, so the
//! model can neither keep nor be injected by what it was not permitted to see.
//!
//! Two load-bearing defaults, both matching the safety layer's philosophy:
//! **default-deny for humans** (and for any unlisted class), and **fail-closed
//! on classifier uncertainty** — an unsure classification is treated as the
//! most-restricted case and refused. Guessing toward capture would launder
//! surveillance into "we weren't sure"; per the memory-integrity rule, never
//! guess up.

use serde::{Deserialize, Serialize};

/// How a permitted subject's imagery may leave the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Transmit {
    /// Never leaves the node.
    #[default]
    None,
    /// Only derived model weights/counts leave, never imagery (federated).
    WeightsOnly,
    /// Frames may be transmitted.
    Frames,
    /// Full imagery + metadata.
    Full,
}

/// A declarative consent rule for one subject class. The model cannot write
/// these — they are operator-declared config, exactly like `[[safety.limits]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRule {
    /// Subject class this rule governs (e.g. `"wildlife"`, `"human"`).
    pub class: String,
    /// Whether this class may be captured at all. Default-deny elsewhere.
    #[serde(default)]
    pub capture: bool,
    /// Retention bound in days (0 = do not retain).
    #[serde(default)]
    pub retain_days: u32,
    /// How imagery may be transmitted.
    #[serde(default)]
    pub transmit: Transmit,
}

impl ConsentRule {
    /// A permitted class with the given retention/transmit (builder for tests/config).
    pub fn allow(class: impl Into<String>, retain_days: u32, transmit: Transmit) -> Self {
        Self {
            class: class.into(),
            capture: true,
            retain_days,
            transmit,
        }
    }

    /// An explicit denial for a class (e.g. humans on a wildlife body).
    pub fn deny(class: impl Into<String>) -> Self {
        Self {
            class: class.into(),
            capture: false,
            retain_days: 0,
            transmit: Transmit::None,
        }
    }
}

/// Why perception of a subject was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum PerceptionRefusal {
    /// No rule permits this class — default-deny.
    NoRule { class: String },
    /// A rule exists but forbids capture of this class.
    CaptureDenied { class: String },
    /// Classifier confidence below threshold — fail closed, treat as restricted.
    LowConfidence {
        class: String,
        confidence: f32,
        threshold: f32,
    },
    /// The classifier did not recognize the detector's label at all — fail
    /// closed and refuse outright, never mapping it onto a permitted class.
    Unrecognized { label: String },
}

impl std::fmt::Display for PerceptionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerceptionRefusal::NoRule { class } => {
                write!(f, "conscience: no consent rule for class '{class}' (default-deny)")
            }
            PerceptionRefusal::CaptureDenied { class } => {
                write!(f, "conscience: capture of class '{class}' is denied")
            }
            PerceptionRefusal::LowConfidence { class, confidence, threshold } => write!(
                f,
                "conscience: classifier unsure ('{class}' at {confidence:.2} < {threshold:.2}) — fail closed"
            ),
            PerceptionRefusal::Unrecognized { label } => write!(
                f,
                "conscience: unrecognized subject label '{label}' — fail closed (refused)"
            ),
        }
    }
}

impl std::error::Error for PerceptionRefusal {}

/// The decision the gate returns for a detected subject.
#[derive(Debug, Clone, PartialEq)]
pub enum PerceptionDecision {
    /// Capture permitted with these bounds.
    Allow {
        retain_days: u32,
        transmit: Transmit,
    },
    /// Refused — the frame is dropped, never stored, never sent to the reasoner.
    Refuse(PerceptionRefusal),
}

impl PerceptionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, PerceptionDecision::Allow { .. })
    }
}

/// Enforces [`ConsentRule`]s before a frame reaches storage or the reasoner.
#[derive(Debug, Clone)]
pub struct PerceptionGate {
    rules: Vec<ConsentRule>,
    /// Below this classifier confidence, the gate fails closed.
    confidence_threshold: f32,
}

impl PerceptionGate {
    pub fn new(rules: Vec<ConsentRule>, confidence_threshold: f32) -> Self {
        Self {
            rules,
            confidence_threshold,
        }
    }

    fn rule_for(&self, class: &str) -> Option<&ConsentRule> {
        self.rules.iter().find(|r| r.class == class)
    }

    /// Decide whether a detected subject may be perceived. `confidence` is the
    /// classifier's certainty in the class label, in `[0.0, 1.0]`.
    ///
    /// Order matters: uncertainty is checked FIRST, so a low-confidence
    /// "wildlife" that might be a person is refused before its (permissive)
    /// rule is ever consulted.
    pub fn check(&self, class: &str, confidence: f32) -> PerceptionDecision {
        if confidence < self.confidence_threshold {
            return PerceptionDecision::Refuse(PerceptionRefusal::LowConfidence {
                class: class.to_string(),
                confidence,
                threshold: self.confidence_threshold,
            });
        }
        match self.rule_for(class) {
            None => PerceptionDecision::Refuse(PerceptionRefusal::NoRule {
                class: class.to_string(),
            }),
            Some(rule) if !rule.capture => {
                PerceptionDecision::Refuse(PerceptionRefusal::CaptureDenied {
                    class: class.to_string(),
                })
            }
            Some(rule) => PerceptionDecision::Allow {
                retain_days: rule.retain_days,
                transmit: rule.transmit,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> PerceptionGate {
        // A wildlife body: wildlife permitted weights-only; humans explicitly denied.
        PerceptionGate::new(
            vec![
                ConsentRule::allow("wildlife", 30, Transmit::WeightsOnly),
                ConsentRule::deny("human"),
            ],
            0.60,
        )
    }

    #[test]
    fn permits_declared_class() {
        let d = gate().check("wildlife", 0.95);
        assert_eq!(
            d,
            PerceptionDecision::Allow {
                retain_days: 30,
                transmit: Transmit::WeightsOnly
            }
        );
    }

    #[test]
    fn denies_human_by_default_even_when_confident() {
        let d = gate().check("human", 0.99);
        assert!(matches!(
            d,
            PerceptionDecision::Refuse(PerceptionRefusal::CaptureDenied { .. })
        ));
    }

    #[test]
    fn unlisted_class_is_default_denied() {
        let d = gate().check("license_plate", 0.99);
        assert!(matches!(
            d,
            PerceptionDecision::Refuse(PerceptionRefusal::NoRule { .. })
        ));
    }

    #[test]
    fn fails_closed_on_low_confidence() {
        // Might be a person mislabeled wildlife — refuse before consulting the rule.
        let d = gate().check("wildlife", 0.40);
        assert!(matches!(
            d,
            PerceptionDecision::Refuse(PerceptionRefusal::LowConfidence { .. })
        ));
    }

    #[test]
    fn empty_registry_denies_everything() {
        let g = PerceptionGate::new(vec![], 0.5);
        assert!(!g.check("wildlife", 0.99).is_allowed());
        assert!(!g.check("human", 0.99).is_allowed());
    }
}

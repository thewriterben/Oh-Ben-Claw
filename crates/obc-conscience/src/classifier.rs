//! The perception classifier — maps a detector's raw label onto a consent CLASS.
//!
//! The gate ([`crate::consent::PerceptionGate`]) decides consent by *class*
//! (`"human"`, `"wildlife"`). Something has to turn a detector's raw label
//! (`"person"`, `"deer"`, `"white_tailed_deer"`, `"drone"`) into that class.
//! That is this — the heuristic the spec (§0, §2.1, §6) names: **the gate acts
//! on the class; this produces it.**
//!
//! Two rules, both fail-closed, both matching the safety layer's "never guess
//! up":
//! - A label that maps to a known class returns that class.
//! - A label this classifier does **not** recognize is **unrecognized** — the
//!   caller must refuse it outright ([`crate::Conscience::may_perceive_label`]
//!   does), regardless of how permissive the registry is. An unrecognized
//!   subject is never captured just because *some* class is allowed; guessing
//!   toward capture would launder surveillance into "we weren't sure."
//!
//! The default map covers common human and wildlife labels so a wildlife body
//! works out of the box; a deployment whose detector emits a species taxonomy
//! extends the map via `[conscience.classifier]` config — its own vocabulary,
//! mapped to broad consent classes, with everything unmapped failing closed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_restricted() -> String {
    "human".to_string()
}
fn default_true() -> bool {
    true
}

/// Normalize a raw label for lookup: lowercase, trimmed, spaces/hyphens → `_`.
fn normalize(label: &str) -> String {
    label
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect()
}

/// Config for the perception classifier (`[conscience.classifier]`).
///
/// Unknown keys are rejected, like the rest of conscience config: a typo must
/// not silently drop a mapping and let a subject through misclassified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifierConfig {
    /// Extra / override `label → class` mappings (labels are normalized).
    /// Merged over the built-in defaults; the operator adds their detector's
    /// vocabulary here (e.g. `"white_tailed_deer" = "wildlife"`).
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Class assigned to an UNRECOGNIZED label. Callers treat an unrecognized
    /// subject as fail-closed (refused). Default `"human"` — the most-restricted
    /// class on a default-deny body, so the fallback is also denied if ever
    /// consulted.
    #[serde(default = "default_restricted")]
    pub restricted_class: String,
    /// Start from the built-in default label map (recommended). If `false`, only
    /// `labels` is used — the operator supplies the entire vocabulary.
    #[serde(default = "default_true")]
    pub use_defaults: bool,
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            labels: HashMap::new(),
            restricted_class: default_restricted(),
            use_defaults: true,
        }
    }
}

/// The result of classifying a raw label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The consent class the gate should check.
    pub class: String,
    /// `false` when the label was not in the map (the `class` is the fail-closed
    /// `restricted_class`, and the caller must refuse rather than consult a rule).
    pub recognized: bool,
}

/// Maps raw detector labels onto broad consent classes, fail-closed.
#[derive(Debug, Clone)]
pub struct SubjectClassifier {
    labels: HashMap<String, String>, // normalized label -> class
    restricted_class: String,
}

impl SubjectClassifier {
    /// Build from config, merging operator labels over the defaults (unless
    /// `use_defaults = false`).
    pub fn from_config(cfg: &ClassifierConfig) -> Self {
        let mut labels = if cfg.use_defaults {
            default_labels()
        } else {
            HashMap::new()
        };
        for (k, v) in &cfg.labels {
            labels.insert(normalize(k), v.clone());
        }
        Self {
            labels,
            restricted_class: cfg.restricted_class.clone(),
        }
    }

    /// The armed default: built-in human + wildlife vocabulary, restricted class
    /// `"human"`.
    pub fn with_defaults() -> Self {
        Self::from_config(&ClassifierConfig::default())
    }

    /// The class an unrecognized label falls back to — the most-restricted class,
    /// and the one that requires consent. Exposed so callers (multi-party consent,
    /// the false-negative harness) can gate on it without a sentinel lookup.
    pub fn restricted_class(&self) -> &str {
        &self.restricted_class
    }

    /// Classify a raw label into a consent class. A label not in the map is
    /// `recognized == false` and returns the restricted class — the caller must
    /// refuse it (see [`crate::Conscience::may_perceive_label`]).
    pub fn classify(&self, label: &str) -> Classification {
        match self.labels.get(&normalize(label)) {
            Some(class) => Classification {
                class: class.clone(),
                recognized: true,
            },
            None => Classification {
                class: self.restricted_class.clone(),
                recognized: false,
            },
        }
    }
}

impl Default for SubjectClassifier {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// The built-in label → class map. Generic labels a common detector emits; a
/// species taxonomy is added by the operator via config.
fn default_labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    for h in [
        "person", "people", "pedestrian", "human", "man", "woman", "child",
        "boy", "girl", "face", "head", "body",
    ] {
        m.insert(h.to_string(), "human".to_string());
    }
    for w in [
        "animal", "wildlife", "deer", "elk", "moose", "bird", "bear", "coyote",
        "wolf", "fox", "rabbit", "hare", "squirrel", "raccoon", "skunk",
        "opossum", "bobcat", "cougar", "mountain_lion", "turkey", "duck",
        "goose", "owl", "hawk", "eagle", "boar", "hog",
    ] {
        m.insert(w.to_string(), "wildlife".to_string());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_human_and_wildlife_labels() {
        let c = SubjectClassifier::with_defaults();
        assert_eq!(c.classify("person").class, "human");
        assert!(c.classify("person").recognized);
        assert_eq!(c.classify("deer").class, "wildlife");
        assert!(c.classify("deer").recognized);
    }

    #[test]
    fn is_case_and_separator_insensitive() {
        let c = SubjectClassifier::with_defaults();
        assert_eq!(c.classify("  Person ").class, "human");
        assert_eq!(c.classify("Mountain Lion").class, "wildlife");
        assert_eq!(c.classify("mountain-lion").class, "wildlife");
    }

    #[test]
    fn unrecognized_label_fails_closed_to_restricted() {
        let c = SubjectClassifier::with_defaults();
        let got = c.classify("drone");
        assert!(!got.recognized); // caller must refuse
        assert_eq!(got.class, "human"); // most-restricted fallback
    }

    #[test]
    fn operator_labels_extend_defaults() {
        let cfg = ClassifierConfig {
            labels: HashMap::from([("white_tailed_deer".to_string(), "wildlife".to_string())]),
            ..Default::default()
        };
        let c = SubjectClassifier::from_config(&cfg);
        assert_eq!(c.classify("white_tailed_deer").class, "wildlife");
        assert!(c.classify("white_tailed_deer").recognized);
        // defaults still present
        assert_eq!(c.classify("person").class, "human");
    }

    #[test]
    fn use_defaults_false_uses_only_operator_labels() {
        let cfg = ClassifierConfig {
            labels: HashMap::from([("gopher".to_string(), "wildlife".to_string())]),
            use_defaults: false,
            ..Default::default()
        };
        let c = SubjectClassifier::from_config(&cfg);
        assert!(c.classify("gopher").recognized);
        // "deer" is a default; with defaults off it's now unrecognized → fail closed
        assert!(!c.classify("deer").recognized);
    }
}

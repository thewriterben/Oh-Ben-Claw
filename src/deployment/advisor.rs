//! Hardware requirement advisor — gap analysis and hardware suggestions.
//!
//! The `HardwareAdvisor` analyses a `HardwareInventory` against a list of
//! feature desires and:
//!
//! 1. Identifies which desires are **satisfied** by the available hardware.
//! 2. Identifies which desires are **unsatisfied** and explains what is missing.
//! 3. Suggests specific boards or accessories that would close the gap.
//!
//! This information is embedded in the `DeploymentScheme` as the
//! `suggested_hardware` list.

use crate::deployment::inventory::{FeatureDesire, HardwareInventory};
use crate::deployment::scheme::SuggestedHardware;
use crate::peripherals::registry::{boards_with_capability, known_boards};
use serde::{Deserialize, Serialize};

// ── Satisfaction Result ───────────────────────────────────────────────────────

/// Whether a feature desire is satisfied by the available hardware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatisfactionResult {
    /// The feature desire checked.
    pub desire: String,
    /// Whether the desire is satisfied.
    pub satisfied: bool,
    /// The capability token that was required (if any).
    pub required_capability: Option<String>,
    /// The hardware item that satisfies this desire (if any).
    pub satisfied_by: Option<String>,
}

// ── Hardware Advisor ──────────────────────────────────────────────────────────

/// Analyses hardware inventory against feature desires and produces gap reports.
pub struct HardwareAdvisor;

impl HardwareAdvisor {
    /// Analyse the inventory and return a satisfaction report for each desire.
    pub fn analyse(inventory: &HardwareInventory) -> Vec<SatisfactionResult> {
        inventory
            .feature_desires
            .iter()
            .map(|desire| Self::check_desire(inventory, desire))
            .collect()
    }

    /// Check whether a single feature desire is satisfied.
    pub fn check_desire(inventory: &HardwareInventory, desire: &FeatureDesire) -> SatisfactionResult {
        let required = desire.required_capabilities();

        // Desires with no required capability tokens are always satisfied if
        // there is a host board (e.g., EdgeInference, PersistentMemory depend
        // on host-level configuration, not a specific capability token).
        if required.is_empty() {
            return SatisfactionResult {
                desire: desire.description(),
                satisfied: true,
                required_capability: None,
                satisfied_by: None,
            };
        }

        // Check each required capability against the inventory
        for cap in required {
            let items = inventory.items_with_capability(cap);
            if !items.is_empty() {
                return SatisfactionResult {
                    desire: desire.description(),
                    satisfied: true,
                    required_capability: Some(cap.to_string()),
                    satisfied_by: Some(items[0].name.clone()),
                };
            }
        }

        // Not satisfied — build a suggestion
        let first_cap = required[0].to_string();
        SatisfactionResult {
            desire: desire.description(),
            satisfied: false,
            required_capability: Some(first_cap),
            satisfied_by: None,
        }
    }

    /// Return suggestions for all unsatisfied feature desires.
    pub fn suggest_missing(inventory: &HardwareInventory) -> Vec<SuggestedHardware> {
        Self::analyse(inventory)
            .into_iter()
            .filter(|r| !r.satisfied)
            .filter_map(|r| {
                let cap = r.required_capability?;
                let suggested = Self::boards_for_capability(&cap);
                Some(SuggestedHardware {
                    missing_capability: cap.clone(),
                    for_feature: r.desire.clone(),
                    suggested_boards: suggested,
                    reason: format!(
                        "No available hardware provides '{}', which is required for: {}",
                        cap, r.desire
                    ),
                })
            })
            .collect()
    }

    /// Return board names that provide a given capability, drawn from the registry.
    pub fn boards_for_capability(capability: &str) -> Vec<String> {
        boards_with_capability(capability)
            .iter()
            .map(|b| b.name.to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Generate a human-readable compatibility report for the entire inventory.
    pub fn compatibility_report(inventory: &HardwareInventory) -> String {
        let results = Self::analyse(inventory);
        let mut out = String::new();
        out.push_str(&format!(
            "# Hardware Compatibility Report — {}\n\n",
            inventory.scenario_name
        ));

        let satisfied: Vec<_> = results.iter().filter(|r| r.satisfied).collect();
        let unsatisfied: Vec<_> = results.iter().filter(|r| !r.satisfied).collect();

        out.push_str(&format!(
            "**{}/{}** feature desires are satisfied.\n\n",
            satisfied.len(),
            results.len()
        ));

        if !satisfied.is_empty() {
            out.push_str("## Satisfied Features\n\n");
            for r in &satisfied {
                let by = r
                    .satisfied_by
                    .as_deref()
                    .map(|s| format!(" (by `{s}`)"))
                    .unwrap_or_default();
                out.push_str(&format!("- ✅ {}{}\n", r.desire, by));
            }
            out.push('\n');
        }

        if !unsatisfied.is_empty() {
            out.push_str("## Unsatisfied Features — Hardware Gaps\n\n");
            for r in &unsatisfied {
                let cap = r
                    .required_capability
                    .as_deref()
                    .map(|s| format!(" (needs `{s}`)"))
                    .unwrap_or_default();
                out.push_str(&format!("- ❌ {}{}\n", r.desire, cap));
                let suggestions = r
                    .required_capability
                    .as_ref()
                    .map(|c| Self::boards_for_capability(c))
                    .unwrap_or_default();
                if !suggestions.is_empty() {
                    out.push_str(&format!(
                        "  → Suggestion: add one of: {}\n",
                        suggestions.join(", ")
                    ));
                }
            }
        }

        // Board-level notes
        out.push_str("\n## Available Hardware\n\n");
        for item in &inventory.items {
            let caps = item.resolved_capabilities();
            out.push_str(&format!(
                "- **{}** (`{}`) — transport: `{}`, capabilities: {}\n",
                item.name,
                item.board_name,
                item.transport,
                if caps.is_empty() {
                    "unknown".to_string()
                } else {
                    caps.join(", ")
                }
            ));
        }

        out
    }

    /// Return a list of warnings about the inventory (e.g., no host board,
    /// duplicate roles, orphaned accessories, etc.).
    pub fn validate(inventory: &HardwareInventory) -> Vec<String> {
        let mut warnings = Vec::new();

        if inventory.items.is_empty() {
            warnings.push("Inventory is empty — add at least one hardware item.".to_string());
            return warnings;
        }

        // Check for host board
        if inventory
            .find_role(&crate::deployment::inventory::ItemRole::Host)
            .is_none()
        {
            // Try to find a native-transport board as implicit host
            let native = inventory
                .items
                .iter()
                .find(|i| i.transport == "native");
            if native.is_none() {
                warnings.push(
                    "No host board found. Add a board with transport='native' or \
                     assign a board the 'host' role."
                        .to_string(),
                );
            }
        }

        // Check for unknown boards (not in registry)
        let known_names: Vec<_> = known_boards().iter().map(|b| b.name).collect();
        for item in &inventory.items {
            if !known_names.contains(&item.board_name.as_str()) && item.capabilities.is_empty() {
                warnings.push(format!(
                    "Board '{}' is not in the registry and has no explicit capabilities set. \
                     It may not be recognised correctly.",
                    item.board_name
                ));
            }
        }

        // Warn if listening desired but no audio_sample capability
        if inventory
            .feature_desires
            .contains(&FeatureDesire::Listening)
            && inventory.items_with_capability("audio_sample").is_empty()
        {
            warnings.push(
                "Listening feature desired but no audio_sample hardware found. \
                 Consider adding a Sipeed 6+1 Mic Array or XIAO ESP32S3-Sense."
                    .to_string(),
            );
        }

        warnings
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::inventory::{HardwareInventory, HardwareItem, ItemRole};

    #[test]
    fn nanopi_scenario_all_desires_satisfied() {
        let inv = HardwareInventory::nanopi_scenario();
        let results = HardwareAdvisor::analyse(&inv);
        let unsatisfied: Vec<_> = results.iter().filter(|r| !r.satisfied).collect();
        // All desires in the reference scenario should be satisfiable by the listed hardware
        // (EdgeInference and PersistentMemory are host-level and always satisfied)
        for r in &unsatisfied {
            // Only allow desires that legitimately need extra hardware
            let cap = r.required_capability.as_deref().unwrap_or("");
            // If not empty capability (custom desires), it's a real gap
            if !cap.is_empty() {
                panic!("Unsatisfied desire in reference scenario: {:?}", r.desire);
            }
        }
    }

    #[test]
    fn missing_camera_generates_suggestion() {
        let mut inv = HardwareInventory::new("no-camera");
        inv.add_item(HardwareItem::new("host", "nanopi-neo3", "native").with_role(ItemRole::Host));
        inv.add_desire(FeatureDesire::Vision);

        let suggestions = HardwareAdvisor::suggest_missing(&inv);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].missing_capability, "camera_capture");
        assert!(!suggestions[0].suggested_boards.is_empty());
    }

    #[test]
    fn no_suggestions_when_all_desires_met() {
        let inv = HardwareInventory::nanopi_scenario();
        let suggestions = HardwareAdvisor::suggest_missing(&inv);
        assert!(
            suggestions.is_empty(),
            "Unexpected suggestions: {:?}",
            suggestions
        );
    }

    #[test]
    fn validate_warns_on_empty_inventory() {
        let inv = HardwareInventory::new("empty");
        let warnings = HardwareAdvisor::validate(&inv);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("empty"));
    }

    #[test]
    fn validate_no_warnings_for_complete_scenario() {
        let inv = HardwareInventory::nanopi_scenario();
        let warnings = HardwareAdvisor::validate(&inv);
        // Should be empty or only contain informational notes, not critical errors
        for w in &warnings {
            assert!(
                !w.contains("No host board"),
                "Unexpected host warning: {w}"
            );
        }
    }

    #[test]
    fn compatibility_report_contains_scenario_name() {
        let inv = HardwareInventory::nanopi_scenario();
        let report = HardwareAdvisor::compatibility_report(&inv);
        assert!(report.contains("NanoPi-Neo3 Reference Deployment"));
        assert!(report.contains("nanopi-neo3"));
    }

    #[test]
    fn boards_for_capability_returns_non_empty_for_camera() {
        let boards = HardwareAdvisor::boards_for_capability("camera_capture");
        assert!(!boards.is_empty());
        assert!(boards.iter().any(|b| b.contains("esp32-s3") || b.contains("xiao")));
    }
}

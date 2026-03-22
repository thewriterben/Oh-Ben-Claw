//! Hardware inventory — describes the physical hardware available for a deployment.
//!
//! The `HardwareInventory` is the input to the `DeploymentPlanner`.  It lists
//! every board and accessory that is present in the target deployment, along
//! with the role the operator wants each piece of hardware to play and the
//! high-level feature desires they want the deployment to fulfil.

use serde::{Deserialize, Serialize};

// ── Feature Desires ───────────────────────────────────────────────────────────

/// A high-level capability the operator wants the deployment to provide.
///
/// Feature desires are mapped to specific hardware capability tokens and agent
/// roles during planning.  Unsatisfied desires produce suggestions for missing
/// hardware.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureDesire {
    /// Agent can see — requires `camera_capture` hardware.
    Vision,
    /// Agent can hear — requires `audio_sample` hardware.
    Listening,
    /// Agent can speak — requires `audio_sample` (TTS playback) or `display` hardware.
    Speech,
    /// Agent can sense the environment — requires `sensor_read` hardware.
    EnvironmentalSensing,
    /// Agent can display information — requires `display` hardware.
    DisplayOutput,
    /// Agent can accept touch input — requires `touch` hardware.
    TouchInput,
    /// Agent runs locally without a cloud LLM — requires a capable host board.
    EdgeInference,
    /// Agent mesh communicates wirelessly — requires `wifi` hardware.
    WirelessMesh,
    /// Agent persists context across sessions — requires a host with storage.
    PersistentMemory,
    /// A custom feature desire described by a free-form string.
    Custom(String),
}

impl FeatureDesire {
    /// Return the capability tokens that must be present to satisfy this desire.
    pub fn required_capabilities(&self) -> &'static [&'static str] {
        match self {
            Self::Vision => &["camera_capture"],
            Self::Listening => &["audio_sample"],
            Self::Speech => &["audio_sample"],
            Self::EnvironmentalSensing => &["sensor_read"],
            Self::DisplayOutput => &["display"],
            Self::TouchInput => &["touch"],
            Self::EdgeInference => &[],
            Self::WirelessMesh => &["wifi"],
            Self::PersistentMemory => &[],
            Self::Custom(_) => &[],
        }
    }

    /// Human-readable description of this desire.
    pub fn description(&self) -> String {
        match self {
            Self::Vision => "visual perception via camera".to_string(),
            Self::Listening => "audio input via microphone".to_string(),
            Self::Speech => "speech output via speaker or TTS".to_string(),
            Self::EnvironmentalSensing => "environmental sensing (temperature, humidity, etc.)".to_string(),
            Self::DisplayOutput => "display output on a screen".to_string(),
            Self::TouchInput => "capacitive or resistive touch input".to_string(),
            Self::EdgeInference => "on-device LLM inference without cloud dependency".to_string(),
            Self::WirelessMesh => "wireless P2P node mesh networking".to_string(),
            Self::PersistentMemory => "persistent conversation memory across sessions".to_string(),
            Self::Custom(s) => s.clone(),
        }
    }
}

// ── Item Role ─────────────────────────────────────────────────────────────────

/// The operator-assigned role for a hardware item in the deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemRole {
    /// Primary host: runs the main Oh-Ben-Claw agent process.
    Host,
    /// Display and/or sound output node.
    Display,
    /// Vision node — captures images or video.
    Vision,
    /// Listening node — captures audio or provides mic array.
    Listening,
    /// Environmental sensing node — reads temperature, humidity, pressure, etc.
    Sensing,
    /// General-purpose peripheral — GPIO expansion, actuator control, etc.
    Peripheral,
    /// Unassigned — role will be inferred by the planner.
    #[default]
    Unassigned,
}

impl std::fmt::Display for ItemRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host => write!(f, "host"),
            Self::Display => write!(f, "display"),
            Self::Vision => write!(f, "vision"),
            Self::Listening => write!(f, "listening"),
            Self::Sensing => write!(f, "sensing"),
            Self::Peripheral => write!(f, "peripheral"),
            Self::Unassigned => write!(f, "unassigned"),
        }
    }
}

// ── Hardware Item ─────────────────────────────────────────────────────────────

/// A single piece of hardware (board or accessory) in the deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareItem {
    /// Human-readable label (e.g., `"nanopi-neo3"`, `"xiao-esp32s3-sense"`).
    pub name: String,
    /// The board registry name — matches a `BoardInfo::name` entry or an
    /// `AccessoryInfo::name` entry.  Used to look up capabilities.
    pub board_name: String,
    /// How this item connects to the host: `"native"`, `"serial"`, `"mqtt"`, etc.
    pub transport: String,
    /// Serial port path, if applicable.
    pub path: Option<String>,
    /// MQTT node ID, if applicable.
    pub node_id: Option<String>,
    /// Operator-assigned role.  When `Unassigned`, the planner infers the role.
    #[serde(default)]
    pub role: ItemRole,
    /// Accessories (sensors, modules) connected to this board.
    #[serde(default)]
    pub accessories: Vec<String>,
    /// Capabilities provided by this item.  When empty, looked up from the registry.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl HardwareItem {
    /// Create a new hardware item with the given board name.
    pub fn new(name: impl Into<String>, board_name: impl Into<String>, transport: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            board_name: board_name.into(),
            transport: transport.into(),
            path: None,
            node_id: None,
            role: ItemRole::Unassigned,
            accessories: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    /// Assign an explicit role to this item.
    pub fn with_role(mut self, role: ItemRole) -> Self {
        self.role = role;
        self
    }

    /// Add accessories to this item.
    pub fn with_accessories(mut self, accessories: Vec<String>) -> Self {
        self.accessories = accessories;
        self
    }

    /// Override capabilities (e.g., for non-registry boards).
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Resolve capabilities: use explicit list if provided, otherwise look up
    /// from the board registry.
    pub fn resolved_capabilities(&self) -> Vec<String> {
        if !self.capabilities.is_empty() {
            return self.capabilities.clone();
        }
        // Look up in board registry
        use crate::peripherals::registry::{known_accessories, known_boards};
        let board_caps: Vec<String> = known_boards()
            .iter()
            .filter(|b| b.name == self.board_name.as_str())
            .flat_map(|b| b.capabilities.iter().map(|s| s.to_string()))
            .collect();

        // Also collect accessory capabilities
        let acc_caps: Vec<String> = self
            .accessories
            .iter()
            .flat_map(|acc_name| {
                known_accessories()
                    .iter()
                    .filter(move |a| a.name == acc_name.as_str())
                    .flat_map(|a| a.capabilities.iter().map(|s| s.to_string()))
            })
            .collect();

        let mut all: Vec<String> = board_caps;
        for cap in acc_caps {
            if !all.contains(&cap) {
                all.push(cap);
            }
        }
        all
    }

    /// Check if this item provides a specific capability.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.resolved_capabilities()
            .iter()
            .any(|c| c.as_str() == cap)
    }
}

// ── Hardware Inventory ────────────────────────────────────────────────────────

/// A complete description of the hardware available for a deployment, plus the
/// feature desires the operator wants to fulfil.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInventory {
    /// Human-readable name for this deployment scenario.
    pub scenario_name: String,
    /// The hardware items (boards and accessories) available.
    pub items: Vec<HardwareItem>,
    /// The high-level features the operator wants to achieve.
    pub feature_desires: Vec<FeatureDesire>,
}

impl HardwareInventory {
    /// Create a new, empty inventory with the given scenario name.
    pub fn new(scenario_name: impl Into<String>) -> Self {
        Self {
            scenario_name: scenario_name.into(),
            items: Vec::new(),
            feature_desires: Vec::new(),
        }
    }

    /// Add a hardware item to the inventory.
    pub fn add_item(&mut self, item: HardwareItem) {
        self.items.push(item);
    }

    /// Add a feature desire to the inventory.
    pub fn add_desire(&mut self, desire: FeatureDesire) {
        self.feature_desires.push(desire);
    }

    /// Return the first item whose role matches, if any.
    pub fn find_role(&self, role: &ItemRole) -> Option<&HardwareItem> {
        self.items.iter().find(|i| &i.role == role)
    }

    /// Return all items that provide a given capability.
    pub fn items_with_capability(&self, cap: &str) -> Vec<&HardwareItem> {
        self.items
            .iter()
            .filter(|i| i.has_capability(cap))
            .collect()
    }

    /// Return all capability tokens provided by the entire inventory (union).
    pub fn all_capabilities(&self) -> Vec<String> {
        let mut caps: Vec<String> = Vec::new();
        for item in &self.items {
            for cap in item.resolved_capabilities() {
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
        }
        caps
    }

    /// Build the standard NanoPi + ESP32-S3 Touch LCD + XIAO + Sipeed mic + DHT22 scenario.
    ///
    /// This is the reference deployment scenario described in the Oh-Ben-Claw
    /// Phase 13 roadmap and matching the hardware list in the problem statement.
    pub fn nanopi_scenario() -> Self {
        let mut inv = Self::new("NanoPi-Neo3 Reference Deployment");

        // ── Host ─────────────────────────────────────────────────────────────
        inv.add_item(
            HardwareItem::new("nanopi-neo3", "nanopi-neo3", "native")
                .with_role(ItemRole::Host)
                .with_accessories(vec!["dht22".to_string()]),
        );

        // ── Display / Sound ───────────────────────────────────────────────────
        inv.add_item(
            HardwareItem::new(
                "waveshare-esp32-s3-touch-lcd-2.1",
                "waveshare-esp32-s3-touch-lcd-2.1",
                "serial",
            )
            .with_role(ItemRole::Display),
        );

        // ── Vision ────────────────────────────────────────────────────────────
        inv.add_item(
            HardwareItem::new("xiao-esp32s3-sense", "xiao-esp32s3-sense", "serial")
                .with_role(ItemRole::Vision),
        );

        // ── Listening ─────────────────────────────────────────────────────────
        inv.add_item(
            HardwareItem::new("sipeed-6plus1-mic-array", "sipeed-6plus1-mic-array", "serial")
                .with_role(ItemRole::Listening),
        );

        // ── Feature desires ───────────────────────────────────────────────────
        inv.add_desire(FeatureDesire::Vision);
        inv.add_desire(FeatureDesire::Listening);
        inv.add_desire(FeatureDesire::Speech);
        inv.add_desire(FeatureDesire::EnvironmentalSensing);
        inv.add_desire(FeatureDesire::DisplayOutput);
        inv.add_desire(FeatureDesire::TouchInput);
        inv.add_desire(FeatureDesire::WirelessMesh);
        inv.add_desire(FeatureDesire::PersistentMemory);

        inv
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_item_resolves_capabilities_from_registry() {
        let item = HardwareItem::new("xiao", "xiao-esp32s3-sense", "serial");
        let caps = item.resolved_capabilities();
        assert!(caps.iter().any(|c| c == "camera_capture"), "expected camera_capture in {:?}", caps);
        assert!(caps.iter().any(|c| c == "audio_sample"), "expected audio_sample in {:?}", caps);
        assert!(caps.iter().any(|c| c == "wifi"), "expected wifi in {:?}", caps);
    }

    #[test]
    fn hardware_item_resolves_accessory_capabilities() {
        let item = HardwareItem::new("host", "nanopi-neo3", "native")
            .with_accessories(vec!["dht22".to_string()]);
        let caps = item.resolved_capabilities();
        assert!(caps.iter().any(|c| c == "sensor_read"), "expected sensor_read from dht22 in {:?}", caps);
    }

    #[test]
    fn hardware_item_has_capability_check() {
        let item = HardwareItem::new("display", "waveshare-esp32-s3-touch-lcd-2.1", "serial");
        assert!(item.has_capability("display"));
        assert!(item.has_capability("touch"));
        assert!(!item.has_capability("camera_capture"));
    }

    #[test]
    fn inventory_finds_items_by_role() {
        let inv = HardwareInventory::nanopi_scenario();
        assert!(inv.find_role(&ItemRole::Host).is_some());
        assert_eq!(inv.find_role(&ItemRole::Host).unwrap().board_name, "nanopi-neo3");
        assert!(inv.find_role(&ItemRole::Vision).is_some());
        assert!(inv.find_role(&ItemRole::Listening).is_some());
    }

    #[test]
    fn inventory_items_with_capability() {
        let inv = HardwareInventory::nanopi_scenario();
        let vision_items = inv.items_with_capability("camera_capture");
        assert_eq!(vision_items.len(), 1);
        assert_eq!(vision_items[0].board_name, "xiao-esp32s3-sense");
    }

    #[test]
    fn nanopi_scenario_has_all_expected_hardware() {
        let inv = HardwareInventory::nanopi_scenario();
        assert_eq!(inv.items.len(), 4);
        let names: Vec<_> = inv.items.iter().map(|i| i.board_name.as_str()).collect();
        assert!(names.contains(&"nanopi-neo3"));
        assert!(names.contains(&"waveshare-esp32-s3-touch-lcd-2.1"));
        assert!(names.contains(&"xiao-esp32s3-sense"));
        assert!(names.contains(&"sipeed-6plus1-mic-array"));
    }

    #[test]
    fn nanopi_scenario_has_feature_desires() {
        let inv = HardwareInventory::nanopi_scenario();
        assert!(inv.feature_desires.contains(&FeatureDesire::Vision));
        assert!(inv.feature_desires.contains(&FeatureDesire::Listening));
        assert!(inv.feature_desires.contains(&FeatureDesire::EnvironmentalSensing));
    }

    #[test]
    fn feature_desire_required_capabilities() {
        assert_eq!(FeatureDesire::Vision.required_capabilities(), &["camera_capture"]);
        assert_eq!(FeatureDesire::Listening.required_capabilities(), &["audio_sample"]);
        assert_eq!(FeatureDesire::DisplayOutput.required_capabilities(), &["display"]);
    }
}

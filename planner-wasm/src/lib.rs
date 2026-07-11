//! Oh-Ben-Claw deployment planner as WebAssembly — Ecosystem Integration **I5**.
//!
//! This crate compiles the *same source files* as the native runtime's
//! registry / deployment planner / site optimizer (via `include!` of the pure,
//! serde-only closure: `peripherals::registry`, `geo`, `siteplan`,
//! `deployment::{inventory, advisor, firmware_scaffold, scheme, planner}`) and
//! exposes them to JavaScript through `wasm-bindgen`. No re-implementation —
//! drift between the runtime and JS consumers is impossible by construction.
//!
//! Excluded on purpose (not wasm-compatible, not needed for planning):
//! `geo::anchor` (SQLite world memory; cfg-gated off on wasm32),
//! `deployment::swarm` (LLM providers) and `deployment::saga` (async runtime) —
//! the shim below simply never declares them.
//!
//! Build:
//! ```bash
//! rustup target add wasm32-unknown-unknown
//! npx wasm-pack build planner-wasm --target nodejs --out-dir pkg
//! ```
//!
//! The JS API is JSON-in/JSON-out, matching the shared fixture contract in
//! `tests/fixtures/` (Ecosystem Integration I2): the golden files are the
//! acceptance tests for this package on both sides of the wire.

// ── Source shim: the pure planner closure, compiled verbatim via #[path] ──────
// Inside an inline module block, #[path] resolves relative to the enclosing
// file's directory *plus* the inline module components — hence the extra `..`.

pub mod peripherals {
    #[path = "../../../src/peripherals/registry.rs"]
    pub mod registry;
}

#[path = "../../src/geo/mod.rs"]
pub mod geo;

#[path = "../../src/siteplan/mod.rs"]
pub mod siteplan;

pub mod deployment {
    #[path = "../../../src/deployment/advisor.rs"]
    pub mod advisor;
    #[path = "../../../src/deployment/firmware_scaffold.rs"]
    pub mod firmware_scaffold;
    #[path = "../../../src/deployment/inventory.rs"]
    pub mod inventory;
    #[path = "../../../src/deployment/planner.rs"]
    pub mod planner;
    #[path = "../../../src/deployment/scheme.rs"]
    pub mod scheme;
}

// ── Core API (target-independent; String errors) ──────────────────────────────

/// The JSON-in/JSON-out planner API. Pure Rust — used directly by native
/// tests, wrapped for JS below. `JsValue` never appears here because
/// constructing one aborts off-wasm.
pub mod api {
    use crate::deployment::inventory::HardwareInventory;
    use crate::deployment::planner::DeploymentPlanner;
    use crate::geo::Site;
    use crate::siteplan::{plan_site as run_plan_site, PlacementSpec};

    /// The live hardware registry as JSON — identical to `emit-registry` output.
    pub fn registry_json() -> Result<String, String> {
        crate::peripherals::registry::registry_json().map_err(|e| e.to_string())
    }

    /// Run the rule-based deployment planner on a `HardwareInventory` JSON
    /// document (the shared fixture shape); returns the `DeploymentScheme` JSON.
    pub fn plan_deployment(inventory_json: &str) -> Result<String, String> {
        let inventory: HardwareInventory =
            serde_json::from_str(inventory_json).map_err(|e| e.to_string())?;
        let scheme = DeploymentPlanner::plan(&inventory);
        serde_json::to_string(&scheme).map_err(|e| e.to_string())
    }

    /// Render the paste-ready `[deployment]` TOML for an inventory JSON
    /// document (byte-identical to the native `to_deployment_toml`).
    pub fn deployment_toml(inventory_json: &str) -> Result<String, String> {
        let inventory: HardwareInventory =
            serde_json::from_str(inventory_json).map_err(|e| e.to_string())?;
        Ok(inventory.to_deployment_toml())
    }

    /// Run the site coverage optimizer on a `{site, spec}` JSON case (the
    /// shared fixture shape); returns `{plan, toml, summary}` JSON.
    pub fn plan_site(case_json: &str) -> Result<String, String> {
        #[derive(serde::Deserialize)]
        struct Case {
            site: Site,
            spec: PlacementSpec,
        }
        let case: Case = serde_json::from_str(case_json).map_err(|e| e.to_string())?;
        let plan = run_plan_site(&case.site, &case.spec);
        let toml = plan.to_toml(&case.site.id);
        let summary = plan.summary();
        serde_json::to_string(&serde_json::json!({
            "plan": plan,
            "toml": toml,
            "summary": summary,
        }))
        .map_err(|e| e.to_string())
    }
}

// ── JS bindings (thin wrappers; JsValue only materializes on wasm) ────────────

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn registry_json() -> Result<String, JsValue> {
    api::registry_json().map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen]
pub fn registry_schema_version() -> u32 {
    peripherals::registry::REGISTRY_SCHEMA_VERSION
}

#[wasm_bindgen]
pub fn plan_deployment(inventory_json: &str) -> Result<String, JsValue> {
    api::plan_deployment(inventory_json).map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen]
pub fn deployment_toml(inventory_json: &str) -> Result<String, JsValue> {
    api::deployment_toml(inventory_json).map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen]
pub fn plan_site(case_json: &str) -> Result<String, JsValue> {
    api::plan_site(case_json).map_err(|e| JsValue::from_str(&e))
}

#[cfg(test)]
mod wasm_api_tests {
    //! Native-target tests of the shim + core API (fast feedback without a
    //! wasm VM). These exercise `api::*` — the JsValue wrappers are
    //! wasm-runtime-only.
    use super::api;
    use super::deployment::inventory::HardwareInventory;

    #[test]
    fn registry_serializes_through_the_shim() {
        let j = api::registry_json().unwrap();
        assert!(j.contains("\"schema_version\""));
        assert!(j.contains("\"esp32-s3\""));
    }

    #[test]
    fn nanopi_inventory_plans_and_renders_toml() {
        let inv = HardwareInventory::nanopi_scenario();
        let inv_json = serde_json::to_string(&inv).unwrap();

        let scheme_json = api::plan_deployment(&inv_json).unwrap();
        let scheme: serde_json::Value = serde_json::from_str(&scheme_json).unwrap();
        assert!(scheme["assignments"].as_array().unwrap().len() >= 2);

        let toml = api::deployment_toml(&inv_json).unwrap();
        assert!(toml.starts_with("[deployment]\n"));
        assert_eq!(toml, inv.to_deployment_toml(), "same source, same output");
    }

    #[test]
    fn siteplan_case_runs_through_the_json_api() {
        let case = r#"{
            "site": {
                "id": "sq", "name": "",
                "origin": {"lat": 45.5, "lon": -122.6, "alt": 0.0},
                "boundary": [
                    {"lat": 45.49955, "lon": -122.60064, "alt": 0.0},
                    {"lat": 45.49955, "lon": -122.59936, "alt": 0.0},
                    {"lat": 45.50045, "lon": -122.59936, "alt": 0.0},
                    {"lat": 45.50045, "lon": -122.60064, "alt": 0.0}
                ],
                "dem_ref": null
            },
            "spec": {
                "budget": 4, "detection_radius_m": 30.0, "min_spacing_m": 20.0,
                "mesh_range_m": 250.0, "lattice_step_m": 10.0, "demand_step_m": 10.0,
                "require_mesh_connectivity": true, "node_height_m": 3.0
            }
        }"#;
        let out = api::plan_site(case).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["toml"].as_str().unwrap().contains("[[site.node]]"));
        assert!(v["plan"]["mesh_connected"].as_bool().unwrap());
    }

    #[test]
    fn bad_json_is_a_clean_error() {
        assert!(api::plan_deployment("not json").is_err());
        assert!(api::plan_site("{}").is_err());
    }
}

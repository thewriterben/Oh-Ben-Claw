//! Cross-repo planner parity goldens — Ecosystem Integration **I2**.
//!
//! The fixtures in `tests/fixtures/` are the shared contract between this
//! runtime (Rust) and the OBC-deployment-generator (TypeScript): the same
//! `inventory.json` / `case.json` inputs must render byte-identical TOML in
//! both implementations. The generator repo carries a copy of the fixtures
//! (`tests/fixtures/`, synced like `registry.json`) and runs the same
//! comparisons in Vitest — drift in either implementation fails one suite.
//!
//! Regenerate (bless) the goldens after an intentional change:
//!
//! ```bash
//! OBC_BLESS=1 cargo test --test planner_parity
//! # then copy tests/fixtures/ to OBC-deployment-generator/tests/fixtures/
//! ```

use oh_ben_claw::config::DeploymentConfig;
use oh_ben_claw::deployment::inventory::HardwareInventory;
use oh_ben_claw::geo::Site;
use oh_ben_claw::siteplan::{plan_site, PlacementSpec};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn bless() -> bool {
    std::env::var("OBC_BLESS").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "missing fixture {} ({e}) — bless with `OBC_BLESS=1 cargo test --test planner_parity`",
            path.display()
        )
    })
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

// ── Deployment: inventory.json → expected-deployment.toml ─────────────────────

#[test]
fn deployment_toml_matches_golden() {
    let dir = fixtures_dir().join("deployment/nanopi");
    let inv_path = dir.join("inventory.json");
    let expected_path = dir.join("expected-deployment.toml");

    if bless() {
        let inv = HardwareInventory::nanopi_scenario();
        write(
            &inv_path,
            &format!("{}\n", serde_json::to_string_pretty(&inv).unwrap()),
        );
        write(&expected_path, &inv.to_deployment_toml());
        eprintln!("blessed {}", dir.display());
    }

    let inv: HardwareInventory = serde_json::from_str(&read(&inv_path)).unwrap();
    let rendered = inv.to_deployment_toml();
    assert_eq!(
        norm(&read(&expected_path)),
        norm(&rendered),
        "deployment TOML drifted from the golden — if intentional, re-bless \
         and sync the generator's fixture copy"
    );
}

/// Paste-ready guarantee: the emitted `[deployment]` section must parse into
/// the real runtime config types.
#[test]
fn deployment_toml_is_paste_ready_for_the_runtime() {
    #[derive(Deserialize)]
    struct Wrapper {
        deployment: DeploymentConfig,
    }

    let inv = HardwareInventory::nanopi_scenario();
    let toml_src = inv.to_deployment_toml();
    let parsed: Wrapper =
        toml::from_str(&toml_src).expect("emitted [deployment] TOML parses into DeploymentConfig");

    assert!(parsed.deployment.enabled);
    assert_eq!(parsed.deployment.scenario, inv.scenario_name);
    assert_eq!(parsed.deployment.hardware.len(), inv.items.len());
    assert_eq!(
        parsed.deployment.feature_desires.len(),
        inv.feature_desires.len()
    );
    // Field-level spot checks against the first item.
    let first = &parsed.deployment.hardware[0];
    assert_eq!(first.name, inv.items[0].name);
    assert_eq!(first.board_name, inv.items[0].board_name);
    assert_eq!(first.transport, inv.items[0].transport);
    assert_eq!(first.role, inv.items[0].role.to_string());
    assert_eq!(first.accessories, inv.items[0].accessories);
}

// ── Siteplan: case.json → expected-site.toml ──────────────────────────────────

#[derive(serde::Serialize, Deserialize)]
struct SiteplanCase {
    site: Site,
    spec: PlacementSpec,
}

fn square_case() -> SiteplanCase {
    // The ~100 m × 100 m square near (45.5, -122.6) used across both repos.
    let dlat = 50.0 / 111_194.9;
    let dlon = 50.0 / (111_194.9 * 45.5_f64.to_radians().cos());
    let (lat, lon) = (45.5, -122.6);
    let site = Site::new(
        "square",
        "parity square",
        vec![
            oh_ben_claw::geo::GeoPoint::new(lat - dlat, lon - dlon, 0.0),
            oh_ben_claw::geo::GeoPoint::new(lat - dlat, lon + dlon, 0.0),
            oh_ben_claw::geo::GeoPoint::new(lat + dlat, lon + dlon, 0.0),
            oh_ben_claw::geo::GeoPoint::new(lat + dlat, lon - dlon, 0.0),
        ],
    );
    let spec = PlacementSpec {
        budget: 4,
        ..Default::default()
    };
    SiteplanCase { site, spec }
}

#[test]
fn siteplan_toml_matches_golden() {
    let dir = fixtures_dir().join("siteplan/square");
    let case_path = dir.join("case.json");
    let expected_path = dir.join("expected-site.toml");

    if bless() {
        let case = square_case();
        write(
            &case_path,
            &format!("{}\n", serde_json::to_string_pretty(&case).unwrap()),
        );
        let plan = plan_site(&case.site, &case.spec);
        write(&expected_path, &plan.to_toml(&case.site.id));
        eprintln!("blessed {}", dir.display());
    }

    let case: SiteplanCase = serde_json::from_str(&read(&case_path)).unwrap();
    let plan = plan_site(&case.site, &case.spec);
    let rendered = plan.to_toml(&case.site.id);
    assert_eq!(
        norm(&read(&expected_path)),
        norm(&rendered),
        "siteplan TOML drifted from the golden — the TS port \
         (OBC-deployment-generator lib/siteplan.ts) must stay in lockstep"
    );
    // Sanity: the golden is a real, useful plan.
    assert!(plan.nodes.len() >= 2);
    assert!(plan.mesh_connected);
}

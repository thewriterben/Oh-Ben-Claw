//! `[deployment]` is a write-only schema no longer.
//!
//! `HardwareInventory::to_deployment_toml` emits the block. The TypeScript
//! emitter in OBC-deployment-generator emits a byte-identical copy. Golden
//! fixtures in `tests/fixtures/deployment/` pin both, and OBC-Prime hashes those
//! fixtures across three repositories.
//!
//! And until 2026-07-30 nothing ever read one back. The only consumer of
//! `config.deployment` in the entire tree was `planner_parity.rs` asserting that
//! emitted TOML deserialises into `DeploymentConfig` — which checks serde, not
//! the contract. A field could be emitted under one name and read under another,
//! or dropped on the way in, and every one of those gates would stay green.
//!
//! `from_deployment_config` closes the loop, so the contract can be stated as a
//! fixed point instead of a parse:
//!
//! ```text
//! inventory → emit → parse → rebuild → emit   ==   inventory → emit
//! ```
//!
//! That is the strongest form of the claim the goldens are reaching for, and it
//! is the form that catches an asymmetric change to either direction.

use oh_ben_claw::config::{Config, DeploymentConfig};
use oh_ben_claw::deployment::{HardwareInventory, ItemRole};

/// Parse a `[deployment]`-only TOML document into the config type.
fn parse_block(toml_src: &str) -> DeploymentConfig {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        deployment: DeploymentConfig,
    }
    let w: Wrapper = toml::from_str(toml_src)
        .unwrap_or_else(|e| panic!("emitted [deployment] does not parse: {e}\n---\n{toml_src}"));
    w.deployment
}

#[test]
fn emit_parse_rebuild_emit_is_a_fixed_point() {
    let original = HardwareInventory::nanopi_scenario();
    let once = original.to_deployment_toml();

    let rebuilt = HardwareInventory::from_deployment_config(&parse_block(&once));
    let twice = rebuilt.to_deployment_toml();

    assert_eq!(
        once, twice,
        "the [deployment] block does not survive a round trip. Whatever differs \
         below is a field one direction knows about and the other does not — and \
         because the emitter is mirrored in TypeScript and pinned by goldens, \
         fixing the emitter is a cross-repository change. Fix the reader first \
         and check that it was not the reader that was right."
    );
}

/// The fixed-point test above would pass on two empty strings. This is the
/// control: the fixture is not vacuous.
///
/// Written because three tests in `learning_approval_gate.rs` passed on
/// `[] == []` a day before this file existed.
#[test]
fn proof_the_fixture_is_hot() {
    let inv = HardwareInventory::nanopi_scenario();
    let emitted = inv.to_deployment_toml();

    assert!(
        inv.items.len() >= 4,
        "fixture shrank: {} items",
        inv.items.len()
    );
    assert!(
        !inv.feature_desires.is_empty(),
        "fixture has no feature desires, so the desire round-trip asserts nothing"
    );
    assert!(
        emitted.contains("[[deployment.hardware]]"),
        "emitted block has no hardware table:\n{emitted}"
    );
    assert!(
        emitted.contains("role = "),
        "emitted block assigns no roles, so ItemRole::from_str is untested by the \
         round trip"
    );
}

/// Fields are compared by value, not just by the emitted text, because two
/// different inventories could in principle render identically if a field were
/// dropped from the emitter *and* the reader at the same time.
#[test]
fn rebuilt_items_match_field_by_field() {
    let original = HardwareInventory::nanopi_scenario();
    let rebuilt =
        HardwareInventory::from_deployment_config(&parse_block(&original.to_deployment_toml()));

    assert_eq!(rebuilt.scenario_name, original.scenario_name);
    assert_eq!(rebuilt.feature_desires, original.feature_desires);
    assert_eq!(rebuilt.items.len(), original.items.len());

    for (r, o) in rebuilt.items.iter().zip(original.items.iter()) {
        assert_eq!(r.name, o.name);
        assert_eq!(r.board_name, o.board_name, "{}: board_name", o.name);
        assert_eq!(r.transport, o.transport, "{}: transport", o.name);
        assert_eq!(r.path, o.path, "{}: path", o.name);
        assert_eq!(r.node_id, o.node_id, "{}: node_id", o.name);
        assert_eq!(r.role, o.role, "{}: role", o.name);
        assert_eq!(r.accessories, o.accessories, "{}: accessories", o.name);

        // Deliberately NOT round-tripped: capabilities come from the board
        // registry, and carrying them in the config would let a stale file
        // override it. Asserted so that "empty" stays a decision rather than
        // becoming a bug someone silently fixes.
        assert!(
            r.capabilities.is_empty(),
            "{}: capabilities were round-tripped through the config. They must \
             come from the registry — see from_deployment_config.",
            o.name
        );
    }
}

/// `to_deployment_toml` writes roles with `Display`; the reader parses them with
/// `FromStr`. Those are two hand-written match arms and nothing forces them to
/// agree, so this walks every variant.
#[test]
fn every_item_role_survives_display_then_parse() {
    let all = [
        ItemRole::Host,
        ItemRole::Display,
        ItemRole::Vision,
        ItemRole::Listening,
        ItemRole::Sensing,
        ItemRole::Peripheral,
        ItemRole::Console,
        ItemRole::Unassigned,
    ];

    for role in all {
        let token = role.to_string();
        let back: ItemRole = token.parse().expect("FromStr is infallible");
        assert_eq!(
            back, role,
            "ItemRole::{role:?} renders as {token:?} and parses back as {back:?} — \
             Display and FromStr have diverged"
        );
    }

    // Unknown text is Unassigned, not a startup failure: an unrecognised role is
    // a hint the planner can re-infer.
    let unknown: ItemRole = "definitely-not-a-role".parse().unwrap();
    assert_eq!(unknown, ItemRole::Unassigned);
}

/// The `[deployment]` example in the `DeploymentConfig` doc comment must parse.
///
/// It did not. Three keys per line separated by semicolons is not TOML, and it
/// sat in `src/config/mod.rs` from Phase 13 until someone pasted it. This repo
/// has shipped a `[[safety.limit]]` that silently did nothing because the real
/// key was `[[safety.limits]]`, and a generator config carrying two keys the
/// agent does not have. Documented configuration is prose until something
/// executes it, so this executes it.
#[test]
fn the_doc_comment_example_parses() {
    // The types moved to crates/obc-planner on 2026-07-30, so they sit beside
    // the emitter and the reader rather than a crate away from both. This test
    // followed them, and its own failure message is what said so: it read
    // "DeploymentConfig moved; this test can no longer find its doc comment",
    // which is a better outcome than passing because the string it searched for
    // happened to be absent.
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/obc-planner/src/config.rs"
    ))
    .expect("crates/obc-planner/src/config.rs");

    let anchor = src
        .find("pub struct DeploymentConfig")
        .expect("DeploymentConfig moved; this test can no longer find its doc comment");
    let doc = &src[..anchor];
    let start = doc
        .rfind("/// ```toml")
        .expect("the DeploymentConfig doc comment has no toml example any more");
    let body = &doc[start + "/// ```toml".len()..];
    let end = body
        .find("/// ```")
        .expect("unterminated toml fence in the DeploymentConfig doc comment");

    let example: String = body[..end]
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("///"))
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        example.contains("[[deployment.hardware]]"),
        "extracted the wrong block from the doc comment:\n{example}"
    );

    let parsed = parse_block(&example);
    assert!(
        !parsed.hardware.is_empty(),
        "the documented example parses but describes no hardware"
    );

    // And it must be a *valid agent config*, not merely valid TOML — the example
    // is there to be pasted into config.toml.
    let _: Config = toml::from_str(&example).expect(
        "the documented [deployment] example parses alone but not as part of a \
         Config — it cannot be pasted into config.toml as the doc implies",
    );
}

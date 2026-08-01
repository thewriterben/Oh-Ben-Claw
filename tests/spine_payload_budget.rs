//! What actually goes on the mesh, in bytes — step 1 of the spine-auth plan.
//!
//! `OBC-Prime/docs/SPINE-AUTH.md` proposes appending a truncated HMAC and a
//! monotonic counter to every frame, and closes with: *"Measure the payload
//! distribution on the bench mesh. If typical frames sit near 240 bytes,
//! everything above needs rethinking before it is built. Step 1 is a morning and
//! could invalidate steps 2–4."*
//!
//! A radio capture would need the bench. This measures the same thing one layer
//! up and without hardware: the host builds every one of these payloads, so the
//! distribution is a property of the code, not of the air. It is also a standing
//! gate rather than a morning's number — the answer stops being true the moment
//! someone adds a field, and a one-off measurement in a document would not
//! notice.
//!
//! Budgets:
//! - **today** — `MAX_PAYLOAD` = 240 bytes, the node's line framer discards
//!   anything longer *whole*.
//! - **v2** — 240 − 12 for the proposed `[ctr:u32]` + `[mac:8]`, so 228 bytes.
//!
//! What it found, in the order it matters:
//!
//! 1. **The tag is affordable for the traffic that matters.** Actuation
//!    (`gpio_write`, `sensor_read`, `capabilities`), reflex ticks, fleet
//!    heartbeats and assignments all leave 45–169 bytes spare under v2.
//! 2. **It is not free.** `set_limits` with one allowed pin lands on *exactly*
//!    228 bytes — zero spare — and with two pins it goes 2 bytes over. That is a
//!    real command the mesh carries today and would stop carrying.
//! 3. **The saving is already in the frame.** `mesh_command` spends **36 bytes
//!    on a UUIDv4 correlation id**, three times the entire tag, on a link where
//!    the id only has to be unique among a handful of in-flight requests.
//!    Shortening it pays for authentication with 20 bytes left over.
//! 4. **One config-push command never fit at all.** `set_reflex_rules` with a
//!    single modest rule is 344 bytes — 104 over today's frame, before any
//!    authentication. `mesh_command` accepts any `cmd` a model names, so this is
//!    reachable, and until 2026-08-01 it returned `sent: true` and vanished into
//!    the node's line framer. That is now refused host-side.
//!
//! So step 1 does not invalidate steps 2–4, which was the thing worth knowing
//! before building them. It does add a prerequisite the design did not have: cut
//! the correlation id in the same change, or `set_limits` becomes collateral.

#[path = "../firmware/heltec-lora-linktest/src/spine.rs"]
mod spine;

// Command bodies are built from the real types rather than hand-written JSON —
// a census of a shape nobody serializes is a census of nothing.
//
// These are the *host* types on purpose. The host is what puts bytes on the
// wire, so the host's serialization is the measurement; the node's mirrors carry
// fewer fields (no `fire_on_change`), and measuring those would flatter the
// result. `firmware/obc-esp32-s3/src/safety.rs` already pins the two wire
// formats together in `wire_format_matches_the_host_limit_json`.
use oh_ben_claw::agent::reflex::{Action, Cmp, Condition, ReflexRule};
use oh_ben_claw::security::limits::SafetyLimit;
use oh_ben_claw::spine::lora_gateway::{NodeCommand, MESH_LINE_BUDGET};
use oh_ben_claw::spine::lora_mesh::MeshFrame;
use serde_json::json;

/// Bytes the proposed v2 frame spends on `[ctr:u32]` + `[mac:8]`.
const V2_TAG: usize = 12;
const V2_BUDGET: usize = MESH_LINE_BUDGET - V2_TAG;

/// A node id of the length this fleet actually uses.
const NODE: &str = "obc-esp32-s3-001";
/// A UUIDv4, as `mesh_command` generates with `Uuid::new_v4().to_string()`.
const UUID: &str = "6f1a3c58-2b7d-4e69-9a10-c4d2e8f70b53";

/// One reflex rule as the node deserializes it — a modest one: a single sensor
/// condition, one gpio action, a debounce, no rate cap.
fn one_rule() -> serde_json::Value {
    serde_json::to_value(ReflexRule {
        id: "vent-on-heat".to_string(),
        when: Condition::Sensor {
            entity: "temperature".to_string(),
            op: Cmp::Gt,
            value: 35.0,
        },
        then: Action::GpioWrite {
            node_id: NODE.to_string(),
            pin: 3,
            value: 1,
        },
        debounce_ms: 60_000,
        max_rate_hz: None,
        fire_on_change: false,
    })
    .expect("a ReflexRule serializes")
}

/// One safety limit as the host pushes it.
fn limit_for(pins: Vec<i64>) -> serde_json::Value {
    serde_json::to_value(SafetyLimit {
        node_id: NODE.to_string(),
        tool: "gpio_write".to_string(),
        allowed_pins: Some(pins),
        value_min: Some(0),
        value_max: Some(1),
        min_interval_ms: Some(250),
    })
    .expect("a SafetyLimit serializes")
}

fn one_limit() -> serde_json::Value {
    limit_for(vec![3])
}

fn two_pin_limit() -> serde_json::Value {
    limit_for(vec![3, 4])
}

/// Whether a payload is something the mesh is expected to carry.
///
/// Three states rather than two, because the middle one is the finding: a
/// payload can fit the frame we have and not fit the frame the auth design
/// proposes. Collapsing that into "too big" would have hidden the only thing
/// step 1 was asked to discover.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Carriage {
    /// Fits a frame today, and with the proposed tag.
    Mesh,
    /// Fits today; the 12-byte tag would push it over. A casualty of v2, not of
    /// the present.
    V2Casualty(&'static str),
    /// Does not fit today either. Recorded rather than hidden: `mesh_command`
    /// accepts any `cmd` the model names, so these are reachable, and until
    /// 2026-08-01 they returned `sent: true` and vanished.
    TooBig(&'static str),
}

struct Row {
    name: &'static str,
    bytes: usize,
    carriage: Carriage,
}

fn census() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // ── Host → node, the actuating direction ────────────────────────────────
    // Shapes taken from the real call sites: `tools/builtin/mesh.rs` (UUID id),
    // `spine/mesh_supervisor.rs` (`sup-{node}-{ms}` id), and the node's own
    // dispatcher in `firmware/obc-esp32-s3/src/main.rs`.
    let mut push = |name, id: &str, c: &str, args, carriage| {
        rows.push(Row {
            name,
            bytes: NodeCommand::new(NODE, id, c, args).encoded_len(),
            carriage,
        })
    };

    push(
        "capabilities (uuid id)",
        UUID,
        "capabilities",
        json!({}),
        Carriage::Mesh,
    );
    push(
        "gpio_write (uuid id)",
        UUID,
        "gpio_write",
        json!({"pin": 3, "value": 1}),
        Carriage::Mesh,
    );
    push(
        "sensor_read (uuid id)",
        UUID,
        "sensor_read",
        json!({"sensor": "temperature"}),
        Carriage::Mesh,
    );
    push(
        "supervisor recovery probe",
        "sup-obc-esp32-s3-001-1785549000000",
        "announce",
        json!({}),
        Carriage::Mesh,
    );
    push(
        "reflex_tick, four quantities",
        UUID,
        "reflex_tick",
        json!({"snapshot": {
            "temperature": 21.5, "humidity": 48.2, "battery_soc": 91.0, "pressure": 1013.2
        }}),
        Carriage::Mesh,
    );

    // The config-push commands, which is where the census stops being
    // reassuring. These are MQTT/serial-shaped and they are in here because
    // `mesh_command` accepts any `cmd` string a model names, so they are
    // reachable over the mesh whether or not anyone intended them to be.
    //
    // `set_limits` with one pin lands on exactly 228 bytes — it fits today with
    // 12 to spare and lands on precisely zero under the proposed tag. That is
    // not a comfortable pass; see `the_tightest_mesh_payload_is_at_the_edge`.
    push(
        "set_limits, one pin",
        UUID,
        "set_limits",
        json!({ "limits": [one_limit()] }),
        Carriage::Mesh,
    );
    push(
        "set_limits, two pins",
        UUID,
        "set_limits",
        json!({ "limits": [two_pin_limit()] }),
        Carriage::V2Casualty("230 bytes: fits today, 2 over once the tag is added"),
    );
    push(
        "set_reflex_rules, one rule",
        UUID,
        "set_reflex_rules",
        json!({ "rules": [one_rule()] }),
        Carriage::TooBig("a single modest rule is already 104 bytes over"),
    );

    // ── Node → host, the direction that reaches world memory ────────────────
    // Compact fleet codec (`spine/lora_mesh.rs`), which is what the mesh carries
    // between coordinator and nodes.
    rows.push(Row {
        name: "fleet heartbeat (pose + battery)",
        bytes: MeshFrame::Heartbeat {
            node: NODE.to_string(),
            x: Some(142.755_5),
            y: Some(-87.201_3),
            battery: Some(91.5),
            mode: "explore".to_string(),
        }
        .encode()
        .len(),
        carriage: Carriage::Mesh,
    });
    rows.push(Row {
        name: "fleet assignment",
        bytes: MeshFrame::Assign {
            node: NODE.to_string(),
            x: 142.755_5,
            y: -87.201_3,
        }
        .encode()
        .len(),
        carriage: Carriage::Mesh,
    });

    rows
}

/// The census itself. Prints with `cargo test -- --nocapture`; the assertions
/// below are what run unattended.
#[test]
fn the_payload_census() {
    println!(
        "\n{:<34}{:>7}{:>12}{:>12}",
        "payload", "bytes", "spare/240", "spare/228"
    );
    println!("{}", "-".repeat(65));
    for r in census() {
        let mark = match r.carriage {
            Carriage::Mesh => "",
            Carriage::V2Casualty(_) => " (fits today; the auth tag breaks it)",
            Carriage::TooBig(_) => " (does not fit today either)",
        };
        println!(
            "{:<34}{:>7}{:>12}{:>12}{}",
            r.name,
            r.bytes,
            MESH_LINE_BUDGET as i64 - r.bytes as i64,
            V2_BUDGET as i64 - r.bytes as i64,
            mark
        );
    }
    println!();
}

/// The question step 1 asked, and the answer: **yes, for everything the mesh
/// actually carries.** Actuation, telemetry and fleet coordination leave 45–169
/// bytes spare under the proposed tag.
///
/// If this fails, `SPINE-AUTH.md` §3.2 needs rethinking before anything is
/// built, which is the entire reason step 1 comes first.
#[test]
fn every_mesh_payload_leaves_room_for_the_proposed_auth_tag() {
    for r in census().iter().filter(|r| r.carriage == Carriage::Mesh) {
        assert!(
            r.bytes <= V2_BUDGET,
            "{} is {} bytes; with the {V2_TAG}-byte v2 tag the budget is {V2_BUDGET}",
            r.name,
            r.bytes
        );
    }
}

/// Weaker, and separate on purpose: this failing is a live bug rather than a
/// design question. An over-budget line is discarded whole by the node's framer,
/// so the command simply does not happen.
#[test]
fn everything_that_fits_today_still_does() {
    for r in census() {
        if matches!(r.carriage, Carriage::TooBig(_)) {
            continue;
        }
        assert!(
            r.bytes <= MESH_LINE_BUDGET,
            "{} encodes to {} bytes; the mesh carries {MESH_LINE_BUDGET} and drops the rest",
            r.name,
            r.bytes
        );
    }
}

/// Every classification has to stay earned, in both directions. A payload that
/// shrinks into a smaller category leaves a stale reason behind it, and a stale
/// reason in a census is how a measurement turns back into a belief.
#[test]
fn the_classifications_are_still_true() {
    for r in census() {
        match r.carriage {
            Carriage::Mesh => {}
            Carriage::V2Casualty(why) => {
                assert!(
                    r.bytes <= MESH_LINE_BUDGET && r.bytes > V2_BUDGET,
                    "{} is {} bytes, which is no longer 'fits today, breaks under the tag' \
                     ({why:?}). Reclassify it.",
                    r.name,
                    r.bytes
                );
            }
            Carriage::TooBig(why) => {
                assert!(
                    r.bytes > MESH_LINE_BUDGET,
                    "{} is {} bytes and now fits a frame — {why:?} is no longer the case. \
                     Reclassify it.",
                    r.name,
                    r.bytes
                );
            }
        }
    }
}

/// The actionable half of the census, and the reason the UUID measurement is in
/// here rather than in a note.
///
/// `set_limits` with two allowed pins is the payload the auth tag would break.
/// It is also carrying a 36-byte UUID correlation id. Shortening that id frees
/// three times what the tag costs — so the design does not have to choose
/// between authentication and pushing a two-pin limit over the mesh, provided it
/// spends the saving deliberately rather than discovering it later.
#[test]
fn shortening_the_correlation_id_pays_for_the_tag_and_then_some() {
    let casualties: Vec<_> = census()
        .into_iter()
        .filter(|r| matches!(r.carriage, Carriage::V2Casualty(_)))
        .collect();
    assert!(
        !casualties.is_empty(),
        "no payload is broken by the tag any more — good, but re-read this test's \
         premise before deleting it"
    );

    let uuid_cost = UUID.len() - "a7".len();
    for r in &casualties {
        let over = r.bytes - V2_BUDGET;
        assert!(
            over <= uuid_cost,
            "{} is {over} bytes over the v2 budget, which a shorter correlation id \
             ({uuid_cost} bytes) no longer covers — v2 needs fragmentation, not \
             just a cheaper id",
            r.name
        );
    }
}

/// The pass above is not comfortable, and saying "all clear" without this would
/// be the kind of narrow gate this project keeps catching itself building.
///
/// `set_limits` with a single allowed pin lands on exactly the v2 budget — zero
/// spare. It is the reason `SPINE-AUTH.md` §3.2's "worth measuring against real
/// traffic before committing" is the right instinct: the tag is affordable for
/// every frame that matters and it consumes the entire margin of one that is
/// already at the edge.
#[test]
fn the_tightest_mesh_payload_is_at_the_edge() {
    let tightest = census()
        .into_iter()
        .filter(|r| r.carriage == Carriage::Mesh)
        .max_by_key(|r| r.bytes)
        .expect("the census is not empty");
    assert_eq!(
        tightest.name, "set_limits, one pin",
        "the tightest mesh payload changed; re-read the margin before trusting the \
         auth-tag conclusion"
    );
    assert_eq!(
        V2_BUDGET - tightest.bytes,
        0,
        "the tightest mesh payload no longer sits exactly on the v2 budget \
         ({} bytes); update the note in SPINE-AUTH.md rather than this number",
        tightest.bytes
    );
}

/// The 240 in `lora_gateway.rs` and the 240 in the firmware are the same number
/// living in two workspaces that cannot link to each other. Pin them.
#[test]
fn the_host_budget_matches_the_firmware() {
    assert_eq!(
        MESH_LINE_BUDGET,
        spine::MAX_PAYLOAD,
        "the host's mesh line budget has drifted from the node's MAX_PAYLOAD"
    );
}

/// What the census is for, stated as an assertion so it cannot quietly stop
/// being true: the correlation id costs more than the authentication would.
#[test]
fn the_uuid_correlation_id_costs_three_times_the_proposed_tag() {
    let with_uuid =
        NodeCommand::new(NODE, UUID, "gpio_write", json!({"pin": 3, "value": 1})).encoded_len();
    // Unique among a handful of in-flight requests is all the node needs.
    let with_short =
        NodeCommand::new(NODE, "a7", "gpio_write", json!({"pin": 3, "value": 1})).encoded_len();
    let saved = with_uuid - with_short;
    assert!(
        saved > V2_TAG,
        "a short correlation id saves {saved} bytes, which no longer pays for the \
         {V2_TAG}-byte auth tag — recheck the argument in SPINE-AUTH.md before citing it"
    );
}

/// The host now refuses what the mesh cannot carry. Before this, `mesh_command`
/// returned `sent: true` and the node's framer dropped the line.
#[test]
fn an_over_budget_command_is_refused_rather_than_reported_sent() {
    let big = NodeCommand::new(
        NODE,
        UUID,
        "set_reflex_rules",
        json!({ "rules": vec![one_rule(); 3] }),
    );
    assert!(
        !big.fits_one_frame(),
        "three reflex rules should exceed one frame; if they no longer do, this test \
         is no longer testing the refusal path ({} bytes)",
        big.encoded_len()
    );

    let small = NodeCommand::new(NODE, UUID, "gpio_write", json!({"pin": 3, "value": 1}));
    assert!(small.fits_one_frame());
}

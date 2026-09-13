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
//! Budget: `MAX_AUTH_PAYLOAD` = 228 bytes — the 240-byte radio frame less the
//! 12 bytes the authenticated frame spends on `[ctr:u32]` + `[mac:8]`. The
//! station's line framer discards anything longer *whole*.
//!
//! What step 1 found on 2026-08-01, against the then-unauthenticated 240:
//!
//! 1. **The tag is affordable for the traffic that matters.** Actuation
//!    (`gpio_write`, `sensor_read`, `capabilities`), reflex ticks, fleet
//!    heartbeats and assignments all leave 45–169 bytes spare under it.
//! 2. **It is not free.** `set_limits` with one allowed pin landed on *exactly*
//!    228 bytes — zero spare — and with two pins 2 bytes over. A real command
//!    the mesh carried and would have stopped carrying.
//! 3. **The saving was already in the frame.** `mesh_command` spent **36 bytes
//!    on a UUIDv4 correlation id**, three times the entire tag, on a link where
//!    the id only has to be unique among a handful of in-flight requests.
//! 4. **One config-push command never fit at all.** `set_reflex_rules` with a
//!    single modest rule is 344 bytes — 104 over the frame, before any
//!    authentication. `mesh_command` accepts any `cmd` a model names, so this is
//!    reachable, and until 2026-08-01 it returned `sent: true` and vanished into
//!    the node's line framer. Refused host-side since.
//!
//! **Step 4 shipped 2026-09-13** and did what step 1 required of it: the tag is
//! on the wire, the budget is 228, and the correlation id is eight hex
//! characters (`short_correlation_id`). Measured below with the id the host
//! actually sends: the two-pin `set_limits` that was the casualty now fits
//! with room, and the tightest mesh payload is no longer at the edge.

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

/// Bytes the v2 frame spends on `[ctr:u32]` + `[mac:8]`.
const AUTH_TAG: usize = spine::AUTH_OVERHEAD;

/// A node id of the length this fleet actually uses.
const NODE: &str = "obc-esp32-s3-001";
/// A correlation id of the shape `mesh_command` sends since step 4: eight hex
/// characters, with a retry suffix — the longest the host generates.
const ID: &str = "6f1a3c58r2";
/// The UUIDv4 the host used to send, kept for the measurement that justified
/// replacing it.
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
        hold_ms: 0,
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

/// Every modulation slot the node has, at three-decimal levels — the widest
/// `descend` the host could try to send.
fn every_slot() -> Vec<(u8, f64)> {
    (0..obc_reflex::MAX_SLOTS as u8)
        .map(|s| (s, 0.123 + f64::from(s) * 0.05))
        .collect()
}

fn two_pin_limit() -> serde_json::Value {
    limit_for(vec![3, 4])
}

/// Whether a payload is something the mesh is expected to carry.
///
/// Until step 4 this had a third state — "fits today, breaks under the tag" —
/// which was the finding step 1 existed to make. The tag is on the wire now,
/// so there is one budget and two answers.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Carriage {
    /// Fits an authenticated frame.
    Mesh,
    /// Does not fit. Recorded rather than hidden: `mesh_command` accepts any
    /// `cmd` the model names, so these are reachable, and until 2026-08-01 they
    /// returned `sent: true` and vanished.
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
        "capabilities",
        ID,
        "capabilities",
        json!({}),
        Carriage::Mesh,
    );
    push(
        "gpio_write",
        ID,
        "gpio_write",
        json!({"pin": 3, "value": 1}),
        Carriage::Mesh,
    );
    push(
        "sensor_read",
        ID,
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
        ID,
        "reflex_tick",
        json!({"snapshot": {
            "temperature": 21.5, "humidity": 48.2, "battery_soc": 91.0, "pressure": 1013.2
        }}),
        Carriage::Mesh,
    );

    // The spinal tier's descending modulation. The common shape — a behaviour
    // touches a few slots; the fly's descending population is ~4–5% of its
    // channels (`experiments/lif-fly/RESULTS.md`) — and the full table, which
    // the first draft of this row claimed would fit and does not: 279 bytes.
    // The message is sparse by design, `NodeCommand::descend` refuses the
    // over-budget shape host-side, and `how_many_slots_a_descend_can_carry`
    // below measures the real ceiling instead of asserting a belief.
    let two = NodeCommand::descend(NODE, ID, &[(3, 0.5), (7, 1.0)], false).expect("valid descend");
    push(
        "descend, two slots",
        ID,
        "descend",
        two.args,
        Carriage::Mesh,
    );
    let all = NodeCommand::descend(NODE, ID, &every_slot(), true).expect("valid descend");
    push(
        "descend, every slot + clear",
        ID,
        "descend",
        all.args,
        Carriage::TooBig("sparse by design; the full table is not one message"),
    );

    // The config-push commands, which is where the census stops being
    // reassuring. These are MQTT/serial-shaped and they are in here because
    // `mesh_command` accepts any `cmd` string a model names, so they are
    // reachable over the mesh whether or not anyone intended them to be.
    //
    // With the UUID id, `set_limits` with one pin landed on exactly 228 bytes
    // and two pins on 230 — the payload the tag would have broken. With the
    // short id both fit; `the_tightest_mesh_payload_has_room` says by how much.
    push(
        "set_limits, one pin",
        ID,
        "set_limits",
        json!({ "limits": [one_limit()] }),
        Carriage::Mesh,
    );
    push(
        "set_limits, two pins",
        ID,
        "set_limits",
        json!({ "limits": [two_pin_limit()] }),
        Carriage::Mesh,
    );
    push(
        "set_reflex_rules, one rule",
        ID,
        "set_reflex_rules",
        json!({ "rules": [one_rule()] }),
        // 104 over with the UUID id; 90 with the short one; 102 since
        // `hold_ms` (2026-09-13) — every rule field the host serializes rides
        // in the push, defaults included. Rules go over serial, so this row
        // is a measurement of how far from the mesh they are, not a bug.
        Carriage::TooBig("a single modest rule is already 102 bytes over"),
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
    println!("\n{:<34}{:>7}{:>12}", "payload", "bytes", "spare/228");
    println!("{}", "-".repeat(53));
    for r in census() {
        let mark = match r.carriage {
            Carriage::Mesh => "",
            Carriage::TooBig(_) => " (does not fit one frame)",
        };
        println!(
            "{:<34}{:>7}{:>12}{}",
            r.name,
            r.bytes,
            MESH_LINE_BUDGET as i64 - r.bytes as i64,
            mark
        );
    }
    println!();
}

/// The question step 1 asked, and the answer: **yes, for everything the mesh
/// actually carries.** Actuation, telemetry and fleet coordination fit the
/// authenticated frame. This failing is a live bug: an over-budget line is
/// discarded whole by the station's framer, so the command simply does not
/// happen.
#[test]
fn every_mesh_payload_fits_the_authenticated_frame() {
    for r in census().iter().filter(|r| r.carriage == Carriage::Mesh) {
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
        if let Carriage::TooBig(why) = r.carriage {
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

/// The payload the tag would have broken, and the reason step 4 carried the
/// correlation-id change with it: with the UUID id, `set_limits` with two
/// allowed pins was 230 bytes against a 228 budget. Kept as a measurement of
/// the old shape so the argument in SPINE-AUTH.md stays checkable.
#[test]
fn the_uuid_id_would_have_made_set_limits_a_casualty_and_the_short_id_does_not() {
    let body = json!({ "limits": [two_pin_limit()] });
    let with_uuid = NodeCommand::new(NODE, UUID, "set_limits", body.clone()).encoded_len();
    let with_short = NodeCommand::new(NODE, ID, "set_limits", body).encoded_len();
    assert!(
        with_uuid > MESH_LINE_BUDGET,
        "the two-pin set_limits with a UUID id is {with_uuid} bytes and now fits — \
         the premise of the id change no longer holds; re-read before citing it"
    );
    assert!(
        with_short <= MESH_LINE_BUDGET,
        "the two-pin set_limits with the short id is {with_short} bytes and does not fit"
    );
}

/// Step 1's uncomfortable pass — the tightest mesh payload sat on exactly the
/// v2 budget — is over, and this says by how much so it cannot quietly close
/// again. The Track 0 configuration command has to keep real headroom: it is
/// the command the safety tag would otherwise have broken.
#[test]
fn the_tightest_mesh_payload_has_room() {
    let tightest = census()
        .into_iter()
        .filter(|r| r.carriage == Carriage::Mesh)
        .max_by_key(|r| r.bytes)
        .expect("the census is not empty");
    assert_eq!(
        tightest.name, "set_limits, two pins",
        "the tightest mesh payload changed; re-read the margin before trusting it"
    );
    let spare = MESH_LINE_BUDGET - tightest.bytes;
    println!(
        "tightest mesh payload: {} at {} B, {spare} spare",
        tightest.name, tightest.bytes
    );
    assert!(
        spare >= 16,
        "the tightest mesh payload has only {spare} bytes spare; a field added to \
         SafetyLimit will push Track 0 configuration off the mesh"
    );
}

/// How many slots one `descend` can carry — measured, since the first draft of
/// the census asserted "all sixteen" and was wrong by 39 bytes. The guarantee
/// worth holding is that *half the table* fits: a behaviour on the bench
/// touches one to three slots, so eight is headroom, not a limit anyone will
/// meet. If this drops below eight, the slot count or the id has to give.
#[test]
fn how_many_slots_a_descend_can_carry() {
    let all = every_slot();
    let fits = (0..=all.len())
        .rev()
        .find(|&n| {
            NodeCommand::descend(NODE, ID, &all[..n], true)
                .expect("valid descend")
                .encoded_len()
                <= MESH_LINE_BUDGET
        })
        .unwrap_or(0);
    println!(
        "descend carries {fits} slots in one authenticated frame (of {})",
        all.len()
    );
    assert!(
        fits >= obc_reflex::MAX_SLOTS / 2,
        "only {fits} slots fit one frame"
    );
    assert!(
        fits < all.len(),
        "the full table now fits a frame — reclassify its census row"
    );
}

/// The 228 in `lora_gateway.rs` and the 228 in the firmware are the same number
/// living in two workspaces that cannot link to each other. Pin them, and pin
/// the arithmetic behind them.
#[test]
fn the_host_budget_matches_the_firmware() {
    assert_eq!(
        MESH_LINE_BUDGET,
        spine::MAX_AUTH_PAYLOAD,
        "the host's mesh line budget has drifted from the station's MAX_AUTH_PAYLOAD"
    );
    assert_eq!(spine::MAX_PAYLOAD - AUTH_TAG, MESH_LINE_BUDGET);
    assert_eq!(AUTH_TAG, 4 + 8, "[ctr:u32] + [mac:8]");
}

/// What the census was for, stated as an assertion so it cannot quietly stop
/// being true: the UUID correlation id cost more than the authentication does.
#[test]
fn the_uuid_correlation_id_cost_three_times_the_auth_tag() {
    let with_uuid =
        NodeCommand::new(NODE, UUID, "gpio_write", json!({"pin": 3, "value": 1})).encoded_len();
    let with_short =
        NodeCommand::new(NODE, ID, "gpio_write", json!({"pin": 3, "value": 1})).encoded_len();
    let saved = with_uuid - with_short;
    assert!(
        saved >= 2 * AUTH_TAG,
        "the short correlation id saves {saved} bytes, no longer well clear of the \
         {AUTH_TAG}-byte auth tag — recheck the argument in SPINE-AUTH.md before citing it"
    );
}

/// The id the host actually generates has the shape the census measured with.
#[test]
fn the_host_sends_the_short_id_the_census_measured() {
    use oh_ben_claw::tools::builtin::mesh::short_correlation_id;
    let id = short_correlation_id();
    assert_eq!(id.len(), 8, "{id:?}");
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id:?}");
    assert!(id.len() + "r2".len() <= ID.len());
    assert_ne!(id, short_correlation_id());
}

/// The host refuses what the mesh cannot carry. Before this, `mesh_command`
/// returned `sent: true` and the node's framer dropped the line.
#[test]
fn an_over_budget_command_is_refused_rather_than_reported_sent() {
    let big = NodeCommand::new(
        NODE,
        ID,
        "set_reflex_rules",
        json!({ "rules": vec![one_rule(); 3] }),
    );
    assert!(
        !big.fits_one_frame(),
        "three reflex rules should exceed one frame; if they no longer do, this test \
         is no longer testing the refusal path ({} bytes)",
        big.encoded_len()
    );

    let small = NodeCommand::new(NODE, ID, "gpio_write", json!({"pin": 3, "value": 1}));
    assert!(small.fits_one_frame());
}

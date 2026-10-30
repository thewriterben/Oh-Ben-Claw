//! Host harness for the node's mesh-reply sizing (`mesh_reply.rs`), checked
//! against the bridge that enforces the limit.
//!
//! Both firmware crates build for the MCU, so their tests cannot run there;
//! including the sources here runs them for real, and including *both* means
//! the node's idea of the limit cannot drift from the bridge's.

#[path = "../firmware/obc-esp32-s3/src/mesh_reply.rs"]
mod mesh_reply;
#[path = "../firmware/heltec-lora-linktest/src/spine.rs"]
mod spine;

use serde_json::{json, Value};
use spine::{Framed, LineFramer, MAX_AUTH_PAYLOAD};

/// What the bridge's framer does with one line: forwarded or discarded.
fn bridge_forwards(line: &str) -> bool {
    let mut f = LineFramer::new();
    let mut forwarded = false;
    for &b in line.as_bytes().iter().chain(b"\n") {
        match f.push(b) {
            Framed::Line(_) => forwarded = true,
            Framed::Overflow => return false,
            Framed::Pending => {}
        }
    }
    forwarded
}

fn reply(result: &str) -> Value {
    json!({
        "id": "a1b2c3d4", "ok": true, "result": result,
        "type": "cmd_result", "node_id": "obc-esp32-s3-001",
    })
}

#[test]
fn the_node_and_the_bridge_agree_on_the_limit() {
    assert_eq!(mesh_reply::MESH_LINE_MAX, MAX_AUTH_PAYLOAD);
}

#[test]
fn a_reply_that_fits_goes_out_unchanged() {
    let r = reply("done");
    let line = mesh_reply::fit(&r);
    assert_eq!(line, r.to_string());
    assert!(bridge_forwards(&line));
}

/// The bug: a `capabilities`-sized reply was discarded by the bridge, and the
/// host heard nothing. Now the host hears why, under the same id.
#[test]
fn an_oversized_reply_becomes_an_error_the_bridge_forwards() {
    let big = reply(&"x".repeat(1170));
    assert!(!bridge_forwards(&big.to_string()), "the bug this fixes");

    let line = mesh_reply::fit(&big);
    assert!(bridge_forwards(&line), "{} B: {line}", line.len());
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], "a1b2c3d4");
    assert_eq!(v["ok"], false);
    assert_eq!(v["type"], "cmd_result");
    assert_eq!(v["node_id"], "obc-esp32-s3-001");
    assert!(v["error"].as_str().unwrap().contains("ask over USB"), "{v}");
}

#[test]
fn the_boundary_is_the_bridges_boundary() {
    // Grow a reply one byte at a time across the limit: whatever `fit`
    // returns, the bridge must forward it.
    for n in 150..260 {
        let line = mesh_reply::fit(&reply(&"y".repeat(n)));
        assert!(bridge_forwards(&line), "n={n}: {} B", line.len());
    }
}

#[test]
fn even_an_absurd_node_name_cannot_push_the_error_over() {
    let mut r = reply(&"x".repeat(500));
    r["node_id"] = json!("n".repeat(300));
    let line = mesh_reply::fit(&r);
    assert!(bridge_forwards(&line), "{} B", line.len());
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["id"],
        "a1b2c3d4"
    );
}

#[test]
fn an_id_too_long_to_carry_still_gets_a_line_through() {
    let mut r = reply(&"x".repeat(500));
    r["id"] = json!("i".repeat(300));
    let line = mesh_reply::fit(&r);
    assert!(bridge_forwards(&line), "{} B", line.len());
    let v = serde_json::from_str::<Value>(&line).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["id"].is_null());
}

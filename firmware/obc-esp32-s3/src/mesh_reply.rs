//! A mesh reply that fits the mesh, or says why it does not.
//!
//! A command that arrives over LoRa is answered on the spine UART, and the
//! bridge Heltec frames that line into one radio frame. Its framer discards
//! any line longer than `MAX_AUTH_PAYLOAD` (228 bytes,
//! `firmware/heltec-lora-linktest/src/spine.rs`) and says so only on its own
//! console, which nobody watching the host can see. So an oversized reply
//! vanished, and the host waited out its retries and blamed range or
//! collisions. `capabilities` answers in about 1170 bytes, so it failed this
//! way every time, and it is one of the commands the host retries
//! automatically.
//!
//! [`fit`] passes a reply through when it fits, and otherwise replaces it with
//! a short `ok: false` reply carrying the same `id`, so the host's mesh tool
//! correlates it and reports the real reason. No ESP dependencies: it is
//! compiled for the host by `tests/firmware_mesh_reply.rs`, which also checks
//! [`MESH_LINE_MAX`] against the bridge's own constant.

use serde_json::{json, Value};

/// The longest line the bridge forwards: its `MAX_AUTH_PAYLOAD`.
pub const MESH_LINE_MAX: usize = 228;

/// The line to write on the spine UART for `reply` (no newline).
pub fn fit(reply: &Value) -> String {
    let full = reply.to_string();
    if full.len() <= MESH_LINE_MAX {
        return full;
    }
    let id = reply.get("id").cloned().unwrap_or(Value::Null);
    let error = format!(
        "reply is {} B, over the {MESH_LINE_MAX} B mesh line; ask over USB",
        full.len()
    );
    let mut short = json!({
        "type": "cmd_result",
        "id": id,
        "ok": false,
        "error": error,
    });
    if let Some(node) = reply.get("node_id") {
        short["node_id"] = node.clone();
    }
    let line = short.to_string();
    if line.len() <= MESH_LINE_MAX {
        return line;
    }
    // Only an absurd id or node name gets here. The id still has to travel,
    // or the host cannot tell which command this is about.
    json!({ "type": "cmd_result", "id": id, "ok": false, "error": "reply too big for mesh" })
        .to_string()
}

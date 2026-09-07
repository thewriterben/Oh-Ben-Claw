//! A deliberately narrow-minded MCP server, for testing protocol negotiation.
//!
//! `tests/mcp_protocol_negotiation.rs` needs servers that speak **exactly one**
//! lifecycle and refuse the other, because negotiation can only be proven
//! against a server that can say no. The repo's own `src/mcp/server.rs` is
//! bilingual by design and answers both, so it cannot play either role.
//!
//! It is a `[[bin]]` rather than an `examples/` entry for one reason: cargo sets
//! `CARGO_BIN_EXE_<name>` for integration tests and has no equivalent for
//! examples. That environment variable is what makes the test cross-platform —
//! no shell, no python, no path assumptions.
//!
//! ```text
//! mcp-conformance-server legacy      # requires initialize; refuses 2026 methods
//! mcp-conformance-server stateless   # refuses initialize; answers server/discover
//! mcp-conformance-server quiet-2026  # 2026, but no server/discover (spec-legal)
//! mcp-conformance-server hostile     # answers nothing; both lifecycles fail
//! mcp-conformance-server chatty      # stateless, but prints prose to stdout before every reply
//! mcp-conformance-server loud        # stateless, but writes >64 KiB to stderr before every reply
//! ```
//!
//! `chatty` reproduces what OpenDesignCore's server did on 2026-09-06: its
//! geometry kernel wrote "Disposing Library" to stdout between two frames. The
//! spec says logs go to stderr, so that is a server bug — but a client that
//! drops the connection over it loses every later reply too, and the fix is on
//! the client as well as the server.
//!
//! `loud` is the other half of that lesson (2026-09-07). Once the client
//! captures stderr instead of discarding it, a server that logs more than the
//! pipe holds — 64 KiB on Linux, 4 KiB on some Windows configurations — blocks
//! on `write(2)` unless someone drains the pipe, and then every reply after
//! that is a hang. This role logs a full buffer's worth before each frame so
//! the test fails if the drain ever goes missing.
//!
//! JSON-RPC over stdio, one message per line, synchronous. Errors use -32601
//! (method not found) and -32600 (invalid request), which is what a real server
//! returns for "I don't know that method" and "not in this state".

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Legacy,
    Stateless,
    Quiet2026,
    Hostile,
    Chatty,
    Loud,
}

fn main() {
    let role = match std::env::args().nth(1).as_deref() {
        Some("legacy") => Role::Legacy,
        Some("stateless") => Role::Stateless,
        Some("quiet-2026") => Role::Quiet2026,
        Some("hostile") => Role::Hostile,
        Some("chatty") => Role::Chatty,
        Some("loud") => Role::Loud,
        other => {
            eprintln!(
                "unknown role {other:?}; expected legacy|stateless|quiet-2026|hostile|chatty|loud"
            );
            std::process::exit(2);
        }
    };

    let stdin = io::stdin();
    let mut out = io::stdout();
    let mut initialized = false;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req): Result<Value, _> = serde_json::from_str(&line) else {
            continue;
        };

        let method = req["method"].as_str().unwrap_or("");
        let id = req.get("id").cloned();

        // Notifications get no response. Replying to one desynchronises the
        // client's one-request-one-line read loop: the reply sits in the pipe and
        // is consumed as the answer to the next request, so every later call
        // answers the previous question.
        //
        // `id: null` is treated as a notification here as well as an absent id.
        // Strictly it is a malformed request, and this server *did* reject it —
        // which is how the client's own `"id": null` bug was found. Being
        // tolerant now matches what real servers mostly do, and the client-side
        // fix means the strict path is no longer exercised from this direction.
        if id.is_none() || id.as_ref() == Some(&Value::Null) {
            if method == "notifications/initialized" {
                initialized = true;
            }
            continue;
        }
        let id = id.unwrap();

        let response = match (role, method) {
            // ── The legacy server ────────────────────────────────────────────
            (Role::Legacy, "initialize") => {
                initialized = true;
                ok(
                    &id,
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "serverInfo": {"name": "legacy-only", "version": "1.0.0"}
                    }),
                )
            }
            // The whole point: 2026 methods are unknown to it.
            (Role::Legacy, "server/discover") => {
                err(&id, -32601, "server/discover: no such method")
            }
            // And real work is refused until the handshake happened. This is the
            // behaviour that makes a naive default-flip fail *late* instead of at
            // connect, which is the bug negotiation exists to prevent.
            (Role::Legacy, _) if !initialized => {
                err(&id, -32600, "not initialized: send initialize first")
            }
            (Role::Legacy, "tools/list") => ok(&id, json!({"tools": tools()})),
            (Role::Legacy, "tools/call") => ok(&id, call_result("legacy")),

            // ── The stateless server ─────────────────────────────────────────
            (Role::Stateless | Role::Quiet2026 | Role::Chatty | Role::Loud, "initialize") => {
                err(&id, -32601, "initialize was removed in 2026-07-28")
            }
            (Role::Stateless | Role::Chatty | Role::Loud, "server/discover") => ok(
                &id,
                json!({"serverInfo": {"name": "stateless-only", "version": "2.0.0"}}),
            ),
            // Spec-legal: `server/discover` is optional, so this role answers real
            // requests while implementing no discovery at all. If negotiation
            // treated a failed discover as proof of a legacy server, this role
            // would be misclassified — which is why the decisive probe is
            // tools/list.
            (Role::Quiet2026, "server/discover") => {
                err(&id, -32601, "server/discover: not implemented")
            }
            (Role::Stateless | Role::Quiet2026 | Role::Chatty | Role::Loud, "tools/list") => {
                // A 2026 client must send clientInfo in `_meta` on every request.
                // Refusing when it is absent turns "did the client really switch
                // lifecycles" into something the test can observe.
                if req["params"]["_meta"]["io.modelcontextprotocol/clientInfo"].is_null() {
                    err(&id, -32600, "missing _meta clientInfo (SEP-2575)")
                } else {
                    ok(&id, json!({"tools": tools(), "ttlMs": 60000}))
                }
            }
            (Role::Stateless | Role::Quiet2026 | Role::Chatty | Role::Loud, "tools/call") => {
                ok(&id, call_result("stateless"))
            }

            // ── The server that answers nothing ──────────────────────────────
            (Role::Hostile, _) => err(&id, -32601, "no"),

            (_, other) => err(&id, -32601, &format!("no such method: {other}")),
        };

        if role == Role::Loud {
            // More than any stdio pipe buffers, in one burst, before the reply.
            // A client that pipes stderr and does not drain it never sees the
            // frame that follows.
            let err = io::stderr();
            let mut err = err.lock();
            let row = format!(
                "{} loud server log line, padding to make it long enough\n",
                method
            );
            let mut written = 0usize;
            while written < 96 * 1024 {
                if err.write_all(row.as_bytes()).is_err() {
                    break;
                }
                written += row.len();
            }
            let _ = err.flush();
        }
        let mut line = String::new();
        if role == Role::Chatty {
            // Two lines of prose on the protocol channel, one of them blank-ish,
            // before the frame -- the shape of a kernel's Dispose chatter.
            line.push_str("Disposing Library\nDone Disposing Library\n");
        }
        line.push_str(&serde_json::to_string(&response).unwrap());
        line.push('\n');
        if out.write_all(line.as_bytes()).is_err() || out.flush().is_err() {
            break;
        }
    }
}

fn tools() -> Value {
    json!([{
        "name": "echo",
        "description": "returns what it was given",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
    }])
}

/// The tool result names the lifecycle it was served under, so a test can assert
/// which protocol actually carried the call rather than trusting `mode()`.
fn call_result(lifecycle: &str) -> Value {
    json!({"content": [{"type": "text", "text": format!("served over {lifecycle}")}]})
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

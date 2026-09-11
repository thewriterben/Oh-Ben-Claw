//! The MCP client must reach `tools/call` against a server speaking either
//! lifecycle, without being told which.
//!
//! `ProtocolMode::default()` became `Stateless2026` on 2026-07-30 — the Phase 15
//! item scheduled for July 28. That flip is one line and was never the work. The
//! work is that `establish()` used to pick a mode from config and commit to it,
//! and `discover()` deliberately *tolerates* a failed `server/discover` because
//! the 2026 spec makes discovery optional. Those two facts compose badly: flip
//! the default and every connection to a legacy server reports success at
//! connect and fails at the first `tools/call`.
//!
//! So the assertion here is deliberately not "did it pick the right mode". It is
//! **"did a tool actually run"**, end to end, over a real process and a real
//! stdio pipe. `mode()` agreeing with expectations is checked too, but second —
//! a client that reports `Stateless2026` and cannot call a tool has failed, and
//! a client that quietly speaks legacy while a tool runs has merely disappointed.
//!
//! The counterparty is `src/bin/mcp-conformance-server.rs`, which speaks exactly
//! one lifecycle per role and refuses the other. The repo's own MCP server is
//! bilingual and would pass every test here without proving anything.

use oh_ben_claw::mcp::client::McpClient;
use oh_ben_claw::mcp::{McpServerConfig, ModeSource, ProtocolMode};
use serde_json::json;

fn server(role: &str, pin: Option<ProtocolMode>) -> McpServerConfig {
    McpServerConfig {
        transport: "stdio".to_string(),
        command: Some(env!("CARGO_BIN_EXE_mcp-conformance-server").to_string()),
        args: Some(vec![role.to_string()]),
        url: None,
        token: None,
        env: None,
        protocol_mode: pin,
    }
}

/// A 2026 server that implements `server/discover`: settled in one round trip.
#[tokio::test]
async fn negotiates_the_2026_lifecycle_when_the_server_speaks_it() {
    let mut client = McpClient::connect(&server("stateless", None))
        .await
        .expect("connect to a stateless server");

    assert_eq!(client.mode(), ProtocolMode::Stateless2026);
    assert_eq!(client.mode_source(), ModeSource::Preferred);
    assert_eq!(client.server_name(), "stateless-only");

    let out = client
        .call_tool("echo", json!({"text": "hi"}))
        .await
        .expect("tool call over the 2026 lifecycle");
    assert_eq!(out, "served over stateless");
}

/// The case the whole change exists for.
///
/// The legacy server refuses `server/discover` *and* refuses `tools/list` until
/// `initialize` has happened. Without negotiation this connects cleanly and then
/// fails on the first real request.
#[tokio::test]
async fn falls_back_to_the_legacy_handshake_and_still_runs_a_tool() {
    let mut client = McpClient::connect(&server("legacy", None))
        .await
        .expect("connect to a legacy-only server");

    assert_eq!(
        client.mode(),
        ProtocolMode::Legacy2024,
        "negotiation did not fall back"
    );
    assert_eq!(client.mode_source(), ModeSource::Fallback);
    assert_eq!(
        client.server_name(),
        "legacy-only",
        "the fallback handshake did not record serverInfo, so the fallback \
         completed only partially"
    );

    // The assertion that matters. Everything above could be right while this
    // fails, and this failing is the production symptom.
    let out = client
        .call_tool("echo", json!({"text": "hi"}))
        .await
        .expect("tool call after falling back to the legacy handshake");
    assert_eq!(out, "served over legacy");
}

/// `server/discover` is optional in the 2026 spec, so a server may speak the new
/// lifecycle and implement no discovery at all.
///
/// This is why the decisive probe is `tools/list` rather than `server/discover`.
/// A negotiation that treated a failed discover as proof of a legacy server
/// would downgrade this connection — silently, and forever, since it would still
/// work.
#[tokio::test]
async fn a_2026_server_without_discovery_is_not_mistaken_for_a_legacy_one() {
    let mut client = McpClient::connect(&server("quiet-2026", None))
        .await
        .expect("connect to a 2026 server that has no server/discover");

    assert_eq!(
        client.mode(),
        ProtocolMode::Stateless2026,
        "a spec-legal 2026 server with no discovery was downgraded to legacy"
    );
    assert_eq!(client.mode_source(), ModeSource::Preferred);

    let out = client
        .call_tool("echo", json!({}))
        .await
        .expect("tool call");
    assert_eq!(out, "served over stateless");
}

/// The `tools/list` sent during negotiation is a real answer; throwing it away
/// would mean asking twice on every connect.
#[tokio::test]
async fn the_negotiation_probe_seeds_the_first_tool_listing() {
    let mut client = McpClient::connect(&server("quiet-2026", None))
        .await
        .expect("connect");

    let first = client.list_tools().await.expect("first listing");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].name, "echo");
    assert_eq!(
        client.tools_ttl_ms(),
        Some(60000),
        "ttlMs from the probe response was dropped"
    );

    // The cache is one-shot on purpose: a tool catalog that never expires is how
    // a client keeps calling a tool the server removed.
    let second = client
        .list_tools()
        .await
        .expect("second listing hits the wire");
    assert_eq!(second.len(), 1);
}

/// Pinning is a statement of intent. A pinned client must not quietly do
/// something else — including something that would have worked.
#[tokio::test]
async fn a_pinned_mode_is_not_renegotiated() {
    // Pinned to legacy against a server that would happily speak 2026.
    let pinned = McpClient::connect(&server("stateless", Some(ProtocolMode::Legacy2024))).await;

    match pinned {
        Ok(c) => {
            assert_eq!(c.mode(), ProtocolMode::Legacy2024);
            assert_eq!(
                c.mode_source(),
                ModeSource::Pinned,
                "a pinned connection reported a negotiated source"
            );
        }
        Err(_) => {
            // Also acceptable: the stateless server refuses `initialize`, so the
            // pinned handshake fails. What must NOT happen is a silent upgrade to
            // 2026 — that is the case the Ok branch above rules out.
        }
    }
}

/// A server that answers neither lifecycle must fail loudly, and the error must
/// name both attempts.
///
/// Without this, "the server is broken" and "negotiation is broken" produce the
/// same message, and the first person to hit it debugs the wrong one.
#[tokio::test]
async fn a_server_that_answers_nothing_reports_both_failures() {
    // `McpClient` is not `Debug` (it owns a child process and a transport), so
    // match rather than `expect_err`.
    let err = match McpClient::connect(&server("hostile", None)).await {
        Ok(_) => panic!("a server answering nothing must not yield a working client"),
        Err(e) => e,
    };

    let msg = format!("{err:#}");
    assert!(
        msg.contains("2026-07-28") && msg.contains("2024-11-05"),
        "the failure should name both lifecycles it tried, got: {msg}"
    );
}

/// A server that writes prose to stdout between frames is breaking the spec,
/// and until 2026-09-06 the client answered by breaking the connection: the
/// first non-JSON line was a hard error, and because the real reply was still
/// in the pipe, every later call read the stale one. OpenDesignCore's geometry
/// kernel does exactly this on Dispose. The line is skipped and warned about;
/// the reply behind it is delivered.
#[tokio::test]
async fn prose_on_the_servers_stdout_is_skipped_not_fatal() {
    let mut client = McpClient::connect(&server("chatty", None))
        .await
        .expect("connect to a server that chatters before every reply");

    let out = client
        .call_tool("echo", json!({"text": "hi"}))
        .await
        .expect("the reply behind the prose is still delivered");
    assert_eq!(out, "served over stateless");

    // And the one after it: the stream is not desynchronised.
    let again = client
        .call_tool("echo", json!({"text": "again"}))
        .await
        .expect("second call still answered");
    assert_eq!(again, "served over stateless");
}

/// A server that logs more to stderr than the pipe holds, before every reply.
///
/// The client captures stderr (2026-09-07) so a server's own reasons reach our
/// log. Capturing without draining is a deadlock with a delay: the child blocks
/// on `write(2)` once the buffer fills and the frame after it never arrives.
/// `loud` writes 96 KiB per request — past Linux's 64 KiB default and far past
/// Windows' — so this test hangs, and the harness's timeout fails it, if the
/// drain task is ever removed. Four requests to be sure it is not a one-buffer
/// fluke.
#[tokio::test]
async fn a_server_flooding_stderr_is_drained_not_deadlocked() {
    let cfg = server("loud", None);
    let connect = McpClient::connect(&cfg);
    let mut client = tokio::time::timeout(std::time::Duration::from_secs(30), connect)
        .await
        .expect("connect finished: stderr was drained during negotiation")
        .expect("connect to a server that floods stderr");

    for i in 0..4 {
        let call = client.call_tool("echo", json!({"text": i.to_string()}));
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), call)
            .await
            .expect("call finished: the pipe was drained while the server wrote")
            .expect("reply delivered behind 96 KiB of stderr");
        assert_eq!(out, "served over stateless");
    }
}

/// A server that dies after its first call.
///
/// Reproduces 2026-09-11: OpenDesignCore's server was killed under a live agent
/// and every later `odc_*` call failed with "The pipe is being closed" until the
/// agent restarted. Through `McpRemoteTool` — where the respawn lives — the
/// sequence must be: first call served; second call finds the server gone, the
/// tool respawns it and returns a *readable refusal* that says so (never a
/// silent retry: `tools/call` is not idempotent and the tool is physical); third
/// call is served by the new process.
#[tokio::test]
async fn a_dead_server_is_respawned_and_the_model_is_told() {
    use obc_tool_api::Tool;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let cfg = server("mortal", None);
    let client = McpClient::connect(&cfg).await.expect("connect to mortal");
    let tool = oh_ben_claw::mcp::McpRemoteTool {
        name: "mortal_echo".to_string(),
        remote_name: "echo".to_string(),
        description: String::new(),
        schema: json!({"type": "object"}),
        server: "mortal".to_string(),
        client: Arc::new(Mutex::new(client)),
        reach: None,
        auditor: None,
    };

    let first = tool.execute(json!({"text": "1"})).await.unwrap();
    assert!(first.success, "first call served: {:?}", first.error);
    assert_eq!(first.output, "served over stateless");

    // The server exited right after replying. Give the OS a moment to close
    // the pipes; the client must cope either way (write error or EOF).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let second = tool.execute(json!({"text": "2"})).await.unwrap();
    assert!(!second.success, "second call must not pretend to succeed");
    let msg = second.error.clone().unwrap_or_default();
    assert!(msg.contains("had exited"), "names the death: {msg}");
    assert!(msg.contains("restarted"), "names the respawn: {msg}");
    assert!(
        msg.contains("call mortal_echo again"),
        "tells the model what to do: {msg}"
    );
    assert!(
        !msg.contains("pipe is being closed") && !msg.contains("os error"),
        "not the raw OS error: {msg}"
    );

    let third = tool.execute(json!({"text": "3"})).await.unwrap();
    assert!(
        third.success,
        "served by the respawned process: {:?}",
        third.error
    );
    assert_eq!(third.output, "served over stateless");
}

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

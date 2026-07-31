//! Protocol negotiation against a real third-party MCP server.
//!
//! `tests/mcp_protocol_negotiation.rs` proves the negotiation logic against
//! `src/bin/mcp-conformance-server.rs`, a fixture written alongside it. That is
//! the right way to test the branches — a fixture can be made to refuse things a
//! real server never would — but it shares an author with the code under test,
//! and the commit that added it said so: "nothing here has been run against a
//! third-party MCP server; the conformance server is a fixture, not the field."
//!
//! This is the field. It drives `@modelcontextprotocol/server-everything`, the
//! reference implementation published by the MCP project, over a real stdio pipe.
//!
//! `#[ignore]` so a plain `cargo test` stays offline and fast. CI runs it
//! explicitly, so it is not a check that waits to be remembered — that pattern
//! is what this whole audit kept finding. Locally:
//!
//! ```bash
//! cargo test --test mcp_real_server -- --ignored --nocapture
//! ```
//!
//! The server version is **pinned**. An unpinned `npx -y <pkg>` makes this job
//! depend on whatever npm serves that morning, so a red build could mean our
//! regression or their release, and the two would be indistinguishable. Pinning
//! costs the ability to notice an upstream change automatically; bumping the
//! version below is the deliberate act that buys it back, and any behaviour
//! change shows up in that commit rather than in an unrelated one.

use oh_ben_claw::mcp::client::McpClient;
use oh_ben_claw::mcp::{McpServerConfig, ModeSource};
use serde_json::json;

/// npx is a shell script on Windows; spawning it by bare name fails there.
fn npx() -> &'static str {
    if cfg!(windows) {
        "npx.cmd"
    } else {
        "npx"
    }
}

/// Pinned deliberately — see the module docs.
const REFERENCE_SERVER: &str = "@modelcontextprotocol/server-everything@2026.7.4";

fn reference_server() -> McpServerConfig {
    McpServerConfig {
        transport: "stdio".to_string(),
        command: Some(npx().to_string()),
        args: Some(vec!["-y".into(), REFERENCE_SERVER.into(), "stdio".into()]),
        url: None,
        token: None,
        env: None,
        // Omitted on purpose: this is the negotiation being exercised.
        protocol_mode: None,
    }
}

#[tokio::test]
#[ignore = "needs network and npx; run with --ignored"]
async fn negotiates_with_the_reference_implementation_and_calls_a_tool() {
    let mut client = McpClient::connect(&reference_server())
        .await
        .expect("connect to @modelcontextprotocol/server-everything");

    println!(
        "negotiated {:?} via {:?} with {} v{}",
        client.mode(),
        client.mode_source(),
        client.server_name(),
        client.server_version()
    );

    // Whichever lifecycle it settled on, the connection must be usable. That is
    // the assertion that matters — a client reporting a mode it cannot act on is
    // the exact failure the negotiation exists to prevent.
    assert_ne!(
        client.mode_source(),
        ModeSource::Pinned,
        "nothing was pinned, so the source must be Preferred or Fallback"
    );

    let tools = client.list_tools().await.expect("tools/list");
    assert!(
        !tools.is_empty(),
        "the reference server advertises no tools; the connection is not usable"
    );
    println!(
        "  {} tools: {:?}",
        tools.len(),
        tools
            .iter()
            .take(5)
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
    );

    // `echo` is part of the reference server's documented surface.
    let out = client
        .call_tool("echo", json!({ "message": "from oh-ben-claw" }))
        .await
        .expect("tools/call echo");
    assert!(
        out.contains("from oh-ben-claw"),
        "echo did not round-trip the message, got: {out}"
    );
    println!("  echo returned: {out}");
}

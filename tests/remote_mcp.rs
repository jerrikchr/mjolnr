//! Phase 26 remote-MCP acceptance tests.
//!
//! The merged stub reported remote servers `Connected` with zero tools without
//! ever opening a connection. These assert the honest replacement: a remote
//! server that does not answer is `Unavailable`, never a fabricated `Connected`,
//! and registers no tools.

#![allow(clippy::indexing_slicing, clippy::expect_used)]

use smed::core::mcp::McpConnectionState;
use smed::mcp::connect_project;

/// Write an `.smed/mcp.yaml` into a fresh workspace and connect it.
async fn connect_with(yaml: &str) -> smed::mcp::McpCatalog {
    let temp = tempfile::tempdir().expect("tempdir");
    let smed_dir = temp.path().join(".smed");
    std::fs::create_dir_all(&smed_dir).expect("create_dir_all");
    std::fs::write(smed_dir.join("mcp.yaml"), yaml).expect("write yaml");
    connect_project(temp.path()).await.expect("connect project")
}

#[tokio::test]
async fn a_dead_remote_server_is_unavailable_not_a_fabricated_connected() {
    // Port 1 on loopback refuses immediately: a real connection attempt that
    // cannot succeed, so the result is the honest `Unavailable`, fast.
    let catalog =
        connect_with("servers:\n  - name: remote-dead\n    url: \"http://127.0.0.1:1/mcp\"\n")
            .await;
    assert_eq!(catalog.servers.len(), 1);
    assert_eq!(catalog.servers[0].name, "remote-dead");
    assert_eq!(
        catalog.servers[0].state,
        McpConnectionState::Unavailable,
        "a server that never answered must not be reported Connected"
    );
    assert_eq!(catalog.servers[0].tool_count, 0);
    assert!(catalog.registry.get("mcp:remote-dead:anything").is_none());
}

#[tokio::test]
async fn a_remote_server_with_bearer_and_headers_still_reports_honestly_when_dead() {
    // The bearer/header rung must not turn a dead server into a live one: the
    // credential is resolved, the connection is attempted, and the honest
    // `Unavailable` still results.
    let catalog = connect_with(
        "servers:\n  - name: remote-auth\n    url: \"http://127.0.0.1:1/mcp\"\n    \
         bearer_token_env_var: \"MJOLNR_TEST_MCP_TOKEN\"\n    headers:\n      X-Client: \"smed\"\n",
    )
    .await;
    assert_eq!(catalog.servers[0].state, McpConnectionState::Unavailable);
}

#[tokio::test]
async fn a_remote_url_with_a_bad_scheme_is_rejected_before_any_connection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let smed_dir = temp.path().join(".smed");
    std::fs::create_dir_all(&smed_dir).expect("create_dir_all");
    std::fs::write(
        smed_dir.join("mcp.yaml"),
        "servers:\n  - name: bad\n    url: \"ftp://example.com/mcp\"\n",
    )
    .expect("write yaml");
    // Malformed configuration fails closed: the whole catalog load errors rather
    // than silently connecting to a scheme the transport cannot speak.
    assert!(connect_project(temp.path()).await.is_err());
}

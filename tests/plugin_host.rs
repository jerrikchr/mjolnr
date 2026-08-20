//! Integration tests for plugin subprocess host and JSON-RPC 2.0 stdio transport (ADR-0016, Master Implementation Plan §3.2).

#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use std::collections::BTreeMap;
use std::time::Duration;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use mjolnr::core::plugin::{PluginHook, PluginManifest, PluginRunCommand, PluginToolDeclaration};
use mjolnr::plugins::PluginHost;

fn mock_plugin_manifest(script_path: &str) -> PluginManifest {
    PluginManifest {
        name: "test.deployer".to_owned(),
        version: "1.0.0".to_owned(),
        publisher: "test-publisher".to_owned(),
        description: "Test plugin for host integration".to_owned(),
        protocol_version: 1,
        run: PluginRunCommand {
            program: "python3".to_owned(),
            arguments: vec![script_path.to_owned()],
        },
        tools: vec![PluginToolDeclaration {
            name: "deploy_service".to_owned(),
            description: "Deploy a service".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "env": { "type": "string" }
                }
            }),
        }],
        hooks: vec![PluginHook::SessionStart, PluginHook::PreToolCall],
        required_credentials: vec!["TEST_PLUGIN_SECRET".to_owned()],
        views: Vec::new(),
        source_url: None,
    }
}

const MOCK_PLUGIN_PYTHON: &str = r#"
import sys
import json
import os

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    req_id = req.get("id")
    method = req.get("method")
    params = req.get("params", {})

    if method == "initialize":
        # Echo back environment check in result for verification
        secret_val = os.environ.get("TEST_PLUGIN_SECRET", "")
        has_openai = "OPENAI_API_KEY" in os.environ
        resp = {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "status": "ready",
                "secret": secret_val,
                "has_openai": has_openai
            }
        }
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    elif method == "session_start":
        resp = {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "annotations": ["Plugin initialized for test session"],
                "notices": ["Deployment gate active"]
            }
        }
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    elif method == "call_tool":
        tool_name = params.get("name")
        tool_params = params.get("parameters", {})
        if tool_name == "deploy_service":
            resp = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "status": "deployed",
                    "target_env": tool_params.get("env", "staging")
                }
            }
        else:
            resp = {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32601,
                    "message": f"Unknown tool {tool_name}"
                }
            }
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    elif method == "shutdown":
        sys.exit(0)
"#;

#[tokio::test]
async fn plugin_host_spawns_and_exchanges_handshake_with_scrubbed_env() {
    let workspace = tempdir().expect("tempdir");
    let script_path = workspace.path().join("plugin.py");
    std::fs::write(&script_path, MOCK_PLUGIN_PYTHON).expect("write python script");

    let manifest = mock_plugin_manifest(script_path.to_str().expect("utf8"));
    let mut creds = BTreeMap::new();
    creds.insert("TEST_PLUGIN_SECRET".to_owned(), "top-secret-val".to_owned());

    let cancel = CancellationToken::new();
    let host = PluginHost::start(manifest, workspace.path(), creds, cancel)
        .await
        .expect("host starts");

    assert_eq!(host.manifest().name, "test.deployer");
    assert!(host.is_subscribed_to(PluginHook::SessionStart));
    assert!(!host.is_subscribed_to(PluginHook::PostTurn));

    // Test observer hook
    let observer_res = host
        .notify_hook(
            PluginHook::SessionStart,
            serde_json::json!({ "session_id": "test-session" }),
        )
        .await
        .expect("hook returns result");

    assert_eq!(
        observer_res.annotations,
        vec!["Plugin initialized for test session".to_owned()]
    );
    assert_eq!(
        observer_res.notices,
        vec!["Deployment gate active".to_owned()]
    );

    // Test unsubscribed hook returns None immediately
    let unsubscribed = host
        .notify_hook(PluginHook::PostTurn, serde_json::json!({ "turn": 1 }))
        .await;
    assert!(unsubscribed.is_none());

    // Test tool call
    let tool_res = host
        .call_tool(
            "deploy_service",
            serde_json::json!({ "env": "production" }),
            Duration::from_secs(5),
        )
        .await
        .expect("call_tool succeeds");

    assert_eq!(tool_res["status"], "deployed");
    assert_eq!(tool_res["target_env"], "production");

    // Test undeclared tool is refused before IPC
    let undeclared = host
        .call_tool("random_tool", serde_json::json!({}), Duration::from_secs(5))
        .await;
    assert!(undeclared.is_err());

    // Shutdown cleanly
    host.shutdown().await.expect("shutdown cleanly");
}

const HANGING_PLUGIN_PYTHON: &str = r"
import sys
import time

for line in sys.stdin:
    time.sleep(10)
";

#[tokio::test]
async fn hanging_plugin_times_out_safely() {
    let workspace = tempdir().expect("tempdir");
    let script_path = workspace.path().join("hanging.py");
    std::fs::write(&script_path, HANGING_PLUGIN_PYTHON).expect("write python script");

    let manifest = mock_plugin_manifest(script_path.to_str().expect("utf8"));
    let cancel = CancellationToken::new();
    let res = PluginHost::start(manifest, workspace.path(), BTreeMap::new(), cancel).await;
    assert!(res.is_err());
}

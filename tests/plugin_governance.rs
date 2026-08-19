//! Integration tests for plugin tool registration and governed execution (ADR-0016 §3, Master Implementation Plan §3.3).

#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use smed::core::message::ToolOutcome;
use smed::core::plugin::{PluginHook, PluginManifest, PluginRunCommand, PluginToolDeclaration};
use smed::core::tool::{ReadSet, ToolContext, ToolTier};
use smed::plugins::PluginHost;
use smed::tools::{ToolRegistry, register_plugin_tools};

fn mock_plugin_manifest(script_path: &str) -> PluginManifest {
    PluginManifest {
        name: "acme.deploy".to_owned(),
        version: "1.0.0".to_owned(),
        publisher: "acme-corp".to_owned(),
        description: "Deployment governance plugin".to_owned(),
        protocol_version: 1,
        run: PluginRunCommand {
            program: "python3".to_owned(),
            arguments: vec![script_path.to_owned()],
        },
        tools: vec![
            PluginToolDeclaration {
                name: "trigger_pipeline".to_owned(),
                description: "Trigger deployment pipeline".to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "env": { "type": "string" },
                        "version": { "type": "string" }
                    },
                    "required": ["env", "version"]
                }),
            },
            PluginToolDeclaration {
                name: "fail_pipeline".to_owned(),
                description: "Failing deployment pipeline".to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ],
        hooks: vec![PluginHook::SessionStart],
        required_credentials: vec!["DEPLOY_TOKEN".to_owned()],
        views: Vec::new(),
        source_url: None,
    }
}

const MOCK_GOVERNED_PLUGIN_PYTHON: &str = r#"
import sys
import json

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    req_id = req.get("id")
    method = req.get("method")
    params = req.get("params", {})

    if method == "initialize":
        resp = {"jsonrpc": "2.0", "id": req_id, "result": {"status": "ready"}}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    elif method == "call_tool":
        tool_name = params.get("name")
        tool_params = params.get("parameters", {})
        if tool_name == "trigger_pipeline":
            env = tool_params.get("env", "unknown")
            ver = tool_params.get("version", "0.0.0")
            resp = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "deployed": True,
                    "target_env": env,
                    "target_version": ver
                }
            }
        elif tool_name == "fail_pipeline":
            resp = {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32603,
                    "message": "Deployment failed due to policy rejection"
                }
            }
        else:
            resp = {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {"code": -32601, "message": f"Unknown tool {tool_name}"}
            }
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    elif method == "shutdown":
        sys.exit(0)
"#;

fn test_context(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_root: workspace.to_path_buf(),
        read_set: Arc::new(ReadSet::default()),
        max_output_bytes: 64 * 1024,
        command_timeout: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn plugin_tool_is_namespaced_and_pinned_to_tier_execute() {
    let workspace = tempdir().expect("tempdir");
    let script_path = workspace.path().join("plugin.py");
    std::fs::write(&script_path, MOCK_GOVERNED_PLUGIN_PYTHON).expect("write python script");

    let manifest = mock_plugin_manifest(script_path.to_str().expect("utf8"));
    let mut creds = BTreeMap::new();
    creds.insert("DEPLOY_TOKEN".to_owned(), "secret-token".to_owned());

    let cancel = CancellationToken::new();
    let host = Arc::new(
        PluginHost::start(manifest, workspace.path(), creds, cancel)
            .await
            .expect("host starts"),
    );

    let mut registry = ToolRegistry::builtins();
    register_plugin_tools(&mut registry, &host);

    // Verify namespacing: plugin:<plugin_name>:<tool_name>
    let tool = registry
        .get("plugin:acme.deploy:trigger_pipeline")
        .expect("namespaced tool found in registry");

    assert_eq!(tool.name(), "plugin:acme.deploy:trigger_pipeline");
    assert_eq!(tool.description(), "Trigger deployment pipeline");

    // Tier MUST be Execute (ADR-0016 §3)
    assert_eq!(tool.tier(), ToolTier::Execute);

    // Verify preview contains plugin name, tool name, and arguments
    let preview = tool
        .preview(
            &serde_json::json!({ "env": "prod", "version": "1.2.0" }),
            &test_context(workspace.path()),
        )
        .await
        .expect("preview formats");
    assert!(preview.contains("Plugin: acme.deploy"));
    assert!(preview.contains("Tool: trigger_pipeline"));
    assert!(preview.contains("prod"));

    // Verify schema validation via registry
    assert!(
        registry
            .validate(
                tool.as_ref(),
                &serde_json::json!({ "env": "prod", "version": "1.2.0" }),
            )
            .is_ok()
    );
    assert!(
        registry
            .validate(tool.as_ref(), &serde_json::json!({ "env": "prod" }))
            .is_err()
    );

    // Execute tool
    let result = tool
        .execute(
            serde_json::json!({ "env": "prod", "version": "1.2.0" }),
            test_context(workspace.path()),
            CancellationToken::new(),
        )
        .await
        .expect("execute succeeds");

    assert_eq!(result.outcome, ToolOutcome::Ok);
    assert!(result.content.contains("\"deployed\": true"));
    assert!(result.content.contains("\"target_env\": \"prod\""));

    // Execute failing tool
    let fail_tool = registry
        .get("plugin:acme.deploy:fail_pipeline")
        .expect("fail tool found");
    let fail_result = fail_tool
        .execute(
            serde_json::json!({}),
            test_context(workspace.path()),
            CancellationToken::new(),
        )
        .await
        .expect("execute handles error as result");

    assert!(matches!(fail_result.outcome, ToolOutcome::Failed(_)));
    assert!(fail_result.content.contains("Deployment failed"));

    // Execute with cancelled token
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let cancelled_res = tool
        .execute(
            serde_json::json!({ "env": "prod", "version": "1.2.0" }),
            test_context(workspace.path()),
            cancel_token,
        )
        .await
        .expect("execute handles cancellation as refused outcome");
    assert!(matches!(cancelled_res.outcome, ToolOutcome::Refused(_)));

    host.shutdown().await.expect("shutdown cleanly");
}

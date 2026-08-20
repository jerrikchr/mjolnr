//! Integration tests for plugin manifest validation and discovery (ADR-0016, Master Implementation Plan §3.1).

#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use tempfile::tempdir;

use mjolnr::context::DiscoveryLimits;
use mjolnr::context::plugins::PluginCatalog;
use mjolnr::core::context::SkillScope;
use mjolnr::core::error::ReasonCode;
use mjolnr::core::plugin::{
    MAX_PLUGIN_CREDENTIALS, MAX_PLUGIN_TOOLS, PLUGIN_PROTOCOL_VERSION, PluginHook, PluginManifest,
};

const VALID_PLUGIN: &str = "name: acme.deploy
version: 1.0.0
publisher: acme-corp
description: Continuous deployment plugin for mjolnr.
protocol_version: 1
run:
  program: node
  arguments: [\"dist/index.js\"]
tools:
  - name: trigger_deploy
    description: Trigger a deployment pipeline.
    parameters:
      type: object
      properties:
        environment:
          type: string
          description: Target environment name.
      required: [\"environment\"]
hooks:
  - session_start
  - post_tool_call
required_credentials:
  - VERCEL_TOKEN
  - DEPLOY_KEY
views:
  - id: deployments
    title: Deployments
    view_type: table
source_url: https://github.com/acme/mjolnr-deploy
";

#[test]
fn a_well_formed_plugin_manifest_parses() {
    let manifest = PluginManifest::parse(VALID_PLUGIN).expect("valid manifest");
    assert_eq!(manifest.name, "acme.deploy");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.publisher, "acme-corp");
    assert_eq!(manifest.protocol_version, PLUGIN_PROTOCOL_VERSION);
    assert_eq!(manifest.run.program, "node");
    assert_eq!(manifest.run.arguments, vec!["dist/index.js".to_owned()]);
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.tools[0].name, "trigger_deploy");
    assert_eq!(manifest.hooks.len(), 2);
    assert!(manifest.hooks.contains(&PluginHook::SessionStart));
    assert!(manifest.hooks.contains(&PluginHook::PostToolCall));
    assert_eq!(
        manifest.required_credentials,
        vec!["VERCEL_TOKEN".to_owned(), "DEPLOY_KEY".to_owned()]
    );
    assert_eq!(manifest.views.len(), 1);
    assert_eq!(manifest.views[0].id, "deployments");

    let summary = manifest.summary();
    assert_eq!(summary.name, "acme.deploy");
    assert_eq!(summary.tool_count, 1);
    assert_eq!(summary.hook_count, 2);
    assert_eq!(summary.required_credentials.len(), 2);
}

#[test]
fn unknown_field_is_refused_by_fail_closed_manifest() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
tier: read
";
    let error = PluginManifest::parse(yaml).expect_err("unknown field must fail");
    assert!(error.contains("unknown field") || error.contains("tier"));
}

#[test]
fn pass_env_field_is_refused() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
pass_env: true
";
    let error = PluginManifest::parse(yaml).expect_err("pass_env must fail");
    assert!(error.contains("pass_env") || error.contains("unknown field"));
}

#[test]
fn unsupported_protocol_version_is_refused() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 99
run:
  program: test
";
    let error = PluginManifest::parse(yaml).expect_err("protocol version mismatch must fail");
    assert!(error.contains("unsupported plugin protocol version 99"));
}

#[test]
fn path_traversal_in_executable_is_refused() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: ../../evil.sh
";
    let error = PluginManifest::parse(yaml).expect_err("path traversal must fail");
    assert!(error.contains("path traversal"));
}

#[test]
fn tool_schema_with_forbidden_ref_is_refused() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
tools:
  - name: bad_schema
    description: Uses $ref
    parameters:
      \"$ref\": \"https://example.com/schema.json\"
";
    let error = PluginManifest::parse(yaml).expect_err("$ref in tool schema must fail");
    assert!(error.contains("forbidden `$ref`"));
}

#[test]
fn duplicate_tool_names_are_refused() {
    let yaml = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
tools:
  - name: test_tool
    description: Tool 1
  - name: test_tool
    description: Tool 2
";
    let error = PluginManifest::parse(yaml).expect_err("duplicate tool names must fail");
    assert!(error.contains("duplicate tool name `test_tool`"));
}

#[test]
fn lowercase_and_identifier_rules_are_enforced() {
    let bad_name = "name: AcmeDeploy!
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
";
    let error = PluginManifest::parse(bad_name).expect_err("uppercase plugin name must fail");
    assert!(error.contains("plugin name"));

    let bad_tool = "name: acme.deploy
version: 1.0.0
publisher: acme
description: test
protocol_version: 1
run:
  program: test
tools:
  - name: BadToolName
    description: Uppercase tool name
";
    let error = PluginManifest::parse(bad_tool).expect_err("uppercase tool name must fail");
    assert!(error.contains("tool name"));
}

#[test]
fn bounds_on_tools_and_credentials_are_enforced() {
    let mut tools = Vec::new();
    for i in 0..=MAX_PLUGIN_TOOLS {
        tools.push(format!(
            "  - name: tool_{i}\n    description: desc for tool {i}"
        ));
    }
    let too_many_tools = format!(
        "name: acme.deploy\nversion: 1.0.0\npublisher: acme\ndescription: test\nprotocol_version: 1\nrun:\n  program: test\ntools:\n{}",
        tools.join("\n")
    );
    let error = PluginManifest::parse(&too_many_tools).expect_err("too many tools must fail");
    assert!(error.contains("at most"));

    let mut creds = Vec::new();
    for i in 0..=MAX_PLUGIN_CREDENTIALS {
        creds.push(format!("  - TOKEN_{i}"));
    }
    let too_many_creds = format!(
        "name: acme.deploy\nversion: 1.0.0\npublisher: acme\ndescription: test\nprotocol_version: 1\nrun:\n  program: test\nrequired_credentials:\n{}",
        creds.join("\n")
    );
    let error = PluginManifest::parse(&too_many_creds).expect_err("too many credentials must fail");
    assert!(error.contains("at most"));
}

#[test]
fn discovery_scans_and_reports_diagnostics_on_malformed_plugin() {
    let workspace = tempdir().expect("tempdir");
    let plugins_dir = workspace.path().join(".mjolnr").join("plugins");
    std::fs::create_dir_all(&plugins_dir).expect("create plugins dir");

    // Write one valid plugin
    std::fs::write(plugins_dir.join("deploy.yaml"), VALID_PLUGIN).expect("write valid plugin");

    // Write one malformed plugin
    let malformed = "name: bad.plugin\nversion: 1.0.0\npublisher: bad\ndescription: bad\nprotocol_version: 99\nrun:\n  program: test\n";
    std::fs::write(plugins_dir.join("bad.yaml"), malformed).expect("write malformed plugin");

    let roots = vec![(plugins_dir, SkillScope::Project, None)];
    let mut diagnostics = Vec::new();
    let catalog = PluginCatalog::discover(roots, DiscoveryLimits::default(), &mut diagnostics);

    assert_eq!(catalog.len(), 1);
    let deploy = catalog.get("acme.deploy").expect("found valid plugin");
    assert_eq!(deploy.summary.publisher, "acme-corp");

    // Malformed plugin emitted a diagnostic
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, ReasonCode::SchemaInvalid);
    assert!(
        diagnostics[0]
            .detail
            .contains("unsupported plugin protocol")
    );
}

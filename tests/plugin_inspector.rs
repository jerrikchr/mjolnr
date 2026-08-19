//! Integration tests for plugin inspector UI and client DTO bridge (ADR-0016, Master Implementation Plan §3.4).

#![allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use smed::core::client::types::ClientSnapshot;
use smed::core::plugin::PluginSummary;
use smed::core::runtime::RuntimeSnapshot;
use smed::runtime::client_bridge::convert::snapshot_to_client;
use smed::tui::commands::COMMANDS;
use smed::tui::reducer::{Overlay, ViewState};

#[test]
fn plugins_slash_command_exists_in_registry() {
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "/plugins")
        .expect("/plugins command must be registered");
    assert_eq!(cmd.name, "/plugins");
    assert!(cmd.summary.contains("plugins"));

    let snapshot = RuntimeSnapshot {
        plugins: Arc::new(vec![PluginSummary {
            name: "acme.deploy".to_owned(),
            version: "1.0.0".to_owned(),
            publisher: "acme-corp".to_owned(),
            description: "Deployment plugin".to_owned(),
            tool_count: 2,
            hook_count: 1,
            required_credentials: vec!["DEPLOY_TOKEN".to_owned()],
            source_url: None,
        }]),
        ..Default::default()
    };

    let mut view = ViewState::default();
    view.sync(snapshot);

    let state = (cmd.state)(&view).expect("state formatted");
    assert!(state.contains("1 discovered"));
}

#[test]
fn view_state_toggles_plugins_overlay() {
    let mut view = ViewState::default();
    assert_eq!(view.overlay, Overlay::None);

    view.toggle_plugins();
    assert_eq!(view.overlay, Overlay::Plugins);

    view.toggle_plugins();
    assert_eq!(view.overlay, Overlay::None);
}

#[test]
fn client_snapshot_bridges_plugin_summary() {
    let snapshot = RuntimeSnapshot {
        plugins: Arc::new(vec![PluginSummary {
            name: "acme.deploy".to_owned(),
            version: "1.0.0".to_owned(),
            publisher: "acme-corp".to_owned(),
            description: "Deployment plugin".to_owned(),
            tool_count: 2,
            hook_count: 1,
            required_credentials: vec!["DEPLOY_TOKEN".to_owned()],
            source_url: None,
        }]),
        ..Default::default()
    };

    let client_snapshot = snapshot_to_client(1, &snapshot);
    assert_eq!(client_snapshot.plugins.len(), 1);
    assert_eq!(client_snapshot.plugins[0].name, "acme.deploy");
    assert_eq!(client_snapshot.plugins[0].tool_count, 2);
    assert_eq!(client_snapshot.plugins[0].hook_count, 1);
    assert_eq!(
        client_snapshot.plugins[0].required_credentials,
        vec!["DEPLOY_TOKEN".to_owned()]
    );

    // Serialization check
    let json = serde_json::to_string(&client_snapshot).expect("serialize client snapshot");
    let deserialized: ClientSnapshot =
        serde_json::from_str(&json).expect("deserialize client snapshot");
    assert_eq!(client_snapshot, deserialized);
}

#[test]
fn plugins_overlay_renders_cleanly_on_empty_and_populated() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");

    let mut view = ViewState::default();
    view.toggle_plugins();
    assert_eq!(view.overlay, Overlay::Plugins);

    terminal
        .draw(|f| {
            smed::tui::layout::render(f, &view);
        })
        .expect("draw empty plugins overlay");

    let populated_snapshot = RuntimeSnapshot {
        plugins: Arc::new(vec![
            PluginSummary {
                name: "acme.deploy".to_owned(),
                version: "1.0.0".to_owned(),
                publisher: "acme-corp".to_owned(),
                description: "Deployment governance plugin".to_owned(),
                tool_count: 3,
                hook_count: 2,
                required_credentials: vec!["DEPLOY_TOKEN".to_owned()],
                source_url: None,
            },
            PluginSummary {
                name: "acme.monitor".to_owned(),
                version: "0.2.1".to_owned(),
                publisher: "acme-corp".to_owned(),
                description: "Telemetry and health check observer".to_owned(),
                tool_count: 0,
                hook_count: 4,
                required_credentials: Vec::new(),
                source_url: None,
            },
        ]),
        ..Default::default()
    };

    view.sync(populated_snapshot);

    terminal
        .draw(|f| {
            smed::tui::layout::render(f, &view);
        })
        .expect("draw populated plugins overlay");
}

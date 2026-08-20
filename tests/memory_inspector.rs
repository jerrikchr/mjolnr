//! Integration tests for Glassbox Memory Inspector UI and DTO translation (master implementation plan §2.3).

use std::sync::Arc;

use mjolnr::core::client::types::ClientMemorySummary;
use mjolnr::core::memory::MemorySummary;
use mjolnr::core::runtime::RuntimeSnapshot;
use mjolnr::runtime::client_bridge::convert::snapshot_to_client;
use mjolnr::tui::reducer::{Overlay, ViewState};

#[test]
fn memory_slash_command_exists_in_registry() {
    let snapshot = RuntimeSnapshot {
        memory: Arc::new(MemorySummary {
            rules_count: 3,
            user_profile_present: true,
            facts_count: Some(12),
            episodes_count: Some(4),
            projection_error: None,
            rules_error: None,
            rule_names: vec!["style".to_owned(), "security".to_owned()],
        }),
        ..Default::default()
    };

    let mut view = ViewState::default();
    view.sync(snapshot);
    let memory_cmd = mjolnr::tui::commands::COMMANDS
        .iter()
        .find(|cmd| cmd.name == "/memory")
        .expect("/memory command must be registered");

    let status = (memory_cmd.state)(&view).expect("status string");
    assert!(status.contains("3 rules"));
    assert!(status.contains("12 facts"));
}

#[test]
fn view_state_toggles_memory_overlay() {
    let mut view = ViewState::default();
    assert_eq!(view.overlay, Overlay::None);

    view.toggle_memory();
    assert_eq!(view.overlay, Overlay::Memory);

    view.toggle_memory();
    assert_eq!(view.overlay, Overlay::None);
}

#[test]
fn client_snapshot_bridges_memory_summary() {
    let snapshot = RuntimeSnapshot {
        memory: Arc::new(MemorySummary {
            rules_count: 2,
            projection_error: None,
            rules_error: None,
            user_profile_present: true,
            facts_count: Some(5),
            episodes_count: Some(1),
            rule_names: vec!["architecture".to_owned()],
        }),
        ..Default::default()
    };

    let client = snapshot_to_client(1, &snapshot);
    let memory = client
        .memory
        .clone()
        .expect("client memory summary present");

    assert_eq!(
        memory,
        ClientMemorySummary {
            rules_count: 2,
            projection_error: None,
            rules_error: None,
            user_profile_present: true,
            facts_count: Some(5),
            episodes_count: Some(1),
            rule_names: vec!["architecture".to_owned()],
        }
    );

    // Verify JSON serialization roundtrip
    let json = serde_json::to_string(&client).expect("serialize client snapshot");
    assert!(json.contains("\"memory\":"));
    assert!(json.contains("\"rulesCount\":2"));
    assert!(json.contains("\"userProfilePresent\":true"));
}

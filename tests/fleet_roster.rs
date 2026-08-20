//! Integration tests for fleet roster DTOs and client bridge (Master Implementation Plan Phase 3 Slice 3.1).

#![allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use std::sync::Arc;

use mjolnr::core::client::types::ClientSnapshot;
use mjolnr::core::event::SessionId;
use mjolnr::core::fleet::{FleetAgentStatus, FleetAgentSummary, FleetSummary};
use mjolnr::core::runtime::RuntimeSnapshot;
use mjolnr::runtime::client_bridge::convert::snapshot_to_client;

#[test]
fn fleet_summary_calculates_visibility_and_active_count() {
    let a1 = SessionId::new();
    let a2 = SessionId::new();

    // Single agent: visible = false (needs >= 2 agents)
    let single = FleetSummary::from_agents(vec![FleetAgentSummary {
        child_session_id: a1,
        short_name: "sub-1".to_owned(),
        role: Some("indexer".to_owned()),
        status: FleetAgentStatus::Running,
        latest_activity: "indexing AST".to_owned(),
        feed: vec!["started".to_owned()],
        worktree_branch: Some("mjolnr/worktree-sub-1".to_owned()),
    }]);
    assert!(!single.visible);
    assert_eq!(single.active_count, 1);

    // Two agents, one active: visible = true
    let two_agents = FleetSummary::from_agents(vec![
        FleetAgentSummary {
            child_session_id: a1,
            short_name: "sub-1".to_owned(),
            role: Some("indexer".to_owned()),
            status: FleetAgentStatus::Running,
            latest_activity: "indexing AST".to_owned(),
            feed: vec!["started".to_owned()],
            worktree_branch: Some("mjolnr/worktree-sub-1".to_owned()),
        },
        FleetAgentSummary {
            child_session_id: a2,
            short_name: "sub-2".to_owned(),
            role: Some("tester".to_owned()),
            status: FleetAgentStatus::Completed,
            latest_activity: "all tests passed".to_owned(),
            feed: vec!["started".to_owned(), "all tests passed".to_owned()],
            worktree_branch: None,
        },
    ]);
    assert!(two_agents.visible);
    assert_eq!(two_agents.active_count, 1);

    // Two agents, both completed: visible = false
    let settled = FleetSummary::from_agents(vec![
        FleetAgentSummary {
            child_session_id: a1,
            short_name: "sub-1".to_owned(),
            role: Some("indexer".to_owned()),
            status: FleetAgentStatus::Completed,
            latest_activity: "done".to_owned(),
            feed: vec!["done".to_owned()],
            worktree_branch: None,
        },
        FleetAgentSummary {
            child_session_id: a2,
            short_name: "sub-2".to_owned(),
            role: Some("tester".to_owned()),
            status: FleetAgentStatus::Completed,
            latest_activity: "all tests passed".to_owned(),
            feed: vec!["done".to_owned()],
            worktree_branch: None,
        },
    ]);
    assert!(!settled.visible);
    assert_eq!(settled.active_count, 0);
}

#[test]
fn client_snapshot_bridges_fleet_summary() {
    let a1 = SessionId::new();
    let a2 = SessionId::new();

    let fleet = FleetSummary::from_agents(vec![
        FleetAgentSummary {
            child_session_id: a1,
            short_name: "sub-1".to_owned(),
            role: Some("researcher".to_owned()),
            status: FleetAgentStatus::Running,
            latest_activity: "reading docs".to_owned(),
            feed: vec!["started".to_owned(), "reading docs".to_owned()],
            worktree_branch: Some("mjolnr/worktree-sub-1".to_owned()),
        },
        FleetAgentSummary {
            child_session_id: a2,
            short_name: "sub-2".to_owned(),
            role: Some("refactor".to_owned()),
            status: FleetAgentStatus::Failed {
                reason: "merge conflict".to_owned(),
            },
            latest_activity: "conflict in Cargo.toml".to_owned(),
            feed: vec!["conflict in Cargo.toml".to_owned()],
            worktree_branch: Some("mjolnr/worktree-sub-2".to_owned()),
        },
    ]);

    let snapshot = RuntimeSnapshot {
        fleet: Arc::new(fleet.clone()),
        ..Default::default()
    };

    let client = snapshot_to_client(1, &snapshot);
    let bridged = client.fleet.as_ref().expect("fleet summary present");

    assert!(bridged.visible);
    assert_eq!(bridged.active_count, 1);
    assert_eq!(bridged.agents.len(), 2);
    assert_eq!(bridged.agents[0].short_name, "sub-1");
    assert_eq!(bridged.agents[0].status, FleetAgentStatus::Running);
    assert_eq!(
        bridged.agents[1].status,
        FleetAgentStatus::Failed {
            reason: "merge conflict".to_owned()
        }
    );

    // Serialization roundtrip check
    let json = serde_json::to_string(&client).expect("serialize client snapshot");
    assert!(json.contains("\"fleet\":"));
    assert!(json.contains("\"activeCount\":1"));
    let back: ClientSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(client, back);
}

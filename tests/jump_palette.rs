//! Integration tests for Jump Palette fleet navigation (Master Implementation Plan Phase 3 Slice 3.3).

#![allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use mjolnr::core::event::{MjolnrEvent, RunId, SessionId};
use mjolnr::tui::jump_palette::{JumpKind, JumpState, build_jump_items, filter_jump_items};
use mjolnr::tui::reducer::ViewState;

#[test]
fn jump_palette_indexes_and_filters_fleet_agents() {
    let mut view = ViewState::default();
    let child_1 = SessionId::new();
    let child_2 = SessionId::new();
    let run = RunId::new();

    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_1,
        label: "indexer starting".to_owned(),
    });
    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_2,
        label: "failed: lint error".to_owned(),
    });

    let items = build_jump_items(&view);
    let fleet_items: Vec<_> = items.iter().filter(|i| i.kind == JumpKind::Fleet).collect();

    assert_eq!(fleet_items.len(), 2);
    assert!(fleet_items[0].title.starts_with("agent:"));
    assert!(fleet_items[0].detail.contains("running"));
    assert!(fleet_items[1].detail.contains("failed"));

    // Filter by agent prefix
    let filtered = filter_jump_items(&items, "agent:");
    assert_eq!(filtered.len(), 2);

    // Filter by failure
    let failed_filtered = filter_jump_items(&items, "failed");
    assert_eq!(failed_filtered.len(), 1);
    assert_eq!(failed_filtered[0].kind, JumpKind::Fleet);
}

#[test]
fn jump_state_lifecycle() {
    let mut state = JumpState::new();
    assert!(!state.active);

    state.toggle();
    assert!(state.active);

    state.input_char('a');
    state.input_char('g');
    state.input_char('e');
    state.input_char('n');
    state.input_char('t');
    assert_eq!(state.query, "agent");

    state.close();
    assert!(!state.active);
    assert_eq!(state.query, "");
}

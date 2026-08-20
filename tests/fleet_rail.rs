//! Integration tests for the Worktree Fleet Rail and status dot indicators (Master Implementation Plan Phase 3 Slice 3.2).

#![allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use mjolnr::core::event::{MjolnrEvent, RunId, SessionId};
use mjolnr::core::fleet::FleetAgentStatus;
use mjolnr::tui::reducer::ViewState;

#[test]
fn fleet_activity_tracks_success_and_failure_states() {
    let mut view = ViewState::default();
    let child_1 = SessionId::new();
    let child_2 = SessionId::new();
    let run = RunId::new();

    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_1,
        label: "started".to_owned(),
    });
    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_1,
        label: "deliberating".to_owned(),
    });

    assert_eq!(view.fleet.len(), 1);
    assert!(!view.fleet[0].done);
    assert!(!view.fleet[0].failed);
    assert_eq!(view.fleet[0].latest, "deliberating");

    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_2,
        label: "failed: branch conflict".to_owned(),
    });

    assert_eq!(view.fleet.len(), 2);
    assert!(view.fleet[1].done);
    assert!(view.fleet[1].failed);
    assert_eq!(view.fleet[1].latest, "failed: branch conflict");

    let summary = view.fleet_summary();
    assert!(summary.visible);
    assert_eq!(summary.active_count, 1);
    assert_eq!(summary.agents[0].status, FleetAgentStatus::Running);
    assert_eq!(
        summary.agents[1].status,
        FleetAgentStatus::Failed {
            reason: "failed: branch conflict".to_owned(),
        }
    );
}

#[test]
fn fleet_activity_projects_settled_summary_when_all_done() {
    let mut view = ViewState::default();
    let child = SessionId::new();
    let run = RunId::new();

    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child,
        label: "finished".to_owned(),
    });

    assert!(view.fleet[0].done);
    assert!(!view.fleet[0].failed);

    let summary = view.fleet_summary();
    assert!(!summary.visible, "all finished hides the rail");
    assert_eq!(summary.active_count, 0);
    assert_eq!(summary.agents[0].status, FleetAgentStatus::Completed);
}

#[test]
fn fleet_rail_renders_status_dots_in_terminal() {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");

    let mut view = ViewState::default();
    let child_1 = SessionId::new();
    let child_2 = SessionId::new();
    let run = RunId::new();

    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_1,
        label: "running".to_owned(),
    });
    view.apply(&MjolnrEvent::SubagentActivity {
        session: SessionId::new(),
        run,
        child: child_2,
        label: "finished".to_owned(),
    });

    assert!(view.fleet_visible());

    terminal
        .draw(|f| {
            mjolnr::tui::layout::render(f, &view);
        })
        .expect("draw layout with fleet rail");

    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }

    assert!(
        text.contains("WORKTREE FLEET"),
        "header missing in frame:\n{text}"
    );
    assert!(text.contains('●'), "status dot missing in frame:\n{text}");
}

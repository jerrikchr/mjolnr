//! Integration tests for TUI Auxiliary Side Panel and Negative-Space Telemetry (Phase 5 Slice 5.1).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use mjolnr::core::model::{ModelId, ProviderId, Usage};
use mjolnr::tui::auxiliary_panel::format_negative_space_telemetry;
use mjolnr::tui::reducer::{FleetAgent, ViewState};
use mjolnr::tui::shell::render_workspace_shell;
use mjolnr::tui::workspace_types::WorkspaceSurface;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn render_shell_to_string(width: u16, height: u16, view: &ViewState) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("create terminal");
    terminal
        .draw(|f| {
            render_workspace_shell(f, f.area(), view, WorkspaceSurface::Work);
        })
        .expect("draw shell");

    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        for x in 0..width {
            text.push_str(buffer.cell((x, y)).map_or(" ", |c| c.symbol()));
        }
        text.push('\n');
    }
    text
}

#[test]
fn auxiliary_panel_toggle_and_visibility() {
    let mut view = ViewState::default();
    assert!(!view.auxiliary_panel_visible);

    view.toggle_auxiliary_panel();
    assert!(view.auxiliary_panel_visible);

    view.hide_auxiliary_panel();
    assert!(!view.auxiliary_panel_visible);
}

#[test]
fn negative_space_telemetry_formatting() {
    let mut view = ViewState::default();
    view.snapshot.usage = Usage {
        input_tokens: 150,
        output_tokens: 75,
    };
    view.fleet.push(FleetAgent {
        child: mjolnr::core::event::SessionId::new(),
        short: "sub-1".to_owned(),
        latest: "indexing".to_owned(),
        feed: Vec::new(),
        role: Some("indexer".to_owned()),
        done: false,
        failed: false,
        worktree_branch: None,
    });

    let spans = format_negative_space_telemetry(&view);
    let joined = spans.iter().map(|s| s.content.as_ref()).collect::<String>();

    assert!(joined.contains("150"));
    assert!(joined.contains("75"));
    assert!(joined.contains("1 active"));
    assert!(joined.contains("Alt+P"));
}

#[test]
fn auxiliary_panel_renders_across_viewport_sizes() {
    let mut view = ViewState::default();
    view.snapshot.provider = Some(ProviderId::new("anthropic"));
    view.snapshot.model = Some(ModelId::new("claude-3-5-sonnet"));
    view.auxiliary_panel_visible = true;

    // Wide terminal
    let wide_output = render_shell_to_string(140, 30, &view);
    assert!(wide_output.contains("Auxiliary Inspector"));
    assert!(wide_output.contains("Governance & Runtime"));

    // Narrow terminal
    let narrow_output = render_shell_to_string(80, 24, &view);
    assert!(narrow_output.contains("Auxiliary Inspector"));
}

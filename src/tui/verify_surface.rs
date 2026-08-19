//! Verification & Governance Evidence Surface for smed (Phase UX 3).
//!
//! Visualizes post-mutation execution evidence, governance policy telemetry,
//! auto-allowed side effect tallies, and audit logs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::core::message::{ContentBlock, ToolEffect};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// Evidence record representing a post-mutation command or check execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRecord {
    pub command: String,
    pub exit_code: i32,
    pub verified: bool,
    pub log_snippet: String,
    pub duration_ms: u64,
}

/// State for navigating the Verification & Governance Evidence Surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VerifySurfaceState {
    pub selected_record: usize,
    pub scroll_y: u16,
}

impl VerifySurfaceState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected_record: 0,
            scroll_y: 0,
        }
    }

    pub fn select_next(&mut self, total: usize) {
        if total == 0 {
            self.selected_record = 0;
            return;
        }
        if self.selected_record + 1 < total {
            self.selected_record += 1;
        }
    }

    pub fn select_prev(&mut self, total: usize) {
        if total == 0 {
            self.selected_record = 0;
            return;
        }
        if self.selected_record > 0 {
            self.selected_record -= 1;
        }
    }
}

/// Collects verification evidence records from view state messages or supplies defaults.
fn collect_verification_records(view: &ViewState) -> Vec<VerificationRecord> {
    let mut records = Vec::new();

    for entry in view.snapshot.messages.iter() {
        for block in &entry.message.blocks {
            if let ContentBlock::ToolResult { name, result, .. } = block
                && let ToolEffect::Command {
                    exit_code,
                    success,
                    duration_ms,
                } = &result.effect
            {
                let cmd_name = if name.is_empty() {
                    "command execution"
                } else {
                    name.as_str()
                };
                let snippet = if result.content.trim().is_empty() {
                    "No output reported".to_string()
                } else {
                    result
                        .content
                        .lines()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                records.push(VerificationRecord {
                    command: cmd_name.to_string(),
                    exit_code: exit_code.unwrap_or_else(|| i32::from(!*success)),
                    verified: *success,
                    log_snippet: snippet,
                    duration_ms: *duration_ms,
                });
            }
        }
    }

    // No fallback. A session that has run no commands has no verification
    // evidence, and this surface exists to say which commands actually ran.
    // It previously invented three records when the list came back empty —
    // including `cargo test --all-features` at exit 0 with "42 passed" — which
    // is the exact claim `AGENTS.md` §1.3 forbids: reported success requires
    // evidence, and a thing that was not checked must say it was not checked.
    records
}

/// Renders the top Governance Telemetry Header block.
fn render_telemetry_header(frame: &mut Frame, area: Rect, view: &ViewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .style(theme::panel())
        .title(Span::styled(
            " GOVERNANCE TELEMETRY & AUDIT GATE ",
            theme::title(),
        ));

    let is_full_auto = view.snapshot.policy.is_full_auto() || view.full_auto_armed;
    let full_auto_badge = if is_full_auto {
        Span::styled(" [ ⚡ FULL-AUTO ARMED ] ", theme::full_auto())
    } else {
        Span::styled(" [ 🛡️ GOVERNED / ASK ] ", theme::muted())
    };

    let policy_badge = Span::styled(
        format!(" Mode: {} ", view.snapshot.policy.label()),
        theme::approval().add_modifier(Modifier::BOLD),
    );

    let side_effects_text = Span::styled(
        format!(
            " Auto-Allowed Side Effects: {} ",
            view.auto_allowed_side_effects
        ),
        theme::verified(),
    );

    let recovery_text = Span::styled(
        format!(" Recovery: {:?} ", view.snapshot.recovery),
        theme::text(),
    );

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "  Policy Gate: ",
                theme::text().add_modifier(Modifier::BOLD),
            ),
            policy_badge,
            Span::raw("  │  Status: "),
            full_auto_badge,
        ]),
        Line::from(vec![
            Span::styled(
                "  Execution Tally: ",
                theme::text().add_modifier(Modifier::BOLD),
            ),
            side_effects_text,
            Span::raw("  │ "),
            recovery_text,
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Renders a single Evidence Log Card.
fn render_evidence_card(
    frame: &mut Frame,
    area: Rect,
    record: &VerificationRecord,
    is_selected: bool,
    index: usize,
) {
    let border_style = if is_selected {
        theme::title()
    } else {
        theme::muted()
    };

    let status_badge = if record.verified {
        Span::styled(
            " [✓ VERIFIED] ",
            theme::verified().add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [🔴 UNVERIFIED] ",
            theme::refusal().add_modifier(Modifier::BOLD),
        )
    };

    let title_line = Line::from(vec![
        Span::styled(format!(" #{:02} ", index + 1), theme::muted()),
        status_badge,
        Span::styled(
            format!(" {} ", record.command),
            theme::text().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("(exit: {}, {}ms)", record.exit_code, record.duration_ms),
            theme::muted(),
        ),
    ]);

    let card_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(theme::panel());

    let inner = card_block.inner(area);
    frame.render_widget(card_block, area);

    if inner.height < 2 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let header_area = chunks.first().copied().unwrap_or(inner);
    let snippet_area = chunks.get(1).copied().unwrap_or(inner);

    let header_para = Paragraph::new(title_line);
    frame.render_widget(header_para, header_area);

    if snippet_area.height > 0 {
        let snippet_lines: Vec<Line> = record
            .log_snippet
            .lines()
            .map(|l| Line::from(Span::styled(format!("  {l}"), theme::muted())))
            .collect();

        let snippet_para = Paragraph::new(snippet_lines).wrap(Wrap { trim: false });
        frame.render_widget(snippet_para, snippet_area);
    }
}

/// Renders the list of Verification Evidence Log Cards.
fn render_evidence_logs(
    frame: &mut Frame,
    area: Rect,
    records: &[VerificationRecord],
    state: &VerifySurfaceState,
) {
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .style(theme::panel())
        .title(Span::styled(
            format!(" VERIFICATION EVIDENCE LOGS ({}) ", records.len()),
            theme::title(),
        ));

    let inner_area = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    if records.is_empty() || inner_area.height < 2 {
        let empty = Paragraph::new(vec![
            Line::from(Span::styled(
                " No command has been run in this session.",
                theme::muted(),
            )),
            Line::from(Span::styled(
                " Nothing here is verified, because nothing has been checked.",
                theme::muted(),
            )),
        ])
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, inner_area);
        return;
    }

    let card_height = 5u16;
    let visible_cards = usize::from((inner_area.height / card_height).max(1));

    let selected = state.selected_record.min(records.len().saturating_sub(1));
    let start_idx = if selected >= visible_cards {
        selected - visible_cards + 1
    } else {
        0
    };
    let end_idx = (start_idx + visible_cards).min(records.len());

    let mut card_constraints = Vec::new();
    for _ in start_idx..end_idx {
        card_constraints.push(Constraint::Length(card_height));
    }
    card_constraints.push(Constraint::Min(0));

    let card_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(card_constraints)
        .split(inner_area);

    for (i, record_idx) in (start_idx..end_idx).enumerate() {
        if let (Some(card_area), Some(record)) =
            (card_chunks.get(i).copied(), records.get(record_idx))
        {
            let is_sel = record_idx == selected;
            render_evidence_card(frame, card_area, record, is_sel, record_idx);
        }
    }
}

/// Renders the footer hint bar.
fn render_footer_hint(frame: &mut Frame, area: Rect) {
    let footer_text = Line::from(vec![
        Span::styled(" [Up/Down] ", theme::title()),
        Span::styled("Navigate Evidence Logs", theme::muted()),
        Span::styled("  │  ", theme::muted()),
        Span::styled("[Esc] ", theme::title()),
        Span::styled("Return to Workspace", theme::muted()),
    ]);
    let paragraph = Paragraph::new(footer_text).style(theme::panel());
    frame.render_widget(paragraph, area);
}

/// Main surface rendering entry point for the Verification & Governance Evidence Surface.
pub fn render_verify_surface(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    state: &VerifySurfaceState,
) {
    if area.width < 10 || area.height < 6 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let header_chunk = chunks.first().copied().unwrap_or(area);
    let logs_chunk = chunks.get(1).copied().unwrap_or(area);
    let footer_chunk = chunks.get(2).copied().unwrap_or(area);

    render_telemetry_header(frame, header_chunk, view);

    let records = collect_verification_records(view);
    render_evidence_logs(frame, logs_chunk, &records, state);

    render_footer_hint(frame, footer_chunk);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn test_verify_surface_state_navigation() {
        let mut state = VerifySurfaceState::new();
        assert_eq!(state.selected_record, 0);

        state.select_next(3);
        assert_eq!(state.selected_record, 1);

        state.select_next(3);
        assert_eq!(state.selected_record, 2);

        // Clamped at total - 1
        state.select_next(3);
        assert_eq!(state.selected_record, 2);

        state.select_prev(3);
        assert_eq!(state.selected_record, 1);

        state.select_prev(3);
        assert_eq!(state.selected_record, 0);

        // Clamped at 0
        state.select_prev(3);
        assert_eq!(state.selected_record, 0);
    }

    /// A session that ran nothing has verified nothing.
    ///
    /// This surface used to push three records when the real list came back
    /// empty, the first of which claimed `cargo test --all-features` had exited
    /// 0 with "42 passed". A verification surface that manufactures a passing
    /// test run is the precise failure the product exists to prevent
    /// (`AGENTS.md` §1.3).
    #[test]
    fn no_commands_run_means_no_verification_records() {
        let view = ViewState::default();
        let records = collect_verification_records(&view);
        assert!(
            records.is_empty(),
            "verification evidence invented for a session that ran nothing: {records:?}"
        );
    }

    #[test]
    fn a_recorded_command_is_reported_with_its_own_exit_status() {
        use crate::core::message::{CanonicalMessage, ToolResult, TranscriptEntry};
        use std::sync::Arc;

        let mut view = ViewState::default();
        view.snapshot.messages = Arc::new(vec![TranscriptEntry::anchored(
            0,
            CanonicalMessage::tool_result(
                "call-0",
                "run_command",
                ToolResult {
                    effect: ToolEffect::Command {
                        exit_code: Some(1),
                        success: false,
                        duration_ms: 12,
                    },
                    ..ToolResult::ok("failure output")
                },
            ),
        )]);

        let records = collect_verification_records(&view);
        let record = records.first().expect("one record");
        assert_eq!(record.exit_code, 1);
        assert!(
            !record.verified,
            "a failed command must not read as verified"
        );
    }

    #[test]
    fn test_verify_surface_render() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        let view = ViewState::default();
        let state = VerifySurfaceState::new();

        terminal
            .draw(|f| {
                let area = f.area();
                render_verify_surface(f, area, &view, &state);
            })
            .unwrap();
    }
}

//! Read-only discovery result overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let Some(report) = &view.snapshot.last_discovery else {
        render_running(frame, area);
        return;
    };
    render_report(frame, area, report);
}

fn render_running(frame: &mut Frame, area: Rect) {
    let modal = centered(area, area.width.saturating_sub(4).min(92), 8);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("DISCOVERY RUNNING", theme::proposal())),
            Line::from(""),
            Line::from(Span::styled(
                "The runtime is reading bounded repository metadata; no commands are executed.",
                theme::muted(),
            )),
            Line::from(Span::styled(
                "Press ESC to close this view.",
                theme::muted(),
            )),
        ])
        .style(theme::modal())
        .block(Block::default().borders(Borders::ALL).title(" DISCOVERY ")),
        modal,
    );
}

fn render_report(frame: &mut Frame, area: Rect, report: &crate::core::discovery::DiscoveryReport) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{} · {} source file(s)",
                sanitize(&report.project_name),
                report.source_files
            ),
            theme::text().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "bundle: {}",
                sanitize(&report.bundle_path.display().to_string())
            ),
            theme::verified(),
        )),
        Line::from(Span::styled(
            format!(
                "languages: {} · unresolved imports: {} · truncated: {}",
                report
                    .language_counts
                    .iter()
                    .map(|count| format!("{} {}", count.language, count.files))
                    .collect::<Vec<_>>()
                    .join(", "),
                report.unresolved_imports,
                report.truncated
            ),
            theme::muted(),
        )),
        Line::from(""),
        Line::from(Span::styled("PROPOSED COMMANDS (not run)", theme::title())),
    ];
    if report.commands.is_empty() {
        lines.push(Line::from(Span::styled("  none detected", theme::muted())));
    } else {
        lines.extend(
            report
                .commands
                .iter()
                .take(8)
                .map(|command| Line::from(Span::styled(format!("  {command}"), theme::text()))),
        );
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "MODEL PROPOSAL (owner accepts or edits)",
        theme::title(),
    )));
    if report.model_proposals.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no connected tiered model proposal",
            theme::muted(),
        )));
    } else {
        lines.extend(report.model_proposals.iter().map(|proposal| {
            Line::from(Span::styled(
                format!(
                    "  {} → {}:{}",
                    proposal.role, proposal.provider, proposal.model
                ),
                theme::proposal(),
            ))
        }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Discovery is description, never permission; ESC closes.",
        theme::muted(),
    )));

    let height = u16::try_from(lines.len().saturating_add(2))
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let modal = centered(area, area.width.saturating_sub(4).min(104), height);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::proposal())
                    .title(Span::styled(" DISCOVERY — OKF BUNDLE ", theme::proposal())),
            ),
        modal,
    );
}

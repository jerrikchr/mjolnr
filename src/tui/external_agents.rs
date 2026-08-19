use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let height = area.height.saturating_sub(4).min(24);
    let modal = centered(area, area.width.saturating_sub(4).min(96), height);
    let agents = &view.snapshot.external_agents;

    let mut lines = vec![
        Line::from(Span::styled(
            "Dedicated worktrees (mjolnr/ext-*). Work is isolated and untrusted until you import.",
            theme::muted(),
        )),
        Line::from(""),
    ];

    if agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no external agents — add .mjolnr/external-agent/<name>.yaml and launch",
            theme::muted(),
        )));
    } else {
        for agent in agents {
            let status = match &agent.status {
                crate::core::client::external_agent::ExternalAgentStatus::Running => "RUNNING",
                crate::core::client::external_agent::ExternalAgentStatus::Stopped { .. } => {
                    "STOPPED"
                }
                crate::core::client::external_agent::ExternalAgentStatus::Failed { .. } => "FAILED",
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", sanitize(&agent.profile_name)),
                    theme::proposal().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{status} "), theme::muted()),
                Span::styled(sanitize(&agent.branch), theme::text()),
                Span::styled("  [EXTERNAL \u{00b7} UNVERIFIED]", theme::refusal()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    ", theme::muted()),
                Span::styled(sanitize(&agent.id), theme::muted()),
                Span::styled("  ", theme::muted()),
                Span::styled(sanitize(&agent.executable), theme::muted()),
            ]));
            lines.push(Line::from(""));
        }
    }

    lines.push(Line::from(Span::styled(
        "type /external again to close",
        theme::muted(),
    )));

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::proposal())
                    .title(Span::styled(
                        format!(" EXTERNAL AGENTS ({}) ", agents.len()),
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

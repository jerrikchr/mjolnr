//! Discovered and activated Agent Skills overlay.

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
    let modal = centered(area, area.width.saturating_sub(4).min(110), height);
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{} discovered · {} active · project trust {}",
                view.snapshot.skills.len(),
                view.snapshot.activated_skills.len(),
                if view.snapshot.workspace_trusted {
                    "granted"
                } else {
                    "not granted"
                }
            ),
            theme::text(),
        )),
        Line::from(""),
    ];
    if view.snapshot.skills.is_empty() {
        lines.push(Line::from(Span::styled(
            "no skills discovered",
            theme::muted(),
        )));
    }
    for skill in view.snapshot.skills.iter() {
        let active = view
            .snapshot
            .activated_skills
            .iter()
            .any(|name| name == &skill.name);
        lines.push(Line::from(vec![
            Span::styled(
                if active { "ACTIVE    " } else { "AVAILABLE " },
                if active {
                    theme::verified()
                } else {
                    theme::proposal()
                },
            ),
            Span::styled(
                format!("{} [{}]", sanitize(&skill.name), skill.scope.label()),
                theme::text().add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {}", sanitize(&skill.description)),
            theme::muted(),
        )));
    }
    if !view.snapshot.context_diagnostics.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("DIAGNOSTICS", theme::refusal())));
        for diagnostic in view.snapshot.context_diagnostics.iter() {
            lines.push(Line::from(Span::styled(
                format!("{} — {}", diagnostic.code, sanitize(&diagnostic.detail)),
                theme::refusal(),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "type /skills again to close",
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
                        " SKILLS ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

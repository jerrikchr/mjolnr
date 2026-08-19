//! Responsive empty-session dashboard.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::reducer::ViewState;
use crate::tui::theme;

const MAX_CONTENT_WIDTH: u16 = 92;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let width = area.width.min(MAX_CONTENT_WIDTH);
    let content = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    );
    frame.render_widget(
        Paragraph::new(lines(content, view))
            .style(theme::canvas())
            .wrap(Wrap { trim: false }),
        content,
    );
}

fn lines(area: Rect, view: &ViewState) -> Vec<Line<'static>> {
    const WORDMARK: [&str; 1] = ["mjolnr"];
    if area.width < 34 || area.height < 6 {
        return Vec::new();
    }

    let mut content = Vec::new();
    if area.height >= 18 {
        for row in WORDMARK {
            content.push(if theme::active_theme().has_gradient_wordmark {
                gradient_row(row, view.tick)
            } else {
                Line::from(Span::styled(row, theme::muted())).alignment(Alignment::Center)
            });
        }
    } else {
        content.push(
            Line::from(Span::styled(
                "mjolnr",
                theme::muted().add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        );
    }
    if theme::active_theme().has_gradient_wordmark {
        content.push(gradient_row(
            "────────────────────────────────────────",
            view.tick,
        ));
    }
    content.push(Line::from(""));
    content.push(
        Line::from(vec![
            Span::styled("GOVERNED AUTONOMOUS WORKSPACE · VERSION ", theme::muted()),
            Span::styled(env!("CARGO_PKG_VERSION"), theme::muted()),
        ])
        .alignment(Alignment::Center),
    );
    content.push(
        Line::from(Span::styled(
            "The model proposes; mjolnr's deterministic code disposes.",
            theme::muted(),
        ))
        .alignment(Alignment::Center),
    );

    if area.height >= 15 {
        content.push(Line::from(""));
        content.extend(workspace_lines(view));
        if area.height >= 22 {
            content.push(Line::from(""));
            content.extend(directive_lines());
        }
    }
    content.push(Line::from(""));
    content.push(
        Line::from(vec![
            Span::styled("› Type a directive below · ", theme::muted()),
            Span::styled("F1", theme::approval()),
            Span::styled(" opens the keymap", theme::muted()),
        ])
        .alignment(Alignment::Center),
    );

    let padding = (usize::from(area.height).saturating_sub(content.len()) / 2).max(1);
    let mut output = vec![Line::from(""); padding];
    output.extend(content);
    output
}

fn workspace_lines(view: &ViewState) -> Vec<Line<'static>> {
    let workspace = view
        .snapshot
        .workspace_root
        .as_ref()
        .map_or_else(|| "unknown".to_owned(), |path| path.display().to_string());
    let ready = view
        .snapshot
        .providers
        .iter()
        .filter(|provider| {
            provider.state == crate::core::runtime::ProviderConnectionState::Connected
        })
        .count();
    let not_connected = view.snapshot.providers.len().saturating_sub(ready);

    vec![
        Line::from(Span::styled(
            "── WORKSPACE STATUS ─────────────────────────────────────────",
            theme::muted(),
        )),
        Line::from(vec![
            Span::styled("  Workspace   ", theme::muted()),
            Span::styled(workspace, theme::text()),
        ]),
        Line::from(vec![
            Span::styled("  Policy      ", theme::muted()),
            Span::styled(
                format!("◆ {}", view.snapshot.policy.label()),
                if view.snapshot.policy.is_full_auto() {
                    theme::full_auto()
                } else {
                    theme::approval()
                },
            ),
            Span::styled(" · governed execution", theme::muted()),
        ]),
        Line::from(vec![
            Span::styled("  Providers   ", theme::muted()),
            Span::styled(format!("● {ready} ready"), theme::verified()),
            Span::styled(" · ", theme::muted()),
            Span::styled(format!("○ {not_connected} not connected"), theme::muted()),
        ]),
    ]
}

fn directive_lines() -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "── QUICK DIRECTIVES ─────────────────────────────────────────",
        theme::muted(),
    ))];
    lines.extend(
        [
            ("/model [P] [M]", "Select provider and model"),
            ("/auth", "Inspect provider readiness"),
            ("/skills", "List workspace capabilities"),
            ("/theme", "Switch the visual palette"),
            ("/mcp", "Inspect governed MCP hosts"),
            ("/tree", "Explore session branches"),
        ]
        .into_iter()
        .map(|(command, summary)| {
            Line::from(vec![
                Span::styled("  ", theme::muted()),
                Span::styled(format!("{command:<18}"), theme::proposal()),
                Span::styled(summary, theme::text()),
            ])
        }),
    );
    lines
}

fn gradient_row(row: &str, tick: u64) -> Line<'static> {
    let width = row.chars().count().max(1);
    let shift = theme::pulse(tick, 0.05);
    let spans = row
        .chars()
        .enumerate()
        .map(|(index, character)| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "wordmark width is far below f32 precision"
            )]
            let position = index as f32 / width.saturating_sub(1).max(1) as f32;
            Span::styled(
                character.to_string(),
                Style::default()
                    .fg(theme::wordmark_gradient((position + shift) % 1.0))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans).alignment(Alignment::Center)
}

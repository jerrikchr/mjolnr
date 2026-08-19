//! Trigger status overlay — configured triggers, connection/next-firing
//! state, and last outcomes.
//!
//! Read-only, like `/mcp` and `/usage`: this renders `snapshot.triggers`,
//! computed once at startup by the composition root exactly the way
//! `snapshot.mcp_servers` is. Firing a trigger, disabling it, or re-arming it
//! all happen through `mjolnr triggers run` and `mjolnr triggers rearm` — a
//! background process and a CLI command, neither of which the TUI may drive
//! (`tests/architecture.rs`: `tui` may not depend on `runtime` or `store`).

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
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} configured trigger(s)", view.snapshot.triggers.len()),
            theme::text(),
        )),
        Line::from(""),
    ];
    if view.snapshot.triggers.is_empty() {
        lines.push(Line::from(Span::styled(
            "no .mjolnr/triggers/ configuration",
            theme::muted(),
        )));
    }
    for trigger in view.snapshot.triggers.iter() {
        let (state, style) = if trigger.enabled {
            ("ARMED", theme::verified())
        } else {
            ("DISABLED", theme::refusal())
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{state:<9}"), style),
            Span::styled(
                sanitize(&trigger.name),
                theme::text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " · {} · overlap {} · failures {}/{}",
                    trigger.source.label(),
                    trigger.overlap.label(),
                    trigger.consecutive_failures,
                    trigger.max_consecutive_failures
                ),
                theme::muted(),
            ),
        ]));
        if let Some(code) = trigger.disabled_reason {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {code} — {} · `mjolnr triggers rearm {}`",
                    code.sentence(),
                    trigger.name
                ),
                theme::refusal(),
            )));
        }
        let outcome = trigger
            .last_outcome
            .map_or("never fired", |outcome| outcome.label());
        lines.push(Line::from(Span::styled(
            format!("  last outcome: {outcome}"),
            theme::muted(),
        )));
        if let Some(next) = trigger.next_fire_at {
            lines.push(Line::from(Span::styled(
                format!("  next firing: {next}"),
                theme::muted(),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Triggers fire through `mjolnr triggers run`, not this session · type /triggers again to close",
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
                        " TRIGGERS — scheduled and webhook runs ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

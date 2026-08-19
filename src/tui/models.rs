//! Model picker overlay.
//!
//! The one interactive overlay: arrows move the cursor, Enter commits, Esc
//! cancels, and typing filters (see `keymap::resolve_picker`). Every other
//! overlay is a read-only projection of snapshot state.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// Rows visible at once. The cursor scrolls the window rather than the list
/// growing without bound — a picker taller than the terminal cannot be used.
const VISIBLE_ROWS: usize = 12;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let choices = view.filtered_models();
    let cursor = view.model_cursor.min(choices.len().saturating_sub(1));

    let height = area
        .height
        .saturating_sub(4)
        .min(u16::try_from(choices.len().min(VISIBLE_ROWS) + 6).unwrap_or(u16::MAX));
    let modal = centered(area, area.width.saturating_sub(4).min(88), height);

    let mut lines = Vec::new();
    if view.composer.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} model(s) · type to filter", choices.len()),
            theme::muted(),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled("filter ", theme::muted()),
            Span::styled(
                sanitize(view.composer.trim()),
                theme::text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  · {} match(es)", choices.len()), theme::muted()),
        ]));
    }
    lines.push(Line::from(""));

    if choices.is_empty() {
        lines.push(Line::from(Span::styled(
            "no connected model matches · open /auth to connect a provider",
            theme::refusal(),
        )));
    }

    // Scroll the window so the cursor stays inside it.
    let first = cursor.saturating_sub(VISIBLE_ROWS.saturating_sub(1));
    for (index, choice) in choices.iter().enumerate().skip(first).take(VISIBLE_ROWS) {
        let selected = index == cursor;
        let active = Some(&choice.descriptor.provider) == view.snapshot.provider.as_ref()
            && Some(&choice.descriptor.id) == view.snapshot.model.as_ref();

        let marker = if selected { "▸ " } else { "  " };
        let provider_str = choice.descriptor.provider.as_str().to_uppercase();
        let model_str = choice.descriptor.id.as_str();

        let style = if selected {
            theme::text().add_modifier(Modifier::BOLD)
        } else {
            theme::text()
        };

        let mut spans = vec![
            Span::styled(marker, theme::proposal()),
            Span::styled(format!("{provider_str:<12} "), theme::title()),
            Span::styled("│ ", theme::muted()),
            Span::styled(sanitize(model_str), style),
        ];
        if active {
            spans.push(Span::styled("  (current)", theme::verified()));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ move · ENTER select · ESC cancel",
        theme::muted(),
    )));

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines).style(theme::modal()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::proposal())
                .title(Span::styled(
                    " MODEL SELECT ",
                    theme::proposal().add_modifier(Modifier::BOLD),
                )),
        ),
        modal,
    );
}

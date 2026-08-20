//! Model picker overlay.
//!
//! The model picker groups by provider, shows display names, and highlights
//! the active model. Typing filters (see `keymap::resolve_picker`).

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

const VISIBLE_ROWS: usize = 16;

#[allow(
    clippy::too_many_lines,
    reason = "grouped model picker; one render boundary"
)]
pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let choices = view.filtered_models();
    let cursor = view.model_cursor.min(choices.len().saturating_sub(1));
    let total = view.snapshot.models.len();

    let filter_active = !view.composer.trim().is_empty();
    let visible = VISIBLE_ROWS.min(choices.len().max(1));
    let height = area
        .height
        .saturating_sub(4)
        .min(u16::try_from(visible + 10).unwrap_or(u16::MAX).max(12));
    let modal = centered(area, area.width.saturating_sub(4).min(108), height);

    let mut lines = Vec::new();
    if filter_active {
        lines.push(Line::from(vec![
            Span::styled("filter ", theme::muted()),
            Span::styled(
                sanitize(view.composer.trim()),
                theme::text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  · {}/{} match(es)", choices.len(), total),
                theme::muted(),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} model(s)", choices.len()),
                theme::text().add_modifier(Modifier::BOLD),
            ),
            Span::styled("  · type to filter", theme::muted()),
        ]));
    }
    lines.push(Line::from(""));

    if choices.is_empty() {
        lines.push(Line::from(Span::styled(
            "no connected model matches · open /auth to connect a provider",
            theme::refusal(),
        )));
    } else {
        // Group by provider, preserving provider display order by first occurrence.
        let mut groups: BTreeMap<String, Vec<(usize, &crate::core::runtime::ModelChoice)>> =
            BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for (idx, choice) in choices.iter().enumerate() {
            let key = choice.descriptor.provider.as_str().to_owned();
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push((idx, choice));
        }
        // Sort provider groups alphabetically for determinism, but keep special
        // locals together: lm-studio, ollama first if present (local-first UX).
        order.sort_by(|a, b| {
            let local = |s: &str| matches!(s, "lm-studio" | "ollama");
            match (local(a), local(b)) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.cmp(b),
            }
        });

        let first = cursor.saturating_sub(VISIBLE_ROWS.saturating_sub(1));
        let last = (first + VISIBLE_ROWS).min(choices.len());

        for provider_key in order {
            let Some(group) = groups.get(&provider_key) else {
                continue;
            };
            let in_window = group.iter().any(|(idx, _)| *idx >= first && *idx < last);
            if !in_window {
                continue;
            }
            let count = group.len();
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", provider_key.to_uppercase()),
                    theme::proposal().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" · {count} model(s)"), theme::muted()),
            ]));
            for (idx, choice) in group {
                if *idx < first || *idx >= last {
                    continue;
                }
                let selected = *idx == cursor;
                let active = Some(&choice.descriptor.provider) == view.snapshot.provider.as_ref()
                    && Some(&choice.descriptor.id) == view.snapshot.model.as_ref();

                let marker = if selected { "▸ " } else { "  " };
                let model_str = choice.descriptor.id.as_str();
                let display = choice.descriptor.display_name.as_str();

                let style = if selected {
                    theme::text().add_modifier(Modifier::BOLD)
                } else {
                    theme::text()
                };

                let mut spans = vec![
                    Span::styled(marker, theme::proposal()),
                    Span::styled(sanitize(model_str), style),
                ];
                if display != model_str {
                    spans.push(Span::styled(
                        format!("  · {}", sanitize(display)),
                        theme::muted(),
                    ));
                }
                if active {
                    spans.push(Span::styled(
                        "  ● current",
                        theme::verified().add_modifier(Modifier::BOLD),
                    ));
                }
                // Capabilities: vision/tools hints when set (cheap signal, not noisy).
                if choice.descriptor.capabilities.images_in {
                    spans.push(Span::styled("  👁", theme::muted()));
                }
                lines.push(Line::from(spans));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ move · ENTER select · type to filter · ESC cancel",
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

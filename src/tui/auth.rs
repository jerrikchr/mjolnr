//! Provider credential status — interactive picker.
//!
//! Credential entry is handled by suspending raw mode and prompting via
//! `rpassword` so the key is never echoed or placed in the TUI transcript.
//! The overlay is navigable: arrow keys select a provider, Enter triggers
//! credential registration. Typing filters like the model picker.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::core::runtime::ProviderConnectionState;
use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

const VISIBLE_ROWS: usize = 14;

fn method_for(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "  · subscription or API key",
        "openai-codex" | "gemini-cli" | "antigravity" => "  · subscription login",
        "lm-studio" => "  · local server; optional API token",
        "ollama" => "  · local server",
        _ => "  · API key",
    }
}

fn group_label(state: ProviderConnectionState) -> (&'static str, ratatui::style::Style) {
    match state {
        ProviderConnectionState::Connected => ("● CONNECTED", theme::verified()),
        ProviderConnectionState::Discovering => ("◐ CONNECTING", theme::proposal()),
        ProviderConnectionState::NeedsReauth => ("○ NEEDS REAUTH", theme::refusal()),
        ProviderConnectionState::Unavailable => ("○ UNAVAILABLE", theme::refusal()),
        ProviderConnectionState::Disconnected => ("○ NOT CONNECTED", theme::muted()),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "grouped provider picker; one render boundary"
)]
pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let filtered = view.filtered_auth_providers();
    let cursor = view.auth_cursor.min(filtered.len().saturating_sub(1));
    let total = view.auth_providers().len();
    let connected = view
        .auth_providers()
        .iter()
        .filter(|c| c.state == ProviderConnectionState::Connected)
        .count();

    let filter_active = !view.composer.trim().is_empty();
    let visible = VISIBLE_ROWS.min(filtered.len().max(1));
    // Height: header (3) + rows + group headers + footer (3) + chrome
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
                format!("  · {}/{} match(es)", filtered.len(), total),
                theme::muted(),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{connected} connected"),
                theme::verified().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  · {} available", total - connected),
                theme::muted(),
            ),
            Span::styled("  · type to filter", theme::muted()),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "credentials resolve from environment first, then stored file; local endpoints are project config",
        theme::muted(),
    )));
    lines.push(Line::from(""));

    if filtered.is_empty() {
        if filter_active {
            lines.push(Line::from(Span::styled(
                "no provider matches that filter",
                theme::muted(),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "no providers registered",
                theme::muted(),
            )));
        }
    } else {
        // Group order: connected → connecting → needs-auth → unavailable → disconnected
        let order = |s: ProviderConnectionState| match s {
            ProviderConnectionState::Connected => 0,
            ProviderConnectionState::Discovering => 1,
            ProviderConnectionState::NeedsReauth => 2,
            ProviderConnectionState::Unavailable => 3,
            ProviderConnectionState::Disconnected => 4,
        };
        // Preserve overall cursor position; track first visible across all groups.
        let mut grouped: BTreeMap<u8, Vec<(usize, &crate::core::runtime::ProviderConnection)>> =
            BTreeMap::new();
        for (idx, conn) in filtered.iter().enumerate() {
            grouped
                .entry(order(conn.state))
                .or_default()
                .push((idx, conn));
        }

        let mut rendered = 0usize;
        let first = cursor.saturating_sub(VISIBLE_ROWS.saturating_sub(1));
        let last = (first + VISIBLE_ROWS).min(filtered.len());

        for (_key, group) in grouped {
            let in_window = group.iter().any(|(idx, _)| *idx >= first && *idx < last);
            if !in_window {
                continue;
            }
            let Some(first_entry) = group.first() else {
                continue;
            };
            let state = first_entry.1.state;
            let (label, style) = group_label(state);
            lines.push(Line::from(Span::styled(
                format!(" {label} "),
                style.add_modifier(Modifier::BOLD),
            )));
            for (idx, connection) in group {
                if idx < first || idx >= last {
                    continue;
                }
                rendered += 1;
                let selected = idx == cursor;
                let (base_style, suffix) = match connection.state {
                    ProviderConnectionState::Connected => (theme::verified(), ""),
                    ProviderConnectionState::Discovering => (theme::proposal(), ""),
                    ProviderConnectionState::Disconnected
                    | ProviderConnectionState::NeedsReauth
                    | ProviderConnectionState::Unavailable => {
                        (theme::refusal(), connection.detail.as_deref().unwrap_or(""))
                    }
                };
                let marker = if selected { "▸ " } else { "  " };
                let row_style = if selected {
                    base_style.add_modifier(Modifier::REVERSED)
                } else {
                    base_style
                };
                let provider_style = if selected {
                    theme::text()
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    theme::text().add_modifier(Modifier::BOLD)
                };
                let method = method_for(connection.provider.as_str());
                lines.push(Line::from(vec![
                    Span::styled(marker.to_owned(), row_style),
                    Span::styled(format!("  {:<13}", connection.state.label()), row_style),
                    Span::styled(sanitize(connection.provider.as_str()), provider_style),
                    Span::styled(method.to_owned(), theme::muted()),
                    Span::styled(
                        if suffix.is_empty() {
                            String::new()
                        } else {
                            format!("  · {}", sanitize(suffix))
                        },
                        theme::muted(),
                    ),
                ]));
            }
        }
        if rendered == 0 && !filtered.is_empty() {
            // Cursor window fell between groups after filter change; show hint.
            lines.push(Line::from(Span::styled(
                "use ↑/↓ — filtered results span multiple sections",
                theme::muted(),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("↑↓ move", theme::text().add_modifier(Modifier::BOLD)),
        Span::styled(
            "  ·  ENTER configure  ·  type to filter  ·  ESC close",
            theme::muted(),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "CLI fallback: `mjolnr auth login <provider>`",
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
                        " CONNECT PROVIDER ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

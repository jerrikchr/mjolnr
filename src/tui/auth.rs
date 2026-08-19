//! Provider credential status — interactive picker.
//!
//! Credential entry is handled by suspending raw mode and prompting via
//! `rpassword` so the key is never echoed or placed in the TUI transcript.
//! The overlay is navigable: arrow keys select a provider, Enter triggers
//! credential registration.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let providers = view.auth_providers();

    let height = area
        .height
        .saturating_sub(4)
        .min(u16::try_from(providers.len() + 8).unwrap_or(u16::MAX));
    let modal = centered(area, area.width.saturating_sub(4).min(84), height);

    let mut lines = vec![
        Line::from(Span::styled(
            "↑/↓ select · ENTER configure connection · ESC close",
            theme::muted(),
        )),
        Line::from(Span::styled(
            "credentials resolve from environment first, then stored file; local endpoints are project config",
            theme::muted(),
        )),
        Line::from(""),
    ];

    if providers.is_empty() {
        lines.push(Line::from(Span::styled(
            "no providers registered",
            theme::muted(),
        )));
    }

    let cursor = view.auth_cursor;
    for (idx, connection) in providers.iter().enumerate() {
        let selected = idx == cursor;
        let (base_style, suffix) = match connection.state {
            crate::core::runtime::ProviderConnectionState::Connected => (theme::verified(), ""),
            crate::core::runtime::ProviderConnectionState::Discovering => (theme::proposal(), ""),
            crate::core::runtime::ProviderConnectionState::Disconnected
            | crate::core::runtime::ProviderConnectionState::NeedsReauth
            | crate::core::runtime::ProviderConnectionState::Unavailable => {
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

        // Name the login shape so nobody stores a metered key expecting a
        // subscription (or hunts for a key a provider does not take).
        let method = match connection.provider.as_str() {
            "anthropic" => "  · subscription or API key",
            "openai-codex" | "gemini-cli" | "antigravity" => "  · subscription login",
            "lm-studio" => "  · local server; optional API token",
            "ollama" => "  · local server",
            _ => "  · API key",
        };

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

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "CLI fallback: `smed auth login <provider>`",
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
                        " AUTH — CREDENTIALS ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

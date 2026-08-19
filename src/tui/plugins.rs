//! Glassbox Plugin Inspector overlay (ADR-0016, Master Implementation Plan §3.4).
//!
//! Visualises discovered third-party plugins, declared tools, observer hooks,
//! required credentials, and trust tier pinning.
//!
//! Follows **ADR-0016 §3, §4 (Subprocess isolation & fixed `ToolTier::Execute`)**:
//! Displays projections and emits commands; never executes tools or passes secrets directly.

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
    let plugins = &view.snapshot.plugins;

    let mut lines = vec![
        Line::from(Span::styled(
            "ADR-0016: Subprocesses run in scrubbed env. All tools pinned to ToolTier::Execute.",
            theme::muted(),
        )),
        Line::from(""),
    ];

    if plugins.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no plugins discovered in .mjolnr/plugins/*.yaml or user config dir",
            theme::muted(),
        )));
        lines.push(Line::from(Span::styled(
            "  run mjolnr plugin create <name> or place a manifest at .mjolnr/plugins/<name>.yaml",
            theme::muted(),
        )));
    } else {
        for plugin in plugins.iter() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", sanitize(&plugin.name)),
                    theme::proposal().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "v{} by {}",
                        sanitize(&plugin.version),
                        sanitize(&plugin.publisher)
                    ),
                    theme::muted(),
                ),
                Span::styled("  [THIRDPARTY · EXECUTE]", theme::refusal()),
            ]));

            lines.push(Line::from(vec![
                Span::styled("    ", theme::muted()),
                Span::styled(sanitize(&plugin.description), theme::text()),
            ]));

            let mut meta_spans = vec![
                Span::styled("    Capabilities: ", theme::muted()),
                Span::styled(
                    format!("{} tool(s)", plugin.tool_count),
                    theme::approval().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" · ", theme::muted()),
                Span::styled(
                    format!("{} observer hook(s)", plugin.hook_count),
                    theme::text(),
                ),
            ];

            if !plugin.required_credentials.is_empty() {
                meta_spans.push(Span::styled(" · requires: ", theme::muted()));
                meta_spans.push(Span::styled(
                    plugin.required_credentials.join(", "),
                    theme::approval(),
                ));
            }

            lines.push(Line::from(meta_spans));
            lines.push(Line::from(""));
        }
    }

    lines.push(Line::from(Span::styled(
        "type /plugins again to close",
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
                        format!(" THIRD-PARTY PLUGINS ({}) ", plugins.len()),
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

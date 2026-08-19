//! Governed MCP server status overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::core::mcp::McpConnectionState;
use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let height = area.height.saturating_sub(4).min(22);
    let modal = centered(area, area.width.saturating_sub(4).min(92), height);
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{} explicitly configured server(s)",
                view.snapshot.mcp_servers.len()
            ),
            theme::text(),
        )),
        Line::from(""),
    ];
    if view.snapshot.mcp_servers.is_empty() {
        lines.push(Line::from(Span::styled(
            "no .mjolnr/mcp.yaml configuration",
            theme::muted(),
        )));
    }
    for server in view.snapshot.mcp_servers.iter() {
        let (state, style) = match server.state {
            McpConnectionState::Connected => ("CONNECTED", theme::verified()),
            McpConnectionState::Unavailable => ("UNAVAILABLE", theme::refusal()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{state:<12}"), style),
            Span::styled(
                sanitize(&server.name),
                theme::text().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · {} tools · {:?} tier", server.tool_count, server.tier),
                theme::muted(),
            ),
        ]));
        if let Some(reason) = server.reason {
            lines.push(Line::from(Span::styled(
                format!("  {reason} — {}", reason.sentence()),
                theme::refusal(),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "MCP annotations never lower Execute tier · type /mcp again to close",
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
                        " MCP TOOLS ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

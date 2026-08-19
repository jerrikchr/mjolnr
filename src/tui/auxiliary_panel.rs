//! Auxiliary side panel and negative-space telemetry widgets (Master Implementation Plan Phase 5 Slice 5.1).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::core::model::{ModelId, ProviderId};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// Builds negative-space telemetry spans for header/status rows on wide viewports.
#[must_use]
pub fn format_negative_space_telemetry(view: &ViewState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // Token usage telemetry
    let in_tok = view.snapshot.usage.input_tokens;
    let out_tok = view.snapshot.usage.output_tokens;
    if in_tok > 0 || out_tok > 0 {
        spans.push(Span::styled(" [", theme::muted()));
        spans.push(Span::styled(
            format!("↑{in_tok} ↓{out_tok} tok"),
            theme::muted().add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled("] ", theme::muted()));
    }

    // Memory rules telemetry
    if view.snapshot.memory.rules_count > 0 {
        spans.push(Span::styled(
            format!("🧠 {} rules ", view.snapshot.memory.rules_count),
            theme::muted(),
        ));
    }

    // Active fleet subagents telemetry
    if !view.fleet.is_empty() {
        let active_count = view.fleet.iter().filter(|a| !a.failed).count();
        spans.push(Span::styled(
            format!("🤖 {active_count} active "),
            theme::proposal().add_modifier(Modifier::BOLD),
        ));
    }

    // Shortcut hint
    spans.push(Span::styled(
        "[Alt+P: Inspector]",
        theme::muted().add_modifier(Modifier::DIM),
    ));

    spans
}

fn build_governance_lines(view: &ViewState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Governance & Runtime:",
        theme::proposal().add_modifier(Modifier::BOLD),
    )]));
    let provider_name = view
        .snapshot
        .provider
        .as_ref()
        .map_or("none", ProviderId::as_str);
    let model_name = view.snapshot.model.as_ref().map_or("none", ModelId::as_str);
    lines.push(Line::from(vec![
        Span::styled("  Provider / Model: ", theme::muted()),
        Span::styled(format!("{provider_name} / {model_name}"), theme::text()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Policy Mode: ", theme::muted()),
        Span::styled(format!("{:?}", view.snapshot.policy), theme::approval()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Tokens: ", theme::muted()),
        Span::styled(
            format!(
                "in: {}, out: {}",
                view.snapshot.usage.input_tokens, view.snapshot.usage.output_tokens
            ),
            theme::text(),
        ),
    ]));
    lines.push(Line::from(""));
    lines
}

fn build_memory_lines(view: &ViewState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Memory & Knowledge:",
        theme::proposal().add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("  Rules Active: ", theme::muted()),
        Span::styled(
            format!("{}", view.snapshot.memory.rules_count),
            theme::text(),
        ),
    ]));
    if let Some(facts) = view.snapshot.memory.facts_count {
        lines.push(Line::from(vec![
            Span::styled("  Facts Indexed: ", theme::muted()),
            Span::styled(format!("{facts}"), theme::text()),
        ]));
    }
    if let Some(episodes) = view.snapshot.memory.episodes_count {
        lines.push(Line::from(vec![
            Span::styled("  Episodes: ", theme::muted()),
            Span::styled(format!("{episodes}"), theme::text()),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

fn build_fleet_and_plugin_lines(view: &ViewState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Multi-Agent Fleet:",
        theme::proposal().add_modifier(Modifier::BOLD),
    )]));
    if view.fleet.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (no subagents currently running)",
            theme::muted().add_modifier(Modifier::DIM),
        )]));
    } else {
        for agent in view.fleet.iter().take(4) {
            let status_dot = if agent.failed {
                Span::styled("● ", theme::approval())
            } else {
                Span::styled("● ", theme::proposal())
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                status_dot,
                Span::styled(agent.short.clone(), theme::text()),
                Span::styled(format!(" - {}", agent.latest), theme::muted()),
            ]));
        }
    }
    lines.push(Line::from(""));

    lines.push(Line::from(vec![Span::styled(
        "Plugins Host:",
        theme::proposal().add_modifier(Modifier::BOLD),
    )]));
    if view.snapshot.plugins.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (no plugins loaded)",
            theme::muted().add_modifier(Modifier::DIM),
        )]));
    } else {
        for plugin in view.snapshot.plugins.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} v{}", plugin.name, plugin.version),
                    theme::text(),
                ),
                Span::styled(format!(" ({} tools)", plugin.tool_count), theme::muted()),
            ]));
        }
    }
    lines
}

/// Renders the Auxiliary Inspector side panel (toggled via Alt+P).
pub fn render_auxiliary_panel(frame: &mut Frame, area: Rect, view: &ViewState) {
    if area.width < 20 || area.height < 6 {
        return;
    }

    // Clear background to prevent bleed-through
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Span::styled(
            " ✦ Auxiliary Inspector (Alt+P) ",
            theme::title().add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .style(theme::panel());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = build_governance_lines(view);
    lines.extend(build_memory_lines(view));
    lines.extend(build_fleet_and_plugin_lines(view));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}

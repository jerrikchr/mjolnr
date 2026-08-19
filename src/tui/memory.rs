//! Glassbox Memory Inspector overlay (master implementation plan §2.3).
//!
//! Visualises the workspace memory hierarchy:
//! - **Tier 1 (Frozen Rules & Profile):** `.mjolnr/rules/*.md` and `.mjolnr/USER.md`
//! - **Tier 2 (Temporal Facts Projection):** SQLite triples in `.mjolnr/data/memory.db`
//! - **Tier 3 (Progressive Recall):** `memory_search`, `memory_timeline`, `memory_expand`
//! - **Episodic Distillation:** Consolidated sessions & key decisions
//!
//! Follows **Standing Law #2 (Recall is a projection, never authority)**:
//! Displays projections and emits commands; never executes direct SQL or queries.

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
    let memory = &view.snapshot.memory;

    let mut lines = vec![
        Line::from(Span::styled(
            "Standing Law #2: Memory is a disposable projection, never authority.",
            theme::muted(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "TIER 1 · RULES SNAPSHOT  ",
                theme::verified().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{} active rule(s) · user profile: {}",
                    memory.rules_count,
                    if memory.user_profile_present {
                        "loaded (.mjolnr/USER.md)"
                    } else {
                        "none"
                    }
                ),
                theme::text(),
            ),
        ]),
    ];

    if let Some(rules_error) = &memory.rules_error {
        lines.push(Line::from(Span::styled(
            format!("  rules load refused: {}", sanitize(rules_error)),
            theme::refusal(),
        )));
    }

    if memory.rule_names.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no rules in .mjolnr/rules/*.md",
            theme::muted(),
        )));
    } else {
        for rule in &memory.rule_names {
            lines.push(Line::from(vec![
                Span::styled("  • ", theme::proposal()),
                Span::styled(sanitize(rule), theme::text().add_modifier(Modifier::BOLD)),
                Span::styled(" (.mjolnr/rules/)", theme::muted()),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "TIER 2 & 3 · KNOWLEDGE TRIPLES  ",
            theme::proposal().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} fact(s) in .mjolnr/data/memory.db",
                memory
                    .facts_count
                    .map_or("unknown".to_owned(), |count| count.to_string())
            ),
            theme::text(),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  progressive recall: memory_search (hybrid), memory_timeline, memory_expand",
        theme::muted(),
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "EPISODIC MEMORY · CONSOLIDATION  ",
            theme::approval().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} episode(s) consolidated",
                memory
                    .episodes_count
                    .map_or("unknown".to_owned(), |count| count.to_string())
            ),
            theme::text(),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  background distiller derives session summaries from event ledger",
        theme::muted(),
    )));

    if let Some(projection_error) = &memory.projection_error {
        lines.push(Line::from(Span::styled(
            format!("  projection stale: {}", sanitize(projection_error)),
            theme::refusal(),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "type /memory again to close",
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
                        " MEMORY INSPECTOR ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

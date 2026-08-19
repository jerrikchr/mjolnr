//! Responsive Workspace Shell for smed.
//!
//! Provides spatial navigation, top navigation tabs, left work rail, primary
//! surfaces, right attention queue rail, and telemetry bottom status line.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::tui::reducer::ViewState;
use crate::tui::theme;
use crate::tui::workspace_types::{AttentionItem, WorkspaceSurface};

/// Terminal width tier classification according to layout policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalWidthTier {
    Wide,
    Medium,
    Narrow,
}

/// Computed geometry layout for the workspace shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellLayout {
    pub tier: TerminalWidthTier,
    pub top_nav: Rect,
    pub main_workspace: Rect,
    pub left_rail: Option<Rect>,
    pub primary_surface: Rect,
    pub right_attention_rail: Option<Rect>,
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' => ' ',
            '\n' => '\n',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// Classifies terminal width and computes shell layout sections.
#[must_use]
pub fn compute_shell_layout(area: Rect) -> ShellLayout {
    compute_shell_layout_with_context(area, false, false)
}

/// Computes shell layout sections accounting for attention item presence.
#[must_use]
pub fn compute_shell_layout_with_attention(area: Rect, has_attention: bool) -> ShellLayout {
    compute_shell_layout_with_context(area, has_attention, false)
}

/// Computes shell layout sections accounting for attention items and left rail content.
#[must_use]
pub fn compute_shell_layout_with_context(
    area: Rect,
    has_attention: bool,
    has_left_content: bool,
) -> ShellLayout {
    let tier = if area.width >= 120 {
        TerminalWidthTier::Wide
    } else if area.width >= 80 {
        TerminalWidthTier::Medium
    } else {
        TerminalWidthTier::Narrow
    };

    let [top_nav, main_workspace] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);

    if !has_left_content || tier == TerminalWidthTier::Narrow {
        let (r_w, show_r) = (30, has_attention && main_workspace.width >= 50);
        if show_r {
            let [primary_surface, right_attention_rail] =
                Layout::horizontal([Constraint::Min(1), Constraint::Length(r_w)])
                    .areas(main_workspace);

            ShellLayout {
                tier,
                top_nav,
                main_workspace,
                left_rail: None,
                primary_surface,
                right_attention_rail: Some(right_attention_rail),
            }
        } else {
            ShellLayout {
                tier,
                top_nav,
                main_workspace,
                left_rail: None,
                primary_surface: main_workspace,
                right_attention_rail: None,
            }
        }
    } else {
        let l_w = if tier == TerminalWidthTier::Wide {
            25
        } else {
            18
        };
        let r_w = 30;
        let show_r = has_attention && main_workspace.width >= l_w + r_w + 20;

        if show_r {
            let [left_rail, primary_surface, right_attention_rail] = Layout::horizontal([
                Constraint::Length(l_w),
                Constraint::Min(1),
                Constraint::Length(r_w),
            ])
            .areas(main_workspace);

            ShellLayout {
                tier,
                top_nav,
                main_workspace,
                left_rail: Some(left_rail),
                primary_surface,
                right_attention_rail: Some(right_attention_rail),
            }
        } else {
            let [left_rail, primary_surface] =
                Layout::horizontal([Constraint::Length(l_w), Constraint::Min(1)])
                    .areas(main_workspace);

            ShellLayout {
                tier,
                top_nav,
                main_workspace,
                left_rail: Some(left_rail),
                primary_surface,
                right_attention_rail: None,
            }
        }
    }
}

/// Renders top navigation bar with brand logo, workspace tabs, and attention badge.
pub fn render_top_nav(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    active_surface: WorkspaceSurface,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let attention_count = view.project_attention_items().len();

    let mut spans = vec![
        Span::styled(" ✦ mjolnr ", theme::title().add_modifier(Modifier::BOLD)),
        Span::styled("│ ", theme::muted()),
    ];

    for s in [
        WorkspaceSurface::Work,
        WorkspaceSurface::Conversation,
        WorkspaceSurface::Plan,
        WorkspaceSurface::Changes,
        WorkspaceSurface::Verify,
        WorkspaceSurface::Attention,
    ] {
        let label = s.label();
        if s == active_surface {
            spans.push(Span::styled(
                format!("[ {label} ] "),
                theme::proposal().add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(format!("{label} "), theme::muted()));
        }
    }

    spans.push(Span::styled("│ ", theme::muted()));
    let badge_style = if attention_count > 0 {
        theme::approval().add_modifier(Modifier::BOLD)
    } else {
        theme::muted()
    };
    spans.push(Span::styled(
        format!("[ ⚡ Attention: {attention_count} ]"),
        badge_style,
    ));

    if area.width >= 100 {
        spans.push(Span::styled(" │", theme::muted()));
        spans.extend(crate::tui::auxiliary_panel::format_negative_space_telemetry(view));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::panel()),
        area,
    );
}

/// Renders bottom status line with telemetry and keyboard hints.
pub fn render_bottom_status(frame: &mut Frame, area: Rect, view: &ViewState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if area.height == 1 {
        crate::tui::chrome::render_status(frame, area, view);
        return;
    }
    let [header, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    crate::tui::chrome::render_header(frame, header, view);
    crate::tui::chrome::render_status(frame, status, view);
}

/// Orchestrates the top nav, shell layout, side rails, and primary surface.
pub fn render_workspace_shell(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    active_surface: WorkspaceSurface,
) {
    frame.render_widget(Block::default().style(theme::canvas()), area);

    if area.width < 24 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Terminal too small — expand window")
                .style(theme::approval())
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let items = view.project_attention_items();
    let has_left_content = !view.fleet.is_empty();
    let layout = compute_shell_layout_with_context(area, !items.is_empty(), has_left_content);

    render_top_nav(frame, layout.top_nav, view, active_surface);

    if let Some(left) = layout.left_rail {
        render_left_rail(frame, left, view, active_surface);
    }
    if let Some(right) = layout.right_attention_rail {
        render_right_attention_rail(frame, right, &items);
    }

    let primary_area = if view.auxiliary_panel_visible && layout.primary_surface.width >= 75 {
        let [main, aux] = Layout::horizontal([Constraint::Min(40), Constraint::Length(35)])
            .areas(layout.primary_surface);
        crate::tui::auxiliary_panel::render_auxiliary_panel(frame, aux, view);
        main
    } else {
        layout.primary_surface
    };

    render_primary_surface(frame, primary_area, view, active_surface);

    if view.auxiliary_panel_visible
        && layout.primary_surface.width < 75
        && layout.primary_surface.width >= 24
    {
        let [_, aux] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(layout.primary_surface.width.min(40)),
        ])
        .areas(layout.primary_surface);
        crate::tui::auxiliary_panel::render_auxiliary_panel(frame, aux, view);
    }
}

fn render_left_rail(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    active_surface: WorkspaceSurface,
) {
    let title = if area.width >= 20 {
        " WORK RAIL "
    } else {
        " WORK "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::chrome())
        .style(theme::panel())
        .title(Span::styled(title, theme::title()));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines = vec![Line::from(vec![Span::styled(
        "SURFACES",
        theme::muted().add_modifier(Modifier::BOLD),
    )])];

    for s in [
        WorkspaceSurface::Work,
        WorkspaceSurface::Conversation,
        WorkspaceSurface::Plan,
        WorkspaceSurface::Changes,
        WorkspaceSurface::Verify,
        WorkspaceSurface::Attention,
    ] {
        let (prefix, style) = if s == active_surface {
            ("› ", theme::proposal().add_modifier(Modifier::BOLD))
        } else {
            ("  ", theme::text())
        };
        lines.push(Line::from(vec![
            Span::styled(prefix, theme::title()),
            Span::styled(s.label(), style),
        ]));
    }

    if !view.fleet.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "WORKTREE FLEET",
            theme::muted().add_modifier(Modifier::BOLD),
        )]));
        for (i, a) in view.fleet.iter().enumerate() {
            let is_focused = view.focused_agent == Some(i);
            let (dot, dot_style) = if a.failed {
                ("●", theme::refusal())
            } else if a.done {
                ("●", theme::verified())
            } else {
                ("●", theme::approval())
            };
            let prefix = if is_focused { "▸ " } else { "  " };
            let name_style = if is_focused {
                theme::proposal().add_modifier(Modifier::BOLD)
            } else if a.done {
                theme::muted()
            } else {
                theme::text()
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, theme::proposal()),
                Span::styled(format!("{dot} "), dot_style),
                Span::styled(a.short.clone(), name_style),
            ]));
            if let Some(role) = &a.role {
                lines.push(Line::from(vec![
                    Span::styled("    ", theme::muted()),
                    Span::styled(format!("({role})"), theme::muted()),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_right_attention_rail(frame: &mut Frame, area: Rect, items: &[AttentionItem]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::approval())
        .style(theme::panel())
        .title(Span::styled(
            " ⚡ ATTENTION QUEUE ",
            theme::approval().add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled("No pending items", theme::muted())));
    } else {
        for item in items {
            lines.push(Line::from(vec![
                Span::styled("⚡ ", theme::approval()),
                Span::styled(
                    item.priority.label(),
                    theme::approval().add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                sanitize(&item.title),
                theme::text(),
            )));
            lines.push(Line::from(Span::styled(
                format!("Code: {}", item.reason_code),
                theme::muted(),
            )));
            lines.push(Line::from(""));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn surface_work_lines(view: &ViewState) -> Vec<Line<'static>> {
    let session_str = view
        .snapshot
        .session
        .as_ref()
        .map_or_else(|| "none".to_string(), ToString::to_string);

    vec![
        Line::from(Span::styled("Active Session Overview", theme::title())),
        Line::from(""),
        Line::from(vec![
            Span::styled("Session ID: ", theme::muted()),
            Span::styled(session_str, theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Policy Mode: ", theme::muted()),
            Span::styled(view.snapshot.policy.label(), theme::text()),
        ]),
        Line::from(vec![
            Span::styled("Active Fleet Subagents: ", theme::muted()),
            Span::styled(view.fleet.len().to_string(), theme::proposal()),
        ]),
    ]
}

fn surface_attention_lines(view: &ViewState) -> Vec<Line<'static>> {
    let items = view.project_attention_items();
    let mut att_lines = Vec::new();
    if items.is_empty() {
        att_lines.push(Line::from(Span::styled(
            "No items requiring operator attention.",
            theme::muted(),
        )));
    } else {
        for item in items {
            att_lines.push(Line::from(vec![
                Span::styled("⚡ ", theme::approval()),
                Span::styled(
                    item.priority.label(),
                    theme::approval().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" — {}", item.title), theme::text()),
            ]));
            att_lines.push(Line::from(Span::styled(
                format!("  Reason Code: {}", item.reason_code),
                theme::muted(),
            )));
            att_lines.push(Line::from(""));
        }
    }
    att_lines
}

fn render_primary_surface(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    active_surface: WorkspaceSurface,
) {
    match active_surface {
        WorkspaceSurface::Conversation => {
            if should_render_launcher(view) {
                crate::tui::launcher::render_quick_launcher(frame, area, view, view.launcher);
            } else if view.focused_fleet_agent().is_some() {
                render_agent_feed(frame, area, view);
            } else {
                crate::tui::timeline::render(frame, area, view);
            }
            return;
        }
        WorkspaceSurface::Work => {
            if should_render_launcher(view) {
                crate::tui::launcher::render_quick_launcher(frame, area, view, view.launcher);
                return;
            }
            if view.snapshot.messages.is_empty() && !view.plan_steps.is_empty() {
                crate::tui::plan_surface::render_plan_surface(
                    frame,
                    area,
                    view,
                    &view.plan_surface,
                );
                return;
            }
            if view.focused_fleet_agent().is_some() {
                render_agent_feed(frame, area, view);
            } else {
                crate::tui::timeline::render(frame, area, view);
            }
            return;
        }
        WorkspaceSurface::Plan => {
            crate::tui::plan_surface::render_plan_surface(frame, area, view, &view.plan_surface);
            return;
        }
        WorkspaceSurface::Changes => {
            crate::tui::changes_surface::render_changes_surface(
                frame,
                area,
                view,
                &view.changes_surface,
            );
            return;
        }
        WorkspaceSurface::Verify => {
            crate::tui::verify_surface::render_verify_surface(
                frame,
                area,
                view,
                &view.verify_surface,
            );
            return;
        }
        WorkspaceSurface::Attention => {}
    }

    let (title, lines) = match active_surface {
        WorkspaceSurface::Conversation
        | WorkspaceSurface::Plan
        | WorkspaceSurface::Changes
        | WorkspaceSurface::Verify => unreachable!(),
        WorkspaceSurface::Work => (" WORKSPACE SURFACES & SESSIONS ", surface_work_lines(view)),
        WorkspaceSurface::Attention => {
            (" OPERATOR ATTENTION QUEUE ", surface_attention_lines(view))
        }
    };

    let border_style = if active_surface == WorkspaceSurface::Attention {
        theme::approval()
    } else {
        theme::chrome()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(theme::panel())
        .title(Span::styled(title, theme::title()));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Whether the quick launcher owns the primary surface.
///
/// The event loop asks the same question the renderer does, so the keys the
/// launcher claims are exactly the keys it is on screen to receive.
pub(crate) fn should_render_launcher(view: &ViewState) -> bool {
    view.snapshot.messages.is_empty()
        && view.plan_steps.is_empty()
        && view.streaming_text.is_empty()
        && view.model_notice.is_none()
        && !view.cancelling
        && matches!(view.status, crate::tui::reducer::RunStatus::Idle)
}

/// The focused child remains observable after the navigation-shell migration.
fn render_agent_feed(frame: &mut Frame, area: Rect, view: &ViewState) {
    let Some(agent) = view.focused_fleet_agent() else {
        crate::tui::timeline::render(frame, area, view);
        return;
    };
    let mut lines = vec![
        Line::from(Span::styled(
            format!("AGENT {} — activity feed", agent.short),
            theme::title(),
        )),
        Line::from(""),
    ];
    for entry in &agent.feed {
        lines.push(Line::from(Span::styled(sanitize(entry), theme::text())));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Span::styled(" FOCUS ", theme::title()));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_shell_layout_wide() {
        let area = Rect::new(0, 0, 160, 40);
        let layout_empty = compute_shell_layout(area);
        assert_eq!(layout_empty.tier, TerminalWidthTier::Wide);
        assert_eq!(layout_empty.top_nav.height, 1);
        assert!(layout_empty.left_rail.is_none());

        let layout_with_rail = compute_shell_layout_with_context(area, false, true);
        assert!(layout_with_rail.left_rail.is_some());
        assert_eq!(layout_with_rail.left_rail.unwrap().width, 25);
    }

    #[test]
    fn test_compute_shell_layout_medium() {
        let area = Rect::new(0, 0, 100, 30);
        let layout_empty = compute_shell_layout(area);
        assert_eq!(layout_empty.tier, TerminalWidthTier::Medium);
        assert_eq!(layout_empty.top_nav.height, 1);
        assert!(layout_empty.left_rail.is_none());

        let layout_with_rail = compute_shell_layout_with_context(area, false, true);
        assert!(layout_with_rail.left_rail.is_some());
        assert_eq!(layout_with_rail.left_rail.unwrap().width, 18);
        assert!(layout_with_rail.right_attention_rail.is_none());
    }

    #[test]
    fn test_compute_shell_layout_narrow() {
        let area = Rect::new(0, 0, 70, 20);
        let layout = compute_shell_layout(area);
        assert_eq!(layout.tier, TerminalWidthTier::Narrow);
        assert_eq!(layout.top_nav.height, 1);
        assert!(layout.left_rail.is_none());
        assert!(layout.right_attention_rail.is_none());
        assert_eq!(layout.primary_surface.width, 70);
    }
}

//! Pure Ratatui rendering: view state in, frame out.
//!
//! smed's visual language is a cyber-noir mission-control console, but colour
//! is operational rather than ornamental: cyan proposes, amber asks, phosphor
//! confirms, and magenta refuses. Untrusted text is sanitised before it reaches
//! the terminal buffer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::tui::reducer::{Overlay, ViewState};
use crate::tui::theme;

/// Below this size the console cannot state its boundaries honestly.
const MIN_WIDTH: u16 = 24;
const MIN_HEIGHT: u16 = 8;

pub fn render(frame: &mut Frame, view: &ViewState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::canvas()), area);

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small — expand window")
                .style(theme::approval())
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    // Encodings are invalidated on terminal width, here, rather than by any one
    // pane: the transcript and the composer preview ask for different cell
    // boxes, and letting either drive invalidation would clear the cache every
    // frame (see `image::ImageStore::prepare`).
    if let Ok(mut store) = view.images.try_borrow_mut() {
        store.prepare(area.width);
    }
    let attachments = composer_attachments(view, area.width);
    let preview_height = attachments
        .iter()
        .map(|attachment| attachment.size.height)
        .max()
        .unwrap_or(0);

    let is_idle_launcher = crate::tui::shell::should_render_launcher(view);

    let (_workspace, composer_rect) = if is_idle_launcher {
        let [workspace, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);
        crate::tui::shell::render_workspace_shell(frame, workspace, view, view.active_surface);
        crate::tui::shell::render_bottom_status(frame, status, view);
        (area, area)
    } else {
        let [workspace, preview, composer, status] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(preview_height),
            Constraint::Length(composer_height(view)),
            Constraint::Length(2),
        ])
        .areas(area);
        crate::tui::shell::render_workspace_shell(frame, workspace, view, view.active_surface);
        render_attachment_preview(frame, preview, view, &attachments);
        render_composer(frame, composer, view);
        crate::tui::shell::render_bottom_status(frame, status, view);
        (workspace, composer)
    };

    // Universal Jump Palette modal overlay (Ctrl+P)
    if view.jump_state.active {
        crate::tui::jump_palette::render_jump_palette(frame, area, view, &view.jump_state);
    }

    render_overlays(frame, area, view);

    // Last, and outside the overlay chain: the command menu is an affordance
    // attached to what is being typed, not a mode. It may sit over a transcript
    // but must never cover a gate or the jump palette.
    if crate::tui::commands::menu_applies(&view.composer)
        && view.overlay == Overlay::None
        && !view.jump_state.active
        && !view.snapshot.recovery.is_required()
        && view.snapshot.pending_approval.is_none()
    {
        let target_menu_rect = if is_idle_launcher {
            let width = area.width.min(84);
            let left = area.x + (area.width.saturating_sub(width)) / 2;
            let natural_h = 10u16;
            let top_margin = (area.height.saturating_sub(natural_h)) / 2;
            Rect {
                x: left,
                y: area.y.saturating_add(top_margin).saturating_add(3),
                width,
                height: 5,
            }
        } else {
            composer_rect
        };
        render_command_menu(frame, target_menu_rect, view);
    }
}

/// Rows the composer band needs: borders plus the draft's own lines.
fn composer_height(view: &ViewState) -> u16 {
    const MAX_INNER: u16 = 3;
    const BORDER: u16 = 2;

    let lines = u16::try_from(view.composer.split('\n').count()).unwrap_or(MAX_INNER);
    BORDER + lines.clamp(1, MAX_INNER)
}

/// Thumbnails for the image links in the draft, resolved before the layout is
/// split because their height decides how much room the band gets.
///
/// Returns empty when nothing is drawable — no picker, no workspace, an
/// unreadable path — so the band collapses to zero rows and the composer sits
/// where it always did.
fn composer_attachments(view: &ViewState, width: u16) -> Vec<crate::tui::image::Attachment> {
    if view.composer.is_empty() {
        return Vec::new();
    }
    let Ok(mut store) = view.images.try_borrow_mut() else {
        return Vec::new();
    };
    let columns = crate::tui::image::PREVIEW_COLUMNS.min(width.saturating_sub(2));
    crate::tui::image::attachments(
        &mut store,
        &view.composer,
        view.snapshot.workspace_root.as_deref(),
        ratatui::layout::Size::new(columns, crate::tui::image::PREVIEW_ROWS),
    )
}

/// The draft's images, drawn above the composer while it is still being typed.
///
/// The link text stays in the composer: it is editable text, and hiding it
/// would leave the cursor moving through characters that are not on screen.
/// Supported vision models receive bounded image bytes directly; unsupported
/// provider routes refuse image attachments before execution.
fn render_attachment_preview(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    attachments: &[crate::tui::image::Attachment],
) {
    if attachments.is_empty() || area.height == 0 || area.width == 0 {
        return;
    }
    let Ok(store) = view.images.try_borrow() else {
        return;
    };
    frame.render_widget(Block::default().style(theme::canvas()), area);

    let mut x = area.x.saturating_add(2);
    for attachment in attachments {
        let width = attachment.size.width.min(area.right().saturating_sub(x));
        let height = attachment.size.height.min(area.height);
        if width == 0 || height == 0 {
            break;
        }
        if let Some(protocol) = store.protocol(&attachment.key) {
            frame.render_widget(
                ratatui_image::Image::new(protocol),
                Rect::new(x, area.y, width, height),
            );
        }
        x = x.saturating_add(width).saturating_add(2);
    }

    // One caption for the band, to the right of the thumbnails, dimmed.
    let label = if attachments.len() == 1 {
        attachments
            .first()
            .map_or_else(String::new, |attachment| attachment.caption.clone())
    } else {
        format!("{} images", attachments.len())
    };
    let caption = format!(
        "{} · image attached — vision models receive image bytes",
        sanitize(&label)
    );
    if x < area.right() {
        frame.render_widget(
            Paragraph::new(caption)
                .style(theme::muted())
                .wrap(Wrap { trim: true }),
            Rect::new(x, area.y, area.right().saturating_sub(x), area.height),
        );
    }
}

/// The slash-command menu, docked directly above the composer.
///
/// Sized to its contents and anchored to the input rather than centred like the
/// overlays: it is a continuation of what the user is typing, and a menu that
/// jumps to the middle of the screen breaks that thread.
fn render_command_menu(frame: &mut Frame, composer: Rect, view: &ViewState) {
    const MAX_ROWS: usize = 8;

    let matches = crate::tui::commands::menu_entries(&view.composer, view);
    // At least one row so the "no command matches" line has somewhere to render.
    let rows = matches.len().clamp(1, MAX_ROWS);
    let height = u16::try_from(rows + 2).unwrap_or(u16::MAX);
    if composer.y < height {
        return;
    }

    let width = composer.width.min(64);
    let area = Rect {
        x: composer.x,
        y: composer.y.saturating_sub(height),
        width,
        height,
    };

    let mut lines = Vec::new();
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "no command matches",
            theme::refusal(),
        )));
    }
    for command in matches.iter().take(MAX_ROWS) {
        let mut spans = vec![Span::styled(
            format!("{:<11}", command.name),
            theme::title().add_modifier(Modifier::BOLD),
        )];
        match command.hint.as_deref() {
            Some(hint) => spans.push(Span::styled(format!("{hint:<18}"), theme::muted())),
            None => spans.push(Span::styled(format!("{:<18}", ""), theme::muted())),
        }
        // Live state beside the command, so the menu answers "what is it now?"
        // without the user having to open anything.
        match command.state.as_deref() {
            Some(state) => spans.push(Span::styled(sanitize(state), theme::text())),
            None => spans.push(Span::styled(command.summary.clone(), theme::muted())),
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).style(theme::modal()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::chrome())
                .title(Span::styled(" COMMANDS ", theme::muted())),
        ),
        area,
    );
}

fn render_resume_advisor(frame: &mut Frame, area: Rect, view: &ViewState) {
    use crate::core::continuation::ResumeWarning;
    let Some(advice) = &view.snapshot.resume_advice else {
        return;
    };
    let warning = match &advice.warning {
        ResumeWarning::QuotaStopped { resets_at } => format!(
            "previous run stopped at the quota reserve — reset {}",
            resets_at.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
        ),
        ResumeWarning::Stale { idle_seconds } => {
            format!("session idle for approximately {}h", idle_seconds / 3600)
        }
    };
    let handoff = advice.handoff.map_or_else(
        || "no handoff is available; create one before compact continuation".to_owned(),
        |id| format!("latest handoff {id}"),
    );
    let lines = vec![
        Line::from(Span::styled(
            warning,
            theme::approval().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "full resume ≈{} tokens (estimate; no cache discount assumed)",
            advice.estimated_full_resume_tokens
        )),
        Line::from(handoff),
        Line::from(""),
        Line::from(Span::styled(
            "[c] compact resume  [n] new session from handoff  [f] full resume",
            theme::approval().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Enter has no default while this warning is active.",
            theme::muted(),
        )),
    ];
    let modal = centered(area, area.width.saturating_sub(4).min(94), 12);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::approval())
                    .title(Span::styled(
                        " RESUME ADVISOR ",
                        theme::approval().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

/// The recovery gate.
///
/// Says exactly what smed knows and exactly what it does not. 's
/// anti-pattern is inferring that an interrupted command failed; the wording
/// here is the user-facing half of refusing to do that.
fn render_recovery(frame: &mut Frame, area: Rect, view: &ViewState) {
    let Some(work) = view.snapshot.recovery.work() else {
        return;
    };
    let modal = centered(area, area.width.saturating_sub(4).min(100), 16);

    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} (interrupted)", work.kind.label()),
            theme::refusal().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    append_preview(&mut lines, &sanitize(&work.summary()), "", theme::text());
    lines.push(Line::from(""));

    // The distinction that matters: whether smed can prove nothing happened.
    // Colour follows meaning — phosphor confirms, magenta refuses to.
    let (verdict, style) = if work.effect_is_certain() {
        (
            "mjolnr can prove this did not run: it was never authorised.",
            theme::verified(),
        )
    } else {
        (
            "mjolnr cannot prove whether this ran. Check the repository yourself \
             before continuing.",
            theme::refusal(),
        )
    };
    append_indented(&mut lines, verdict, "", style);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Nothing is retried automatically.",
        theme::muted(),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[c] abandon it and continue this session  [e] end session",
        theme::approval().add_modifier(Modifier::BOLD),
    )));

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::refusal())
                    .title(Span::styled(
                        " RECOVERY_REQUIRES_DECISION ",
                        theme::refusal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

fn append_indented(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    prefix: &str,
    style: ratatui::style::Style,
) {
    lines.extend(
        text.lines()
            .map(|line| Line::from(Span::styled(format!("{prefix}{line}"), style))),
    );
}

fn append_preview(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    prefix: &str,
    fallback: ratatui::style::Style,
) {
    if let Some(diff) = crate::tui::highlight::style_diff(text) {
        for line in diff {
            let mut spans = vec![Span::raw(prefix.to_owned())];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    } else {
        append_indented(lines, text, prefix, fallback);
    }
}
fn render_composer(frame: &mut Frame, area: Rect, view: &ViewState) {
    // The composer states its own availability. A directive box that looks ready
    // while the runtime would refuse the directive is a lie the user only
    // discovers after typing (AGENTS.md §1.3).
    let blocked = view.snapshot.recovery.is_required()
        || view.snapshot.resume_advice.is_some()
        || view.snapshot.store_failure.is_some();
    let plan_approval_pending = view.snapshot.plan.as_ref().is_some_and(|workflow| {
        matches!(
            workflow.stage,
            crate::core::plan::PlanStage::Proposed { .. }
                | crate::core::plan::PlanStage::Reviewed { .. }
        )
    });
    let title: Option<&str> = if blocked {
        Some(" HALTED ")
    } else if plan_approval_pending {
        Some(" PLAN READY · CTRL-Y TO APPROVE ")
    } else {
        None
    };
    let border = if blocked {
        theme::refusal()
    } else {
        theme::focus_ring()
    };

    let left_title = Line::from(vec![
        Span::styled(" Directive · ", theme::title().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("[ {} ] ", view.snapshot.policy.label()),
            if view.snapshot.policy.is_full_auto() {
                theme::full_auto()
            } else {
                theme::approval()
            },
        ),
    ]);

    let right_title = if view.snapshot.run_active {
        " Enter steers · Alt-Enter queues ".to_owned()
    } else {
        let model_name = view
            .snapshot
            .model
            .as_ref()
            .map_or("no-model", |m| m.as_str());
        format!(" {model_name} ")
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .style(theme::panel())
        .title(left_title)
        .title(Line::from(Span::styled(right_title, theme::muted())).alignment(Alignment::Right));

    if let Some(t) = title {
        block = block.title(Span::styled(
            t,
            if blocked {
                theme::refusal()
            } else {
                theme::approval()
            },
        ));
    }
    let inner = block.inner(area);
    let composer = sanitize(&view.composer);
    let last_row = composer.split('\n').count().saturating_sub(1);
    let visible_rows = usize::from(inner.height.max(1));

    // Calculate cursor row and column using prefix up to view.composer_cursor
    let prefix: String = view.composer.chars().take(view.composer_cursor).collect();
    let sanitized_prefix = sanitize(&prefix);
    let cursor_row = sanitized_prefix.split('\n').count().saturating_sub(1);
    let cursor_col = sanitized_prefix
        .split('\n')
        .next_back()
        .map_or(0, |line| line.chars().count());

    // Calculate dynamic scrolling keeping cursor in view
    let scroll = composer_scroll(last_row, cursor_row, visible_rows);

    let show_placeholder = composer.is_empty() && !view.snapshot.run_active && !blocked;
    let display = if show_placeholder {
        "Directive smed…  ·  / commands  ·  F1 keys"
    } else {
        composer.as_str()
    };
    frame.render_widget(
        Paragraph::new(display)
            .style(if show_placeholder {
                theme::muted().bg(theme::active_theme().panel)
            } else {
                theme::panel()
            })
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(block),
        area,
    );

    if view.snapshot.pending_approval.is_none()
        && view.overlay == Overlay::None
        && !blocked
        && inner.width > 0
    {
        let (x, y) = composer_cursor_position(inner, cursor_col, cursor_row, usize::from(scroll));
        frame.set_cursor_position((x, y));
    }
}

/// Scroll offset that keeps the composer cursor visible, saturating rather
/// than truncating when the transcript row count exceeds `u16`.
fn composer_scroll(last_row: usize, cursor_row: usize, visible_rows: usize) -> u16 {
    let mut scroll_rows = last_row.saturating_sub(visible_rows.saturating_sub(1));
    if cursor_row < scroll_rows {
        scroll_rows = cursor_row;
    }
    if cursor_row >= scroll_rows + visible_rows {
        scroll_rows = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    }
    u16::try_from(scroll_rows).unwrap_or(u16::MAX)
}

/// Cursor column/row inside the composer box, clamped to the inner bounds.
fn composer_cursor_position(
    inner: Rect,
    cursor_col: usize,
    cursor_row: usize,
    scroll_rows: usize,
) -> (u16, u16) {
    let x = inner.x.saturating_add(
        u16::try_from(cursor_col)
            .unwrap_or(u16::MAX)
            .min(inner.width.saturating_sub(1)),
    );
    let y = inner.y.saturating_add(
        u16::try_from(cursor_row.saturating_sub(scroll_rows))
            .unwrap_or(u16::MAX)
            .min(inner.height.saturating_sub(1)),
    );
    (x, y)
}

fn render_approval(frame: &mut Frame, area: Rect, view: &ViewState) {
    let Some(approval) = &view.snapshot.pending_approval else {
        return;
    };
    let modal = centered(area, area.width.saturating_sub(4).min(100), 15);
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{} — {}",
                sanitize(&approval.tool_name),
                format!("{:?}", approval.tier).to_ascii_uppercase()
            ),
            theme::approval().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "mjolnr policy gate — not an OS security sandbox.",
            theme::text().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    append_preview(&mut lines, &sanitize(&approval.preview), "", theme::text());
    lines.push(Line::from(""));
    let controls = if approval.tier == crate::core::tool::ToolTier::Execute {
        "[y] approve once  [a] approve this exact command for session  [n] deny"
    } else {
        "[y] approve once  [n] deny"
    };
    lines.push(Line::from(Span::styled(
        controls,
        theme::approval().add_modifier(Modifier::BOLD),
    )));

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(
                        ratatui::style::Style::default()
                            .fg(theme::dimmed_approval(theme::pulse(view.tick, 0.04))),
                    )
                    .title(Span::styled(
                        " AUTHORIZATION GATE ",
                        theme::approval().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height.saturating_sub(1)).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Strip terminal controls from model-, path-, provider-, and file-originated
/// text before Ratatui receives it.
pub(super) fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\t' => ' ',
            '\n' => '\n',
            character if character.is_control() => '\u{fffd}',
            character => character,
        })
        .collect()
}

#[allow(
    clippy::cognitive_complexity,
    reason = "the overlay dispatcher is one authoritative priority-ordered render boundary"
)]
fn render_overlays(frame: &mut Frame, area: Rect, view: &ViewState) {
    if view.snapshot.recovery.is_required()
        || view.snapshot.resume_advice.is_some()
        || view.snapshot.pending_approval.is_some()
    {
        frame.render_widget(
            Block::default().style(ratatui::style::Style::default().add_modifier(Modifier::DIM)),
            area,
        );
    }
    if view.snapshot.recovery.is_required() {
        render_recovery(frame, area, view);
    } else if view.snapshot.resume_advice.is_some() {
        render_resume_advisor(frame, area, view);
    } else if view.snapshot.pending_approval.is_some() {
        render_approval(frame, area, view);
    } else if view.overlay == Overlay::Help {
        crate::tui::help::render(frame, area);
    } else if view.overlay == Overlay::Skills {
        crate::tui::skills::render(frame, area, view);
    } else if view.overlay == Overlay::Usage {
        crate::tui::usage::render(frame, area, view);
    } else if view.overlay == Overlay::Mcp {
        crate::tui::mcp::render(frame, area, view);
    } else if view.overlay == Overlay::Triggers {
        crate::tui::triggers::render(frame, area, view);
    } else if view.overlay == Overlay::Memory {
        crate::tui::memory::render(frame, area, view);
    } else if view.overlay == Overlay::Plugins {
        crate::tui::plugins::render(frame, area, view);
    } else if view.overlay == Overlay::ExternalAgents {
        crate::tui::external_agents::render(frame, area, view);
    } else if view.overlay == Overlay::Models {
        crate::tui::models::render(frame, area, view);
    } else if view.overlay == Overlay::Auth {
        crate::tui::auth::render(frame, area, view);
    } else if view.overlay == Overlay::Tree {
        render_tree(frame, area, view);
    } else if view.overlay == Overlay::Theme {
        render_theme(frame, area, view);
    } else if view.overlay == Overlay::Config {
        render_config(frame, area, view);
    } else if view.overlay == Overlay::Discovery {
        crate::tui::discovery::render(frame, area, view);
    }
}

fn render_config(frame: &mut Frame, area: Rect, view: &ViewState) {
    let rows = view.config_rows();
    let height = u16::try_from(rows.len()).unwrap_or(0).saturating_add(8);
    let modal = centered(area, area.width.saturating_sub(4).min(90), height.min(24));
    let mut lines = vec![
        Line::from(Span::styled("SETTINGS & CONFIGURATION", theme::title())),
        Line::from(Span::styled(
            "A lens over diffable files — every change writes the file that owns it.",
            theme::muted(),
        )),
        Line::from(""),
    ];
    for (index, row) in rows.iter().enumerate() {
        let focused = index == view.config_cursor;
        let marker = if focused { "› " } else { "  " };
        let value_style = if row.staged.is_some() {
            theme::proposal()
        } else {
            theme::text()
        };
        let mut spans = vec![
            Span::styled(marker, theme::title()),
            Span::styled(format!("{:<26}", row.label), theme::muted()),
        ];
        if let Some(staged) = &row.staged {
            spans.push(Span::styled(row.current.clone(), theme::muted()));
            spans.push(Span::raw(" → "));
            spans.push(Span::styled(staged.clone(), value_style));
        } else {
            spans.push(Span::styled(row.current.clone(), value_style));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    if let Some(row) = rows.get(view.config_cursor) {
        if row.staged.is_some() {
            lines.push(Line::from(vec![
                Span::styled("will write ", theme::muted()),
                Span::styled(row.writes.clone(), theme::proposal()),
                Span::styled("  ·  Enter to write, Esc to discard", theme::muted()),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                "Space to change · ↑↓ to move · Esc to close",
                theme::muted(),
            )));
        }
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::chrome())
        .title(Span::styled(" CONFIGURATION ", theme::title()));
    frame.render_widget(Paragraph::new(lines).block(block), modal);
}

fn render_theme(frame: &mut Frame, area: Rect, view: &ViewState) {
    let modal = centered(area, area.width.saturating_sub(4).min(90), 16);
    let active_id = theme::active_theme_id();
    let depth = theme::detected_color_depth();

    let mut lines = vec![
        Line::from(Span::styled(
            format!("COLOR DEPTH: {} (detected at startup)", depth.label()),
            theme::muted(),
        )),
        Line::from(""),
    ];

    for (idx, theme_id) in theme::ThemeId::all().iter().enumerate() {
        let t = theme::Theme::for_id(*theme_id);
        let selected = idx == view.theme_cursor;
        let is_active = *theme_id == active_id;

        let prefix = if selected { " > " } else { "   " };
        let active_tag = if is_active { " [active]" } else { "" };
        let style = if selected {
            theme::title().add_modifier(Modifier::BOLD)
        } else {
            theme::text()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{prefix}{:<16}", t.name()), style),
            Span::styled(t.display_name(), style),
            Span::styled(active_tag, theme::muted()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Use Up/Down to navigate, Enter to select, Esc to close",
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
                    .border_style(theme::muted())
                    .title(Span::styled(" THEMES ", theme::title())),
            ),
        modal,
    );
}

/// One line of a message, short enough for a list row.
///
/// Truncates by *characters*, not bytes: slicing a `String` at a fixed byte
/// offset panics the moment a prompt contains a multi-byte character, and a
/// terminal is exactly where those turn up.
fn preview(text: &str) -> String {
    const LIMIT: usize = 60;
    let single_line = text.replace('\n', " ");
    let mut characters = single_line.chars();
    let head: String = characters.by_ref().take(LIMIT).collect();
    if characters.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

/// The "left behind" header: what the last switch walked away from.
///
/// Empty when nothing was left, which is the common case — a session that has
/// never branched has nothing to report and says nothing.
fn left_branch_lines(view: &ViewState) -> Vec<Line<'static>> {
    let Some(left) = &view.snapshot.left_branch else {
        return Vec::new();
    };

    let mut lines = vec![Line::from(Span::styled(" LEFT BEHIND ", theme::title()))];
    if let Some(origin) = &left.origin {
        lines.push(Line::from(vec![
            Span::styled("  from: ", theme::muted()),
            Span::styled(sanitize(&preview(origin)), theme::text()),
        ]));
    }

    // Only non-zero facts are shown: "0 files changed" is not a fact anyone
    // needs, and a row of zeroes buries the one number that is not zero.
    let mut facts = vec![format!("{} turns", left.turns)];
    if !left.files_changed.is_empty() {
        facts.push(format!("{} files changed", left.files_changed.len()));
    }
    if !left.files_read.is_empty() {
        facts.push(format!("{} files read", left.files_read.len()));
    }
    if !left.commands.is_empty() {
        facts.push(format!("{} commands", left.commands.len()));
    }
    if left.tool_failures > 0 {
        facts.push(format!("{} tool failures", left.tool_failures));
    }
    lines.push(Line::from(Span::styled(
        format!("  {}", facts.join(" · ")),
        theme::muted(),
    )));
    lines.push(Line::from(""));
    lines
}

/// One turn in the tree: the prompt line, its reply, and a blank separator.
fn tree_row_lines(
    row: &crate::tui::reducer::TreeRow,
    index: usize,
    is_selected: bool,
    rows: &[crate::tui::reducer::TreeRow],
) -> Vec<Line<'static>> {
    // The last row *at this depth in this run of siblings* — the next row is
    // shallower, or there is no next row. Using "last in the list" would draw
    // every branch as if it ended the tree.
    let is_last = rows
        .get(index + 1)
        .is_none_or(|next| next.depth < row.depth);
    let indent = "    ".repeat(row.depth);
    let branch_marker = if is_last { "└── " } else { "├── " };
    let child_marker = if is_last {
        "    └── "
    } else {
        "│   └── "
    };

    // An abandoned turn is dimmed throughout: it is history the session is not
    // following, and rendering it identically to the live branch would
    // misreport which conversation is in play.
    let live = if row.on_active_branch {
        theme::proposal()
    } else {
        theme::muted()
    };
    let prompt_style = if !row.on_active_branch {
        theme::muted()
    } else if is_selected {
        theme::title().add_modifier(Modifier::BOLD)
    } else {
        theme::text()
    };

    let mut prompt_line = vec![
        Span::styled(if is_selected { "> " } else { "  " }, theme::proposal()),
        Span::raw(indent.clone()),
        Span::styled(branch_marker, theme::muted()),
        Span::styled(format!("[{}] You: ", index + 1), live),
        Span::styled(sanitize(&preview(&row.prompt)), prompt_style),
    ];
    if !row.on_active_branch {
        prompt_line.push(Span::styled("  (other branch)", theme::muted()));
    }
    // A row with no durable event behind it cannot be branched from. Marked in
    // the list rather than only on refusal: a user should not have to press
    // Enter to find out a row is not a branch point.
    if row.sequence.is_none() {
        prompt_line.push(Span::styled("  (no branch point)", theme::muted()));
    }

    vec![
        Line::from(prompt_line),
        Line::from(vec![
            Span::raw("  "),
            Span::raw(indent),
            Span::styled(child_marker, theme::muted()),
            Span::styled(
                "Assistant: ",
                if row.on_active_branch {
                    theme::verified()
                } else {
                    theme::muted()
                },
            ),
            Span::styled(
                sanitize(
                    &row.answer
                        .as_deref()
                        .map_or_else(|| "(streaming / pending)".to_owned(), preview),
                ),
                theme::muted(),
            ),
        ]),
        Line::from(""),
    ]
}

fn render_tree(frame: &mut Frame, area: Rect, view: &ViewState) {
    let modal = centered(area, area.width.saturating_sub(4).min(100), 20);
    let mut lines = vec![
        Line::from(Span::styled(" SESSION TREE ", theme::title())),
        Line::from(Span::styled(
            " [Enter] on this branch rewinds to that turn; on another, returns to it.",
            theme::muted(),
        )),
        Line::from(""),
    ];

    lines.extend(left_branch_lines(view));

    let rows = view.tree_rows();
    if rows.is_empty() {
        lines.push(Line::from("  No user turns in history yet."));
    } else {
        for (index, row) in rows.iter().enumerate() {
            lines.extend(tree_row_lines(row, index, index == view.tree_cursor, &rows));
        }
    }

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(
                        ratatui::style::Style::default()
                            .fg(theme::dimmed_approval(theme::pulse(view.tick, 0.04))),
                    )
                    .title(Span::styled(" TIME TRAVEL ", theme::title())),
            ),
        modal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_cannot_reach_the_screen() {
        let hostile = "hello\x1b[2Jworld\x07";
        let clean = sanitize(hostile);

        assert!(!clean.contains('\x1b'), "escape survived sanitisation");
        assert!(!clean.contains('\x07'), "bell survived sanitisation");
        assert!(clean.contains("hello"));
        assert!(clean.contains("world"));
    }

    #[test]
    fn newlines_survive_but_tabs_become_spaces() {
        assert_eq!(sanitize("a\nb"), "a\nb");
        assert_eq!(sanitize("a\tb"), "a b");
    }
}

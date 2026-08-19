//! Quick Launcher dashboard for an idle, empty workspace (Phase UX 4).
//!
//! Shows what the next directive would run under — policy, route, workspace,
//! credential readiness — and lets the operator change the policy before
//! typing. Every value on this surface is read from the runtime snapshot.
//! Nothing here is a placeholder or an example: a launcher that displays a
//! model the session is not configured to use is worse than one that admits it
//! has no model, because the operator acts on what it says.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::core::policy::PolicyMode;
use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// A selectable policy mode, described by what it actually permits.
///
/// `effect` is written from the decision table in `policy::decide` and must be
/// changed with it. Read is always allowed; the modes differ only in how they
/// treat Write and Execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LauncherPreset {
    pub name: &'static str,
    pub mode: PolicyMode,
    pub effect: &'static str,
}

/// The policy modes the launcher may select.
///
/// `full-auto` is deliberately absent. It is reachable only through the armed
/// grant that states what it authorises and waits for a separate confirmation,
/// and a preset card that set it on one keypress would be a surface taking an
/// action it is not allowed to take (`AGENTS.md` §11 law 3).
pub const PRESETS: [LauncherPreset; 3] = [
    LauncherPreset {
        name: "Research",
        mode: PolicyMode::ReadOnly,
        effect: "Reads freely. Every file write and command is refused outright.",
    },
    LauncherPreset {
        name: "Governed",
        mode: PolicyMode::Ask,
        effect: "Reads freely. Every file write and command stops for approval.",
    },
    LauncherPreset {
        name: "Workspace Write",
        mode: PolicyMode::WorkspaceWrite,
        effect: "Reads and writes inside the workspace. Commands still stop for approval.",
    },
];

/// Cursor position over [`PRESETS`].
///
/// The cursor is not the policy: moving it changes nothing until the operator
/// confirms, and the live mode is always read back from the runtime snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LauncherState {
    pub selected_preset: usize,
}

impl LauncherState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the cursor to the next preset, wrapping.
    pub fn select_next(&mut self) {
        self.selected_preset = (self.selected_preset + 1) % PRESETS.len();
    }

    /// Moves the cursor to the previous preset, wrapping.
    pub fn select_prev(&mut self) {
        self.selected_preset = self
            .selected_preset
            .checked_sub(1)
            .unwrap_or(PRESETS.len() - 1);
    }

    /// The policy mode under the cursor.
    #[must_use]
    pub fn selected_mode(&self) -> Option<PolicyMode> {
        PRESETS.get(self.selected_preset).map(|preset| preset.mode)
    }
}

/// Style definition for the active focus ring.
#[must_use]
pub fn focus_ring() -> Style {
    theme::proposal().add_modifier(Modifier::BOLD)
}

/// Renders the Quick Launcher dashboard with a centered hero prompt.
pub fn render_quick_launcher(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    state: LauncherState,
) {
    if area.width < 20 || area.height < 6 {
        return;
    }

    let header_h = if area.height >= 16 && area.width >= 76 {
        7
    } else {
        2
    };
    let natural_h = header_h + 1 + 5 + 1 + 1;
    let top_margin = (area.height.saturating_sub(natural_h)) / 2;
    let content_area = Rect {
        x: area.x,
        y: area.y.saturating_add(top_margin),
        width: area.width,
        height: area.height.saturating_sub(top_margin),
    };

    let [header, _space1, prompt_area, _space2, footer] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(content_area);

    render_header(frame, header, view);
    render_hero_prompt(frame, prompt_area, view, state);
    render_footer(frame, footer, view, state);
}

fn render_hero_prompt(frame: &mut Frame, area: Rect, view: &ViewState, _state: LauncherState) {
    if area.width < 30 || area.height < 3 {
        return;
    }
    let width = area.width.min(84);
    let left = area.x + (area.width.saturating_sub(width)) / 2;
    let prompt_rect = Rect {
        x: left,
        y: area.y,
        width,
        height: area.height.min(5),
    };

    let (route_text, route_style) = match (
        view.snapshot.provider.as_ref(),
        view.snapshot.model.as_ref(),
    ) {
        (Some(provider), Some(model)) => (format!("{provider}:{model}"), theme::proposal()),
        _ => (
            "none configured — type /model to choose".to_owned(),
            theme::muted(),
        ),
    };

    // Border title carrying active policy and route trust signals
    let title_line = Line::from(vec![
        Span::styled(
            " Directive Session · ",
            theme::title().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[ {} ] ", view.snapshot.policy.label()),
            if view.snapshot.policy.is_full_auto() {
                theme::full_auto()
            } else {
                theme::approval()
            },
        ),
        Span::styled(format!("· {route_text} "), route_style),
    ]);

    let block = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::focus_ring())
        .style(theme::panel());

    let inner = block.inner(prompt_rect);
    frame.render_widget(block, prompt_rect);
    if inner.height == 0 {
        return;
    }

    let composer = crate::tui::layout::sanitize(&view.composer);
    let last_row = composer.split('\n').count().saturating_sub(1);
    let visible_rows = usize::from(inner.height.max(1));

    let prefix: String = view.composer.chars().take(view.composer_cursor).collect();
    let sanitized_prefix = crate::tui::layout::sanitize(&prefix);
    let cursor_row = sanitized_prefix.split('\n').count().saturating_sub(1);
    let cursor_col = sanitized_prefix
        .split('\n')
        .next_back()
        .map_or(0, |line| line.chars().count());

    let mut scroll_rows = last_row.saturating_sub(visible_rows.saturating_sub(1));
    if cursor_row < scroll_rows {
        scroll_rows = cursor_row;
    }
    if cursor_row >= scroll_rows + visible_rows {
        scroll_rows = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    }
    let scroll = u16::try_from(scroll_rows).unwrap_or(u16::MAX);

    let show_placeholder = composer.is_empty();
    let display = if show_placeholder {
        "Directive smed…  ·  / commands  ·  F1 keys"
    } else {
        composer.as_str()
    };

    frame.render_widget(
        Paragraph::new(display)
            .style(if show_placeholder {
                theme::muted()
            } else {
                theme::text()
            })
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );

    // Render active terminal cursor in the centered prompt box
    if view.snapshot.pending_approval.is_none()
        && view.overlay == crate::tui::reducer::Overlay::None
        && inner.width > 0
    {
        let visible_cursor_row = cursor_row.saturating_sub(scroll_rows);
        let x = inner
            .x
            .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX));
        let y = inner
            .y
            .saturating_add(u16::try_from(visible_cursor_row).unwrap_or(u16::MAX));
        if x < inner.right() && y < inner.bottom() {
            frame.set_cursor_position((x, y));
        }
    }
}

fn render_header(frame: &mut Frame, area: Rect, view: &ViewState) {
    const BANNER: [&str; 6] = [
        "███╗   ███╗     ██╗ ██████╗ ██╗     ███╗   ██╗██████╗ ",
        "████╗ ████║     ██║██╔═══██╗██║     ████╗  ██║██╔══██╗",
        "██╔████╔██║     ██║██║   ██║██║     ██╔██╗ ██║██████╔╝",
        "██║╚██╔╝██║██   ██║██║   ██║██║     ██║╚██╗██║██╔══██╗",
        "██║ ╚═╝ ██║╚█████╔╝╚██████╔╝███████╗██║ ╚████║██║  ██║",
        "╚═╝     ╚═╝ ╚════╝  ╚═════╝ ╚══════╝╚═╝  ╚═══╝╚═╝  ╚═╝",
    ];

    if area.height == 0 {
        return;
    }

    let mut lines = Vec::new();
    if area.height >= 6 && area.width >= 56 {
        for (i, line) in BANNER.iter().enumerate() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "banner is six rows and tick is a frame counter; sub-23-bit precision is invisible"
            )]
            let shift = (view.tick as f32 * 0.05 + i as f32 * 0.08) % 1.0;
            lines.push(gradient_line(line, shift));
        }
    } else {
        lines.push(gradient_title("✦ mjolnr", view.tick));
    }

    if area.height > u16::try_from(lines.len()).unwrap_or(u16::MAX) {
        lines.push(
            Line::from(Span::styled(
                "governed execution · the model proposes, code disposes",
                theme::muted(),
            ))
            .alignment(Alignment::Center),
        );
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn gradient_line(text: &str, shift: f32) -> Line<'static> {
    let width = text.chars().count();
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "gradient position across a banner line of at most ~80 columns"
            )]
            let pos = idx as f32 / width.saturating_sub(1).max(1) as f32;
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(theme::wordmark_gradient((pos + shift) % 1.0))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans).alignment(Alignment::Center)
}

fn gradient_title(text: &str, tick: u64) -> Line<'static> {
    let width = text.chars().count().max(1);
    let shift = theme::pulse(tick, 0.05);
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "wordmark width is far below f32 precision"
            )]
            let pos = idx as f32 / width.saturating_sub(1).max(1) as f32;
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(theme::wordmark_gradient((pos + shift) % 1.0))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans).alignment(Alignment::Center)
}

fn render_footer(frame: &mut Frame, area: Rect, view: &ViewState, state: LauncherState) {
    if area.height == 0 {
        return;
    }

    let selected_idx = state.selected_preset.min(PRESETS.len().saturating_sub(1));
    let mut preset_spans = vec![Span::styled("Presets: ", theme::muted())];
    for (idx, preset) in PRESETS.iter().enumerate() {
        let is_selected = idx == selected_idx;
        let is_live = view.snapshot.policy == preset.mode;

        let label = if is_live {
            format!("[ {} · IN EFFECT ] ", preset.name)
        } else if is_selected {
            format!("[ ▸ {} ] ", preset.name)
        } else {
            format!("[ {} ] ", preset.name)
        };

        let style = if is_live {
            theme::verified().add_modifier(Modifier::BOLD)
        } else if is_selected {
            theme::focus_ring()
        } else {
            theme::muted()
        };
        preset_spans.push(Span::styled(label, style));
    }

    preset_spans.push(Span::styled(" · ", theme::muted()));
    preset_spans.push(Span::styled("Shift+Tab", theme::title()));
    preset_spans.push(Span::styled(" apply policy · ", theme::muted()));
    preset_spans.push(Span::styled("Ctrl+P", theme::title()));
    preset_spans.push(Span::styled(" jump palette · ", theme::muted()));
    preset_spans.push(Span::styled("/help", theme::title()));
    preset_spans.push(Span::styled(" commands", theme::muted()));

    let footer = Line::from(preset_spans).alignment(Alignment::Center);
    frame.render_widget(Paragraph::new(footer).style(theme::canvas()), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_wraps_in_both_directions() {
        let mut state = LauncherState::new();
        assert_eq!(state.selected_preset, 0);

        state.select_next();
        state.select_next();
        assert_eq!(state.selected_preset, 2);
        state.select_next();
        assert_eq!(state.selected_preset, 0);

        state.select_prev();
        assert_eq!(state.selected_preset, 2);
    }

    #[test]
    fn full_auto_is_not_selectable_from_the_launcher() {
        // Reaching full-auto requires the armed grant, which states what it
        // authorises and waits for a separate confirmation.
        assert!(!PRESETS.iter().any(|preset| preset.mode.is_full_auto()));
    }

    #[test]
    fn every_preset_offers_a_distinct_reachable_mode() {
        let mut modes: Vec<PolicyMode> = PRESETS.iter().map(|preset| preset.mode).collect();
        let before = modes.len();
        modes.dedup();
        assert_eq!(before, modes.len(), "duplicate policy mode in presets");

        for preset in PRESETS {
            assert_eq!(
                LauncherState {
                    selected_preset: PRESETS
                        .iter()
                        .position(|candidate| candidate.mode == preset.mode)
                        .expect("preset present"),
                }
                .selected_mode(),
                Some(preset.mode)
            );
        }
    }

    // The cross-check that each `effect` string matches `policy::decide` lives
    // in `tests/tui_frames.rs`: `tui` may not import `policy` (AGENTS.md §2.1),
    // and an integration test can see both without inverting the dependency.
}

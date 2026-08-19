//! Viewport Scroll Engine for smed.
//!
//! Controls scroll position, follow-output intent, history pinning, and
//! rendering of the floating pinned history indicator pill.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::widgets::Paragraph;

use crate::tui::theme;

/// The user's scroll intent for the transcript viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewportIntent {
    /// Auto-scroll to newest output during streaming.
    #[default]
    FollowOutput,
    /// Locks scroll position at a historical line offset from the bottom.
    PinnedHistory { offset: u16 },
}

/// State container for managing viewport scrolling and auto-follow behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewportState {
    /// Active scroll intent.
    pub intent: ViewportIntent,
    /// Total lines in the rendered document.
    pub total_lines: usize,
    /// Visible height of the viewport container.
    pub visible_height: usize,
}

impl ViewportState {
    /// Creates a new `ViewportState` with default `FollowOutput` intent.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            intent: ViewportIntent::FollowOutput,
            total_lines: 0,
            visible_height: 0,
        }
    }

    /// Creates a `ViewportState` with specified initial dimensions.
    #[must_use]
    pub const fn with_dimensions(total_lines: usize, visible_height: usize) -> Self {
        Self {
            intent: ViewportIntent::FollowOutput,
            total_lines,
            visible_height,
        }
    }

    /// Returns `true` if the scroll position is pinned in history.
    #[must_use]
    pub const fn is_pinned(&self) -> bool {
        matches!(self.intent, ViewportIntent::PinnedHistory { .. })
    }

    /// Computes the maximum valid line offset from the bottom of the content.
    #[must_use]
    pub fn max_offset(&self) -> u16 {
        let max = self.total_lines.saturating_sub(self.visible_height);
        u16::try_from(max).unwrap_or(u16::MAX)
    }

    /// Returns the current line offset from the bottom.
    #[must_use]
    pub const fn current_offset(&self) -> u16 {
        match self.intent {
            ViewportIntent::FollowOutput => 0,
            ViewportIntent::PinnedHistory { offset } => offset,
        }
    }

    /// Computes the top-line scroll position for a Ratatui `Paragraph`.
    #[must_use]
    pub fn compute_scroll_y(&self) -> u16 {
        let max = self.total_lines.saturating_sub(self.visible_height);
        let offset = usize::from(self.current_offset());
        let top = max.saturating_sub(offset);
        u16::try_from(top).unwrap_or(u16::MAX)
    }

    /// Updates total lines and visible height, adjusting offset if bounds change.
    pub fn set_bounds(&mut self, total_lines: usize, visible_height: usize) {
        self.total_lines = total_lines;
        self.visible_height = visible_height;
        if let ViewportIntent::PinnedHistory { offset } = self.intent {
            let max = self.max_offset();
            if offset > max {
                if max == 0 {
                    self.intent = ViewportIntent::FollowOutput;
                } else {
                    self.intent = ViewportIntent::PinnedHistory { offset: max };
                }
            }
        }
    }

    /// Scrolls up by the specified number of lines.
    ///
    /// Scrolling up automatically switches intent to `PinnedHistory`.
    pub fn scroll_up(&mut self, lines: u16) {
        if lines == 0 {
            return;
        }
        let max = self.max_offset();
        if max == 0 {
            self.intent = ViewportIntent::FollowOutput;
            return;
        }
        let current = self.current_offset();
        let new_offset = current.saturating_add(lines).min(max);
        if new_offset > 0 {
            self.intent = ViewportIntent::PinnedHistory { offset: new_offset };
        } else {
            self.intent = ViewportIntent::FollowOutput;
        }
    }

    /// Scrolls down by the specified number of lines.
    ///
    /// Reaching the bottom automatically switches intent back to `FollowOutput`.
    pub fn scroll_down(&mut self, lines: u16) {
        if let ViewportIntent::PinnedHistory { offset } = self.intent {
            let new_offset = offset.saturating_sub(lines);
            if new_offset == 0 {
                self.intent = ViewportIntent::FollowOutput;
            } else {
                self.intent = ViewportIntent::PinnedHistory { offset: new_offset };
            }
        }
    }

    /// Scrolls up by one page (the visible viewport height).
    pub fn page_up(&mut self) {
        let step = u16::try_from(self.visible_height.max(1)).unwrap_or(u16::MAX);
        self.scroll_up(step);
    }

    /// Scrolls down by one page (the visible viewport height).
    pub fn page_down(&mut self) {
        let step = u16::try_from(self.visible_height.max(1)).unwrap_or(u16::MAX);
        self.scroll_down(step);
    }

    /// Scrolls all the way to the top of historical content.
    pub fn home(&mut self) {
        let max = self.max_offset();
        if max > 0 {
            self.intent = ViewportIntent::PinnedHistory { offset: max };
        } else {
            self.intent = ViewportIntent::FollowOutput;
        }
    }

    /// Scrolls all the way to the bottom and resumes live output follow.
    pub fn end(&mut self) {
        self.intent = ViewportIntent::FollowOutput;
    }

    /// Resumes follow mode, returning scroll position to live streaming output.
    pub fn resume_follow(&mut self) {
        self.intent = ViewportIntent::FollowOutput;
    }
}

/// Renders a floating banner pill at the top-right of the viewport when history is pinned.
///
/// Banner format: `[ 📌 Pinned History (+N lines) — Press End / Esc to resume live output ]`
/// Styled with `theme::proposal()` text on `theme::panel()` background.
pub fn render_pinned_history_pill(frame: &mut Frame, area: Rect, offset: u16) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let pill_text =
        format!("[ 📌 Pinned History (+{offset} lines) — Press End / Esc to resume live output ]");
    let char_count = pill_text.chars().count();
    let display_width = u16::try_from(char_count.saturating_add(1)).unwrap_or(u16::MAX);
    let pill_width = display_width.min(area.width);
    let pill_x = area.right().saturating_sub(pill_width);
    let pill_area = Rect::new(pill_x, area.y, pill_width, 1);

    let active = theme::active_theme();
    let style = theme::proposal().bg(active.panel);

    frame.render_widget(
        Paragraph::new(pill_text)
            .style(style)
            .alignment(Alignment::Right),
        pill_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn default_state_is_follow_output() {
        let state = ViewportState::new();
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
        assert!(!state.is_pinned());
        assert_eq!(state.current_offset(), 0);
    }

    #[test]
    fn scroll_up_pins_history() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.scroll_up(5);
        assert_eq!(state.intent, ViewportIntent::PinnedHistory { offset: 5 });
        assert!(state.is_pinned());
        assert_eq!(state.current_offset(), 5);
        assert_eq!(state.compute_scroll_y(), 75);
    }

    #[test]
    fn scroll_up_clamps_to_max_offset() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.scroll_up(200);
        assert_eq!(state.max_offset(), 80);
        assert_eq!(state.intent, ViewportIntent::PinnedHistory { offset: 80 });
        assert_eq!(state.compute_scroll_y(), 0);
    }

    #[test]
    fn scroll_down_unpins_at_bottom() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.scroll_up(10);
        assert!(state.is_pinned());

        state.scroll_down(5);
        assert_eq!(state.intent, ViewportIntent::PinnedHistory { offset: 5 });

        state.scroll_down(10);
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
        assert!(!state.is_pinned());
        assert_eq!(state.compute_scroll_y(), 80);
    }

    #[test]
    fn page_up_and_down_moves_by_visible_height() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.page_up();
        assert_eq!(state.intent, ViewportIntent::PinnedHistory { offset: 20 });

        state.page_down();
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
    }

    #[test]
    fn home_and_end_control() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.home();
        assert_eq!(state.intent, ViewportIntent::PinnedHistory { offset: 80 });

        state.end();
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
    }

    #[test]
    fn resume_follow_resets_intent() {
        let mut state = ViewportState::with_dimensions(50, 10);
        state.scroll_up(15);
        assert!(state.is_pinned());

        state.resume_follow();
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
    }

    #[test]
    fn bounds_update_adjusts_pinned_offset() {
        let mut state = ViewportState::with_dimensions(100, 20);
        state.scroll_up(80);
        assert_eq!(state.current_offset(), 80);

        // Shrink content so max offset becomes 30
        state.set_bounds(50, 20);
        assert_eq!(state.current_offset(), 30);

        // Shrink content below visible height
        state.set_bounds(15, 20);
        assert_eq!(state.intent, ViewportIntent::FollowOutput);
    }

    #[test]
    fn render_pill_does_not_panic() {
        let backend = TestBackend::new(100, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                render_pinned_history_pill(f, area, 12);
            })
            .unwrap();
    }
}

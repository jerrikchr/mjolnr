//! Universal Jump Palette component ( / Soft TUI Design System).
//!
//! Provides a centered modal overlay (`Ctrl+P`) allowing fuzzy search and
//! quick navigation across active work items, primary surfaces, slash
//! commands, and session files.
//!
//! `Ctrl+J` — the key the UX phase specified — is the composer's newline on
//! terminals that cannot report `Shift+Enter`, so the palette does not take it.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// Category of jump target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpKind {
    WorkItem,
    Surface,
    Command,
    File,
    Fleet,
}

impl JumpKind {
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::WorkItem => "[WORK]",
            Self::Surface => "[VIEW]",
            Self::Command => "[CMD ]",
            Self::File => "[FILE]",
            Self::Fleet => "[FLEET]",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::WorkItem => "📋",
            Self::Surface => "📍",
            Self::Command => "⚡",
            Self::File => "📄",
            Self::Fleet => "🤖",
        }
    }
}

/// A single searchable target in the Jump Palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpItem {
    pub title: String,
    pub detail: String,
    pub kind: JumpKind,
    pub target: String,
}

/// State container for the Jump Palette interactive modal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JumpState {
    pub query: String,
    pub selected_index: usize,
    pub active: bool,
}

impl JumpState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle(&mut self) {
        self.active = !self.active;
        if !self.active {
            self.close();
        }
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.selected_index = 0;
    }

    pub fn input_char(&mut self, c: char) {
        self.query.push(c);
        self.selected_index = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected_index = 0;
    }

    pub fn move_cursor_up(&mut self, total: usize) {
        if total == 0 {
            self.selected_index = 0;
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = total.saturating_sub(1);
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn move_cursor_down(&mut self, total: usize) {
        if total == 0 {
            self.selected_index = 0;
            return;
        }
        if self.selected_index + 1 < total {
            self.selected_index += 1;
        } else {
            self.selected_index = 0;
        }
    }
}

/// Focus ring style derived from theme palette.
fn focus_ring() -> Style {
    theme::proposal().add_modifier(Modifier::BOLD)
}

fn surface_jump_items() -> Vec<JumpItem> {
    let surfaces = [
        (
            "Work",
            "Primary surface: active work items & session list",
            "Work",
        ),
        (
            "Conversation",
            "Primary surface: interactive chat & timeline",
            "Conversation",
        ),
        (
            "Plan",
            "Primary surface: execution plan & step breakdown",
            "Plan",
        ),
        (
            "Changes",
            "Primary surface: file diffs & modified files",
            "Changes",
        ),
        (
            "Verify",
            "Primary surface: verification results & test outputs",
            "Verify",
        ),
        (
            "Attention",
            "Primary surface: items requiring operator decision",
            "Attention",
        ),
    ];
    surfaces
        .into_iter()
        .map(|(title, detail, target)| JumpItem {
            title: title.to_string(),
            detail: detail.to_string(),
            kind: JumpKind::Surface,
            target: target.to_string(),
        })
        .collect()
}

fn fleet_jump_items(fleet: &[crate::tui::reducer::FleetAgent]) -> Vec<JumpItem> {
    fleet
        .iter()
        .map(|agent| {
            let status_label = if agent.failed {
                "failed"
            } else if agent.done {
                "completed"
            } else {
                "running"
            };
            let detail = format!(
                "Fleet Agent ({status_label}) · {}{}",
                agent.role.as_deref().unwrap_or("subagent"),
                agent
                    .worktree_branch
                    .as_deref()
                    .map_or(String::new(), |b| format!(" · {b}"))
            );
            JumpItem {
                title: format!("agent:{}", agent.short),
                detail,
                kind: JumpKind::Fleet,
                target: format!("fleet:{}", agent.child),
            }
        })
        .collect()
}

fn command_jump_items() -> Vec<JumpItem> {
    let commands = [
        ("/help", "Show available keyboard shortcuts & help overlay"),
        ("/skills", "View registered agent skills & tools"),
        (
            "/memory",
            "Inspect workspace memory state, rules, and facts",
        ),
        (
            "/plugins",
            "Inspect third-party capability plugins (.mjolnr/plugins/)",
        ),
        ("/theme", "Switch color theme"),
        ("/model", "Select active LLM provider & model"),
        ("/config", "Configure routes, personas, and settings"),
    ];
    commands
        .into_iter()
        .map(|(cmd, detail)| JumpItem {
            title: cmd.to_string(),
            detail: detail.to_string(),
            kind: JumpKind::Command,
            target: cmd.to_string(),
        })
        .collect()
}

fn file_jump_items(view: &ViewState) -> Vec<JumpItem> {
    extract_session_files(view)
        .into_iter()
        .map(|file| {
            let filename = std::path::Path::new(&file)
                .file_name()
                .map_or_else(|| file.as_str(), |f| f.to_str().unwrap_or(file.as_str()));
            JumpItem {
                title: filename.to_string(),
                detail: format!("File in session · {file}"),
                kind: JumpKind::File,
                target: file,
            }
        })
        .collect()
}

/// Build searchable jump targets from the current view state.
#[must_use]
pub fn build_jump_items(view: &ViewState) -> Vec<JumpItem> {
    let mut items = surface_jump_items();

    for item in view.project_work_items() {
        items.push(JumpItem {
            title: item.title.clone(),
            detail: format!("Work Item ({:?}) · {}", item.lifecycle, item.provider_model),
            kind: JumpKind::WorkItem,
            target: item.id.clone(),
        });
    }

    items.extend(fleet_jump_items(&view.fleet));
    items.extend(command_jump_items());
    items.extend(file_jump_items(view));
    items
}

/// Filter items by query using case-insensitive title and detail matching.
#[must_use]
pub fn filter_jump_items(items: &[JumpItem], query: &str) -> Vec<JumpItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| {
            item.title.to_lowercase().contains(&q) || item.detail.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// Render the Universal Jump Palette modal overlay.
pub fn render_jump_palette(frame: &mut Frame, area: Rect, view: &ViewState, state: &JumpState) {
    if !state.active {
        return;
    }

    let width = area.width.saturating_mul(60) / 100;
    let height = area.height.saturating_mul(50) / 100;
    let modal_area = centered_modal(area, width.max(30), height.max(8));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Universal Jump (Ctrl+P) ")
        .style(theme::modal())
        .border_style(focus_ring());

    frame.render_widget(Clear, modal_area);
    let inner_area = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let [input_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(3), // Search Input Box
        Constraint::Min(1),    // Filtered Items List
        Constraint::Length(1), // Footer Navigation Hint
    ])
    .areas(inner_area);

    render_input_box(frame, input_area, &state.query);

    let all_items = build_jump_items(view);
    let filtered = filter_jump_items(&all_items, &state.query);
    render_item_list(frame, list_area, &filtered, state.selected_index);

    render_footer_hint(frame, footer_area);
}

fn centered_modal(area: Rect, width: u16, height: u16) -> Rect {
    let clamped_w = width.min(area.width);
    let clamped_h = height.min(area.height);

    let x = area.x + (area.width.saturating_sub(clamped_w)) / 2;
    let y = area.y + (area.height.saturating_sub(clamped_h)) / 2;

    Rect::new(x, y, clamped_w, clamped_h)
}

fn render_input_box(frame: &mut Frame, area: Rect, query: &str) {
    let input_line = Line::from(vec![
        Span::styled("🔍 Jump: ", theme::proposal().add_modifier(Modifier::BOLD)),
        Span::styled(query, theme::text().add_modifier(Modifier::BOLD)),
        Span::styled("█", focus_ring()),
    ]);

    let input_widget = Paragraph::new(input_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::muted()),
    );

    frame.render_widget(input_widget, area);
}

fn render_item_list(frame: &mut Frame, area: Rect, items: &[JumpItem], selected_index: usize) {
    if items.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            "  No matching jump targets",
            theme::muted(),
        )));
        frame.render_widget(empty_msg, area);
        return;
    }

    let cursor = selected_index.min(items.len().saturating_sub(1));
    let visible_height = usize::from(area.height);
    let first = cursor.saturating_sub(visible_height.saturating_sub(1));

    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate().skip(first).take(visible_height) {
        let selected = index == cursor;
        let marker = if selected { "▸ " } else { "  " };

        let item_style = if selected {
            focus_ring().add_modifier(Modifier::BOLD)
        } else {
            theme::text()
        };

        let badge_style = match item.kind {
            JumpKind::WorkItem => theme::verified(),
            JumpKind::Surface | JumpKind::Fleet => theme::proposal(),
            JumpKind::Command => theme::approval(),
            JumpKind::File => theme::muted(),
        };

        lines.push(Line::from(vec![
            Span::styled(marker, focus_ring()),
            Span::styled(format!("{} ", item.kind.icon()), theme::text()),
            Span::styled(format!("{:<6} ", item.kind.badge()), badge_style),
            Span::styled(&item.title, item_style),
            Span::styled(" · ", theme::muted()),
            Span::styled(&item.detail, theme::muted()),
        ]));
    }

    let list_widget = Paragraph::new(lines);
    frame.render_widget(list_widget, area);
}

fn render_footer_hint(frame: &mut Frame, area: Rect) {
    let footer_line = Line::from(Span::styled(
        "[Up/Down] Navigate  [Enter] Select  [Esc] Close",
        theme::muted(),
    ));
    frame.render_widget(Paragraph::new(footer_line), area);
}

fn extract_session_files(view: &ViewState) -> Vec<String> {
    let mut files = BTreeSet::new();

    if let Some(ref lb) = view.snapshot.left_branch {
        for p in &lb.files_changed {
            files.insert(p.to_string_lossy().to_string());
        }
        for p in &lb.files_read {
            files.insert(p.to_string_lossy().to_string());
        }
    }

    for entry in view.snapshot.messages.iter() {
        for block in &entry.blocks {
            match block {
                crate::core::message::ContentBlock::ToolResult { result, .. } => {
                    match &result.effect {
                        crate::core::message::ToolEffect::Read { path, .. }
                        | crate::core::message::ToolEffect::Mutation { path, .. } => {
                            files.insert(path.clone());
                        }
                        _ => {}
                    }
                }
                crate::core::message::ContentBlock::ToolCall(tc) => {
                    extract_paths_from_value(&tc.arguments, &mut files);
                }
                _ => {}
            }
        }
    }

    files.into_iter().collect()
}

fn extract_paths_from_value(val: &serde_json::Value, files: &mut BTreeSet<String>) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if (k.contains("path") || k.contains("file") || k.contains("target"))
                    && let serde_json::Value::String(s) = v
                {
                    if !s.is_empty() {
                        files.insert(s.clone());
                    }
                } else {
                    extract_paths_from_value(v, files);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_paths_from_value(item, files);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "AGENTS.md §7: tests may index freely"
    )]

    use super::*;

    #[test]
    fn test_jump_state_navigation() {
        let mut state = JumpState::new();
        assert!(!state.active);

        state.toggle();
        assert!(state.active);

        state.input_char('c');
        state.input_char('m');
        state.input_char('d');
        assert_eq!(state.query, "cmd");

        state.backspace();
        assert_eq!(state.query, "cm");

        state.move_cursor_down(3);
        assert_eq!(state.selected_index, 1);

        state.move_cursor_down(3);
        assert_eq!(state.selected_index, 2);

        state.move_cursor_down(3);
        assert_eq!(state.selected_index, 0);

        state.move_cursor_up(3);
        assert_eq!(state.selected_index, 2);

        state.close();
        assert!(!state.active);
        assert_eq!(state.query, "");
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_build_and_filter_jump_items() {
        let view = ViewState::default();
        let items = build_jump_items(&view);

        assert!(!items.is_empty());

        let help_filtered = filter_jump_items(&items, "/help");
        assert_eq!(help_filtered.len(), 1);
        assert_eq!(help_filtered.first().expect("has item").title, "/help");

        let surface_filtered = filter_jump_items(&items, "Conversation");
        assert!(!surface_filtered.is_empty());
        assert_eq!(
            surface_filtered.first().expect("has item").kind,
            JumpKind::Surface
        );
    }

    #[test]
    fn test_fleet_agents_indexed_in_jump_palette() {
        let mut view = ViewState::default();
        let child = crate::core::event::SessionId::new();
        let run = crate::core::event::RunId::new();

        view.apply(&crate::core::event::SmedEvent::SubagentActivity {
            session: crate::core::event::SessionId::new(),
            run,
            child,
            label: "indexing AST".to_owned(),
        });

        let items = build_jump_items(&view);
        let fleet_items: Vec<_> = items
            .into_iter()
            .filter(|i| i.kind == JumpKind::Fleet)
            .collect();

        assert_eq!(fleet_items.len(), 1);
        assert!(fleet_items[0].title.starts_with("agent:"));
        assert!(fleet_items[0].detail.contains("running"));
        assert!(fleet_items[0].target.starts_with("fleet:"));
    }
}

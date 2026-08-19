//! Changes surface: the file mutation record for this session (Phase UX 3).
//!
//! Shows which files smed actually wrote, in order, with the content hash it
//! recorded for each write and whether a read of that path was on record first.
//!
//! It deliberately does **not** present itself as a diff viewer. A unified diff
//! exists at exactly one moment — the approval gate, where `PendingApproval`
//! carries the review diff that the operator approves against — and is not
//! retained afterwards. The durable record of a mutation is its path and
//! `sha256` ([`ToolEffect::Mutation`]). Rendering that as a diff would mean
//! reconstructing one, and the only text available to reconstruct it from is
//! what the model wrote, which is not evidence of anything (`AGENTS.md` §1.1).
//!
//! [`ToolEffect::Mutation`]: crate::core::message::ToolEffect::Mutation

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::core::message::{ContentBlock, ToolEffect};
use crate::tui::layout::sanitize;
use crate::tui::reducer::ViewState;
use crate::tui::theme;

/// One recorded write to one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutation {
    /// Post-write content hash, exactly as the tool reported it.
    pub sha256: String,
    /// Whether a read of this path was recorded before this write.
    ///
    /// Read-before-edit is the evidence that a write was made against content
    /// smed had actually seen. `false` is not proof of a bad write — a
    /// freshly created file has nothing to read — but it is the thing a
    /// reviewer wants flagged rather than inferred.
    pub read_first: bool,
}

/// Every recorded write to one path, in the order they happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub mutations: Vec<Mutation>,
}

impl ChangedFile {
    /// The hash of the most recent write, which is the file's current recorded
    /// state.
    #[must_use]
    pub fn latest_sha256(&self) -> Option<&str> {
        self.mutations.last().map(|m| m.sha256.as_str())
    }
}

/// Cursor and scroll position over the changed-file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangesSurfaceState {
    pub selected_file: usize,
    pub scroll_y: u16,
}

impl ChangesSurfaceState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected_file: 0,
            scroll_y: 0,
        }
    }

    /// Select the next file in the list.
    pub fn select_next(&mut self, total: usize) {
        if total == 0 {
            self.selected_file = 0;
            self.scroll_y = 0;
            return;
        }
        if self.selected_file + 1 < total {
            self.selected_file += 1;
            self.scroll_y = 0;
        }
    }

    /// Select the previous file in the list.
    pub fn select_prev(&mut self, total: usize) {
        if total == 0 {
            self.selected_file = 0;
            self.scroll_y = 0;
            return;
        }
        if self.selected_file > 0 {
            self.selected_file -= 1;
            self.scroll_y = 0;
        } else if self.selected_file >= total {
            self.selected_file = total.saturating_sub(1);
            self.scroll_y = 0;
        }
    }

    /// Scroll the evidence viewport up by one line.
    pub fn scroll_up(&mut self) {
        self.scroll_y = self.scroll_y.saturating_sub(1);
    }

    /// Scroll the evidence viewport down by one line.
    pub fn scroll_down(&mut self) {
        self.scroll_y = self.scroll_y.saturating_add(1);
    }
}

/// Walks the transcript in order and collects every recorded file mutation.
///
/// Reads are tracked as they are seen so a write can be marked against whether
/// this session had already read that path. Both come from `ToolResult`
/// effects — what the tool reported doing — never from assistant prose.
#[must_use]
pub fn collect_changed_files(view: &ViewState) -> Vec<ChangedFile> {
    let mut read_paths: BTreeSet<&str> = BTreeSet::new();
    let mut files: Vec<ChangedFile> = Vec::new();

    for entry in view.snapshot.messages.iter() {
        for block in &entry.message.blocks {
            let ContentBlock::ToolResult { result, .. } = block else {
                continue;
            };
            match &result.effect {
                ToolEffect::Read { path, .. } => {
                    read_paths.insert(path.as_str());
                }
                ToolEffect::Mutation { path, sha256 } => {
                    let mutation = Mutation {
                        sha256: sha256.clone(),
                        read_first: read_paths.contains(path.as_str()),
                    };
                    match files.iter_mut().find(|file| file.path == *path) {
                        Some(existing) => existing.mutations.push(mutation),
                        None => files.push(ChangedFile {
                            path: path.clone(),
                            mutations: vec![mutation],
                        }),
                    }
                    // A write makes the path known content from here on.
                    read_paths.insert(path.as_str());
                }
                _ => {}
            }
        }
    }

    files
}

/// Renders the two-column Changes surface.
pub fn render_changes_surface(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    state: &ChangesSurfaceState,
) {
    if area.width < 20 || area.height < 4 {
        return;
    }

    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);

    let files = collect_changed_files(view);

    let left_width = 32.min(area.width / 2);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(1)]).areas(main_area);

    render_file_list(frame, left_area, &files, state.selected_file);
    render_evidence(frame, right_area, &files, state);
    render_footer_hint(frame, footer_area);
}

fn render_footer_hint(frame: &mut Frame, area: Rect) {
    let hint = Line::from(vec![
        Span::styled("[Up/Down]", theme::title()),
        Span::styled(" Files  ", theme::muted()),
        Span::styled("[PageUp/PageDown]", theme::title()),
        Span::styled(" Scroll", theme::muted()),
    ]);
    frame.render_widget(Paragraph::new(hint).style(theme::panel()), area);
}

fn render_file_list(frame: &mut Frame, area: Rect, files: &[ChangedFile], selected: usize) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::chrome())
        .style(theme::panel())
        .title(Span::styled(
            format!(" FILES WRITTEN ({}) ", files.len()),
            theme::title().add_modifier(Modifier::BOLD),
        ));

    let lines: Vec<Line<'static>> = if files.is_empty() {
        vec![Line::from(Span::styled("No files written", theme::muted()))]
    } else {
        files
            .iter()
            .enumerate()
            .map(|(index, file)| format_file_entry(file, index == selected))
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines).block(block).style(theme::panel()),
        area,
    );
}

fn format_file_entry(file: &ChangedFile, selected: bool) -> Line<'static> {
    let writes = file.mutations.len();
    let unread = file.mutations.iter().any(|mutation| !mutation.read_first);

    Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            if selected {
                theme::title()
            } else {
                theme::muted()
            },
        ),
        Span::styled(
            sanitize(&file.path),
            if selected {
                theme::text().add_modifier(Modifier::BOLD)
            } else {
                theme::text()
            },
        ),
        Span::styled(
            if writes > 1 {
                format!(" ×{writes}")
            } else {
                String::new()
            },
            theme::muted(),
        ),
        Span::styled(if unread { " !" } else { "" }, theme::approval()),
    ])
}

/// The evidence trail for the selected file.
fn render_evidence(
    frame: &mut Frame,
    area: Rect,
    files: &[ChangedFile],
    state: &ChangesSurfaceState,
) {
    let selected = files
        .get(state.selected_file.min(files.len().saturating_sub(1)))
        .filter(|_| !files.is_empty());

    let (title, lines) = match selected {
        Some(file) => (
            format!(" {} ", sanitize(&file.path)),
            evidence_lines(file, state.scroll_y),
        ),
        None => (" CHANGES ".to_owned(), empty_state_lines()),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::chrome())
        .style(theme::panel())
        .title(Span::styled(
            title,
            theme::title().add_modifier(Modifier::BOLD),
        ));

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(theme::panel())
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn empty_state_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "No file has been written in this session.",
            theme::muted(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Writes appear here once a tool reports one, with the",
            theme::muted(),
        )),
        Line::from(Span::styled("content hash it recorded.", theme::muted())),
    ]
}

fn evidence_lines(file: &ChangedFile, scroll_y: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("writes recorded  ", theme::muted()),
            Span::styled(file.mutations.len().to_string(), theme::text()),
        ]),
        Line::from(vec![
            Span::styled("current sha256   ", theme::muted()),
            Span::styled(
                file.latest_sha256().unwrap_or("none").to_owned(),
                theme::text(),
            ),
        ]),
        Line::from(""),
    ];

    for (index, mutation) in file.mutations.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("#{}  ", index.saturating_add(1)), theme::title()),
            Span::styled(mutation.sha256.clone(), theme::proposal()),
        ]));
        lines.push(if mutation.read_first {
            Line::from(Span::styled(
                "    read recorded before this write",
                theme::verified(),
            ))
        } else {
            Line::from(Span::styled(
                "    no prior read of this path on record",
                theme::approval(),
            ))
        });
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "The reviewable diff is shown at the approval gate; it is",
        theme::muted(),
    )));
    lines.push(Line::from(Span::styled(
        "not retained after the write.",
        theme::muted(),
    )));

    lines.into_iter().skip(usize::from(scroll_y)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::{CanonicalMessage, ToolResult, TranscriptEntry};
    use std::sync::Arc;

    fn result_entry(index: u64, name: &str, effect: ToolEffect) -> TranscriptEntry {
        TranscriptEntry::anchored(
            index,
            CanonicalMessage::tool_result(
                format!("call-{index}"),
                name,
                ToolResult {
                    effect,
                    ..ToolResult::ok("")
                },
            ),
        )
    }

    fn view_with(entries: Vec<TranscriptEntry>) -> ViewState {
        let mut view = ViewState::default();
        view.snapshot.messages = Arc::new(entries);
        view
    }

    #[test]
    fn a_session_with_no_writes_reports_no_files() {
        let view = view_with(Vec::new());
        assert!(collect_changed_files(&view).is_empty());
    }

    /// The surface used to invent three file changes — including a `Cargo.toml`
    /// dependency bump and a deleted `src/legacy_view.rs` — whenever the real
    /// list came back empty.
    #[test]
    fn an_empty_session_invents_no_changes() {
        let view = view_with(Vec::new());
        let files = collect_changed_files(&view);
        assert!(
            !files
                .iter()
                .any(|file| file.path.contains("Cargo.toml") || file.path.contains("legacy_view")),
            "fabricated sample diff returned: {files:?}"
        );
    }

    #[test]
    fn writes_are_collected_in_order_with_their_recorded_hash() {
        let view = view_with(vec![
            result_entry(
                0,
                "write",
                ToolEffect::Mutation {
                    path: "src/a.rs".to_owned(),
                    sha256: "aaa".to_owned(),
                },
            ),
            result_entry(
                1,
                "edit",
                ToolEffect::Mutation {
                    path: "src/a.rs".to_owned(),
                    sha256: "bbb".to_owned(),
                },
            ),
        ]);

        let files = collect_changed_files(&view);
        assert_eq!(files.len(), 1, "same path must collapse to one entry");
        let file = files.first().expect("one file");
        assert_eq!(file.mutations.len(), 2);
        assert_eq!(file.latest_sha256(), Some("bbb"));
    }

    #[test]
    fn a_read_before_a_write_is_recorded_as_such() {
        let view = view_with(vec![
            result_entry(
                0,
                "read",
                ToolEffect::Read {
                    path: "src/a.rs".to_owned(),
                    sha256: "aaa".to_owned(),
                },
            ),
            result_entry(
                1,
                "edit",
                ToolEffect::Mutation {
                    path: "src/a.rs".to_owned(),
                    sha256: "bbb".to_owned(),
                },
            ),
        ]);

        let file = collect_changed_files(&view);
        let first = file.first().expect("one file");
        assert!(
            first.mutations.first().expect("one write").read_first,
            "read-before-edit evidence lost"
        );
    }

    #[test]
    fn a_write_with_no_prior_read_is_flagged_not_assumed() {
        let view = view_with(vec![result_entry(
            0,
            "write",
            ToolEffect::Mutation {
                path: "src/new.rs".to_owned(),
                sha256: "aaa".to_owned(),
            },
        )]);

        let files = collect_changed_files(&view);
        let first = files.first().expect("one file");
        assert!(!first.mutations.first().expect("one write").read_first);
    }

    #[test]
    fn test_changes_surface_state_navigation() {
        let mut state = ChangesSurfaceState::new();
        assert_eq!(state.selected_file, 0);
        assert_eq!(state.scroll_y, 0);

        state.scroll_down();
        assert_eq!(state.scroll_y, 1);
        state.scroll_up();
        assert_eq!(state.scroll_y, 0);
        state.scroll_up();
        assert_eq!(state.scroll_y, 0);

        state.select_next(3);
        assert_eq!(state.selected_file, 1);
        state.select_next(3);
        assert_eq!(state.selected_file, 2);
        state.select_next(3);
        assert_eq!(state.selected_file, 2);

        state.select_prev(3);
        assert_eq!(state.selected_file, 1);
        state.select_prev(3);
        assert_eq!(state.selected_file, 0);
        state.select_prev(3);
        assert_eq!(state.selected_file, 0);
    }
}

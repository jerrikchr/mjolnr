//! Structured Plan Surface component for mjolnr (Phase UX 3).
//!
//! Visualizes structured plan execution steps in a 2-column split layout:
//! - Left column: Step list with state badges ([ ], [▶], [✓], [✗], [⛔])
//! - Right column: Step details (rationale, target files, risk level, telemetry)

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::tui::layout::sanitize;
use crate::tui::reducer::{PlanStep, ViewState};
use crate::tui::theme;
use crate::tui::workspace_types::PlanStepState;

/// Navigation and selection state for the Structured Plan Surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanSurfaceState {
    pub selected_step: usize,
    pub details_expanded: bool,
}

impl PlanSurfaceState {
    /// Create a new plan surface state initialized at the first step.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected_step: 0,
            details_expanded: false,
        }
    }

    /// Move selection cursor up to the previous step.
    pub fn move_cursor_up(&mut self, total: usize) {
        if total == 0 {
            self.selected_step = 0;
            return;
        }
        if self.selected_step > 0 {
            self.selected_step = self.selected_step.saturating_sub(1);
        } else {
            self.selected_step = total.saturating_sub(1);
        }
    }

    /// Move selection cursor down to the next step.
    pub fn move_cursor_down(&mut self, total: usize) {
        if total == 0 {
            self.selected_step = 0;
            return;
        }
        if self.selected_step + 1 < total {
            self.selected_step = self.selected_step.saturating_add(1);
        } else {
            self.selected_step = 0;
        }
    }

    /// Toggle detail expansion panel for the selected step.
    pub fn toggle_details(&mut self) {
        self.details_expanded = !self.details_expanded;
    }
}

/// Returns badge text, human label, and theme style for a `PlanStepState`.
#[must_use]
pub fn badge_info(state: PlanStepState) -> (&'static str, &'static str, Style) {
    match state {
        PlanStepState::Pending => ("[ ]", "Pending", theme::text()),
        PlanStepState::InProgress => ("[▶]", "In Progress", theme::proposal()),
        PlanStepState::Completed => ("[✓]", "Completed", theme::verified()),
        PlanStepState::Failed => ("[✗]", "Failed", theme::refusal()),
        PlanStepState::Refused => ("[⛔]", "Refused", theme::refusal()),
    }
}

/// Infer execution state for a step from step attributes and position.
#[must_use]
pub fn infer_step_state(
    step: &PlanStep,
    index: usize,
    active_index: Option<usize>,
) -> PlanStepState {
    let desc_lower = step.description.to_lowercase();
    if desc_lower.contains("[failed]") || desc_lower.contains("state: failed") {
        PlanStepState::Failed
    } else if desc_lower.contains("[refused]") || desc_lower.contains("state: refused") {
        PlanStepState::Refused
    } else if step.done {
        PlanStepState::Completed
    } else if Some(index) == active_index {
        PlanStepState::InProgress
    } else {
        PlanStepState::Pending
    }
}

/// Extract target file path references from a step description string.
#[must_use]
pub fn extract_target_files(description: &str) -> Vec<String> {
    let mut files = Vec::new();
    for word in description.split_whitespace() {
        let clean = word.trim_matches(|c| {
            c == '`' || c == '\'' || c == '"' || c == '(' || c == ')' || c == ',' || c == ':'
        });
        let path = std::path::Path::new(clean);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if (clean.contains('/')
            || ext.eq_ignore_ascii_case("rs")
            || ext.eq_ignore_ascii_case("md")
            || ext.eq_ignore_ascii_case("json")
            || ext.eq_ignore_ascii_case("toml"))
            && !clean.starts_with("http")
        {
            files.push(clean.to_owned());
        }
    }
    files
}

/// Assess step execution risk based on description keywords.
#[must_use]
pub fn assess_risk_level(description: &str) -> (&'static str, Style) {
    let desc_lower = description.to_lowercase();
    if desc_lower.contains("delete")
        || desc_lower.contains("remove")
        || desc_lower.contains("drop")
        || desc_lower.contains("exec")
        || desc_lower.contains("unsafe")
    {
        ("High (Governed Approval Required)", theme::refusal())
    } else if desc_lower.contains("edit")
        || desc_lower.contains("write")
        || desc_lower.contains("create")
        || desc_lower.contains("modify")
        || desc_lower.contains("implement")
    {
        ("Medium (Governed Side-Effect)", theme::approval())
    } else {
        ("Low (Read-Only / Inspection)", theme::verified())
    }
}

/// Render the Structured Plan Surface component.
pub fn render_plan_surface(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    state: &PlanSurfaceState,
) {
    if area.width < 10 || area.height < 4 {
        return;
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let content_area = main_chunks.first().copied().unwrap_or(area);
    let footer_area = main_chunks.get(1).copied().unwrap_or(area);

    let steps = &view.plan_steps;
    if steps.is_empty() {
        render_empty_state(frame, content_area, view);
        render_footer(frame, footer_area);
        return;
    }

    let selected_idx = state.selected_step.min(steps.len().saturating_sub(1));
    let active_idx = steps.iter().position(|step| !step.done);

    let col_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(content_area);

    let left_col = col_chunks.first().copied().unwrap_or(content_area);
    let right_col = col_chunks.get(1).copied().unwrap_or(content_area);

    render_step_list(frame, left_col, steps, selected_idx, active_idx);

    if let Some(step) = steps.get(selected_idx) {
        let step_state = infer_step_state(step, selected_idx, active_idx);
        render_step_details(
            frame,
            right_col,
            step,
            step_state,
            state.details_expanded,
            selected_idx,
            steps.len(),
        );
    }

    render_footer(frame, footer_area);
}

fn render_empty_state(frame: &mut Frame, area: Rect, view: &ViewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .title(Span::styled(" STRUCTURED PLAN SURFACE ", theme::title()));

    let text = match view.snapshot.plan.as_ref() {
        Some(workflow) => match &workflow.stage {
            crate::core::plan::PlanStage::QuestionPending { question } => vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Interview question pending",
                    theme::proposal(),
                )),
                Line::from(Span::styled(&question.prompt, theme::text())),
                Line::from(Span::styled(
                    "Type an answer in the composer and submit.",
                    theme::muted(),
                )),
            ],
            _ if workflow.prd.is_some() && workflow.council_link.is_none() => vec![
                Line::from(""),
                Line::from(Span::styled("PRD recorded", theme::verified())),
                Line::from(Span::styled(
                    "The advisory council is reviewing it; no action is authorized.",
                    theme::text(),
                )),
            ],
            _ => vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No active structured plan steps available.",
                    theme::muted(),
                )),
                Line::from(Span::styled(
                    "Plans proposed by the model will appear here with step governance telemetry.",
                    theme::text(),
                )),
            ],
        },
        None => vec![
            Line::from(""),
            Line::from(Span::styled(
                "No active structured plan steps available.",
                theme::muted(),
            )),
            Line::from(Span::styled(
                "Use /plan <goal> to start a bounded owner interview.",
                theme::text(),
            )),
        ],
    };

    frame.render_widget(
        Paragraph::new(text).style(theme::panel()).block(block),
        area,
    );
}

fn render_step_list(
    frame: &mut Frame,
    area: Rect,
    steps: &[PlanStep],
    selected_idx: usize,
    active_idx: Option<usize>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::proposal())
        .title(Span::styled(" PLAN STEPS ", theme::title()));

    let mut lines = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let step_state = infer_step_state(step, i, active_idx);
        let (badge, _, badge_style) = badge_info(step_state);

        let is_selected = i == selected_idx;
        let prefix = if is_selected { "▸ " } else { "  " };

        let num_style = if is_selected {
            theme::title()
        } else {
            theme::muted()
        };

        let desc_style = if is_selected {
            theme::text().add_modifier(Modifier::BOLD)
        } else if step.done {
            theme::muted()
        } else {
            theme::text()
        };

        let mut spans = vec![
            Span::styled(
                prefix,
                if is_selected {
                    theme::proposal()
                } else {
                    theme::muted()
                },
            ),
            Span::styled(format!("{badge} "), badge_style),
            Span::styled(format!("{}. ", step.number), num_style),
        ];
        spans.extend(crate::tui::markdown::render_inline(
            &sanitize(&step.description),
            desc_style,
        ));
        lines.push(Line::from(spans));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::panel())
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn render_step_details(
    frame: &mut Frame,
    area: Rect,
    step: &PlanStep,
    step_state: PlanStepState,
    expanded: bool,
    selected_idx: usize,
    total_steps: usize,
) {
    let title_str = format!(" STEP DETAILS ({}/{}) ", selected_idx + 1, total_steps);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::verified())
        .title(Span::styled(title_str, theme::title()));

    let lines = build_detail_lines(step, step_state, expanded);

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::panel())
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn build_detail_lines(
    step: &PlanStep,
    step_state: PlanStepState,
    expanded: bool,
) -> Vec<Line<'static>> {
    let (badge, label, badge_style) = badge_info(step_state);
    let target_files = extract_target_files(&step.description);
    let (risk_label, risk_style) = assess_risk_level(&step.description);

    let mut description = vec![Span::styled("Description: ", theme::muted())];
    description.extend(crate::tui::markdown::render_inline(
        &sanitize(&step.description),
        theme::text().add_modifier(Modifier::BOLD),
    ));
    let mut lines = vec![
        Line::from(description),
        Line::from(vec![
            Span::styled("Status:      ", theme::muted()),
            Span::styled(format!("{badge} {label}"), badge_style),
        ]),
        Line::from(""),
        Line::from(Span::styled("Rationale:", theme::title())),
        Line::from(Span::styled(
            format!(
                "  Step {} resolves target objectives under mjolnr's governed execution policy.",
                step.number
            ),
            theme::text(),
        )),
        Line::from(""),
        Line::from(Span::styled("Target Files:", theme::title())),
    ];

    if target_files.is_empty() {
        lines.push(Line::from(Span::styled("  None specified", theme::muted())));
    } else {
        for file in target_files {
            lines.push(Line::from(vec![
                Span::styled("  • ", theme::proposal()),
                Span::styled(file, theme::text()),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Risk Level:   ", theme::title()),
        Span::styled(risk_label, risk_style),
    ]));

    append_telemetry_lines(&mut lines, expanded);
    lines
}

fn append_telemetry_lines(lines: &mut Vec<Line<'static>>, expanded: bool) {
    lines.push(Line::from(""));
    if expanded {
        lines.push(Line::from(Span::styled(
            "Expanded Inspection Telemetry:",
            theme::proposal(),
        )));
        lines.push(Line::from(Span::styled(
            "  - Deterministic Policy Gate: Active (Fail-closed)",
            theme::muted(),
        )));
        lines.push(Line::from(Span::styled(
            "  - Approval Requirement: Evaluated per side effect",
            theme::muted(),
        )));
        lines.push(Line::from(Span::styled(
            "  - Execution State: Unmodified without explicit verification",
            theme::muted(),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  [Press Enter to toggle detailed telemetry]",
            theme::muted(),
        )));
    }
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let hint = Line::from(vec![
        Span::styled(" [Up/Down] ", theme::proposal()),
        Span::styled("Navigate  ", theme::muted()),
        Span::styled("[Enter] ", theme::proposal()),
        Span::styled("Toggle Details  ", theme::muted()),
        Span::styled("[Space] ", theme::proposal()),
        Span::styled("Select Step", theme::muted()),
    ]);

    frame.render_widget(Paragraph::new(hint).style(theme::panel()), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_initialization_and_toggle() {
        let mut state = PlanSurfaceState::new();
        assert_eq!(state.selected_step, 0);
        assert!(!state.details_expanded);

        state.toggle_details();
        assert!(state.details_expanded);

        state.toggle_details();
        assert!(!state.details_expanded);
    }

    #[test]
    fn cursor_navigation_bounds() {
        let mut state = PlanSurfaceState::new();

        state.move_cursor_down(3);
        assert_eq!(state.selected_step, 1);

        state.move_cursor_down(3);
        assert_eq!(state.selected_step, 2);

        state.move_cursor_down(3);
        assert_eq!(state.selected_step, 0);

        state.move_cursor_up(3);
        assert_eq!(state.selected_step, 2);

        state.move_cursor_up(3);
        assert_eq!(state.selected_step, 1);
    }

    #[test]
    fn empty_total_navigation() {
        let mut state = PlanSurfaceState::new();
        state.selected_step = 5;

        state.move_cursor_down(0);
        assert_eq!(state.selected_step, 0);

        state.selected_step = 5;
        state.move_cursor_up(0);
        assert_eq!(state.selected_step, 0);
    }

    #[test]
    fn badge_info_covers_all_states() {
        assert_eq!(badge_info(PlanStepState::Pending).0, "[ ]");
        assert_eq!(badge_info(PlanStepState::InProgress).0, "[▶]");
        assert_eq!(badge_info(PlanStepState::Completed).0, "[✓]");
        assert_eq!(badge_info(PlanStepState::Failed).0, "[✗]");
        assert_eq!(badge_info(PlanStepState::Refused).0, "[⛔]");
    }

    #[test]
    fn infer_step_state_resolution() {
        let step_completed = PlanStep {
            number: 1,
            description: "Step 1".into(),
            done: true,
        };
        assert_eq!(
            infer_step_state(&step_completed, 0, Some(1)),
            PlanStepState::Completed
        );

        let step_in_progress = PlanStep {
            number: 2,
            description: "Step 2".into(),
            done: false,
        };
        assert_eq!(
            infer_step_state(&step_in_progress, 1, Some(1)),
            PlanStepState::InProgress
        );

        let step_pending = PlanStep {
            number: 3,
            description: "Step 3".into(),
            done: false,
        };
        assert_eq!(
            infer_step_state(&step_pending, 2, Some(1)),
            PlanStepState::Pending
        );

        let step_failed = PlanStep {
            number: 4,
            description: "Step 4 [FAILED]".into(),
            done: false,
        };
        assert_eq!(
            infer_step_state(&step_failed, 3, Some(1)),
            PlanStepState::Failed
        );
    }

    #[test]
    fn extract_target_files_parsing() {
        let desc = "Implement feature in src/tui/plan_surface.rs and docs/tui-design.md";
        let files = extract_target_files(desc);
        assert_eq!(files, vec!["src/tui/plan_surface.rs", "docs/tui-design.md"]);
    }

    #[test]
    fn assess_risk_level_evaluation() {
        let (label_high, _) = assess_risk_level("Delete old files");
        assert!(label_high.contains("High"));

        let (label_med, _) = assess_risk_level("Implement component in src/file.rs");
        assert!(label_med.contains("Medium"));

        let (label_low, _) = assess_risk_level("Inspect status");
        assert!(label_low.contains("Low"));
    }
}

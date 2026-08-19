//! Transcript rendering: provenance, markdown, tool lifecycle, and live lanes.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui_image::Image as RatatuiImage;

use crate::core::message::{
    CanonicalMessage, ContentBlock, Role, ToolCall, ToolEffect, ToolOutcome, ToolResult,
};
use crate::tui::image::{ImageStore, Placement, Slot};
use crate::tui::layout::sanitize;
use crate::tui::reducer::{RunStatus, ViewState};
use crate::tui::theme;

const DETAIL_LINE_LIMIT: usize = 40;
const SUMMARY_LIMIT: usize = 60;
/// Images sit under the same two-column indent as message text.
const IMAGE_INDENT: u16 = 2;
const SPINNER_FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let (lines, placements) = timeline_lines(view, area.width);
    if lines.is_empty() {
        crate::tui::empty::render(frame, area, view);
        if view.cancelling {
            render_cancelling(frame, area);
        }
        return;
    }

    let text_style = if view.cancelling {
        theme::text().add_modifier(Modifier::DIM)
    } else {
        theme::text()
    };
    // Row positions are resolved before the paragraph takes ownership of the
    // lines, and only when an image is actually present — a transcript without
    // one pays nothing for this.
    let image_rows = image_rows(&lines, &placements, area.width);

    let paragraph = Paragraph::new(lines)
        .style(text_style)
        .wrap(Wrap { trim: false });
    let wrapped_height = paragraph.line_count(area.width);
    let previous_height = view.last_timeline_height.replace(wrapped_height);
    let viewport_height = usize::from(area.height);
    let maximum_scroll = wrapped_height.saturating_sub(viewport_height);
    let offset = maximum_scroll
        .saturating_sub(usize::from(view.timeline_scroll_from_bottom))
        .min(usize::from(u16::MAX));
    let scroll = u16::try_from(offset).unwrap_or(u16::MAX);

    frame.render_widget(
        paragraph.scroll((scroll, 0)).block(
            Block::default()
                .borders(Borders::NONE)
                .style(theme::canvas()),
        ),
        area,
    );

    draw_images(frame, area, view, &image_rows, scroll);

    if view.timeline_scroll_from_bottom > 0 && area.width >= 22 && area.height > 0 {
        let new_below = wrapped_height > previous_height && previous_height > 0;
        let label = if new_below {
            " ↓ new below "
        } else {
            " ↑ scrollback · End "
        };
        let width = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
        let badge = Rect::new(
            area.right().saturating_sub(width),
            area.bottom().saturating_sub(1),
            width.min(area.width),
            1,
        );
        frame.render_widget(
            Paragraph::new(label)
                .style(if new_below {
                    theme::approval()
                } else {
                    theme::muted()
                })
                .alignment(Alignment::Right),
            badge,
        );
    }

    if view.cancelling {
        render_cancelling(frame, area);
    }
}

/// Resolve each placement to its wrapped row in the assembled transcript.
///
/// The reserved rows are blank lines, so they never wrap; everything before
/// them can, which is why the row is measured with the same `Paragraph` the
/// renderer uses rather than counted by hand. Measuring per segment keeps this
/// one pass over the transcript, not one pass per image.
fn image_rows(
    lines: &[Line<'static>],
    placements: &[Placement],
    width: u16,
) -> Vec<(usize, Placement)> {
    if placements.is_empty() || width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::with_capacity(placements.len());
    let mut cursor = 0usize;
    let mut row = 0usize;
    for placement in placements {
        let Some(segment) = lines.get(cursor..placement.offset) else {
            continue;
        };
        row = row.saturating_add(
            Paragraph::new(segment.to_vec())
                .wrap(Wrap { trim: false })
                .line_count(width),
        );
        cursor = placement.offset;
        rows.push((row, placement.clone()));
    }
    rows
}

/// Draw every image whose reserved rows are fully inside the viewport.
///
/// Partially scrolled images are skipped rather than clipped: a graphics
/// protocol that half-draws across the viewport edge leaves residue the next
/// frame does not own, and the caption above the gap still says what is there.
fn draw_images(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    rows: &[(usize, Placement)],
    scroll: u16,
) {
    if rows.is_empty() {
        return;
    }
    let Ok(store) = view.images.try_borrow() else {
        return;
    };
    for (row, placement) in rows {
        let Some(top) = row.checked_sub(usize::from(scroll)) else {
            continue;
        };
        let Ok(top) = u16::try_from(top) else {
            continue;
        };
        let height = placement.size.height;
        let width = placement
            .size
            .width
            .min(area.width.saturating_sub(IMAGE_INDENT));
        if width == 0 || height == 0 || top.saturating_add(height) > area.height {
            continue;
        }
        let Some(protocol) = store.protocol(&placement.key) else {
            continue;
        };
        let rect = Rect::new(
            area.x.saturating_add(IMAGE_INDENT),
            area.y.saturating_add(top),
            width,
            height,
        );
        frame.render_widget(RatatuiImage::new(protocol), rect);
    }
}

fn render_cancelling(frame: &mut Frame, area: Rect) {
    let banner_area = crate::tui::layout::centered(area, 40, 5);
    frame.render_widget(ratatui::widgets::Clear, banner_area);
    frame.render_widget(
        Paragraph::new("\n  CANCELLING...  \n  Interrupting active run  \n")
            .style(theme::approval().add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::approval().add_modifier(Modifier::BOLD))
                    .style(theme::panel()),
            ),
        banner_area,
    );
}

fn timeline_lines(view: &ViewState, width: u16) -> (Vec<Line<'static>>, Vec<Placement>) {
    let mut lines = notice_lines(view);
    let mut placements = Vec::new();
    let results = result_map(view);
    let calls = call_ids(view);
    let fallback_model = view
        .snapshot
        .model
        .as_ref()
        .map_or("Assistant", |model| model.as_str());

    // Borrowed for the whole assembly: `resolve` decodes on a miss, and doing
    // that once per message beats re-borrowing per link. Invalidation is
    // `layout::render`'s job — it holds the terminal width, which is the only
    // width every pane agrees on.
    let mut store = view.images.try_borrow_mut().ok();
    let workspace_root = view.snapshot.workspace_root.as_deref();

    if let Ok(mut cache) = view.render_cache.try_borrow_mut() {
        cache.prepare(width, theme::active_theme_id(), view.show_tool_details);
        for message in view.snapshot.messages.iter() {
            for call in message.tool_calls() {
                cache.register_call(&call.id, message.id);
            }
        }
        for call_id in results.keys() {
            cache.note_result(call_id);
        }

        for message in view.snapshot.messages.iter() {
            let running = message
                .tool_calls()
                .any(|call| !results.contains_key(call.id.as_str()))
                && view.snapshot.run_active;
            let base = lines.len();
            if !running && let Some((cached, images)) = cache.get(message.id) {
                lines.extend(cached);
                placements.extend(rebase(images, base));
                continue;
            }
            let (rendered, images) = message_lines(
                message,
                fallback_model,
                &results,
                &calls,
                view.show_tool_details,
                view.snapshot.run_active,
                view.tick,
                usize::from(width),
                &mut Images {
                    store: store.as_deref_mut(),
                    workspace_root,
                },
            );
            if !running {
                cache.insert(message.id, rendered.clone(), images.clone());
            }
            lines.extend(rendered);
            placements.extend(rebase(images, base));
        }
    } else {
        for message in view.snapshot.messages.iter() {
            let base = lines.len();
            let (rendered, images) = message_lines(
                message,
                fallback_model,
                &results,
                &calls,
                view.show_tool_details,
                view.snapshot.run_active,
                view.tick,
                usize::from(width),
                &mut Images {
                    store: store.as_deref_mut(),
                    workspace_root,
                },
            );
            lines.extend(rendered);
            placements.extend(rebase(images, base));
        }
    }

    append_live_lanes(&mut lines, view, fallback_model);
    (lines, placements)
}

/// Message-relative placements become transcript-relative ones. The cache
/// stores the former because only the latter moves when an earlier message
/// grows.
fn rebase(images: Vec<Placement>, base: usize) -> Vec<Placement> {
    images
        .into_iter()
        .map(|placement| Placement {
            offset: placement.offset.saturating_add(base),
            ..placement
        })
        .collect()
}

/// The image machinery `message_lines` needs, as one argument rather than two.
/// `store` is `None` when another borrow holds it — links then render as
/// captions, which is the same honest degradation as an unavailable protocol.
struct Images<'a> {
    store: Option<&'a mut ImageStore>,
    workspace_root: Option<&'a std::path::Path>,
}

fn notice_lines(view: &ViewState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(notice) = view.model_notice.as_deref() {
        lines.push(Line::from(Span::styled(
            sanitize(notice),
            theme::approval(),
        )));
        lines.push(Line::from(""));
    }
    if let Some(report) = view.snapshot.last_reload.as_ref() {
        if let Some(failure) = report.failure.as_deref() {
            lines.push(Line::from(Span::styled(
                sanitize(&format!("RELOAD REFUSED — {failure}")),
                theme::refusal(),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                sanitize(&format!(
                    "reloaded · {} skill(s) · {} template(s)",
                    report.skills, report.prompts
                )),
                theme::verified(),
            )));
            if report.changes.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  nothing changed",
                    theme::muted(),
                )));
            } else {
                lines.extend(report.changes.iter().take(12).map(|change| {
                    Line::from(Span::styled(
                        sanitize(&format!("  {change}")),
                        theme::muted(),
                    ))
                }));
            }
        }
        lines.push(Line::from(""));
    }
    if let Some(report) = view.snapshot.last_extension_load.as_ref() {
        let line = match (&report.loaded_program, report.failure.as_deref()) {
            (Some(program), _) => Some((
                format!("loaded extension `{}` · runs {program}", report.name),
                theme::verified(),
            )),
            (None, Some(failure)) => Some((format!("LOAD REFUSED — {failure}"), theme::refusal())),
            (None, None) => None,
        };
        if let Some((text, style)) = line {
            lines.push(Line::from(Span::styled(sanitize(&text), style)));
            lines.push(Line::from(""));
        }
    }
    lines
}

fn result_map(view: &ViewState) -> HashMap<&str, (&str, &ToolResult)> {
    let mut results = HashMap::new();
    for message in view.snapshot.messages.iter() {
        for block in &message.blocks {
            if let ContentBlock::ToolResult {
                call_id,
                name,
                result,
            } = block
            {
                results.insert(call_id.as_str(), (name.as_str(), result));
            }
        }
    }
    results
}

fn call_ids(view: &ViewState) -> HashSet<&str> {
    view.snapshot
        .messages
        .iter()
        .flat_map(|message| message.tool_calls().map(|call| call.id.as_str()))
        .collect()
}

#[allow(
    clippy::too_many_arguments,
    reason = "all arguments are immutable render context for one message"
)]
fn message_lines(
    message: &CanonicalMessage,
    fallback_model: &str,
    results: &HashMap<&str, (&str, &ToolResult)>,
    calls: &HashSet<&str>,
    show_details: bool,
    run_active: bool,
    tick: u64,
    width: usize,
    images: &mut Images<'_>,
) -> (Vec<Line<'static>>, Vec<Placement>) {
    let mut lines = Vec::new();
    let mut placements = Vec::new();
    let (marker, marker_style) = match message.role {
        Role::User => ("❯ You".to_owned(), theme::user()),
        Role::Assistant => (
            message.model.as_ref().map_or_else(
                || fallback_model.to_owned(),
                |model| model.as_str().to_owned(),
            ),
            theme::assistant(),
        ),
        Role::System => ("◇ System".to_owned(), theme::muted()),
        Role::Tool => ("◇ Tool".to_owned(), theme::muted()),
    };
    lines.push(Line::from(Span::styled(marker, marker_style)));

    let (prose, image_links) = crate::tui::image::extract_links(&message.text());
    if !prose.is_empty() {
        let sanitized = sanitize(&prose);
        if message.role == Role::Assistant {
            lines.extend(crate::tui::markdown::render(
                &sanitized,
                "  ",
                theme::text(),
                width,
            ));
        } else {
            append_indented(&mut lines, &sanitized, "  ", theme::text());
        }
    }
    for link in &image_links {
        append_image(&mut lines, &mut placements, link, images);
    }

    for block in &message.blocks {
        match block {
            ContentBlock::ToolCall(call) => {
                append_tool_call(
                    &mut lines,
                    call,
                    results.get(call.id.as_str()).map(|(_, result)| *result),
                    show_details,
                    run_active,
                    tick,
                );
            }
            ContentBlock::ToolResult {
                call_id,
                name,
                result,
            } if !calls.contains(call_id.as_str()) => {
                append_orphan_result(&mut lines, name, result, show_details);
            }
            ContentBlock::Text { .. }
            | ContentBlock::ImageRef { .. }
            | ContentBlock::ToolResult { .. } => {}
        }
    }
    lines.push(Line::from(""));
    (lines, placements)
}

/// Append an image link: a caption, then either the rows the image will be
/// drawn into or the reason it will not be.
fn append_image(
    lines: &mut Vec<Line<'static>>,
    placements: &mut Vec<Placement>,
    link: &crate::tui::image::Link,
    images: &mut Images<'_>,
) {
    let alt = sanitize(&link.alt);
    let caption = if alt.is_empty() {
        "image".to_owned()
    } else {
        alt
    };
    let slot = images.store.as_deref_mut().map(|store| {
        store.resolve(
            &link.target,
            images.workspace_root,
            Size::new(crate::tui::image::MAX_COLUMNS, crate::tui::image::MAX_ROWS),
        )
    });

    match slot {
        Some(Slot::Ready { key, size }) => {
            lines.push(Line::from(Span::styled(
                format!("  ▣ {caption}"),
                theme::muted(),
            )));
            placements.push(Placement {
                offset: lines.len(),
                key,
                size,
            });
            // Blank rows the graphics protocol draws over. They are real lines
            // in the transcript, so scrolling and `line_count` account for the
            // image exactly as they do for text.
            for _ in 0..size.height {
                lines.push(Line::from(""));
            }
        }
        Some(Slot::Refused(refusal)) => {
            lines.push(Line::from(vec![
                Span::styled(format!("  ▣ {caption}"), theme::muted()),
                Span::styled(format!("  · {}", refusal.detail()), theme::muted()),
            ]));
        }
        None => {
            lines.push(Line::from(Span::styled(
                format!("  ▣ {caption}"),
                theme::muted(),
            )));
        }
    }
}

fn append_tool_call(
    lines: &mut Vec<Line<'static>>,
    call: &ToolCall,
    result: Option<&ToolResult>,
    show_details: bool,
    run_active: bool,
    tick: u64,
) {
    let name = sanitize(&call.name);
    let summary = argument_summary(call);
    let summary = if summary.is_empty() {
        String::new()
    } else {
        format!("  {summary}")
    };
    match result {
        Some(result) => {
            let (glyph, style, suffix) = outcome_display(result);
            lines.push(Line::from(vec![
                Span::styled(format!("  {glyph} {name}"), style),
                Span::styled(summary, theme::muted()),
                Span::styled(suffix, style),
            ]));
            let summarised = append_completion_summary(lines, result);
            if show_details && !summarised {
                append_detail(lines, &result.content);
            }
        }
        None if run_active => {
            lines.push(Line::from(vec![
                Span::styled(format!("  {} {name}", spinner(tick)), theme::proposal()),
                Span::styled(summary, theme::muted()),
            ]));
        }
        None => lines.push(Line::from(vec![
            Span::styled(format!("  ◦ {name}"), theme::muted()),
            Span::styled(summary, theme::muted()),
            Span::styled("  · no outcome recorded", theme::muted()),
        ])),
    }
    if show_details {
        let args = sanitize(&call.arguments.to_string());
        append_indented(lines, &format!("args {args}"), "    ", theme::muted());
    }
}

fn append_orphan_result(
    lines: &mut Vec<Line<'static>>,
    name: &str,
    result: &ToolResult,
    show_details: bool,
) {
    let (glyph, style, suffix) = outcome_display(result);
    lines.push(Line::from(Span::styled(
        format!("  {glyph} {}{suffix}", sanitize(name)),
        style,
    )));
    // A completion reaches this path whenever the result outlives its call in
    // the rendered window — a resumed session, or a scrolled transcript. The
    // answer must survive that, so the same rule applies here.
    let summarised = append_completion_summary(lines, result);
    if show_details && !summarised {
        append_detail(lines, &result.content);
    }
}

fn outcome_display(result: &ToolResult) -> (&'static str, Style, String) {
    match result.outcome {
        ToolOutcome::Ok => ("✓", theme::verified(), effect_suffix(result)),
        ToolOutcome::Refused(code) | ToolOutcome::Failed(code) => {
            ("✗", theme::refusal(), format!("  — {code}"))
        }
    }
}

fn effect_suffix(result: &ToolResult) -> String {
    let mut facts = Vec::new();
    match &result.effect {
        ToolEffect::Mutation { path, .. } => facts.push(format!("wrote {}", sanitize(path))),
        ToolEffect::Command {
            exit_code,
            duration_ms,
            ..
        } => {
            facts.push(format!("{duration_ms}ms"));
            if let Some(code) = exit_code {
                facts.push(format!("exit {code}"));
            }
        }
        ToolEffect::Read { .. }
        | ToolEffect::Completion { .. }
        | ToolEffect::SkillActivated { .. }
        | ToolEffect::None => {}
    }
    if result.truncated {
        facts.push("truncated".to_owned());
    }
    if facts.is_empty() {
        String::new()
    } else {
        format!("  · {}", facts.join(" · "))
    }
}

fn argument_summary(call: &ToolCall) -> String {
    let candidate = ["path", "file", "command"]
        .iter()
        .find_map(|key| call.arguments.get(key).and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .or_else(|| {
            call.arguments
                .get("argv")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
        })
        .or_else(|| first_scalar(&call.arguments))
        .unwrap_or_default();
    bound(&sanitize(&candidate), SUMMARY_LIMIT)
}

fn first_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Array(values) => values.iter().find_map(first_scalar),
        serde_json::Value::Object(values) => values.values().find_map(first_scalar),
        serde_json::Value::Null => None,
    }
}

fn bound(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn append_detail(lines: &mut Vec<Line<'static>>, text: &str) {
    let sanitized = sanitize(text);
    if let Some(diff) = crate::tui::highlight::style_diff(&sanitized) {
        for line in diff.into_iter().take(DETAIL_LINE_LIMIT) {
            let mut spans = vec![Span::raw("    ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    } else {
        append_indented(
            lines,
            &sanitized
                .lines()
                .take(DETAIL_LINE_LIMIT)
                .collect::<Vec<_>>()
                .join("\n"),
            "    ",
            theme::muted(),
        );
    }
    if sanitized.lines().count() > DETAIL_LINE_LIMIT {
        lines.push(Line::from(Span::styled(
            "    … output truncated in view",
            theme::muted(),
        )));
    }
}

fn append_live_lanes(lines: &mut Vec<Line<'static>>, view: &ViewState, model: &str) {
    if !view.streaming_text.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{model}  streaming"),
            theme::proposal(),
        )));
        append_indented(lines, &sanitize(&view.streaming_text), "  ", theme::text());
        lines.push(Line::from(Span::styled(
            "  ▍",
            Style::default().fg(theme::pulsing_proposal(theme::pulse(view.tick, 0.08))),
        )));
    }
    if !view.reasoning_text.is_empty() {
        let dots = ".".repeat(usize::try_from((view.tick / 8) % 3 + 1).unwrap_or(1));
        lines.push(Line::from(Span::styled(
            format!("{model}  thinking{dots}"),
            theme::muted()
                .add_modifier(Modifier::ITALIC)
                .add_modifier(Modifier::DIM),
        )));
        append_indented(
            lines,
            &sanitize(&view.reasoning_text),
            "  ",
            theme::muted()
                .add_modifier(Modifier::ITALIC)
                .add_modifier(Modifier::DIM),
        );
    } else if let Some(elapsed) = view.thought_for {
        lines.push(Line::from(Span::styled(
            format!("thought for {:.1}s", elapsed.as_secs_f32()),
            theme::muted(),
        )));
    }
    if let RunStatus::Failed { code, detail } = &view.status {
        lines.push(Line::from(Span::styled(
            format!("Refused ({code})"),
            theme::refusal(),
        )));
        append_indented(lines, &sanitize(detail), "  ", theme::refusal());
        lines.push(Line::from(""));
    }
}

/// Show the summary a run ended on, whether or not details are expanded.
///
/// `finish_task` returns its summary as the result content (`tools/finish.rs`),
/// and a run can legitimately end with the answer there and nowhere else — a
/// question answered without a single line of assistant prose. Before this, a
/// completion contributed no facts to the outcome row and its content sat
/// behind Ctrl-O, so such a run drew a bare tool row and the user saw no answer.
///
/// Not gated on `show_details`, because a detail is something you open to audit
/// a step, and this is the outcome. Hiding the outcome inside the audit view is
/// how a finished run reads as a silent one.
///
/// Returns whether it rendered, so the caller can skip the ordinary detail body
/// rather than print the same text twice.
fn append_completion_summary(lines: &mut Vec<Line<'static>>, result: &ToolResult) -> bool {
    if !matches!(result.effect, ToolEffect::Completion { .. }) {
        return false;
    }
    let summary = sanitize(&result.content);
    if summary.trim().is_empty() {
        return false;
    }
    append_indented(lines, &summary, "    ", theme::text());
    true
}

fn append_indented(lines: &mut Vec<Line<'static>>, text: &str, prefix: &str, style: Style) {
    lines.extend(
        text.lines()
            .map(|line| Line::from(Span::styled(format!("{prefix}{line}"), style))),
    );
}

fn spinner(tick: u64) -> char {
    let step = usize::try_from(tick / 4).unwrap_or(0);
    SPINNER_FRAMES
        .get(step % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or('⠋')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;
    use serde_json::json;

    fn call() -> ToolCall {
        ToolCall {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: json!({"argv": ["cargo", "test"], "noise": "\u{1b}[2J"}),
            provider_signature: None,
        }
    }

    #[test]
    fn argument_summary_is_bounded_and_sanitized() {
        let mut call = call();
        call.arguments = json!({"path": format!("\u{1b}[2J{}", "x".repeat(100))});
        let summary = argument_summary(&call);
        assert!(!summary.contains('\u{1b}'));
        assert!(summary.chars().count() <= SUMMARY_LIMIT + 1);
    }

    #[test]
    fn running_glyph_only_appears_during_a_run() {
        let mut lines = Vec::new();
        append_tool_call(&mut lines, &call(), None, false, true, 0);
        assert!(
            lines
                .first()
                .expect("running line")
                .to_string()
                .contains('⠋')
        );
        lines.clear();
        append_tool_call(&mut lines, &call(), None, false, false, 0);
        let inactive = lines.first().expect("inactive line").to_string();
        assert!(!inactive.contains('⠋'));
        assert!(inactive.contains("no outcome"));
    }

    #[test]
    fn refusal_displays_the_stable_reason_code() {
        let result = ToolResult::refused(ReasonCode::ApprovalDenied, "denied");
        let (_, _, suffix) = outcome_display(&result);
        assert!(suffix.contains("APPROVAL_DENIED"));
    }
}

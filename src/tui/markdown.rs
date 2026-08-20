//! Markdown rendering for assistant prose.
//!
//! Models answer in markdown whether or not anything asked them to, so a
//! transcript that renders it literally shows `**bold**` and `###` as
//! punctuation the user has to read past. This turns the common subset into
//! styled [`Line`]s.
//!
//! Deliberately a *subset*, hand-rolled rather than a parser dependency: the
//! goal is a readable transcript, not a conforming `CommonMark` renderer. Tables,
//! nested emphasis, reference links, and block quotes render as their source
//! text, which is legible even when it is not pretty.
//!
//! Fenced code blocks are the one construct that suppresses inline parsing —
//! `**` inside a shell snippet is shell syntax, not emphasis.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::{highlight, theme};

/// Render `text` as styled lines, each prefixed with `prefix`.
pub(crate) fn render(
    text: &str,
    prefix: &str,
    base: Style,
    band_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut language: Option<String> = None;
    let mut code = Vec::new();

    for raw in text.lines() {
        let trimmed = raw.trim_end();

        if let Some(fence) = trimmed.trim_start().strip_prefix("```") {
            if language.is_some() || !code.is_empty() {
                append_code_block(
                    &mut lines,
                    language.as_deref(),
                    &code.join("\n"),
                    prefix,
                    band_width,
                );
                language = None;
                code.clear();
            } else {
                language = Some(fence.trim().to_owned());
            }
            continue;
        }

        if language.is_some() {
            code.push(trimmed.to_owned());
            continue;
        }

        lines.push(render_block_line(trimmed, prefix, base));
    }

    // Streaming responses routinely end mid-fence. Treat the remainder as code
    // until the closing fence arrives rather than leaking markdown punctuation.
    if language.is_some() || !code.is_empty() {
        append_code_block(
            &mut lines,
            language.as_deref(),
            &code.join("\n"),
            prefix,
            band_width,
        );
    }

    lines
}

fn append_code_block(
    lines: &mut Vec<Line<'static>>,
    language: Option<&str>,
    body: &str,
    prefix: &str,
    band_width: usize,
) {
    let language = language.filter(|value| !value.is_empty());
    if let Some(label) = language {
        let palette = theme::syntax();
        let width = band_width.saturating_sub(prefix.chars().count());
        let label = format!(" {label} ");
        let fill = " ".repeat(width.saturating_sub(label.chars().count()));
        lines.push(Line::from(vec![
            Span::styled(prefix.to_owned(), Style::default().bg(palette.code_bg)),
            Span::styled(label, theme::muted().bg(palette.code_bg)),
            Span::styled(fill, Style::default().bg(palette.code_bg)),
        ]));
    }
    lines.extend(highlight::highlight_block(
        language,
        body,
        &format!("{prefix}  "),
        band_width,
    ));
}

/// One non-code line: heading, rule, list item, or paragraph text.
fn render_block_line(line: &str, prefix: &str, base: Style) -> Line<'static> {
    let indent = line.len() - line.trim_start().len();
    let body = line.trim_start();

    // A horizontal rule is a separator, so draw one rather than printing `---`.
    if is_thematic_break(body) {
        return Line::from(Span::styled(
            format!("{prefix}{}", "─".repeat(40)),
            theme::muted(),
        ));
    }

    if let Some(heading) = body.strip_prefix('#') {
        let level = 1 + heading.chars().take_while(|c| *c == '#').count();
        let title = heading.trim_start_matches('#').trim_start();
        if !title.is_empty() {
            // Depth is carried by indentation rather than by hash marks, which
            // are noise once the line is already bold.
            let lead = "  ".repeat(level.saturating_sub(1));
            let mut spans = vec![Span::styled(format!("{prefix}{lead}"), base)];
            spans.extend(render_inline(
                title,
                theme::title().add_modifier(Modifier::BOLD),
            ));
            return Line::from(spans);
        }
    }

    if let Some(item) = list_marker(body) {
        let lead = " ".repeat(indent);
        let mut spans = vec![Span::styled(format!("{prefix}{lead}• "), theme::proposal())];
        spans.extend(render_inline(item, base));
        return Line::from(spans);
    }

    let lead = " ".repeat(indent);
    let mut spans = vec![Span::styled(format!("{prefix}{lead}"), base)];
    spans.extend(render_inline(body, base));
    Line::from(spans)
}

/// `---`, `***`, or `___` on a line of its own.
fn is_thematic_break(body: &str) -> bool {
    let mut chars = body.chars().filter(|c| !c.is_whitespace()).peekable();
    let Some(first) = chars.peek().copied() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let run: Vec<char> = chars.collect();
    run.len() >= 3 && run.iter().all(|c| *c == first)
}

/// The content of a bullet or numbered item, if this line is one.
fn list_marker(body: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(marker) {
            return Some(rest);
        }
    }
    // `1. `, `2) ` and friends.
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &body[digits..];
        for marker in [". ", ") "] {
            if let Some(item) = rest.strip_prefix(marker) {
                return Some(item);
            }
        }
    }
    None
}

/// Split inline markup — `**bold**`, `*italic*`, and `` `code` `` — into spans.
///
/// A delimiter with no partner on the same line stays literal, so prose that
/// happens to contain an asterisk is not swallowed up to the end of the line.
///
/// Kept available to sibling TUI widgets so model-authored labels do not grow
/// a second, inconsistent Markdown implementation.
pub(crate) fn render_inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut rest = text;

    while !rest.is_empty() {
        let matched = ["**", "`", "*", "_"].into_iter().find_map(|delimiter| {
            let body = rest.strip_prefix(delimiter)?;
            let end = body.find(delimiter)?;
            // `**` and `__` are bold; a single mark is emphasis. An empty body
            // (`****`) is punctuation, not markup.
            (end > 0).then(|| (delimiter, &body[..end], &body[end + delimiter.len()..]))
        });

        if let Some((delimiter, content, remainder)) = matched {
            if !plain.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut plain), base));
            }
            spans.push(Span::styled(
                content.to_owned(),
                inline_style(delimiter, base),
            ));
            rest = remainder;
        } else {
            // No markup here: consume one character and keep looking. Stepping
            // by `char` rather than by byte keeps multi-byte text intact.
            let mut chars = rest.chars();
            if let Some(next) = chars.next() {
                plain.push(next);
            }
            rest = chars.as_str();
        }
    }

    if !plain.is_empty() {
        spans.push(Span::styled(plain, base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn inline_style(delimiter: &str, base: Style) -> Style {
    match delimiter {
        "**" => base.add_modifier(Modifier::BOLD),
        "`" => theme::verified(),
        _ => base.add_modifier(Modifier::ITALIC),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The visible text of a line, delimiters and all.
    fn flatten(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn rendered(text: &str) -> Vec<String> {
        render(text, "", theme::text(), 80)
            .iter()
            .map(flatten)
            .collect()
    }

    #[test]
    fn emphasis_markers_do_not_survive_into_the_transcript() {
        // The bug this module exists for: `**mjolnr**` shown as punctuation.
        let lines = rendered("**mjolnr** is a *local-first* harness");
        assert_eq!(lines, vec!["mjolnr is a local-first harness"]);
    }

    #[test]
    fn bold_text_is_actually_bold() {
        let lines = render("**mjolnr** rules", "", theme::text(), 80);
        let line = lines.first().expect("one line");
        let bold = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "mjolnr")
            .expect("the bold span survives");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn headings_lose_their_hashes_and_gain_weight() {
        let lines = render("### Core Philosophy", "", theme::text(), 80);
        let line = lines.first().expect("one line");
        assert!(
            !flatten(line).contains('#'),
            "hash marks must not reach the transcript: {}",
            flatten(line)
        );
        assert!(flatten(line).contains("Core Philosophy"));
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn list_items_become_bullets() {
        let lines = rendered("- first\n- second");
        assert_eq!(lines, vec!["• first", "• second"]);
    }

    #[test]
    fn numbered_items_are_recognised_too() {
        let lines = rendered("1. first\n2) second");
        assert_eq!(lines, vec!["• first", "• second"]);
    }

    #[test]
    fn a_horizontal_rule_is_drawn_rather_than_spelled() {
        let lines = rendered("---");
        let line = lines.first().expect("one line");
        assert!(!line.contains('-'), "a rule must not render as dashes");
        assert!(line.contains('─'));
    }

    #[test]
    fn inline_code_keeps_its_content_and_drops_its_backticks() {
        let lines = rendered("run `cargo test` now");
        assert_eq!(lines, vec!["run cargo test now"]);
    }

    #[test]
    fn a_fenced_block_is_not_parsed_as_prose() {
        // `**` inside a shell snippet is shell syntax. Treating it as emphasis
        // would silently rewrite the command the user is being shown.
        let lines = rendered("text\n```sh\nls **/*.rs\n```\nafter");
        assert_eq!(lines.first().map(String::as_str), Some("text"));
        assert!(lines.iter().any(|line| line.contains("ls **/*.rs")));
        assert_eq!(lines.last().map(String::as_str), Some("after"));
    }

    #[test]
    fn an_unpaired_delimiter_stays_literal() {
        // Prose legitimately contains asterisks and underscores. Swallowing to
        // end-of-line would mangle it.
        let lines = rendered("2 * 3 is six and file_name stays");
        assert_eq!(lines, vec!["2 * 3 is six and file_name stays"]);
    }

    #[test]
    fn plain_prose_is_left_exactly_alone() {
        let lines = rendered("Just a sentence.");
        assert_eq!(lines, vec!["Just a sentence."]);
    }

    #[test]
    fn every_line_carries_the_prefix() {
        // The transcript indents message bodies under their role marker.
        let lines: Vec<String> = render("one\ntwo", "  ", theme::text(), 80)
            .iter()
            .map(flatten)
            .collect();
        assert!(
            lines.iter().all(|line| line.starts_with("  ")),
            "indentation must survive: {lines:?}"
        );
    }

    #[test]
    fn fenced_language_is_shown_and_body_is_highlighted() {
        let lines = render(
            "```rust\nlet answer = \"yes\";\n```",
            "  ",
            theme::text(),
            60,
        );
        assert!(flatten(lines.first().expect("language row")).contains("rust"));
        let code = lines.get(1).expect("code row");
        let colours = code
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect::<std::collections::HashSet<_>>();
        assert!(colours.len() > 1, "keyword and string need distinct roles");
    }

    #[test]
    fn unterminated_fence_stays_a_code_band() {
        let lines = render("```rust\nlet answer = 42;", "", theme::text(), 40);
        let band = theme::syntax().code_bg;
        assert!(
            lines
                .iter()
                .all(|line| line.spans.iter().all(|span| span.style.bg == Some(band)))
        );
    }
}

//! Syntax highlighting for transcript code and diffs (Phase 20).
//!
//! `syntect` supplies the grammars; the colours are mjolnr's own, derived from
//! the active theme's semantic roles (`theme::syntax`) so a `/theme` switch
//! re-skins highlighted code for free. The heavy `SyntaxSet` is loaded once
//! behind a `OnceLock` — immutable after init, so this is not the mutable
//! global state AGENTS.md §2.3 forbids (recorded in the report).
//!
//! Everything here is fail-closed: an unknown language renders as plain text
//! on the code band, a parse error renders the line unstyled, and text that
//! is not provably a unified diff is returned unstyled (`None`).

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::tui::theme::{self, SyntaxPalette};

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Convert a ratatui colour into syntect's RGBA. Quantized themes hand us
/// `Color::Indexed`, so the standard xterm cube math is needed for the
/// round trip.
fn to_syntect(color: Color) -> SyntectColor {
    let (r, g, b) = match theme::rgb_components(color) {
        Some(rgb) => rgb,
        None => match color {
            Color::Indexed(idx) => indexed_to_rgb(idx),
            _ => {
                return SyntectColor {
                    r: 236,
                    g: 236,
                    b: 229,
                    a: 0xff,
                };
            }
        },
    };
    SyntectColor { r, g, b, a: 0xff }
}

fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    // The quantizer in theme.rs only emits the 16..=231 cube; the 16 ANSI
    // bases and the grayscale ramp are covered so any indexed input resolves.
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let cube = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    match idx {
        0..=15 => ANSI
            .get(usize::from(idx))
            .copied()
            .unwrap_or((236, 236, 229)),
        16..=231 => {
            let i = idx - 16;
            (cube(i / 36), cube((i % 36) / 6), cube(i % 6))
        }
        _ => {
            let v = 8 + 10 * (idx - 232);
            (v, v, v)
        }
    }
}

fn selector(text: &str) -> ScopeSelectors {
    // Every selector here is a compile-time constant; a parse failure would be
    // a syntect regression, and an empty selector (matches nothing) is the
    // fail-closed answer to it.
    text.parse().unwrap_or_default()
}

fn item(scope: &str, foreground: Color, font_style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: selector(scope),
        style: StyleModifier {
            foreground: Some(to_syntect(foreground)),
            background: None,
            font_style,
        },
    }
}

/// Build a syntect theme from the active mjolnr palette. Scope coverage is the
/// classic minimal editor set; anything unscoped falls to `settings.foreground`.
fn syntect_theme() -> Theme {
    let palette: SyntaxPalette = theme::syntax();
    let active = theme::active_theme();
    let bold = Some(FontStyle::BOLD);
    let italic = Some(FontStyle::ITALIC);
    let settings = ThemeSettings {
        foreground: Some(to_syntect(palette.text)),
        background: Some(to_syntect(palette.code_bg)),
        ..ThemeSettings::default()
    };
    Theme {
        name: Some("mjolnr".to_owned()),
        author: Some("mjolnr".to_owned()),
        settings,
        scopes: vec![
            item("comment", palette.comment, italic),
            item("string", palette.string, None),
            item("constant", palette.number, None),
            item("keyword, storage, variable.language", palette.keyword, bold),
            item("keyword.operator", palette.operator, None),
            item(
                "entity.name.function, support.function, meta.function-call",
                palette.function,
                None,
            ),
            item(
                "entity.name.type, entity.name.class, support.type, support.class",
                palette.type_name,
                None,
            ),
            item("entity.name.tag", palette.keyword, None),
            item("entity.name.attribute", palette.function, None),
            item("markup.heading", palette.keyword, bold),
            item("markup.inserted", active.verified, None),
            item("markup.deleted", active.refusal, None),
            item("markup.changed", active.approval, None),
            item("punctuation", palette.punctuation, None),
            item("invalid.illegal, invalid", active.refusal, None),
        ],
    }
}

/// Highlight one fenced code block into ratatui lines.
///
/// Every line sits on the `code_bg` band from column zero: the `prefix`
/// indent and the right-hand padding are painted with the band colour so the
/// block reads as one raised surface. `band_width` is the timeline's content
/// width; padding is estimated in `char`s, so a line full of wide glyphs may
/// wrap instead of padding — cosmetic only.
pub(crate) fn highlight_block(
    language: Option<&str>,
    body: &str,
    prefix: &str,
    band_width: usize,
) -> Vec<Line<'static>> {
    let palette = theme::syntax();
    let band = Style::default().bg(palette.code_bg);
    let syntax = language
        .and_then(|token| syntaxes().find_syntax_by_token(token))
        .unwrap_or_else(|| syntaxes().find_syntax_plain_text());
    let synt_theme = syntect_theme();
    let mut highlighter = HighlightLines::new(syntax, &synt_theme);

    body.lines()
        .map(|source| {
            let mut spans = vec![Span::styled(prefix.to_owned(), band)];
            let ranges = highlighter
                .highlight_line(source, syntaxes())
                .unwrap_or_else(|_| {
                    vec![(
                        syntect::highlighting::Style {
                            foreground: to_syntect(palette.text),
                            background: to_syntect(palette.code_bg),
                            font_style: FontStyle::empty(),
                        },
                        source,
                    )]
                });
            for (style, text) in ranges {
                let mut ratatui_style = Style::default()
                    .fg(theme::syntax_rgb(
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                    ))
                    .bg(palette.code_bg);
                if style.font_style.contains(FontStyle::BOLD) {
                    ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
                }
                if style.font_style.contains(FontStyle::ITALIC) {
                    ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
                }
                spans.push(Span::styled(text.to_owned(), ratatui_style));
            }
            let used = prefix.chars().count() + source.chars().count();
            if used < band_width {
                spans.push(Span::styled(" ".repeat(band_width - used), band));
            }
            Line::from(spans)
        })
        .collect()
}

/// The reference for a fence token, exposed so markdown can ask "is this a
/// language syntect knows?" without committing to highlight.
#[allow(dead_code)]
pub(crate) fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    syntaxes().find_syntax_by_token(language)
}

/// True when `text` is provably a unified diff: a `---`/`+++` file header
/// pair *and* an `@@` hunk header. Anything less certain is not coloured —
/// prose that happens to start a line with `-` must survive untouched.
fn is_diff(text: &str) -> bool {
    let mut saw_old_header = false;
    let mut has_header_pair = false;
    let mut has_hunk = false;
    for line in text.lines() {
        if line.starts_with("--- ") {
            saw_old_header = true;
        } else if saw_old_header && line.starts_with("+++ ") {
            has_header_pair = true;
        } else if line.starts_with("@@") {
            has_hunk = true;
        }
    }
    has_header_pair && has_hunk
}

/// Colour a unified diff: file headers and context in muted, hunks in
/// proposal, additions verified, removals refusal. `None` when the text is
/// not diff-shaped, so callers fall back to their default styling.
pub(crate) fn style_diff(text: &str) -> Option<Vec<Line<'static>>> {
    if !is_diff(text) {
        return None;
    }
    let lines = text
        .lines()
        .map(|line| {
            let style = if line.starts_with("+++ ") || line.starts_with("--- ") {
                theme::muted().add_modifier(Modifier::BOLD)
            } else if line.starts_with("@@") {
                theme::proposal()
            } else if line.starts_with('+') {
                theme::verified()
            } else if line.starts_with('-') {
                theme::refusal()
            } else {
                theme::muted()
            };
            Line::from(Span::styled(line.to_owned(), style))
        })
        .collect();
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str =
        "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n context";

    #[test]
    fn a_real_diff_is_coloured_line_by_line() {
        let lines = style_diff(SAMPLE_DIFF).expect("a review diff is detected");
        assert_eq!(lines.len(), 6);
        let added = lines.get(4).expect("the +new line exists");
        assert_eq!(
            added.spans.first().and_then(|s| s.style.fg),
            theme::verified().fg
        );
        let removed = lines.get(3).expect("the -old line exists");
        assert_eq!(
            removed.spans.first().and_then(|s| s.style.fg),
            theme::refusal().fg
        );
    }

    #[test]
    fn prose_with_dashes_is_not_a_diff() {
        // The negative test the guard exists for: a markdown list must never
        // pick up refusal colouring.
        assert!(style_diff("- first\n- second\n- third").is_none());
        assert!(style_diff("---\njust a rule\n---").is_none());
        assert!(style_diff("@@ looks like a hunk but has no header").is_none());
    }

    #[test]
    fn known_languages_highlight_to_distinct_roles() {
        let lines = highlight_block(Some("rust"), "let x = 42;", "", 40);
        let line = lines.first().expect("one highlighted line");
        // `let` (keyword) and `42` (number) must not share a colour.
        let keyword = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "let")
            .expect("the keyword span exists");
        let number = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "42")
            .expect("the number span exists");
        assert_ne!(keyword.style.fg, number.style.fg);
    }

    #[test]
    fn unknown_languages_fall_back_to_plain_band() {
        let lines = highlight_block(Some("not-a-language"), "hello", "  ", 20);
        let line = lines.first().expect("the fallback line");
        let band = theme::syntax().code_bg;
        assert!(
            line.spans.iter().all(|span| span.style.bg == Some(band)),
            "every span sits on the code band"
        );
    }

    #[test]
    fn the_band_pads_to_the_timeline_width() {
        let lines = highlight_block(None, "fn", "", 30);
        let line = lines.first().expect("one line");
        let width: usize = line
            .spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum();
        assert_eq!(width, 30);
    }
}

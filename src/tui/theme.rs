//! smed's theme contract and semantic palettes.
//!
//! Colour communicates governance state: sky/cyan proposes, citron/amber asks,
//! green confirms, and red/magenta refuses. Render code asks for meaning
//! rather than scattering literal colours across widgets.

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicU8, Ordering};

/// Available terminal colour depths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ColorDepth {
    #[default]
    TrueColor,
    Color256,
    Color16,
}

impl ColorDepth {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::TrueColor => "truecolor (24-bit)",
            Self::Color256 => "256-colour (8-bit)",
            Self::Color16 => "16-colour (4-bit)",
        }
    }
}

/// Identifiers for shipped themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ThemeId {
    Zeppi,
    ZeppiLight,
    /// Cyber-noir is the default (Phase 20): the owner's stated house
    /// aesthetic, and the theme the gradient wordmark was designed for.
    #[default]
    Noir,
    Mono,
}

impl ThemeId {
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Zeppi => "zeppi",
            Self::ZeppiLight => "zeppi-light",
            Self::Noir => "noir",
            Self::Mono => "mono",
        }
    }

    #[must_use]
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Zeppi => "Zeppi (Dark Default)",
            Self::ZeppiLight => "Zeppi Light",
            Self::Noir => "Cyber Noir",
            Self::Mono => "Monochrome",
        }
    }

    #[must_use]
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "zeppi" | "default" => Some(Self::Zeppi),
            "zeppi-light" | "light" => Some(Self::ZeppiLight),
            "noir" | "cyber-noir" => Some(Self::Noir),
            "mono" | "monochrome" => Some(Self::Mono),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) const fn all() -> &'static [Self] {
        &[Self::Zeppi, Self::ZeppiLight, Self::Noir, Self::Mono]
    }
}

/// A complete semantic theme definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    pub(crate) id: ThemeId,
    pub(crate) canvas: Color,
    pub(crate) panel: Color,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) proposal: Color,
    pub(crate) approval: Color,
    pub(crate) verified: Color,
    pub(crate) refusal: Color,
    pub(crate) has_gradient_wordmark: bool,
    pub(crate) is_mono: bool,
}

impl Theme {
    #[must_use]
    pub(crate) const fn name(&self) -> &'static str {
        self.id.name()
    }

    #[must_use]
    pub(crate) const fn display_name(&self) -> &'static str {
        self.id.display_name()
    }

    #[must_use]
    pub(crate) fn for_id(id: ThemeId) -> Self {
        match id {
            ThemeId::Zeppi => Self {
                id: ThemeId::Zeppi,
                canvas: Color::Rgb(11, 21, 38), // #0B1526 Deepest navy
                panel: Color::Rgb(19, 35, 61),  // #13233D Zeppi navy
                text: Color::Rgb(236, 236, 229), // #ECECE5 Bone
                muted: Color::Rgb(107, 128, 168), // #6B80A8 Muted blue
                proposal: Color::Rgb(98, 153, 208), // #6299D0 Sky
                approval: Color::Rgb(209, 216, 113), // #D1D871 Citron
                verified: Color::Rgb(138, 226, 138), // #8AE28A Green
                refusal: Color::Rgb(226, 138, 138), // #E28A8A Red
                has_gradient_wordmark: false,
                is_mono: false,
            },
            ThemeId::ZeppiLight => Self {
                id: ThemeId::ZeppiLight,
                canvas: Color::Rgb(244, 245, 232),  // Bone ground
                panel: Color::Rgb(228, 230, 210),   // Light panel
                text: Color::Rgb(11, 21, 38),       // Deep navy text
                muted: Color::Rgb(70, 90, 130),     // Dark muted blue
                proposal: Color::Rgb(30, 95, 160),  // Dark sky blue
                approval: Color::Rgb(120, 128, 20), // Dark citron/olive
                verified: Color::Rgb(35, 125, 35),  // Dark green
                refusal: Color::Rgb(175, 45, 45),   // Dark red
                has_gradient_wordmark: false,
                is_mono: false,
            },
            ThemeId::Noir => Self {
                id: ThemeId::Noir,
                canvas: Color::Rgb(5, 9, 15),
                panel: Color::Rgb(10, 18, 29),
                text: Color::Rgb(207, 224, 222),
                muted: Color::Rgb(91, 112, 126),
                proposal: Color::Rgb(64, 220, 255),  // Cyan
                approval: Color::Rgb(255, 176, 32),  // Amber
                verified: Color::Rgb(105, 255, 157), // Phosphor
                refusal: Color::Rgb(255, 72, 137),   // Magenta
                has_gradient_wordmark: true,
                is_mono: false,
            },
            ThemeId::Mono => Self {
                id: ThemeId::Mono,
                canvas: Color::Rgb(0, 0, 0),
                panel: Color::Rgb(30, 30, 30),
                text: Color::Rgb(255, 255, 255),
                muted: Color::Rgb(150, 150, 150),
                proposal: Color::Rgb(255, 255, 255),
                approval: Color::Rgb(255, 255, 255),
                verified: Color::Rgb(255, 255, 255),
                refusal: Color::Rgb(255, 255, 255),
                has_gradient_wordmark: false,
                is_mono: true,
            },
        }
    }

    #[must_use]
    pub(crate) fn quantized(self, depth: ColorDepth) -> Self {
        match depth {
            ColorDepth::TrueColor => self,
            ColorDepth::Color256 => Self {
                canvas: quantize_256(self.canvas),
                panel: quantize_256(self.panel),
                text: quantize_256(self.text),
                muted: quantize_256(self.muted),
                proposal: quantize_256(self.proposal),
                approval: quantize_256(self.approval),
                verified: quantize_256(self.verified),
                refusal: quantize_256(self.refusal),
                ..self
            },
            ColorDepth::Color16 => Self {
                canvas: quantize_16(self.canvas),
                panel: quantize_16(self.panel),
                text: quantize_16(self.text),
                muted: quantize_16(self.muted),
                proposal: quantize_16(self.proposal),
                approval: quantize_16(self.approval),
                verified: quantize_16(self.verified),
                refusal: quantize_16(self.refusal),
                ..self
            },
        }
    }
}

fn quantize_256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    // Map RGB to closest 256-colour index
    #[expect(
        clippy::cast_possible_truncation,
        reason = "RGB channel index calculation is bounded to 0..=5"
    )]
    let r_idx = (u16::from(r) * 5 / 255) as u8;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "RGB channel index calculation is bounded to 0..=5"
    )]
    let g_idx = (u16::from(g) * 5 / 255) as u8;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "RGB channel index calculation is bounded to 0..=5"
    )]
    let b_idx = (u16::from(b) * 5 / 255) as u8;
    let idx = 16 + 36 * r_idx + 6 * g_idx + b_idx;
    Color::Indexed(idx)
}

#[expect(
    clippy::cognitive_complexity,
    reason = "16-colour ANSI mapping evaluates RGB ranges directly"
)]
fn quantize_16(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let intensity = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    if intensity < 30 {
        Color::Black
    } else if intensity > 220 {
        Color::White
    } else if r > g && r > b {
        if intensity > 140 {
            Color::LightRed
        } else {
            Color::Red
        }
    } else if g > r && g > b {
        if intensity > 140 {
            Color::LightGreen
        } else {
            Color::Green
        }
    } else if b > r && b > g {
        if intensity > 140 {
            Color::LightBlue
        } else {
            Color::Blue
        }
    } else if r > 100 && g > 100 && b < 80 {
        if intensity > 140 {
            Color::LightYellow
        } else {
            Color::Yellow
        }
    } else if r > 100 && b > 100 && g < 80 {
        if intensity > 140 {
            Color::LightMagenta
        } else {
            Color::Magenta
        }
    } else if g > 100 && b > 100 && r < 80 {
        if intensity > 140 {
            Color::LightCyan
        } else {
            Color::Cyan
        }
    } else if intensity > 120 {
        Color::Gray
    } else {
        Color::DarkGray
    }
}

static ACTIVE_THEME_ID: AtomicU8 = AtomicU8::new(2); // 2 = Noir (default)
static DETECTED_COLOR_DEPTH: AtomicU8 = AtomicU8::new(0); // 0 = TrueColor

pub(crate) fn active_theme_id() -> ThemeId {
    match ACTIVE_THEME_ID.load(Ordering::Relaxed) {
        0 => ThemeId::Zeppi,
        1 => ThemeId::ZeppiLight,
        3 => ThemeId::Mono,
        _ => ThemeId::Noir,
    }
}

pub(crate) fn set_active_theme_id(id: ThemeId) {
    let val = match id {
        ThemeId::Zeppi => 0,
        ThemeId::ZeppiLight => 1,
        ThemeId::Noir => 2,
        ThemeId::Mono => 3,
    };
    ACTIVE_THEME_ID.store(val, Ordering::Relaxed);
}

/// The shipped themes as `(name, display_name)` pairs, for a preference picker.
/// A narrow public surface so the composition root can offer a theme step in the
/// onboarding flow without that flow importing `tui` (AGENTS.md §2.1).
#[must_use]
pub fn preference_options() -> Vec<(String, String)> {
    ThemeId::all()
        .iter()
        .map(|id| (id.name().to_owned(), id.display_name().to_owned()))
        .collect()
}

/// The name of the theme the process is currently rendering with.
#[must_use]
pub fn active_preference_name() -> String {
    active_theme_id().name().to_owned()
}

/// Persist a theme preference by name to the owner-scoped config file the TUI
/// reads at startup, and apply it to the running process. Returns whether the
/// name was a shipped theme; the write itself is best-effort. Public so the
/// onboarding flow's theme step can persist a choice through the composition
/// root rather than reaching into `tui` directly.
#[must_use]
pub fn persist_preference(name: &str) -> bool {
    use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_native_strategy};
    let Some(id) = ThemeId::parse(name) else {
        return false;
    };
    set_active_theme_id(id);
    if let Ok(strategy) = choose_native_strategy(AppStrategyArgs {
        top_level_domain: String::new(),
        author: String::new(),
        app_name: "smed".to_owned(),
    }) {
        let path = strategy.config_dir().join("theme");
        let parent_ok = path
            .parent()
            .is_none_or(|parent| std::fs::create_dir_all(parent).is_ok());
        if parent_ok {
            let _ = std::fs::write(path, id.name());
        }
    }
    true
}

pub(crate) fn detected_color_depth() -> ColorDepth {
    match DETECTED_COLOR_DEPTH.load(Ordering::Relaxed) {
        1 => ColorDepth::Color256,
        2 => ColorDepth::Color16,
        _ => ColorDepth::TrueColor,
    }
}

pub(crate) fn set_detected_color_depth(depth: ColorDepth) {
    let val = match depth {
        ColorDepth::TrueColor => 0,
        ColorDepth::Color256 => 1,
        ColorDepth::Color16 => 2,
    };
    DETECTED_COLOR_DEPTH.store(val, Ordering::Relaxed);
}

pub(crate) fn active_theme() -> Theme {
    Theme::for_id(active_theme_id()).quantized(detected_color_depth())
}

pub(crate) fn canvas() -> Style {
    let t = active_theme();
    Style::default().fg(t.text).bg(t.canvas)
}

pub(crate) fn panel() -> Style {
    let t = active_theme();
    Style::default().fg(t.text).bg(t.panel)
}

pub(crate) fn chrome() -> Style {
    let t = active_theme();
    Style::default().fg(t.muted).bg(t.canvas)
}

pub(crate) fn focus_ring() -> Style {
    let t = active_theme();
    Style::default()
        .fg(t.proposal)
        .bg(t.panel)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn title() -> Style {
    let t = active_theme();
    Style::default()
        .fg(t.proposal)
        .bg(t.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn user() -> Style {
    let t = active_theme();
    Style::default()
        .fg(t.proposal)
        .bg(t.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn assistant() -> Style {
    let t = active_theme();
    Style::default()
        .fg(t.verified)
        .bg(t.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn text() -> Style {
    let t = active_theme();
    Style::default().fg(t.text).bg(t.canvas)
}

pub(crate) fn muted() -> Style {
    let t = active_theme();
    Style::default().fg(t.muted).bg(t.canvas)
}

pub(crate) fn proposal() -> Style {
    let t = active_theme();
    Style::default().fg(t.proposal).bg(t.canvas)
}

pub(crate) fn verified() -> Style {
    let t = active_theme();
    Style::default().fg(t.verified).bg(t.canvas)
}

pub(crate) fn approval() -> Style {
    let t = active_theme();
    Style::default().fg(t.approval).bg(t.canvas)
}

pub(crate) fn refusal() -> Style {
    let t = active_theme();
    Style::default().fg(t.refusal).bg(t.canvas)
}

pub(crate) fn full_auto() -> Style {
    refusal().add_modifier(Modifier::BOLD)
}

pub(crate) fn quota_style(used_fraction: f32) -> Style {
    if used_fraction >= 0.95 {
        refusal()
    } else if used_fraction >= 0.80 {
        approval()
    } else {
        verified()
    }
}

pub(crate) fn wordmark_gradient(position: f32) -> Color {
    let t = active_theme();
    if !t.has_gradient_wordmark {
        return t.muted;
    }
    let (cyan, phosphor, magenta) = (
        Theme::for_id(ThemeId::Noir).proposal,
        Theme::for_id(ThemeId::Noir).verified,
        Theme::for_id(ThemeId::Noir).refusal,
    );
    if position <= 0.5 {
        lerp(cyan, phosphor, position * 2.0)
    } else {
        lerp(phosphor, magenta, (position - 0.5) * 2.0)
    }
}

/// A slow triangular pulse. `speed` is cycles per animation tick.
pub(crate) fn pulse(tick: u64, speed: f32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "animation ticks wrap before visible precision matters"
    )]
    let phase = (tick as f32 * speed) % 2.0;
    if phase <= 1.0 { phase } else { 2.0 - phase }
}

pub(crate) fn dimmed_approval(brightness: f32) -> Color {
    let t = active_theme();
    lerp(
        t.canvas,
        t.approval,
        0.35 + brightness.clamp(0.0, 1.0) * 0.65,
    )
}

/// The streaming caret's pulse: proposal hue surfacing out of the canvas.
pub(crate) fn pulsing_proposal(brightness: f32) -> Color {
    let t = active_theme();
    lerp(
        t.canvas,
        t.proposal,
        0.35 + brightness.clamp(0.0, 1.0) * 0.65,
    )
}

/// Syntax-highlighting colours, *derived* from the theme's semantic roles
/// rather than declared as literals — a reskin re-derives its code colours
/// automatically, and quantisation to the terminal's colour depth comes free
/// because the base roles are already quantised (Phase 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxPalette {
    pub(crate) keyword: Color,
    pub(crate) string: Color,
    pub(crate) comment: Color,
    pub(crate) number: Color,
    pub(crate) function: Color,
    pub(crate) type_name: Color,
    pub(crate) operator: Color,
    pub(crate) punctuation: Color,
    pub(crate) text: Color,
    pub(crate) code_bg: Color,
}

impl SyntaxPalette {
    fn for_theme(t: &Theme) -> Self {
        if t.is_mono {
            // Monochrome separates by weight (bold/dim), never by hue.
            return Self {
                keyword: t.text,
                string: t.text,
                comment: t.muted,
                number: t.text,
                function: t.text,
                type_name: t.text,
                operator: t.text,
                punctuation: t.muted,
                text: t.text,
                code_bg: t.panel,
            };
        }
        Self {
            keyword: t.proposal,
            string: t.verified,
            // Comments must stay legible: muted brightened a third of the way
            // toward text, or they sink below the WCAG floor on `panel`.
            comment: lerp(t.muted, t.text, 0.35),
            number: t.approval,
            function: lerp(t.proposal, t.text, 0.3),
            type_name: lerp(t.verified, t.proposal, 0.5),
            operator: t.text,
            punctuation: t.muted,
            text: t.text,
            code_bg: t.panel,
        }
    }
}

pub(crate) fn syntax() -> SyntaxPalette {
    SyntaxPalette::for_theme(&active_theme())
}

/// Convert a syntax-engine RGB token back through smed's detected terminal
/// depth. Keeping this constructor here preserves the "no colour literals in
/// widgets" contract and prevents truecolor from leaking onto 16/256 terminals.
pub(crate) fn syntax_rgb(r: u8, g: u8, b: u8) -> Color {
    let color = Color::Rgb(r, g, b);
    match detected_color_depth() {
        ColorDepth::TrueColor => color,
        ColorDepth::Color256 => quantize_256(color),
        ColorDepth::Color16 => quantize_16(color),
    }
}

pub(crate) const fn rgb_components(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// What the environment claims to support, detected once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnvColorDepth {
    pub(crate) depth: ColorDepth,
    pub(crate) force_mono: bool,
}

/// Pure detection over env values (kept string-in/string-out for tests).
///
/// `NO_COLOR` presence wins outright (<https://no-color.org>). Otherwise:
/// `COLORTERM=truecolor|24bit` → truecolor; a `TERM` advertising `256color`
/// → 256-colour; `dumb`/`linux` consoles → 16-colour; anything else keeps the
/// historical truecolor default rather than degrading a modern terminal.
pub(crate) fn detect_color_depth(
    no_color: bool,
    colorterm: Option<&str>,
    term: Option<&str>,
) -> EnvColorDepth {
    if no_color {
        return EnvColorDepth {
            depth: ColorDepth::Color16,
            force_mono: true,
        };
    }
    let depth = match colorterm.map(str::to_ascii_lowercase) {
        Some(value) if value == "truecolor" || value == "24bit" => ColorDepth::TrueColor,
        _ => match term.map(str::to_ascii_lowercase) {
            Some(value) if value.contains("256color") => ColorDepth::Color256,
            Some(value) if value == "dumb" || value == "linux" => ColorDepth::Color16,
            _ => ColorDepth::TrueColor,
        },
    };
    EnvColorDepth {
        depth,
        force_mono: false,
    }
}

fn lerp(from: Color, to: Color, amount: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        return to;
    };
    let amount = amount.clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "interpolated channels are clamped to the u8 range"
    )]
    let channel = |start: u8, end: u8| {
        let value = f32::from(start) + (f32::from(end) - f32::from(start)) * amount;
        value.round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(channel(fr, tr), channel(fg, tg), channel(fb, tb))
}

pub(crate) fn modal() -> Style {
    let t = active_theme();
    Style::default().fg(t.text).bg(t.panel)
}

/// Compute relative luminance according to WCAG 2.1 specs.
#[allow(dead_code)]
fn relative_luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        return 0.5;
    };
    let channel = |c: u8| {
        let s = f64::from(c) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// Compute WCAG 2.1 contrast ratio between two colours.
#[allow(dead_code)]
pub(crate) fn contrast_ratio(fg: Color, bg: Color) -> f64 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_states_have_distinct_signals() {
        for theme_id in ThemeId::all() {
            let t = Theme::for_id(*theme_id);
            if t.is_mono {
                continue; // Mono separates by weight/label, not hue
            }
            assert_ne!(t.proposal, t.approval, "theme {}", t.name());
            assert_ne!(t.approval, t.verified, "theme {}", t.name());
            assert_ne!(t.verified, t.refusal, "theme {}", t.name());
        }
    }

    #[test]
    fn wcag_contrast_meets_floor_for_all_shipped_themes() {
        for theme_id in ThemeId::all() {
            let t = Theme::for_id(*theme_id);
            let fg_roles = [
                ("text", t.text),
                ("proposal", t.proposal),
                ("approval", t.approval),
                ("verified", t.verified),
                ("refusal", t.refusal),
            ];
            for (bg_name, bg) in [("canvas", t.canvas), ("panel", t.panel)] {
                for (fg_name, fg) in fg_roles {
                    let ratio = contrast_ratio(fg, bg);
                    assert!(
                        ratio >= 2.5,
                        "Theme {} role {} on {} failed contrast ratio: {:.2} < 2.5",
                        t.name(),
                        fg_name,
                        bg_name,
                        ratio
                    );
                }
            }
        }
    }

    #[test]
    fn syntax_roles_stay_legible_on_the_code_band() {
        for theme_id in ThemeId::all() {
            let palette = SyntaxPalette::for_theme(&Theme::for_id(*theme_id));
            let fg_roles = [
                ("keyword", palette.keyword),
                ("string", palette.string),
                ("comment", palette.comment),
                ("number", palette.number),
                ("function", palette.function),
                ("type_name", palette.type_name),
                ("operator", palette.operator),
                ("punctuation", palette.punctuation),
                ("text", palette.text),
            ];
            for (fg_name, fg) in fg_roles {
                let ratio = contrast_ratio(fg, palette.code_bg);
                assert!(
                    ratio >= 2.5,
                    "Theme {} syntax role {} on code band failed contrast: {:.2} < 2.5",
                    theme_id.name(),
                    fg_name,
                    ratio
                );
            }
        }
    }

    #[test]
    fn no_color_forces_mono_and_sixteen_colours() {
        let detected = detect_color_depth(true, Some("truecolor"), Some("xterm-256color"));
        assert_eq!(detected.depth, ColorDepth::Color16);
        assert!(detected.force_mono);
    }

    #[test]
    fn colorterm_truecolor_wins() {
        let detected = detect_color_depth(false, Some("truecolor"), Some("xterm"));
        assert_eq!(detected.depth, ColorDepth::TrueColor);
        assert!(!detected.force_mono);
    }

    #[test]
    fn term_256color_without_colorterm_is_256() {
        let detected = detect_color_depth(false, None, Some("xterm-256color"));
        assert_eq!(detected.depth, ColorDepth::Color256);
    }

    #[test]
    fn dumb_terminals_get_sixteen_colours() {
        assert_eq!(
            detect_color_depth(false, None, Some("dumb")).depth,
            ColorDepth::Color16
        );
        assert_eq!(
            detect_color_depth(false, None, Some("linux")).depth,
            ColorDepth::Color16
        );
    }

    #[test]
    fn unknown_terminals_keep_the_truecolor_default() {
        assert_eq!(
            detect_color_depth(false, None, None).depth,
            ColorDepth::TrueColor
        );
        assert_eq!(
            detect_color_depth(false, None, Some("wezterm")).depth,
            ColorDepth::TrueColor
        );
    }

    #[test]
    fn noir_is_the_default_theme() {
        assert_eq!(ThemeId::default(), ThemeId::Noir);
    }
}

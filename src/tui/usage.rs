//! Provider-reported quota overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use time::OffsetDateTime;

use crate::core::pricing::{PricingTable, estimate_cost};
use crate::core::routing::BreakerState;
use crate::tui::layout::{centered, sanitize};
use crate::tui::reducer::ViewState;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ViewState) {
    let modal = centered(
        area,
        area.width.saturating_sub(4).min(90),
        area.height.saturating_sub(4).min(24),
    );
    let mut lines = Vec::new();

    render_session_usage(&mut lines, view);
    render_provider_quota(&mut lines, view);
    lines.push(Line::from(""));
    render_route(&mut lines, view);
    render_cost_estimate(&mut lines, view);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press ESC or type /usage again to close",
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
                    .border_style(theme::proposal())
                    .title(Span::styled(
                        " USAGE & QUOTA OVERLAY ",
                        theme::proposal().add_modifier(Modifier::BOLD),
                    )),
            ),
        modal,
    );
}

/// Route position and breaker states ("surfaced in the
/// `/usage`/provider overlay").
fn render_route(lines: &mut Vec<Line<'static>>, view: &ViewState) {
    let Some(route) = &view.snapshot.route else {
        return;
    };
    lines.push(Line::from(Span::styled(
        format!(
            "ROUTE — {} [position {}]",
            sanitize(&route.route),
            route.position
        ),
        theme::text().add_modifier(Modifier::BOLD),
    )));
    for breaker in view.snapshot.breakers.iter() {
        let style = match breaker.state {
            BreakerState::Closed => theme::text(),
            BreakerState::HalfOpen => theme::quota_style(0.85),
            BreakerState::Open => theme::quota_style(1.0),
        };
        lines.push(Line::from(Span::styled(
            format!(
                "  breaker {} — {} · {} consecutive failures",
                sanitize(breaker.provider.as_str()),
                breaker.state.label(),
                breaker.consecutive_failures
            ),
            style,
        )));
    }
    lines.push(Line::from(""));
}

/// A labelled spend estimate from the bundled per-Mtok pricing table (plan
/// §Phase 15). Never rendered as a fact: the line says "estimate" and nothing
/// here reads a provider-reported dollar figure, because providers do not
/// report one.
fn render_cost_estimate(lines: &mut Vec<Line<'static>>, view: &ViewState) {
    let (Some(provider), Some(model)) = (&view.snapshot.provider, &view.snapshot.model) else {
        return;
    };
    let table = PricingTable::bundled_defaults();
    let Some(price) = table.rate(provider, model) else {
        return;
    };
    let estimate = estimate_cost(&view.snapshot.usage, price);
    lines.push(Line::from(Span::styled(
        format!(
            "SPEND ESTIMATE — ${:.4} (bundled pricing, not provider-reported)",
            estimate.usd
        ),
        theme::muted(),
    )));
}

fn bar(used: f32) -> String {
    const WIDTH: usize = 20;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "clamped gauge maps to twenty cells"
    )]
    let filled = (used.clamp(0.0, 1.0) * WIDTH as f32).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(WIDTH.saturating_sub(filled))
    )
}

fn reset_label(reset: Option<OffsetDateTime>) -> String {
    let Some(reset) = reset else {
        return "  reset time not reported".to_owned();
    };
    let remaining = (reset - OffsetDateTime::now_utc()).whole_seconds().max(0);
    format!("  resets in {}", countdown(remaining))
}

/// A human-scaled countdown — a five-hour window reads as `4h32m`, not
/// `272m00s`, and a weekly one as `6d8h`, not four digits of minutes.
/// Shared with the status-bar quota gauge in `chrome.rs`.
pub(super) fn countdown(remaining_seconds: i64) -> String {
    #[expect(
        clippy::cast_sign_loss,
        reason = "remaining is clamped to non-negative by the caller"
    )]
    let total = remaining_seconds as u64;
    let days = total / 86_400;
    let hours = total % 86_400 / 3_600;
    let minutes = total % 3_600 / 60;
    let seconds = total % 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn render_session_usage(lines: &mut Vec<Line<'static>>, view: &ViewState) {
    let usage = view.snapshot.usage;
    let total_tokens = usage.input_tokens + usage.output_tokens;
    lines.push(Line::from(Span::styled(
        "SESSION TOKEN USAGE",
        theme::title().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled("Input Tokens:  ", theme::muted()),
        Span::styled(format!("{:<10}", usage.input_tokens), theme::text()),
        Span::styled("Output Tokens: ", theme::muted()),
        Span::styled(format!("{:<10}", usage.output_tokens), theme::text()),
        Span::styled("Total: ", theme::muted()),
        Span::styled(format!("{total_tokens}"), theme::title()),
    ]));
    lines.push(Line::from(""));
}

/// Every other quota producer reads response headers already sent for a
/// request the owner asked for; `gemini_cli`'s `fetchAvailableModels` is
/// smed's one deliberate side request (E1, `providers::gemini_cli`), and the
/// overlay says so rather than implying it rode along on the last turn. The
/// TUI may not depend on `providers` (AGENTS.md §2.1), so the two ids are
/// literals here rather than imported constants — same pattern `chrome.rs`'s
/// `pool_covers_model` already uses for the pool-label strings.
fn is_actively_probed(provider: &str) -> bool {
    matches!(provider, "gemini-cli" | "antigravity")
}

fn render_provider_quota(lines: &mut Vec<Line<'static>>, view: &ViewState) {
    lines.push(Line::from(Span::styled(
        "PROVIDER QUOTA & RATE LIMITS",
        theme::title().add_modifier(Modifier::BOLD),
    )));

    match &view.quota {
        Some(snapshot) => {
            let source = if is_actively_probed(snapshot.provider.as_str()) {
                "polled separately (may be up to 5m stale)"
            } else {
                "reported on the recent API response"
            };
            lines.push(Line::from(Span::styled(
                format!("{} — {source}", sanitize(snapshot.provider.as_str())),
                theme::text(),
            )));
            lines.push(Line::from(""));
            for window in &snapshot.windows {
                let used = window.used_fraction.clamp(0.0, 1.0);
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<18}", sanitize(&window.label)), theme::text()),
                    Span::styled(bar(used), theme::quota_style(used)),
                    Span::styled(
                        format!(" {:>3.0}% used", used * 100.0),
                        theme::quota_style(used),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    reset_label(window.resets_at),
                    theme::muted(),
                )));
            }
        }
        None => match &view.snapshot.quota_reserve.basis {
            crate::core::continuation::QuotaReserveBasis::ConfiguredTokens { limit } => {
                let used = view.snapshot.quota_reserve.used_fraction.unwrap_or(0.0);
                lines.push(Line::from(Span::styled(
                    format!("Configured token budget: {limit} tokens (estimated)"),
                    theme::text(),
                )));
                lines.push(Line::from(Span::styled(
                    format!("{} {:.0}% used (estimated)", bar(used), used * 100.0),
                    theme::quota_style(used),
                )));
            }
            crate::core::continuation::QuotaReserveBasis::ProviderReported { window } => {
                lines.push(Line::from(Span::styled(
                    format!("Last provider-reported window: {}", sanitize(window)),
                    theme::text(),
                )));
            }
            crate::core::continuation::QuotaReserveBasis::Unavailable => {
                lines.push(Line::from(Span::styled(
                    "No quota data reported and no token budget configured (reserve unavailable)",
                    theme::muted(),
                )));
                lines.push(Line::from(Span::styled(
                    "mjolnr will not guess quota bounds.",
                    theme::muted(),
                )));
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::countdown;

    #[test]
    fn countdown_uses_the_coarsest_readable_units() {
        assert_eq!(countdown(45), "45s");
        assert_eq!(countdown(200), "3m20s");
        assert_eq!(countdown(16_320), "4h32m");
        assert_eq!(countdown(547_200), "6d8h");
        assert_eq!(countdown(0), "0s");
    }
}

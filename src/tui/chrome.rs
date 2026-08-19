//! Header and status-row rendering for live session state.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::message::ContentBlock;
use crate::core::model::ModelId;
use crate::core::pricing::{PricingTable, estimate_cost};
use crate::tui::layout::sanitize;
use crate::tui::reducer::{RunStatus, ViewState};
use crate::tui::theme;

pub(super) fn render_header(frame: &mut Frame, area: Rect, view: &ViewState) {
    let model = sanitize(
        view.snapshot
            .model
            .as_ref()
            .map_or("no-model", |model| model.as_str()),
    );

    let project = view
        .snapshot
        .workspace_root
        .as_deref()
        .and_then(std::path::Path::file_name)
        .map_or_else(
            || "no-project".to_owned(),
            |name| sanitize(&name.to_string_lossy()),
        );
    let usage = view.snapshot.usage;
    let budget = view.snapshot.budget;

    // No wordmark here: the shell's top navigation bar carries it one row from
    // the top of the same screen, and printing it twice cost a reader a second
    // look to establish it was the same application.
    let mut spans = vec![
        Span::styled("PROJECT ", theme::muted()),
        Span::styled(format!("{project}  "), theme::text()),
        Span::styled("MODEL ", theme::muted()),
        Span::styled(format!("{model}  "), theme::text()),
    ];

    let has_usage = usage.input_tokens > 0 || usage.output_tokens > 0 || budget.provider_turns > 0;

    if has_usage {
        spans.push(Span::styled("USAGE ", theme::muted()));
        spans.push(Span::styled(
            format!(
                "{} in / {} out  ",
                humanize_tokens(usage.input_tokens),
                humanize_tokens(usage.output_tokens)
            ),
            theme::text(),
        ));
    }

    spans.push(Span::styled("POLICY ", theme::muted()));
    spans.push(Span::styled(
        format!("{} ", view.snapshot.policy.label()),
        if view.snapshot.policy.is_full_auto() {
            theme::full_auto()
        } else {
            theme::text()
        },
    ));

    // An armed envelope authorises spawns that will not be prompted for, so it
    // belongs beside the policy rather than behind a command someone has to
    // remember to run.
    if let Some(active) = &view.snapshot.envelope {
        spans.push(Span::styled(" ENVELOPE ", theme::muted()));
        spans.push(Span::styled(
            format!(
                "{}/{} ",
                active.children_remaining(),
                active.envelope.max_children
            ),
            theme::approval(),
        ));
    }

    if area.width >= 105 && has_usage {
        if let (Some(provider), Some(model)) = (&view.snapshot.provider, &view.snapshot.model) {
            let table = PricingTable::bundled_defaults();
            if let Some(rate) = table.rate(provider, model) {
                let cost = estimate_cost(&usage, rate);
                spans.push(Span::styled(
                    format!("  ≈${:.2} ", cost.usd),
                    theme::muted(),
                ));
            }
        }
        spans.push(Span::styled("  OPS ", theme::muted()));
        spans.push(Span::styled(
            format!(
                "{}/{}  TOOLS {}/{} ",
                budget.provider_turns,
                budget.max_provider_turns,
                budget.tool_calls,
                budget.max_tool_calls
            ),
            theme::text(),
        ));
    }

    if area.width >= 75 && has_usage {
        spans.push(Span::styled("  NEXT ", theme::muted()));
        spans.push(Span::styled(
            format!("≈{} tok ", next_context_estimate(view)),
            theme::text(),
        ));
        if let Some(gauge) = quota_gauge(view) {
            spans.push(Span::styled("  ", theme::muted()));
            spans.push(gauge);
        }
    }

    if view.snapshot.policy.is_full_auto() {
        spans.push(Span::styled("  FULL-AUTO ", theme::full_auto()));
        spans.push(Span::styled(
            format!("{} AUTO ", view.auto_allowed_side_effects),
            theme::full_auto(),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::canvas()),
        area,
    );
}

pub(super) fn render_status(frame: &mut Frame, area: Rect, view: &ViewState) {
    let (status, style) = status_line(view);
    let mut spans = vec![Span::styled(format!(" {status} "), style)];
    if view.lagged {
        spans.push(Span::styled(
            "  VIEW RESYNCED (EVENTS DROPPED) ",
            theme::approval(),
        ));
    }
    spans.push(Span::styled(
        if view.snapshot.run_active {
            "  ESC INTERRUPTS · ENTER STEERS · ALT-ENTER QUEUES "
        } else {
            "  F1 KEYMAP · SHIFT-TAB POLICY · CTRL-O DETAILS · /USAGE QUOTA "
        },
        theme::muted(),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::canvas()),
        area,
    );
}

fn humanize_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "compact UI labels do not require lossless integer recovery"
    )]
    let compact = tokens as f64 / 1_000.0;
    if compact < 100.0 {
        format!("{compact:.1}k")
    } else {
        format!("{compact:.0}k")
    }
}

fn status_line(view: &ViewState) -> (String, ratatui::style::Style) {
    if view.cancelling {
        return (
            "CANCELLING — Interrupting run...".to_owned(),
            theme::approval(),
        );
    }
    if view.keymap.exit_armed() {
        return (
            "EXIT ARMED · Ctrl-C again to quit".to_owned(),
            theme::approval(),
        );
    }
    if view.full_auto_armed {
        let color = theme::dimmed_approval(theme::pulse(view.tick, 0.04));
        return (
            "FULL-AUTO REQUESTED · writes and commands run without asking · not a sandbox · [y] confirm, any other key cancels".to_owned(),
            ratatui::style::Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
    }
    if let Some(envelope) = &view.envelope_armed {
        let color = theme::dimmed_approval(theme::pulse(view.tick, 0.04));
        return (
            format!(
                "ENVELOPE REQUESTED · up to {} children at {} across {} turns, spawned without asking · [y] confirm, any other key cancels",
                envelope.max_children,
                envelope.ceiling.label(),
                envelope.expires_after_turns
            ),
            ratatui::style::Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD),
        );
    }
    if let Some(detail) = &view.snapshot.store_failure {
        return (
            format!("DURABILITY LOST — {}", sanitize(detail)),
            theme::refusal(),
        );
    }
    if view.snapshot.recovery.is_required() {
        return (
            "RECOVERY_REQUIRES_DECISION — session halted".to_owned(),
            theme::refusal(),
        );
    }
    if let Some(approval) = &view.snapshot.pending_approval {
        let color = theme::dimmed_approval(theme::pulse(view.tick, 0.04));
        return (
            format!("AUTHORIZATION REQUIRED — {}", sanitize(&approval.tool_name)),
            ratatui::style::Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD),
        );
    }
    run_status(view)
}

fn run_status(view: &ViewState) -> (String, ratatui::style::Style) {
    match &view.status {
        RunStatus::Idle => ("STANDBY".to_owned(), theme::muted()),
        RunStatus::Streaming => {
            let activity = view.activity.as_ref().map_or_else(
                || "waiting on provider".to_owned(),
                crate::tui::reducer::Activity::label,
            );
            let phase = elapsed_label(view.phase_started_at);
            let turn = elapsed_label(view.run_started_at);
            let live_tokens = view.streaming_text.len().saturating_add(3) / 4;
            (
                format!(
                    "{} {} · phase {phase} · turn {turn} · ~{live_tokens} live tok · esc interrupts",
                    spinner(view.tick),
                    sanitize(&activity)
                ),
                theme::proposal(),
            )
        }
        RunStatus::Finished(reason) => (reason.label().to_owned(), theme::muted()),
        RunStatus::Failed { code, .. } => {
            let intent = view
                .last_intent
                .as_ref()
                .map(|name| format!(" while {}(…)", sanitize(name)))
                .unwrap_or_default();
            (format!("{code}{intent} · see transcript"), theme::refusal())
        }
    }
}

/// Anthropic and Codex windows (`"5h"`, `"7d"`, …) apply account-wide no
/// matter which of that provider's models is in use, so the worst of them is
/// the right single number. Google's pools (`"gemini"`, `"claude/gpt"`, from
/// `pool_label` in `gemini_cli`) do not — they're split by model family, so
/// "worst across all pools" can show a 67%-used pool the owner isn't even
/// drawing from while they sit on a 1%-used one. Prefer the pool that
/// actually covers the model in use; only fall back to worst-of-all when no
/// window names a pool the current model belongs to.
fn quota_gauge(view: &ViewState) -> Option<Span<'static>> {
    if let Some(worst) = view.quota.as_ref().and_then(|quota| {
        let model = view.snapshot.model.as_ref().map(ModelId::as_str);
        model
            .and_then(|model| {
                quota
                    .windows
                    .iter()
                    .find(|window| pool_covers_model(&window.label, model))
            })
            .or_else(|| {
                quota
                    .windows
                    .iter()
                    .max_by(|left, right| left.used_fraction.total_cmp(&right.used_fraction))
            })
    }) {
        let reset = worst.resets_at.map(|reset| {
            let remaining = (reset - time::OffsetDateTime::now_utc())
                .whole_seconds()
                .max(0);
            format!(" ({})", crate::tui::usage::countdown(remaining))
        });
        return Some(Span::styled(
            format!(
                "QUOTA {} {:.0}% used{} ",
                sanitize(&worst.label),
                worst.used_fraction.clamp(0.0, 1.0) * 100.0,
                reset.unwrap_or_default()
            ),
            theme::quota_style(worst.used_fraction),
        ));
    }
    let crate::core::continuation::QuotaReserveBasis::ConfiguredTokens { .. } =
        view.snapshot.quota_reserve.basis
    else {
        return None;
    };
    let used = view.snapshot.quota_reserve.used_fraction?;
    Some(Span::styled(
        format!("BUDGET ≈{:.0}% ", used.clamp(0.0, 1.0) * 100.0),
        theme::quota_style(used),
    ))
}

/// Mirrors `pool_label` in `providers::gemini_cli` — the only producer that
/// currently names a window after a model family rather than a duration.
/// Duration-style labels (`"5h"`, `"7d"`) never match either arm, so
/// Anthropic and Codex windows fall through to `quota_gauge`'s worst-of-all
/// fallback unchanged.
fn pool_covers_model(label: &str, model: &str) -> bool {
    match label {
        "gemini" => model.contains("gemini"),
        "claude/gpt" => model.contains("claude") || model.contains("gpt"),
        _ => false,
    }
}

fn next_context_estimate(view: &ViewState) -> usize {
    let mut bytes = view.composer.len();
    for message in view.snapshot.messages.iter() {
        bytes = bytes.saturating_add(message.text().len());
        for block in &message.blocks {
            match block {
                ContentBlock::ToolCall(call) => {
                    bytes = bytes.saturating_add(call.name.len());
                    bytes = bytes.saturating_add(call.arguments.to_string().len());
                }
                ContentBlock::ToolResult { name, result, .. } => {
                    bytes = bytes.saturating_add(name.len());
                    bytes = bytes.saturating_add(result.content.len());
                }
                ContentBlock::Text { .. } | ContentBlock::ImageRef { .. } => {}
            }
        }
    }
    bytes.saturating_add(3) / 4
}

const SPINNER_FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

fn spinner(tick: u64) -> char {
    let step = usize::try_from(tick / 4).unwrap_or(0);
    SPINNER_FRAMES
        .get(step % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or('⠋')
}

fn elapsed_label(since: Option<std::time::Instant>) -> String {
    let Some(since) = since else {
        return "0.0s".to_owned();
    };
    let elapsed = since.elapsed();
    if elapsed.as_secs() >= 60 {
        format!("{}m{:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!(
            "{}.{:01}s",
            elapsed.as_secs(),
            elapsed.subsec_millis() / 100
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ProviderId, QuotaSnapshot, QuotaWindow};
    use crate::tui::reducer::ViewState;

    #[test]
    fn pool_covers_model_matches_family_not_duration_labels() {
        assert!(pool_covers_model("gemini", "gemini-3.5-flash-low"));
        assert!(pool_covers_model("claude/gpt", "claude-sonnet-4-6"));
        assert!(pool_covers_model("claude/gpt", "gpt-oss-120b-medium"));
        assert!(!pool_covers_model("gemini", "claude-sonnet-4-6"));
        assert!(!pool_covers_model("5h", "gemini-3.5-flash-low"));
    }

    #[test]
    fn gauge_prefers_the_pool_the_active_model_actually_draws_from() {
        let mut view = ViewState::default();
        view.snapshot.model = Some(ModelId::new("gemini-3.5-flash-low"));
        view.quota = Some(QuotaSnapshot {
            provider: ProviderId::new("antigravity"),
            windows: vec![
                QuotaWindow {
                    label: "gemini".to_owned(),
                    used_fraction: 0.01,
                    resets_at: None,
                },
                QuotaWindow {
                    label: "claude/gpt".to_owned(),
                    used_fraction: 0.67,
                    resets_at: None,
                },
            ],
        });

        let gauge = quota_gauge(&view).expect("gauge renders");
        assert!(
            gauge.content.contains("gemini"),
            "showed {} while a gemini model was in use",
            gauge.content
        );
        assert!(!gauge.content.contains("claude/gpt"));
    }

    #[test]
    fn gauge_falls_back_to_worst_when_no_pool_names_the_model() {
        let mut view = ViewState::default();
        view.snapshot.model = Some(ModelId::new("claude-sonnet-4-6"));
        view.quota = Some(QuotaSnapshot {
            provider: ProviderId::new("anthropic"),
            windows: vec![
                QuotaWindow {
                    label: "5h".to_owned(),
                    used_fraction: 0.66,
                    resets_at: None,
                },
                QuotaWindow {
                    label: "7d".to_owned(),
                    used_fraction: 0.49,
                    resets_at: None,
                },
            ],
        });

        let gauge = quota_gauge(&view).expect("gauge renders");
        assert!(gauge.content.contains("5h"), "{}", gauge.content);
        assert!(gauge.content.contains("66%"), "{}", gauge.content);
    }
}

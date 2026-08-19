//! Provider response-header quota normalisation.
//!
//! Only facts present on the response become windows. This module does not
//! poll, extrapolate, or substitute a provider-family default.

use reqwest::header::HeaderMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{ProviderId, QuotaSnapshot, QuotaWindow};

struct HeaderWindow<'a> {
    label: &'a str,
    limit: &'a str,
    remaining: &'a str,
    reset: &'a str,
}

const WINDOWS: [HeaderWindow<'static>; 5] = [
    HeaderWindow {
        label: "requests",
        limit: "x-ratelimit-limit-requests",
        remaining: "x-ratelimit-remaining-requests",
        reset: "x-ratelimit-reset-requests",
    },
    HeaderWindow {
        label: "tokens",
        limit: "x-ratelimit-limit-tokens",
        remaining: "x-ratelimit-remaining-tokens",
        reset: "x-ratelimit-reset-tokens",
    },
    HeaderWindow {
        label: "requests",
        limit: "anthropic-ratelimit-requests-limit",
        remaining: "anthropic-ratelimit-requests-remaining",
        reset: "anthropic-ratelimit-requests-reset",
    },
    HeaderWindow {
        label: "input tokens",
        limit: "anthropic-ratelimit-input-tokens-limit",
        remaining: "anthropic-ratelimit-input-tokens-remaining",
        reset: "anthropic-ratelimit-input-tokens-reset",
    },
    HeaderWindow {
        label: "output tokens",
        limit: "anthropic-ratelimit-output-tokens-limit",
        remaining: "anthropic-ratelimit-output-tokens-remaining",
        reset: "anthropic-ratelimit-output-tokens-reset",
    },
];

pub(crate) fn from_headers(provider: ProviderId, headers: &HeaderMap) -> Option<QuotaSnapshot> {
    let mut windows = WINDOWS
        .iter()
        .filter_map(|window| standard_window(headers, window))
        .collect::<Vec<_>>();
    append_codex_window(headers, "primary", &mut windows);
    append_codex_window(headers, "secondary", &mut windows);
    append_anthropic_unified_windows(headers, &mut windows);
    (!windows.is_empty()).then_some(QuotaSnapshot { provider, windows })
}

pub(crate) async fn emit_from_headers(
    provider: ProviderId,
    headers: &HeaderMap,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let Some(snapshot) = from_headers(provider, headers) else {
        return Ok(());
    };
    tokio::select! {
        () = cancel.cancelled() => Err(ProviderError::Cancelled),
        result = events.send(ProviderEvent::Quota { snapshot }) => result.map_err(|_| ProviderError::Cancelled),
    }
}

fn standard_window(headers: &HeaderMap, window: &HeaderWindow<'_>) -> Option<QuotaWindow> {
    let limit = header_f64(headers, window.limit)?;
    let remaining = header_f64(headers, window.remaining)?;
    if limit <= 0.0 {
        return None;
    }
    Some(QuotaWindow {
        label: window.label.to_owned(),
        used_fraction: (1.0 - remaining / limit).clamp(0.0, 1.0),
        resets_at: header(headers, window.reset).and_then(parse_reset),
    })
}

/// Codex's `x-codex-{primary,secondary}-*` triple (E0 spike: confirmed
/// `primary` is the account's weekly window; `secondary` was inert —
/// `window-minutes: 0` — in every capture so far and is skipped rather than
/// shown as a real zero-length period). The label is the window's actual
/// duration (`"5h"`, `"7d"`, …), not the Codex-internal `primary`/`secondary`
/// naming, because the owner reads a duration, not an implementation slot.
fn append_codex_window(headers: &HeaderMap, name: &str, windows: &mut Vec<QuotaWindow>) {
    let minutes_name = format!("x-codex-{name}-window-minutes");
    let Some(minutes) = header_f64(headers, &minutes_name).filter(|value| *value > 0.0) else {
        return;
    };
    let used_name = format!("x-codex-{name}-used-percent");
    let Some(used) = header_f64(headers, &used_name) else {
        return;
    };
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "window-minutes is a small positive count reported by the provider"
    )]
    let label = duration_label(minutes as u64 * 60);
    let reset_name = format!("x-codex-{name}-reset-at");
    windows.push(QuotaWindow {
        label,
        used_fraction: (used / 100.0).clamp(0.0, 1.0),
        resets_at: header(headers, &reset_name).and_then(parse_reset),
    });
}

/// Anthropic's OAuth/subscription traffic (E0 spike) reports
/// `anthropic-ratelimit-unified-{period}-{status,utilization,reset}`, one
/// triple per window — `5h` and `7d` observed, no fixed list assumed here
/// since Anthropic may add periods. `utilization` is already a 0.0–1.0
/// fraction; no limit/remaining division needed. This is a distinct header
/// family from the older per-resource `anthropic-ratelimit-{requests,
/// input-tokens,output-tokens}-*` triples above, which stay as they are for
/// API-key traffic.
fn append_anthropic_unified_windows(headers: &HeaderMap, windows: &mut Vec<QuotaWindow>) {
    const PREFIX: &str = "anthropic-ratelimit-unified-";
    const SUFFIX: &str = "-utilization";
    let periods: Vec<String> = headers
        .keys()
        .filter_map(|name| {
            let name = name.as_str();
            let period = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
            (!period.is_empty()).then(|| period.to_owned())
        })
        .collect();
    for period in periods {
        let Some(utilization) = header_f64(headers, &format!("{PREFIX}{period}{SUFFIX}")) else {
            continue;
        };
        let reset = header(headers, &format!("{PREFIX}{period}-reset")).and_then(parse_reset);
        windows.push(QuotaWindow {
            label: period,
            used_fraction: utilization.clamp(0.0, 1.0),
            resets_at: reset,
        });
    }
}

/// A human-scaled window label from a duration in seconds — minutes alone
/// are not a useful unit once a window spans days, so this rounds to the
/// coarsest unit that still reads as a real period (`"5h"`, not `"300m"`;
/// `"7d"`, not `"10080m"`).
#[expect(
    clippy::cast_precision_loss,
    reason = "a real-world window duration never approaches f64's mantissa limit"
)]
pub(crate) fn duration_label(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if seconds >= DAY && seconds.is_multiple_of(DAY) {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR && seconds.is_multiple_of(HOUR) {
        format!("{}h", seconds / HOUR)
    } else if seconds >= MINUTE && seconds.is_multiple_of(MINUTE) {
        format!("{}m", seconds / MINUTE)
    } else if seconds >= DAY {
        format!("{:.1}d", seconds as f64 / DAY as f64)
    } else if seconds >= HOUR {
        format!("{:.1}h", seconds as f64 / HOUR as f64)
    } else if seconds >= MINUTE {
        format!("{:.1}m", seconds as f64 / MINUTE as f64)
    } else {
        format!("{seconds}s")
    }
}

fn header_f64(headers: &HeaderMap, name: &str) -> Option<f32> {
    header(headers, name)?.parse().ok()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// Shared with `gemini_cli`'s `fetchAvailableModels` probe, whose `resetTime`
/// is the same RFC 3339 shape this already parses.
pub(crate) fn parse_reset(raw: &str) -> Option<OffsetDateTime> {
    if let Ok(unix) = raw.parse::<i64>() {
        return if unix > 1_000_000_000 {
            OffsetDateTime::from_unix_timestamp(unix).ok()
        } else {
            OffsetDateTime::now_utc().checked_add(time::Duration::seconds(unix))
        };
    }
    if let Ok(timestamp) = OffsetDateTime::parse(raw, &Rfc3339) {
        return Some(timestamp);
    }
    duration_seconds(raw).and_then(|seconds| {
        OffsetDateTime::now_utc().checked_add(time::Duration::seconds_f64(seconds))
    })
}

fn duration_seconds(raw: &str) -> Option<f64> {
    for (suffix, multiplier) in [("ms", 0.001), ("s", 1.0), ("m", 60.0), ("h", 3_600.0)] {
        if let Some(number) = raw.strip_suffix(suffix) {
            return number.parse::<f64>().ok().map(|value| value * multiplier);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_headers_become_used_fraction_without_guessing() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit-tokens", "100".parse().unwrap());
        headers.insert("x-ratelimit-remaining-tokens", "25".parse().unwrap());
        headers.insert("x-ratelimit-reset-tokens", "1700000000".parse().unwrap());

        let snapshot = from_headers(ProviderId::new("openai"), &headers).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        let window = snapshot.windows.first().expect("one window");
        assert_eq!(window.label, "tokens");
        assert!((window.used_fraction - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn absent_or_incomplete_headers_produce_no_snapshot() {
        let headers = HeaderMap::new();
        assert!(from_headers(ProviderId::new("openai"), &headers).is_none());
    }

    #[test]
    fn provider_duration_resets_are_understood_without_inventing_a_window() {
        assert!((duration_seconds("250ms").expect("milliseconds") - 0.25).abs() < f64::EPSILON);
        assert_eq!(duration_seconds("2m"), Some(120.0));
        assert_eq!(duration_seconds("unknown"), None);
    }

    #[test]
    fn anthropic_unified_headers_become_utilization_windows_by_period() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "0.66".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-5h-reset",
            "1785487200".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-utilization",
            "0.49".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-reset",
            "1785952800".parse().unwrap(),
        );
        // Non-window unified fields must not be mistaken for a period.
        headers.insert(
            "anthropic-ratelimit-unified-fallback-percentage",
            "0.5".parse().unwrap(),
        );

        let snapshot = from_headers(ProviderId::new("anthropic"), &headers).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        let five_hour = snapshot
            .windows
            .iter()
            .find(|window| window.label == "5h")
            .expect("5h window");
        assert!((five_hour.used_fraction - 0.66).abs() < f32::EPSILON);
        assert!(five_hour.resets_at.is_some());
        assert!(snapshot.windows.iter().any(|window| window.label == "7d"));
    }

    #[test]
    fn codex_window_label_is_the_reported_duration_not_the_slot_name() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-primary-window-minutes", "10080".parse().unwrap());
        headers.insert("x-codex-primary-used-percent", "100".parse().unwrap());
        headers.insert("x-codex-primary-reset-at", "1785907533".parse().unwrap());
        // Inert secondary window: window-minutes 0 must not become a window.
        headers.insert("x-codex-secondary-window-minutes", "0".parse().unwrap());
        headers.insert("x-codex-secondary-used-percent", "0".parse().unwrap());

        let snapshot = from_headers(ProviderId::new("openai-codex"), &headers).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        let window = snapshot.windows.first().expect("one window");
        assert_eq!(window.label, "7d");
        assert!((window.used_fraction - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn duration_label_prefers_the_coarsest_exact_unit() {
        assert_eq!(duration_label(30), "30s");
        assert_eq!(duration_label(300), "5m");
        assert_eq!(duration_label(18_000), "5h");
        assert_eq!(duration_label(604_800), "7d");
        assert_eq!(duration_label(90), "1.5m");
    }
}

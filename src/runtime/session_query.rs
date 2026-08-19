//! Answering `query_session` from the store.
//!
//! The actor holds the store; the tool is a marker. This module is the
//! projection between them: durable events in, a bounded and sanitised window
//! out.
//!
//! Scope is enforced by construction rather than by validation. The window is
//! read from `self.state.session` and the tool's schema has no session
//! parameter, so there is no argument to check and nothing to get wrong. A child
//! cannot read its parent's session, and a parent cannot read a child's: the
//! parent's grant to a child is a directive and a result schema, and the child's
//! answer to the parent is a validated result and a branch. Neither includes the
//! other's transcript.

use crate::core::error::ReasonCode;
use crate::core::event::{RunId, SmedEvent, StoredEvent};
use crate::core::message::{ContentBlock, ToolResult};
use crate::tools::session_query::{DEFAULT_ENTRIES, MAX_ENTRIES, MAX_SUMMARY_CHARS};

use super::Actor;

impl Actor {
    /// Read this session's recorded history and record the answer.
    ///
    /// Returns `false` in the same shape the other synchronous tool paths do: the
    /// result is already recorded, and the loop should carry on rather than wait
    /// for a spawned task.
    pub(super) async fn answer_session_query(
        &mut self,
        run: RunId,
        call: crate::core::message::ToolCall,
    ) -> bool {
        let Some(session) = self.state.session else {
            let result = ToolResult::failed(
                ReasonCode::ToolExecution,
                "query_session needs an open session",
            );
            let _ = self.record_tool_result(run, &call, result).await;
            return false;
        };
        let limit = call
            .arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(DEFAULT_ENTRIES, |value| {
                usize::try_from(value).unwrap_or(DEFAULT_ENTRIES)
            })
            .min(MAX_ENTRIES);
        let kind = call
            .arguments
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);

        // `branch_events` resolves the active branch, so a session that rewound
        // reads the history it is actually on — the same view `/tree` shows.
        let events = match self.store.branch_events(session).await {
            Ok(events) => events,
            Err(error) => {
                // An unreadable database and an empty window must not look the
                // same to the model: one means "nothing happened", the other
                // means "I cannot tell you what happened". `ToolExecution`
                // rather than a code of its own — a store-read failure arguably
                // deserves one, but adding a `ReasonCode` is a change to the
                // wire contract every other surface asserts on, and the detail
                // already distinguishes this from an empty window.
                let result = ToolResult::failed(
                    ReasonCode::ToolExecution,
                    format!("could not read this session's record: {error}"),
                );
                let _ = self.record_tool_result(run, &call, result).await;
                return false;
            }
        };

        let rendered = render_window(&events, kind.as_deref(), limit);
        let _ = self
            .record_tool_result(run, &call, ToolResult::ok(rendered))
            .await;
        false
    }
}

/// Project the newest matching events into a bounded, human-readable window.
fn render_window(events: &[StoredEvent], kind: Option<&str>, limit: usize) -> String {
    let mut lines = Vec::new();
    for stored in events.iter().rev() {
        let Some((event_kind, summary)) = project(&stored.event) else {
            continue;
        };
        if kind.is_some_and(|wanted| wanted != event_kind) {
            continue;
        }
        lines.push(format!(
            "{} {event_kind}: {}",
            stored.sequence,
            bound(&summary)
        ));
        if lines.len() >= limit {
            break;
        }
    }
    if lines.is_empty() {
        return match kind {
            Some(kind) => format!("no `{kind}` entries in this session's record"),
            None => "this session's record is empty".to_owned(),
        };
    }
    lines.join("\n")
}

/// One event's kind and summary, or `None` for events that say nothing useful
/// to the model.
///
/// Deliberately not exhaustive over `SmedEvent`: this is a view for a reader,
/// not a serialisation. A new event kind that nobody projected is absent from
/// the window rather than rendered as a debug blob.
fn project(event: &SmedEvent) -> Option<(&'static str, String)> {
    match event {
        SmedEvent::MessageAppended { message, .. } => {
            Some(("message_appended", message_summary(message)))
        }
        SmedEvent::ToolProposed { call, tier, .. } => Some((
            "tool_proposed",
            format!("{} [{tier:?}] {}", call.name, call.arguments),
        )),
        SmedEvent::ToolCompleted { name, result, .. } => Some((
            "tool_completed",
            format!("{name} — {:?}: {}", result.outcome, result.content),
        )),
        SmedEvent::ToolFailed { name, code, .. } => {
            Some(("tool_failed", format!("{name} — {code}")))
        }
        SmedEvent::ApprovalResolved { decision, .. } => {
            Some(("approval_resolved", format!("{decision:?}")))
        }
        SmedEvent::PolicyChanged { mode, .. } => Some(("policy_changed", mode.label().to_owned())),
        SmedEvent::ModelChanged {
            provider, model, ..
        } => Some(("model_changed", format!("{provider}/{model}"))),
        SmedEvent::FileSaved {
            path,
            observed_digest,
            new_digest,
            ..
        } => Some((
            "file_saved",
            format!("{path} {observed_digest} -> {new_digest}"),
        )),
        SmedEvent::RunFinished { reason, .. } => Some(("run_finished", format!("{reason:?}"))),
        SmedEvent::RunFailed { code, detail, .. } => {
            Some(("run_failed", format!("{code}: {detail}")))
        }
        SmedEvent::HandoffCreated { handoff, .. } => {
            Some(("handoff_created", handoff.status.clone()))
        }
        SmedEvent::SubagentSpawned { directive, .. } => {
            Some(("subagent_spawned", directive.clone()))
        }
        SmedEvent::ReadSetCollision {
            reader,
            writer,
            path,
            ..
        } => Some((
            "read_set_collision",
            format!("{path} (read by {reader}, written by {writer})"),
        )),
        SmedEvent::BudgetExhausted { .. } => {
            Some(("budget_exhausted", "the run's budget ran out".to_owned()))
        }
        SmedEvent::RecoveryRequired { work, .. } => {
            Some(("recovery_required", format!("{work:?}")))
        }
        _ => None,
    }
}

fn message_summary(message: &crate::core::message::CanonicalMessage) -> String {
    let text: Vec<&str> = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    format!("{:?} {}", message.role, text.join(" "))
}

/// Sanitise and cap one summary.
///
/// Control characters are stripped for the same reason the timeline strips them:
/// recorded text can contain anything a model or a command produced, and this
/// window is read back into a prompt.
fn bound(summary: &str) -> String {
    let cleaned: String = summary
        .chars()
        .map(|character| {
            if character.is_control() && character != '\t' {
                ' '
            } else {
                character
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= MAX_SUMMARY_CHARS {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(MAX_SUMMARY_CHARS).collect();
    format!("{kept}…")
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;
    use crate::core::event::{EventId, SessionId};
    use crate::core::message::CanonicalMessage;
    use time::OffsetDateTime;

    fn stored(sequence: u64, event: SmedEvent) -> StoredEvent {
        StoredEvent {
            id: EventId::new(),
            sequence,
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            event,
        }
    }

    fn message(session: SessionId, text: &str) -> SmedEvent {
        SmedEvent::MessageAppended {
            session,
            message: Box::new(CanonicalMessage::user(text)),
        }
    }

    #[test]
    fn a_refused_write_reads_as_a_refusal_and_never_as_a_write() {
        // This is the property the Read tier rests on. Every event the window
        // can return describes something that already passed the gate, so a
        // write that was refused must come back as a refusal — if it read as a
        // write, the window would be reporting an act that never happened, and
        // "replaying decisions already made grants nothing" would stop being
        // true.
        let session = SessionId::new();
        let events = vec![stored(
            1,
            SmedEvent::ToolCompleted {
                session,
                run: RunId::new(),
                call_id: "call_1".to_owned(),
                name: "edit_file".to_owned(),
                result: ToolResult::refused(
                    ReasonCode::ApprovalDenied,
                    "the human denied this edit",
                ),
            },
        )];

        let window = render_window(&events, None, 10);
        assert!(
            window.contains("Refused") || window.contains("APPROVAL_DENIED"),
            "the refusal must be visible in the projection:\n{window}"
        );
        assert!(
            window.contains("edit_file"),
            "the refused tool must still be named:\n{window}"
        );
    }

    #[test]
    fn the_window_is_newest_first_and_bounded() {
        let session = SessionId::new();
        let events: Vec<StoredEvent> = (1..=40)
            .map(|n| stored(n, message(session, &format!("turn {n}"))))
            .collect();

        let window = render_window(&events, None, 5);
        let lines: Vec<&str> = window.lines().collect();
        assert_eq!(lines.len(), 5, "the limit must hold:\n{window}");
        assert!(
            lines[0].starts_with("40 "),
            "newest first, not oldest:\n{window}"
        );
        assert!(lines[4].starts_with("36 "));
    }

    #[test]
    fn a_kind_filter_excludes_everything_else() {
        let session = SessionId::new();
        let events = vec![
            stored(1, message(session, "hello")),
            stored(
                2,
                SmedEvent::PolicyChanged {
                    session,
                    mode: crate::core::policy::PolicyMode::ReadOnly,
                },
            ),
        ];

        let window = render_window(&events, Some("policy_changed"), 10);
        assert!(window.contains("policy_changed"));
        assert!(
            !window.contains("hello"),
            "a filtered query must not leak other kinds:\n{window}"
        );
    }

    #[test]
    fn an_empty_window_says_so_rather_than_returning_nothing() {
        let window = render_window(&[], None, 10);
        assert!(window.contains("empty"), "{window}");
        let filtered = render_window(&[], Some("tool_completed"), 10);
        assert!(
            filtered.contains("tool_completed"),
            "an empty filtered window should name the filter:\n{filtered}"
        );
    }

    #[test]
    fn a_long_summary_is_capped_and_control_characters_never_survive() {
        let session = SessionId::new();
        let hostile = format!("\u{1b}[31mred\u{1b}[0m {}", "x".repeat(1000));
        let events = vec![stored(1, message(session, &hostile))];

        let window = render_window(&events, None, 10);
        assert!(
            !window.contains('\u{1b}'),
            "escape sequences must not survive"
        );
        assert!(
            window.chars().count() < MAX_SUMMARY_CHARS + 80,
            "summary cap did not hold: {} chars",
            window.chars().count()
        );
    }
}

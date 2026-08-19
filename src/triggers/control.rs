//! A trigger's control session: the durable home for its lifecycle events.
//!
//! "Every firing is a session"  — but skip/queue/replace
//! decisions, disablement, and re-arming are facts about the *trigger*, not
//! about any one firing, and every durable event needs a session to belong
//! to. Each trigger gets one small, deterministic control session that never
//! itself sends a directive or calls a provider; the firings it records are
//! ordinary sessions parented to it (`sessions.parent_session_id`), the same
//! column Phase 13 added for subagent children.
//!
//! Deterministic identity — hashed from the project root and trigger name,
//! not a fresh `SessionId::new()` — is what lets a restarted scheduler find
//! the same control session and replay it, rather than losing the failure
//! count and disabled state on every restart.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::core::error::ReasonCode;
use crate::core::event::{SessionId, SmedEvent, StoredEvent};
use crate::core::store::{EventStore, ProjectId, StoreError};
use crate::core::trigger::TriggerOutcome;

/// Deterministic control-session identity for one trigger in one project.
///
/// Hashing rather than `Uuid::new_v5` avoids a new `uuid` crate feature for a
/// single call site: SHA-256 is already a dependency (`store::secrets`), and
/// sixteen of its bytes are exactly as suitable a UUID payload as a v5 hash.
#[must_use]
pub fn control_session_id(project_root_realpath: &str, trigger_name: &str) -> SessionId {
    let mut hasher = Sha256::new();
    hasher.update(b"smed-trigger-control-session:v1:");
    hasher.update(project_root_realpath.as_bytes());
    hasher.update(b":");
    hasher.update(trigger_name.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    if let Some(prefix) = digest.get(..16) {
        bytes.copy_from_slice(prefix);
    }
    SessionId::from_uuid(uuid::Uuid::from_bytes(bytes))
}

/// The canonical string every control-session identity is hashed from.
///
/// One function so the scheduler, `smed triggers list`/`rearm`, and the TUI
/// snapshot always agree on the same control session for the same project —
/// disagreement here would mean two processes computing two different
/// identities for what a human sees as "one trigger".
///
/// # Errors
/// If the path cannot be canonicalised (does not exist, or a filesystem
/// error).
pub fn root_realpath(workspace_root: &Path) -> std::io::Result<String> {
    Ok(workspace_root
        .canonicalize()?
        .to_string_lossy()
        .into_owned())
}

/// Create the control session if it does not already exist. Idempotent: a
/// second call after a restart finds the same row via
/// [`EventStore::sessions`] and does nothing.
pub async fn ensure(
    store: &dyn EventStore,
    project: ProjectId,
    session: SessionId,
    trigger_name: &str,
) -> Result<(), StoreError> {
    let exists = store
        .sessions()
        .await?
        .iter()
        .any(|summary| summary.id == session);
    if exists {
        return Ok(());
    }
    store
        .create_session(session, project, format!("trigger:{trigger_name}"), None)
        .await
}

/// What replaying a control session's history establishes about a trigger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TriggerRuntimeState {
    pub consecutive_failures: u32,
    pub disabled_reason: Option<ReasonCode>,
    pub last_outcome: Option<TriggerOutcome>,
}

/// Fold a control session's durable events into the trigger's current state.
///
/// Event-sourced rather than a mutable row: the same posture every other
/// piece of smed state takes (`AGENTS.md` — "do not invent a side channel").
/// A human re-arming a disabled trigger is a later [`SmedEvent::TriggerRearmed`]
/// event, so "is it disabled" is answered by "is the last disable/rearm event
/// a disable", not by a flag that could drift from the log that explains it.
#[must_use]
pub fn replay(events: &[StoredEvent], trigger_name: &str) -> TriggerRuntimeState {
    let mut state = TriggerRuntimeState::default();
    for stored in events {
        match &stored.event {
            SmedEvent::TriggerSettled {
                trigger, outcome, ..
            } if trigger == trigger_name => {
                state.last_outcome = Some(*outcome);
                if outcome.counts_as_failure() {
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                } else {
                    state.consecutive_failures = 0;
                }
            }
            SmedEvent::TriggerDisabled { trigger, code, .. } if trigger == trigger_name => {
                state.disabled_reason = Some(*code);
            }
            SmedEvent::TriggerRearmed { trigger, .. } if trigger == trigger_name => {
                state.disabled_reason = None;
                state.consecutive_failures = 0;
            }
            _ => {}
        }
    }
    state
}

/// Fetch a control session's history, defaulting to empty for one that does
/// not exist yet (a trigger that has never fired).
pub async fn history(
    store: &dyn EventStore,
    session: SessionId,
) -> Result<Vec<StoredEvent>, StoreError> {
    match store.events(session).await {
        Ok(events) => Ok(events),
        Err(StoreError::UnknownSession { .. }) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[test]
    fn control_session_ids_are_deterministic_and_distinct() {
        let a = control_session_id("/repo/one", "nightly");
        let b = control_session_id("/repo/one", "nightly");
        let c = control_session_id("/repo/one", "other");
        let d = control_session_id("/repo/two", "nightly");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    fn stored(event: SmedEvent, sequence: u64) -> StoredEvent {
        StoredEvent {
            id: crate::core::event::EventId::new(),
            sequence,
            occurred_at: OffsetDateTime::now_utc(),
            event,
        }
    }

    #[test]
    fn replay_counts_consecutive_failures_and_resets_on_success() {
        let control = SessionId::new();
        let child = SessionId::new();
        let events = vec![
            stored(
                SmedEvent::TriggerSettled {
                    session: control,
                    trigger: "t".to_owned(),
                    child,
                    outcome: TriggerOutcome::Failed,
                    reason_code: Some(ReasonCode::ToolExecution),
                },
                0,
            ),
            stored(
                SmedEvent::TriggerSettled {
                    session: control,
                    trigger: "t".to_owned(),
                    child,
                    outcome: TriggerOutcome::Failed,
                    reason_code: Some(ReasonCode::ToolExecution),
                },
                1,
            ),
            stored(
                SmedEvent::TriggerSettled {
                    session: control,
                    trigger: "t".to_owned(),
                    child,
                    outcome: TriggerOutcome::Verified,
                    reason_code: None,
                },
                2,
            ),
        ];
        let state = replay(&events, "t");
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_outcome, Some(TriggerOutcome::Verified));
    }

    #[test]
    fn a_disable_event_disables_until_a_later_rearm() {
        let control = SessionId::new();
        let events = vec![stored(
            SmedEvent::TriggerDisabled {
                session: control,
                trigger: "t".to_owned(),
                code: ReasonCode::TriggerDisabled,
                consecutive_failures: 3,
            },
            0,
        )];
        let state = replay(&events, "t");
        assert_eq!(state.disabled_reason, Some(ReasonCode::TriggerDisabled));

        let mut rearmed = events;
        rearmed.push(stored(
            SmedEvent::TriggerRearmed {
                session: control,
                trigger: "t".to_owned(),
            },
            1,
        ));
        let state = replay(&rearmed, "t");
        assert_eq!(state.disabled_reason, None);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn events_for_a_different_trigger_are_ignored() {
        let control = SessionId::new();
        let events = vec![stored(
            SmedEvent::TriggerDisabled {
                session: control,
                trigger: "other".to_owned(),
                code: ReasonCode::TriggerDisabled,
                consecutive_failures: 3,
            },
            0,
        )];
        let state = replay(&events, "t");
        assert_eq!(state.disabled_reason, None);
    }
}

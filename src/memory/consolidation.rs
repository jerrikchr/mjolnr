//! Background consolidation cycle for episodic memory and fact extraction
//! (master implementation plan §2.2).
//!
//! One responsibility: derive episodic summaries and temporal facts from recorded
//! session events.
//!
//! **Standing Law #2 (Recall is a projection, never authority):**
//! Consolidation reads the durable event ledger (`EventStore`) and writes only to
//! the disposable `.mjolnr/data/memory.db`. It never writes to the ledger, never
//! mutates transcripts in place, and never widens policy. Cancellation is plumbed
//! via [`CancellationToken`], and work is bounded per cycle.

use std::fmt::Write as _;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::core::event::{SmedEvent, StoredEvent};
use crate::core::message::Role;
use crate::memory::error::MemoryError;
use crate::memory::store::{Episode, MemoryStore};

/// Most events consolidated in one pass to prevent unbounded memory usage.
pub const MAX_EVENTS_PER_PASS: usize = 100;

/// Process un-consolidated events from `events` and persist an episodic summary
/// and extracted knowledge triples into `store`.
pub async fn consolidate_events(
    store: &MemoryStore,
    session_id: &str,
    events: &[StoredEvent],
    cancel: &CancellationToken,
) -> Result<Option<Episode>, MemoryError> {
    if cancel.is_cancelled() || events.is_empty() {
        return Ok(None);
    }

    let progress = store.get_consolidation_progress(session_id).await?;
    let start_seq = progress.map_or(0, |seq| seq + 1);

    let eligible: Vec<&StoredEvent> = events
        .iter()
        .filter(|event| event.sequence >= start_seq)
        .take(MAX_EVENTS_PER_PASS)
        .collect();

    if eligible.is_empty() {
        return Ok(None);
    }

    let min_seq = eligible.first().map_or(start_seq, |e| e.sequence);
    let max_seq = eligible.last().map_or(start_seq, |e| e.sequence);

    let mut user_prompts = Vec::new();
    let mut decisions = Vec::new();
    let mut tool_actions = Vec::new();

    for event in &eligible {
        if cancel.is_cancelled() {
            return Ok(None);
        }

        match &event.event {
            SmedEvent::MessageAppended { message, .. } => {
                if message.role == Role::User {
                    let text = message.text();
                    if !text.is_empty() {
                        user_prompts.push(text);
                    }
                }
            }
            SmedEvent::ApprovalResolved { decision, .. } => {
                decisions.push(format!("approval: {decision:?}"));
            }
            SmedEvent::ToolCompleted { name, result, .. } => {
                tool_actions.push(format!("{name}: {result:?}"));
            }
            SmedEvent::PolicyChanged { mode, .. } => {
                decisions.push(format!("policy changed to {mode:?}"));
            }
            _ => {}
        }
    }

    // Build a concise episodic summary
    let mut summary = String::new();
    if !user_prompts.is_empty() {
        let _ = writeln!(
            summary,
            "User Intent: {}",
            user_prompts
                .join("; ")
                .chars()
                .take(500)
                .collect::<String>()
        );
    }
    if !tool_actions.is_empty() {
        let count = tool_actions.len();
        let sample = tool_actions
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(summary, "Actions ({count} total): {sample}");
    }

    let key_decisions = if decisions.is_empty() {
        "No explicit policy changes".to_owned()
    } else {
        decisions.join("; ")
    };

    if summary.is_empty() {
        summary = format!("Consolidated {} event(s)", eligible.len());
    }

    let now = OffsetDateTime::now_utc();
    let episode_id = store
        .record_episode(session_id, &summary, &key_decisions, min_seq, max_seq, now)
        .await?;

    Ok(Some(Episode {
        id: episode_id,
        session_id: session_id.to_owned(),
        summary,
        key_decisions,
        source_event_start: min_seq,
        source_event_end: max_seq,
        created_at: now,
    }))
}

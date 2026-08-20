//! Integration tests for the memory consolidation cycle (master implementation plan §2.2).

use tempfile::tempdir;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use mjolnr::core::command::{ApprovalDecision, ApprovalId};
use mjolnr::core::event::{EventId, MjolnrEvent, RunId, SessionId, StoredEvent};
use mjolnr::core::message::{CanonicalMessage, ToolResult};
use mjolnr::memory::consolidation::consolidate_events;
use mjolnr::memory::store::MemoryStore;

#[tokio::test]
async fn consolidation_on_empty_or_cancelled_returns_none() {
    let workspace = tempdir().expect("tempdir");
    let db_path = workspace.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.expect("open store");

    let cancel = CancellationToken::new();
    let res = consolidate_events(&store, "session-1", &[], &cancel)
        .await
        .expect("empty events");
    assert_eq!(res, None);

    cancel.cancel();
    let dummy_event = StoredEvent {
        id: EventId::new(),
        sequence: 1,
        occurred_at: OffsetDateTime::now_utc(),
        event: MjolnrEvent::SessionCreated {
            session: SessionId::new(),
            provider: mjolnr::core::model::ProviderId::new("p"),
            model: mjolnr::core::model::ModelId::new("m"),
        },
    };
    let res = consolidate_events(&store, "session-1", &[dummy_event], &cancel)
        .await
        .expect("cancelled");
    assert_eq!(res, None);
}

#[tokio::test]
async fn consolidation_distills_events_and_updates_progress() {
    let workspace = tempdir().expect("tempdir");
    let db_path = workspace.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.expect("open store");
    let session = SessionId::new();
    let session_id = session.to_string();
    let now = OffsetDateTime::now_utc();

    let events = vec![
        StoredEvent {
            id: EventId::new(),
            sequence: 1,
            occurred_at: now,
            event: MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::user("Refactor memory layer for mjolnr")),
            },
        },
        StoredEvent {
            id: EventId::new(),
            sequence: 2,
            occurred_at: now,
            event: MjolnrEvent::ApprovalResolved {
                session,
                run: RunId::new(),
                approval: ApprovalId::new(),
                decision: ApprovalDecision::AutoByPolicy,
            },
        },
        StoredEvent {
            id: EventId::new(),
            sequence: 3,
            occurred_at: now,
            event: MjolnrEvent::ToolCompleted {
                session,
                run: RunId::new(),
                call_id: "call-1".to_owned(),
                name: "write_file".to_owned(),
                result: ToolResult::ok("written"),
            },
        },
    ];

    let cancel = CancellationToken::new();
    let episode = consolidate_events(&store, &session_id, &events, &cancel)
        .await
        .expect("consolidate")
        .expect("produced episode");

    assert_eq!(episode.session_id, session_id);
    assert_eq!(episode.source_event_start, 1);
    assert_eq!(episode.source_event_end, 3);
    assert!(episode.summary.contains("Refactor memory layer"));
    assert!(episode.summary.contains("write_file"));

    // Verify progress tracking
    let progress = store
        .get_consolidation_progress(&session_id)
        .await
        .expect("progress");
    assert_eq!(progress, Some(3));

    // Verify recent episodes retrieval
    let recent = store.get_recent_episodes(10).await.expect("recent");
    assert_eq!(recent.len(), 1);
    let first_recent = recent.first().expect("first episode");
    assert_eq!(first_recent.id, episode.id);

    // Re-running without new events produces None
    let second_run = consolidate_events(&store, &session_id, &events, &cancel)
        .await
        .expect("second run");
    assert_eq!(second_run, None);

    // Appending a new event consolidates only the new slice
    let next_events = vec![
        events.first().expect("event 0").clone(),
        events.get(1).expect("event 1").clone(),
        events.get(2).expect("event 2").clone(),
        StoredEvent {
            id: EventId::new(),
            sequence: 4,
            occurred_at: now,
            event: MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::user("Run verification tests")),
            },
        },
    ];

    let next_episode = consolidate_events(&store, &session_id, &next_events, &cancel)
        .await
        .expect("next consolidation")
        .expect("produced next episode");

    assert_eq!(next_episode.source_event_start, 4);
    assert_eq!(next_episode.source_event_end, 4);
    assert!(next_episode.summary.contains("Run verification tests"));

    let updated_progress = store
        .get_consolidation_progress(&session_id)
        .await
        .expect("updated progress");
    assert_eq!(updated_progress, Some(4));
}

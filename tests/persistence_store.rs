//! Store-level guards: schema versions, ordering, ownership, diagnostics.
//!
//! Every test runs against a disposable database. `AGENTS.md` §7 requires the
//! default test run to touch nothing real, and these are exactly the tests that
//! would notice.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::PathBuf;
use std::sync::Arc;

use smed::core::checkpoint::SessionCheckpoint;
use smed::core::command::SmedCommand;
use smed::core::event::{RunId, SessionId, SmedEvent};
use smed::core::message::{CanonicalMessage, ContentBlock, ToolCall};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::{EventStore, IntegrityReport, StoreDiagnostics, StoreError};
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::sqlite::SqliteEventStore;
use tempfile::TempDir;

/// Spelled out rather than read from the crate, deliberately: importing
/// `SCHEMA_VERSION` would make every assertion here tautological, and this
/// number's whole job is to fail when someone bumps the schema without thinking
/// about migration. Raise it in the same commit that adds the migration.
///
/// 5: the D4 producer's `workspace_search` rebuild (migration 5).
const EXPECTED_SCHEMA_VERSION: u32 = 5;

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let database = directory.path().join("smed.sqlite3");
        let workspace = directory
            .path()
            .canonicalize()
            .expect("canonical")
            .join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        Self {
            _directory: directory,
            database,
            workspace,
        }
    }

    async fn store(&self) -> SqliteEventStore {
        SqliteEventStore::open(&self.database).await.expect("open")
    }
}

async fn open_session(store: &SqliteEventStore, root: &std::path::Path) -> SessionId {
    let project = store
        .open_project(root.to_path_buf())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "test".to_owned(), None)
        .await
        .expect("session");
    store
        .append(SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("append");
    session
}

// ---------------------------------------------------------------------------
// Schema and migrations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_empty_database_is_created_and_migrated_on_first_open() {
    let fixture = Fixture::new();
    assert!(!fixture.database.exists());

    let store = fixture.store().await;
    let report = store.report().await.expect("diagnostics");

    assert!(
        fixture.database.exists(),
        "opening must create the database"
    );
    assert_eq!(report.schema_version, report.supported_schema_version);
    assert_eq!(report.journal_mode.to_lowercase(), "wal", " requires WAL");
    assert!(report.foreign_keys, " requires foreign keys");
    assert!(
        report.busy_timeout_ms > 0,
        " requires a finite busy timeout, and zero is not finite — it is instant failure"
    );
}

#[tokio::test]
async fn reopening_an_existing_database_migrates_nothing_and_keeps_its_data() {
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        open_session(&store, &fixture.workspace).await
    };

    // The second open must be idempotent: if it re-ran the migration, CREATE
    // TABLE would fail and this would not return a store at all.
    let store = fixture.store().await;
    let events = store.events(session).await.expect("events");

    assert_eq!(events.len(), 1, "reopening must not lose data");
    assert_eq!(
        store.report().await.expect("diagnostics").schema_version,
        EXPECTED_SCHEMA_VERSION,
        "an up-to-date database must not be re-migrated"
    );
}

#[tokio::test]
async fn a_database_from_a_newer_smed_is_refused_rather_than_read() {
    // Fail closed (AGENTS.md §1.2): a build that reads a newer schema
    // best-effort drops the columns it does not understand on its next write.
    let fixture = Fixture::new();
    {
        let store = fixture.store().await;
        open_session(&store, &fixture.workspace).await;
        drop(store);
    }

    bump_user_version(&fixture.database, 99);

    match SqliteEventStore::open(&fixture.database).await {
        Err(StoreError::UnsupportedSchema { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, EXPECTED_SCHEMA_VERSION);
        }
        Err(other) => panic!("expected an unsupported-schema refusal, got {other:?}"),
        Ok(_) => panic!("a database from a newer smed must not be opened"),
    }
}

#[tokio::test]
async fn refusing_a_newer_schema_does_not_switch_its_journal_mode() {
    // WAL is persistent database state. Compatibility must be checked before
    // changing it, otherwise a build that promises to refuse a newer database
    // still mutates that database on the way out.
    let fixture = Fixture::new();
    {
        let connection = tokio_rusqlite_connection(&fixture.database);
        connection
            .pragma_update(None, "user_version", 99)
            .expect("future version");
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal_mode.to_lowercase(), "delete");
    }

    assert!(matches!(
        SqliteEventStore::open(&fixture.database).await,
        Err(StoreError::UnsupportedSchema {
            found: 99,
            supported: EXPECTED_SCHEMA_VERSION
        })
    ));

    let connection = tokio_rusqlite_connection(&fixture.database);
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal mode after refusal");
    assert_eq!(
        journal_mode.to_lowercase(),
        "delete",
        "refusing a newer schema must leave persistent pragmas untouched"
    );
}

/// Rewrite `user_version` out of band, as a future smed would.
fn bump_user_version(path: &std::path::Path, version: u32) {
    let connection = tokio_rusqlite_connection(path);
    connection
        .pragma_update(None, "user_version", version)
        .expect("bump");
}

/// A raw connection for tests that must corrupt the database on purpose.
///
/// Uses the same `rusqlite` the store uses — via `tokio-rusqlite`'s re-export —
/// so there is exactly one SQLite in the binary (`docs/persistence.md` §1.4).
fn tokio_rusqlite_connection(path: &std::path::Path) -> tokio_rusqlite::rusqlite::Connection {
    tokio_rusqlite::rusqlite::Connection::open(path).expect("raw connection")
}

// ---------------------------------------------------------------------------
// Ordering guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_duplicate_event_id_is_refused_by_the_database() {
    // `finish_task` cites event ids as evidence. Two events sharing one id would
    // make evidence ambiguous, so the constraint lives in the schema rather than
    // in a check a future writer might skip.
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        open_session(&store, &fixture.workspace).await
    };

    let connection = tokio_rusqlite_connection(&fixture.database);
    let existing: String = connection
        .query_row("SELECT event_id FROM events LIMIT 1", [], |row| row.get(0))
        .expect("an event");

    let duplicate = connection.execute(
        "INSERT INTO events (session_id, sequence, event_id, kind, occurred_at, schema_version, payload_json)
         VALUES (?1, 99, ?2, 'run_started', '2026-01-01T00:00:00Z', 1, '{}')",
        tokio_rusqlite::rusqlite::params![session.to_string(), existing],
    );

    assert!(
        duplicate.is_err(),
        "a duplicate event id must be refused by the UNIQUE constraint"
    );
}

#[tokio::test]
async fn a_gap_in_stored_history_is_refused_rather_than_papered_over() {
    // A hole means an event was lost. Returning the remainder would present an
    // incomplete transcript as a complete one (AGENTS.md §1.3).
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        let session = open_session(&store, &fixture.workspace).await;
        let run = RunId::new();
        for event in [
            SmedEvent::RunStarted { session, run },
            SmedEvent::RunFinished {
                session,
                run,
                reason: smed::core::event::FinishReason::Stop,
            },
        ] {
            store.append(event).await.expect("append");
        }
        session
    };

    // Delete the middle event, leaving 0 and 2.
    let connection = tokio_rusqlite_connection(&fixture.database);
    connection
        .execute(
            "DELETE FROM events WHERE session_id = ?1 AND sequence = 1",
            tokio_rusqlite::rusqlite::params![session.to_string()],
        )
        .expect("delete");
    drop(connection);

    let store = fixture.store().await;
    match store.events(session).await {
        Err(StoreError::SequenceGap {
            session: found,
            missing,
        }) => {
            assert_eq!(found, session);
            assert_eq!(missing, 1);
        }
        Err(other) => panic!("expected a sequence gap, got {other:?}"),
        Ok(events) => panic!(
            "a history with a hole must be refused, not returned as {} events",
            events.len()
        ),
    }
}

#[tokio::test]
async fn a_missing_final_event_is_refused_as_a_sequence_gap() {
    // There is no later row to expose a missing tail from inside the row loop.
    // The sessions.last_sequence mirror is the durable terminal boundary.
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        let session = open_session(&store, &fixture.workspace).await;
        let run = RunId::new();
        store
            .append(SmedEvent::RunStarted { session, run })
            .await
            .expect("started");
        store
            .append(SmedEvent::RunFinished {
                session,
                run,
                reason: smed::core::event::FinishReason::Stop,
            })
            .await
            .expect("finished");
        session
    };

    let connection = tokio_rusqlite_connection(&fixture.database);
    connection
        .execute(
            "DELETE FROM events WHERE session_id = ?1 AND sequence = 2",
            tokio_rusqlite::rusqlite::params![session.to_string()],
        )
        .expect("delete final event");
    drop(connection);

    let store = fixture.store().await;
    assert!(matches!(
        store.events(session).await,
        Err(StoreError::SequenceGap {
            session: found,
            missing: 2
        }) if found == session
    ));
}

#[tokio::test]
async fn sequences_are_contiguous_and_per_session() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let first = open_session(&store, &fixture.workspace).await;
    let second = open_session(&store, &fixture.workspace).await;

    let a = store
        .append(SmedEvent::RunStarted {
            session: first,
            run: RunId::new(),
        })
        .await
        .expect("append");
    let b = store
        .append(SmedEvent::RunStarted {
            session: second,
            run: RunId::new(),
        })
        .await
        .expect("append");

    assert_eq!(a.sequence, 1, "the second event of session one");
    assert_eq!(b.sequence, 1, "sequences are per-session, not global");
}

// ---------------------------------------------------------------------------
// Checkpoint integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_checkpoint_cannot_claim_more_events_than_are_durable() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;
    store
        .write_checkpoint(SessionCheckpoint::empty(session))
        .await
        .expect("checkpoint");

    let connection = tokio_rusqlite_connection(&fixture.database);
    connection
        .execute(
            "UPDATE checkpoints SET sequence = 2 WHERE session_id = ?1",
            tokio_rusqlite::rusqlite::params![session.to_string()],
        )
        .expect("corrupt extent");
    drop(connection);

    assert!(
        matches!(
            store.latest_checkpoint(session).await,
            Err(StoreError::Decode { .. })
        ),
        "a checkpoint beyond durable history must be refused"
    );
}

#[tokio::test]
async fn a_checkpoint_payload_cannot_impersonate_another_session() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;
    store
        .write_checkpoint(SessionCheckpoint::empty(session))
        .await
        .expect("checkpoint");

    let connection = tokio_rusqlite_connection(&fixture.database);
    let state_json: String = connection
        .query_row(
            "SELECT state_json FROM checkpoints WHERE session_id = ?1",
            tokio_rusqlite::rusqlite::params![session.to_string()],
            |row| row.get(0),
        )
        .expect("checkpoint payload");
    let mut payload: serde_json::Value = serde_json::from_str(&state_json).expect("valid json");
    payload["session"] = serde_json::Value::String(SessionId::new().to_string());
    connection
        .execute(
            "UPDATE checkpoints SET state_json = ?2 WHERE session_id = ?1",
            tokio_rusqlite::rusqlite::params![session.to_string(), payload.to_string()],
        )
        .expect("corrupt identity");
    drop(connection);

    assert!(
        matches!(
            store.latest_checkpoint(session).await,
            Err(StoreError::Decode { .. })
        ),
        "a checkpoint payload for another session must be refused"
    );
}

// ---------------------------------------------------------------------------
// Ephemeral events never reach the database
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_text_delta_has_no_persisted_form() {
    //  forbids one row per token. The wire format has no delta variant, so
    // this is refused rather than silently written.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;

    let refused = store
        .append(SmedEvent::TextDelta {
            session,
            run: RunId::new(),
            text: "tok".to_owned(),
        })
        .await;

    assert!(
        matches!(refused, Err(StoreError::Decode { .. })),
        "an ephemeral event reaching the store is a bug and must surface, not vanish"
    );
    assert_eq!(
        store.events(session).await.expect("events").len(),
        1,
        "no delta row may exist"
    );
}

#[tokio::test]
async fn a_streamed_answer_produces_one_message_row_not_one_per_fragment() {
    // The end-to-end version of the same rule: the fake streams its answer in
    // fragments, and exactly one durable message must result.
    let fixture = Fixture::new();
    let store = Arc::new(fixture.store().await);
    let session;

    {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
        let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
        runtime
            .dispatch(SmedCommand::OpenProject {
                root: fixture.workspace.clone(),
            })
            .await
            .expect("open project");
        runtime
            .dispatch(SmedCommand::CreateSession {
                provider: ProviderId::new(FakeProvider::ID),
                model: ModelId::new(FakeProvider::MODEL),
            })
            .await
            .expect("create session");
        runtime
            .dispatch(SmedCommand::SendUserMessage {
                text: "hello".to_owned(),
                source: smed::core::directive::DirectiveSource::Human,
            })
            .await
            .expect("send");

        for _ in 0..400 {
            if runtime
                .snapshot()
                .messages
                .iter()
                .any(|message| message.role == smed::core::message::Role::Assistant)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        session = runtime.snapshot().session.expect("session");
        runtime.close().await.expect("close");
    }

    let events = store.events(session).await.expect("events");
    let assistant_rows = events
        .iter()
        .filter(|stored| {
            matches!(
                &stored.event,
                SmedEvent::MessageAppended { message, .. }
                    if message.role == smed::core::message::Role::Assistant
            )
        })
        .count();

    assert_eq!(
        assistant_rows, 1,
        "the streamed answer must coalesce into exactly one durable message"
    );

    // And the coalesced text is whole, not a fragment.
    let text = events
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::MessageAppended { message, .. }
                if message.role == smed::core::message::Role::Assistant =>
            {
                Some(message.text())
            }
            _ => None,
        })
        .expect("an assistant message");
    assert!(
        text.len() > 1,
        "a coalesced message must hold the whole answer, got {text:?}"
    );
}

// ---------------------------------------------------------------------------
// Ownership
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_writer_is_refused_and_told_who_holds_the_session() {
    // SQLite serialises writers to the *file*; nothing in SQLite stops two smed
    // processes interleaving runs into one *session*. This is that gate
    // (`docs/persistence.md` §5).
    let fixture = Fixture::new();
    let first = fixture.store().await;
    let session = open_session(&first, &fixture.workspace).await;

    let lease = first.acquire_session(session).await.expect("first lease");

    // A second store on the same file, as a second process would be.
    let second = fixture.store().await;
    match second.acquire_session(session).await {
        Err(StoreError::SessionOwned {
            session: found,
            holder,
        }) => {
            assert_eq!(found, session);
            assert!(
                holder.contains("pid"),
                "the refusal must name the holder so a user can check: got {holder}"
            );
        }
        Err(other) => panic!("expected an ownership refusal, got {other:?}"),
        Ok(_) => panic!("two writers on one session is exactly the split brain this prevents"),
    }

    // Released, the session is available again.
    first.release_session(&lease).await.expect("release");
    second
        .acquire_session(session)
        .await
        .expect("a released session must be acquirable");
}

#[tokio::test]
async fn a_lease_left_by_a_crash_is_reclaimed_only_by_an_explicit_act() {
    // smed cannot prove the holder is dead, so it does not steal the lease. The
    // explicit act is `smed sessions release`.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;

    store.acquire_session(session).await.expect("lease");
    drop(store);

    // A new process. The lease row is still there.
    let store = fixture.store().await;
    assert!(
        matches!(
            store.acquire_session(session).await,
            Err(StoreError::SessionOwned { .. })
        ),
        "a stale lease must not be taken silently"
    );

    let listed = store.sessions().await.expect("sessions");
    assert!(
        listed
            .iter()
            .any(|summary| summary.id == session && summary.leased),
        "`smed sessions list` must show the held lease"
    );

    store.break_lease(session).await.expect("break");
    store
        .acquire_session(session)
        .await
        .expect("an explicitly released session must be acquirable");
}

#[tokio::test]
async fn a_lease_cannot_be_released_by_a_process_that_no_longer_holds_it() {
    // After an explicit break, the previous holder must not delete the new
    // holder's row on its way out.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;

    let stale = store.acquire_session(session).await.expect("lease");
    store.break_lease(session).await.expect("break");
    let current = store.acquire_session(session).await.expect("new lease");
    assert_ne!(stale.token, current.token);

    store.release_session(&stale).await.expect("stale release");

    assert!(
        matches!(
            store.acquire_session(session).await,
            Err(StoreError::SessionOwned { .. })
        ),
        "a stale release must not free the current holder's lease"
    );
}

#[tokio::test]
async fn leasing_an_unknown_session_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    assert!(matches!(
        store.acquire_session(SessionId::new()).await,
        Err(StoreError::UnknownSession { .. })
    ));
}

#[tokio::test]
async fn an_ended_session_cannot_be_leased_again() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;
    store.end_session(session).await.expect("end session");

    match store.acquire_session(session).await {
        Err(StoreError::Unavailable { detail }) => {
            assert!(
                detail.contains("ended"),
                "refusal must explain why: {detail}"
            );
        }
        Err(other) => panic!("expected ended-session refusal, got {other:?}"),
        Ok(_) => panic!("an ended session must never acquire a write lease"),
    }
}

#[tokio::test]
async fn ending_an_unknown_session_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    assert!(matches!(
        store.end_session(SessionId::new()).await,
        Err(StoreError::UnknownSession { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn a_non_utf8_project_root_is_explicitly_refused() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let root = fixture.workspace.join(std::ffi::OsString::from_vec(vec![
        b'r', b'o', b'o', b't', 0xff,
    ]));
    let store = fixture.store().await;

    let error = store
        .open_project(root)
        .await
        .expect_err("SQLite text paths must refuse non-UTF-8 roots");
    assert!(
        error.to_string().contains("valid UTF-8"),
        "the limitation must be explicit: {error}"
    );
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_report_the_database_state() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;
    store
        .write_checkpoint(SessionCheckpoint::empty(session))
        .await
        .expect("checkpoint");
    store.acquire_session(session).await.expect("lease");

    let report = store.report().await.expect("diagnostics");

    assert_eq!(report.database_path, fixture.database);
    assert_eq!(report.schema_version, EXPECTED_SCHEMA_VERSION);
    assert_eq!(report.journal_mode.to_lowercase(), "wal");
    assert!(report.foreign_keys);
    assert_eq!(report.sessions, 1);
    assert_eq!(report.events, 1);
    assert_eq!(report.checkpoints, 1);
    assert_eq!(report.leased_sessions, 1);
    assert!(report.page_size > 0 && report.page_count > 0);
}

#[tokio::test]
async fn integrity_is_an_explicit_action_and_passes_on_a_healthy_database() {
    //  forbids running this on every launch: it is O(N log N). It exists
    // as a diagnostic a human asks for.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    open_session(&store, &fixture.workspace).await;

    assert_eq!(
        store.integrity_check().await.expect("integrity"),
        IntegrityReport::Ok
    );
}

#[tokio::test]
async fn sessions_are_listed_newest_first_with_their_model_and_project() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let older = open_session(&store, &fixture.workspace).await;
    let newer = open_session(&store, &fixture.workspace).await;

    let listed = store.sessions().await.expect("sessions");
    let ids: Vec<SessionId> = listed.iter().map(|summary| summary.id).collect();

    assert_eq!(ids, vec![newer, older], "newest first");
    assert_eq!(
        listed[0].model.as_ref().map(ModelId::as_str),
        Some(FakeProvider::MODEL),
        "the active model must be listable without replaying history"
    );
    assert_eq!(listed[0].project_root, fixture.workspace);
    assert_eq!(listed[0].event_count, 1);
    assert!(!listed[0].leased);
}

#[tokio::test]
async fn one_project_row_is_shared_by_every_session_in_it() {
    let fixture = Fixture::new();
    let store = fixture.store().await;

    let first = store
        .open_project(fixture.workspace.clone())
        .await
        .expect("project");
    let second = store
        .open_project(fixture.workspace.clone())
        .await
        .expect("project");

    assert_eq!(
        first, second,
        "opening the same root twice must not create a second project"
    );
    assert_eq!(store.report().await.expect("report").sessions, 0);
}

#[tokio::test]
async fn a_tool_call_survives_the_wire_format_intact() {
    // The round trip that matters most: a stored tool call must come back with
    // its correlation id and arguments exactly, or a resumed session cannot
    // match results to calls.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = open_session(&store, &fixture.workspace).await;

    let call = ToolCall {
        id: "call_abc".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({ "path": "src/lib.rs", "limit": 40 }),
        provider_signature: None,
    };
    store
        .append(SmedEvent::MessageAppended {
            session,
            message: Box::new(CanonicalMessage::assistant(
                vec![ContentBlock::ToolCall(call.clone())],
                ProviderId::new(FakeProvider::ID),
                ModelId::new(FakeProvider::MODEL),
            )),
        })
        .await
        .expect("append");

    let events = store.events(session).await.expect("events");
    let restored = events
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::MessageAppended { message, .. } => message.tool_calls().next().cloned(),
            _ => None,
        })
        .expect("a tool call");

    assert_eq!(restored, call);
}

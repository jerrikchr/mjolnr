//! Regression locks for Phase 4 recovery boundaries found during checkpoint review.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "AGENTS.md section 7: tests may panic freely"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mjolnr::core::checkpoint::SessionCheckpoint;
use mjolnr::core::command::{ApprovalDecision, ApprovalId, MjolnrCommand};
use mjolnr::core::error::{ProviderError, ReasonCode};
use mjolnr::core::event::{
    FinishReason, MjolnrEvent, ProviderEvent, RunId, SessionId, StoredEvent,
};
use mjolnr::core::message::{CanonicalMessage, ContentBlock, ToolCall, ToolOutcome, ToolResult};
use mjolnr::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use mjolnr::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSnapshot};
use mjolnr::core::store::{
    EventStore, ProjectId, SessionLease, SessionStatus, SessionSummary, StoreError,
    StoredCheckpoint,
};
use mjolnr::core::tool::ToolTier;
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::runtime::recovery::project;
use mjolnr::store::memory::InMemoryEventStore;
use mjolnr::store::sqlite::SqliteEventStore;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

const HANGING_PROVIDER: &str = "recovery-hanging";
const HANGING_MODEL: &str = "recovery-hanging-1";
const COMMAND_PROVIDER: &str = "recovery-command";
const COMMAND_MODEL: &str = "recovery-command-1";

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temporary directory");
        let database = directory.path().join("mjolnr.sqlite3");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        Self {
            _directory: directory,
            database,
            workspace,
        }
    }

    async fn store(&self) -> Arc<SqliteEventStore> {
        Arc::new(
            SqliteEventStore::open(&self.database)
                .await
                .expect("SQLite store"),
        )
    }
}

async fn create_session(store: &dyn EventStore, root: &Path) -> SessionId {
    let project = store
        .open_project(root.to_path_buf())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "regression".to_owned(), None)
        .await
        .expect("session");
    session
}

fn created(session: SessionId, provider: &str, model: &str) -> MjolnrEvent {
    MjolnrEvent::SessionCreated {
        session,
        provider: ProviderId::new(provider),
        model: ModelId::new(model),
    }
}

async fn append_all(store: &dyn EventStore, events: impl IntoIterator<Item = MjolnrEvent>) {
    for event in events {
        store.append(event).await.expect("append event");
    }
}

async fn wait_snapshot(
    runtime: &Runtime,
    ready: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    let mut snapshots = runtime.snapshots();
    let current = runtime.snapshot();
    if ready(&current) {
        return current;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime remains open");
            if ready(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .expect("timed out waiting for runtime state")
}

async fn wait_event(
    events: &mut mjolnr::core::runtime::RuntimeSubscription,
    ready: impl Fn(&MjolnrEvent) -> bool,
) -> MjolnrEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if ready(&event) {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for runtime event")
}

fn fake_runtime(store: &Arc<SqliteEventStore>) -> Runtime {
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    Runtime::spawn(vec![provider], Arc::clone(store) as Arc<dyn EventStore>)
}

#[derive(Debug)]
struct HangingProvider {
    started: watch::Sender<bool>,
}

#[async_trait]
impl Provider for HangingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(HANGING_PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![descriptor(HANGING_PROVIDER, HANGING_MODEL)]
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let _ = self.started.send(true);
        cancel.cancelled().await;
        Err(ProviderError::Cancelled)
    }
}

#[derive(Debug)]
struct CommandProvider;

#[async_trait]
impl Provider for CommandProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(COMMAND_PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![descriptor(COMMAND_PROVIDER, COMMAND_MODEL)]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let has_result = request.messages.iter().any(|message| {
            message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        });
        let reason = if has_result {
            FinishReason::Stop
        } else {
            let call = ToolCall {
                id: "call_escape".to_owned(),
                name: "run_command".to_owned(),
                arguments: serde_json::json!({
                    "program": "/bin/sh",
                    "arguments": ["-c", "printf escaped > escaped.txt"]
                }),
                provider_signature: None,
            };
            for event in [
                ProviderEvent::ToolCallStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                },
                ProviderEvent::ToolArgumentsDelta {
                    id: call.id.clone(),
                    fragment: call.arguments.to_string(),
                },
                ProviderEvent::ToolCallCompleted { call },
            ] {
                events
                    .send(event)
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
            }
            FinishReason::ToolCalls
        };
        events
            .send(ProviderEvent::Finished { reason })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason,
            usage: None,
        })
    }
}

fn descriptor(provider: &str, model: &str) -> ModelDescriptor {
    ModelDescriptor {
        id: ModelId::new(model),
        provider: ProviderId::new(provider),
        display_name: "recovery regression".to_owned(),
        capabilities: ModelCapabilities::text_and_tools(),
        context_tokens: Some(1_024),
        max_output_tokens: Some(1_024),
        tier: None,
    }
}

#[tokio::test]
async fn close_during_active_provider_resumes_into_recovery() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (started, mut observed) = watch::channel(false);
    let provider: Arc<dyn Provider> = Arc::new(HangingProvider { started });
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(HANGING_PROVIDER),
            model: ModelId::new(HANGING_MODEL),
        })
        .await
        .expect("create session");
    let session = wait_snapshot(&runtime, |state| state.session.is_some())
        .await
        .session
        .expect("session id");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "wait".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start provider");
    tokio::time::timeout(Duration::from_secs(5), observed.wait_for(|value| *value))
        .await
        .expect("provider started")
        .expect("watch remains open");
    runtime.close().await.expect("close");

    let resumed = Runtime::spawn(
        vec![Arc::new(HangingProvider {
            started: watch::channel(false).0,
        })],
        Arc::clone(&store) as Arc<dyn EventStore>,
    );
    resumed
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");
    let state = wait_snapshot(&resumed, |state| state.session == Some(session)).await;
    assert!(
        state.recovery.is_required(),
        "closing an in-flight provider turn must not checkpoint away its interruption"
    );
    let _ = resumed.close().await;
}

#[tokio::test]
async fn ending_an_active_run_cannot_erase_uncertain_work() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (started, mut observed) = watch::channel(false);
    let provider: Arc<dyn Provider> = Arc::new(HangingProvider { started });
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(HANGING_PROVIDER),
            model: ModelId::new(HANGING_MODEL),
        })
        .await
        .expect("create session");
    let session = wait_snapshot(&runtime, |state| state.session.is_some())
        .await
        .session
        .expect("session id");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "wait".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start provider");
    tokio::time::timeout(Duration::from_secs(5), observed.wait_for(|value| *value))
        .await
        .expect("provider started")
        .expect("watch remains open");

    runtime
        .dispatch(MjolnrCommand::EndSession)
        .await
        .expect("queue end request");
    runtime.close().await.expect("close");

    let summary = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|summary| summary.id == session)
        .expect("session row");
    assert_eq!(summary.status, SessionStatus::Active);
    assert!(
        store
            .events(session)
            .await
            .expect("events")
            .iter()
            .all(|event| !matches!(event.event, MjolnrEvent::SessionEnded { .. })),
        "an active run must remain recoverable instead of being sealed as ended"
    );
}

#[tokio::test]
async fn unresolved_recovery_survives_close_and_second_resume() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = create_session(store.as_ref(), &fixture.workspace).await;
    let run = RunId::new();
    let approval = ApprovalId::new();
    let call = ToolCall {
        id: "call_1".to_owned(),
        name: "write_file".to_owned(),
        arguments: serde_json::json!({"path": "a", "content": "b"}),
        provider_signature: None,
    };
    append_all(
        store.as_ref(),
        [
            created(session, FakeProvider::ID, FakeProvider::MODEL),
            MjolnrEvent::RunStarted { session, run },
            MjolnrEvent::ToolProposed {
                session,
                run,
                approval: Some(approval),
                call,
                tier: ToolTier::Write,
                preview: "+ b".to_owned(),
            },
            MjolnrEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision: ApprovalDecision::ApproveOnce,
            },
        ],
    )
    .await;

    let first = fake_runtime(&store);
    first
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("first resume");
    assert!(
        wait_snapshot(&first, |state| state.recovery.is_required())
            .await
            .recovery
            .is_required()
    );
    first.close().await.expect("close unresolved session");

    let second = fake_runtime(&store);
    second
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("second resume");
    let state = wait_snapshot(&second, |state| state.session == Some(session)).await;
    assert!(
        state.recovery.is_required(),
        "opening and closing must not count as a recovery decision"
    );
    let _ = second.close().await;
}

#[derive(Debug)]
struct CheckpointFailingStore {
    inner: InMemoryEventStore,
}

#[async_trait]
impl EventStore for CheckpointFailingStore {
    async fn open_project(&self, root: PathBuf) -> Result<ProjectId, StoreError> {
        self.inner.open_project(root).await
    }
    async fn create_session(
        &self,
        session: SessionId,
        project: ProjectId,
        title: String,
        parent: Option<SessionId>,
    ) -> Result<(), StoreError> {
        self.inner
            .create_session(session, project, title, parent)
            .await
    }
    async fn end_session(&self, session: SessionId) -> Result<(), StoreError> {
        self.inner.end_session(session).await
    }
    async fn rename_session(&self, session: SessionId, title: String) -> Result<(), StoreError> {
        self.inner.rename_session(session, title).await
    }
    async fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        self.inner.sessions().await
    }
    async fn append(&self, event: MjolnrEvent) -> Result<StoredEvent, StoreError> {
        self.inner.append(event).await
    }
    async fn events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events(session).await
    }
    async fn events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_from(session, from).await
    }
    async fn write_checkpoint(&self, _: SessionCheckpoint) -> Result<u64, StoreError> {
        Err(StoreError::Unavailable {
            detail: "checkpoint disk full".to_owned(),
        })
    }
    async fn latest_checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<StoredCheckpoint>, StoreError> {
        self.inner.latest_checkpoint(session).await
    }
    async fn find_session_by_dir(
        &self,
        project_root: std::path::PathBuf,
    ) -> Result<Option<SessionId>, StoreError> {
        self.inner.find_session_by_dir(project_root).await
    }
    async fn search_workspace(
        &self,
        filter: mjolnr::core::store::WorkspaceSearchFilter,
    ) -> Result<mjolnr::core::store::WorkspaceSearchPage, StoreError> {
        self.inner.search_workspace(filter).await
    }
    async fn acquire_session(&self, session: SessionId) -> Result<SessionLease, StoreError> {
        self.inner.acquire_session(session).await
    }
    async fn release_session(&self, lease: &SessionLease) -> Result<(), StoreError> {
        self.inner.release_session(lease).await
    }
    async fn break_lease(&self, session: SessionId) -> Result<(), StoreError> {
        self.inner.break_lease(session).await
    }
    async fn flush(&self) -> Result<(), StoreError> {
        self.inner.flush().await
    }
}

#[tokio::test]
async fn checkpoint_write_failure_makes_close_fail() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(CheckpointFailingStore {
        inner: InMemoryEventStore::new(),
    });
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    wait_snapshot(&runtime, |state| state.session.is_some()).await;
    assert!(
        runtime.close().await.is_err(),
        "close must not report success when its checkpoint was rejected"
    );
}

#[tokio::test]
async fn orphaned_tool_outcomes_restore_canonical_result_messages() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    for failed in [false, true] {
        let session = create_session(store.as_ref(), &fixture.workspace).await;
        let run = RunId::new();
        let call = ToolCall {
            id: format!("call_{failed}"),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({"path": "a"}),
            provider_signature: None,
        };
        let mut events = vec![
            created(session, FakeProvider::ID, FakeProvider::MODEL),
            MjolnrEvent::RunStarted { session, run },
            MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::assistant(
                    vec![ContentBlock::ToolCall(call.clone())],
                    ProviderId::new(FakeProvider::ID),
                    ModelId::new(FakeProvider::MODEL),
                )),
            },
            MjolnrEvent::ToolProposed {
                session,
                run,
                approval: None,
                call: call.clone(),
                tier: ToolTier::Read,
                preview: "a".to_owned(),
            },
        ];
        events.push(if failed {
            MjolnrEvent::ToolFailed {
                session,
                run,
                call_id: call.id.clone(),
                name: call.name.clone(),
                code: ReasonCode::ToolExecution,
                detail: "read failed".to_owned(),
            }
        } else {
            MjolnrEvent::ToolCompleted {
                session,
                run,
                call_id: call.id.clone(),
                name: call.name.clone(),
                result: ToolResult::ok("read ok"),
            }
        });
        append_all(store.as_ref(), events).await;

        let stored = store.events(session).await.expect("events");
        let recovered = project(None, &[], &stored).expect("recovery projection");
        let result = recovered
            .state
            .messages()
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                ContentBlock::ToolResult {
                    call_id,
                    name,
                    result,
                } if call_id == &call.id && name == &call.name => Some(result),
                _ => None,
            })
            .expect("durable tool outcome must become a canonical result message");
        if failed {
            assert_eq!(
                result.outcome,
                ToolOutcome::Failed(ReasonCode::ToolExecution)
            );
        } else {
            assert_eq!(result.outcome, ToolOutcome::Ok);
        }
    }
}

#[tokio::test]
async fn ended_session_does_not_accept_new_work_after_resume() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = create_session(store.as_ref(), &fixture.workspace).await;
    append_all(
        store.as_ref(),
        [
            created(session, FakeProvider::ID, FakeProvider::MODEL),
            MjolnrEvent::SessionEnded { session },
        ],
    )
    .await;
    store.end_session(session).await.expect("end row");
    store
        .write_checkpoint(SessionCheckpoint {
            status: SessionStatus::Ended,
            project_root: Some(fixture.workspace.clone()),
            provider: Some(ProviderId::new(FakeProvider::ID)),
            model: Some(ModelId::new(FakeProvider::MODEL)),
            ..SessionCheckpoint::empty(session)
        })
        .await
        .expect("checkpoint ended session");
    let before = store.events(session).await.expect("history").len();

    let runtime = fake_runtime(&store);
    let _ = runtime
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await;
    let _ = runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "resurrect".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await;
    let _ = runtime.close().await;
    assert_eq!(
        store.events(session).await.expect("history").len(),
        before,
        "an ended session must not append a new user message or run"
    );
}

#[tokio::test]
async fn tail_sequence_gap_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = create_session(store.as_ref(), &fixture.workspace).await;
    let run = RunId::new();
    append_all(
        store.as_ref(),
        [
            created(session, FakeProvider::ID, FakeProvider::MODEL),
            MjolnrEvent::RunStarted { session, run },
            MjolnrEvent::RunFinished {
                session,
                run,
                reason: FinishReason::Stop,
            },
        ],
    )
    .await;
    let connection = raw_connection(&fixture.database);
    connection
        .execute(
            "DELETE FROM events WHERE session_id = ?1 AND sequence = 2",
            tokio_rusqlite::rusqlite::params![session.to_string()],
        )
        .expect("delete tail event");
    drop(connection);

    assert!(
        matches!(
            store.events(session).await,
            Err(StoreError::SequenceGap { missing: 2, .. })
        ),
        "sessions.last_sequence proves the final event is missing"
    );
}

#[tokio::test]
async fn mismatched_checkpoint_extent_or_session_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let first = create_session(store.as_ref(), &fixture.workspace).await;
    let second = create_session(store.as_ref(), &fixture.workspace).await;
    append_all(
        store.as_ref(),
        [
            created(first, FakeProvider::ID, FakeProvider::MODEL),
            created(second, FakeProvider::ID, FakeProvider::MODEL),
        ],
    )
    .await;
    store
        .write_checkpoint(SessionCheckpoint::empty(first))
        .await
        .expect("first checkpoint");
    store
        .write_checkpoint(SessionCheckpoint::empty(second))
        .await
        .expect("second checkpoint");
    let connection = raw_connection(&fixture.database);
    let second_json: String = connection
        .query_row(
            "SELECT state_json FROM checkpoints WHERE session_id = ?1",
            [second.to_string()],
            |row| row.get(0),
        )
        .expect("second checkpoint JSON");
    connection
        .execute(
            "UPDATE checkpoints SET state_json = ?2 WHERE session_id = ?1",
            tokio_rusqlite::rusqlite::params![first.to_string(), second_json],
        )
        .expect("cross-wire checkpoint session");
    drop(connection);
    assert!(
        store.latest_checkpoint(first).await.is_err(),
        "checkpoint payload session must match its row"
    );

    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = create_session(store.as_ref(), &fixture.workspace).await;
    append_all(
        store.as_ref(),
        [created(session, FakeProvider::ID, FakeProvider::MODEL)],
    )
    .await;
    store
        .write_checkpoint(SessionCheckpoint::empty(session))
        .await
        .expect("checkpoint");
    let connection = raw_connection(&fixture.database);
    connection
        .execute(
            "UPDATE checkpoints SET sequence = 9 WHERE session_id = ?1",
            [session.to_string()],
        )
        .expect("corrupt checkpoint extent");
    drop(connection);
    assert!(
        store.latest_checkpoint(session).await.is_err(),
        "checkpoint extent cannot exceed durable history"
    );
}

fn raw_connection(path: &Path) -> tokio_rusqlite::rusqlite::Connection {
    tokio_rusqlite::rusqlite::Connection::open(path).expect("raw SQLite connection")
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_root_symlink_cannot_run_command_outside_workspace() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let outside = fixture
        .database
        .parent()
        .expect("fixture parent")
        .join("outside");
    std::fs::create_dir(&outside).expect("outside directory");
    let store = fixture.store().await;
    let session = create_session(store.as_ref(), &fixture.workspace).await;
    append_all(
        store.as_ref(),
        [created(session, COMMAND_PROVIDER, COMMAND_MODEL)],
    )
    .await;
    store
        .write_checkpoint(SessionCheckpoint {
            project_root: Some(fixture.workspace.clone()),
            provider: Some(ProviderId::new(COMMAND_PROVIDER)),
            model: Some(ModelId::new(COMMAND_MODEL)),
            ..SessionCheckpoint::empty(session)
        })
        .await
        .expect("checkpoint");
    std::fs::remove_dir(&fixture.workspace).expect("remove original root");
    symlink(&outside, &fixture.workspace).expect("replace root with symlink");

    let provider: Arc<dyn Provider> = Arc::new(CommandProvider);
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open current path");
    runtime
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");
    let state = wait_snapshot(&runtime, |state| {
        state.session == Some(session) || state.store_failure.is_some()
    })
    .await;
    if state.store_failure.is_some() {
        assert!(!outside.join("escaped.txt").exists());
        let _ = runtime.close().await;
        return;
    }

    let mut events = runtime.subscribe();
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "run it".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let proposal = wait_event(&mut events, |event| {
        matches!(event, MjolnrEvent::ToolProposed { call, .. } if call.name == "run_command")
            || matches!(event, MjolnrEvent::RunFailed { .. })
    })
    .await;
    if let MjolnrEvent::ToolProposed {
        approval: Some(approval),
        ..
    } = proposal
    {
        runtime
            .dispatch(MjolnrCommand::ResolveApproval {
                approval,
                decision: ApprovalDecision::ApproveOnce,
            })
            .await
            .expect("approve exact proposal");
        wait_event(&mut events, |event| {
            matches!(
                event,
                MjolnrEvent::ToolCompleted { name, .. }
                    | MjolnrEvent::ToolFailed { name, .. } if name == "run_command"
            )
        })
        .await;
    }
    assert!(
        !outside.join("escaped.txt").exists(),
        "a persisted root replaced by a symlink must not become command cwd"
    );
    let _ = runtime.close().await;
}

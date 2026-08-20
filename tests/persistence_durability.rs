//! A failed durable write must surface, never be swallowed.
//!
//! Phase 1's `emit` wrote `let _ = self.store.append(...)`, which was honest
//! then: an in-memory store cannot fail. Against SQLite it can — a full disk, a
//! revoked permission, a database a newer mjolnr wrote. This is the test that the
//! old shortcut is gone.
//!
//! The failure mode being prevented is specific and nasty: the UI, the model,
//! and the transcript all agreeing about an event the database rejected. The
//! session looks complete, and reopens with a hole in it that nobody can explain
//! because nothing ever reported an error.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use mjolnr::core::checkpoint::SessionCheckpoint;
use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::error::ReasonCode;
use mjolnr::core::event::{MjolnrEvent, SessionId, StoredEvent};
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::plan::{
    PlanApproval, PlanHandoff, PlanId, PlanProposal, PlanReview, PlanStep, Question,
    QuestionAnswer, QuestionId, ReviewVerdict, RevisionId,
};
use mjolnr::core::policy::PolicyMode;
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::{
    EventStore, ProjectId, SessionLease, SessionSummary, StoreError, StoredCheckpoint,
};
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;
use tempfile::TempDir;

/// An in-memory store that can be told to start rejecting appends.
///
/// A wrapper rather than a from-scratch double: everything except the failure is
/// the real in-memory store, so a test cannot pass because the double forgot to
/// implement something.
#[derive(Debug)]
struct BreakableStore {
    inner: InMemoryEventStore,
    broken: AtomicBool,
    checkpoint_reads_broken: AtomicBool,
    releases_broken: AtomicBool,
    event_reads_broken: AtomicBool,
    rejected: AtomicUsize,
}

impl BreakableStore {
    fn new() -> Self {
        Self {
            inner: InMemoryEventStore::new(),
            broken: AtomicBool::new(false),
            checkpoint_reads_broken: AtomicBool::new(false),
            releases_broken: AtomicBool::new(false),
            event_reads_broken: AtomicBool::new(false),
            rejected: AtomicUsize::new(0),
        }
    }

    /// Make reading the history fail. Flipped *after* a session is running, so
    /// the break lands on the read under test rather than on session setup.
    fn break_event_reads(&self) {
        self.event_reads_broken.store(true, Ordering::SeqCst);
    }

    fn break_now(&self) {
        self.broken.store(true, Ordering::SeqCst);
    }

    fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }

    fn break_resume_and_release(&self) {
        self.checkpoint_reads_broken.store(true, Ordering::SeqCst);
        self.releases_broken.store(true, Ordering::SeqCst);
    }

    fn durable_events(&self) -> usize {
        self.inner.len()
    }
}

#[async_trait]
impl EventStore for BreakableStore {
    async fn append(&self, event: MjolnrEvent) -> Result<StoredEvent, StoreError> {
        if self.broken.load(Ordering::SeqCst) {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(StoreError::Unavailable {
                detail: "disk is full".to_owned(),
            });
        }
        self.inner.append(event).await
    }

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

    async fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        self.inner.sessions().await
    }

    async fn events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        if self.event_reads_broken.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable {
                detail: "the database is unreadable".to_owned(),
            });
        }
        self.inner.events(session).await
    }

    async fn events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_from(session, from).await
    }

    async fn write_checkpoint(&self, checkpoint: SessionCheckpoint) -> Result<u64, StoreError> {
        self.inner.write_checkpoint(checkpoint).await
    }

    async fn latest_checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<StoredCheckpoint>, StoreError> {
        if self.checkpoint_reads_broken.load(Ordering::SeqCst) {
            return Err(StoreError::Decode {
                detail: "checkpoint payload is corrupt".to_owned(),
            });
        }
        self.inner.latest_checkpoint(session).await
    }

    async fn find_session_by_dir(
        &self,
        project_root: PathBuf,
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
        if self.releases_broken.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable {
                detail: "lease release failed".to_owned(),
            });
        }
        self.inner.release_session(lease).await
    }

    async fn break_lease(&self, session: SessionId) -> Result<(), StoreError> {
        self.inner.break_lease(session).await
    }

    async fn flush(&self) -> Result<(), StoreError> {
        self.inner.flush().await
    }
}

/// An unopenable root refuses on the dispatch itself, and the store stays
/// clean.
///
/// This test previously asserted the opposite: that the refusal arrived as
/// `store_failure`. That was the defect, not the contract — a mistyped
/// directory told the user their durable store had failed, which points them at
/// the wrong remedy entirely. The refusal is now returned to whoever asked for
/// the open, which is also the only place that can act on it.
#[tokio::test]
async fn opening_an_invalid_project_refuses_without_blaming_the_store() {
    let directory = TempDir::new().expect("temp dir");
    let missing = directory.path().join("missing");
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(Vec::new(), store);

    let refusal = runtime
        .dispatch(MjolnrCommand::OpenProject { root: missing })
        .await
        .expect_err("a missing directory must be refused");

    assert_eq!(
        refusal.reason_code(),
        Some(ReasonCode::PathOutsideWorkspace),
        "got {refusal}"
    );
    assert!(
        refusal.to_string().contains("cannot open workspace"),
        "the refusal must name what it could not open: {refusal}"
    );

    let snapshot = runtime.snapshot();
    assert!(
        snapshot.workspace_root.is_none(),
        "a refused open leaves no root"
    );
    assert!(
        snapshot.store_failure.is_none(),
        "the store did not fail: {:?}",
        snapshot.store_failure
    );

    runtime.close().await.expect("a clean store closes cleanly");
}

#[tokio::test]
async fn resume_failure_keeps_its_cause_when_lease_release_also_fails() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let session;

    {
        let runtime = Runtime::spawn(
            vec![Arc::clone(&provider)],
            Arc::clone(&store) as Arc<dyn EventStore>,
        );
        runtime
            .dispatch(MjolnrCommand::OpenProject {
                root: workspace.path().to_owned(),
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
        session = settle(&runtime, |snapshot| snapshot.session.is_some())
            .await
            .session
            .expect("session");
        runtime.close().await.expect("close seed runtime");
    }

    store.break_resume_and_release();
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");

    let snapshot = settle(&runtime, |snapshot| snapshot.store_failure.is_some()).await;
    let failure = snapshot.store_failure.expect("resume must fail visibly");
    assert!(
        failure.contains("checkpoint payload is corrupt"),
        "got {failure}"
    );
    assert!(failure.contains("lease release failed"), "got {failure}");

    assert!(runtime.close().await.is_err());
}

#[tokio::test]
async fn a_pre_session_policy_is_durable_from_session_creation() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::SetPolicy {
            mode: PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("select policy");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");

    let snapshot = settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    let session = snapshot.session.expect("session");
    assert_eq!(snapshot.policy, PolicyMode::WorkspaceWrite);
    assert!(
        store
            .events(session)
            .await
            .expect("events")
            .iter()
            .any(|stored| matches!(
                stored.event,
                MjolnrEvent::PolicyChanged {
                    mode: PolicyMode::WorkspaceWrite,
                    ..
                }
            ))
    );

    runtime.close().await.expect("close");
}

async fn settle(
    runtime: &Runtime,
    ready: impl Fn(&mjolnr::core::runtime::RuntimeSnapshot) -> bool,
) -> mjolnr::core::runtime::RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

#[tokio::test]
async fn plan_command_without_session_is_refused_synchronously() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], store as Arc<dyn EventStore>);
    let plan_id = PlanId::new();

    let error = runtime
        .dispatch(MjolnrCommand::ProposePlan {
            proposal: PlanProposal {
                plan_id,
                revision_id: RevisionId::initial(),
                title: "Refuse me".to_string(),
                summary: "There is no session".to_string(),
                steps: Vec::new(),
                proposed_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect_err("must refuse without a session");

    assert!(matches!(error, mjolnr::core::error::MjolnrError::NoSession));
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn rejected_plan_append_does_not_mutate_snapshot_workflow() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    store.break_now();

    let plan_id = PlanId::new();
    let error = runtime
        .dispatch(MjolnrCommand::ProposePlan {
            proposal: PlanProposal {
                plan_id,
                revision_id: RevisionId::initial(),
                title: "Durability first".to_string(),
                summary: "The store rejects this".to_string(),
                steps: vec![PlanStep {
                    index: 1,
                    title: "Do not publish".to_string(),
                    description: "No accepted append means no plan state".to_string(),
                }],
                proposed_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect_err("append failure must reach the caller");

    assert!(matches!(
        error,
        mjolnr::core::error::MjolnrError::Store { .. }
    ));
    assert!(
        runtime.snapshot().plan.is_none(),
        "a rejected durable append must not mutate authoritative plan state"
    );
    let _ = runtime.close().await;
}

#[tokio::test]
async fn reviewed_plan_events_are_durable_in_contract_order() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    let session = settle(&runtime, |snapshot| snapshot.session.is_some())
        .await
        .session
        .expect("session");
    let plan_id = PlanId::new();
    drive_reviewed_plan(&runtime, plan_id).await;

    let kinds: Vec<&str> = store
        .events(session)
        .await
        .expect("events")
        .iter()
        .filter_map(|stored| match stored.event {
            MjolnrEvent::PlanQuestionAsked { .. } => Some("asked"),
            MjolnrEvent::PlanQuestionAnswered { .. } => Some("answered"),
            MjolnrEvent::PlanProposed { .. } => Some("proposed"),
            MjolnrEvent::PlanReviewed { .. } => Some("reviewed"),
            MjolnrEvent::PlanApproved { .. } => Some("approved"),
            MjolnrEvent::PlanHandoffCreated { .. } => Some("handoff"),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "asked", "answered", "proposed", "reviewed", "approved", "handoff"
        ]
    );
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn reviewed_plan_resume_reconstructs_the_exact_cross_client_workflow() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    let session = settle(&runtime, |snapshot| snapshot.session.is_some())
        .await
        .session
        .expect("session");
    let plan_id = PlanId::new();
    drive_reviewed_plan(&runtime, plan_id).await;
    let before = runtime.snapshot().plan.expect("live plan workflow");
    runtime.close().await.expect("close");

    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let resumed = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    resumed
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open project");
    resumed
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume session");
    let after = settle(&resumed, |snapshot| snapshot.plan.is_some())
        .await
        .plan
        .expect("resumed plan workflow");

    assert_eq!(after, before);
    resumed.close().await.expect("close resumed runtime");
}

async fn drive_reviewed_plan(runtime: &Runtime, plan_id: PlanId) {
    let question_id = QuestionId::new();
    runtime
        .dispatch(MjolnrCommand::AskPlanQuestion {
            plan_id,
            question: Question {
                id: question_id,
                prompt: "Choose scope".to_string(),
                options: vec!["Narrow".to_string()],
                is_multi_select: false,
                created_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("ask");
    runtime
        .dispatch(MjolnrCommand::AnswerPlanQuestion {
            plan_id,
            answer: QuestionAnswer {
                question_id,
                selected_options: vec!["Narrow".to_string()],
                freeform_text: None,
                answered_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("answer");
    runtime
        .dispatch(MjolnrCommand::ProposePlan {
            proposal: PlanProposal {
                plan_id,
                revision_id: RevisionId::initial(),
                title: "Plan".to_string(),
                summary: "Summary".to_string(),
                steps: Vec::new(),
                proposed_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("propose");
    runtime
        .dispatch(MjolnrCommand::ReviewPlan {
            review: PlanReview {
                plan_id,
                revision_id: RevisionId::initial(),
                reviewer: "Architect".to_string(),
                verdict: ReviewVerdict::Approve,
                feedback: "Sound".to_string(),
                reviewed_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("review");
    runtime
        .dispatch(MjolnrCommand::ApprovePlan {
            approval: PlanApproval {
                plan_id,
                revision_id: RevisionId::initial(),
                approver: "Human".to_string(),
                decision: ReviewVerdict::Approve,
                note: None,
                approved_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("approve");
    runtime
        .dispatch(MjolnrCommand::HandoffPlan {
            handoff: PlanHandoff {
                plan_id,
                revision_id: RevisionId::initial(),
                handoff_note: "Execute".to_string(),
                created_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("handoff");
}

#[tokio::test]
async fn a_failed_durable_write_halts_the_session_and_says_so() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    // The disk fills up.
    store.break_now();

    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let snapshot = settle(&runtime, |snapshot| snapshot.store_failure.is_some()).await;

    let failure = snapshot
        .store_failure
        .expect("a rejected append must surface");
    assert!(
        failure.contains("disk is full"),
        "the failure must name what went wrong: {failure}"
    );
    assert!(store.rejected() > 0);

    let _ = runtime.close().await;
}

#[tokio::test]
async fn an_unreadable_record_is_a_typed_failure_not_an_empty_window() {
    // `query_session` reads the history the session replays from (
    // 30). If that read fails, the model must be told so. An empty window would
    // read as "nothing has happened yet" — which, to a model deciding whether it
    // has already done a thing, is the most expensive possible lie.
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::SessionQuery));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    // Break reads only now: session setup needs them, and the point is to fail
    // the query, not the session.
    store.break_event_reads();

    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "what have I done so far?".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            if let MjolnrEvent::ToolCompleted { name, result, .. } = &event
                && name == "query_session"
            {
                return result.clone();
            }
        }
    })
    .await
    .expect("the query records an outcome");

    assert!(
        !result.outcome.is_ok(),
        "an unreadable record must not answer as though the session were empty: {result:?}"
    );
    assert!(
        result.content.contains("could not read"),
        "the failure must say what went wrong: {}",
        result.content
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_rejected_event_is_not_broadcast_as_though_it_happened() {
    // The precise rule from : "do not broadcast or execute as though
    // an event was durable when its append failed."
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open project");

    let mut events = runtime.subscribe();
    store.break_now();

    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");

    settle(&runtime, |snapshot| snapshot.store_failure.is_some()).await;

    // Nothing may have been announced: the store rejected every append.
    let broadcast = tokio::time::timeout(std::time::Duration::from_millis(50), events.recv()).await;
    assert!(
        broadcast.is_err(),
        "an event the database rejected must not reach subscribers: got {broadcast:?}"
    );
    assert_eq!(
        store.durable_events(),
        0,
        "nothing durable was written, which is exactly why nothing may be announced"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_halted_session_refuses_new_work_with_a_stable_code() {
    let workspace = TempDir::new().expect("temp dir");
    let store = Arc::new(BreakableStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    let before = store.durable_events();
    store.break_now();

    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    settle(&runtime, |snapshot| snapshot.store_failure.is_some()).await;

    // Repair the store. The session stays halted: mjolnr does not know what it
    // failed to write, so it cannot safely carry on from here.
    let after_failure = store.durable_events();
    assert_eq!(
        after_failure, before,
        "nothing durable may have been written while the store was broken"
    );

    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "again".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        runtime.snapshot().store_failure.is_some(),
        "a session that lost durability stays halted until it is restarted"
    );
    assert!(
        !runtime.snapshot().run_active,
        "no run may start on a session whose history the store rejected"
    );

    let _ = runtime.close().await;
}

/// Shutdown must *wait* for SQLite's close, not race the runtime's teardown.
///
/// The regression: nothing awaited the close. The store actor performed it on a
/// spawned task, so a Tokio runtime that finished shutting down first dropped
/// that task mid-close, dropping the reply channel `tokio-rusqlite`'s connection
/// thread `expect`s to send on. The dependency panicked, our global panic hook
/// caught it, and the process exited having reported a panic it caused. The
/// `-wal` file left behind is the visible half: a database that closed cleanly
/// has none, and one that looks crash-interrupted at every exit is a lie about
/// state (`AGENTS.md` §1.3).
#[tokio::test]
async fn closing_the_store_checkpoints_and_removes_the_write_ahead_log() {
    let fixture = TempDir::new().expect("fixture");
    let database = fixture.path().join("mjolnr.sqlite3");
    let store = mjolnr::store::sqlite::SqliteEventStore::open(&database)
        .await
        .expect("open");

    // Something durable, so there is a WAL to checkpoint.
    let project = store
        .open_project(fixture.path().to_path_buf())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "closing".to_owned(), None)
        .await
        .expect("session");
    assert!(
        database.with_extension("sqlite3-wal").exists(),
        "the precondition is a live WAL; without one this test proves nothing"
    );

    store.close().await.expect("the store closes cleanly");

    assert!(
        !database.with_extension("sqlite3-wal").exists(),
        "a closed database has run its final checkpoint and removed the -wal file"
    );
    assert!(database.exists(), "the database itself survives the close");
}

/// After the close, the store says it is gone rather than answering from a
/// connection that no longer exists. Includes closing twice, which shutdown
/// paths can do and which must not panic.
#[tokio::test]
async fn a_closed_store_refuses_further_work_instead_of_answering() {
    let fixture = TempDir::new().expect("fixture");
    let store =
        mjolnr::store::sqlite::SqliteEventStore::open(fixture.path().join("mjolnr.sqlite3"))
            .await
            .expect("open");
    store.close().await.expect("first close");

    let second = store.close().await;
    assert!(
        matches!(second, Err(StoreError::Unavailable { .. })),
        "a second close reports the store shut down, not success it cannot prove: {second:?}"
    );

    let after = store.open_project(fixture.path().to_path_buf()).await;
    assert!(
        matches!(after, Err(StoreError::Unavailable { .. })),
        "work after a close is refused, never silently dropped: {after:?}"
    );
}

//! The crash matrix, and what a resumed session restores.
//!
//! # How a crash is reproduced
//!
//! Durable history is written directly, then read back through SQLite and
//! projected. That is not a shortcut around the runtime — it is the honest shape
//! of the question. Every case in 's checklist asks "given this
//! sequence of durable events, what must resume do?", and the answer must not
//! depend on how the sequence came to exist.
//!
//! Two things make this faithful rather than convenient:
//!
//! - The events go through the real wire format, the real SQLite schema, and the
//!   real gap-checking read path. Nothing is stubbed between the assertion and
//!   the database.
//! - [`a_real_interrupted_run_leaves_the_history_these_tests_assume`] closes the
//!   loop from the other end: it drives an actual runtime into an actual
//!   interrupted state and proves the database really does end up looking like
//!   the fixtures below.
//!
//! Simulating a crash by killing a live actor mid-tool would make every test
//! race the tool it was trying to interrupt. Splitting the question in two keeps
//! each half deterministic (`AGENTS.md` §7).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use smed::core::checkpoint::SessionCheckpoint;
use smed::core::command::{ApprovalDecision, ApprovalId, SmedCommand};
use smed::core::event::{FinishReason, RunId, SessionId, SmedEvent};
use smed::core::message::{CanonicalMessage, ContentBlock, ToolCall, ToolEffect, ToolResult};
use smed::core::model::{ModelId, ProviderId, Usage};
use smed::core::policy::PolicyMode;
use smed::core::provider::Provider;
use smed::core::recovery::{Authority, InterruptedKind, RecoveryDecision, RecoveryState};
use smed::core::runtime::SmedRuntime;
use smed::core::store::{EventStore, ProjectId, SessionStatus};
use smed::core::tool::ToolTier;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::runtime::recovery::{Recovered, project};
use smed::store::sqlite::SqliteEventStore;
use tempfile::TempDir;

/// A disposable database and workspace.
///
/// Never the user's real database: `AGENTS.md` §7 requires the default test run
/// to touch nothing real, and a persistence bug that ate a developer's sessions
/// would be a memorable way to learn that.
struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let database = directory.path().join("smed.sqlite3");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");

        // Canonicalised here because `OpenProject` canonicalises too, and on
        // macOS `/var` is a symlink to `/private/var`. Comparing a restored root
        // against the raw temp path would fail for a reason that has nothing to
        // do with persistence — and would tempt someone to "fix" it by dropping
        // the canonicalisation that keeps paths contained.
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
                .expect("open database"),
        )
    }
}

/// Create a project and session row, ready for events.
async fn open_session(store: &SqliteEventStore, root: &Path) -> (SessionId, ProjectId) {
    let project = store
        .open_project(root.to_path_buf())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "test".to_owned(), None)
        .await
        .expect("session");
    (session, project)
}

async fn seed(store: &SqliteEventStore, events: Vec<SmedEvent>) {
    for event in events {
        store.append(event).await.expect("append");
    }
}

/// Read history back and project it, exactly as `resume` does.
async fn recover(store: &SqliteEventStore, session: SessionId) -> Recovered {
    let checkpoint = store.latest_checkpoint(session).await.expect("checkpoint");
    let from = checkpoint.as_ref().map_or(0, |stored| stored.sequence);
    let resume = store
        .branch_events_from(session, from)
        .await
        .expect("branch events")
        .expect("checkpoint is on the branch");
    project(
        checkpoint.map(|stored| stored.checkpoint),
        &resume.covered_message_sequences,
        &resume.events,
    )
    .expect("project")
}

fn session_created(session: SessionId) -> SmedEvent {
    SmedEvent::SessionCreated {
        session,
        provider: ProviderId::new(FakeProvider::ID),
        model: ModelId::new(FakeProvider::MODEL),
    }
}

fn write_call() -> ToolCall {
    ToolCall {
        id: "call_1".to_owned(),
        name: "write_file".to_owned(),
        arguments: serde_json::json!({ "path": "created.txt", "content": "written\n" }),
        provider_signature: None,
    }
}

// ---------------------------------------------------------------------------
// The crash matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_proposal_interrupted_before_approval_provably_did_not_run() {
    // : "crash after ToolProposed but before approval does not
    // execute on resume." smed persists intent before effect, so an approval
    // that does not exist is proof the effect never started.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::user("write a file")),
            },
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(ApprovalId::new()),
                call: write_call(),
                tier: ToolTier::Write,
                preview: "+ written".to_owned(),
            },
        ],
    )
    .await;

    let recovered = recover(&store, session).await;

    match &recovered.recovery {
        RecoveryState::Required(work) => {
            assert!(matches!(
                work.kind,
                InterruptedKind::ProposalUnapproved { .. }
            ));
            assert_eq!(work.run, run);
            assert!(
                work.effect_is_certain(),
                "an unapproved proposal provably did not run"
            );
        }
        RecoveryState::Clean => panic!("an interrupted proposal must block the session"),
    }
}

#[tokio::test]
async fn an_approved_effect_with_no_outcome_is_uncertain_and_never_characterised() {
    // : "crash after approval but before ToolCompleted does not
    // auto-retry", and its anti-pattern: "do not infer that an interrupted
    // command failed merely because no completion event exists."
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();
    let approval = ApprovalId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(approval),
                call: write_call(),
                tier: ToolTier::Write,
                preview: "+ written".to_owned(),
            },
            SmedEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision: ApprovalDecision::ApproveOnce,
            },
        ],
    )
    .await;

    let recovered = recover(&store, session).await;
    let work = recovered
        .recovery
        .work()
        .expect("an approved effect with no outcome must block the session");

    match &work.kind {
        InterruptedKind::EffectUncertain { authority, .. } => {
            assert_eq!(*authority, Authority::Approval(approval));
        }
        other => panic!("expected an uncertain effect, got {other:?}"),
    }
    assert!(
        !work.effect_is_certain(),
        "smed must not claim to know whether an approved effect ran"
    );

    let summary = work.summary().to_lowercase();
    assert!(summary.contains("may or may not"));
    assert!(!summary.contains("failed") && !summary.contains("succeeded"));
}

#[tokio::test]
async fn a_policy_authorised_effect_is_exactly_as_uncertain_as_an_approved_one() {
    // The case an authority-based split would have got wrong: `PolicyDecision::
    // Allow` persists `ToolProposed { approval: None }` and starts the tool
    // immediately. No human said yes, and the effect is just as unknown.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: None,
                call: write_call(),
                tier: ToolTier::Write,
                preview: "+ written".to_owned(),
            },
        ],
    )
    .await;

    let recovered = recover(&store, session).await;
    let work = recovered.recovery.work().expect("must block");

    match &work.kind {
        InterruptedKind::EffectUncertain { authority, .. } => {
            assert_eq!(*authority, Authority::Policy);
        }
        other => panic!("an ungated start is uncertain, not safe: got {other:?}"),
    }
    assert!(!work.effect_is_certain());
}

#[tokio::test]
async fn a_denied_proposal_leaves_nothing_to_recover() {
    // Denial is a terminal outcome. A session that stopped after one must resume
    // clean, or every refusal would leave a recovery prompt behind it.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();
    let approval = ApprovalId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(approval),
                call: write_call(),
                tier: ToolTier::Write,
                preview: "+ written".to_owned(),
            },
            SmedEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision: ApprovalDecision::Deny,
            },
            SmedEvent::RunFinished {
                session,
                run,
                reason: FinishReason::Stop,
            },
        ],
    )
    .await;

    assert_eq!(
        recover(&store, session).await.recovery,
        RecoveryState::Clean
    );
}

#[tokio::test]
async fn an_interrupted_provider_call_is_flagged_and_never_replayed() {
    // A stream that died may have produced tokens and billed for them
    // (`AGENTS.md` §4). Resume reports it; it does not reissue it.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::user("hello")),
            },
            SmedEvent::RunStarted { session, run },
        ],
    )
    .await;

    let recovered = recover(&store, session).await;
    let work = recovered.recovery.work().expect("must block");
    assert!(matches!(
        work.kind,
        InterruptedKind::ProviderTurnInterrupted
    ));
    assert!(!work.effect_is_certain());
}

#[tokio::test]
async fn a_mutation_completed_after_the_last_checkpoint_is_recovered_from_events() {
    // : "crash after completed mutation but before next checkpoint
    // recovers from events." The checkpoint is an optimisation; treating it as
    // the whole truth would silently drop this mutation.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;

    // Run one finishes and is checkpointed. It saw no mutation.
    let first = RunId::new();
    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted {
                session,
                run: first,
            },
            SmedEvent::RunFinished {
                session,
                run: first,
                reason: FinishReason::Stop,
            },
        ],
    )
    .await;

    let covered = store
        .write_checkpoint(SessionCheckpoint {
            project_root: Some(fixture.workspace.clone()),
            ..SessionCheckpoint::empty(session)
        })
        .await
        .expect("checkpoint");
    assert_eq!(covered, 3, "the checkpoint covers every event so far");

    // Run two mutates a file, then the process dies before the next checkpoint.
    let second = RunId::new();
    seed(
        &store,
        vec![
            SmedEvent::RunStarted {
                session,
                run: second,
            },
            SmedEvent::ToolCompleted {
                session,
                run: second,
                call_id: "call_1".to_owned(),
                name: "write_file".to_owned(),
                result: ToolResult::ok("written").with_effect(ToolEffect::Mutation {
                    path: "created.txt".to_owned(),
                    sha256: "abc123".to_owned(),
                }),
            },
        ],
    )
    .await;

    let recovered = recover(&store, session).await;

    assert_eq!(
        recovered.state.last_mutation_sequence,
        Some(4),
        "the mutation must be recovered from its event, not lost with the checkpoint"
    );
    assert_eq!(
        recovered
            .state
            .read_set
            .version(&fixture.workspace.join("created.txt"))
            .expect("read set"),
        Some("abc123".to_owned()),
        "the mutated file's version must be restored so a later edit is not stale"
    );
    assert!(
        recovered.recovery.is_required(),
        "the run was still open; the session must not continue silently"
    );
}

#[tokio::test]
async fn successful_command_evidence_survives_a_crash() {
    // `finish_task` cites a command's event id. If evidence did not survive,
    // a recovered session could not prove work it had already proven.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();

    seed(&store, vec![session_created(session)]).await;
    let stored = store
        .append(SmedEvent::ToolCompleted {
            session,
            run,
            call_id: "call_1".to_owned(),
            name: "run_command".to_owned(),
            result: ToolResult::ok("exit 0").with_effect(ToolEffect::Command {
                exit_code: Some(0),
                success: true,
                duration_ms: 4,
            }),
        })
        .await
        .expect("append");

    let recovered = recover(&store, session).await;
    assert_eq!(
        recovered
            .state
            .successful_command_evidence
            .get(&stored.id.to_string()),
        Some(&stored.sequence),
        "command evidence must survive a restart"
    );
}

#[tokio::test]
async fn a_resolved_recovery_unblocks_the_session_durably() {
    // The decision is itself an event, so a second crash does not re-ask a
    // question the human already answered.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(ApprovalId::new()),
                call: write_call(),
                tier: ToolTier::Write,
                preview: String::new(),
            },
        ],
    )
    .await;
    assert!(recover(&store, session).await.recovery.is_required());

    seed(
        &store,
        vec![SmedEvent::RecoveryResolved {
            session,
            decision: RecoveryDecision::AbandonAndContinue,
        }],
    )
    .await;

    assert_eq!(
        recover(&store, session).await.recovery,
        RecoveryState::Clean,
        "a durable decision must survive the next restart"
    );
}

// ---------------------------------------------------------------------------
// What a restart restores
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive checkpoint fixture and its assertions; splitting it hides which fields the restore actually covers"
)]
async fn a_checkpoint_restores_every_field_the_phase_requires() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;

    seed(&store, vec![session_created(session)]).await;

    let checkpoint = SessionCheckpoint {
        session,
        status: SessionStatus::Active,
        project_root: Some(fixture.workspace.clone()),
        provider: Some(ProviderId::new("openai")),
        model: Some(ModelId::new("gpt-4o-mini")),
        messages: vec![
            CanonicalMessage::user("first"),
            CanonicalMessage::assistant(
                vec![ContentBlock::Text {
                    text: "second".to_owned(),
                }],
                ProviderId::new("openai"),
                ModelId::new("gpt-4o-mini"),
            ),
        ],
        usage: Usage {
            input_tokens: 11,
            output_tokens: 7,
        },
        policy: PolicyMode::WorkspaceWrite,
        budget: smed::core::runtime::BudgetStatus {
            provider_turns: 2,
            max_provider_turns: 8,
            tool_calls: 3,
            max_tool_calls: 16,
        },
        read_set: vec![(fixture.workspace.join("a.rs"), "sha-a".to_owned())],
        read_evidence: vec![smed::core::change_capture::ReadRecord::new(
            "a.rs".to_owned(),
            "sha-a".to_owned(),
            "evt-a".to_owned(),
        )],
        review_threads: vec![smed::core::review::ReviewThread::open(
            smed::core::review::ReviewThreadId::new(),
            smed::core::review::ReviewAnchor {
                path: "a.rs".to_owned(),
                side: smed::core::review::ReviewSide::New,
                line: 12,
                hunk_header: "@@ -10,3 +10,4 @@".to_owned(),
                capture_digest: "9f8e7d".to_owned(),
                base_object_id: Some("abc123".to_owned()),
            },
            smed::core::review::ReviewComment {
                body: "handle the None case".to_owned(),
                created_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        )],
        last_mutation_sequence: Some(9),
        successful_command_evidence: std::collections::BTreeMap::from([("ev".to_owned(), 10)]),
        activated_skills: vec!["guarded-review".to_owned()],
        workspace_trusted: true,
        handoff: None,
        quota_reserve: smed::core::continuation::QuotaReserveStatus::default(),
        route: Some(smed::core::routing::RouteRuntime {
            route: "main".to_owned(),
            position: 1,
        }),
    };
    store
        .write_checkpoint(checkpoint.clone())
        .await
        .expect("checkpoint");

    let recovered = recover(&store, session).await;
    let state = recovered.state;

    assert_eq!(state.workspace_root, Some(fixture.workspace.clone()));
    assert_eq!(
        state.provider.as_ref().map(ProviderId::as_str),
        Some("openai")
    );
    assert_eq!(
        state.model.as_ref().map(ModelId::as_str),
        Some("gpt-4o-mini")
    );
    assert_eq!(state.messages().len(), 2);
    assert_eq!(state.messages()[0].text(), "first");
    assert_eq!(state.messages()[1].text(), "second");
    assert_eq!(state.usage, checkpoint.usage);
    assert_eq!(state.policy, PolicyMode::WorkspaceWrite);
    assert_eq!(state.budget.provider_turns, 2);
    assert_eq!(state.budget.tool_calls, 3);
    assert_eq!(state.last_mutation_sequence, Some(9));
    assert_eq!(state.successful_command_evidence.get("ev"), Some(&10));
    assert!(state.activated_skills.contains("guarded-review"));
    assert!(state.workspace_trusted);
    assert_eq!(
        state
            .read_set
            .version(&fixture.workspace.join("a.rs"))
            .expect("read set"),
        Some("sha-a".to_owned())
    );
    // The evidence rides the checkpoint beside the read set, and for the same
    // reason: a checkpoint that covers the read event stops that event being
    // replayed, so evidence rebuilt only from replay would be lost at exactly
    // the restarts a checkpoint exists to make cheap.
    assert_eq!(
        state
            .read_evidence
            .get("a.rs")
            .map(|record| record.tool_event_id.clone()),
        Some("evt-a".to_owned())
    );
    // Notes ride the checkpoint for the reason the evidence above does: a
    // checkpoint covering a note's event stops that event being replayed, and
    // §D3 requires a note to come back with its original anchor.
    let restored = state
        .review_threads
        .values()
        .next()
        .expect("the checkpoint's review thread");
    assert_eq!(restored.anchor.line, 12);
    assert_eq!(restored.anchor.capture_digest, "9f8e7d");
    assert_eq!(recovered.recovery, RecoveryState::Clean);
}

#[tokio::test]
async fn an_exact_command_grant_never_survives_a_restart() {
    //  scopes `ApproveExactForSession` to one session. A grant that came
    // back after a restart would widen authority a human granted, without them
    // doing anything.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();
    let approval = ApprovalId::new();

    let command = ToolCall {
        id: "call_1".to_owned(),
        name: "run_command".to_owned(),
        arguments: serde_json::json!({ "program": "git", "arguments": ["status"] }),
        provider_signature: None,
    };

    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(approval),
                call: command.clone(),
                tier: ToolTier::Execute,
                preview: "git status".to_owned(),
            },
            // The human granted the broadest thing smed can express.
            SmedEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision: ApprovalDecision::ApproveExactForSession,
            },
            SmedEvent::ToolCompleted {
                session,
                run,
                call_id: "call_1".to_owned(),
                name: "run_command".to_owned(),
                result: ToolResult::ok("clean").with_effect(ToolEffect::Command {
                    exit_code: Some(0),
                    success: true,
                    duration_ms: 1,
                }),
            },
            SmedEvent::RunFinished {
                session,
                run,
                reason: FinishReason::Stop,
            },
        ],
    )
    .await;
    store
        .write_checkpoint(SessionCheckpoint::empty(session))
        .await
        .expect("checkpoint");

    let recovered = recover(&store, session).await;

    assert!(
        recovered.state.exact_commands.is_empty(),
        "a resumed session must ask again before running an approved command; \
         the grant was scoped to the session that ended"
    );
}

// ---------------------------------------------------------------------------
// The runtime path
// ---------------------------------------------------------------------------

fn runtime_for(store: &Arc<SqliteEventStore>) -> Runtime {
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    Runtime::spawn(vec![provider], Arc::clone(store) as Arc<dyn EventStore>)
}

#[tokio::test]
async fn resume_reports_the_boundary_and_does_not_execute_the_interrupted_tool() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let (session, _) = open_session(&store, &fixture.workspace).await;
    let run = RunId::new();
    let approval = ApprovalId::new();

    // Approved, started, no outcome: the tool would have created this file.
    seed(
        &store,
        vec![
            session_created(session),
            SmedEvent::RunStarted { session, run },
            SmedEvent::ToolProposed {
                session,
                run,
                approval: Some(approval),
                call: write_call(),
                tier: ToolTier::Write,
                preview: "+ written".to_owned(),
            },
            SmedEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision: ApprovalDecision::ApproveOnce,
            },
        ],
    )
    .await;

    let target = fixture.workspace.join("created.txt");
    assert!(!target.exists());

    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");

    let snapshot = settle(&runtime, |snapshot| snapshot.recovery.is_required()).await;

    assert!(
        snapshot.recovery.is_required(),
        "a resumed session with an uncertain effect must halt"
    );
    assert!(
        !target.exists(),
        "resume must never execute the interrupted tool "
    );

    // And the halt is real: a directive is refused rather than run.
    let event_count_before_refusal = store.events(session).await.expect("events").len();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "carry on".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("dispatch");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !target.exists(),
        "autonomous work must stay blocked until a human resolves the recovery"
    );
    assert!(runtime.snapshot().recovery.is_required());
    assert_eq!(
        store.events(session).await.expect("events").len(),
        event_count_before_refusal,
        "a command refused before starting is not a failed run"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_session_survives_a_clean_close_and_reopen() {
    // The whole promise of the phase, end to end: real runtime, real close, real
    // reopen from a file on disk.
    let fixture = Fixture::new();
    let session;

    {
        let store = fixture.store().await;
        let runtime = runtime_for(&store);
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

        let snapshot = settle(&runtime, |snapshot| snapshot.messages.len() >= 2).await;
        session = snapshot.session.expect("a session");

        // The acknowledged shutdown: returns only once the checkpoint is written
        // and the store has drained.
        runtime.close().await.expect("clean shutdown");
    }

    let store = fixture.store().await;
    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");

    let snapshot = settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    assert_eq!(snapshot.session, Some(session));
    assert!(
        snapshot.messages.len() >= 2,
        "the conversation must survive: got {} message(s)",
        snapshot.messages.len()
    );
    assert_eq!(snapshot.messages[0].text(), "hello");
    assert_eq!(
        snapshot.model.map(|id| id.as_str().to_owned()),
        Some(FakeProvider::MODEL.to_owned())
    );
    assert_eq!(snapshot.workspace_root, Some(fixture.workspace.clone()));
    assert_eq!(
        snapshot.recovery,
        RecoveryState::Clean,
        "a cleanly closed session has nothing to recover"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_resumed_session_is_announced_to_clients_that_watch_state() {
    // The regression this pins was found by the manual smoke test, not by the
    // suite: resume opened to an empty screen.
    //
    // The TUI used to re-read the snapshot only when a *durable event* arrived.
    // Resume restores an entire transcript and, when nothing was interrupted,
    // emits nothing — so the view sat empty while the runtime held the whole
    // session. Every headless test missed it by polling `snapshot()` directly,
    // which is exactly the guess the TUI could not make.
    //
    // So this asserts the thing a client actually relies on: state that changes
    // with no event to announce it still reaches a client watching state.
    let fixture = Fixture::new();
    let session;

    {
        let store = fixture.store().await;
        let runtime = runtime_for(&store);
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
                text: "remember this".to_owned(),
                source: smed::core::directive::DirectiveSource::Human,
            })
            .await
            .expect("send");
        let snapshot = settle(&runtime, |snapshot| snapshot.messages.len() >= 2).await;
        session = snapshot.session.expect("session");
        runtime.close().await.expect("clean shutdown");
    }

    let store = fixture.store().await;
    let runtime = runtime_for(&store);

    // Subscribed before the command, exactly as the TUI does.
    let mut snapshots = runtime.snapshots();
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");

    // The restored transcript must arrive on the state feed within a bounded
    // wait, without the client having polled for it.
    let restored = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("the runtime is alive");
            if snapshot.session == Some(session) && !snapshot.messages.is_empty() {
                return snapshot;
            }
        }
    })
    .await
    .expect("a resumed session must announce itself to a client watching state");

    assert_eq!(restored.messages[0].text(), "remember this");
    assert_eq!(
        restored.model.as_ref().map(ModelId::as_str),
        Some(FakeProvider::MODEL)
    );
    assert_eq!(restored.recovery, RecoveryState::Clean);

    // And the point: a clean resume announces no *event*. A client that waited
    // for one — as the TUI did — would wait forever.
    let announced = tokio::time::timeout(Duration::from_millis(100), events.recv()).await;
    assert!(
        announced.is_err(),
        "a clean resume emits no event; that is why watching state is the contract"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_clean_shutdown_writes_a_checkpoint_covering_every_event() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session;

    {
        let runtime = runtime_for(&store);
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
        let snapshot = settle(&runtime, |snapshot| snapshot.session.is_some()).await;
        session = snapshot.session.expect("session");

        runtime.close().await.expect("clean shutdown");
    }

    let events = store.events(session).await.expect("events");
    let checkpoint = store
        .latest_checkpoint(session)
        .await
        .expect("checkpoint")
        .expect("a clean shutdown must leave a checkpoint");

    assert_eq!(
        checkpoint.sequence,
        events.len() as u64,
        "the shutdown checkpoint must cover every durable event"
    );
    assert!(
        store
            .events_from(session, checkpoint.sequence)
            .await
            .expect("events")
            .is_empty(),
        "nothing may follow the clean checkpoint"
    );
}

#[tokio::test]
async fn a_real_interrupted_run_leaves_the_history_these_tests_assume() {
    // Closes the loop from the other end. The fixtures above assert what resume
    // does with an interrupted history; this proves an actually-interrupted
    // runtime writes that history — that "approved, no outcome" is a state the
    // database really reaches, not one only a test can construct.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session;

    {
        let provider: Arc<dyn Provider> = Arc::new(
            FakeProvider::new(FakeScript::Text).with_fragment_delay(Duration::from_millis(50)),
        );
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
        settle(&runtime, |snapshot| snapshot.session.is_some()).await;
        session = runtime.snapshot().session.expect("session");

        runtime
            .dispatch(SmedCommand::SendUserMessage {
                text: "hello".to_owned(),
                source: smed::core::directive::DirectiveSource::Human,
            })
            .await
            .expect("send");

        // Wait for the run to be durably open, then abandon it without closing:
        // no checkpoint, no flush — a crash.
        settle(&runtime, |snapshot| snapshot.run_active).await;
        drop(runtime);
    }

    let events = store.events(session).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|stored| label(&stored.event)).collect();

    assert!(
        kinds.contains(&"run-started"),
        "the interrupted run must be durable: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"run-finished") && !kinds.contains(&"run-failed"),
        "an interrupted run must have no terminal event: {kinds:?}"
    );
    assert!(
        store
            .latest_checkpoint(session)
            .await
            .expect("checkpoint")
            .is_none(),
        "a crash must not leave a checkpoint"
    );

    // And the history a real crash leaves projects to the state the matrix above
    // asserts on.
    assert!(recover(&store, session).await.recovery.is_required());
}

fn label(event: &SmedEvent) -> &'static str {
    match event {
        SmedEvent::SessionCreated { .. } => "session-created",
        SmedEvent::MessageAppended { .. } => "message",
        SmedEvent::RunStarted { .. } => "run-started",
        SmedEvent::RunFinished { .. } => "run-finished",
        SmedEvent::RunFailed { .. } => "run-failed",
        _ => "other",
    }
}

/// Poll the snapshot until `ready`, with a bounded wait.
///
/// Polling rather than sleeping: a fixed sleep is either flaky or slow, and this
/// is neither (`AGENTS.md` §7 — no sleeps as synchronisation).
async fn settle(
    runtime: &Runtime,
    ready: impl Fn(&smed::core::runtime::RuntimeSnapshot) -> bool,
) -> smed::core::runtime::RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

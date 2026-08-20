//! Deterministic Phase 5.2 read-set collision invalidation across a spawn
//! group, with real git worktrees.
//!
//! Two siblings touch the same workspace-relative path: one reads it, the
//! other edits it. The reader's verified finish must be refused — its result
//! withheld and a durable `ReadSetCollision` event recorded (Prime Directives
//! §1–2). A disjoint group must produce no collision.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::command::{ApprovalDecision, MjolnrCommand};
use mjolnr::core::event::{MjolnrEvent, SessionId};
use mjolnr::core::message::ToolResult;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::policy::PolicyMode;
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSubscription};
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(30);

struct Harness {
    _repository: TempDir,
    runtime: Runtime,
    store: Arc<InMemoryEventStore>,
}

impl Harness {
    async fn new(policy: PolicyMode) -> Self {
        let repository = repository();
        let store = Arc::new(InMemoryEventStore::new());
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Subagent));
        let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

        runtime
            .dispatch(MjolnrCommand::OpenProject {
                root: repository.path().to_path_buf(),
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
        runtime
            .dispatch(MjolnrCommand::SetPolicy { mode: policy })
            .await
            .expect("set policy");
        wait_ready(&runtime, policy).await;

        Self {
            _repository: repository,
            runtime,
            store,
        }
    }
}

async fn wait_ready(runtime: &Runtime, policy: PolicyMode) {
    if runtime.snapshot().session.is_some() && runtime.snapshot().policy == policy {
        return;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(DEADLINE, async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime remains open");
            if snapshot.session.is_some() && snapshot.policy == policy {
                break;
            }
        }
    })
    .await
    .expect("session becomes ready");
}

async fn approve_spawn(
    runtime: &Runtime,
    directive: &str,
) -> (RuntimeSubscription, Vec<MjolnrEvent>) {
    let mut events = runtime.subscribe();
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: directive.to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send directive");

    let mut observed = Vec::new();
    let approval = tokio::time::timeout(DEADLINE, async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            let approval = match &event {
                MjolnrEvent::ToolProposed {
                    approval: Some(approval),
                    call,
                    ..
                } if call.name == "spawn_subagent" => Some(*approval),
                _ => None,
            };
            observed.push(event);
            if let Some(approval) = approval {
                break approval;
            }
        }
    })
    .await
    .expect("spawn proposal arrives");

    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve spawn");
    (events, observed)
}

async fn run_spawn(runtime: &Runtime, directive: &str) -> Vec<MjolnrEvent> {
    let (mut events, mut observed) = approve_spawn(runtime, directive).await;
    tokio::time::timeout(DEADLINE, async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            let terminal = matches!(
                event,
                MjolnrEvent::RunFinished { .. } | MjolnrEvent::RunFailed { .. }
            );
            observed.push(event);
            if terminal {
                break;
            }
        }
    })
    .await
    .expect("parent run settles");
    observed
}

fn repository() -> TempDir {
    let repository = tempfile::tempdir().expect("temporary repository");
    std::fs::write(repository.path().join("README.md"), "base\n").expect("seed README");
    std::fs::write(repository.path().join("shared.txt"), "before\n").expect("seed shared file");
    git(repository.path(), &["init", "-q"]);
    git(repository.path(), &["config", "user.name", "mjolnr Test"]);
    git(
        repository.path(),
        &["config", "user.email", "mjolnr-test@localhost"],
    );
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "-q", "-m", "seed"]);
    repository
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn spawn_result(events: &[MjolnrEvent]) -> &ToolResult {
    events
        .iter()
        .find_map(|event| match event {
            MjolnrEvent::ToolCompleted { name, result, .. } if name == "spawn_subagent" => {
                Some(result)
            }
            _ => None,
        })
        .expect("spawn result")
}

fn spawned(events: &[MjolnrEvent]) -> Vec<(SessionId, PolicyMode)> {
    events
        .iter()
        .filter_map(|event| match event {
            MjolnrEvent::SubagentSpawned { child, policy, .. } => Some((*child, *policy)),
            _ => None,
        })
        .collect()
}

fn collision(events: &[MjolnrEvent]) -> Option<(SessionId, SessionId, String)> {
    events.iter().find_map(|event| match event {
        MjolnrEvent::ReadSetCollision {
            reader,
            writer,
            path,
            ..
        } => Some((*reader, *writer, path.clone())),
        _ => None,
    })
}

#[tokio::test]
async fn a_sibling_write_refuses_the_readers_verified_finish() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let parent = harness.runtime.snapshot().session.expect("parent session");

    let events = run_spawn(&harness.runtime, "spawn-collide:").await;

    // The spawn settles normally: one child (the writer) delivered a result.
    assert!(spawn_result(&events).outcome.is_ok());

    // The reader is the `worker-read` child (read-only); the writer is the
    // `worker-edit` child (workspace-write). Both boundaries are visible.
    let children = spawned(&events);
    assert_eq!(children.len(), 2, "both children must dispatch");
    let reader = children
        .iter()
        .find(|(_, policy)| *policy == PolicyMode::ReadOnly)
        .map(|(child, _)| *child)
        .expect("the reader child is read-only");
    let writer = children
        .iter()
        .find(|(_, policy)| *policy == PolicyMode::WorkspaceWrite)
        .map(|(child, _)| *child)
        .expect("the writer child is workspace-write");

    // The collision is durably recorded against the parent session, naming the
    // stale reader, the sibling that invalidated it, and the shared path.
    let (collided_reader, collided_writer, path) =
        collision(&events).expect("a collision must be detected");
    assert_eq!(collided_reader, reader);
    assert_eq!(collided_writer, writer);
    assert_eq!(path, "shared.txt");

    // The settlement JSON refuses the reader's verified finish: its result is
    // withheld and it is marked for re-validation.
    let content: serde_json::Value =
        serde_json::from_str(&spawn_result(&events).content).expect("settlement is JSON");
    let records = content["children"].as_array().expect("child records");
    let reader_record = records
        .iter()
        .find(|record| {
            record
                .get("collision_paths")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|paths| !paths.is_empty())
        })
        .expect("one child must be invalidated");
    assert_eq!(reader_record["session"], reader.to_string());
    assert_eq!(reader_record["outcome"], "revalidation_required");
    assert_eq!(reader_record["reason_code"], "READ_SET_COLLISION");
    assert_eq!(
        reader_record["collision_paths"],
        serde_json::json!(["shared.txt"])
    );
    assert!(
        reader_record.get("result").is_none(),
        "a stale read-set must not report a verified result"
    );

    // The writer's own finish is untouched: reading then editing the same file
    // is an ordinary edit, not a collision.
    let writer_record = records
        .iter()
        .find(|record| record["session"] == writer.to_string())
        .expect("the writer's record");
    assert_eq!(writer_record["outcome"], "completed");
    assert!(writer_record.get("result").is_some());

    // The diagnostic is durable, not merely an in-memory settlement fact.
    let stored = harness.store.events(parent).await.expect("read the ledger");
    assert!(
        stored
            .iter()
            .any(|entry| matches!(&entry.event, MjolnrEvent::ReadSetCollision { .. })),
        "the collision must be persisted to the event ledger"
    );
}

#[tokio::test]
async fn disjoint_siblings_produce_no_collision() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let events = run_spawn(&harness.runtime, "spawn-two:").await;

    assert!(spawn_result(&events).outcome.is_ok());
    assert!(
        collision(&events).is_none(),
        "disjoint reads and writes must not invent a collision:\n{events:#?}"
    );
}

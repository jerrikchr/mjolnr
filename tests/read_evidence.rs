//! Read-before-edit evidence: which durable event recorded each read.
//!
//! One reason to change: whether `ChangeSet::read_evidence` cites the real
//! `ToolCompleted` event that produced a read, and keeps citing it across a
//! restart.
//!
//! The assertions go through `snapshot_to_client` and are cross-checked against
//! the *stored* transcript, because the property under test is an equality
//! between two things a client can otherwise never compare: the id on the wire
//! and the id in the event log. A test that only read the wire would pass just
//! as happily against a manufactured id, which is the failure §D3 names.

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helpers these
// tests share. Same allowance, same reason (AGENTS.md §7).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use smed::core::changes::ChangeSet;
use smed::core::command::SmedCommand;
use smed::core::event::{SessionId, SmedEvent};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSubscription, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::runtime::client_bridge::snapshot_to_client;
use smed::store::memory::InMemoryEventStore;

fn spawn_runtime(store: &Arc<InMemoryEventStore>) -> Runtime {
    Runtime::spawn(
        vec![Arc::new(FakeProvider::new(FakeScript::GuardedLoop)) as Arc<dyn Provider>],
        Arc::clone(store) as Arc<dyn EventStore>,
    )
}

/// A repository whose `fixture.txt` is committed and then modified, so the file
/// the model reads is also a file the change set shows. Evidence is projected
/// only for files under review, so both halves have to be true at once.
fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("smed-d3-evidence-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical temp dir");

    git(&dir, &["init", "--initial-branch=main"]);
    git(&dir, &["config", "user.email", "test@smed.invalid"]);
    git(&dir, &["config", "user.name", "smed Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    fs::write(dir.join("fixture.txt"), "before\n").expect("write");
    git(&dir, &["add", "fixture.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("fixture.txt"), "before\nedited by a human\n").expect("write");
    dir
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn wait_for(
    events: &mut RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&SmedEvent) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(event) if predicate(&event) => return,
                Ok(_) => {}
                Err(error) => panic!("event feed ended while waiting for {label}: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}

fn client_changes(runtime: &Runtime) -> Option<ChangeSet> {
    snapshot_to_client(1, &runtime.snapshot()).changes
}

/// Wait for a resume to land, then hand back what a client would render.
///
/// Polls the published snapshot rather than sleeping: `ResumeSession` is
/// unacknowledged, so the only honest completion signal is the snapshot
/// carrying the resumed session. A store failure is reported as itself instead
/// of timing out, because "resume refused" and "resume slow" send a reader to
/// very different places.
async fn await_resumed_changes(runtime: &Runtime, session: SessionId) -> ChangeSet {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = runtime.snapshot();
            assert!(
                snapshot.store_failure.is_none(),
                "resume refused: {:?}",
                snapshot.store_failure
            );
            if snapshot.session == Some(session)
                && let Some(changes) = snapshot_to_client(1, &snapshot).changes
            {
                return changes;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a change set after resume")
}

/// Drive the guarded fake until its first tool — `read_file` on `fixture.txt` —
/// has completed and been recorded, then cancel so the session settles without
/// an in-flight run.
async fn read_then_settle(runtime: &Runtime, root: &Path) -> SessionId {
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: root.to_path_buf(),
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
            text: "update the fixture and verify it".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    wait_for(
        &mut events,
        "read_file completion",
        |event| matches!(event, SmedEvent::ToolCompleted { name, .. } if name == "read_file"),
    )
    .await;

    let session = runtime.snapshot().session.expect("a session");
    runtime
        .dispatch(SmedCommand::CancelRun)
        .await
        .expect("cancel");
    wait_for(&mut events, "run finished", |event| {
        matches!(
            event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        )
    })
    .await;
    session
}

/// The id of the durable `ToolCompleted` event for `read_file`, read from the
/// store rather than from the wire. This is the value the wire must match.
async fn stored_read_event_id(store: &InMemoryEventStore, session: SessionId) -> String {
    store
        .events(session)
        .await
        .expect("stored transcript")
        .iter()
        .find(|stored| {
            matches!(&stored.event, SmedEvent::ToolCompleted { name, .. } if name == "read_file")
        })
        .expect("a stored read_file completion")
        .id
        .to_string()
}

#[tokio::test]
async fn evidence_cites_the_durable_event_that_recorded_the_read() {
    let dir = setup_repo("cites");
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = spawn_runtime(&store);

    let session = read_then_settle(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");
    let evidence = changes
        .read_evidence
        .iter()
        .find(|item| item.path == "fixture.txt")
        .expect("evidence for the file that was read");

    // The equality that matters: the id on the wire *is* the id in the log.
    // Anything smed invented would fail here, which is the whole reason this
    // field stayed empty until an event id was actually available.
    assert_eq!(
        evidence.tool_event_id,
        stored_read_event_id(&store, session).await
    );
    assert!(
        !evidence.read_revision.is_empty(),
        "evidence must carry the content hash that was read"
    );

    runtime.close().await.expect("close");
}

/// Evidence is not a promotion. Citing a read event says a read happened; it
/// says nothing about a change being applied or verified, and the change set
/// this producer builds is still a working-tree read.
#[tokio::test]
async fn evidence_does_not_promote_the_change_set_beyond_a_working_tree_read() {
    let dir = setup_repo("no-promotion");
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = spawn_runtime(&store);

    read_then_settle(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    assert!(!changes.read_evidence.is_empty());
    assert_eq!(
        changes.state,
        smed::core::changes::ChangeState::CurrentWorkingTree
    );
    let wire = serde_json::to_string(&changes).expect("serialize");
    assert!(
        !wire.contains("\"applied\"") && !wire.contains("verified"),
        "a cited read must not read as an applied or verified change: {wire}"
    );

    runtime.close().await.expect("close");
}

/// The restart property. A resumed session replays the same `ToolCompleted`
/// events, so it cites the same ids — the evidence is rebuilt from the record
/// rather than lost with the process that observed it.
#[tokio::test]
async fn evidence_survives_a_restart_with_the_same_event_id() {
    let dir = setup_repo("restart");
    let store = Arc::new(InMemoryEventStore::new());

    let first = spawn_runtime(&store);
    let session = read_then_settle(&first, &dir).await;
    let before = client_changes(&first)
        .expect("a change set")
        .read_evidence
        .clone();
    assert!(!before.is_empty());
    first.close().await.expect("close the first runtime");

    let second = spawn_runtime(&store);
    second
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");

    // No explicit refresh: resuming re-establishes the project root, and the
    // resume reads git for exactly that reason. A resumed session that showed
    // no diffs until the next unrelated write would make "notes survive
    // restart" untestable from a client.
    //
    // `ResumeSession` is not an acknowledged command — it returns once the
    // mailbox accepts it — so this waits for the resumed snapshot rather than
    // sleeping for one.
    let after = await_resumed_changes(&second, session)
        .await
        .read_evidence
        .clone();

    assert_eq!(
        before, after,
        "evidence must survive the restart unchanged: {before:?} vs {after:?}"
    );
    assert_eq!(
        after
            .first()
            .map(|item| item.tool_event_id.clone())
            .unwrap_or_default(),
        stored_read_event_id(&store, session).await
    );

    second.close().await.expect("close the second runtime");
}

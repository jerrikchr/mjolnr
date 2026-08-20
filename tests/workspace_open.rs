//! Opening a workspace root either succeeds or refuses out loud.
//!
//! One reason to change: the `OpenProject` refusal contract. Every case here
//! previously left the client with no signal at all — the command was
//! fire-and-forget, so a refusal was indistinguishable from a dead button, and
//! an unopenable path was reported as a store failure, which tells a user their
//! database is broken when they mistyped a directory (AGENTS.md §1.3).

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::error::{MjolnrError, ReasonCode};
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::FakeProvider;
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;

fn spawn_runtime() -> Runtime {
    Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        Arc::new(InMemoryEventStore::new()) as Arc<dyn EventStore>,
    )
}

/// The refusal code, or a panic naming what came back instead. Asserting on the
/// code rather than the prose is the contract (AGENTS.md §6).
fn refusal_code(error: &MjolnrError) -> ReasonCode {
    error
        .reason_code()
        .unwrap_or_else(|| panic!("expected a coded refusal, got: {error}"))
}

#[tokio::test]
async fn opening_a_real_directory_is_acknowledged_and_sets_the_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = spawn_runtime();

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");

    // No settling loop: the acknowledgement *is* the guarantee that the root is
    // set. A client that has to poll after a successful command cannot tell
    // "not yet" from "refused".
    assert!(
        runtime.snapshot().workspace_root.is_some(),
        "an acknowledged open must have set the workspace root"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_path_that_is_not_a_directory_is_refused_not_reported_as_a_store_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("not-a-directory.txt");
    std::fs::write(&file, b"regular file").expect("write file");
    let runtime = spawn_runtime();

    let error = runtime
        .dispatch(MjolnrCommand::OpenProject { root: file })
        .await
        .expect_err("a file is not a workspace root");

    assert_eq!(refusal_code(&error), ReasonCode::PathOutsideWorkspace);
    let snapshot = runtime.snapshot();
    assert!(
        snapshot.workspace_root.is_none(),
        "a refused open must not set a root"
    );
    // The distinction this test exists for: the store is fine. Reporting this
    // as `store_failure` would send the user to fix a database.
    assert!(
        snapshot.store_failure.is_none(),
        "a bad path is not a store fault: {:?}",
        snapshot.store_failure
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_missing_directory_is_refused_with_a_path_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = spawn_runtime();

    let error = runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: temp.path().join("no-such-directory"),
        })
        .await
        .expect_err("a missing directory is not a workspace root");

    assert_eq!(refusal_code(&error), ReasonCode::PathOutsideWorkspace);
    assert!(runtime.snapshot().store_failure.is_none());

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn an_empty_root_is_refused_before_it_reaches_the_filesystem() {
    let runtime = spawn_runtime();

    let error = runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: std::path::PathBuf::new(),
        })
        .await
        .expect_err("an empty path is not a workspace root");

    assert_eq!(refusal_code(&error), ReasonCode::PathOutsideWorkspace);
    assert!(runtime.snapshot().workspace_root.is_none());

    runtime.close().await.expect("close");
}

/// The refusal that had no signal at all. A session anchors its durable record,
/// policy, and contained paths to one root; repointing that root underneath the
/// session would invalidate all three, so the refusal is correct. Returning
/// silently was not.
#[tokio::test]
async fn the_root_is_locked_while_a_session_is_open_and_says_so() {
    let first = tempfile::tempdir().expect("first tempdir");
    let second = tempfile::tempdir().expect("second tempdir");
    let runtime = spawn_runtime();

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: first.path().to_path_buf(),
        })
        .await
        .expect("open first project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");

    let opened = runtime.snapshot().workspace_root.expect("first root");

    let error = runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: second.path().to_path_buf(),
        })
        .await
        .expect_err("the root is locked while a session is open");

    assert_eq!(refusal_code(&error), ReasonCode::WorkspaceRootLocked);
    assert_eq!(
        runtime.snapshot().workspace_root.as_ref(),
        Some(&opened),
        "a refused open must leave the original root in place"
    );

    runtime.close().await.expect("close");
}

/// `WorkspaceRootLocked` and `RunActive` are separate facts with separate
/// exits — end the session versus cancel the run — so they must not collapse
/// into one code. This pins the distinction the enum documents.
#[test]
fn the_locked_root_code_is_distinct_from_run_active() {
    assert_ne!(
        ReasonCode::WorkspaceRootLocked.as_str(),
        ReasonCode::RunActive.as_str()
    );
    assert_eq!(
        ReasonCode::WorkspaceRootLocked.as_str(),
        "WORKSPACE_ROOT_LOCKED"
    );
    assert_eq!(
        ReasonCode::parse("WORKSPACE_ROOT_LOCKED"),
        Some(ReasonCode::WorkspaceRootLocked)
    );
}

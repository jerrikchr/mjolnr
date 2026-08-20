//! The D3 producer: exact working-tree diffs reaching a client snapshot.
//!
//! One reason to change: whether the change set a client renders is what `git
//! diff` actually produced, bounded honestly, and paired with the repository
//! status from the same capture.
//!
//! Assertions go through `snapshot_to_client`, not the runtime's internal
//! state, for the reason the D5 producer tests record: a correct internal field
//! behind an empty projection is precisely the failure that shipped once
//! already, and only the client's value proves the wiring exists.

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helpers these
// tests share. Same allowance, same reason (AGENTS.md §7).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use mjolnr::core::changes::{ChangeSet, ChangeState, FileContent, FileStatus, LineKind};
use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::FakeProvider;
use mjolnr::runtime::Runtime;
use mjolnr::runtime::client_bridge::snapshot_to_client;
use mjolnr::store::memory::InMemoryEventStore;

fn spawn_runtime() -> Runtime {
    Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        Arc::new(InMemoryEventStore::new()) as Arc<dyn EventStore>,
    )
}

fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mjolnr-d3-producer-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical temp dir");

    git(&dir, &["init", "--initial-branch=main"]);
    git(&dir, &["config", "user.email", "test@mjolnr.invalid"]);
    git(&dir, &["config", "user.name", "mjolnr Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    fs::write(dir.join("README.md"), "one\ntwo\nthree\n").expect("write");
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-m", "init"]);
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

/// What a client would actually receive.
fn client_changes(runtime: &Runtime) -> Option<ChangeSet> {
    snapshot_to_client(1, &runtime.snapshot()).changes
}

async fn open(runtime: &Runtime, root: &Path) {
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: root.to_path_buf(),
        })
        .await
        .expect("open project");
}

#[tokio::test]
async fn a_modified_file_reaches_the_client_as_an_exact_diff() {
    // The bullet this closes: reviewing a multi-file change must not require
    // the user to run `git diff` themselves.
    let dir = setup_repo("modified");
    fs::write(dir.join("README.md"), "one\nTWO\nthree\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    let file = changes.files.first().expect("one changed file");
    assert_eq!(file.path, "README.md");
    assert_eq!(file.status, FileStatus::Modified);

    let hunk = file.hunks.first().expect("one hunk");
    let removed: Vec<_> = hunk
        .lines
        .iter()
        .filter(|line| line.kind == LineKind::Removed)
        .map(|line| (line.content.as_str(), line.old_line_number))
        .collect();
    let added: Vec<_> = hunk
        .lines
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .map(|line| (line.content.as_str(), line.new_line_number))
        .collect();
    assert_eq!(removed, vec![("two", Some(2))]);
    assert_eq!(added, vec![("TWO", Some(2))]);

    runtime.close().await.expect("close");
}

/// A working-tree read cannot tell a governed tool's write from a human's, so
/// the only state it may claim is the one it observed. `Applied` here would be
/// the false promotion §D3 requires a negative test against.
#[tokio::test]
async fn a_captured_change_set_never_claims_to_be_applied_or_verified() {
    let dir = setup_repo("state");
    fs::write(dir.join("README.md"), "changed\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");
    assert_eq!(changes.state, ChangeState::CurrentWorkingTree);

    // And the wire form carries no word a surface could promote.
    let json = serde_json::to_string(&changes).expect("serialize");
    for forbidden in ["\"applied\"", "\"verified\"", "\"proposed\""] {
        assert!(
            !json.contains(forbidden),
            "a working-tree capture must not carry {forbidden}: {json}"
        );
    }

    runtime.close().await.expect("close");
}

/// The pairing claim the two producers make together: a change set and the
/// repository status beside it must come from one capture, or a surface will
/// render a diff against a HEAD the status never saw.
#[tokio::test]
async fn a_change_set_and_its_repository_status_share_one_capture_sequence() {
    let dir = setup_repo("pairing");
    fs::write(dir.join("README.md"), "changed\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let snapshot = snapshot_to_client(1, &runtime.snapshot());
    let changes = snapshot.changes.expect("a change set");

    match snapshot.repository.freshness {
        mjolnr::core::client::workspace::RepositoryFreshness::CapturedAt { sequence, .. } => {
            assert_eq!(changes.capture_sequence, sequence);
        }
        other => panic!("expected a capture, got {other:?}"),
    }
    assert_eq!(
        changes.base_object_id, snapshot.repository.head,
        "the diff's base must be the HEAD the status reported"
    );

    runtime.close().await.expect("close");
}

/// The digest is what a review anchor is pinned to. It has to move when the
/// working tree moves — including when HEAD does not, which is the staleness a
/// commit id alone cannot see.
#[tokio::test]
async fn the_capture_digest_moves_when_the_working_tree_does_though_head_does_not() {
    let dir = setup_repo("digest");
    fs::write(dir.join("README.md"), "first edit\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let before = client_changes(&runtime).expect("a change set");

    fs::write(dir.join("README.md"), "second edit\n").expect("write");
    runtime
        .dispatch(MjolnrCommand::RefreshRepository)
        .await
        .expect("refresh");
    let after = client_changes(&runtime).expect("a change set");

    assert_eq!(
        before.base_object_id, after.base_object_id,
        "HEAD did not move in this test — that is the point"
    );
    assert_ne!(
        before.capture_digest, after.capture_digest,
        "a review note anchored to the first capture must be detectable as stale"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn an_untracked_file_is_diffed_without_touching_the_index() {
    let dir = setup_repo("untracked");
    fs::write(dir.join("new.txt"), "fresh\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    let file = changes
        .files
        .iter()
        .find(|file| file.path == "new.txt")
        .expect("the untracked file is in the change set");
    assert_eq!(file.status, FileStatus::Added);
    assert!(
        file.hunks
            .iter()
            .any(|hunk| hunk.lines.iter().any(|line| line.content == "fresh")),
        "an untracked file's content must be reviewable"
    );

    // The guard on *how* it was diffed: `--intent-to-add` would have staged it.
    // A read path that mutates the index is the failure this avoids.
    let porcelain = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(&dir)
        .output()
        .expect("git");
    assert!(
        porcelain.stdout.is_empty(),
        "capturing changes must not stage anything: {}",
        String::from_utf8_lossy(&porcelain.stdout)
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_binary_file_is_flagged_rather_than_rendered() {
    let dir = setup_repo("binary");
    fs::write(dir.join("blob.bin"), [0_u8, 159, 146, 150, 0, 1, 2]).expect("write");
    git(&dir, &["add", "blob.bin"]);
    git(&dir, &["commit", "-m", "add blob"]);
    fs::write(dir.join("blob.bin"), [0_u8, 1, 2, 3, 0, 9]).expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    let file = changes
        .files
        .iter()
        .find(|file| file.path == "blob.bin")
        .expect("the binary file is present");
    assert_eq!(file.content, FileContent::Binary);
    assert!(
        file.hunks.is_empty(),
        "a binary file carries no lines to review"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_renamed_file_keeps_its_old_path() {
    let dir = setup_repo("rename");
    git(&dir, &["mv", "README.md", "GUIDE.md"]);
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    let file = changes
        .files
        .iter()
        .find(|file| file.path == "GUIDE.md")
        .expect("the renamed file is present");
    assert_eq!(file.status, FileStatus::Renamed);
    assert_eq!(file.old_path.as_deref(), Some("README.md"));

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_deleted_file_is_reported_as_deleted() {
    let dir = setup_repo("delete");
    fs::remove_file(dir.join("README.md")).expect("remove");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    let file = changes.files.first().expect("one file");
    assert_eq!(file.path, "README.md");
    assert_eq!(file.status, FileStatus::Deleted);

    runtime.close().await.expect("close");
}

/// A directory that is not a repository must not produce an empty change set:
/// zero changed files reads as "mjolnr looked and nothing had changed", which is
/// a claim about a tree it could not read at all.
#[tokio::test]
async fn a_directory_that_is_not_a_repository_yields_no_change_set() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = spawn_runtime();

    open(&runtime, temp.path()).await;
    assert!(
        client_changes(&runtime).is_none(),
        "an unreadable repository must not render as one with no changes"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn no_project_open_means_no_change_set() {
    let runtime = spawn_runtime();
    assert!(client_changes(&runtime).is_none());
    runtime.close().await.expect("close");
}

/// Untracked *directories* cannot be diffed and must be named rather than
/// dropped — the surface has to be able to say what it is not showing.
#[tokio::test]
async fn an_untracked_directory_is_named_not_silently_omitted() {
    let dir = setup_repo("untracked-dir");
    fs::create_dir(dir.join("scratch")).expect("mkdir");
    fs::write(dir.join("scratch").join("a.txt"), "a\n").expect("write");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let changes = client_changes(&runtime).expect("a change set");

    assert!(
        changes
            .undiffed_untracked
            .iter()
            .any(|path| path.starts_with("scratch")),
        "expected the untracked directory to be named, got {:?}",
        changes.undiffed_untracked
    );

    runtime.close().await.expect("close");
}

/// A capture is a read. It must never leave the repository different from how
/// it found it — no staged paths, no moved HEAD, no stash.
#[tokio::test]
async fn capturing_changes_leaves_the_repository_untouched() {
    let dir = setup_repo("read-only");
    fs::write(dir.join("README.md"), "edited\n").expect("write");
    fs::write(dir.join("extra.txt"), "extra\n").expect("write");

    let status_before = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&dir)
        .output()
        .expect("git");
    let head_before = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&dir)
        .output()
        .expect("git");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;
    assert!(client_changes(&runtime).is_some());

    let status_after = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&dir)
        .output()
        .expect("git");
    let head_after = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&dir)
        .output()
        .expect("git");

    assert_eq!(status_before.stdout, status_after.stdout);
    assert_eq!(head_before.stdout, head_after.stdout);

    runtime.close().await.expect("close");
}

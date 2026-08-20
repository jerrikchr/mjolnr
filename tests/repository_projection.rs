//! The D5 producer: live repository truth reaching a client snapshot.
//!
//! One reason to change: whether what a client renders about the repository is
//! what git actually said, and whether it is honest about *when* git said it.
//!
//! The phase this closes had a gap of a specific shape — the contract existed,
//! the git operations existed, and nothing connected them, while the phase
//! report described a wiring (`empty_repository_state()` reaching the snapshot)
//! that was never called at all. So these tests assert on the value a client
//! receives, not on the runtime's internal state: an internal field that is
//! correct while the projection stays empty is the exact failure that shipped.

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helpers these
// tests share. Same allowance, same reason (AGENTS.md §7).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use mjolnr::core::client::workspace::{RepositoryFreshness, TrustClass};
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

/// A fresh repository per test with deterministic identity and signing off, so
/// a developer's global git config cannot make these fail for the wrong reason
/// — the lesson `a_signing_failure_never_becomes_an_unsigned_commit` taught.
fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mjolnr-d5-producer-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical temp dir");

    git(&dir, &["init", "--initial-branch=main"]);
    git(&dir, &["config", "user.email", "test@mjolnr.invalid"]);
    git(&dir, &["config", "user.name", "mjolnr Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    fs::write(dir.join("README.md"), "hello\n").expect("write");
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

/// What a client would actually receive. Every assertion goes through this
/// rather than the runtime snapshot, because the projection is where the
/// previous gap lived.
fn client_repository(runtime: &Runtime) -> mjolnr::core::client::workspace::RepositoryState {
    snapshot_to_client(1, &runtime.snapshot()).repository
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
async fn opening_a_repository_puts_live_status_on_the_client_snapshot() {
    let dir = setup_repo("open");
    let runtime = spawn_runtime();

    // Before: nothing is open, and the projection says exactly that rather
    // than describing a clean repository.
    assert_eq!(
        client_repository(&runtime).freshness,
        RepositoryFreshness::NoProject
    );

    open(&runtime, &dir).await;

    let state = client_repository(&runtime);
    assert_eq!(state.branch.as_deref(), Some("main"));
    assert!(
        state.head.is_some(),
        "an opened repository must report its HEAD"
    );
    assert_eq!(state.trust, TrustClass::MjolnrGoverned);
    match state.freshness {
        RepositoryFreshness::CapturedAt { trigger, sequence } => {
            assert_eq!(trigger, "projectOpened");
            assert_eq!(sequence, 1, "the first capture is sequence 1");
        }
        other => panic!("expected a capture, got {other:?}"),
    }

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_is_unavailable_not_clean() {
    // The failure mode: reporting "no branch, zero dirty files, no conflicts"
    // for a directory git was never able to answer about. That reads as a clean
    // repository, which is a positive claim about something unmeasured
    // (AGENTS.md §1.3).
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = spawn_runtime();

    open(&runtime, temp.path()).await;

    let state = client_repository(&runtime);
    assert_eq!(
        state.trust,
        TrustClass::ExternalUnverified,
        "an absence of evidence must not be labelled governed"
    );
    match state.freshness {
        RepositoryFreshness::Unavailable { code, detail } => {
            assert!(!code.is_empty(), "an unavailable repository carries a code");
            assert!(!detail.is_empty());
        }
        other => panic!("expected unavailable, got {other:?}"),
    }

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn staged_and_unstaged_work_reaches_the_client_separately() {
    let dir = setup_repo("paths");
    fs::write(dir.join("staged.rs"), "fn a() {}\n").expect("write");
    git(&dir, &["add", "staged.rs"]);
    fs::write(dir.join("README.md"), "changed\n").expect("write");
    fs::write(dir.join("untracked.txt"), "loose\n").expect("write");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let state = client_repository(&runtime);
    assert!(state.staged_files.iter().any(|p| p == "staged.rs"));
    assert!(state.modified_files.iter().any(|p| p == "README.md"));
    assert!(state.untracked_files.iter().any(|p| p == "untracked.txt"));
    // A staged file is not a worktree modification and vice versa; collapsing
    // them is how a surface offers to stage something already staged.
    assert!(!state.staged_files.iter().any(|p| p == "README.md"));
    assert!(state.dirty_count >= 3);

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_conflict_is_reported_as_unmerged_and_never_as_stageable() {
    let dir = setup_repo("conflict");
    git(&dir, &["checkout", "-b", "other"]);
    fs::write(dir.join("README.md"), "from other\n").expect("write");
    git(&dir, &["commit", "-am", "other side"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("README.md"), "from main\n").expect("write");
    git(&dir, &["commit", "-am", "main side"]);
    // Expected to fail: that is the point.
    let _ = Command::new("git")
        .args(["merge", "other"])
        .current_dir(&dir)
        .output()
        .expect("run git");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let state = client_repository(&runtime);
    assert!(
        state.unmerged_files.iter().any(|p| p == "README.md"),
        "a conflicted path must be reported as unmerged, got {state:?}"
    );
    assert!(
        !state.staged_files.iter().any(|p| p == "README.md"),
        "a conflict must never be offered as an ordinary stageable change"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_conflicted_index_still_yields_a_projection_without_an_index_revision() {
    // `git write-tree` refuses on an unmerged index. If that refusal failed the
    // whole projection, the repository surface would go blank exactly when a
    // human most needs to see it — so the projection survives and only the
    // advisory revision is absent.
    let dir = setup_repo("conflict-revision");
    git(&dir, &["checkout", "-b", "other"]);
    fs::write(dir.join("README.md"), "from other\n").expect("write");
    git(&dir, &["commit", "-am", "other side"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("README.md"), "from main\n").expect("write");
    git(&dir, &["commit", "-am", "main side"]);
    let _ = Command::new("git")
        .args(["merge", "other"])
        .current_dir(&dir)
        .output()
        .expect("run git");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let state = client_repository(&runtime);
    assert!(
        matches!(state.freshness, RepositoryFreshness::CapturedAt { .. }),
        "a conflicted repository must still produce a projection"
    );
    assert!(
        state.index_revision.is_none(),
        "an unmerged index cannot supply an expected revision, so none is offered"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_repository_command_refreshes_the_projection_and_advances_the_capture() {
    let dir = setup_repo("after-command");
    fs::write(dir.join("new.rs"), "fn b() {}\n").expect("write");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let before = client_repository(&runtime);
    assert!(before.staged_files.is_empty());
    let before_sequence = capture_sequence(&before);

    runtime
        .dispatch(MjolnrCommand::StagePaths {
            paths: vec!["new.rs".to_owned()],
        })
        .await
        .expect("stage");

    let after = client_repository(&runtime);
    assert!(
        after.staged_files.iter().any(|p| p == "new.rs"),
        "staging must be visible without the client asking again"
    );
    assert!(
        capture_sequence(&after) > before_sequence,
        "a completed refresh advances the capture sequence"
    );
    match after.freshness {
        RepositoryFreshness::CapturedAt { trigger, .. } => {
            assert_eq!(trigger, "repositoryCommand");
        }
        other => panic!("expected a capture, got {other:?}"),
    }

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_refused_repository_command_still_refreshes_rather_than_leaving_a_stale_view() {
    // Refreshing only on success would leave the surface most wrong in the case
    // that matters most: a command that failed after changing something.
    let dir = setup_repo("after-refusal");
    let runtime = spawn_runtime();
    open(&runtime, &dir).await;
    let before = capture_sequence(&client_repository(&runtime));

    // Nothing staged, so this is refused.
    let refused = runtime
        .dispatch(MjolnrCommand::Commit {
            message: "nothing to commit".to_owned(),
            expected_index_revision: "0000000000000000000000000000000000000000".to_owned(),
        })
        .await;
    assert!(refused.is_err(), "an empty index must refuse a commit");

    assert!(
        capture_sequence(&client_repository(&runtime)) > before,
        "a refusal must still re-read the repository"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn an_explicit_refresh_is_refused_when_no_project_is_open() {
    // "Nothing to read" and "read and found nothing" are different answers, and
    // the caller asked a question.
    let runtime = spawn_runtime();

    let refused = runtime.dispatch(MjolnrCommand::RefreshRepository).await;
    assert!(
        refused.is_err(),
        "a refresh with no project open must refuse, not report an empty repository"
    );
    assert_eq!(
        client_repository(&runtime).freshness,
        RepositoryFreshness::NoProject
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn an_explicit_refresh_picks_up_a_change_made_outside_mjolnr() {
    // The honest limit of the whole design: between triggers, work done in a
    // terminal is invisible. This proves the manual trigger is the remedy, and
    // that the projection before it was stale rather than wrong about itself.
    let dir = setup_repo("external-change");
    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let before = client_repository(&runtime);
    assert!(before.untracked_files.is_empty());

    fs::write(dir.join("outside.txt"), "written in a terminal\n").expect("write");

    // Still unaware — and saying so, because the capture it names is the older
    // one. This is the state the freshness marker exists to disclose.
    let stale = client_repository(&runtime);
    assert!(stale.untracked_files.is_empty());
    assert_eq!(capture_sequence(&stale), capture_sequence(&before));

    runtime
        .dispatch(MjolnrCommand::RefreshRepository)
        .await
        .expect("refresh");

    let fresh = client_repository(&runtime);
    assert!(fresh.untracked_files.iter().any(|p| p == "outside.txt"));
    assert!(capture_sequence(&fresh) > capture_sequence(&before));
    match fresh.freshness {
        RepositoryFreshness::CapturedAt { trigger, .. } => assert_eq!(trigger, "requested"),
        other => panic!("expected a capture, got {other:?}"),
    }

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn the_projection_carries_an_index_revision_a_commit_can_be_armed_with() {
    // §D5 acceptance: a modifying operation shows the expected index revision
    // *before* approval. Without this field on the wire there is no path by
    // which a client could display it, which is why the bullet was unmet rather
    // than merely undelivered.
    let dir = setup_repo("expected-revision");
    fs::write(dir.join("armed.rs"), "fn c() {}\n").expect("write");
    git(&dir, &["add", "armed.rs"]);

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    let revision = client_repository(&runtime)
        .index_revision
        .expect("a staged index must offer an expected revision");

    runtime
        .dispatch(MjolnrCommand::Commit {
            message: "armed with the projected revision".to_owned(),
            expected_index_revision: revision,
        })
        .await
        .expect("the projected revision must be the one the commit accepts");

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_stale_index_revision_from_an_old_projection_is_still_refused() {
    // The freshness marker is disclosure, not permission: rendering a stale
    // projection must not weaken the guard the commit path applies.
    let dir = setup_repo("stale-revision");
    fs::write(dir.join("first.rs"), "fn d() {}\n").expect("write");
    git(&dir, &["add", "first.rs"]);

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;
    let captured = client_repository(&runtime)
        .index_revision
        .expect("staged index");

    // The index moves after that projection was taken.
    fs::write(dir.join("second.rs"), "fn e() {}\n").expect("write");
    git(&dir, &["add", "second.rs"]);

    let refused = runtime
        .dispatch(MjolnrCommand::Commit {
            message: "armed with a revision that has since moved".to_owned(),
            expected_index_revision: captured,
        })
        .await;
    assert!(
        refused.is_err(),
        "a commit armed from a superseded projection must fail closed"
    );

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn ending_a_session_keeps_the_project_repository_visible() {
    // The repository belongs to the project. Blanking it on every new session
    // and refilling it on the next unrelated trigger reads as data appearing
    // for no reason.
    let dir = setup_repo("across-sessions");
    let runtime = spawn_runtime();
    open(&runtime, &dir).await;
    let before = client_repository(&runtime);

    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: mjolnr::core::model::ProviderId::new("fake"),
            model: mjolnr::core::model::ModelId::new("fake-1"),
        })
        .await
        .expect("create session");

    let after = client_repository(&runtime);
    assert_eq!(
        after.branch, before.branch,
        "a new session must not blank the project's repository view"
    );
    assert!(matches!(
        after.freshness,
        RepositoryFreshness::CapturedAt { .. }
    ));

    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_governed_tool_write_refreshes_the_projection() {
    // The fourth trigger. An agent edit that does not move the repository
    // surface is the same defect as a stale status after a commit, just harder
    // to notice: the user watches mjolnr change a file and the Changes panel
    // keeps insisting the tree is clean.
    use mjolnr::core::command::ApprovalDecision;
    use mjolnr::core::event::MjolnrEvent;
    use mjolnr::providers::fake::FakeScript;

    let dir = setup_repo("tool-write");
    fs::write(dir.join("fixture.txt"), "before\n").expect("fixture");
    git(&dir, &["add", "fixture.txt"]);
    git(&dir, &["commit", "-m", "add fixture"]);

    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::GuardedLoop));
    let runtime = Runtime::spawn(
        vec![provider],
        Arc::new(InMemoryEventStore::new()) as Arc<dyn EventStore>,
    );
    let mut events = runtime.subscribe();

    open(&runtime, &dir).await;
    let before = capture_sequence(&client_repository(&runtime));
    assert!(
        client_repository(&runtime).modified_files.is_empty(),
        "the tree starts clean"
    );

    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: mjolnr::core::model::ProviderId::new(FakeProvider::ID),
            model: mjolnr::core::model::ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "update the fixture and verify it".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    // Approve just the edit; the run's later steps are not this test's subject.
    let proposed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(MjolnrEvent::ToolProposed {
                    approval: Some(approval),
                    call,
                    ..
                }) if call.name == "edit_file" => return approval,
                Ok(_) => {}
                Err(error) => panic!("event feed ended: {error}"),
            }
        }
    })
    .await
    .expect("an edit was proposed");

    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
            approval: proposed,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve the edit");

    // Wait on the *snapshot stream*, not the event feed. `ToolCompleted` is
    // published when the write is recorded, which is strictly before the
    // refresh it triggers has finished — reading the snapshot on that event is
    // a race, and one that would have been "fixed" by a sleep. The event feed
    // says what happened; the snapshot stream says what is now true, and this
    // assertion is about the latter.
    let after = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut snapshots = runtime.snapshots();
        loop {
            let snapshot = snapshots.changed().await.expect("snapshot stream");
            let state = snapshot_to_client(1, &snapshot).repository;
            if state.modified_files.iter().any(|p| p == "fixture.txt") {
                return state;
            }
        }
    })
    .await
    .expect("a governed edit must move the repository projection");

    assert!(capture_sequence(&after) > before);
    match after.freshness {
        RepositoryFreshness::CapturedAt { trigger, .. } => assert_eq!(trigger, "toolWrite"),
        other => panic!("expected a capture, got {other:?}"),
    }
    // The event feed still has to carry the completion — the projection moving
    // is not a substitute for the durable record.
    drop(events);

    runtime.close().await.expect("close");
}

fn capture_sequence(state: &mjolnr::core::client::workspace::RepositoryState) -> u32 {
    match &state.freshness {
        RepositoryFreshness::CapturedAt { sequence, .. } => *sequence,
        other => panic!("expected a capture, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Remote sync from the last-seen tracking ref (ADR 0008)
// ---------------------------------------------------------------------------

/// A bare "remote" plus a clone of it, wired as upstream. No server, no network:
/// a local path is a perfectly good git remote, which is exactly what makes the
/// no-network property testable rather than merely asserted.
fn setup_repo_with_upstream(name: &str) -> PathBuf {
    let origin = std::env::temp_dir().join(format!("mjolnr-d5-origin-{name}.git"));
    let clone = std::env::temp_dir().join(format!("mjolnr-d5-clone-{name}"));
    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone);

    let source = setup_repo(&format!("upstream-src-{name}"));
    // `--initial-branch=main` on the *bare* repository too, not only on the
    // working one. Without it the bare repo's HEAD comes from the ambient
    // `init.defaultBranch`; where that is unset git 2.43 picks `master`, the
    // clone finds a HEAD pointing at a ref the push never created, checks out
    // nothing, and has no upstream — so all three sync tests below asserted
    // against `Unknown` and failed on a machine configured differently from
    // the author's. A test whose result depends on the developer's global git
    // config is not deterministic (AGENTS.md §7).
    git(
        &source,
        &[
            "init",
            "--bare",
            "--quiet",
            "--initial-branch=main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&source, &["push", "-u", "origin", "main"]);

    let temp_dir = std::env::temp_dir();
    git(
        &temp_dir,
        &["clone", origin.to_str().unwrap(), clone.to_str().unwrap()],
    );
    let clone = clone.canonicalize().expect("canonical clone");
    git(&clone, &["config", "user.email", "test@mjolnr.invalid"]);
    git(&clone, &["config", "user.name", "mjolnr Test"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);
    clone
}

#[tokio::test]
async fn a_branch_level_with_its_upstream_reports_synced() {
    let dir = setup_repo_with_upstream("level");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let state = client_repository(&runtime);

    assert_eq!(
        state.remote_sync,
        mjolnr::core::client::workspace::RepositorySyncState::Synced
    );
    // `remote_sync_as_of` is deliberately NOT asserted present. A fresh clone
    // writes its tracking ref without a reflog entry — measured, not assumed —
    // so `None` here is the normal case, and the design says so: the *qualifier*
    // a surface renders comes from the variant's meaning, and this timestamp
    // only sharpens it. Asserting it would pin behaviour git does not promise.
    //
    // What must never happen is `Synced` rendering as a bare "synced", and that
    // is a rendering obligation the surface test owns, not this one.

    runtime.close().await.expect("close");
}

/// Ahead and behind must not be transposed against real git output — the unit
/// test covers the mapping, this covers what git actually prints.
#[tokio::test]
async fn a_local_commit_reports_ahead_not_behind() {
    let dir = setup_repo_with_upstream("ahead");
    fs::write(dir.join("local.txt"), "local work\n").expect("write");
    git(&dir, &["add", "local.txt"]);
    git(&dir, &["commit", "-m", "local work"]);

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    assert_eq!(
        client_repository(&runtime).remote_sync,
        mjolnr::core::client::workspace::RepositorySyncState::Ahead { count: 1 },
        "one unpushed commit is one ahead, never one behind"
    );

    runtime.close().await.expect("close");
}

/// A repository with no upstream reports `Unknown` — which now means exactly
/// "nothing to compare against", not "mjolnr did not look".
#[tokio::test]
async fn no_upstream_reports_unknown_rather_than_a_fabricated_position() {
    let dir = setup_repo("no-upstream");
    let runtime = spawn_runtime();

    open(&runtime, &dir).await;
    let state = client_repository(&runtime);
    assert_eq!(
        state.remote_sync,
        mjolnr::core::client::workspace::RepositorySyncState::Unknown
    );
    assert_eq!(state.remote_sync_as_of, None);

    runtime.close().await.expect("close");
}

/// The load-bearing property: computing sync position performs no network I/O.
///
/// Proved by deleting the remote entirely. Every ref the computation needs is
/// already local, so it must still answer — a version that reached the network
/// would fail here, and that failure is the test.
#[tokio::test]
async fn sync_position_is_computed_without_contacting_the_remote() {
    let dir = setup_repo_with_upstream("offline");
    fs::write(dir.join("local.txt"), "local work\n").expect("write");
    git(&dir, &["add", "local.txt"]);
    git(&dir, &["commit", "-m", "local work"]);

    // Remove the remote repository from disk. Nothing reachable remains.
    let origin = std::env::temp_dir().join("mjolnr-d5-origin-offline.git");
    fs::remove_dir_all(&origin).expect("remove origin");

    let runtime = spawn_runtime();
    open(&runtime, &dir).await;

    assert_eq!(
        client_repository(&runtime).remote_sync,
        mjolnr::core::client::workspace::RepositorySyncState::Ahead { count: 1 },
        "the comparison must use the local tracking ref, not the remote"
    );

    runtime.close().await.expect("close");
}

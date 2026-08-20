//! Phase D5 governed source control against real `git`.
//!
//! The positive paths matter less here than the negative ones: every state the
//! `RepositoryError` taxonomy promises to detect gets a test that proves the
//! detection, because a guard without a negative test is decorative
//! (AGENTS.md §7).

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helper
// functions these tests share. Same allowance, same reason (AGENTS.md §7):
// clarity beats ceremony in a test's setup.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mjolnr::core::error::ReasonCode;
use mjolnr::repository::{Repository, RepositoryError};

/// A fresh repository per test, named by the caller, with deterministic
/// identity and signing explicitly disabled so a developer's global
/// `commit.gpgsign = true` cannot make these tests fail for the wrong reason.
fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mjolnr-d5-{name}"));
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

fn git_stdout(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("run git");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn open(dir: &Path) -> Repository {
    Repository::open(dir).expect("open repository")
}

// ---------------------------------------------------------------------------
// Constructor and projection
// ---------------------------------------------------------------------------

#[test]
fn a_directory_that_is_not_a_repository_is_refused_at_open() {
    let dir = std::env::temp_dir().join("mjolnr-d5-not-a-repo");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical");

    let error = Repository::open(&dir).expect_err("must refuse");
    assert!(matches!(error, RepositoryError::NotARepository { .. }));
    assert_eq!(
        error.reason_code(),
        ReasonCode::WorkspaceCapabilityUnavailable
    );
}

#[test]
fn status_and_projections_separate_staged_from_untracked() {
    let dir = setup_repo("projections");
    let repository = open(&dir);

    assert_eq!(repository.status().expect("status").dirty_count, 0);

    fs::write(dir.join("one.txt"), "a").expect("write");
    fs::write(dir.join("two.txt"), "b").expect("write");
    assert_eq!(repository.status().expect("status").dirty_count, 2);

    let (index, worktree) = repository.projections().expect("projections");
    assert!(index.staged_files.is_empty());
    assert_eq!(worktree.untracked_files.len(), 2);

    repository
        .stage_paths(&["one.txt".to_owned()])
        .expect("stage");
    let (index, worktree) = repository.projections().expect("projections");
    assert_eq!(index.staged_files, vec!["one.txt"]);
    assert_eq!(worktree.untracked_files, vec!["two.txt"]);

    repository
        .unstage_paths(&["one.txt".to_owned()])
        .expect("unstage");
    assert!(
        repository
            .projections()
            .expect("projections")
            .0
            .staged_files
            .is_empty()
    );
}

#[test]
fn state_reports_the_current_branch_and_head() {
    let dir = setup_repo("state-branch");
    let state = open(&dir).status().expect("status");
    assert_eq!(state.branch.as_deref(), Some("main"));
    assert!(state.head.is_some_and(|head| head.len() == 40));
}

#[test]
fn history_is_newest_first_and_explicitly_bounded() {
    let dir = setup_repo("history");
    fs::write(dir.join("second.txt"), "second\n").expect("write");
    git(&dir, &["add", "second.txt"]);
    git(&dir, &["commit", "-m", "second"]);

    let history = open(&dir).history(1).expect("history");
    assert_eq!(history.entries.len(), 1);
    assert_eq!(
        history.entries.first().map(|entry| entry.subject.as_str()),
        Some("second")
    );
    assert!(history.has_more);

    let complete = open(&dir).history(50).expect("complete history");
    assert_eq!(complete.entries.len(), 2);
    assert!(!complete.has_more);
}

#[test]
fn clone_requires_a_new_destination_and_verifies_the_result() {
    let source = setup_repo("clone-source");
    let destination = std::env::temp_dir().join("mjolnr-d5-clone-destination");
    let _ = fs::remove_dir_all(&destination);

    let cloned = Repository::clone_project(source.to_str().expect("source path"), &destination)
        .expect("clone");
    assert_eq!(cloned, destination);
    assert_eq!(
        open(&destination)
            .status()
            .expect("cloned status")
            .branch
            .as_deref(),
        Some("main")
    );

    let error = Repository::clone_project(source.to_str().expect("source path"), &destination)
        .expect_err("must not overwrite an existing destination");
    assert!(matches!(error, RepositoryError::InvalidRoot { .. }));
}

#[test]
fn rebase_moves_a_clean_branch_onto_the_named_ref() {
    let dir = setup_repo("rebase-happy");
    fs::write(dir.join("base.txt"), "base\n").expect("write");
    git(&dir, &["add", "base.txt"]);
    git(&dir, &["commit", "-m", "base"]);
    git(&dir, &["checkout", "-b", "feature"]);
    fs::write(dir.join("feature.txt"), "feature\n").expect("write");
    git(&dir, &["add", "feature.txt"]);
    git(&dir, &["commit", "-m", "feature"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("main.txt"), "main\n").expect("write");
    git(&dir, &["add", "main.txt"]);
    git(&dir, &["commit", "-m", "main"]);
    let main_head = git_stdout(&dir, &["rev-parse", "main"]);
    git(&dir, &["checkout", "feature"]);
    let feature_head = git_stdout(&dir, &["rev-parse", "HEAD"]);

    let rebased = open(&dir).rebase("main", &feature_head).expect("rebase");
    assert_ne!(rebased, feature_head);
    assert_eq!(git_stdout(&dir, &["rev-parse", "HEAD~1"]), main_head);
    assert!(
        !open(&dir)
            .project(mjolnr::core::repository::RefreshTrigger::Requested, 1)
            .expect("projection")
            .rebase_in_progress
    );
}

#[test]
fn rebase_conflict_is_left_for_human_recovery_and_abort_clears_it() {
    let dir = setup_repo("rebase-conflict");
    git(&dir, &["checkout", "-b", "feature"]);
    fs::write(dir.join("README.md"), "feature\n").expect("write");
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-m", "feature"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("README.md"), "main\n").expect("write");
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-m", "main"]);
    git(&dir, &["checkout", "feature"]);
    let feature_head = git_stdout(&dir, &["rev-parse", "HEAD"]);

    let error = open(&dir)
        .rebase("main", &feature_head)
        .expect_err("must report conflict");
    assert!(matches!(error, RepositoryError::Conflict { .. }));
    assert!(
        open(&dir)
            .project(mjolnr::core::repository::RefreshTrigger::Requested, 1)
            .expect("projection")
            .rebase_in_progress
    );
    open(&dir).abort_rebase().expect("abort rebase");
    assert!(
        !open(&dir)
            .project(mjolnr::core::repository::RefreshTrigger::Requested, 2)
            .expect("projection")
            .rebase_in_progress
    );
}

/// The repository module reports its own status type and no trust class.
/// Grading how a result is trusted belongs to the bridge, so a module that
/// performs git side effects cannot also grade them (ADR 0006). This test
/// exists to fail if someone reintroduces the client DTO here.
#[test]
fn status_reports_the_repository_modules_own_type_and_grades_no_trust() {
    let dir = setup_repo("state-type");
    let status: mjolnr::repository::RepositoryStatus = open(&dir).status().expect("status");
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(!status.dirty_count_truncated);
}

#[test]
fn a_detached_head_reports_no_branch_rather_than_inventing_one() {
    let dir = setup_repo("detached-state");
    let head = open(&dir).status().expect("status").head.expect("head");
    git(&dir, &["checkout", "--detach", &head]);

    let state = open(&dir).status().expect("status");
    assert_eq!(state.branch, None);
    assert_eq!(state.head.as_deref(), Some(head.as_str()));
}

// ---------------------------------------------------------------------------
// Commit: the staleness guard and post-effect verification
// ---------------------------------------------------------------------------

#[test]
fn a_commit_returns_the_head_it_actually_created() {
    let dir = setup_repo("commit-happy");
    let repository = open(&dir);
    let before = repository.status().expect("status").head.expect("head");

    fs::write(dir.join("feature.txt"), "x").expect("write");
    repository
        .stage_paths(&["feature.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");

    let after = repository.commit("add feature", &index).expect("commit");
    assert_ne!(after, before);
    // The returned SHA is re-read from the repository, not assumed.
    assert_eq!(
        repository.status().expect("status").head.as_deref(),
        Some(after.as_str())
    );
}

#[test]
fn a_commit_against_a_stale_index_revision_fails_closed() {
    let dir = setup_repo("commit-stale");
    let repository = open(&dir);

    fs::write(dir.join("a.txt"), "a").expect("write");
    repository
        .stage_paths(&["a.txt".to_owned()])
        .expect("stage");
    let approved = repository.index_revision().expect("index revision");

    // Something else stages more work after the human approved the preview.
    fs::write(dir.join("b.txt"), "b").expect("write");
    repository
        .stage_paths(&["b.txt".to_owned()])
        .expect("stage");

    let error = repository
        .commit("add a", &approved)
        .expect_err("must refuse a moved index");
    assert!(matches!(error, RepositoryError::StaleIndex { .. }));
    assert_eq!(error.reason_code(), ReasonCode::WorkspaceStaleRevision);
    // Refused means nothing ran: the work is still uncommitted.
    assert_eq!(
        repository
            .projections()
            .expect("projections")
            .0
            .staged_files,
        vec!["a.txt", "b.txt"]
    );
}

#[test]
fn a_commit_with_an_empty_index_is_refused_rather_than_creating_an_empty_commit() {
    let dir = setup_repo("commit-empty");
    let repository = open(&dir);
    let before = repository.status().expect("status").head;
    let index = repository.index_revision().expect("index revision");

    let error = repository
        .commit("nothing", &index)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::NothingStaged));
    assert_eq!(repository.status().expect("status").head, before);
}

#[test]
fn a_refusing_pre_commit_hook_is_reported_as_a_hook_refusal_not_a_success() {
    let dir = setup_repo("commit-hook");
    let repository = open(&dir);
    write_failing_hook(&dir, "pre-commit");

    fs::write(dir.join("a.txt"), "a").expect("write");
    repository
        .stage_paths(&["a.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");
    let before = repository.status().expect("status").head;

    let error = repository.commit("blocked", &index).expect_err("must fail");
    assert!(
        matches!(error, RepositoryError::HookRefused { .. }),
        "expected a hook refusal, got {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryHookRefused);
    // The hook is the owner's own gate; mjolnr reports it and HEAD is untouched.
    assert_eq!(repository.status().expect("status").head, before);
    // And the hook's own message reaches the human verbatim.
    assert!(error.to_string().contains("refused by policy"));
}

#[test]
fn a_signing_failure_never_becomes_an_unsigned_commit() {
    let dir = setup_repo("commit-signing");
    let repository = open(&dir);
    git(&dir, &["config", "commit.gpgsign", "true"]);
    // `gpg.program` is only consulted for the openpgp format, so the format is
    // pinned before the sabotage. Without this line an ambient
    // `gpg.format = ssh` — a machine that signs its commits with an SSH key —
    // routes signing to `gpg.ssh.program` instead, the sabotage misses
    // entirely, git returns a successful *unsigned* commit, and the test fails
    // having proved nothing about `src/repository/`. Same reason `setup_repo`
    // pins `commit.gpgsign`: a test that reads the developer's global git
    // config is not deterministic (AGENTS.md §7).
    git(&dir, &["config", "gpg.format", "openpgp"]);
    // A signing program that always fails.
    git(&dir, &["config", "gpg.program", "/usr/bin/false"]);

    fs::write(dir.join("a.txt"), "a").expect("write");
    repository
        .stage_paths(&["a.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");
    let before = repository.status().expect("status").head;

    let error = repository.commit("signed", &index).expect_err("must fail");
    assert_eq!(
        repository.status().expect("status").head,
        before,
        "a failed signature must not leave a commit behind"
    );
    // Either classification is honest; what must never happen is `Ok`.
    assert!(
        matches!(
            error,
            RepositoryError::SigningFailed { .. } | RepositoryError::CommandFailed { .. }
        ),
        "unexpected variant {error:?}"
    );
}

// ---------------------------------------------------------------------------
// Branch creation
// ---------------------------------------------------------------------------

#[test]
fn creating_a_branch_never_moves_an_existing_one() {
    let dir = setup_repo("branch-existing");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");
    repository.create_branch("feature", &head).expect("create");

    // Advance main so a permissive implementation would silently reassign.
    fs::write(dir.join("a.txt"), "a").expect("write");
    repository
        .stage_paths(&["a.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");
    let moved = repository.commit("advance", &index).expect("commit");

    let error = repository
        .create_branch("feature", &moved)
        .expect_err("must refuse to reassign");
    assert!(matches!(error, RepositoryError::CommandFailed { .. }));
    assert_eq!(git_stdout(&dir, &["rev-parse", "feature"]), head);
}

// ---------------------------------------------------------------------------
// Integration merge: conflict, detached head, dirty tree, stale head
// ---------------------------------------------------------------------------

#[test]
fn an_integration_merge_uses_the_human_supplied_message() {
    let dir = setup_repo("merge-message");
    let repository = open(&dir);
    make_divergent_branch(&dir, &repository, "child", "child.txt", "child\n");

    let head = repository.status().expect("status").head.expect("head");
    let merged = repository
        .integrate_child_branch("child", "Take the child's work after review", &head)
        .expect("merge");

    assert_eq!(
        git_stdout(&dir, &["log", "-1", "--format=%s", &merged]),
        "Take the child's work after review",
        "the merge commit must carry the human's message, not a generated one"
    );
}

#[test]
fn a_conflicting_integration_is_reported_and_never_resolved_for_the_human() {
    let dir = setup_repo("merge-conflict");
    let repository = open(&dir);
    // Both branches change the same file, so the merge cannot succeed.
    make_divergent_branch(&dir, &repository, "child", "shared.txt", "child version\n");
    fs::write(dir.join("shared.txt"), "main version\n").expect("write");
    repository
        .stage_paths(&["shared.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");
    let head = repository
        .commit("main takes shared", &index)
        .expect("commit");

    let error = repository
        .integrate_child_branch("child", "merge", &head)
        .expect_err("must refuse");
    assert!(
        matches!(error, RepositoryError::Conflict { .. }),
        "unexpected variant {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryConflict);

    // A second attempt sees the pre-existing conflict and refuses before
    // touching anything, rather than compounding it.
    let again = repository
        .integrate_child_branch("child", "merge", &head)
        .expect_err("must refuse again");
    assert!(matches!(again, RepositoryError::Conflict { .. }));
}

#[test]
fn an_integration_onto_a_detached_head_is_refused() {
    let dir = setup_repo("merge-detached");
    let repository = open(&dir);
    make_divergent_branch(&dir, &repository, "child", "child.txt", "child\n");
    let head = repository.status().expect("status").head.expect("head");
    git(&dir, &["checkout", "--detach", &head]);

    let error = open(&dir)
        .integrate_child_branch("child", "merge", &head)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::DetachedHead));
    assert_eq!(error.reason_code(), ReasonCode::RepositoryDetachedHead);
}

#[test]
fn an_integration_into_a_dirty_tree_is_refused_rather_than_stashing() {
    let dir = setup_repo("merge-dirty");
    let repository = open(&dir);
    make_divergent_branch(&dir, &repository, "child", "child.txt", "child\n");
    let head = repository.status().expect("status").head.expect("head");

    // Uncommitted work an automatic stash would have swallowed.
    fs::write(dir.join("README.md"), "edited but not committed\n").expect("write");

    let error = repository
        .integrate_child_branch("child", "merge", &head)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::DirtyTree));
    assert_eq!(error.reason_code(), ReasonCode::WorkspaceDirty);
    assert_eq!(
        fs::read_to_string(dir.join("README.md")).expect("read"),
        "edited but not committed\n",
        "the human's uncommitted work must survive the refusal"
    );
}

#[test]
fn an_integration_against_a_stale_head_fails_closed() {
    let dir = setup_repo("merge-stale");
    let repository = open(&dir);
    make_divergent_branch(&dir, &repository, "child", "child.txt", "child\n");
    let stale = repository.status().expect("status").head.expect("head");

    // Main advances after the human read the preview.
    fs::write(dir.join("a.txt"), "a").expect("write");
    repository
        .stage_paths(&["a.txt".to_owned()])
        .expect("stage");
    let index = repository.index_revision().expect("index revision");
    let current = repository.commit("advance main", &index).expect("commit");

    let error = repository
        .integrate_child_branch("child", "merge", &stale)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::StaleIndex { .. }));
    assert_eq!(
        repository.status().expect("status").head.as_deref(),
        Some(current.as_str()),
        "a refused merge leaves HEAD where it was"
    );
}

// ---------------------------------------------------------------------------
// Capability honesty
// ---------------------------------------------------------------------------

#[test]
fn hunk_staging_reports_unavailable_rather_than_staging_the_whole_file() {
    let dir = setup_repo("hunks");
    let repository = open(&dir);
    fs::write(dir.join("README.md"), "hello\nworld\n").expect("write");

    let error = repository
        .stage_hunks("README.md", &[0])
        .expect_err("must refuse");
    assert!(matches!(
        error,
        RepositoryError::CapabilityUnavailable { .. }
    ));
    assert!(
        repository
            .projections()
            .expect("projections")
            .0
            .staged_files
            .is_empty(),
        "an unavailable capability must not fall back to a coarser write"
    );
}

// ---------------------------------------------------------------------------
// Fetch and push: governed remote operations
// ---------------------------------------------------------------------------

#[test]
fn a_first_push_lands_on_the_remote_and_is_verified_via_the_tracking_ref() {
    let dir = setup_repo_with_upstream("push-first");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");

    repository
        .push(&head)
        .expect("push succeeds and is verified");

    // The remote-tracking ref advanced to exactly the pushed commit: that is
    // the evidence the push landed, not git's exit status.
    assert_eq!(git_stdout(&dir, &["rev-parse", "@{upstream}"]), head);
    // And the bare remote actually has the commit at its branch tip.
    let bare = git_stdout(&dir, &["config", "--get", "remote.origin.url"]);
    assert_eq!(
        git_stdout(Path::new(&bare), &["rev-parse", "refs/heads/main"]),
        head
    );
}

#[test]
fn a_push_refuses_when_the_branch_is_behind_the_remote() {
    let dir = setup_repo_with_upstream("push-diverged");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("first push");

    // Move HEAD back one commit so the local branch is behind its own
    // remote-tracking ref; the remote still holds `head`.
    git(&dir, &["reset", "--hard", "HEAD~1"]);
    let behind_head = repository.status().expect("status").head.expect("head");

    let error = repository
        .push(&behind_head)
        .expect_err("must refuse a diverged push");
    assert!(
        matches!(error, RepositoryError::DivergedFromRemote { behind: 1, .. }),
        "expected a divergence refusal, got {error:?}"
    );
    assert_eq!(
        error.reason_code(),
        ReasonCode::RepositoryDivergedFromRemote
    );
    // Refused before the network: HEAD is untouched.
    assert_eq!(repository.status().expect("status").head, Some(behind_head));
}

#[test]
fn a_push_against_a_stale_head_fails_closed() {
    let dir = setup_repo_with_upstream("push-stale");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");

    // Approve pushing `head`, then move HEAD forward before dispatch.
    fs::write(dir.join("new.txt"), "x\n").expect("write");
    git(&dir, &["add", "new.txt"]);
    git(&dir, &["commit", "-m", "moved on"]);

    let error = repository
        .push(&head)
        .expect_err("must refuse a stale head");
    assert!(matches!(error, RepositoryError::StaleIndex { .. }));
    assert_eq!(error.reason_code(), ReasonCode::WorkspaceStaleRevision);
}

#[test]
fn a_push_with_no_upstream_configured_is_refused_before_the_network() {
    let dir = setup_repo("push-no-upstream");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");

    let error = repository
        .push(&head)
        .expect_err("must refuse without an upstream");
    assert!(
        matches!(error, RepositoryError::NoUpstream { .. }),
        "expected a no-upstream refusal, got {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryNoUpstream);
}

#[test]
fn a_push_from_a_detached_head_is_refused() {
    let dir = setup_repo_with_upstream("push-detached");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");
    git(&dir, &["checkout", "--detach", &head]);

    let error = repository
        .push(&head)
        .expect_err("must refuse a detached head");
    assert!(matches!(error, RepositoryError::DetachedHead));
    assert_eq!(error.reason_code(), ReasonCode::RepositoryDetachedHead);
}

#[test]
fn a_refusing_pre_push_hook_is_reported_and_leaves_the_local_state_untouched() {
    let dir = setup_repo_with_upstream("push-hook");
    let repository = open(&dir);
    write_failing_hook(&dir, "pre-push");
    fs::write(dir.join("work.txt"), "blocked\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "hooked work"]);
    let head = repository.status().expect("status").head.expect("head");

    let error = repository
        .push(&head)
        .expect_err("the hook must block the push");
    assert!(
        matches!(
            error,
            RepositoryError::HookRefused {
                hook: "pre-push",
                ..
            }
        ),
        "expected a pre-push hook refusal, got {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryHookRefused);
    // The hook is the owner's own gate and runs before transfer, so the push
    // did not land: HEAD is still the commit the human tried to push.
    assert_eq!(repository.status().expect("status").head, Some(head));
}

#[test]
fn a_fetch_from_the_upstream_updates_the_local_tracking_ref() {
    let dir = setup_repo_with_upstream("fetch-update");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("push");

    // The remote advances from a second clone; the local tracking ref stays
    // at `head` until mjolnr fetches.
    let bare = git_stdout(&dir, &["config", "--get", "remote.origin.url"]);
    advance_remote_one_commit(Path::new(&bare), "fetch-update");
    assert_eq!(git_stdout(&dir, &["rev-parse", "@{upstream}"]), head);

    repository.fetch().expect("fetch is inert and succeeds");

    // The tracking ref now reflects the remote's advanced commit, not `head`.
    let after = git_stdout(&dir, &["rev-parse", "@{upstream}"]);
    assert_ne!(after, head);
    assert_eq!(git_stdout(&dir, &["rev-parse", "origin/main"]), after);
}

// ---------------------------------------------------------------------------
// Integrate upstream: the merge half of "pull"
// ---------------------------------------------------------------------------

#[test]
fn integrating_a_fetched_upstream_fast_forwards_to_the_upstream_tip() {
    let dir = setup_repo_with_upstream("merge-upstream-ff");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("push");

    // The remote advances; mjolnr's fetch moves the local tracking ref. The
    // message is required even here: git consumes it only when a merge commit
    // is created, and the guard cannot relax per outcome (AGENTS.md §1.2).
    let bare = git_stdout(&dir, &["config", "--get", "remote.origin.url"]);
    advance_remote_one_commit(Path::new(&bare), "merge-upstream-ff");
    repository.fetch().expect("fetch");
    let upstream_tip = git_stdout(&dir, &["rev-parse", "@{upstream}"]);
    assert_ne!(upstream_tip, head, "the test needs the branch behind");

    let merged = repository
        .integrate_upstream("integrate the fetched work", &head)
        .expect("integrate succeeds");

    // A fast-forward lands exactly on the upstream tip and creates no merge
    // commit: the new HEAD *is* the fetched commit, message and all.
    assert_eq!(merged, upstream_tip);
    assert_eq!(
        git_stdout(&dir, &["log", "-1", "--format=%s", &merged]),
        "advance remote",
        "a fast-forward consumes no human message"
    );
}

#[test]
fn integrating_a_diverged_upstream_creates_a_merge_commit_with_the_human_message() {
    let dir = setup_repo_with_upstream("merge-upstream-diverged");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("push");

    // Both sides advance.
    let bare = git_stdout(&dir, &["config", "--get", "remote.origin.url"]);
    advance_remote_one_commit(Path::new(&bare), "merge-upstream-diverged");
    fs::write(dir.join("local.txt"), "local\n").expect("write");
    git(&dir, &["add", "local.txt"]);
    git(&dir, &["commit", "-m", "local work"]);
    let local_head = repository.status().expect("status").head.expect("head");
    repository.fetch().expect("fetch");

    let merged = repository
        .integrate_upstream("Take the fetched upstream after review", &local_head)
        .expect("integrate succeeds");

    // A real merge commit: two parents, the human's subject, and containment
    // of the upstream tip — verified from the repository, not from the exit
    // status.
    assert_eq!(
        git_stdout(&dir, &["log", "-1", "--format=%s", &merged]),
        "Take the fetched upstream after review",
        "the merge commit must carry the human's message, not a generated one"
    );
    let parents = git_stdout(&dir, &["log", "-1", "--format=%P", &merged]);
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "a diverged integration has two parents"
    );
    git(
        &dir,
        &["merge-base", "--is-ancestor", "@{upstream}", &merged],
    );
}

#[test]
fn integrating_when_already_up_to_date_is_a_verified_no_op() {
    let dir = setup_repo_with_upstream("merge-upstream-noop");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("push");

    // `git merge` itself answers "Already up to date." with a success exit;
    // the claimed end state ("the branch contains the upstream tip") is
    // verified against a fresh ahead/behind read.
    let merged = repository
        .integrate_upstream("a message nothing consumes", &head)
        .expect("an up-to-date branch already holds the claimed end state");
    assert_eq!(merged, head, "nothing moved: HEAD is unchanged");
    assert_eq!(
        git_stdout(&dir, &["log", "-1", "--format=%s"]),
        "first work",
        "no merge commit was created"
    );
}

#[test]
fn integrating_without_any_upstream_configured_is_refused() {
    let dir = setup_repo("merge-upstream-none");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");

    let error = repository
        .integrate_upstream("merge", &head)
        .expect_err("must refuse");
    assert!(
        matches!(error, RepositoryError::NoUpstream { .. }),
        "unexpected variant {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryNoUpstream);
}

#[test]
fn integrating_an_upstream_that_was_never_fetched_is_refused() {
    // `branch.main.remote`/`branch.main.merge` exist but `@{upstream}`
    // resolves to nothing because nothing ever fetched (or pushed). There is
    // no local commit to integrate and mjolnr refuses rather than guessing;
    // the remedy is the human's own fetch.
    let dir = setup_repo_with_upstream("merge-upstream-unfetched");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");

    let error = repository
        .integrate_upstream("merge", &head)
        .expect_err("must refuse");
    assert!(
        matches!(error, RepositoryError::NoUpstream { .. }),
        "unexpected variant {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryNoUpstream);
    assert_eq!(repository.status().expect("status").head, Some(head));
}

#[test]
fn integrating_upstream_against_a_stale_head_fails_closed() {
    let dir = setup_repo_with_upstream("merge-upstream-stale");
    let repository = open(&dir);
    let stale = repository.status().expect("status").head.expect("head");
    // Push first so `@{upstream}` resolves; the NoUpstream refusal is checked
    // upstream of the staleness guard, and this test exercises the staleness
    // guard.
    repository.push(&stale).expect("push");

    // The branch advances after the human read the preview.
    fs::write(dir.join("moved.txt"), "moved\n").expect("write");
    git(&dir, &["add", "moved.txt"]);
    git(&dir, &["commit", "-m", "moved on"]);
    let current = repository.status().expect("status").head.expect("head");

    let error = repository
        .integrate_upstream("merge", &stale)
        .expect_err("must refuse a stale head");
    assert!(matches!(error, RepositoryError::StaleIndex { .. }));
    assert_eq!(error.reason_code(), ReasonCode::WorkspaceStaleRevision);
    assert_eq!(
        repository.status().expect("status").head.as_deref(),
        Some(current.as_str()),
        "a refused merge leaves HEAD where it was"
    );
}

#[test]
fn integrating_upstream_into_a_dirty_tree_is_refused_rather_than_stashing() {
    let dir = setup_repo_with_upstream("merge-upstream-dirty");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");
    // Push so `@{upstream}` resolves; the upstream refusal is earlier in the
    // guard order and is covered by its own tests.
    repository.push(&head).expect("push");

    // Uncommitted work an automatic stash would have swallowed.
    fs::write(dir.join("README.md"), "edited but not committed\n").expect("write");

    let error = repository
        .integrate_upstream("merge", &head)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::DirtyTree));
    assert_eq!(error.reason_code(), ReasonCode::WorkspaceDirty);
    assert_eq!(
        fs::read_to_string(dir.join("README.md")).expect("read"),
        "edited but not committed\n",
        "the human's uncommitted work must survive the refusal"
    );
}

#[test]
fn integrating_upstream_onto_a_detached_head_is_refused() {
    let dir = setup_repo_with_upstream("merge-upstream-detached");
    let repository = open(&dir);
    let head = repository.status().expect("status").head.expect("head");
    git(&dir, &["checkout", "--detach", &head]);

    let error = open(&dir)
        .integrate_upstream("merge", &head)
        .expect_err("must refuse");
    assert!(matches!(error, RepositoryError::DetachedHead));
    assert_eq!(error.reason_code(), ReasonCode::RepositoryDetachedHead);
}

#[test]
fn a_conflicting_upstream_merge_is_reported_and_never_resolved_for_the_human() {
    let dir = setup_repo_with_upstream("merge-upstream-conflict");
    let repository = open(&dir);
    fs::write(dir.join("work.txt"), "first\n").expect("write");
    git(&dir, &["add", "work.txt"]);
    git(&dir, &["commit", "-m", "first work"]);
    let head = repository.status().expect("status").head.expect("head");
    repository.push(&head).expect("push");

    // The remote takes `shared.txt` one way from a second clone.
    let bare = git_stdout(&dir, &["config", "--get", "remote.origin.url"]);
    let clone = std::env::temp_dir().join("mjolnr-d5-merge-upstream-conflict-clone");
    let _ = fs::remove_dir_all(&clone);
    git(
        &std::env::temp_dir(),
        &["clone", bare.as_str(), clone.to_str().expect("clone path")],
    );
    git(&clone, &["config", "user.email", "test@mjolnr.invalid"]);
    git(&clone, &["config", "user.name", "mjolnr Test"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);
    fs::write(clone.join("shared.txt"), "remote version\n").expect("write");
    git(&clone, &["add", "shared.txt"]);
    git(&clone, &["commit", "-m", "remote takes shared"]);
    git(&clone, &["push"]);

    // And the local takes it the other way; fetch, then merge must conflict.
    fs::write(dir.join("shared.txt"), "local version\n").expect("write");
    git(&dir, &["add", "shared.txt"]);
    git(&dir, &["commit", "-m", "local takes shared"]);
    let local_head = repository.status().expect("status").head.expect("head");
    repository.fetch().expect("fetch");

    let error = repository
        .integrate_upstream("take the fetched upstream", &local_head)
        .expect_err("must report the conflict");
    assert!(
        matches!(error, RepositoryError::Conflict { .. }),
        "unexpected variant {error:?}"
    );
    assert_eq!(error.reason_code(), ReasonCode::RepositoryConflict);

    // The tree is left mid-merge for the human, and a second attempt sees the
    // pre-existing conflict and refuses before touching anything.
    let again = repository
        .integrate_upstream("take the fetched upstream", &local_head)
        .expect_err("must refuse again");
    assert!(matches!(again, RepositoryError::Conflict { .. }));
    assert!(
        git_stdout(&dir, &["diff", "--name-only", "--diff-filter=U"]).contains("shared.txt"),
        "the conflicted path remains for the human to resolve"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create `branch` from HEAD, commit `file` on it, and return to `main` so the
/// caller is positioned to merge.
fn make_divergent_branch(
    dir: &Path,
    repository: &Repository,
    branch: &str,
    file: &str,
    contents: &str,
) {
    let head = repository.status().expect("status").head.expect("head");
    repository.create_branch(branch, &head).expect("create");
    git(dir, &["checkout", branch]);
    fs::write(dir.join(file), contents).expect("write");
    git(dir, &["add", file]);
    git(dir, &["commit", "-m", "child work"]);
    git(dir, &["checkout", "main"]);
}

fn write_failing_hook(dir: &Path, hook: &str) {
    let hooks = dir.join(".git").join("hooks");
    fs::create_dir_all(&hooks).expect("hooks dir");
    let path = hooks.join(hook);
    fs::write(&path, "#!/bin/sh\necho 'refused by policy' >&2\nexit 1\n").expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// A repository with a bare upstream remote named `origin` and the current
/// branch wired to it via `branch.main.remote` / `branch.main.merge`. The
/// remote starts empty so a first push creates `refs/heads/main` on it.
fn setup_repo_with_upstream(name: &str) -> PathBuf {
    let dir = setup_repo(name);
    let bare = std::env::temp_dir().join(format!("mjolnr-d5-{name}-remote"));
    let _ = fs::remove_dir_all(&bare);
    git(
        &dir,
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare.to_str().expect("bare path"),
        ],
    );
    git(
        &dir,
        &["remote", "add", "origin", bare.to_str().expect("bare path")],
    );
    git(&dir, &["config", "branch.main.remote", "origin"]);
    git(&dir, &["config", "branch.main.merge", "refs/heads/main"]);
    dir
}

/// Advance the bare remote by one commit from a throwaway clone, so a fetch
/// in `dir` has something new to observe on the upstream.
fn advance_remote_one_commit(bare: &Path, suffix: &str) {
    let clone = std::env::temp_dir().join(format!("mjolnr-d5-advance-{suffix}"));
    let _ = fs::remove_dir_all(&clone);
    git(
        &std::env::temp_dir(),
        &[
            "clone",
            bare.to_str().expect("bare path"),
            clone.to_str().expect("clone path"),
        ],
    );
    git(&clone, &["config", "user.email", "test@mjolnr.invalid"]);
    git(&clone, &["config", "user.name", "mjolnr Test"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);
    fs::write(clone.join("remote-work.txt"), "advanced\n").expect("write");
    git(&clone, &["add", "remote-work.txt"]);
    git(&clone, &["commit", "-m", "advance remote"]);
    git(&clone, &["push"]);
}

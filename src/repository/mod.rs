//! Governed local source control (Phase D5).
//!
//! One responsibility: perform bounded, verified git operations against one
//! repository root and report exactly what happened. It owns no policy, no
//! approval, and no persistence — the runtime routes intent here and records
//! the outcome.
//!
//! Two properties are load-bearing and easy to lose in a refactor:
//!
//! 1. **Every argument is argv, never shell text.** See [`git::run`].
//! 2. **Every claimed effect is re-read from the repository afterwards.** A
//!    successful exit status is not evidence that HEAD moved; `git rev-parse`
//!    is (AGENTS.md §1.3). Where the two disagree the result is
//!    [`RepositoryError::UncertainEffect`], never success and never a clean
//!    failure.

mod diff;
mod error;
mod git;
mod status;

use std::path::{Path, PathBuf};

use crate::core::change_capture::ChangeCapture;
use crate::core::repository::{
    MAX_HISTORY_ENTRIES, RefreshTrigger, RepositoryHistory, RepositoryHistoryEntry,
    RepositoryProjection, UpstreamPosition,
};

pub use error::RepositoryError;
pub use status::{IndexProjection, RepositoryStatus, WorktreeProjection};

/// A governed handle on one repository working directory.
///
/// Cheap and stateless beyond its root, so the runtime constructs one per
/// request rather than holding a long-lived handle whose root could drift from
/// the open project.
#[derive(Debug, Clone)]
pub struct Repository {
    work_dir: PathBuf,
}

impl Repository {
    /// Open a repository root, failing closed on anything that could not be
    /// one.
    ///
    /// The constructor validates rather than deferring to the first git call:
    /// a relative or missing root produces a confusing `git` error deep inside
    /// an operation, and by then a partial effect is already possible.
    pub fn open(work_dir: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        let work_dir = work_dir.into();
        let display = work_dir.display().to_string();
        if !work_dir.is_absolute() {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "a repository root must be an absolute path".to_owned(),
            });
        }
        if !work_dir.is_dir() {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "no such directory".to_owned(),
            });
        }
        let repository = Self { work_dir };
        let inside = git::run(
            &repository.work_dir,
            "rev-parse",
            &["rev-parse", "--is-inside-work-tree"],
        )?;
        if !inside.success || inside.stdout.trim() != "true" {
            return Err(RepositoryError::NotARepository {
                path: repository.work_dir.display().to_string(),
            });
        }
        Ok(repository)
    }

    /// What git says about the repository now.
    ///
    /// Returns the repository module's own type, not the client DTO: the trust
    /// label and the remote-sync vocabulary belong to the bridge, and a module
    /// that performs git side effects must not also grade how they are trusted.
    pub fn status(&self) -> Result<RepositoryStatus, RepositoryError> {
        let branch = self.current_branch()?;
        let head = self.head_revision()?;
        let porcelain = git::run_checked(&self.work_dir, "status", &["status", "--porcelain"])?;
        let (_, worktree) = status::parse_porcelain(&porcelain);
        let dirty_lines = porcelain.lines().count();

        Ok(RepositoryStatus {
            branch,
            head,
            dirty_count: u32::try_from(dirty_lines).unwrap_or(u32::MAX),
            // A saturated count and a truncated projection are both "we did
            // not report all of it", and the flag must say so either way.
            dirty_count_truncated: worktree.truncated || u32::try_from(dirty_lines).is_err(),
        })
    }

    /// Verify the exact branch tip a remote submission was approved for.
    ///
    /// This re-reads Git at the effect boundary instead of trusting the
    /// repository projection, which may have gone stale between rendering and
    /// submission.
    pub fn verify_head_and_branch(
        &self,
        expected_head: &str,
        expected_branch: &str,
    ) -> Result<(), RepositoryError> {
        let status = self.status()?;
        if status.branch.as_deref() != Some(expected_branch) {
            return Err(RepositoryError::StaleHead {
                expected: expected_branch.to_owned(),
                found: status.branch.unwrap_or_else(|| "no branch".to_owned()),
            });
        }
        if status.head.as_deref() != Some(expected_head) {
            return Err(RepositoryError::StaleHead {
                expected: expected_head.to_owned(),
                found: status.head.unwrap_or_else(|| "no HEAD".to_owned()),
            });
        }
        Ok(())
    }

    /// Everything a client renders, read as one moment (Phase D5 producer).
    ///
    /// One function rather than the runtime calling [`status`](Self::status),
    /// [`projections`](Self::projections), and
    /// [`index_revision`](Self::index_revision) in turn: those would be three
    /// separate `git status` runs describing three different instants, and a
    /// projection stitched from three moments is a picture of a repository that
    /// never existed. Here `git status --porcelain` runs exactly once and
    /// everything else is derived from that output or from a revision read.
    ///
    /// **`index_revision` is `None` rather than an error when git will not
    /// answer.** The usual cause is an unmerged index, and a conflicted
    /// repository is precisely when a human most needs to see status; failing
    /// the whole projection would black out the surface at the worst moment.
    /// A `None` here simply means no commit can be pre-armed with an expected
    /// revision — the commit path re-reads and compares regardless, so this
    /// value is advisory and a stale one becomes a refusal, never a wrong
    /// commit.
    ///
    /// Note that computing it runs `git write-tree`, which writes tree objects
    /// into the object database. They are unreferenced and collectable, and the
    /// commit path writes the same ones, but it is a write on a read path and
    /// is called out rather than left for someone to discover.
    pub fn project(
        &self,
        captured_after: RefreshTrigger,
        capture_sequence: u32,
    ) -> Result<RepositoryProjection, RepositoryError> {
        let branch = self.current_branch()?;
        let head = self.head_revision()?;
        let porcelain = git::run_checked(&self.work_dir, "status", &["status", "--porcelain"])?;
        let (index, worktree) = status::parse_porcelain(&porcelain);
        let dirty_lines = porcelain.lines().count();

        Ok(RepositoryProjection {
            branch,
            head,
            index_revision: self.index_revision().ok(),
            dirty_count: u32::try_from(dirty_lines).unwrap_or(u32::MAX),
            dirty_count_truncated: worktree.truncated
                || index.truncated
                || u32::try_from(dirty_lines).is_err(),
            staged_files: index.staged_files,
            modified_files: worktree.modified_files,
            untracked_files: worktree.untracked_files,
            unmerged_files: worktree.unmerged_files,
            rebase_in_progress: self.rebase_in_progress(),
            paths_truncated: index.truncated || worktree.truncated,
            upstream: self.upstream_position(),
            captured_after,
            capture_sequence,
        })
    }

    /// Read a bounded newest-first history. The extra row is requested solely
    /// to prove whether the answer was capped; it is never sent to the client.
    pub fn history(&self, limit: u32) -> Result<RepositoryHistory, RepositoryError> {
        if !(1..=MAX_HISTORY_ENTRIES).contains(&limit) {
            return Err(RepositoryError::CommandFailed {
                operation: "history",
                detail: format!("history limit must be between 1 and {MAX_HISTORY_ENTRIES}"),
            });
        }
        let requested = limit.saturating_add(1);
        let count = format!("-{requested}");
        let output = git::run_raw(
            &self.work_dir,
            "history",
            &[
                "log",
                "--no-color",
                "--decorate=no",
                &count,
                "--pretty=format:%H%x00%an%x00%aI%x00%s%x00",
                "--",
            ],
        )?;
        if !output.success {
            return Err(RepositoryError::CommandFailed {
                operation: "history",
                detail: output.stderr,
            });
        }
        if output.truncated {
            return Err(RepositoryError::OutputTruncated {
                operation: "history",
            });
        }

        let text =
            std::str::from_utf8(&output.stdout).map_err(|_| RepositoryError::CommandFailed {
                operation: "history",
                detail: "git returned non-UTF-8 commit metadata".to_owned(),
            })?;
        let fields: Vec<&str> = text.split('\0').filter(|field| !field.is_empty()).collect();
        if !fields.len().is_multiple_of(4) {
            return Err(RepositoryError::CommandFailed {
                operation: "history",
                detail: "git returned incomplete commit metadata".to_owned(),
            });
        }

        let mut entries = Vec::new();
        for chunk in fields.chunks_exact(4) {
            let [revision, author, authored_at, subject] = chunk else {
                return Err(RepositoryError::CommandFailed {
                    operation: "history",
                    detail: "git returned incomplete commit metadata".to_owned(),
                });
            };
            entries.push(RepositoryHistoryEntry {
                revision: bounded_history_text(revision, 128),
                author: bounded_history_text(author, 256),
                authored_at: bounded_history_text(authored_at, 64),
                subject: bounded_history_text(subject, 512),
            });
        }
        let has_more = entries.len() > limit as usize;
        entries.truncate(limit as usize);
        Ok(RepositoryHistory { entries, has_more })
    }

    /// Where the branch stands against its remote-tracking ref (ADR 0008).
    ///
    /// **Performs no network I/O**, and that property is the whole design.
    /// `rev-list --left-right --count HEAD...@{upstream}` walks two commits that
    /// are both already in the object database; the ref it compares against was
    /// written by whatever last fetched or pushed. So the counts are exact about
    /// a possibly-old ref, which is a different and honest claim from being
    /// current — see [`UpstreamPosition`].
    ///
    /// `None` on any failure, and the failures are ordinary: no upstream
    /// configured, a detached HEAD, an unborn branch. None of those is a fault,
    /// and none should black out a status surface, so they collapse to "there is
    /// no upstream to compare against" rather than to an error.
    fn upstream_position(&self) -> Option<UpstreamPosition> {
        let output = git::run(
            &self.work_dir,
            "rev-list",
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .ok()?;
        if !output.success {
            return None;
        }
        // `--left-right` prints the left count first: commits reachable from
        // HEAD but not the upstream. That is "ahead". Reversing these is the
        // easiest possible mistake here and would tell a user to pull when they
        // need to push.
        let mut counts = output.stdout.split_whitespace();
        let ahead = counts.next()?.parse().ok()?;
        let behind = counts.next()?.parse().ok()?;

        Some(UpstreamPosition {
            ahead,
            behind,
            ref_updated_at: self.upstream_ref_updated_at(),
        })
    }

    /// When the remote-tracking ref last moved, from git's reflog.
    ///
    /// Best effort by design. `core.logAllRefUpdates` can be off and a fresh
    /// clone has no entry, so `None` is a normal answer — the qualifier a
    /// surface renders does not depend on this value existing, only the
    /// precision of it does.
    fn upstream_ref_updated_at(&self) -> Option<String> {
        let output = git::run(
            &self.work_dir,
            "reflog",
            &[
                "reflog",
                "show",
                "--date=iso-strict",
                "--format=%gd",
                "-1",
                "@{upstream}",
            ],
        )
        .ok()?;
        if !output.success {
            return None;
        }
        // `%gd` renders as `refs/remotes/origin/main@{2026-07-30T18:34:50+07:00}`.
        // Only the bracketed instant is wanted; the ref name is already known.
        let line = output.stdout.lines().next()?;
        let (_, rest) = line.split_once("@{")?;
        let (instant, _) = rest.rsplit_once('}')?;
        Some(instant.to_owned())
    }

    /// The configured push target (remote, upstream branch) for the current
    /// branch, read from git config (`branch.<b>.remote` + `branch.<b>.merge`).
    ///
    /// Resolved from config rather than the remote-tracking ref so a first
    /// push to a branch whose upstream is configured but whose remote ref does
    /// not yet exist still resolves. The destination is git's own
    /// configuration, never client text: `Push` carries only `expected_head`
    /// because the remote is not the model's to choose.
    fn upstream_target(&self) -> Option<(String, String)> {
        let branch = self.current_branch().ok().flatten()?;
        let remote = self.config_value(&format!("branch.{branch}.remote"))?;
        let merge = self.config_value(&format!("branch.{branch}.merge"))?;
        let upstream_branch = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
        if remote.is_empty() || upstream_branch.is_empty() {
            return None;
        }
        Some((remote, upstream_branch.to_owned()))
    }

    /// A single `git config --get` value, or `None` when unset or git declines.
    fn config_value(&self, key: &str) -> Option<String> {
        let output = git::run(&self.work_dir, "config", &["config", "--get", key]).ok()?;
        if !output.success {
            return None;
        }
        let value = output.stdout.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    /// The current revision of the remote-tracking ref, for post-push
    /// verification. `None` when `@{upstream}` does not resolve: no upstream,
    /// or a first push whose remote ref does not yet exist.
    fn upstream_ref_revision(&self) -> Option<String> {
        let output = git::run(&self.work_dir, "rev-parse", &["rev-parse", "@{upstream}"]).ok()?;
        if !output.success {
            return None;
        }
        let rev = output.stdout.trim().to_owned();
        (!rev.is_empty() && rev != "@{upstream}").then_some(rev)
    }

    /// The exact diffs behind a projection (Phase D3 producer).
    ///
    /// Takes the projection rather than re-deriving `head`, `index_revision`,
    /// and the untracked list, so a capture cannot describe a different HEAD
    /// than the status rendered beside it. It inherits the projection's
    /// `capture_sequence` for the same reason: two numbers would let a client
    /// pair a status with a change set from another moment and never know.
    ///
    /// A second `git` read after `project`, necessarily — no single git command
    /// answers both — so the pair is two adjacent reads, not one atomic one.
    /// That is exactly why neither claims to be current (AGENTS.md §1.3).
    pub fn capture_changes(
        &self,
        projection: &RepositoryProjection,
    ) -> Result<ChangeCapture, RepositoryError> {
        diff::capture(
            &self.work_dir,
            projection.head.as_deref(),
            projection.index_revision.as_deref(),
            &projection.untracked_files,
            projection.capture_sequence,
        )
    }

    /// The index and worktree projections a review surface renders.
    pub fn projections(&self) -> Result<(IndexProjection, WorktreeProjection), RepositoryError> {
        let porcelain = git::run_checked(&self.work_dir, "status", &["status", "--porcelain"])?;
        Ok(status::parse_porcelain(&porcelain))
    }

    /// A content hash of exactly what the index would commit.
    ///
    /// `git write-tree` is the canonical answer: it is a pure function of the
    /// index, and the tree object it writes is the same one a commit would
    /// write. It also fails on an unmerged index, which is why a conflict is
    /// detected here before any commit is attempted.
    pub fn index_revision(&self) -> Result<String, RepositoryError> {
        let output = git::run(&self.work_dir, "write-tree", &["write-tree"])?;
        if output.success {
            return Ok(output.stdout.trim().to_owned());
        }
        if let Some(conflict) = self.unmerged_conflict()? {
            return Err(conflict);
        }
        Err(RepositoryError::CommandFailed {
            operation: "write-tree",
            detail: output.stderr,
        })
    }

    /// Which paths directly under `directory` git considers ignored (Phase D7).
    ///
    /// One invocation, no candidate list: `ls-files --others --ignored` reports
    /// what is ignored under a pathspec directly, so the file producer does not
    /// have to enumerate first and ask second. `--directory` collapses a wholly
    /// ignored directory to the directory itself, which is what a file explorer
    /// needs — `target/` marked once rather than every object file under it
    /// listed and marked individually.
    ///
    /// The answer is deliberately shallow: only entries one level below
    /// `directory` are returned, because that is exactly one page of one
    /// listing, and a set covering the whole subtree would grow without bound
    /// on a repository whose build output git ignores.
    ///
    /// This lives here rather than in `workspace_files` because it is a git
    /// question, and a module that both walked the filesystem and shelled out
    /// to git would have two reasons to change (AGENTS.md §2.3). The runtime
    /// composes the two.
    pub fn ignored_under(
        &self,
        directory: &str,
    ) -> Result<std::collections::BTreeSet<String>, RepositoryError> {
        let pathspec = if directory.is_empty() {
            ".".to_owned()
        } else {
            directory.trim_matches('/').to_owned()
        };
        let output = git::run(
            &self.work_dir,
            "ls-files",
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
                "--",
                &pathspec,
            ],
        )?;
        if !output.success {
            return Err(RepositoryError::CommandFailed {
                operation: "ls-files",
                detail: output.stderr,
            });
        }

        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{}/", directory.trim_matches('/'))
        };
        Ok(output
            .stdout
            .split('\0')
            .filter_map(|entry| {
                // git prints directories with a trailing slash and paths
                // relative to the repository root, which is what the file
                // producer compares against — but only the immediate children
                // belong to this page.
                let entry = entry.trim_end_matches('/');
                let rest = entry.strip_prefix(prefix.as_str())?;
                (!rest.is_empty() && !rest.contains('/')).then(|| entry.to_owned())
            })
            .collect())
    }

    pub fn stage_paths(&self, paths: &[String]) -> Result<(), RepositoryError> {
        self.run_pathspec("add", &["add", "--"], paths)
    }

    pub fn unstage_paths(&self, paths: &[String]) -> Result<(), RepositoryError> {
        self.run_pathspec("restore", &["restore", "--staged", "--"], paths)
    }

    /// Hunk-level staging is on the wire but not implemented. It needs the
    /// Phase D3 diff identity to name a hunk stably; without that, an index
    /// applied by ordinal is an unsound write. Refused, not approximated.
    pub fn stage_hunks(&self, _path: &str, _hunk_indices: &[usize]) -> Result<(), RepositoryError> {
        Err(RepositoryError::CapabilityUnavailable {
            capability: "stageHunks",
        })
    }

    /// Create a branch. Never checks it out, and never moves an existing one:
    /// `git branch` without `--force` refuses, which is the behaviour smed
    /// wants (no silent branch reassignment).
    pub fn create_branch(&self, name: &str, base_revision: &str) -> Result<(), RepositoryError> {
        let output = git::run(
            &self.work_dir,
            "branch",
            &["branch", "--", name, base_revision],
        )?;
        if output.success {
            return Ok(());
        }
        Err(RepositoryError::CommandFailed {
            operation: "branch",
            detail: output.stderr,
        })
    }

    /// Commit the index, verifying the effect against the repository afterwards.
    ///
    /// `expected_index_revision` is required, not optional: it is the value the
    /// human saw when they approved this exact commit. Making it skippable
    /// would make the fail-closed guard opt-in, which is not a guard.
    pub fn commit(
        &self,
        message: &str,
        expected_index_revision: &str,
    ) -> Result<String, RepositoryError> {
        if let Some(conflict) = self.unmerged_conflict()? {
            return Err(conflict);
        }
        let found = self.index_revision()?;
        if found != expected_index_revision {
            return Err(RepositoryError::StaleIndex {
                expected: expected_index_revision.to_owned(),
                found,
            });
        }
        if self.projections()?.0.staged_files.is_empty() {
            return Err(RepositoryError::NothingStaged);
        }

        let before = self.head_revision()?;
        let output = git::run(&self.work_dir, "commit", &["commit", "-m", message])?;
        self.verify_head_moved("commit", before.as_deref(), &output)
    }

    /// Merge an explicitly selected child branch with a human-supplied message.
    ///
    /// `--no-ff` is deliberate: an integration is a recorded decision, so it
    /// gets a commit even where a fast-forward would be possible.
    pub fn integrate_child_branch(
        &self,
        name: &str,
        message: &str,
        expected_head: &str,
    ) -> Result<String, RepositoryError> {
        if let Some(conflict) = self.unmerged_conflict()? {
            return Err(conflict);
        }
        // A merge onto a detached HEAD produces a commit no branch references —
        // work that is trivially lost. Refused rather than performed.
        if self.current_branch()?.is_none() {
            return Err(RepositoryError::DetachedHead);
        }
        // A merge into a dirty tree can overwrite uncommitted work, and smed
        // never stashes on the human's behalf (Phase D5 acceptance: no
        // automatic stash, reset, or clean).
        let (index, worktree) = self.projections()?;
        if !index.staged_files.is_empty() || !worktree.modified_files.is_empty() {
            return Err(RepositoryError::DirtyTree);
        }
        let before = self.head_revision()?;
        let Some(current) = before.clone() else {
            return Err(RepositoryError::CommandFailed {
                operation: "merge",
                detail: "the repository has no commits to merge into".to_owned(),
            });
        };
        if current != expected_head {
            return Err(RepositoryError::StaleIndex {
                expected: expected_head.to_owned(),
                found: current,
            });
        }

        let output = git::run(
            &self.work_dir,
            "merge",
            &["merge", "--no-ff", "-m", message, "--", name],
        )?;
        // A conflicted merge leaves the tree mid-merge. That is a refusal to
        // resolve on the human's behalf, not an uncertain effect: the state is
        // knowable, and `MERGE_HEAD` plus `--diff-filter=U` say so exactly.
        if !output.success
            && self.merge_in_progress()
            && let Some(conflict) = self.unmerged_conflict()?
        {
            return Err(conflict);
        }
        self.verify_head_moved("merge", before.as_deref(), &output)
    }

    /// Merge the branch's configured upstream into it — the merge half of
    /// "pull": pull is fetch plus merge, two evidenced acts, and
    /// [`fetch`](Self::fetch) is the other one.
    ///
    /// Refusals and state guards are
    /// [`integrate_child_branch`](Self::integrate_child_branch)'s: a
    /// conflicted, dirty, detached, or moved repository refuses before any
    /// mutation, and a merge that lands in conflict is reported, never
    /// resolved on the human's behalf. A branch with no configured upstream —
    /// or one whose upstream ref has never been fetched, so `@{upstream}`
    /// resolves to nothing — refuses as [`RepositoryError::NoUpstream`]; the
    /// remedy in the second case is the human's own `fetch`.
    ///
    /// There is no `--no-ff` here: integrating upstream mirrors `git pull`.
    /// A branch that is simply behind fast-forwards, and the message is then
    /// unused, exactly as git does; a genuinely diverged branch produces a
    /// merge commit carrying the human's message. A branch that already
    /// contains the upstream tip is a verified no-op, as `git merge`'s own
    /// "Already up to date." success is.
    ///
    /// The verification is a fresh ahead/behind read, not the exit status:
    /// "the branch contains the upstream tip" is claimed only when `rev-list`
    /// proves it. Either disagreement is [`RepositoryError::UncertainEffect`]
    /// and never auto-retried (AGENTS.md §1.4).
    pub fn integrate_upstream(
        &self,
        message: &str,
        expected_head: &str,
    ) -> Result<String, RepositoryError> {
        if let Some(conflict) = self.unmerged_conflict()? {
            return Err(conflict);
        }
        let Some(branch) = self.current_branch()? else {
            return Err(RepositoryError::DetachedHead);
        };
        // Configured in git (`upstream_target`) and fetched at least once
        // (`upstream_ref_revision`): both are required for `@{upstream}` to
        // name a commit. Merging a ref that does not resolve is not a failure
        // git reports clearly, so the refusal happens here instead.
        if self.upstream_target().is_none() || self.upstream_ref_revision().is_none() {
            return Err(RepositoryError::NoUpstream { branch });
        }
        // A merge into a dirty tree can overwrite uncommitted work, and smed
        // never stashes on the human's behalf (Phase D5 acceptance: no
        // automatic stash, reset, or clean).
        let (index, worktree) = self.projections()?;
        if !index.staged_files.is_empty() || !worktree.modified_files.is_empty() {
            return Err(RepositoryError::DirtyTree);
        }
        let before = self.head_revision()?;
        let Some(current) = before.clone() else {
            return Err(RepositoryError::CommandFailed {
                operation: "merge",
                detail: "the repository has no commits to merge into".to_owned(),
            });
        };
        if current != expected_head {
            return Err(RepositoryError::StaleIndex {
                expected: expected_head.to_owned(),
                found: current,
            });
        }

        let output = git::run(
            &self.work_dir,
            "merge",
            &["merge", "-m", message, "--", "@{upstream}"],
        )?;
        // A conflicted merge leaves the tree mid-merge. Same rule as the
        // child-branch path: the state is knowable and is reported as a typed
        // conflict — smed never resolves it on the human's behalf.
        if !output.success
            && self.merge_in_progress()
            && let Some(conflict) = self.unmerged_conflict()?
        {
            return Err(conflict);
        }
        let contains_upstream = self
            .upstream_position()
            .is_some_and(|position| position.behind == 0);
        match (output.success, contains_upstream) {
            (true, true) => self
                .head_revision()?
                .ok_or_else(|| RepositoryError::UncertainEffect {
                    operation: "merge",
                    detail: "git reported success but HEAD cannot be read".to_owned(),
                }),
            (true, false) => Err(RepositoryError::UncertainEffect {
                operation: "merge",
                detail: "git reported success but the branch is still behind its upstream ref"
                    .to_owned(),
            }),
            // Behind says the merge landed yet git reported failure: neither a
            // success nor a clean failure, and never auto-retried (§1.4).
            (false, true) => Err(RepositoryError::UncertainEffect {
                operation: "merge",
                detail: format!(
                    "git merge failed yet the branch now contains the upstream tip; \
                     the merge may have landed despite the failure: {}",
                    output.stderr
                ),
            }),
            (false, false) => Err(self.classify_clean_failure("merge", &output.stderr)),
        }
    }

    /// Clone into a new absolute destination. The destination is never
    /// removed on failure: a partial clone is an uncertain effect for a human
    /// to inspect, not something smed may guess how to clean up.
    pub fn clone_project(
        source: &str,
        destination: impl Into<PathBuf>,
    ) -> Result<PathBuf, RepositoryError> {
        validate_git_value("clone source", source)?;
        let destination = destination.into();
        let display = destination.display().to_string();
        if !destination.is_absolute() {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "a clone destination must be an absolute path".to_owned(),
            });
        }
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "the clone destination already exists".to_owned(),
            });
        }
        let Some(parent) = destination.parent() else {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "the clone destination has no parent directory".to_owned(),
            });
        };
        if !parent.is_dir() {
            return Err(RepositoryError::InvalidRoot {
                path: display,
                detail: "the clone destination parent does not exist".to_owned(),
            });
        }
        let destination_text = destination.to_string_lossy().into_owned();
        let output = git::run(parent, "clone", &["clone", "--", source, &destination_text])?;
        if !output.success {
            if std::fs::symlink_metadata(&destination).is_ok() {
                return Err(RepositoryError::UncertainEffect {
                    operation: "clone",
                    detail: format!(
                        "git clone failed after creating the destination: {}",
                        output.stderr
                    ),
                });
            }
            return Err(RepositoryError::CommandFailed {
                operation: "clone",
                detail: output.stderr,
            });
        }
        Repository::open(&destination).map_err(|error| RepositoryError::UncertainEffect {
            operation: "clone",
            detail: format!(
                "git clone reported success but the destination could not be opened: {error}"
            ),
        })?;
        Ok(destination)
    }

    /// Rebase a clean branch onto a local ref. Conflicts are deliberately left
    /// in git's recovery state so the human can resolve or abort them.
    pub fn rebase(&self, onto: &str, expected_head: &str) -> Result<String, RepositoryError> {
        validate_git_value("rebase target", onto)?;
        if self.rebase_in_progress() {
            return Err(RepositoryError::Conflict {
                paths: "a rebase is already in progress".to_owned(),
            });
        }
        if let Some(conflict) = self.unmerged_conflict()? {
            return Err(conflict);
        }
        if self.current_branch()?.is_none() {
            return Err(RepositoryError::DetachedHead);
        }
        let (index, worktree) = self.projections()?;
        if !index.staged_files.is_empty()
            || !worktree.modified_files.is_empty()
            || !worktree.untracked_files.is_empty()
        {
            return Err(RepositoryError::DirtyTree);
        }
        let before = self
            .head_revision()?
            .ok_or_else(|| RepositoryError::CommandFailed {
                operation: "rebase",
                detail: "the repository has no commits to rebase".to_owned(),
            })?;
        if before != expected_head {
            return Err(RepositoryError::StaleHead {
                expected: expected_head.to_owned(),
                found: before,
            });
        }
        let target = format!("{onto}^{{commit}}");
        let target_output = git::run(
            &self.work_dir,
            "rev-parse",
            &["rev-parse", "--verify", "--quiet", &target],
        )?;
        if !target_output.success {
            return Err(RepositoryError::CommandFailed {
                operation: "rebase",
                detail: format!("rebase target {onto} does not resolve to a commit"),
            });
        }

        let output = git::run(&self.work_dir, "rebase", &["rebase", "--", onto])?;
        if output.success {
            return self
                .head_revision()?
                .ok_or_else(|| RepositoryError::UncertainEffect {
                    operation: "rebase",
                    detail: "git reported success but HEAD cannot be read".to_owned(),
                });
        }
        if self.rebase_in_progress() {
            if let Some(conflict) = self.unmerged_conflict()? {
                return Err(conflict);
            }
            return Err(RepositoryError::Conflict {
                paths: "a rebase is paused and requires human recovery".to_owned(),
            });
        }
        let after = self.head_revision()?;
        if after.as_deref() != Some(expected_head) {
            return Err(RepositoryError::UncertainEffect {
                operation: "rebase",
                detail: format!("git rebase failed after HEAD changed: {}", output.stderr),
            });
        }
        Err(self.classify_clean_failure("rebase", &output.stderr))
    }

    /// Abort only an explicitly observed in-progress rebase.
    pub fn abort_rebase(&self) -> Result<(), RepositoryError> {
        if !self.rebase_in_progress() {
            return Err(RepositoryError::CommandFailed {
                operation: "rebaseAbort",
                detail: "no rebase is in progress".to_owned(),
            });
        }
        let output = git::run(&self.work_dir, "rebaseAbort", &["rebase", "--abort"])?;
        if output.success && !self.rebase_in_progress() {
            return Ok(());
        }
        if self.rebase_in_progress() {
            return Err(RepositoryError::UncertainEffect {
                operation: "rebaseAbort",
                detail: format!(
                    "git rebase --abort did not clear the recovery state: {}",
                    output.stderr
                ),
            });
        }
        Err(RepositoryError::CommandFailed {
            operation: "rebaseAbort",
            detail: output.stderr,
        })
    }

    /// Fetch from the configured upstream remote.
    ///
    /// Inert by design: `git fetch` updates remote-tracking refs and never
    /// touches the working tree, so there is no `UncertainEffect` case. The
    /// outcome is success or a typed failure carrying git's verbatim stderr;
    /// the caller's refresh afterward surfaces the new `upstream_position`
    /// (ADR-0008). No arguments: `git fetch` resolves the configured upstream.
    pub fn fetch(&self) -> Result<(), RepositoryError> {
        let output = git::run(&self.work_dir, "fetch", &["fetch"])?;
        if output.success {
            return Ok(());
        }
        Err(RepositoryError::CommandFailed {
            operation: "fetch",
            detail: output.stderr,
        })
    }

    /// Push the current branch's `HEAD` to its configured upstream.
    ///
    /// The outcome is verified against the **remote-tracking ref**, not the
    /// exit status: a push that dies mid-transfer leaves the local tree
    /// identical either way, and the tracking ref is the evidence of what the
    /// remote actually accepted (AGENTS.md §1.3). Four cases, three of which
    /// are not plain success.
    ///
    /// `expected_head` is the HEAD the human saw when they approved pushing
    /// this commit; a mismatch is refused before the network call. A branch
    /// behind its remote is refused before the network call rather than
    /// attempting a push the remote will reject.
    pub fn push(&self, expected_head: &str) -> Result<(), RepositoryError> {
        let Some(branch) = self.current_branch()? else {
            return Err(RepositoryError::DetachedHead);
        };
        let Some((remote, upstream_branch)) = self.upstream_target() else {
            return Err(RepositoryError::NoUpstream { branch });
        };
        let Some(found_head) = self.head_revision()? else {
            return Err(RepositoryError::CommandFailed {
                operation: "push",
                detail: "the repository has no commits to push".to_owned(),
            });
        };
        if found_head != expected_head {
            return Err(RepositoryError::StaleIndex {
                expected: expected_head.to_owned(),
                found: found_head,
            });
        }
        // Fail closed before the network: a push now would be rejected as
        // non-fast-forward. Where the ahead/behind cannot be computed (a first
        // push, no remote ref yet), the remote's own rejection is the backstop
        // in the post-effect verification below.
        if let Some(position) = self.upstream_position()
            && position.behind > 0
        {
            return Err(RepositoryError::DivergedFromRemote {
                ahead: position.ahead,
                behind: position.behind,
            });
        }

        let refspec = format!("HEAD:refs/heads/{upstream_branch}");
        let output = git::run(&self.work_dir, "push", &["push", "--", &remote, &refspec])?;

        // The evidence is the remote-tracking ref. After a successful push git
        // advances `@{upstream}` to the pushed commit; after a rejection it does
        // not. A push that died mid-transfer may have advanced it before the
        // failure — knowable, not certain, and never auto-retried (§1.4).
        let after = self.upstream_ref_revision();
        let landed = after.as_deref() == Some(expected_head);

        match (output.success, landed) {
            (true, true) => Ok(()),
            (true, false) => Err(RepositoryError::UncertainEffect {
                operation: "push",
                detail:
                    "git reported success but the remote-tracking ref did not advance to the pushed commit"
                        .to_owned(),
            }),
            (false, true) => Err(RepositoryError::UncertainEffect {
                operation: "push",
                detail: format!(
                    "git push failed but the remote-tracking ref advanced to {expected_head}; \
                     the push may have landed despite the failure: {}",
                    output.stderr
                ),
            }),
            (false, false) => Err(self.classify_clean_failure("push", &output.stderr)),
        }
    }

    /// The shared post-effect verification: an exit status is a claim, and
    /// HEAD is the evidence. Four cases, three of which are not plain success.
    fn verify_head_moved(
        &self,
        operation: &'static str,
        before: Option<&str>,
        output: &git::GitOutput,
    ) -> Result<String, RepositoryError> {
        let after = self.head_revision()?;
        let moved = after.as_deref() != before;

        match (output.success, moved) {
            (true, true) => after.ok_or_else(|| RepositoryError::UncertainEffect {
                operation,
                detail: "git reported success but HEAD cannot be read".to_owned(),
            }),
            (true, false) => Err(RepositoryError::UncertainEffect {
                operation,
                detail: "git reported success but HEAD did not move".to_owned(),
            }),
            // HEAD moved despite a failure: an object was written and something
            // after it failed. Neither success nor clean failure.
            (false, true) => Err(RepositoryError::UncertainEffect {
                operation,
                detail: format!(
                    "git failed after HEAD moved to {}: {}",
                    after.as_deref().unwrap_or("an unreadable revision"),
                    output.stderr
                ),
            }),
            (false, false) => Err(self.classify_clean_failure(operation, &output.stderr)),
        }
    }

    /// Classify a failure that provably had no effect.
    ///
    /// Signing is the one case git documents a stable phrase for. Hook refusal
    /// is attributed from filesystem evidence — the hook exists and is
    /// executable — rather than from prose, so a branch named `hook-cleanup`
    /// cannot masquerade as a hook failure. Anything else stays
    /// `CommandFailed` with git's text intact.
    fn classify_clean_failure(&self, operation: &'static str, stderr: &str) -> RepositoryError {
        if stderr.contains("gpg failed to sign") {
            return RepositoryError::SigningFailed {
                detail: stderr.to_owned(),
            };
        }
        if operation == "commit" {
            for hook in ["pre-commit", "commit-msg"] {
                if self.hook_is_executable(hook) {
                    return RepositoryError::HookRefused {
                        hook: if hook == "commit-msg" {
                            "commit-msg"
                        } else {
                            "pre-commit"
                        },
                        detail: stderr.to_owned(),
                    };
                }
            }
        }
        if operation == "push" && self.hook_is_executable("pre-push") {
            return RepositoryError::HookRefused {
                hook: "pre-push",
                detail: stderr.to_owned(),
            };
        }
        RepositoryError::CommandFailed {
            operation,
            detail: stderr.to_owned(),
        }
    }

    fn run_pathspec(
        &self,
        operation: &'static str,
        prefix: &[&str],
        paths: &[String],
    ) -> Result<(), RepositoryError> {
        let mut arguments: Vec<&str> = prefix.to_vec();
        arguments.extend(paths.iter().map(String::as_str));
        let output = git::run(&self.work_dir, operation, &arguments)?;
        if output.success {
            return Ok(());
        }
        Err(RepositoryError::CommandFailed {
            operation,
            detail: output.stderr,
        })
    }

    /// Ask git for unmerged paths rather than reading the word "conflict" out
    /// of prose. `--diff-filter=U` is the documented query for exactly this.
    fn unmerged_conflict(&self) -> Result<Option<RepositoryError>, RepositoryError> {
        let output = git::run(
            &self.work_dir,
            "diff",
            &["diff", "--name-only", "--diff-filter=U"],
        )?;
        if !output.success {
            return Ok(None);
        }
        let paths: Vec<&str> = output
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .collect();
        if paths.is_empty() {
            return Ok(None);
        }
        Ok(Some(RepositoryError::Conflict {
            paths: paths.join(", "),
        }))
    }

    /// `None` on an unborn branch — a repository with no commits yet. That is a
    /// legitimate state, distinct from an unreadable HEAD.
    fn head_revision(&self) -> Result<Option<String>, RepositoryError> {
        let output = git::run(&self.work_dir, "rev-parse", &["rev-parse", "HEAD"])?;
        if !output.success {
            return Ok(None);
        }
        let head = output.stdout.trim().to_owned();
        Ok((!head.is_empty()).then_some(head))
    }

    /// `None` when HEAD is detached. Callers that need a branch turn this into
    /// [`RepositoryError::DetachedHead`]; a read-only projection does not.
    fn current_branch(&self) -> Result<Option<String>, RepositoryError> {
        let output = git::run(
            &self.work_dir,
            "symbolic-ref",
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )?;
        if !output.success {
            return Ok(None);
        }
        let branch = output.stdout.trim().to_owned();
        Ok((!branch.is_empty()).then_some(branch))
    }

    fn merge_in_progress(&self) -> bool {
        self.git_dir()
            .is_some_and(|git_dir| git_dir.join("MERGE_HEAD").exists())
    }

    fn rebase_in_progress(&self) -> bool {
        self.git_dir().is_some_and(|git_dir| {
            git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
        })
    }

    fn hook_is_executable(&self, hook: &str) -> bool {
        let Ok(hooks_path) = git::run_checked(
            &self.work_dir,
            "rev-parse",
            &["rev-parse", "--git-path", "hooks"],
        ) else {
            return false;
        };
        let candidate = self.work_dir.join(hooks_path.trim()).join(hook);
        is_executable_file(&candidate)
    }

    fn git_dir(&self) -> Option<PathBuf> {
        let path = git::run_checked(
            &self.work_dir,
            "rev-parse",
            &["rev-parse", "--absolute-git-dir"],
        )
        .ok()?;
        let trimmed = path.trim();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    }
}

fn validate_git_value(label: &'static str, value: &str) -> Result<(), RepositoryError> {
    if value.trim().is_empty() {
        return Err(RepositoryError::CommandFailed {
            operation: "validate",
            detail: format!("{label} is required"),
        });
    }
    if value.chars().any(char::is_control) || value.starts_with('-') {
        return Err(RepositoryError::CommandFailed {
            operation: "validate",
            detail: format!("{label} contains an unsafe git argument"),
        });
    }
    Ok(())
}

fn bounded_history_text(value: &str, limit: usize) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if cleaned.len() <= limit {
        return cleaned;
    }
    let mut end = limit.saturating_sub(3);
    while end > 0 && !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = cleaned[..end].to_owned();
    truncated.push_str("...");
    truncated
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;

    #[test]
    fn a_relative_root_is_refused_before_any_process_starts() {
        let error = Repository::open("relative/path").expect_err("must refuse");
        assert!(matches!(error, RepositoryError::InvalidRoot { .. }));
        assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
    }

    #[test]
    fn a_missing_directory_is_refused_by_the_constructor() {
        let error = Repository::open("/smed-does-not-exist-9f3a2b").expect_err("must refuse");
        assert!(matches!(error, RepositoryError::InvalidRoot { .. }));
    }

    #[test]
    fn hunk_staging_reports_unavailable_rather_than_pretending() {
        // Constructed directly: the refusal is unconditional and must not
        // depend on a repository existing.
        let repository = Repository {
            work_dir: PathBuf::from("/"),
        };
        let error = repository
            .stage_hunks("a.rs", &[0])
            .expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            ReasonCode::WorkspaceCapabilityUnavailable
        );
    }
}

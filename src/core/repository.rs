//! Repository truth as the runtime holds it (Phase D5 producer).
//!
//! Three modules, three reasons to change: `core` defines these types,
//! `crate::repository` produces them by running git, and
//! `runtime::client_bridge` projects them onto the wire and applies the trust
//! class. The split is not decoration — two architecture guards enforce it.
//! `core` may not depend on `crate::repository` (AGENTS.md §2.1) and the bridge
//! may not either (`tests/architecture.rs`), so a type both the runtime
//! snapshot and the projection can name has to live here.
//!
//! Nothing in this module claims currency. See [`RepositoryProjection`].

use crate::core::error::ReasonCode;

/// Maximum number of commits a history query may return. History is a read
/// projection, not an invitation to stream an entire repository into a client.
pub const MAX_HISTORY_ENTRIES: u32 = 50;

/// What the runtime knows about the open project's repository.
///
/// Three states rather than an `Option`, because "no project is open" and
/// "the project is not a git repository" send a reader to different remedies,
/// and collapsing them into `None` would offer the wrong one half the time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RepositoryView {
    /// No project is open, so there is nothing to read.
    #[default]
    NoProject,
    /// A project is open but git could not answer — most often because the
    /// root is not a repository. Carries the refusal rather than an empty
    /// projection, which would render as a clean repository with no changes.
    Unavailable { code: ReasonCode, detail: String },
    /// What git said, and when it was asked.
    Projected(Box<RepositoryProjection>),
}

/// What `git` reported at the last refresh.
///
/// **There is no `fresh` flag and no `is_current` method, deliberately.**
/// Nothing watches the filesystem — refreshes happen on the explicit triggers
/// in [`RefreshTrigger`] — so between two triggers a user can commit in a
/// terminal and this projection will not know. smed can honestly say what git
/// said and when it asked; it cannot say the answer is still true, and a field
/// asserting that it is would be the exact false claim AGENTS.md §1.3 forbids.
///
/// A client renders [`captured_after`](Self::captured_after) and
/// [`capture_sequence`](Self::capture_sequence) as the freshness marker. The
/// modifying commands do not trust this projection at all: they carry their own
/// `expected_index_revision` / `expected_head` and fail closed in
/// `crate::repository` (§D5 acceptance).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepositoryProjection {
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The index revision this projection was read at, for a client to echo
    /// into a `Commit`. Advisory: the command re-reads and compares it, so a
    /// stale value here becomes a refusal, never a wrong commit.
    pub index_revision: Option<String>,
    pub dirty_count: u32,
    pub dirty_count_truncated: bool,
    pub staged_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
    /// Conflicted paths, kept separate from `modified_files` so a UI never
    /// offers to stage a conflict marker as an ordinary change.
    pub unmerged_files: Vec<String>,
    /// True when git has left the repository inside an unfinished rebase.
    /// This is separate from conflicts: a rebase can be paused on a clean
    /// stop, and a merge conflict is not proof that a rebase is active.
    pub rebase_in_progress: bool,
    /// True when any path list is bounded rather than complete.
    pub paths_truncated: bool,
    /// Where the branch stands against its remote-tracking ref, when one is
    /// configured (ADR 0008). `None` means no upstream — genuinely unknown,
    /// not "we did not look".
    pub upstream: Option<UpstreamPosition>,
    /// What caused this read.
    pub captured_after: RefreshTrigger,
    /// Monotonic per-runtime counter, incremented on every completed refresh.
    ///
    /// A client compares it against the last value it rendered to know the
    /// projection moved. It is not a git revision and orders nothing outside
    /// this process. `u32` to match the workspace DTO's wire-number contract,
    /// so it crosses to JavaScript without a lossy conversion.
    pub capture_sequence: u32,
}

/// One bounded, read-only commit history row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryHistoryEntry {
    pub revision: String,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
}

/// A bounded history answer. `has_more` is evidence that the limit, not the
/// repository, ended the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryHistory {
    pub entries: Vec<RepositoryHistoryEntry>,
    pub has_more: bool,
}

/// Where the local branch stands against its remote-tracking ref (ADR 0008).
///
/// **These counts are exact and they are not current.** They compare two commits
/// that are both present locally, so computing them touches no network — but the
/// ref they compare against was written by whatever last fetched or pushed, and
/// the remote may have moved since. That is not a defect to fix; it is the only
/// thing a read path can honestly say, because learning otherwise requires a
/// network call that rendering a panel has no business making.
///
/// [`ref_updated_at`](Self::ref_updated_at) is the moment the remote-tracking
/// ref last moved, from git's reflog. `None` when the reflog is unavailable —
/// `core.logAllRefUpdates` can be off, and a fresh clone has no entry. A
/// surface renders the qualifier either way; the timestamp is a refinement of
/// it, never the thing that makes it honest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UpstreamPosition {
    /// Commits on the local branch that the tracked ref does not have.
    pub ahead: u32,
    /// Commits on the tracked ref that the local branch does not have.
    pub behind: u32,
    /// When the remote-tracking ref last moved, if git recorded it.
    ///
    /// "Last fetch" is the usual cause but not the only one — git's reflog
    /// records `update by push` too. The honest phrasing is "when smed last
    /// saw the remote", which is what this is.
    pub ref_updated_at: Option<String>,
}

/// Why the runtime re-read the repository.
///
/// The complete set of triggers: there is no timer and no filesystem watcher.
/// A timer would put wall-clock into tests that must stay deterministic
/// (AGENTS.md §7); a watcher needs a new dependency, which is its own §8
/// checkpoint rather than a detail of this producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefreshTrigger {
    /// A project root was opened.
    ProjectOpened,
    /// One of smed's own repository commands completed.
    RepositoryCommand,
    /// A governed tool finished a write to the working tree.
    ToolWrite,
    /// A human-controlled desktop editor save completed.
    FileSave,
    /// A human asked for it.
    Requested,
}

impl RefreshTrigger {
    /// Stable identifier for the wire. Like a reason code, this is contract:
    /// prose may change, these may not (AGENTS.md §6).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectOpened => "projectOpened",
            Self::RepositoryCommand => "repositoryCommand",
            Self::ToolWrite => "toolWrite",
            Self::FileSave => "fileSave",
            Self::Requested => "requested",
        }
    }
}

impl RepositoryView {
    /// The projection, when there is one.
    #[must_use]
    pub fn projection(&self) -> Option<&RepositoryProjection> {
        match self {
            Self::Projected(projection) => Some(projection),
            Self::NoProject | Self::Unavailable { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_view_claims_nothing() {
        let view = RepositoryView::default();
        assert_eq!(view, RepositoryView::NoProject);
        assert!(view.projection().is_none());
    }

    #[test]
    fn an_unavailable_repository_is_not_an_empty_projection() {
        // The distinction this asserts is the whole reason `RepositoryView` is
        // an enum: an empty projection renders as a clean repository, which is
        // a positive claim about a repository smed could not read at all.
        let view = RepositoryView::Unavailable {
            code: ReasonCode::WorkspaceCapabilityUnavailable,
            detail: "not a git repository".to_owned(),
        };
        assert!(view.projection().is_none());
    }

    #[test]
    fn trigger_identifiers_are_stable_contract() {
        assert_eq!(RefreshTrigger::ProjectOpened.as_str(), "projectOpened");
        assert_eq!(
            RefreshTrigger::RepositoryCommand.as_str(),
            "repositoryCommand"
        );
        assert_eq!(RefreshTrigger::ToolWrite.as_str(), "toolWrite");
        assert_eq!(RefreshTrigger::FileSave.as_str(), "fileSave");
        assert_eq!(RefreshTrigger::Requested.as_str(), "requested");
    }
}

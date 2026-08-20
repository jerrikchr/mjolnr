//! Typed repository failures and their stable reason codes (Phase D5).

use crate::core::error::ReasonCode;

/// Why a governed repository operation did not produce the effect it names.
///
/// Every variant is reachable. An earlier draft declared `StaleIndex` and
/// `PartialEffect` without any path producing them, which made the taxonomy a
/// promise rather than a guard — the exact thing AGENTS.md §1.3 refuses.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepositoryError {
    /// The root mjolnr was handed cannot be a repository root at all. Checked in
    /// the constructor so a bad path fails before any process starts.
    #[error("{path} cannot be a repository root: {detail}")]
    InvalidRoot { path: String, detail: String },

    #[error("{path} is not inside a git repository")]
    NotARepository { path: String },

    /// `git` could not be run, or ran and failed for a reason mjolnr does not
    /// classify further. The stderr text is carried verbatim rather than
    /// summarised, because a guess about a failure is worse than the raw fact.
    #[error("git {operation} failed: {detail}")]
    CommandFailed {
        operation: &'static str,
        detail: String,
    },

    #[error("the working tree has uncommitted changes")]
    DirtyTree,

    /// The branch tip or checked-out branch no longer matches what the human
    /// approved for an external submission.
    #[error("the repository head moved: expected {expected}, found {found}")]
    StaleHead { expected: String, found: String },

    /// The index no longer hashes to the revision the caller inspected, so the
    /// commit the human approved is not the commit that would be created.
    #[error("the index moved: expected {expected}, found {found}")]
    StaleIndex { expected: String, found: String },

    #[error("the repository has unmerged paths: {paths}")]
    Conflict { paths: String },

    #[error("the repository has no branch checked out")]
    DetachedHead,

    /// The current branch has no upstream configured, so a push has no
    /// resolved destination — or, on the merge path, the upstream ref has
    /// never been fetched, so `@{upstream}` resolves to nothing local to
    /// integrate. Refused before any network call rather than guessing a
    /// remote (Phase D5 git surface).
    #[error("branch {branch} has no upstream configured")]
    NoUpstream { branch: String },

    /// The current branch is behind its remote-tracking ref. A push now would
    /// be rejected as non-fast-forward, so mjolnr refuses before the network
    /// call and tells the human to fetch or integrate first. Fail-closed
    /// (AGENTS.md §1.2) and the precondition the `behind > 0` check enforces.
    #[error(
        "branch is behind the remote by {behind} (ahead {ahead}); fetch or integrate before pushing"
    )]
    DivergedFromRemote { ahead: u32, behind: u32 },

    /// Attributed from filesystem evidence (the hook exists and is executable)
    /// plus git's own failure, never from a substring of prose alone.
    #[error("the {hook} hook refused the operation: {detail}")]
    HookRefused { hook: &'static str, detail: String },

    #[error("the commit could not be signed: {detail}")]
    SigningFailed { detail: String },

    /// mjolnr cannot prove whether the effect happened. Distinct from every
    /// failure above because it is neither success nor clean failure: it needs
    /// a human decision and must never be retried automatically
    /// (AGENTS.md §1.4).
    #[error("mjolnr cannot prove whether git {operation} took effect: {detail}")]
    UncertainEffect {
        operation: &'static str,
        detail: String,
    },

    /// The command is on the wire but its execution is not implemented. Named
    /// so a client renders "unavailable", not "failed".
    #[error("{capability} is not implemented")]
    CapabilityUnavailable { capability: &'static str },

    #[error("nothing is staged, so there is no commit to make")]
    NothingStaged,

    #[error("git {operation} returned more output than mjolnr can retain")]
    OutputTruncated { operation: &'static str },
}

impl RepositoryError {
    /// The stable code clients and tests assert on. Prose above may change
    /// freely; these may not (AGENTS.md §6).
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::InvalidRoot { .. } => ReasonCode::PathOutsideWorkspace,
            Self::NotARepository { .. } | Self::CapabilityUnavailable { .. } => {
                ReasonCode::WorkspaceCapabilityUnavailable
            }
            Self::CommandFailed { .. } | Self::NothingStaged => ReasonCode::ToolExecution,
            Self::OutputTruncated { .. } => ReasonCode::OutputTruncated,
            Self::DirtyTree => ReasonCode::WorkspaceDirty,
            Self::StaleHead { .. } | Self::StaleIndex { .. } => ReasonCode::WorkspaceStaleRevision,
            Self::Conflict { .. } => ReasonCode::RepositoryConflict,
            Self::DetachedHead => ReasonCode::RepositoryDetachedHead,
            Self::NoUpstream { .. } => ReasonCode::RepositoryNoUpstream,
            Self::DivergedFromRemote { .. } => ReasonCode::RepositoryDivergedFromRemote,
            Self::HookRefused { .. } => ReasonCode::RepositoryHookRefused,
            Self::SigningFailed { .. } => ReasonCode::RepositorySigningFailed,
            Self::UncertainEffect { .. } => ReasonCode::RepositoryUncertainEffect,
        }
    }

    /// Whether the outcome requires a human recovery decision rather than a
    /// retry or a dismissal.
    #[must_use]
    pub const fn requires_recovery(&self) -> bool {
        matches!(self, Self::UncertainEffect { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_carries_a_distinct_enough_code_to_act_on() {
        // The codes a client branches on must not collapse the three outcomes
        // that need different UI: refused, unavailable, and uncertain.
        assert_eq!(
            RepositoryError::DirtyTree.reason_code(),
            ReasonCode::WorkspaceDirty
        );
        assert_eq!(
            RepositoryError::CapabilityUnavailable {
                capability: "stageHunks"
            }
            .reason_code(),
            ReasonCode::WorkspaceCapabilityUnavailable
        );
        assert_eq!(
            RepositoryError::UncertainEffect {
                operation: "commit",
                detail: "HEAD moved after a failed commit".to_owned(),
            }
            .reason_code(),
            ReasonCode::RepositoryUncertainEffect
        );
    }

    #[test]
    fn only_an_uncertain_effect_asks_for_recovery() {
        assert!(
            RepositoryError::UncertainEffect {
                operation: "commit",
                detail: String::new(),
            }
            .requires_recovery()
        );
        assert!(!RepositoryError::DirtyTree.requires_recovery());
        assert!(
            !RepositoryError::Conflict {
                paths: "a.rs".to_owned()
            }
            .requires_recovery()
        );
    }

    #[test]
    fn display_never_swallows_the_underlying_git_text() {
        let error = RepositoryError::CommandFailed {
            operation: "commit",
            detail: "fatal: cannot lock ref".to_owned(),
        };
        assert!(error.to_string().contains("fatal: cannot lock ref"));
    }
}

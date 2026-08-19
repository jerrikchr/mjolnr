//! Change truth as the runtime holds it (Phase D3 producer).
//!
//! The same three-module split the D5 repository producer uses, for the same
//! reason: `core` defines these types, `crate::repository` produces them by
//! running `git diff`, and `runtime::client_bridge` projects them onto the
//! `core::changes` wire DTOs and applies the `MAX_DIFF_*` bounds. The split is
//! enforced, not stylistic — `tests/architecture.rs` forbids `crate::repository`
//! from naming `core::client::workspace`, which is where those bounds live, so
//! a producer that tried to clamp its own output could not compile.
//!
//! What this module deliberately does **not** carry:
//!
//! - a trust class — grading is the bridge's job, as with `RepositoryProjection`;
//! - a "current" or "fresh" flag — see [`RepositoryProjection`] for why nothing
//!   in a capture may claim its own currency;
//! - decoded text for a file git could not hand over as UTF-8. An undecodable
//!   file arrives with [`FileChange::undecodable`] set and no lines, because
//!   lossy decoding would put U+FFFD on a review surface and call it the file.
//!
//! [`RepositoryProjection`]: super::repository::RepositoryProjection

/// Everything `git diff` reported at one capture.
///
/// `base_revision` and `index_revision` are the anchor identity: a review note
/// taken against this capture is stale the moment either moves, and the bridge
/// refuses to accept it as current rather than sliding it to a new line.
///
/// `digest` is a SHA-256 over the exact diff bytes the human was shown. It is
/// the finest-grained staleness signal available — HEAD can stay put while the
/// working tree changes underneath a review — and it is what a review anchor
/// records. It identifies content only; it is not a git object id and must
/// never be handed to git.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangeCapture {
    pub base_revision: Option<String>,
    pub index_revision: Option<String>,
    pub digest: String,
    pub files: Vec<FileChange>,
    /// True when git's own output hit its byte bound, so this capture describes
    /// part of the working tree rather than all of it.
    pub output_truncated: bool,
    /// Untracked paths that exist but were not diffed, because diffing each one
    /// costs a process and the count is bounded. They are named, never dropped.
    pub undiffed_untracked: Vec<String>,
    /// Matches `RepositoryProjection::capture_sequence` for the same read, so a
    /// client can tell that a change set and a repository status describe one
    /// moment rather than two.
    pub capture_sequence: u32,
}

/// One file's change, with every awkward case as its own flag.
///
/// Binary, renamed, deleted, undecodable, and truncated are five different
/// conditions with five different remedies; collapsing them into one status
/// string is how a renderer ends up guessing which one it is looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileChange {
    pub path: String,
    /// The pre-rename path, when git detected a rename.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub hunks: Vec<Hunk>,
    /// git declined to produce text because the content is binary.
    pub binary: bool,
    /// git produced bytes that are not UTF-8. Carried as a flag with no lines
    /// rather than lossily decoded (see the module docs).
    pub undecodable: bool,
    /// This file's hunks were clamped by the producer's own bound. The bridge
    /// sets the wire-level `is_truncated` from this **or** its own clamping.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HunkLine {
    pub kind: LineSide,
    pub content: String,
    pub old_line_number: Option<u32>,
    pub new_line_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineSide {
    Unchanged,
    Added,
    Removed,
}

/// One file this session read, and the durable event that recorded the read.
///
/// The evidence half of [`ChangeCapture`]. The session's
/// [`ReadSet`](crate::core::tool::ReadSet) already knows *what* was read — path
/// and content hash — and is deliberately left alone: it is written inside the
/// tool, before any event exists, and it is what the edit gate compares
/// against. This is the second, later fact: which `ToolCompleted` event carried
/// that read. It is recorded where the store hands the event its id, which is
/// the only place the id exists.
///
/// `tool_event_id` is therefore never derived, guessed, or synthesised. A read
/// whose event id is unknown produces no record at all, because a review that
/// cites an event that does not exist is worse than a review that cites
/// nothing (AGENTS.md §1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadRecord {
    /// Workspace-relative, as `tools::files::display_path` produced it.
    pub path: String,
    /// SHA-256 of the content that was read.
    pub sha256: String,
    /// The `ToolCompleted` event's id, as the store assigned it.
    pub tool_event_id: String,
}

impl ReadRecord {
    /// Build a record from the three facts a completed read produces.
    ///
    /// A constructor rather than a struct literal because the type is
    /// `#[non_exhaustive]` — a later field must not break the checkpoint
    /// fixtures and integration tests that build one — and because every
    /// caller should have to name `tool_event_id`, which is the field the
    /// whole record exists to carry.
    #[must_use]
    pub fn new(path: String, sha256: String, tool_event_id: String) -> Self {
        Self {
            path,
            sha256,
            tool_event_id,
        }
    }
}

/// What the runtime knows about the open project's changes.
///
/// Mirrors [`RepositoryView`](super::repository::RepositoryView) variant for
/// variant on purpose: the two are captured together and a client that can
/// render one state for the repository and a different one for its changes is
/// showing a repository that never existed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ChangeView {
    #[default]
    NoProject,
    /// A project is open but git could not answer. The change surface renders
    /// the refusal; it must not render an empty change set, which reads as
    /// "nothing has changed".
    Unavailable {
        code: crate::core::error::ReasonCode,
        detail: String,
    },
    Captured(Box<ChangeCapture>),
}

impl ChangeView {
    /// The capture, when there is one.
    #[must_use]
    pub fn capture(&self) -> Option<&ChangeCapture> {
        match self {
            Self::Captured(capture) => Some(capture),
            Self::NoProject | Self::Unavailable { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;

    #[test]
    fn the_default_view_claims_nothing() {
        let view = ChangeView::default();
        assert_eq!(view, ChangeView::NoProject);
        assert!(view.capture().is_none());
    }

    /// The same distinction `RepositoryView` exists for: an unreadable
    /// repository must not be indistinguishable from a clean one. An empty
    /// `files` list is a positive claim that nothing changed.
    #[test]
    fn an_unavailable_change_view_is_not_an_empty_capture() {
        let view = ChangeView::Unavailable {
            code: ReasonCode::WorkspaceCapabilityUnavailable,
            detail: "not a git repository".to_owned(),
        };
        assert!(view.capture().is_none());
    }
}

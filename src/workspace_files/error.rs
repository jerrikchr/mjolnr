//! Typed file-projection failures and their stable reason codes (Phase D7).

use crate::core::error::ReasonCode;
use crate::policy::paths::PathRefusal;

/// Why a contained read of the project's files did not produce a projection.
///
/// Every variant is reachable from a test in this module. A taxonomy with a
/// variant nothing produces is a promise rather than a guard — the same note
/// `RepositoryError` carries, for the same reason.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceFileError {
    /// Containment refused the path. Carries the code `policy::paths` decided —
    /// `PATH_OUTSIDE_WORKSPACE` or `PATH_SYMLINK_ESCAPE` — because the
    /// difference between "you asked for somewhere else" and "this link leads
    /// somewhere else" is the difference between a typo and an escape attempt,
    /// and flattening them would hide the second.
    #[error("{detail}")]
    Refused { code: ReasonCode, detail: String },

    /// The path exists and is contained, but is not the kind of thing the
    /// operation needs.
    #[error("{path} is not {expected}")]
    WrongKind {
        path: String,
        expected: &'static str,
    },

    /// The filesystem refused the read. Carried verbatim: a guess about why a
    /// read failed is worse than the raw fact.
    #[error("cannot read {path}: {detail}")]
    Io { path: String, detail: String },

    /// The bytes on disk no longer match the digest the human approved.
    #[error(
        "{path} changed after it was opened (expected {expected}, found {actual}); compare before overwriting"
    )]
    Stale {
        path: String,
        expected: String,
        actual: String,
    },

    /// The target cannot be represented by the editor save contract.
    #[error("cannot save {path}: {detail}")]
    Uneditable { path: String, detail: &'static str },

    /// The incoming editor buffer exceeds the bounded save contract.
    #[error("cannot save {path}: content exceeds the {limit}-byte save limit")]
    TooLarge { path: String, limit: u64 },

    /// A page past the end of the directory. Refused rather than answered with
    /// an empty page, for the reason D4 refuses a cursor past its bound: an
    /// empty page and "there is nothing here" are the same bytes on the wire
    /// and different facts.
    #[error("page {page} is past the end of {path}, which has {pages} page(s)")]
    PageOutOfRange { path: String, page: u32, pages: u32 },
}

impl WorkspaceFileError {
    /// The stable code a client matches on (AGENTS.md §6).
    #[must_use]
    pub const fn reason_code(&self) -> ReasonCode {
        match self {
            Self::Refused { code, .. } => *code,
            // Both are "you asked for something that is not there in the shape
            // you asked for", which is a schema-level mistake by the caller,
            // not a filesystem fault.
            Self::WrongKind { .. } | Self::PageOutOfRange { .. } => ReasonCode::SchemaInvalid,
            Self::Io { .. } => ReasonCode::ToolExecution,
            Self::Stale { .. } => ReasonCode::StaleFileVersion,
            Self::Uneditable { .. } => ReasonCode::WorkspaceCapabilityUnavailable,
            Self::TooLarge { .. } => ReasonCode::OutputTruncated,
        }
    }
}

impl From<PathRefusal> for WorkspaceFileError {
    fn from(refusal: PathRefusal) -> Self {
        Self::Refused {
            code: refusal.code,
            detail: refusal.detail,
        }
    }
}

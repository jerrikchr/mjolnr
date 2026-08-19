//! Typed memory-projection failures and their stable reason codes
//! (master implementation plan, Phase 1).
//!
//! Codes reuse the existing `ReasonCode` vocabulary rather than minting new
//! ones: every failure memory can produce is already a category the clients
//! know how to render — a path problem, a truncation problem, an
//! availability problem, or an execution problem.

use crate::core::error::ReasonCode;

/// Why a memory operation did not produce what it names.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemoryError {
    /// A declared rule file or the user profile exceeds its character limit.
    ///
    /// The limit is the consolidation mechanism (plan §2.1): refusing the file
    /// is what forces the rules to stay small. The file is skipped, not
    /// silently truncated — a truncated rule says something its author did
    /// not say.
    #[error("{path} is {actual} characters; the limit is {limit}")]
    RuleLimitExceeded {
        path: String,
        actual: usize,
        limit: usize,
    },

    /// A rules path escaped the workspace after canonicalisation.
    #[error("{path} escapes the workspace")]
    PathEscape { path: String },

    /// The projection database could not be opened or is not a database.
    ///
    /// Per Standing Law #2 this is an inconvenience, never data loss: the
    /// answer is to rebuild the projection, not to recover it.
    #[error("the memory projection is unavailable: {detail}")]
    Unavailable { detail: String },

    /// A query could not be answered — structurally, not "nothing matched".
    #[error("the memory query was refused: {detail}")]
    QueryRefused { detail: String },

    /// The projection database failed in a way smed does not classify
    /// further. The SQLite text is carried verbatim.
    #[error("the memory projection failed: {detail}")]
    Execution { detail: String },

    /// A requested fact id does not exist (or is no longer current).
    #[error("no memory entry with id {id}")]
    NotFound { id: String },
}

impl MemoryError {
    /// The stable code clients and tests assert on (AGENTS.md §6).
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::RuleLimitExceeded { .. } => ReasonCode::OutputTruncated,
            Self::PathEscape { .. } => ReasonCode::PathOutsideWorkspace,
            Self::Unavailable { .. } => ReasonCode::WorkspaceCapabilityUnavailable,
            Self::QueryRefused { .. } => ReasonCode::WorkspaceSearchRefused,
            Self::Execution { .. } | Self::NotFound { .. } => ReasonCode::ToolExecution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_a_code_clients_already_render() {
        // No new reason codes: memory failures must land in categories the
        // clients already know, or the taxonomy is a promise, not a guard.
        let cases = [
            (
                MemoryError::RuleLimitExceeded {
                    path: ".mjolnr/rules/x.md".to_owned(),
                    actual: 20_000,
                    limit: 16_384,
                },
                ReasonCode::OutputTruncated,
            ),
            (
                MemoryError::PathEscape {
                    path: "/etc/passwd".to_owned(),
                },
                ReasonCode::PathOutsideWorkspace,
            ),
            (
                MemoryError::Unavailable {
                    detail: "not a database".to_owned(),
                },
                ReasonCode::WorkspaceCapabilityUnavailable,
            ),
            (
                MemoryError::QueryRefused {
                    detail: "query too short".to_owned(),
                },
                ReasonCode::WorkspaceSearchRefused,
            ),
            (
                MemoryError::Execution {
                    detail: "disk I/O error".to_owned(),
                },
                ReasonCode::ToolExecution,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.reason_code(), code);
        }
    }
}

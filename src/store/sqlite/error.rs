//! The store's internal error, and how it becomes a [`StoreError`].
//!
//! # Why a second error type
//!
//! [`StoreError`] is `core`'s vocabulary: it is what the runtime matches on, and
//! it deliberately knows nothing about SQLite. But the SQL layer genuinely
//! produces SQLite errors, and threading `rusqlite::Error` up to `core` would
//! either put a SQLite dependency in `core` (breaking `AGENTS.md` §2.1) or
//! stringify everything at the point of failure, losing the distinctions the
//! runtime needs.
//!
//! So the SQL layer speaks [`SqlError`], and exactly one conversion — here —
//! decides which failures are *conditions the runtime must reason about*
//! (a sequence gap, an unsupported schema) and which are merely *the store being
//! broken* ([`StoreError::Unavailable`]).

use crate::core::event::SessionId;
use crate::core::store::StoreError;
use crate::store::wire::WireError;

pub(super) type SqlResult<T> = Result<T, SqlError>;

/// What can go wrong inside the SQLite store.
#[derive(Debug, thiserror::Error)]
pub(super) enum SqlError {
    #[error(transparent)]
    Sqlite(#[from] tokio_rusqlite::rusqlite::Error),

    /// The connection thread is gone. Every later call fails the same way.
    #[error("the database connection is closed")]
    ConnectionClosed,

    #[error("session {session} is missing event sequence {missing}")]
    Gap { session: SessionId, missing: u64 },

    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },

    #[error("{detail}")]
    Decode { detail: String },

    #[error("workspace path is unsupported: {detail}")]
    InvalidProjectPath { detail: String },

    /// The query was understood and declined. Not a database failure — see
    /// `StoreError::Refused` for why the two must not share a variant.
    #[error("{detail}")]
    Refused { detail: String },
}

impl From<WireError> for SqlError {
    fn from(error: WireError) -> Self {
        match error {
            // A version this build cannot read is a schema condition, not a
            // decode bug: the distinction is what tells a user to upgrade rather
            // than to report corruption.
            WireError::UnsupportedVersion { found, supported } => {
                Self::SchemaTooNew { found, supported }
            }
            WireError::Ephemeral { .. } | WireError::Decode { .. } => Self::Decode {
                detail: error.to_string(),
            },
        }
    }
}

impl From<SqlError> for StoreError {
    fn from(error: SqlError) -> Self {
        match error {
            SqlError::Gap { session, missing } => Self::SequenceGap { session, missing },
            SqlError::SchemaTooNew { found, supported } => {
                Self::UnsupportedSchema { found, supported }
            }
            SqlError::Decode { detail } => Self::Decode { detail },
            SqlError::InvalidProjectPath { detail } => Self::Unavailable { detail },
            SqlError::Refused { detail } => Self::Refused { detail },
            SqlError::Sqlite(inner) => Self::Unavailable {
                detail: inner.to_string(),
            },
            SqlError::ConnectionClosed => Self::Unavailable {
                detail: "the database connection is closed".to_owned(),
            },
        }
    }
}

/// Flatten `tokio_rusqlite`'s two-layer error into ours.
///
/// `Connection::call` returns `Error<E>`, where `E` is whatever the closure
/// returned. `ConnectionClosed` and `Close` are the crate's own failures; only
/// `Error(E)` carries ours.
///
/// The enum is `#[non_exhaustive]`, so a catch-all is mandatory. It resolves to
/// "the store is unusable" rather than to anything recoverable: an unknown
/// failure from the layer that owns the connection is not a condition to
/// interpret optimistically (`AGENTS.md` §1.2).
impl From<tokio_rusqlite::Error<SqlError>> for SqlError {
    fn from(error: tokio_rusqlite::Error<SqlError>) -> Self {
        match error {
            tokio_rusqlite::Error::Error(inner) => inner,
            tokio_rusqlite::Error::ConnectionClosed => Self::ConnectionClosed,
            tokio_rusqlite::Error::Close((_, inner)) => Self::Sqlite(inner),
            other => Self::Decode {
                detail: format!("unrecognised store failure: {other}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gap_stays_a_gap_rather_than_becoming_unavailable() {
        // The runtime must be able to tell "history is incomplete" from "the
        // database is unreachable": one is unrecoverable data loss, the other is
        // worth retrying.
        let session = SessionId::new();
        let error = StoreError::from(SqlError::Gap {
            session,
            missing: 7,
        });

        match error {
            StoreError::SequenceGap {
                session: found,
                missing,
            } => {
                assert_eq!(found, session);
                assert_eq!(missing, 7);
            }
            other => panic!("a gap must not be flattened into {other:?}"),
        }
    }

    #[test]
    fn an_unsupported_payload_version_becomes_an_unsupported_schema() {
        let error = StoreError::from(SqlError::from(WireError::UnsupportedVersion {
            found: 9,
            supported: 1,
        }));
        assert!(matches!(
            error,
            StoreError::UnsupportedSchema {
                found: 9,
                supported: 1
            }
        ));
    }

    #[test]
    fn an_ephemeral_event_reaching_the_store_is_a_decode_failure_not_a_silent_drop() {
        // Persisting a TextDelta is a bug. It must surface, not vanish.
        let error = StoreError::from(SqlError::from(WireError::Ephemeral { kind: "text_delta" }));
        assert!(matches!(error, StoreError::Decode { .. }));
        assert!(error.to_string().contains("text_delta"));
    }

    #[test]
    fn a_closed_connection_reports_as_unavailable() {
        let error = StoreError::from(SqlError::from(
            tokio_rusqlite::Error::<SqlError>::ConnectionClosed,
        ));
        assert!(matches!(error, StoreError::Unavailable { .. }));
    }
}

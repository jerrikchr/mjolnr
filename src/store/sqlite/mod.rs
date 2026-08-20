//! The durable [`EventStore`] .
//!
//! A handle in front of one bounded actor in front of one SQLite connection.
//! The runtime holds `Arc<dyn EventStore>` and never learns any of that exists.
//!
//! Contract details this implementation depends on are recorded in
//! `docs/persistence.md`, read from official sources rather than memory.

mod actor;
mod error;
mod queries;
mod schema;
mod search;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio_rusqlite::Connection;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::event::{MjolnrEvent, SessionId, StoredEvent};
use crate::core::store::{
    DiagnosticsReport, EventStore, IntegrityReport, ProjectId, SessionLease, SessionSummary,
    StoreDiagnostics, StoreError, StoredCheckpoint, WorkspaceSearchFilter, WorkspaceSearchPage,
};
use crate::store::sqlite::actor::Request;
use crate::store::sqlite::schema::MigrationOutcome;

/// Bounded request queue.
///
/// The backpressure boundary (`docs/persistence.md` §1.3). Sized so a burst of
/// tool events during one turn does not stall the runtime, while a store that
/// has genuinely stopped keeping up makes `append` await rather than buffering
/// without limit.
const QUEUE_CAPACITY: usize = 64;

/// A durable event store backed by SQLite.
#[derive(Debug)]
pub struct SqliteEventStore {
    requests: mpsc::Sender<Request>,
    database_path: PathBuf,
}

impl SqliteEventStore {
    /// Rebuild the workspace search index from the durable record (Phase D4).
    ///
    /// Returns how many documents were written. Deterministic: it replays
    /// `events` through the same projection an append uses, so §D4's "rebuilding
    /// the index produces the same document set and stable result order" is a
    /// property of there being exactly one projection rather than of two
    /// implementations agreeing.
    ///
    /// Inherent rather than on the `EventStore` trait. A rebuild is a
    /// maintenance operation on *this* backend — the in-memory store has no
    /// index to rebuild — and putting it on the trait would oblige every
    /// implementation to answer a question only one of them has.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] when SQLite cannot complete the rebuild. An
    /// event whose payload this build cannot decode is skipped rather than
    /// failing the rebuild: it stays durable and is simply not searchable here.
    pub async fn rebuild_search_index(&self) -> Result<u64, StoreError> {
        self.request(|reply| Request::RebuildSearchIndex { reply })
            .await
    }

    /// Open (creating if absent), migrate, and start the writer.
    ///
    /// # Errors
    /// - [`StoreError::UnsupportedSchema`] when the database was written by a
    ///   newer mjolnr. Nothing is modified in that case, including the WAL mode.
    /// - [`StoreError::Unavailable`] when SQLite cannot open or migrate it.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database_path = path.as_ref().to_path_buf();

        let connection =
            Connection::open(&database_path)
                .await
                .map_err(|error| StoreError::Unavailable {
                    detail: format!("could not open {}: {error}", database_path.display()),
                })?;

        Self::start(connection, database_path).await
    }

    /// Open a private in-memory database.
    ///
    /// For tests that need the real SQL — migrations, constraints, integrity —
    /// without a file. It cannot test reopening, by construction: the database
    /// dies with the connection. Tests that exercise restart use a temporary
    /// file instead.
    ///
    /// # Errors
    /// As [`open`](Self::open).
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let connection =
            Connection::open_in_memory()
                .await
                .map_err(|error| StoreError::Unavailable {
                    detail: format!("could not open an in-memory database: {error}"),
                })?;

        Self::start(connection, PathBuf::from(":memory:")).await
    }

    async fn start(connection: Connection, database_path: PathBuf) -> Result<Self, StoreError> {
        let outcome = connection
            .call(|c| {
                schema::apply_connection_pragmas(c)?;
                schema::migrate(c)
            })
            .await
            .map_err(
                |error: tokio_rusqlite::Error<tokio_rusqlite::rusqlite::Error>| {
                    StoreError::Unavailable {
                        detail: format!("could not prepare the database: {error}"),
                    }
                },
            )?;

        // Refuse before starting the writer: a store this build cannot fully
        // understand must not accept a single write (`AGENTS.md` §1.2).
        if let MigrationOutcome::TooNew { found } = outcome {
            let _ = connection.close().await;
            return Err(StoreError::UnsupportedSchema {
                found,
                supported: schema::SCHEMA_VERSION,
            });
        }

        let (requests, receiver) = mpsc::channel(QUEUE_CAPACITY);
        tokio::spawn(actor::run(connection, database_path.clone(), receiver));

        Ok(Self {
            requests,
            database_path,
        })
    }

    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Close the SQLite connection, waiting for its final checkpoint.
    ///
    /// The composition root calls this on the way out, once nothing will use the
    /// store again. Dropping the last handle instead also closes the connection,
    /// but nobody is waiting for that close: the actor task is doing it, and a
    /// Tokio runtime that finishes shutting down first drops the task mid-close.
    /// That drops the reply channel `tokio-rusqlite`'s connection thread sends
    /// on, and it `expect`s that send — which panicked at process exit roughly
    /// one run in seven. Awaiting the close here removes the race rather than
    /// hiding the panic.
    ///
    /// Requests already queued behind this one are answered as unavailable, so
    /// this must be last. Calling it twice is harmless: the second call finds no
    /// actor and reports the store shut down, which it is.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] when SQLite reported a problem closing, or
    /// when the store was already gone. Both are shutdown-time facts a caller
    /// may want to print; neither is actionable beyond that.
    pub async fn close(&self) -> Result<(), StoreError> {
        self.request(|reply| Request::Close { reply }).await
    }

    /// Send a request and await its reply.
    ///
    /// `build` receives the reply channel so each variant can carry its own,
    /// which is what lets callers await their own result rather than a shared
    /// signal.
    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, StoreError>>) -> Request,
    ) -> Result<T, StoreError> {
        let (reply, answer) = oneshot::channel();

        // `send` awaits capacity — this is where backpressure reaches the caller.
        self.requests
            .send(build(reply))
            .await
            .map_err(|_| StoreError::Unavailable {
                detail: "the store has shut down".to_owned(),
            })?;

        answer.await.map_err(|_| StoreError::Unavailable {
            detail: "the store dropped a request without answering".to_owned(),
        })?
    }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn open_project(&self, root: PathBuf) -> Result<ProjectId, StoreError> {
        self.request(|reply| Request::OpenProject { root, reply })
            .await
    }

    async fn create_session(
        &self,
        session: SessionId,
        project: ProjectId,
        title: String,
        parent: Option<SessionId>,
    ) -> Result<(), StoreError> {
        self.request(|reply| Request::CreateSession {
            session,
            project,
            title,
            parent,
            reply,
        })
        .await
    }

    async fn end_session(&self, session: SessionId) -> Result<(), StoreError> {
        self.request(|reply| Request::EndSession { session, reply })
            .await
    }

    async fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        self.request(|reply| Request::Sessions { reply }).await
    }

    async fn append(&self, event: MjolnrEvent) -> Result<StoredEvent, StoreError> {
        self.request(|reply| Request::Append {
            event: Box::new(event),
            reply,
        })
        .await
    }

    async fn events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.events_from(session, 0).await
    }

    async fn append_after(
        &self,
        event: MjolnrEvent,
        parent: Option<u64>,
    ) -> Result<StoredEvent, StoreError> {
        self.request(|reply| Request::AppendAfter {
            event: Box::new(event),
            parent,
            reply,
        })
        .await
    }

    async fn set_active_leaf(
        &self,
        session: SessionId,
        sequence: Option<u64>,
    ) -> Result<(), StoreError> {
        self.request(|reply| Request::SetActiveLeaf {
            session,
            sequence,
            reply,
        })
        .await
    }

    async fn branch_events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.request(|reply| Request::BranchEvents { session, reply })
            .await
    }

    async fn branch_events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Option<crate::core::store::BranchResume>, StoreError> {
        self.request(|reply| Request::BranchEventsFrom {
            session,
            from,
            reply,
        })
        .await
    }

    async fn session_tree(
        &self,
        session: SessionId,
    ) -> Result<Vec<crate::core::store::SessionTreeNode>, StoreError> {
        self.request(|reply| Request::SessionTree { session, reply })
            .await
    }

    async fn branch_summary(
        &self,
        session: SessionId,
        leaf: u64,
    ) -> Result<crate::core::store::BranchSummary, StoreError> {
        self.request(|reply| Request::BranchSummary {
            session,
            leaf,
            reply,
        })
        .await
    }

    async fn events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.request(|reply| Request::EventsFrom {
            session,
            from,
            reply,
        })
        .await
    }

    async fn find_session_by_dir(
        &self,
        project_root: PathBuf,
    ) -> Result<Option<SessionId>, StoreError> {
        self.request(|reply| Request::FindSessionByDir {
            project_root,
            reply,
        })
        .await
    }

    /// One page of a deterministic workspace search (Phase D4 producer).
    ///
    /// Ordered by time, never by relevance: `bm25` scores against corpus
    /// statistics, so identical queries would reorder as the store grows, and
    /// §D4 requires a rebuild to reproduce a stable order.
    ///
    /// An unanswerable question refuses with [`StoreError::Refused`] rather
    /// than returning an empty page, because "nothing matched" and "that could
    /// not be matched" send a user to different remedies.
    async fn search_workspace(
        &self,
        filter: WorkspaceSearchFilter,
    ) -> Result<WorkspaceSearchPage, StoreError> {
        self.request(|reply| Request::SearchWorkspace {
            filter: Box::new(filter),
            reply,
        })
        .await
    }

    async fn write_checkpoint(&self, checkpoint: SessionCheckpoint) -> Result<u64, StoreError> {
        self.request(|reply| Request::WriteCheckpoint {
            checkpoint: Box::new(checkpoint),
            reply,
        })
        .await
    }

    async fn latest_checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<StoredCheckpoint>, StoreError> {
        self.request(|reply| Request::LatestCheckpoint { session, reply })
            .await
    }

    async fn acquire_session(&self, session: SessionId) -> Result<SessionLease, StoreError> {
        self.request(|reply| Request::AcquireSession { session, reply })
            .await
    }

    async fn release_session(&self, lease: &SessionLease) -> Result<(), StoreError> {
        let lease = lease.clone();
        self.request(|reply| Request::ReleaseSession { lease, reply })
            .await
    }

    async fn break_lease(&self, session: SessionId) -> Result<(), StoreError> {
        self.request(|reply| Request::BreakLease { session, reply })
            .await
    }

    async fn flush(&self) -> Result<(), StoreError> {
        self.request(|reply| Request::Flush { reply }).await
    }
}

#[async_trait]
impl StoreDiagnostics for SqliteEventStore {
    async fn report(&self) -> Result<DiagnosticsReport, StoreError> {
        self.request(|reply| Request::Diagnostics { reply }).await
    }

    async fn integrity_check(&self) -> Result<IntegrityReport, StoreError> {
        self.request(|reply| Request::IntegrityCheck { reply })
            .await
    }
}

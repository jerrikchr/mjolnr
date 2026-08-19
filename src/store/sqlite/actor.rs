//! The single bounded writer.
//!
//! ```text
//! callers ──bounded mpsc──▶ actor task ──call()──▶ connection thread ──▶ SQLite
//!         ◀───oneshot────────┘
//! ```
//!
//! # Why this exists when `tokio-rusqlite` already serialises calls
//!
//! Because its queue is `crossbeam_channel::unbounded` (`docs/persistence.md`
//! §1.3). Calling `Connection::call` from every writer would serialise the work
//! but apply **no backpressure**: a runtime that outran SQLite would grow the
//! heap silently, which is exactly what `AGENTS.md` §4 bans unbounded channels
//! to prevent.
//!
//! The actor takes requests from a *bounded* channel and awaits each `call`
//! before accepting the next. Two things follow:
//!
//! 1. At most one closure is ever in the crate's queue, so its unboundedness is
//!    unreachable rather than merely unused.
//! 2. A slow disk stalls the actor, which fills the bounded channel, which makes
//!    the runtime's `append` await. Backpressure composes end-to-end.
//!
//! Ordering is a consequence of the same property: requests are handled one at a
//! time, in arrival order, by one task.  requires exactly that.

use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, oneshot};
use tokio_rusqlite::Connection;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::event::{SessionId, SmedEvent, StoredEvent};
use crate::core::store::{
    DiagnosticsReport, IntegrityReport, ProjectId, SessionLease, SessionSummary, StoreError,
    StoredCheckpoint,
};
use crate::store::sqlite::error::{SqlError, SqlResult};
use crate::store::sqlite::queries;
use crate::store::sqlite::schema;

/// Where a handler sends its answer.
type Reply<T> = oneshot::Sender<Result<T, StoreError>>;

/// One unit of work for the store.
///
/// Every variant carries its own reply channel, so a caller awaits *its* result
/// rather than a shared "the store did something" signal.
pub(super) enum Request {
    OpenProject {
        root: PathBuf,
        reply: Reply<ProjectId>,
    },
    CreateSession {
        session: SessionId,
        project: ProjectId,
        title: String,
        parent: Option<SessionId>,
        reply: Reply<()>,
    },
    EndSession {
        session: SessionId,
        reply: Reply<()>,
    },
    Sessions {
        reply: Reply<Vec<SessionSummary>>,
    },
    Append {
        event: Box<SmedEvent>,
        reply: Reply<StoredEvent>,
    },
    AppendAfter {
        event: Box<SmedEvent>,
        parent: Option<u64>,
        reply: Reply<StoredEvent>,
    },
    SetActiveLeaf {
        session: SessionId,
        sequence: Option<u64>,
        reply: Reply<()>,
    },
    BranchEvents {
        session: SessionId,
        reply: Reply<Vec<StoredEvent>>,
    },
    BranchEventsFrom {
        session: SessionId,
        from: u64,
        reply: Reply<Option<crate::core::store::BranchResume>>,
    },
    SessionTree {
        session: SessionId,
        reply: Reply<Vec<crate::core::store::SessionTreeNode>>,
    },
    BranchSummary {
        session: SessionId,
        leaf: u64,
        reply: Reply<crate::core::store::BranchSummary>,
    },
    EventsFrom {
        session: SessionId,
        from: u64,
        reply: Reply<Vec<StoredEvent>>,
    },
    WriteCheckpoint {
        checkpoint: Box<SessionCheckpoint>,
        reply: Reply<u64>,
    },
    LatestCheckpoint {
        session: SessionId,
        reply: Reply<Option<StoredCheckpoint>>,
    },
    FindSessionByDir {
        project_root: PathBuf,
        reply: Reply<Option<SessionId>>,
    },
    /// One page of a workspace search (Phase D4 producer).
    ///
    /// The reply carries a `Result` inside the store's own so a *refusal* —
    /// a query too short for the trigram index, a cursor from another filter —
    /// stays distinguishable from a database failure. Collapsing the two would
    /// make "you asked something unanswerable" look like "the store broke".
    SearchWorkspace {
        filter: Box<crate::core::store::WorkspaceSearchFilter>,
        reply: Reply<crate::core::store::WorkspaceSearchPage>,
    },
    /// Rebuild the search index from the durable record (Phase D4 producer).
    RebuildSearchIndex {
        reply: Reply<u64>,
    },
    AcquireSession {
        session: SessionId,
        reply: Reply<SessionLease>,
    },
    ReleaseSession {
        lease: SessionLease,
        reply: Reply<()>,
    },
    BreakLease {
        session: SessionId,
        reply: Reply<()>,
    },
    Flush {
        reply: Reply<()>,
    },
    Diagnostics {
        reply: Reply<DiagnosticsReport>,
    },
    IntegrityCheck {
        reply: Reply<IntegrityReport>,
    },
    /// Close the SQLite connection and stop, answering when it is closed.
    ///
    /// The last request the store ever handles. It exists so shutdown *waits*
    /// for SQLite's final checkpoint instead of racing the Tokio runtime's
    /// teardown; see [`run`].
    Close {
        reply: Reply<()>,
    },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::OpenProject { .. } => "OpenProject",
            Self::CreateSession { .. } => "CreateSession",
            Self::EndSession { .. } => "EndSession",
            Self::Sessions { .. } => "Sessions",
            Self::Append { .. } => "Append",
            Self::AppendAfter { .. } => "AppendAfter",
            Self::SetActiveLeaf { .. } => "SetActiveLeaf",
            Self::BranchEvents { .. } => "BranchEvents",
            Self::BranchEventsFrom { .. } => "BranchEventsFrom",
            Self::SessionTree { .. } => "SessionTree",
            Self::BranchSummary { .. } => "BranchSummary",
            Self::EventsFrom { .. } => "EventsFrom",
            Self::WriteCheckpoint { .. } => "WriteCheckpoint",
            Self::LatestCheckpoint { .. } => "LatestCheckpoint",
            Self::FindSessionByDir { .. } => "FindSessionByDir",
            Self::SearchWorkspace { .. } => "SearchWorkspace",
            Self::RebuildSearchIndex { .. } => "RebuildSearchIndex",
            Self::AcquireSession { .. } => "AcquireSession",
            Self::ReleaseSession { .. } => "ReleaseSession",
            Self::BreakLease { .. } => "BreakLease",
            Self::Flush { .. } => "Flush",
            Self::Diagnostics { .. } => "Diagnostics",
            Self::IntegrityCheck { .. } => "IntegrityCheck",
            Self::Close { .. } => "Close",
        };
        formatter.write_str(name)
    }
}

/// Drive the store until it is asked to close or every sender is dropped.
///
/// Closing lets SQLite run its final checkpoint and remove the `-wal` file
/// rather than leaving the database looking crash-interrupted. It is awaited
/// work on a spawned task, which is why [`Request::Close`] exists: reaching this
/// only because the last handle dropped means nothing is waiting for the close,
/// and a Tokio runtime that shuts down first drops this task mid-`close`. That
/// drops the reply channel `tokio-rusqlite`'s connection thread is about to send
/// on, and it `expect`s that send — a panic in a dependency, caused by us.
pub(super) async fn run(
    connection: Connection,
    database_path: PathBuf,
    mut requests: mpsc::Receiver<Request>,
) {
    let closing = drain(&connection, &database_path, &mut requests).await;

    let closed = connection
        .close()
        .await
        .map_err(|error| StoreError::Unavailable {
            detail: format!("could not close the database cleanly: {error}"),
        });

    // Only an explicit `Close` has somebody waiting for the answer.
    if let Some(reply) = closing {
        let _ = reply.send(closed);
    }
}

/// Handle requests in arrival order, returning the reply channel of an explicit
/// close request, or `None` when every handle was dropped instead.
///
/// One request at a time, awaited to completion. This loop *is* the ordering and
/// backpressure guarantee; a `tokio::spawn` per request would silently destroy
/// both.
async fn drain(
    connection: &Connection,
    database_path: &Path,
    requests: &mut mpsc::Receiver<Request>,
) -> Option<Reply<()>> {
    while let Some(request) = requests.recv().await {
        // Nothing after a close can be served, so stop reading rather than
        // answering later requests from a closed connection. Queued senders see
        // the store shut down, which is what it did.
        if let Request::Close { reply } = request {
            return Some(reply);
        }
        handle(connection, database_path, request).await;
    }
    None
}

/// Dispatch one request.
///
/// Split into session, event, and lease groups purely for legibility — every
/// arm still runs on this one task, in arrival order, which is what makes the
/// ordering guarantee true.
async fn handle(connection: &Connection, database_path: &Path, request: Request) {
    match request {
        Request::OpenProject { .. }
        | Request::CreateSession { .. }
        | Request::EndSession { .. }
        | Request::Sessions { .. } => handle_session(connection, request).await,

        Request::Append { .. }
        | Request::AppendAfter { .. }
        | Request::SetActiveLeaf { .. }
        | Request::BranchEvents { .. }
        | Request::BranchEventsFrom { .. }
        | Request::SessionTree { .. }
        | Request::BranchSummary { .. }
        | Request::EventsFrom { .. }
        | Request::WriteCheckpoint { .. }
        | Request::LatestCheckpoint { .. }
        | Request::FindSessionByDir { .. }
        | Request::SearchWorkspace { .. }
        | Request::RebuildSearchIndex { .. } => handle_history(connection, request).await,

        Request::AcquireSession { .. }
        | Request::ReleaseSession { .. }
        | Request::BreakLease { .. } => handle_lease(connection, request).await,

        Request::Flush { reply } => {
            // Reaching this request already means every earlier one committed —
            // the loop is sequential. The round trip through the connection
            // thread proves the thread itself is alive and drained, so `Ok` is a
            // fact rather than an assumption (AGENTS.md §1.3).
            answer(reply, call(connection, |_| Ok(())).await);
        }
        Request::Diagnostics { reply } => {
            let path = database_path.to_path_buf();
            answer(reply, call(connection, move |c| diagnostics(c, path)).await);
        }
        Request::IntegrityCheck { reply } => {
            answer(reply, call(connection, queries::integrity_check).await);
        }
        // `drain` intercepts this before dispatch, because closing needs the
        // connection by value. The arm exists for exhaustiveness; answering
        // anything but an error here would claim a close that did not happen.
        Request::Close { reply } => answer(
            reply,
            Err(StoreError::Unavailable {
                detail: "the store did not reach its close path".to_owned(),
            }),
        ),
    }
}

/// Projects and session rows.
async fn handle_session(connection: &Connection, request: Request) {
    match request {
        Request::OpenProject { root, reply } => {
            answer(
                reply,
                call(connection, move |c| queries::open_project(c, &root)).await,
            );
        }
        Request::CreateSession {
            session,
            project,
            title,
            parent,
            reply,
        } => {
            let result = call(connection, move |c| {
                queries::create_session(c, session, project, &title, parent)
            })
            .await;
            answer(reply, result);
        }
        Request::EndSession { session, reply } => {
            let result = match call(connection, move |c| queries::end_session(c, session)).await {
                Ok(true) => Ok(()),
                Ok(false) => Err(StoreError::UnknownSession { session }),
                Err(error) => Err(error),
            };
            answer(reply, result);
        }
        Request::Sessions { reply } => {
            answer(reply, call(connection, queries::sessions).await);
        }
        other => unreachable_request(&other),
    }
}

/// Events and checkpoints.
#[allow(
    clippy::too_many_lines,
    reason = "one flat request dispatch; arms are routing, not logic — splitting it would hide the routing table"
)]
async fn handle_history(connection: &Connection, request: Request) {
    match request {
        Request::AppendAfter {
            event,
            parent,
            reply,
        } => {
            answer(
                reply,
                call_mut(connection, move |c| {
                    queries::append_after(c, &event, parent)
                })
                .await,
            );
        }
        Request::SetActiveLeaf {
            session,
            sequence,
            reply,
        } => {
            answer(
                reply,
                call_mut(connection, move |c| {
                    queries::set_active_leaf(c, session, sequence)
                })
                .await,
            );
        }
        Request::SearchWorkspace { filter, reply } => {
            answer(
                reply,
                call(connection, move |c| {
                    match super::search::search(c, &filter) {
                        Ok(page) => page,
                        // A refusal is not a SQL error, so it is mapped to the
                        // store's own refusal type rather than dressed as one.
                        Err(refusal) => Err(crate::store::sqlite::error::SqlError::Refused {
                            detail: refusal.detail(),
                        }),
                    }
                })
                .await,
            );
        }
        Request::RebuildSearchIndex { reply } => {
            answer(reply, call_mut(connection, super::search::rebuild).await);
        }
        Request::BranchEvents { session, reply } => {
            answer(
                reply,
                call_mut(connection, move |c| queries::branch_events(c, session)).await,
            );
        }
        Request::BranchEventsFrom {
            session,
            from,
            reply,
        } => {
            answer(
                reply,
                call_mut(connection, move |c| {
                    queries::branch_events_from(c, session, from)
                })
                .await,
            );
        }
        Request::SessionTree { session, reply } => {
            answer(
                reply,
                call_mut(connection, move |c| queries::session_tree(c, session)).await,
            );
        }
        Request::BranchSummary {
            session,
            leaf,
            reply,
        } => {
            answer(
                reply,
                call_mut(connection, move |c| {
                    queries::branch_summary(c, session, leaf)
                })
                .await,
            );
        }
        Request::Append { event, reply } => {
            answer(
                reply,
                call_mut(connection, move |c| queries::append(c, &event)).await,
            );
        }
        Request::EventsFrom {
            session,
            from,
            reply,
        } => {
            answer(
                reply,
                call(connection, move |c| queries::events_from(c, session, from)).await,
            );
        }
        Request::WriteCheckpoint { checkpoint, reply } => {
            answer(
                reply,
                call_mut(connection, move |c| {
                    queries::write_checkpoint(c, *checkpoint)
                })
                .await,
            );
        }
        Request::LatestCheckpoint { session, reply } => {
            answer(
                reply,
                call(connection, move |c| queries::latest_checkpoint(c, session)).await,
            );
        }
        Request::FindSessionByDir {
            project_root,
            reply,
        } => {
            answer(
                reply,
                call(connection, move |c| {
                    queries::find_session_by_dir(c, &project_root)
                })
                .await,
            );
        }
        other => unreachable_request(&other),
    }
}

/// Session ownership.
async fn handle_lease(connection: &Connection, request: Request) {
    match request {
        Request::AcquireSession { session, reply } => {
            answer(reply, acquire(connection, session).await);
        }
        Request::ReleaseSession { lease, reply } => {
            answer(
                reply,
                call(connection, move |c| queries::release_session(c, &lease)).await,
            );
        }
        Request::BreakLease { session, reply } => {
            answer(
                reply,
                call(connection, move |c| queries::break_lease(c, session)).await,
            );
        }
        other => unreachable_request(&other),
    }
}

/// A request routed to the wrong group.
///
/// `handle` routes every variant, so this is unreachable. It drops the request —
/// and with it the reply channel, which the caller sees as
/// [`StoreError::Unavailable`] rather than a hang. A panic here would take the
/// store down over a routing typo (`AGENTS.md` §4).
fn unreachable_request(request: &Request) {
    debug_assert!(false, "{request:?} was routed to the wrong handler");
}

/// Take a lease, turning a conflict into the typed refusal.
async fn acquire(connection: &Connection, session: SessionId) -> Result<SessionLease, StoreError> {
    match call_mut(connection, move |c| queries::acquire_session(c, session)).await? {
        queries::LeaseAcquire::Acquired(lease) => Ok(lease),
        queries::LeaseAcquire::Unknown => Err(StoreError::UnknownSession { session }),
        queries::LeaseAcquire::Ended => Err(StoreError::Unavailable {
            detail: format!("session {session} has ended and cannot be leased"),
        }),
        queries::LeaseAcquire::Owned(holder) => Err(StoreError::SessionOwned { session, holder }),
    }
}

fn diagnostics(
    connection: &tokio_rusqlite::rusqlite::Connection,
    database_path: PathBuf,
) -> SqlResult<DiagnosticsReport> {
    let (sessions, events, checkpoints, leased_sessions) = queries::counts(connection)?;
    Ok(DiagnosticsReport {
        database_path,
        schema_version: schema::user_version(connection)?,
        supported_schema_version: schema::SCHEMA_VERSION,
        journal_mode: queries::pragma_text(connection, "journal_mode")?,
        foreign_keys: queries::pragma_number(connection, "foreign_keys")? == 1,
        busy_timeout_ms: u32::try_from(queries::pragma_number(connection, "busy_timeout")?)
            .unwrap_or(0),
        sessions,
        events,
        checkpoints,
        leased_sessions,
        page_count: u64::try_from(queries::pragma_number(connection, "page_count")?).unwrap_or(0),
        page_size: u64::try_from(queries::pragma_number(connection, "page_size")?).unwrap_or(0),
    })
}

/// Run a read-only closure on the connection thread.
async fn call<T, F>(connection: &Connection, function: F) -> Result<T, StoreError>
where
    F: FnOnce(&tokio_rusqlite::rusqlite::Connection) -> SqlResult<T> + Send + 'static,
    T: Send + 'static,
{
    connection
        .call(move |c| function(c))
        .await
        .map_err(SqlError::from)
        .map_err(StoreError::from)
}

/// Run a closure that needs `&mut` for a transaction.
async fn call_mut<T, F>(connection: &Connection, function: F) -> Result<T, StoreError>
where
    F: FnOnce(&mut tokio_rusqlite::rusqlite::Connection) -> SqlResult<T> + Send + 'static,
    T: Send + 'static,
{
    connection
        .call(function)
        .await
        .map_err(SqlError::from)
        .map_err(StoreError::from)
}

/// Send a reply, tolerating a caller that gave up.
///
/// A dropped receiver means the caller stopped waiting — normal at shutdown. The
/// write already happened either way; nothing is lost by the send failing.
fn answer<T>(reply: Reply<T>, result: Result<T, StoreError>) {
    let _ = reply.send(result);
}

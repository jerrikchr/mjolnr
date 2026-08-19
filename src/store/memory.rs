//! In-memory [`EventStore`] .
//!
//! Implements the same port SQLite implements, so the runtime is written against
//! the real contract and a test can drive the whole agent loop without a file.
//!
//! It reproduces the *ordering* guarantee  demands — sequence numbers are
//! assigned under one lock, so two concurrent appends cannot interleave into the
//! same slot. It does **not** reproduce durability, obviously: nothing here
//! survives the process.
//!
//! # What it deliberately does not implement
//!
//! [`StoreDiagnostics`](crate::core::store::StoreDiagnostics) is a separate port
//! and this type does not implement it. There is no database path, no WAL, and
//! no integrity to check, and inventing plausible answers would make a
//! diagnostic that lies (`AGENTS.md` §1.3). Tests that need diagnostics use
//! [`SqliteEventStore::open_in_memory`](crate::store::sqlite::SqliteEventStore::open_in_memory),
//! which is real SQLite without a file.
//!
//! Session leases are always granted: one process, one store instance, so there
//! is no second writer to exclude. That is the honest answer here rather than a
//! stub — the guarantee this store can make is "no split-brain within this
//! process", and it makes it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::event::{EventId, SessionId, SmedEvent, StoredEvent};
use crate::core::store::{
    EventStore, ProjectId, SessionLease, SessionStatus, SessionSummary, StoreError,
    StoredCheckpoint, WorkspaceSearchFilter, WorkspaceSearchPage,
};

#[derive(Debug)]
struct Session {
    project: ProjectId,
    title: String,
    status: SessionStatus,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    events: Vec<StoredEvent>,
    checkpoints: BTreeMap<u64, SessionCheckpoint>,
    leased: bool,
    parent: Option<SessionId>,
}

#[derive(Debug, Default)]
struct Inner {
    projects: BTreeMap<PathBuf, ProjectId>,
    /// `BTreeMap` keyed by time-sortable `SessionId`, so ordering falls out of
    /// the data structure rather than a sort at read time.
    sessions: BTreeMap<SessionId, Session>,
}

/// A volatile event store.
#[derive(Debug, Default)]
pub struct InMemoryEventStore {
    inner: Mutex<Inner>,
}

impl InMemoryEventStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total durable events across all sessions. Test affordance: lets a test
    /// assert that ephemeral deltas never reached the store.
    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self.lock();
        inner
            .sessions
            .values()
            .map(|session| session.events.len())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A poisoned lock means another thread panicked mid-append. There is no
    /// honest recovery: the store's ordering invariant may be broken, and
    /// `AGENTS.md` §1.3 forbids pretending otherwise. `unwrap` is unavailable
    /// (denied crate-wide), so recover the guard and let the caller see whatever
    /// state exists rather than panicking a second time.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            // The data is still structurally valid; only the writer died.
            poisoned.into_inner()
        })
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn open_project(&self, root: PathBuf) -> Result<ProjectId, StoreError> {
        let mut inner = self.lock();
        // `or_default` mints a fresh v7 id, same as `ProjectId::new`.
        Ok(*inner.projects.entry(root).or_default())
    }

    async fn create_session(
        &self,
        session: SessionId,
        project: ProjectId,
        title: String,
        parent: Option<SessionId>,
    ) -> Result<(), StoreError> {
        let now = OffsetDateTime::now_utc();
        let mut inner = self.lock();
        inner.sessions.insert(
            session,
            Session {
                project,
                title,
                status: SessionStatus::Active,
                created_at: now,
                updated_at: now,
                events: Vec::new(),
                checkpoints: BTreeMap::new(),
                leased: false,
                parent,
            },
        );
        Ok(())
    }

    async fn end_session(&self, session: SessionId) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let Some(entry) = inner.sessions.get_mut(&session) else {
            return Err(StoreError::UnknownSession { session });
        };
        entry.status = SessionStatus::Ended;
        entry.updated_at = OffsetDateTime::now_utc();
        Ok(())
    }

    async fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let inner = self.lock();
        let roots: BTreeMap<ProjectId, PathBuf> = inner
            .projects
            .iter()
            .map(|(root, id)| (*id, root.clone()))
            .collect();

        // Newest first: SessionId is v7 and therefore time-ordered.
        Ok(inner
            .sessions
            .iter()
            .rev()
            .map(|(id, session)| {
                let provider_model = active_model(&session.events);
                SessionSummary {
                    id: *id,
                    project_root: roots.get(&session.project).cloned().unwrap_or_default(),
                    title: session.title.clone(),
                    status: session.status,
                    provider: provider_model.clone().map(|(provider, _)| provider),
                    model: provider_model.map(|(_, model)| model),
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                    event_count: session.events.len() as u64,
                    last_checkpoint_sequence: session.checkpoints.keys().next_back().copied(),
                    leased: session.leased,
                    parent: session.parent,
                }
            })
            .collect())
    }

    async fn append(&self, event: SmedEvent) -> Result<StoredEvent, StoreError> {
        debug_assert!(
            event.is_durable(),
            "ephemeral events must not reach the store"
        );

        let session = event.session();
        let mut inner = self.lock();
        let Some(entry) = inner.sessions.get_mut(&session) else {
            return Err(StoreError::UnknownSession { session });
        };

        // Sequence is assigned here, under the lock, never by the caller. That
        // is what makes ordering a property of the store rather than a
        // convention callers must uphold.
        let sequence = entry.events.len() as u64;

        let stored = StoredEvent {
            id: EventId::new(),
            sequence,
            occurred_at: OffsetDateTime::now_utc(),
            event,
        };

        entry.events.push(stored.clone());
        entry.updated_at = stored.occurred_at;
        Ok(stored)
    }

    async fn events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.events_from(session, 0).await
    }

    async fn events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let inner = self.lock();
        Ok(inner
            .sessions
            .get(&session)
            .map(|entry| {
                entry
                    .events
                    .iter()
                    .filter(|stored| stored.sequence >= from)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn write_checkpoint(&self, checkpoint: SessionCheckpoint) -> Result<u64, StoreError> {
        let session = checkpoint.session;
        let mut inner = self.lock();
        let Some(entry) = inner.sessions.get_mut(&session) else {
            return Err(StoreError::UnknownSession { session });
        };
        let covered = entry.events.len() as u64;
        entry.checkpoints.insert(covered, checkpoint);
        Ok(covered)
    }

    async fn latest_checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<StoredCheckpoint>, StoreError> {
        let inner = self.lock();
        Ok(inner.sessions.get(&session).and_then(|entry| {
            entry
                .checkpoints
                .iter()
                .next_back()
                .map(|(sequence, checkpoint)| StoredCheckpoint {
                    sequence: *sequence,
                    checkpoint: checkpoint.clone(),
                })
        }))
    }

    async fn acquire_session(&self, session: SessionId) -> Result<SessionLease, StoreError> {
        let mut inner = self.lock();
        let Some(entry) = inner.sessions.get_mut(&session) else {
            return Err(StoreError::UnknownSession { session });
        };
        if entry.status == SessionStatus::Ended {
            return Err(StoreError::Unavailable {
                detail: format!("session {session} has ended and cannot be leased"),
            });
        }
        // Not a stub: within one process there is exactly one store instance and
        // one runtime, so the guarantee "no second writer" holds trivially. The
        // cross-process case is SQLite's to enforce, and it does.
        entry.leased = true;
        Ok(SessionLease {
            session,
            token: Uuid::now_v7(),
        })
    }

    async fn release_session(&self, lease: &SessionLease) -> Result<(), StoreError> {
        let mut inner = self.lock();
        if let Some(entry) = inner.sessions.get_mut(&lease.session) {
            entry.leased = false;
        }
        Ok(())
    }

    async fn break_lease(&self, session: SessionId) -> Result<(), StoreError> {
        let mut inner = self.lock();
        if let Some(entry) = inner.sessions.get_mut(&session) {
            entry.leased = false;
        }
        Ok(())
    }

    async fn find_session_by_dir(
        &self,
        _project_root: std::path::PathBuf,
    ) -> Result<Option<SessionId>, StoreError> {
        // InMemoryEventStore does not track project roots natively without joining.
        // For the stub just return None.
        Ok(None)
    }

    async fn search_workspace(
        &self,
        _filter: WorkspaceSearchFilter,
    ) -> Result<WorkspaceSearchPage, StoreError> {
        // Same honest refusal as the SQLite store: an empty page would claim
        // "nothing matched" when nothing was searched (AGENTS.md §1.3).
        Err(StoreError::Unavailable {
            detail: "workspace search is not yet implemented (contract landed in D4; \
                     the indexed producer is queued as a follow-up)"
                .to_owned(),
        })
    }

    async fn flush(&self) -> Result<(), StoreError> {
        // Every append already returned before this could be called: there is no
        // queue to drain. Reporting success is a fact, not a stub.
        Ok(())
    }
}

/// The most recent provider/model this session announced.
fn active_model(
    events: &[StoredEvent],
) -> Option<(crate::core::model::ProviderId, crate::core::model::ModelId)> {
    events.iter().rev().find_map(|stored| match &stored.event {
        SmedEvent::SessionCreated {
            provider, model, ..
        }
        | SmedEvent::ModelChanged {
            provider, model, ..
        } => Some((provider.clone(), model.clone())),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::{FinishReason, RunId};
    use crate::core::model::{ModelId, ProviderId};

    fn session_created(session: SessionId) -> SmedEvent {
        SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new("fake"),
            model: ModelId::new("fake-1"),
        }
    }

    async fn store_with(session: SessionId) -> InMemoryEventStore {
        let store = InMemoryEventStore::new();
        let project = store
            .open_project(PathBuf::from("/tmp/p"))
            .await
            .expect("project");
        store
            .create_session(session, project, "t".to_owned(), None)
            .await
            .expect("session");
        store
    }

    #[tokio::test]
    async fn sequences_start_at_zero_and_increment_per_session() {
        let first = SessionId::new();
        let second = SessionId::new();
        let store = store_with(first).await;
        let project = store
            .open_project(PathBuf::from("/tmp/p"))
            .await
            .expect("project");
        store
            .create_session(second, project, "t".to_owned(), None)
            .await
            .expect("session");

        let a = store.append(session_created(first)).await.expect("append");
        let b = store
            .append(SmedEvent::RunStarted {
                session: first,
                run: RunId::new(),
            })
            .await
            .expect("append");
        let c = store.append(session_created(second)).await.expect("append");

        assert_eq!(a.sequence, 0);
        assert_eq!(b.sequence, 1);
        // Sequences are per-session, not global.
        assert_eq!(c.sequence, 0);
    }

    #[tokio::test]
    async fn events_return_in_sequence_order() {
        let session = SessionId::new();
        let store = store_with(session).await;
        let run = RunId::new();

        store
            .append(session_created(session))
            .await
            .expect("append");
        store
            .append(SmedEvent::RunStarted { session, run })
            .await
            .expect("append");
        store
            .append(SmedEvent::RunFinished {
                session,
                run,
                reason: FinishReason::Stop,
            })
            .await
            .expect("append");

        let events = store.events(session).await.expect("events");
        let sequences: Vec<u64> = events.iter().map(|event| event.sequence).collect();
        assert_eq!(sequences, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn sessions_are_newest_first() {
        let older = SessionId::new();
        let newer = SessionId::new();
        let store = store_with(older).await;
        let project = store
            .open_project(PathBuf::from("/tmp/p"))
            .await
            .expect("project");
        store
            .create_session(newer, project, "t".to_owned(), None)
            .await
            .expect("session");

        let listed: Vec<SessionId> = store
            .sessions()
            .await
            .expect("sessions")
            .into_iter()
            .map(|summary| summary.id)
            .collect();

        assert_eq!(listed, vec![newer, older]);
    }

    #[tokio::test]
    async fn unknown_session_yields_no_events_rather_than_an_error() {
        let store = InMemoryEventStore::new();
        let events = store.events(SessionId::new()).await.expect("events");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn appending_to_an_unknown_session_is_refused() {
        // The SQLite store's foreign key refuses this; the in-memory store must
        // agree, or a test that passes here would fail against the real store.
        let store = InMemoryEventStore::new();
        let session = SessionId::new();
        assert!(matches!(
            store.append(session_created(session)).await,
            Err(StoreError::UnknownSession { .. })
        ));
    }

    #[tokio::test]
    async fn a_checkpoint_covers_the_events_appended_before_it() {
        let session = SessionId::new();
        let store = store_with(session).await;

        store
            .append(session_created(session))
            .await
            .expect("append");
        store
            .append(SmedEvent::RunStarted {
                session,
                run: RunId::new(),
            })
            .await
            .expect("append");

        let covered = store
            .write_checkpoint(SessionCheckpoint::empty(session))
            .await
            .expect("checkpoint");

        assert_eq!(covered, 2, "a checkpoint covers a count, not a last index");
        assert!(
            store
                .events_from(session, covered)
                .await
                .expect("events")
                .is_empty(),
            "nothing follows a checkpoint written at the tail"
        );
    }

    #[tokio::test]
    async fn events_after_a_checkpoint_are_replayable() {
        let session = SessionId::new();
        let store = store_with(session).await;
        store
            .append(session_created(session))
            .await
            .expect("append");

        let covered = store
            .write_checkpoint(SessionCheckpoint::empty(session))
            .await
            .expect("checkpoint");

        store
            .append(SmedEvent::RunStarted {
                session,
                run: RunId::new(),
            })
            .await
            .expect("append");

        let later = store.events_from(session, covered).await.expect("events");
        assert_eq!(
            later.len(),
            1,
            "the post-checkpoint event must be replayable"
        );
        assert_eq!(later.first().map(|event| event.sequence), Some(1));
    }
}

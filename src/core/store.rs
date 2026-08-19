//! The durable-store port.
//!
//! Phase 1 implemented this in memory; Phase 4 implements it over SQLite. The
//! port is defined here, in `core`, so both are interchangeable and the runtime
//! never learns which one it has.
//!
//! # Two ports, not one
//!
//! [`EventStore`] is the durability contract the runtime depends on. It changes
//! when the persistence model changes.
//!
//! [`StoreDiagnostics`] is an operator surface: database path, schema version,
//! WAL state, integrity. It is separate because it changes for a different
//! reason (what an operator needs to see), and because it is *meaningless* for a
//! volatile store — `InMemoryEventStore` has no path, no WAL, and no integrity
//! to check. Folding the two together would force it to invent answers, and an
//! invented diagnostic is worse than an absent one (`AGENTS.md` §1.3).

use std::path::PathBuf;

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::continuation::CommandFact;
use crate::core::event::{EventId, SessionId, SmedEvent, StoredEvent};
use crate::core::model::{ModelId, ProviderId};

/// Failures from a store implementation.
///
/// Every variant names a distinct thing that went wrong, because the runtime's
/// response differs: a sequence gap means history is untrustworthy and the
/// session must not continue, while a busy database means try again.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store unavailable: {detail}")]
    Unavailable { detail: String },

    #[error("event sequence conflict for session {session}: expected {expected}, found {found}")]
    SequenceConflict {
        session: SessionId,
        expected: u64,
        found: u64,
    },

    /// A sequence number is missing from stored history.
    ///
    /// Refused rather than papered over: a gap means an event was lost, and
    /// presenting the remainder as a complete transcript would be a lie about
    /// state (`AGENTS.md` §1.3).
    #[error("session {session} is missing event sequence {missing}; stored history is incomplete")]
    SequenceGap { session: SessionId, missing: u64 },

    /// Two events claim one identity. `finish_task` cites event ids as evidence,
    /// so an ambiguous id makes evidence meaningless.
    #[error("duplicate event id {id}")]
    DuplicateEvent { id: EventId },

    /// The database was written by a newer smed.
    ///
    /// Refused, never best-effort: a build that silently ignores columns it does
    /// not understand will drop data on its next write.
    #[error(
        "database schema version {found} is newer than this build supports ({supported}); \
         upgrade smed rather than risking a partial read"
    )]
    UnsupportedSchema { found: u32, supported: u32 },

    /// Another process holds the session's write lease.
    #[error("session {session} is already open in another smed process ({holder})")]
    SessionOwned { session: SessionId, holder: String },

    #[error("unknown session {session}")]
    UnknownSession { session: SessionId },

    /// A stored payload could not be decoded into a canonical value.
    #[error("stored data could not be decoded: {detail}")]
    Decode { detail: String },

    /// The store understood the request and declined to answer it.
    ///
    /// Distinct from [`Unavailable`](Self::Unavailable), which means the store
    /// itself is not working. This one means the store is fine and the question
    /// was unanswerable — a search query too short for the index to match, a
    /// pagination cursor issued for a different filter. A caller retries the
    /// first and corrects the second, so collapsing them into one variant would
    /// send half of them to the wrong remedy (AGENTS.md §6).
    #[error("refused: {detail}")]
    Refused { detail: String },
}

/// Identifies a project root in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(uuid::Uuid);

impl ProjectId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Whether a session accepts new work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Ended,
}

impl SessionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Ended => "ended",
        }
    }

    /// Parse a stored status.
    ///
    /// An unrecognised status resolves to [`Ended`](Self::Ended), not
    /// [`Active`](Self::Active): fail closed (`AGENTS.md` §1.2). A row this
    /// build cannot interpret must not be handed to a model as a live session.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            _ => Self::Ended,
        }
    }
}

/// What `smed sessions list` shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub project_root: PathBuf,
    pub title: String,
    pub status: SessionStatus,
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    /// Number of durable events, which is also the next sequence to assign.
    pub event_count: u64,
    /// The event count covered by the newest checkpoint, if there is one.
    pub last_checkpoint_sequence: Option<u64>,
    /// Whether some process currently holds the write lease. A stale `true`
    /// after a crash is expected and is not a bug — see `docs/persistence.md` §5.
    pub leased: bool,
    /// The session that spawned this one, for subagent sessions.
    pub parent: Option<SessionId>,
}

/// A checkpoint as stored, with the extent of history it summarises.
///
/// # Why a count and not "the last sequence"
///
/// `sequence` is the **number of events folded into this checkpoint**, i.e. an
/// exclusive upper bound: a checkpoint with `sequence: 5` covers events 0..=4,
/// and recovery replays everything from 5 onward.
///
/// The obvious alternative — "the sequence of the last included event" — cannot
/// represent a session with no events. Zero would mean both "nothing is covered"
/// and "event 0 is covered", and the two differ by exactly one replayed event,
/// which for a mutation is the difference between recovering it and losing it.
/// Counts have no such collision.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredCheckpoint {
    pub sequence: u64,
    pub checkpoint: SessionCheckpoint,
}

/// Proof that this process holds a session's write lease.
///
/// Returned by [`EventStore::acquire_session`] and required to be alive for the
/// duration of writes. It is a value rather than a bare `Ok(())` so that
/// "acquired the lease" and "forgot to acquire the lease" are different types at
/// the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLease {
    pub session: SessionId,
    pub token: uuid::Uuid,
}

/// Everything a resume needs from the branch it is resuming.
///
/// The two halves answer the same question about one branch, and are read
/// together for that reason: as separate calls, a caller could pair a suffix
/// from one branch with anchors from another and never find out.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchResume {
    /// The sequences of the message-bearing events the checkpoint already
    /// covers, in transcript order.
    ///
    /// A checkpoint stores messages but not the events that produced them, so
    /// these are what re-anchor its transcript to the record. Empty means the
    /// store cannot supply them, and the restored messages carry no branch
    /// points rather than invented ones.
    pub covered_message_sequences: Vec<u64>,
    /// The branch's events the checkpoint does not cover, in sequence order.
    pub events: Vec<StoredEvent>,
}

/// One user turn as a node in the session tree.
///
/// The tree `/tree` navigates is a tree of *turns*, not of events: the events
/// between two user messages are that turn's, and a person choosing where to
/// branch is choosing a thing they said. `parent` therefore skips to the
/// nearest ancestor turn rather than naming the immediately preceding event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeNode {
    /// The event to rewind to in order to branch from this turn.
    pub sequence: u64,
    /// The turn this one followed, or `None` for the first turn on the session.
    pub parent: Option<u64>,
    pub prompt: String,
    /// The reply this turn received, if it got one before the branch ended.
    pub answer: Option<String>,
    /// Whether this turn is on the branch the session is currently following.
    ///
    /// The overlay renders abandoned turns differently and selecting one means
    /// something different — returning to that branch rather than leaving the
    /// current one — so the distinction travels with the node instead of being
    /// re-derived by every reader.
    pub on_active_branch: bool,
}

/// What happened on a branch, assembled from the record.
///
/// # Why this is a projection and not a summary a model wrote
///
/// *(Deliberate divergence from Pi, which asks a model to summarise a branch
/// when you switch away from it.)*
///
/// smed will not spend the user's tokens on a step they did not ask for, and
/// a projection of the record cannot hallucinate what the branch did. Every
/// field here is read off events that were already written: the message that
/// started the branch, the files it touched, the commands it ran and how they
/// ended. Nothing here is generated, so nothing here can be wrong in the way a
/// written summary can be wrong.
///
/// This is compaction-as-projection applied to branching.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BranchSummary {
    /// The message that started this branch — the first turn after the point it
    /// diverged from its sibling.
    pub origin: Option<String>,
    /// User turns on the diverged segment.
    pub turns: usize,
    pub files_read: Vec<PathBuf>,
    pub files_changed: Vec<PathBuf>,
    pub commands: Vec<CommandFact>,
    /// Tool calls that ended in a refusal or an error.
    pub tool_failures: usize,
}

impl BranchSummary {
    /// Whether anything actually happened on the branch.
    ///
    /// An empty summary is reported as nothing rather than as an empty report:
    /// "you left a branch where nothing happened" is noise.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.origin.is_none()
            && self.turns == 0
            && self.files_read.is_empty()
            && self.files_changed.is_empty()
            && self.commands.is_empty()
            && self.tool_failures == 0
    }
}

/// Append-only event storage plus session durability.
///
/// Ordering is the whole contract.  requires writes to route through one
/// bounded actor to guarantee it, which is why this port has no batch-write or
/// out-of-order API to misuse.
#[async_trait]
pub trait EventStore: Send + Sync + std::fmt::Debug {
    /// Register a canonical project root, or return the existing id for it.
    async fn open_project(&self, root: PathBuf) -> Result<ProjectId, StoreError>;

    /// Create a durable session row.
    ///
    /// Called before the first append: `events.session_id` references
    /// `sessions.id`, and foreign keys are enforced (`docs/persistence.md` §2.1).
    ///
    /// `parent` links a subagent session to the session that spawned it (plan
    /// §Phase 13). `None` for every session a human opens directly.
    async fn create_session(
        &self,
        session: SessionId,
        project: ProjectId,
        title: String,
        parent: Option<SessionId>,
    ) -> Result<(), StoreError>;

    /// Mark a session as accepting no further work.
    async fn end_session(&self, session: SessionId) -> Result<(), StoreError>;

    /// Sessions known to the store, newest first.
    async fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError>;

    /// Append a durable event, assigning it the next sequence number.
    ///
    /// Callers must not pass ephemeral events; [`SmedEvent::is_durable`]
    /// decides. Implementations may assume the caller checked.
    async fn append(&self, event: SmedEvent) -> Result<StoredEvent, StoreError>;

    /// Every event for a session, in sequence order.
    ///
    /// Implementations must refuse with [`StoreError::SequenceGap`] rather than
    /// return a history with a hole in it.
    async fn events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError>;

    /// Append a durable event whose parent is `parent` rather than the
    /// preceding sequence.
    ///
    /// This is the one operation that creates a branch: the new event takes the
    /// next sequence number like any other, but records that it *followed*
    /// something earlier, leaving the events in between on a sibling branch
    /// that stays readable. The default implementation appends linearly, so a
    /// store with no tree support degrades to Phase 15 behaviour rather than
    /// silently losing the branch point.
    async fn append_after(
        &self,
        event: SmedEvent,
        parent: Option<u64>,
    ) -> Result<StoredEvent, StoreError> {
        let _ = parent;
        self.append(event).await
    }

    /// Move the session's active leaf, so subsequent reads follow the branch
    /// ending at `sequence` .
    ///
    /// `None` restores "the highest sequence", which is where a linear session
    /// always sits.
    async fn set_active_leaf(
        &self,
        session: SessionId,
        sequence: Option<u64>,
    ) -> Result<(), StoreError> {
        let _ = (session, sequence);
        Ok(())
    }

    /// The events on the session's active branch, oldest first.
    ///
    /// Walks parent pointers back from the active leaf, so events on abandoned
    /// siblings are retained in the store but absent from what the session
    /// replays. Defaults to the full linear history, which is the same answer
    /// for any session that has never branched.
    async fn branch_events(&self, session: SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.events(session).await
    }

    /// The active branch's events at or after `from`, or `None` when `from` is
    /// not on the branch.
    ///
    /// This is the branch-aware recovery read. `from` is a checkpoint's covered
    /// count, and the checkpoint's transcript is only a valid base to replay
    /// onto if every sequence it covers lies on the branch being resumed. After
    /// a rewind that can be false, and replaying the branch's suffix onto a
    /// sibling's checkpoint would resurrect exactly the messages the user
    /// branched away from. `None` says so rather than guessing, leaving the
    /// caller to replay the whole branch instead.
    ///
    /// The default implementation always answers `Some`, because a store with
    /// no tree support has one branch and every checkpoint is on it.
    async fn branch_events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Option<BranchResume>, StoreError> {
        // A store with no tree support has one branch, so every checkpoint is
        // on it and the answer is always `Some`.
        //
        // Deriving the anchors here reads the history the checkpoint exists to
        // avoid reading. That is the right trade for a store with no tree — it
        // is either in memory already or small enough not to have branched —
        // and a store that cares overrides this with a query that reads
        // sequences without decoding payloads.
        let covered_message_sequences = self
            .events(session)
            .await?
            .iter()
            .filter(|stored| stored.sequence < from && stored.event.introduces_message())
            .map(|stored| stored.sequence)
            .collect();
        let events = self.events_from(session, from).await?;
        Ok(Some(BranchResume {
            covered_message_sequences,
            events,
        }))
    }

    /// Events with a sequence at or after `from`, in order.
    ///
    /// This is the linear read. Recovery uses
    /// [`branch_events_from`](Self::branch_events_from) instead, which is this
    /// restricted to the branch actually being resumed.
    async fn events_from(
        &self,
        session: SessionId,
        from: u64,
    ) -> Result<Vec<StoredEvent>, StoreError>;

    /// Every user turn in the session, as a tree.
    ///
    /// Includes turns on abandoned branches: that is the whole point of the
    /// read, since a branch nobody can see is a branch nobody can go back to.
    /// Ordered by sequence, so a reader can build the tree in one pass.
    ///
    /// Defaults to empty rather than to the linear history. A store with no
    /// tree support has nothing to say about branches, and returning the active
    /// branch dressed up as a tree would tell the user their history had no
    /// siblings — a claim it cannot make.
    async fn session_tree(&self, session: SessionId) -> Result<Vec<SessionTreeNode>, StoreError> {
        let _ = session;
        Ok(Vec::new())
    }

    /// What happened on the branch ending at `leaf`, since it diverged
    /// .
    ///
    /// Restricted to the *diverged* segment — everything after the nearest
    /// ancestor with more than one child. The shared prefix is not news: it is
    /// on the branch being kept too, and reporting it would bury the difference
    /// in history the user never left.
    ///
    /// Defaults to empty, for the same reason `session_tree` does: a store with
    /// no branches has no branch to summarise.
    async fn branch_summary(
        &self,
        session: SessionId,
        leaf: u64,
    ) -> Result<BranchSummary, StoreError> {
        let _ = (session, leaf);
        Ok(BranchSummary::default())
    }

    /// Record a checkpoint covering every event appended so far.
    ///
    /// Returns the event count it covers — the `from` a later recovery replays
    /// against.
    async fn write_checkpoint(&self, checkpoint: SessionCheckpoint) -> Result<u64, StoreError>;

    /// Find the session ID matching the exact directory path.
    async fn find_session_by_dir(
        &self,
        project_root: PathBuf,
    ) -> Result<Option<SessionId>, StoreError>;

    /// Search across the workspace index.
    async fn search_workspace(
        &self,
        filter: WorkspaceSearchFilter,
    ) -> Result<WorkspaceSearchPage, StoreError>;

    /// The newest checkpoint for a session, if any.
    async fn latest_checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<StoredCheckpoint>, StoreError>;

    /// Take the session's write lease.
    ///
    /// Fails with [`StoreError::SessionOwned`] when another process holds it.
    /// Ended sessions also refuse: acquiring a lease must never resurrect one.
    /// SQLite serialises writers to the *file*; nothing in SQLite prevents two
    /// smed processes interleaving two runs into one *session*, which is what
    /// this prevents (`docs/persistence.md` §5).
    async fn acquire_session(&self, session: SessionId) -> Result<SessionLease, StoreError>;

    /// Release a lease this process holds.
    ///
    /// Releasing a lease held by someone else is a no-op rather than an error:
    /// the caller's intent ("I am done") is satisfied either way.
    async fn release_session(&self, lease: &SessionLease) -> Result<(), StoreError>;

    /// Forcibly clear a lease, whoever holds it.
    ///
    /// The explicit human act behind `smed sessions release`. smed never does
    /// this on its own: it cannot prove the holder is dead.
    async fn break_lease(&self, session: SessionId) -> Result<(), StoreError>;

    /// Wait until every write accepted before this call is durable.
    ///
    /// The shutdown acknowledgement. Returning `Ok` means the data is committed,
    /// not merely queued — otherwise "clean shutdown" would be a claim the store
    /// cannot support (`AGENTS.md` §1.3).
    async fn flush(&self) -> Result<(), StoreError>;
}

/// What an operator can ask the durable store about itself.
#[async_trait]
pub trait StoreDiagnostics: Send + Sync + std::fmt::Debug {
    async fn report(&self) -> Result<DiagnosticsReport, StoreError>;

    /// Run `PRAGMA integrity_check`.
    ///
    /// Explicit and never on startup: it is O(N log N) over the database (plan
    /// §9, `docs/persistence.md` §2.4).
    async fn integrity_check(&self) -> Result<IntegrityReport, StoreError>;
}

/// A filter for deterministic workspace search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct WorkspaceSearchFilter {
    pub query: Option<String>,
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub work_kind: Option<String>,
    pub event_kind: Option<String>,
    pub status: Option<SessionStatus>,
    pub provider_model: Option<String>,
    pub reason_code: Option<String>,
    pub file_path: Option<String>,
    pub time_start: Option<OffsetDateTime>,
    pub time_end: Option<OffsetDateTime>,
    pub cursor: Option<String>,
    pub limit: u32,
}

/// A single result from a workspace search.
///
/// `match_snippet` is bounded by
/// [`crate::core::client::workspace::MAX_SEARCH_SNIPPET_BYTES`] at the
/// producer's projection boundary (enforced when the producer lands; the
/// constant already exists so the obligation is named, not implied).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceSearchResult {
    pub session_id: SessionId,
    pub event_id: EventId,
    pub sequence: u64,
    pub match_snippet: String,
    pub occurred_at: OffsetDateTime,
}

/// A paginated result set for workspace search.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceSearchPage {
    pub items: Vec<WorkspaceSearchResult>,
    pub next_cursor: Option<String>,
}

/// The state of the durable store, for `smed diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsReport {
    pub database_path: PathBuf,
    pub schema_version: u32,
    pub supported_schema_version: u32,
    /// Expected to be `wal`. Reported verbatim rather than as a bool so a
    /// database that silently failed to convert is visible.
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub busy_timeout_ms: u32,
    pub sessions: u64,
    pub events: u64,
    pub checkpoints: u64,
    pub leased_sessions: u64,
    /// Bytes on disk, main database file only.
    pub page_count: u64,
    pub page_size: u64,
}

/// The result of an explicit integrity check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityReport {
    Ok,
    /// SQLite reported problems. Carried verbatim: paraphrasing a corruption
    /// report loses the only detail that makes it actionable.
    Problems(Vec<String>),
}

impl IntegrityReport {
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_session_status_fails_closed_to_ended() {
        assert_eq!(SessionStatus::parse("active"), SessionStatus::Active);
        assert_eq!(SessionStatus::parse("ended"), SessionStatus::Ended);
        // A status written by a newer smed must not resurrect as live work.
        assert_eq!(SessionStatus::parse("archived"), SessionStatus::Ended);
        assert_eq!(SessionStatus::parse(""), SessionStatus::Ended);
    }

    #[test]
    fn status_round_trips_through_its_wire_form() {
        for status in [SessionStatus::Active, SessionStatus::Ended] {
            assert_eq!(SessionStatus::parse(status.as_str()), status);
        }
    }

    #[test]
    fn integrity_problems_are_not_ok() {
        assert!(IntegrityReport::Ok.is_ok());
        assert!(!IntegrityReport::Problems(vec!["row 3 missing".to_owned()]).is_ok());
    }
}

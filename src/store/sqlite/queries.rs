//! The SQL. One reason to change: how mjolnr's data is read and written.
//!
//! Every function here is ordinary blocking `rusqlite` and runs on
//! `tokio-rusqlite`'s connection thread, never on a Tokio worker
//! (`docs/persistence.md` §1.2). They take `&Connection` rather than owning one
//! so the actor stays the only thing that decides *when* they run.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_rusqlite::rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::continuation::CommandFact;
use crate::core::event::{EventId, MjolnrEvent, SessionId, StoredEvent};
use crate::core::model::{ModelId, ProviderId};
use crate::core::store::{BranchResume, BranchSummary, SessionTreeNode};
use crate::core::store::{
    IntegrityReport, ProjectId, SessionLease, SessionStatus, SessionSummary, StoredCheckpoint,
};
use crate::store::sqlite::error::SqlResult;
use crate::store::sqlite::schema;
use crate::store::wire;

/// Register a project root, or return the id it already has.
///
/// Upsert rather than select-then-insert: two mjolnr processes opening the same
/// project at once would race the gap between the two statements, and the
/// `UNIQUE` constraint would surface as a spurious error rather than the shared
/// id both callers want.
pub(super) fn open_project(connection: &Connection, root: &Path) -> SqlResult<ProjectId> {
    let now = timestamp(OffsetDateTime::now_utc())?;
    let realpath =
        root.to_str().ok_or_else(
            || crate::store::sqlite::error::SqlError::InvalidProjectPath {
                detail: "workspace roots must be valid UTF-8 to preserve their identity".to_owned(),
            },
        )?;
    let id = ProjectId::new().to_string();

    let stored: String = connection.query_row(
        "INSERT INTO projects (id, root_realpath, created_at, last_opened_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(root_realpath) DO UPDATE SET last_opened_at = ?3
         RETURNING id",
        params![id, realpath, now],
        |row| row.get(0),
    )?;

    Ok(ProjectId::from_uuid(parse_uuid(&stored)?))
}

pub(super) fn create_session(
    connection: &Connection,
    session: SessionId,
    project: ProjectId,
    title: &str,
    parent: Option<SessionId>,
) -> SqlResult<()> {
    let now = timestamp(OffsetDateTime::now_utc())?;
    connection.execute(
        "INSERT INTO sessions (id, project_id, title, status, created_at, updated_at, last_sequence, parent_session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, ?6)",
        params![
            session.to_string(),
            project.to_string(),
            title,
            SessionStatus::Active.as_str(),
            now,
            parent.map(|parent| parent.to_string()),
        ],
    )?;
    Ok(())
}

pub(super) fn end_session(connection: &Connection, session: SessionId) -> SqlResult<bool> {
    let affected = connection.execute(
        "UPDATE sessions SET status = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            session.to_string(),
            SessionStatus::Ended.as_str(),
            timestamp(OffsetDateTime::now_utc())?
        ],
    )?;
    Ok(affected == 1)
}

pub(super) fn rename_session(
    connection: &Connection,
    session: SessionId,
    title: &str,
) -> SqlResult<bool> {
    let affected = connection.execute(
        "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            session.to_string(),
            title,
            timestamp(OffsetDateTime::now_utc())?
        ],
    )?;
    Ok(affected == 1)
}

/// Every session, newest first.
pub(super) fn sessions(connection: &Connection) -> SqlResult<Vec<SessionSummary>> {
    let mut statement = connection.prepare(
        "SELECT s.id, p.root_realpath, s.title, s.status, s.active_provider, s.active_model,
                s.created_at, s.updated_at, s.last_sequence, s.last_checkpoint_sequence,
                (SELECT COUNT(*) FROM session_owners o WHERE o.session_id = s.id),
                s.parent_session_id
         FROM sessions s
         JOIN projects p ON p.id = s.project_id
         ORDER BY julianday(s.created_at) DESC, s.id DESC",
    )?;

    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, Option<i64>>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, Option<String>>(11)?,
        ))
    })?;

    let mut summaries = Vec::new();
    for row in rows {
        let row = row?;
        summaries.push(SessionSummary {
            id: SessionId::from_uuid(parse_uuid(&row.0)?),
            project_root: PathBuf::from(row.1),
            title: row.2,
            status: SessionStatus::parse(&row.3),
            provider: row.4.map(ProviderId::new),
            model: row.5.map(ModelId::new),
            created_at: parse_timestamp(&row.6)?,
            updated_at: parse_timestamp(&row.7)?,
            event_count: count(row.8),
            last_checkpoint_sequence: row.9.map(count),
            leased: row.10 > 0,
            parent: row
                .11
                .map(|parent| parse_uuid(&parent).map(SessionId::from_uuid))
                .transpose()?,
        });
    }
    Ok(summaries)
}

/// Append one durable event and advance the session's counters, atomically.
///
/// The sequence is derived from `MAX(sequence) + 1` **inside the transaction**
/// rather than read from `sessions.last_sequence`. The column is a mirror kept
/// for listing; deriving from the events themselves means the two can never
/// drift into assigning a slot twice, and the `PRIMARY KEY (session_id,
/// sequence)` would catch it if they somehow did.
pub(super) fn append(connection: &mut Connection, event: &MjolnrEvent) -> SqlResult<StoredEvent> {
    append_after(connection, event, None)
}

/// Append `event`, optionally recording an explicit parent sequence.
///
/// `parent: None` writes NULL, which every pre-Phase-16.5 event also carries
/// and which means "the preceding sequence". A `Some` value is a branch point:
/// the event takes the next sequence like any other, but declares that it
/// followed something earlier.
pub(super) fn append_after(
    connection: &mut Connection,
    event: &MjolnrEvent,
    parent: Option<u64>,
) -> SqlResult<StoredEvent> {
    let payload = wire::encode(event.clone())?;
    let json = wire::encode_json(&payload)?;
    let kind = payload.kind();

    let session = event.session();
    let id = EventId::new();
    let occurred_at = OffsetDateTime::now_utc();
    let occurred_text = timestamp(occurred_at)?;

    let transaction = connection.transaction()?;

    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence) + 1, 0) FROM events WHERE session_id = ?1",
        params![session.to_string()],
        |row| row.get(0),
    )?;

    let parent_sequence: Option<i64> = parent.map(|value| i64::try_from(value).unwrap_or(i64::MAX));
    transaction.execute(
        "INSERT INTO events (session_id, sequence, event_id, kind, occurred_at, schema_version, payload_json, parent_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session.to_string(),
            sequence,
            id.to_string(),
            kind,
            occurred_text,
            wire::WIRE_VERSION,
            json.clone(),
            parent_sequence
        ],
    )?;

    // A branch point moves the session's leaf with it, so the next read
    // follows the new branch rather than the one just departed.
    if parent_sequence.is_some() {
        transaction.execute(
            "UPDATE sessions SET active_leaf_sequence = ?2 WHERE id = ?1",
            params![session.to_string(), sequence],
        )?;
    }

    // `last_sequence` holds the event *count*, which is also the next sequence
    // to assign. See `StoredCheckpoint` for why counts rather than last-indices.
    transaction.execute(
        "UPDATE sessions SET last_sequence = ?2, updated_at = ?3 WHERE id = ?1",
        params![session.to_string(), sequence + 1, occurred_text],
    )?;

    // The session row mirrors the active model so `sessions list` can show it
    // without replaying history.
    match event {
        MjolnrEvent::SessionCreated {
            provider, model, ..
        }
        | MjolnrEvent::ModelChanged {
            provider, model, ..
        } => {
            transaction.execute(
                "UPDATE sessions SET active_provider = ?2, active_model = ?3 WHERE id = ?1",
                params![session.to_string(), provider.as_str(), model.as_str()],
            )?;
        }
        _ => {}
    }

    // Indexed inside the append transaction, not after it. An indexed row that
    // outlived a rolled-back append would be a search hit for an event that
    // does not exist — and search would be the only place anyone ever saw it.
    super::search::index(&transaction, event, &id)?;

    transaction.commit()?;

    Ok(StoredEvent {
        id,
        sequence: count(sequence),
        occurred_at,
        event: event.clone(),
    })
}

/// Events from `from` onward, refusing a history with a hole in it.
pub(super) fn events_from(
    connection: &Connection,
    session: SessionId,
    from: u64,
) -> SqlResult<Vec<StoredEvent>> {
    let mut statement = connection.prepare(
        "SELECT e.sequence, e.event_id, e.occurred_at, e.schema_version, e.payload_json,
                s.last_sequence
         FROM sessions s
         LEFT JOIN events e ON e.session_id = s.id AND e.sequence >= ?2
         WHERE s.id = ?1
         ORDER BY e.sequence ASC",
    )?;

    let rows = statement.query_map(
        params![session.to_string(), i64::try_from(from).unwrap_or(i64::MAX)],
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;

    let mut events = Vec::new();
    let mut expected = from;
    let mut durable_count = None;
    for row in rows {
        let (sequence, event_id, occurred_at, version, json, stored_extent) = row?;
        durable_count = Some(stored_count(stored_extent, "sessions.last_sequence")?);

        let Some(sequence) = sequence else {
            continue;
        };
        let (Some(event_id), Some(occurred_at), Some(version), Some(json)) =
            (event_id, occurred_at, version, json)
        else {
            return Err(crate::store::sqlite::error::SqlError::Decode {
                detail: format!("session {session} has a partially null event row"),
            });
        };
        let sequence = stored_count(sequence, "events.sequence")?;

        // A gap means an event was lost. Returning the remainder would present
        // an incomplete transcript as a complete one (AGENTS.md §1.3).
        if sequence != expected {
            return Err(crate::store::sqlite::error::SqlError::Gap {
                session,
                missing: expected,
            });
        }
        expected = sequence.saturating_add(1);

        let payload = wire::decode_json(&json, version_of(version))?;
        events.push(StoredEvent {
            id: EventId::from_uuid(parse_uuid(&event_id)?),
            sequence,
            occurred_at: parse_timestamp(&occurred_at)?,
            event: wire::decode(session, payload),
        });
    }

    // An unknown session has no joined row and retains the store's established
    // empty-history behaviour. A known empty session produces the LEFT JOIN's
    // null event row with a zero durable count.
    let durable_count = durable_count.unwrap_or(0);

    // A missing final row has no later sequence to expose it inside the loop.
    // `last_sequence` is the durable event count updated in the same
    // transaction as every append, so it is the terminal boundary the read
    // must reach. Without this check, deleting the final event would make an
    // incomplete history look complete.
    if expected < durable_count {
        return Err(crate::store::sqlite::error::SqlError::Gap {
            session,
            missing: expected,
        });
    }
    if expected > durable_count {
        return Err(crate::store::sqlite::error::SqlError::Decode {
            detail: format!(
                "session {session} contains {expected} sequenced events but records durable extent {durable_count}"
            ),
        });
    }

    Ok(events)
}

/// Record a checkpoint covering every event appended so far.
pub(super) fn write_checkpoint(
    connection: &mut Connection,
    checkpoint: SessionCheckpoint,
) -> SqlResult<u64> {
    let session = checkpoint.session;
    let json = wire::encode_checkpoint(checkpoint)?;
    let now = timestamp(OffsetDateTime::now_utc())?;

    let transaction = connection.transaction()?;

    let covered: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence) + 1, 0) FROM events WHERE session_id = ?1",
        params![session.to_string()],
        |row| row.get(0),
    )?;

    // A second checkpoint at the same extent replaces the first: it is a
    // projection of the same events, so keeping both would only grow the file.
    transaction.execute(
        "INSERT INTO checkpoints (session_id, sequence, schema_version, state_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id, sequence) DO UPDATE SET
           schema_version = ?3, state_json = ?4, created_at = ?5",
        params![session.to_string(), covered, wire::WIRE_VERSION, json, now],
    )?;

    transaction.execute(
        "UPDATE sessions SET last_checkpoint_sequence = ?2, updated_at = ?3 WHERE id = ?1",
        params![session.to_string(), covered, now],
    )?;

    transaction.commit()?;
    Ok(count(covered))
}

pub(super) fn latest_checkpoint(
    connection: &Connection,
    session: SessionId,
) -> SqlResult<Option<StoredCheckpoint>> {
    let row: Option<(i64, i64, String, i64, i64, i64)> = connection
        .query_row(
            "SELECT c.sequence, c.schema_version, c.state_json, s.last_sequence,
                    (SELECT COUNT(*) FROM events e WHERE e.session_id = c.session_id),
                    (SELECT COUNT(*) FROM events e
                     WHERE e.session_id = c.session_id
                       AND e.sequence >= 0 AND e.sequence < c.sequence)
             FROM checkpoints c
             JOIN sessions s ON s.id = c.session_id
             WHERE c.session_id = ?1
             ORDER BY c.sequence DESC LIMIT 1",
            params![session.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;

    let Some((sequence, version, json, durable_extent, event_count, covered_count)) = row else {
        return Ok(None);
    };

    let sequence = stored_count(sequence, "checkpoints.sequence")?;
    let durable_extent = stored_count(durable_extent, "sessions.last_sequence")?;
    let event_count = stored_count(event_count, "events count")?;
    let covered_count = stored_count(covered_count, "checkpoint covered-event count")?;

    if durable_extent != event_count || sequence > durable_extent || covered_count != sequence {
        return Err(crate::store::sqlite::error::SqlError::Decode {
            detail: format!(
                "checkpoint for session {session} has extent {sequence}, but durable history has extent {durable_extent}, {event_count} rows, and {covered_count} covered rows"
            ),
        });
    }

    let checkpoint = wire::decode_checkpoint(&json, version_of(version))?;
    if checkpoint.session != session {
        return Err(crate::store::sqlite::error::SqlError::Decode {
            detail: format!(
                "checkpoint row belongs to session {session}, but its payload belongs to {}",
                checkpoint.session
            ),
        });
    }

    Ok(Some(StoredCheckpoint {
        sequence,
        checkpoint,
    }))
}

/// Take a session's write lease.
///
/// The `PRIMARY KEY` on `session_owners.session_id` makes this atomic: the
/// second process's insert conflicts, and it learns who holds the lease rather
/// than joining as a second writer.
pub(super) enum LeaseAcquire {
    Acquired(SessionLease),
    Unknown,
    Ended,
    Owned(String),
}

pub(super) fn acquire_session(
    connection: &mut Connection,
    session: SessionId,
) -> SqlResult<LeaseAcquire> {
    let transaction = connection.transaction()?;
    let token = Uuid::now_v7();
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO session_owners (session_id, owner_token, process_id, acquired_at)
         SELECT ?1, ?2, ?3, ?4
         FROM sessions
         WHERE id = ?1 AND status = ?5",
        params![
            session.to_string(),
            token.to_string(),
            i64::from(std::process::id()),
            timestamp(OffsetDateTime::now_utc())?,
            SessionStatus::Active.as_str()
        ],
    )?;

    let result = if inserted == 1 {
        LeaseAcquire::Acquired(SessionLease { session, token })
    } else {
        let state: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT s.status,
                        CASE WHEN o.session_id IS NULL THEN NULL
                             ELSE 'pid ' || o.process_id || ', held since ' || o.acquired_at END
                 FROM sessions s
                 LEFT JOIN session_owners o ON o.session_id = s.id
                 WHERE s.id = ?1",
                params![session.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match state {
            None => LeaseAcquire::Unknown,
            Some((status, _)) if SessionStatus::parse(&status) == SessionStatus::Ended => {
                LeaseAcquire::Ended
            }
            Some((_, Some(holder))) => LeaseAcquire::Owned(holder),
            Some((_, None)) => {
                return Err(crate::store::sqlite::error::SqlError::Decode {
                    detail: format!(
                        "active session {session} had no owner after its lease insert was refused"
                    ),
                });
            }
        }
    };
    transaction.commit()?;
    Ok(result)
}

/// Release a lease this process holds.
///
/// The token check is what makes this safe to call blindly on shutdown: a
/// process that lost its lease to an explicit `break_lease` cannot delete the
/// new holder's row on its way out.
pub(super) fn release_session(connection: &Connection, lease: &SessionLease) -> SqlResult<()> {
    connection.execute(
        "DELETE FROM session_owners WHERE session_id = ?1 AND owner_token = ?2",
        params![lease.session.to_string(), lease.token.to_string()],
    )?;
    Ok(())
}

pub(super) fn break_lease(connection: &Connection, session: SessionId) -> SqlResult<()> {
    connection.execute(
        "DELETE FROM session_owners WHERE session_id = ?1",
        params![session.to_string()],
    )?;
    Ok(())
}

/// `PRAGMA integrity_check`, which returns the single row `ok` when healthy.
pub(super) fn integrity_check(connection: &Connection) -> SqlResult<IntegrityReport> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;

    let mut problems = Vec::new();
    for row in rows {
        let line = row?;
        if line != "ok" {
            problems.push(line);
        }
    }

    if problems.is_empty() {
        Ok(IntegrityReport::Ok)
    } else {
        Ok(IntegrityReport::Problems(problems))
    }
}

/// Counts for the diagnostics report.
pub(super) fn counts(connection: &Connection) -> SqlResult<(u64, u64, u64, u64)> {
    let one = |sql: &str| -> SqlResult<u64> {
        let value: i64 = connection.query_row(sql, [], |row| row.get(0))?;
        Ok(count(value))
    };
    Ok((
        one("SELECT COUNT(*) FROM sessions")?,
        one("SELECT COUNT(*) FROM events")?,
        one("SELECT COUNT(*) FROM checkpoints")?,
        one("SELECT COUNT(*) FROM session_owners")?,
    ))
}

pub(super) fn pragma_text(connection: &Connection, name: &str) -> SqlResult<String> {
    Ok(schema::pragma_text(connection, name)?)
}

pub(super) fn pragma_number(connection: &Connection, name: &str) -> SqlResult<i64> {
    Ok(schema::pragma_number(connection, name)?)
}

/// A stored count is never negative; SQLite's type is signed regardless.
fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn stored_count(value: i64, field: &str) -> SqlResult<u64> {
    u64::try_from(value).map_err(|_| crate::store::sqlite::error::SqlError::Decode {
        detail: format!("stored {field} is negative: {value}"),
    })
}

fn version_of(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn timestamp(value: OffsetDateTime) -> SqlResult<String> {
    value
        .format(&Rfc3339)
        .map_err(|error| crate::store::sqlite::error::SqlError::Decode {
            detail: format!("timestamp could not be formatted: {error}"),
        })
}

fn parse_timestamp(raw: &str) -> SqlResult<OffsetDateTime> {
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| {
        crate::store::sqlite::error::SqlError::Decode {
            detail: format!("stored timestamp `{raw}` is not RFC3339: {error}"),
        }
    })
}

fn parse_uuid(raw: &str) -> SqlResult<Uuid> {
    Uuid::parse_str(raw).map_err(|error| crate::store::sqlite::error::SqlError::Decode {
        detail: format!("stored id `{raw}` is not a uuid: {error}"),
    })
}

/// Move a session's active leaf.
///
/// `None` clears it, restoring "the highest sequence" — where a session that
/// has never branched always sits.
pub(super) fn set_active_leaf(
    connection: &mut Connection,
    session: SessionId,
    sequence: Option<u64>,
) -> SqlResult<()> {
    let sequence: Option<i64> = sequence.map(|value| i64::try_from(value).unwrap_or(i64::MAX));
    connection.execute(
        "UPDATE sessions SET active_leaf_sequence = ?2 WHERE id = ?1",
        params![session.to_string(), sequence],
    )?;
    Ok(())
}

/// The sequences on a session's active branch, newest first.
///
/// Reads only `sequence` and `parent_sequence`, never a payload. That is the
/// point: the ancestry question — "is this checkpoint on the branch we are
/// about to replay?" — is answerable from the shape of the tree alone, and
/// decoding every event to answer it would undo what the checkpoint is for.
fn branch_sequences(connection: &Connection, session: SessionId) -> SqlResult<Vec<u64>> {
    let leaf: Option<i64> = connection.query_row(
        "SELECT COALESCE(s.active_leaf_sequence, (SELECT MAX(sequence) FROM events WHERE session_id = s.id))
         FROM sessions s WHERE s.id = ?1",
        params![session.to_string()],
        |row| row.get(0),
    )?;
    let Some(leaf) = leaf else {
        return Ok(Vec::new());
    };

    let mut statement = connection
        .prepare("SELECT parent_sequence FROM events WHERE session_id = ?1 AND sequence = ?2")?;

    let mut walked = Vec::new();
    let mut cursor = Some(leaf);
    // Same bound and same reasoning as `branch_events`: a parent is always a
    // lower sequence, so the walk strictly decreases and cannot cycle.
    let mut guard = 0_u64;
    while let Some(sequence) = cursor {
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
        let parent = statement.query_row(params![session.to_string(), sequence], |row| {
            row.get::<_, Option<i64>>(0)
        });
        let Ok(parent) = parent else {
            break;
        };
        walked.push(stored_count(sequence, "events.sequence")?);
        cursor = match parent {
            Some(parent) => Some(parent),
            None if sequence > 0 => Some(sequence - 1),
            None => None,
        };
    }
    Ok(walked)
}

/// The sequences that belong to the branch ending at `leaf` and to no other
/// .
///
/// Walks back from the leaf and stops at the nearest fork — an event with more
/// than one child. Everything above that is shared with a sibling and is not
/// this branch's news.
fn diverged_segment(connection: &Connection, session: SessionId, leaf: u64) -> SqlResult<Vec<u64>> {
    // How many children each event has, which is what makes a fork a fork.
    // Read from parent pointers alone, never from payloads.
    let mut statement =
        connection.prepare("SELECT sequence, parent_sequence FROM events WHERE session_id = ?1")?;
    let rows = statement.query_map(params![session.to_string()], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    let mut child_count: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for row in rows {
        let (sequence, parent) = row?;
        let sequence = stored_count(sequence, "events.sequence")?;
        if let Some(parent) = resolve_parent(sequence, parent)? {
            *child_count.entry(parent).or_default() += 1;
        }
    }

    let mut parents = connection
        .prepare("SELECT parent_sequence FROM events WHERE session_id = ?1 AND sequence = ?2")?;
    let mut segment = Vec::new();
    let mut cursor = Some(leaf);
    // Same bound and reasoning as `branch_events`: the walk strictly decreases.
    let mut guard = 0_u64;
    while let Some(sequence) = cursor {
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
        let Ok(parent) = parents.query_row(params![session.to_string(), sequence], |row| {
            row.get::<_, Option<i64>>(0)
        }) else {
            break;
        };
        segment.push(sequence);
        // Stop *at* the fork: the fork's own event is shared with the sibling,
        // so it belongs to the prefix rather than to this branch.
        match resolve_parent(sequence, parent)? {
            Some(parent) if child_count.get(&parent).copied().unwrap_or(0) > 1 => break,
            other => cursor = other,
        }
    }
    segment.reverse();
    Ok(segment)
}

/// A stored parent pointer, resolved. `NULL` means "the preceding sequence",
/// which is what every event written before the tree migration carries.
fn resolve_parent(sequence: u64, parent: Option<i64>) -> SqlResult<Option<u64>> {
    match parent {
        Some(parent) => Ok(Some(stored_count(parent, "events.parent_sequence")?)),
        None if sequence > 0 => Ok(Some(sequence - 1)),
        None => Ok(None),
    }
}

/// Fold one event's recorded facts into the summary being assembled.
fn absorb(
    summary: &mut BranchSummary,
    files_read: &mut std::collections::BTreeSet<std::path::PathBuf>,
    files_changed: &mut std::collections::BTreeSet<std::path::PathBuf>,
    proposals: &mut std::collections::BTreeMap<String, String>,
    event: &MjolnrEvent,
) {
    use crate::core::message::{Role, ToolEffect};

    match event {
        MjolnrEvent::MessageAppended { message, .. } if message.role == Role::User => {
            if summary.origin.is_none() {
                summary.origin = Some(message.text());
            }
            summary.turns += 1;
        }
        MjolnrEvent::ToolProposed { call, preview, .. } => {
            proposals.insert(call.id.clone(), preview.clone());
        }
        MjolnrEvent::ToolCompleted {
            call_id,
            name,
            result,
            ..
        } => {
            if !result.outcome.is_ok() {
                summary.tool_failures += 1;
            }
            match &result.effect {
                ToolEffect::Read { path, .. } => {
                    files_read.insert(std::path::PathBuf::from(path));
                }
                ToolEffect::Mutation { path, .. } => {
                    files_changed.insert(std::path::PathBuf::from(path));
                }
                ToolEffect::Command {
                    exit_code, success, ..
                } => summary.commands.push(CommandFact {
                    command: proposals
                        .get(call_id)
                        .cloned()
                        .unwrap_or_else(|| name.clone()),
                    outcome: format!("success={success}, exit={exit_code:?}"),
                }),
                ToolEffect::None
                | ToolEffect::Completion { .. }
                | ToolEffect::SkillActivated { .. } => {}
            }
        }
        MjolnrEvent::ToolFailed { call_id, code, .. } => {
            summary.tool_failures += 1;
            if let Some(command) = proposals.get(call_id) {
                summary.commands.push(CommandFact {
                    command: command.clone(),
                    outcome: code.to_string(),
                });
            }
        }
        _ => {}
    }
}

/// What happened on the branch ending at `leaf`, since it diverged
/// .
///
/// Assembled entirely from recorded events — no model is called, and nothing
/// here is generated. See [`BranchSummary`] for why that is the point rather
/// than a limitation.
pub(super) fn branch_summary(
    connection: &mut Connection,
    session: SessionId,
    leaf: u64,
) -> SqlResult<BranchSummary> {
    let segment = diverged_segment(connection, session, leaf)?;

    let mut summary = BranchSummary::default();
    let mut files_read = std::collections::BTreeSet::new();
    let mut files_changed = std::collections::BTreeSet::new();
    let mut proposals = std::collections::BTreeMap::new();

    let mut payloads = connection.prepare(
        "SELECT schema_version, payload_json FROM events WHERE session_id = ?1 AND sequence = ?2",
    )?;
    for sequence in segment {
        let Ok((version, payload)) = payloads
            .query_row(params![session.to_string(), sequence], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
        else {
            continue;
        };
        let event = wire::decode(session, wire::decode_json(&payload, version_of(version))?);
        absorb(
            &mut summary,
            &mut files_read,
            &mut files_changed,
            &mut proposals,
            &event,
        );
    }

    summary.files_read = files_read.into_iter().collect();
    summary.files_changed = files_changed.into_iter().collect();
    Ok(summary)
}

/// Every user turn in the session, as a tree.
///
/// Reads the whole event tree, abandoned branches included — the active branch
/// alone cannot answer "what did I branch away from?". Only message-bearing
/// events are decoded; the rest contribute their parent pointers and nothing
/// else, which is what keeps a turn's parent correct across the tool traffic
/// sitting between two messages.
pub(super) fn session_tree(
    connection: &mut Connection,
    session: SessionId,
) -> SqlResult<Vec<SessionTreeNode>> {
    let active: std::collections::BTreeSet<u64> =
        branch_sequences(connection, session)?.into_iter().collect();

    let mut statement = connection.prepare(
        "SELECT sequence, parent_sequence, schema_version, payload_json
         FROM events WHERE session_id = ?1 ORDER BY sequence ASC",
    )?;
    let rows = statement.query_map(params![session.to_string()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    // The parent of each event, resolved the same way the branch walk does it:
    // a NULL parent means "the preceding sequence".
    let mut parent_of: std::collections::BTreeMap<u64, Option<u64>> =
        std::collections::BTreeMap::new();
    // The nearest ancestor *turn* of each event, filled in as the pass proceeds.
    let mut turn_of: std::collections::BTreeMap<u64, Option<u64>> =
        std::collections::BTreeMap::new();
    let mut nodes: Vec<SessionTreeNode> = Vec::new();
    // Where to hang each turn's answer, keyed by the turn's sequence.
    let mut index_of_turn: std::collections::BTreeMap<u64, usize> =
        std::collections::BTreeMap::new();

    for row in rows {
        let (sequence, parent, version, payload) = row?;
        let sequence = stored_count(sequence, "events.sequence")?;
        let parent = match parent {
            Some(parent) => Some(stored_count(parent, "events.parent_sequence")?),
            None if sequence > 0 => Some(sequence - 1),
            None => None,
        };
        parent_of.insert(sequence, parent);

        // The nearest ancestor turn: the parent if it is one, else whatever the
        // parent inherited. One lookup per event because the pass is in
        // sequence order and a parent always has a lower sequence.
        let inherited = parent.and_then(|parent| {
            if index_of_turn.contains_key(&parent) {
                Some(parent)
            } else {
                turn_of.get(&parent).copied().flatten()
            }
        });
        turn_of.insert(sequence, inherited);

        let decoded = wire::decode_json(&payload, version_of(version))?;
        let event = wire::decode(session, decoded);
        let MjolnrEvent::MessageAppended { message, .. } = &event else {
            continue;
        };

        match message.role {
            crate::core::message::Role::User => {
                index_of_turn.insert(sequence, nodes.len());
                nodes.push(SessionTreeNode {
                    sequence,
                    parent: inherited,
                    prompt: message.text(),
                    answer: None,
                    on_active_branch: active.contains(&sequence),
                });
            }
            crate::core::message::Role::Assistant => {
                // The first reply on this turn, not the last: a turn with
                // several assistant messages is one exchange, and the opening
                // reply is what identifies it in a list.
                if let Some(turn) = inherited
                    && let Some(node) = index_of_turn.get(&turn).and_then(|at| nodes.get_mut(*at))
                    && node.answer.is_none()
                {
                    node.answer = Some(message.text());
                }
            }
            crate::core::message::Role::System | crate::core::message::Role::Tool => {}
        }
    }

    Ok(nodes)
}

/// The active branch's events at or after `from`, or `None` when `from` is not
/// on the branch.
///
/// This is the branch-aware recovery read. `from` is a checkpoint's covered
/// count, so the checkpoint is usable only if every sequence it covers —
/// `0..from` — lies on the branch about to be replayed. After a rewind that
/// may be false: the checkpoint then describes a transcript from a sibling,
/// and replaying onto it would resurrect messages the user branched away from.
/// Saying `None` rather than quietly returning the wrong suffix hands the
/// caller a decision it can make correctly, which is to replay the whole
/// branch.
pub(super) fn branch_events_from(
    connection: &mut Connection,
    session: SessionId,
    from: u64,
) -> SqlResult<Option<BranchResume>> {
    let branch: std::collections::BTreeSet<u64> =
        branch_sequences(connection, session)?.into_iter().collect();

    // Sequences are dense from 0, so "the branch covers `0..from`" is exactly
    // "the branch has `from` entries below `from`".
    if branch.range(..from).count() != usize::try_from(from).unwrap_or(usize::MAX) {
        return Ok(None);
    }

    // The anchors for the checkpoint's own transcript. Read from the `kind`
    // column alone — the sequences are what the entries need, and decoding the
    // payloads to recover them would cost exactly what the checkpoint saved.
    //
    // This kind list is `MjolnrEvent::introduces_message` expressed in SQL. The
    // two are pinned together by a test; if they drifted, every entry after the
    // divergence would anchor to the wrong event.
    // The `IN` list is built from a compile-time constant, never from input.
    let kinds = crate::store::wire::MESSAGE_BEARING_KINDS
        .map(|kind| format!("'{kind}'"))
        .join(", ");
    let mut statement = connection.prepare(&format!(
        "SELECT sequence FROM events
         WHERE session_id = ?1 AND sequence < ?2 AND kind IN ({kinds})
         ORDER BY sequence ASC"
    ))?;
    let rows = statement.query_map(
        params![session.to_string(), i64::try_from(from).unwrap_or(i64::MAX)],
        |row| row.get::<_, i64>(0),
    )?;
    let mut covered_message_sequences = Vec::new();
    for row in rows {
        covered_message_sequences.push(stored_count(row?, "events.sequence")?);
    }

    let events = events_from(connection, session, from)?
        .into_iter()
        .filter(|stored| branch.contains(&stored.sequence))
        .collect();
    Ok(Some(BranchResume {
        covered_message_sequences,
        events,
    }))
}

/// The events on a session's active branch, oldest first.
///
/// Walks parent pointers back from the active leaf, then reverses. A `NULL`
/// parent means "the preceding sequence", which is what every event written
/// before the tree migration carries — so a session that has never branched
/// walks its whole history and returns exactly what `events` would.
pub(super) fn branch_events(
    connection: &mut Connection,
    session: SessionId,
) -> SqlResult<Vec<StoredEvent>> {
    let leaf: Option<i64> = connection.query_row(
        "SELECT COALESCE(s.active_leaf_sequence, (SELECT MAX(sequence) FROM events WHERE session_id = s.id))
         FROM sessions s WHERE s.id = ?1",
        params![session.to_string()],
        |row| row.get(0),
    )?;
    let Some(leaf) = leaf else {
        return Ok(Vec::new());
    };

    let mut statement = connection.prepare(
        "SELECT sequence, event_id, occurred_at, schema_version, payload_json, parent_sequence
         FROM events WHERE session_id = ?1 AND sequence = ?2",
    )?;

    let mut chain = Vec::new();
    let mut cursor = Some(leaf);
    // Bounded by the number of events: a parent is always a lower sequence, so
    // the walk strictly decreases and cannot cycle. The guard is belt-and-braces
    // against a corrupted row rather than an expected case.
    let mut guard = 0_u64;
    while let Some(sequence) = cursor {
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
        let row = statement.query_row(params![session.to_string(), sequence], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        });
        let Ok((sequence, event_id, occurred_at, schema_version, payload, parent)) = row else {
            break;
        };
        let decoded = wire::decode_json(&payload, version_of(schema_version))?;
        chain.push(StoredEvent {
            id: EventId::from_uuid(parse_uuid(&event_id)?),
            sequence: stored_count(sequence, "events.sequence")?,
            occurred_at: parse_timestamp(&occurred_at)?,
            event: wire::decode(session, decoded),
        });
        cursor = match parent {
            Some(parent) => Some(parent),
            None if sequence > 0 => Some(sequence - 1),
            None => None,
        };
    }
    chain.reverse();
    Ok(chain)
}

pub(super) fn find_session_by_dir(
    connection: &Connection,
    project_root: &std::path::Path,
) -> SqlResult<Option<SessionId>> {
    let mut statement = connection.prepare(
        "SELECT s.id FROM sessions s
         JOIN projects p ON s.project_id = p.id
         WHERE p.root_realpath = ?1 LIMIT 1",
    )?;
    let mut rows = statement.query([project_root.to_string_lossy().as_ref()])?;
    if let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        Ok(Some(SessionId::from_uuid(parse_uuid(&id)?)))
    } else {
        Ok(None)
    }
}

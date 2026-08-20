//! Deterministic workspace search (Phase D4 producer).
//!
//! One responsibility: maintain and query the `workspace_search` index. It owns
//! what text is indexable, how a query is made safe, and how a page is ordered
//! and paginated. It owns no policy and no client vocabulary.
//!
//! Four decisions are load-bearing:
//!
//! 1. **The index stores text and identity, nothing else.** Every fact a result
//!    carries — sequence, timestamp, session status — is read back from
//!    `events` and `sessions` by join. Copying them into the index would make
//!    it a second source of truth that goes stale the moment a session's status
//!    changes, and standing law 5 says the record is append-only and everything
//!    else is a projection.
//! 2. **Indexable text is an allowlist, per event kind.** The v4 migration note
//!    already records why: indexing serialized events wholesale would sweep any
//!    future secret-bearing event into a searchable table. A kind this module
//!    does not name is not indexed, which is the fail-closed direction.
//! 3. **A query is a literal phrase, never FTS5 syntax.** User text is quoted
//!    and its quotes doubled before it reaches `MATCH`, so `NEAR`, `*`, `OR`,
//!    and a stray `"` are searched for rather than executed. Values are bound
//!    parameters throughout; no SQL is ever assembled from user text.
//! 4. **Order is time, not relevance.** `bm25` would make result order depend
//!    on corpus statistics, so the same query against a growing store would
//!    reorder — and §D4 requires a rebuild to reproduce a stable order. Newest
//!    first, tie-broken to a total order, is reproducible by construction.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio_rusqlite::rusqlite::{Connection, Result as SqlResult, Transaction, params};

use crate::core::event::{EventId, MjolnrEvent, SessionId};
use crate::core::store::{WorkspaceSearchFilter, WorkspaceSearchPage, WorkspaceSearchResult};

/// Longest indexed text per event.
///
/// A tool result can be megabytes. Indexing all of it would put the transcript
/// in the index twice and make an append's cost depend on a tool's output size.
pub(super) const MAX_INDEXED_TEXT_BYTES: usize = 4 << 10;

/// Longest snippet returned on a result.
///
/// Deliberately duplicated from `core::client::workspace::MAX_SEARCH_SNIPPET_BYTES`
/// rather than imported: `tests/architecture.rs` forbids everything outside
/// `core::client` and the bridge from naming the wire contract, so the store
/// bounds its own output and the bridge re-clamps to the wire's. Two bounds
/// that agree, neither able to reach the other.
pub(super) const MAX_SNIPPET_BYTES: usize = 512;

/// Largest page this producer will return, whatever a caller asks for.
pub(super) const MAX_PAGE_SIZE: u32 = 50;

/// How deep cursor pagination may walk before refusing.
///
/// Keyset pagination stays fast at any depth, so this is not a performance
/// bound — it is a bound on how much of a project one query may enumerate.
pub(super) const MAX_CURSOR_DEPTH: u32 = 1_000;

/// Position of `text_content` in the `workspace_search` table.
///
/// FTS5's `snippet()` takes a column *index*, not a name, so this number is
/// load-bearing and unverifiable by eye. Migration 4 shipped with the producer
/// passing 7 while `text_content` sat at 9 — every snippet came from
/// `file_path`. Migration 5 declares only the columns this module writes, and
/// this constant is the single place the position is stated.
const SNIPPET_TEXT_COLUMN: usize = 7;

/// The shortest query the trigram tokenizer can answer.
///
/// FTS5's trigram tokenizer indexes three-character sequences, so a one- or
/// two-character `MATCH` matches nothing — not because nothing matched, but
/// because nothing *could*. Returning an empty page there would be the exact
/// false answer §D4 refuses; the caller gets a typed refusal instead.
pub(super) const MIN_QUERY_CHARS: usize = 3;

/// What gets written to the index for one event.
pub(super) struct IndexedDocument {
    pub(super) event_kind: String,
    pub(super) provider_model: String,
    pub(super) reason_code: String,
    pub(super) file_path: String,
    pub(super) text: String,
}

/// The indexable projection of an event, or `None` for a kind this module does
/// not index.
///
/// The allowlist. Adding a kind here is a deliberate act with a security
/// question attached — "can this event carry a credential?" — which is why the
/// default is to index nothing.
pub(super) fn document(event: &MjolnrEvent) -> Option<IndexedDocument> {
    /// Every field but the text, which each arm supplies separately.
    fn base(kind: &str) -> IndexedDocument {
        IndexedDocument {
            event_kind: kind.to_owned(),
            provider_model: String::new(),
            reason_code: String::new(),
            file_path: String::new(),
            text: String::new(),
        }
    }

    let mut document = match event {
        MjolnrEvent::MessageAppended { message, .. } => IndexedDocument {
            text: message_text(message),
            ..base("MessageAppended")
        },
        MjolnrEvent::ToolProposed {
            call,
            preview,
            tier,
            ..
        } => IndexedDocument {
            file_path: tool_path(call),
            text: format!("{} {tier:?} {preview}", call.name),
            ..base("ToolProposed")
        },
        MjolnrEvent::ToolCompleted { name, result, .. } => IndexedDocument {
            // `ToolResult` carries no reason code — a failure that has one
            // arrives as `ToolFailed`, which does. Leaving this empty is the
            // honest answer rather than deriving a code from an outcome.
            text: format!("{name} {}", result.content),
            ..base("ToolCompleted")
        },
        MjolnrEvent::ToolFailed {
            name, code, detail, ..
        } => IndexedDocument {
            reason_code: code.as_str().to_owned(),
            text: format!("{name} {detail}"),
            ..base("ToolFailed")
        },
        MjolnrEvent::RunFailed { code, detail, .. } => IndexedDocument {
            reason_code: code.as_str().to_owned(),
            text: detail.clone(),
            ..base("RunFailed")
        },
        MjolnrEvent::ModelChangeRefused {
            provider,
            model,
            code,
            detail,
            ..
        } => IndexedDocument {
            provider_model: format!("{}/{}", provider.as_str(), model.as_str()),
            reason_code: code.as_str().to_owned(),
            text: detail.clone(),
            ..base("ModelChangeRefused")
        },
        MjolnrEvent::SessionCreated {
            provider, model, ..
        } => {
            let pair = format!("{}/{}", provider.as_str(), model.as_str());
            IndexedDocument {
                provider_model: pair.clone(),
                text: pair,
                ..base("SessionCreated")
            }
        }
        MjolnrEvent::ModelChanged {
            provider, model, ..
        } => {
            let pair = format!("{}/{}", provider.as_str(), model.as_str());
            IndexedDocument {
                provider_model: pair.clone(),
                text: pair,
                ..base("ModelChanged")
            }
        }
        MjolnrEvent::SubagentSpawned {
            directive, branch, ..
        } => IndexedDocument {
            text: format!("{directive} {branch}"),
            ..base("SubagentSpawned")
        },
        MjolnrEvent::ExtensionLoaded { name, program, .. } => IndexedDocument {
            text: format!("{name} {program}"),
            ..base("ExtensionLoaded")
        },
        // Not indexed. Includes every ephemeral event, every purely numeric
        // one, and anything not yet reviewed for what it can carry.
        _ => return None,
    };

    if document.text.trim().is_empty() {
        return None;
    }
    document.text = bound(&document.text, MAX_INDEXED_TEXT_BYTES);
    Some(document)
}

/// The human-readable text of a message, tool-call arguments excluded.
///
/// Arguments are excluded deliberately: they are structured data a model
/// produced, they are already searchable through `ToolProposed`'s preview, and
/// a JSON blob in a trigram index is noise that matches everything.
fn message_text(message: &crate::core::message::CanonicalMessage) -> String {
    let mut text = String::new();
    for block in &message.blocks {
        if let crate::core::message::ContentBlock::Text { text: body } = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(body.as_str());
        }
    }
    text
}

/// The `path` argument of a tool call, when it has one.
fn tool_path(call: &crate::core::message::ToolCall) -> String {
    call.arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Truncate on a character boundary, marking that it was cut.
fn bound(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", text.get(..end).unwrap_or_default())
}

/// Index one event, inside the caller's append transaction.
///
/// Inside, not after: an indexed row that outlived a rolled-back append would
/// be a search hit for an event that does not exist.
pub(super) fn index(
    transaction: &Transaction<'_>,
    event: &MjolnrEvent,
    event_id: &EventId,
) -> SqlResult<()> {
    let Some(document) = document(event) else {
        return Ok(());
    };
    let session = event.session().to_string();
    // `project_id` is read from `sessions` in the same statement rather than
    // passed in, so the index can never disagree with the session's project.
    transaction.execute(
        "INSERT INTO workspace_search
           (event_id, session_id, project_id, event_kind, provider_model, reason_code,
            file_path, text_content)
         SELECT ?1, ?2, s.project_id, ?3, ?4, ?5, ?6, ?7
           FROM sessions s WHERE s.id = ?2",
        params![
            event_id.to_string(),
            session,
            document.event_kind,
            document.provider_model,
            document.reason_code,
            document.file_path,
            document.text,
        ],
    )?;
    Ok(())
}

/// Rebuild the whole index from `events`.
///
/// Deterministic by construction: it replays the durable record through the
/// same [`document`] projection an append uses, in `rowid` order. §D4 requires
/// a rebuild to produce the same document set and a stable result order, and
/// the only way to guarantee that is for there to be exactly one projection.
pub(super) fn rebuild(connection: &mut Connection) -> Result<u64, super::error::SqlError> {
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM workspace_search", [])?;

    let rows: Vec<(String, String, i64, String)> = {
        let mut statement = transaction.prepare(
            "SELECT e.event_id, e.session_id, e.schema_version, e.payload_json
               FROM events e
              ORDER BY e.session_id, e.sequence",
        )?;
        let mapped = statement.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        mapped.collect::<SqlResult<Vec<_>>>()?
    };

    let mut indexed = 0_u64;
    for (event_id, session_id, version, payload_json) in rows {
        // A payload this build cannot decode is a forward-compatibility fact,
        // not a rebuild failure: it stays in `events` and is simply not
        // searchable by this build. Failing the whole rebuild would let one
        // unreadable row cost every other row its searchability.
        let Ok(session) = uuid::Uuid::parse_str(&session_id).map(SessionId::from_uuid) else {
            continue;
        };
        let Ok(payload) = crate::store::wire::decode_json(
            &payload_json,
            u32::try_from(version).unwrap_or(u32::MAX),
        ) else {
            continue;
        };
        let event = crate::store::wire::decode(session, payload);
        let Some(document) = document(&event) else {
            continue;
        };
        transaction.execute(
            "INSERT INTO workspace_search
               (event_id, session_id, project_id, event_kind, provider_model, reason_code,
                file_path, text_content)
             SELECT ?1, ?2, s.project_id, ?3, ?4, ?5, ?6, ?7
               FROM sessions s WHERE s.id = ?2",
            params![
                event_id,
                session_id,
                document.event_kind,
                document.provider_model,
                document.reason_code,
                document.file_path,
                document.text,
            ],
        )?;
        indexed = indexed.saturating_add(1);
    }

    transaction.commit()?;
    Ok(indexed)
}

/// Why a search could not be answered, as opposed to answered with nothing.
pub(super) enum SearchRefusal {
    QueryTooShort,
    CursorMismatch,
    CursorTooDeep,
}

impl SearchRefusal {
    pub(super) fn detail(&self) -> String {
        match self {
            Self::QueryTooShort => format!(
                "a search query needs at least {MIN_QUERY_CHARS} characters: the index is \
                 trigram-tokenized, so a shorter query cannot match anything — this is a \
                 refusal, not an empty result"
            ),
            Self::CursorMismatch => {
                "this cursor was issued for a different filter; paging it here would silently \
                 skip or repeat results, so it is refused"
                    .to_owned()
            }
            Self::CursorTooDeep => format!(
                "cursor pagination is bounded at {MAX_CURSOR_DEPTH} results; narrow the filter \
                 rather than enumerating further"
            ),
        }
    }
}

/// Run one page of a search.
pub(super) fn search(
    connection: &Connection,
    filter: &WorkspaceSearchFilter,
) -> Result<Result<WorkspaceSearchPage, super::error::SqlError>, SearchRefusal> {
    let query = filter.query.as_deref().unwrap_or_default().trim();
    if query.chars().count() < MIN_QUERY_CHARS {
        return Err(SearchRefusal::QueryTooShort);
    }

    let fingerprint = fingerprint(filter);
    let cursor = match filter.cursor.as_deref() {
        Some(token) => Some(Cursor::decode(token, &fingerprint)?),
        None => None,
    };
    if let Some(cursor) = &cursor
        && cursor.depth >= MAX_CURSOR_DEPTH
    {
        return Err(SearchRefusal::CursorTooDeep);
    }

    let limit = filter.limit.clamp(1, MAX_PAGE_SIZE);
    Ok(run_page(
        connection,
        filter,
        query,
        cursor.as_ref(),
        limit,
        &fingerprint,
    ))
}

fn run_page(
    connection: &Connection,
    filter: &WorkspaceSearchFilter,
    query: &str,
    cursor: Option<&Cursor>,
    limit: u32,
    fingerprint: &str,
) -> Result<WorkspaceSearchPage, super::error::SqlError> {
    // One extra row decides whether a next cursor is warranted, without a
    // second count query that could disagree with this one.
    let probe = i64::from(limit).saturating_add(1);
    let (cursor_time, cursor_session, cursor_sequence) = cursor.map_or_else(
        || (String::new(), String::new(), 0_i64),
        |cursor| {
            (
                cursor.occurred_at.clone(),
                cursor.session_id.clone(),
                cursor.sequence,
            )
        },
    );
    let has_cursor = i64::from(cursor.is_some());

    let mut statement = connection.prepare(&search_sql())?;
    let rows = statement.query_map(
        params![
            phrase(query),
            optional(filter.project_id.as_ref().map(ToString::to_string)),
            optional(filter.session_id.as_ref().map(ToString::to_string)),
            optional(filter.event_kind.clone()),
            optional(filter.provider_model.clone()),
            optional(filter.reason_code.clone()),
            optional(filter.file_path.clone()),
            optional(filter.status.map(|status| status.as_str().to_owned())),
            optional(filter.time_start.and_then(|at| timestamp(at).ok())),
            optional(filter.time_end.and_then(|at| timestamp(at).ok())),
            has_cursor,
            cursor_time,
            cursor_session,
            cursor_sequence,
            probe,
        ],
        |row| {
            let session: String = row.get(0)?;
            let event: String = row.get(1)?;
            let sequence: i64 = row.get(2)?;
            let occurred_at: String = row.get(3)?;
            let snippet: String = row.get(4)?;
            Ok(RawHit {
                session,
                event,
                sequence,
                occurred_at,
                snippet,
            })
        },
    )?;

    let mut hits = rows.collect::<SqlResult<Vec<_>>>()?;
    let overflowed = hits.len() > limit as usize;
    hits.truncate(limit as usize);

    let depth = cursor.map_or(0, |cursor| cursor.depth);
    let next_cursor = overflowed.then(|| hits.last()).flatten().map(|last| {
        Cursor {
            occurred_at: last.occurred_at.clone(),
            session_id: last.session.clone(),
            sequence: last.sequence,
            depth: depth.saturating_add(limit),
        }
        .encode(fingerprint)
    });

    let items = hits
        .into_iter()
        .filter_map(|hit| {
            Some(WorkspaceSearchResult {
                session_id: uuid::Uuid::parse_str(&hit.session)
                    .map(SessionId::from_uuid)
                    .ok()?,
                event_id: uuid::Uuid::parse_str(&hit.event)
                    .map(EventId::from_uuid)
                    .ok()?,
                sequence: u64::try_from(hit.sequence).ok()?,
                match_snippet: bound(&hit.snippet, MAX_SNIPPET_BYTES),
                occurred_at: parse_timestamp(&hit.occurred_at)?,
            })
        })
        .collect();

    Ok(WorkspaceSearchPage { items, next_cursor })
}

struct RawHit {
    session: String,
    event: String,
    sequence: i64,
    occurred_at: String,
    snippet: String,
}

/// Every filter is `?N IS NULL OR <column> = ?N`, so one prepared statement
/// serves every combination and no SQL is ever built from user input.
///
/// The keyset predicate is spelled out rather than expressed with a tuple
/// comparison because SQLite does not support row values in every position and
/// a silent fallback to a scan is worse than four extra lines.
fn search_sql() -> String {
    format!(
        "
SELECT e.session_id,
       e.event_id,
       e.sequence,
       e.occurred_at,
       snippet(workspace_search, {SNIPPET_TEXT_COLUMN}, '', '', '…', 12)
  FROM workspace_search
  JOIN events e ON e.event_id = workspace_search.event_id
  JOIN sessions s ON s.id = e.session_id
 WHERE workspace_search MATCH ?1
   AND (?2 IS NULL OR workspace_search.project_id = ?2)
   AND (?3 IS NULL OR workspace_search.session_id = ?3)
   AND (?4 IS NULL OR workspace_search.event_kind = ?4)
   AND (?5 IS NULL OR workspace_search.provider_model = ?5)
   AND (?6 IS NULL OR workspace_search.reason_code = ?6)
   AND (?7 IS NULL OR workspace_search.file_path = ?7)
   AND (?8 IS NULL OR s.status = ?8)
   AND (?9 IS NULL OR e.occurred_at >= ?9)
   AND (?10 IS NULL OR e.occurred_at <= ?10)
   AND (?11 = 0
        OR e.occurred_at < ?12
        OR (e.occurred_at = ?12 AND e.session_id > ?13)
        OR (e.occurred_at = ?12 AND e.session_id = ?13 AND e.sequence < ?14))
 ORDER BY e.occurred_at DESC, e.session_id ASC, e.sequence DESC
 LIMIT ?15
"
    )
}

/// Wrap a query as a single FTS5 literal phrase.
///
/// This is the whole injection story for the MATCH expression. Values reach
/// SQLite as bound parameters, so SQL injection is not the risk; FTS5 *query
/// syntax* is. A bare `NEAR(a b)` or a stray `"` would be executed as an
/// operator or produce a syntax error. Quoting the whole string and doubling
/// its quotes makes every character a thing to search for.
fn phrase(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len() + 2);
    escaped.push('"');
    for character in query.chars() {
        if character == '"' {
            escaped.push('"');
        }
        // Control characters are dropped rather than escaped: they cannot
        // appear in indexed text and a terminal-control sequence has no
        // business travelling through a query string (§D4 acceptance).
        if !character.is_control() {
            escaped.push(character);
        }
    }
    escaped.push('"');
    escaped
}

fn optional(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn timestamp(at: OffsetDateTime) -> Result<String, time::error::Format> {
    at.format(&time::format_description::well_known::Rfc3339)
}

fn parse_timestamp(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
}

/// The pagination key: where the previous page ended, plus how far in it was.
struct Cursor {
    occurred_at: String,
    session_id: String,
    sequence: i64,
    depth: u32,
}

impl Cursor {
    /// Encoded with the filter's fingerprint so a cursor cannot be replayed
    /// against a different filter. Doing so would silently skip or repeat
    /// results, which looks like data loss and is indistinguishable from it.
    fn encode(&self, fingerprint: &str) -> String {
        format!(
            "{fingerprint}:{}:{}:{}:{}",
            self.depth, self.sequence, self.session_id, self.occurred_at
        )
    }

    fn decode(token: &str, fingerprint: &str) -> Result<Self, SearchRefusal> {
        let rest = token
            .strip_prefix(fingerprint)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or(SearchRefusal::CursorMismatch)?;
        let mut parts = rest.splitn(4, ':');
        let depth = parts
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(SearchRefusal::CursorMismatch)?;
        let sequence = parts
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(SearchRefusal::CursorMismatch)?;
        let session_id = parts
            .next()
            .ok_or(SearchRefusal::CursorMismatch)?
            .to_owned();
        let occurred_at = parts
            .next()
            .ok_or(SearchRefusal::CursorMismatch)?
            .to_owned();
        Ok(Self {
            occurred_at,
            session_id,
            sequence,
            depth,
        })
    }
}

/// A stable digest of everything about a filter except its page position.
fn fingerprint(filter: &WorkspaceSearchFilter) -> String {
    let mut hasher = Sha256::new();
    for part in [
        filter.query.clone().unwrap_or_default(),
        filter
            .project_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        filter
            .session_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        filter.work_kind.clone().unwrap_or_default(),
        filter.event_kind.clone().unwrap_or_default(),
        filter
            .status
            .map(|status| status.as_str().to_owned())
            .unwrap_or_default(),
        filter.provider_model.clone().unwrap_or_default(),
        filter.reason_code.clone().unwrap_or_default(),
        filter.file_path.clone().unwrap_or_default(),
        filter
            .time_start
            .and_then(|at| timestamp(at).ok())
            .unwrap_or_default(),
        filter
            .time_end
            .and_then(|at| timestamp(at).ok())
            .unwrap_or_default(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update(b"\x1f");
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_quoted_so_fts5_operators_are_searched_for_not_executed() {
        assert_eq!(phrase("hello"), "\"hello\"");
        // The three that would otherwise be FTS5 syntax.
        assert_eq!(phrase("NEAR(a b)"), "\"NEAR(a b)\"");
        assert_eq!(phrase("a OR b"), "\"a OR b\"");
        assert_eq!(phrase("wild*"), "\"wild*\"");
        // A quote is doubled, which is FTS5's own escape, so the phrase stays
        // one phrase instead of closing early and leaving syntax behind it.
        assert_eq!(phrase("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    /// Terminal control characters must not survive into a query string that a
    /// refusal message might later echo (§D4 acceptance).
    #[test]
    fn control_characters_are_dropped_from_a_query() {
        let escaped = phrase("red\u{1b}[31mtext\u{7}");
        assert!(!escaped.contains('\u{1b}'));
        assert!(!escaped.contains('\u{7}'));
        assert_eq!(escaped, "\"red[31mtext\"");
    }

    #[test]
    fn a_cursor_round_trips_within_its_own_filter() {
        let cursor = Cursor {
            occurred_at: "2026-07-30T10:00:00Z".to_owned(),
            session_id: "01234567-89ab-7def-8000-000000000000".to_owned(),
            sequence: 42,
            depth: 50,
        };
        let token = cursor.encode("deadbeef");
        let decoded = Cursor::decode(&token, "deadbeef").ok().expect("decodes");
        assert_eq!(decoded.occurred_at, "2026-07-30T10:00:00Z");
        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.depth, 50);
    }

    /// The guard: a cursor from one filter must not page another. Accepting it
    /// would skip or repeat results, which is indistinguishable from data loss.
    #[test]
    fn a_cursor_from_a_different_filter_is_refused() {
        let token = Cursor {
            occurred_at: "2026-07-30T10:00:00Z".to_owned(),
            session_id: "s".to_owned(),
            sequence: 1,
            depth: 0,
        }
        .encode("aaaaaaaa");
        assert!(matches!(
            Cursor::decode(&token, "bbbbbbbb"),
            Err(SearchRefusal::CursorMismatch)
        ));
        assert!(matches!(
            Cursor::decode("garbage", "aaaaaaaa"),
            Err(SearchRefusal::CursorMismatch)
        ));
    }

    #[test]
    fn a_fingerprint_changes_with_the_filter_and_not_with_the_page() {
        let base = WorkspaceSearchFilter {
            query: Some("hello".to_owned()),
            limit: 10,
            ..WorkspaceSearchFilter::default()
        };

        let paged = WorkspaceSearchFilter {
            cursor: Some("anything".to_owned()),
            limit: 25,
            ..base.clone()
        };
        assert_eq!(
            fingerprint(&base),
            fingerprint(&paged),
            "paging must not change a filter's identity"
        );

        let different = WorkspaceSearchFilter {
            reason_code: Some("TOOL_EXECUTION".to_owned()),
            ..base.clone()
        };
        assert_ne!(fingerprint(&base), fingerprint(&different));
    }

    #[test]
    fn indexed_text_is_bounded_on_a_character_boundary() {
        let long = "é".repeat(MAX_INDEXED_TEXT_BYTES);
        let bounded = bound(&long, MAX_INDEXED_TEXT_BYTES);
        assert!(!bounded.contains('\u{fffd}'));
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn short_text_is_returned_verbatim() {
        assert_eq!(bound("hello", 64), "hello");
    }
}

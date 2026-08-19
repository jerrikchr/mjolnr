//! Tiers 2 and 3: temporal knowledge triples and progressive recall
//! (master implementation plan, §2.2–2.3).
//!
//! One responsibility: own `.mjolnr/data/memory.db` — a **disposable
//! projection** (Standing Law #2). The schema, the invalidation rule, and the
//! recall scoring live here and nowhere else; the runtime calls in, never the
//! reverse.
//!
//! Four decisions are load-bearing:
//!
//! 1. **Updates invalidate, they never rewrite.** Recording a fact that
//!    supersedes a current one sets `valid_until` on the old triple and
//!    appends the new one. Nothing is updated in place, so a fact's history
//!    is derivable and "what did we believe in June" stays answerable.
//! 2. **Recall is a window, never an export.** Search returns bounded
//!    one-line summaries with ids; only `expand` returns full detail, only
//!    for named ids. That is the token-cost mechanism.
//! 3. **A query is a literal phrase, never FTS5 syntax** — the same rule the
//!    workspace index follows: user text is phrase-quoted with quotes
//!    doubled before it reaches `MATCH`, and every value is a bound
//!    parameter.
//! 4. **Scores are hybrid and deterministic**: `0.6 × FTS + 0.4 × recency`,
//!    ties broken by id. The vector term arrives in a later checkpoint
//!    (plan §2.3); if the embedding dependency is rejected, these weights
//!    already stand alone.

use std::path::Path;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_rusqlite::Connection as DbConnection;
use tokio_rusqlite::rusqlite::types::Type as SqlType;
use tokio_rusqlite::rusqlite::{Error as SqlError, Result as SqlResult, params};

use crate::memory::error::MemoryError;

/// Weight of the full-text term in the hybrid score.
pub const FTS_WEIGHT: f64 = 0.6;

/// Weight of the recency term in the hybrid score.
pub const RECENCY_WEIGHT: f64 = 0.4;

/// Most hits one search may return.
pub const MAX_SEARCH_LIMIT: usize = 20;

/// Default when a search names no limit.
pub const DEFAULT_SEARCH_LIMIT: usize = 8;

/// Most ids one expand may name.
pub const MAX_EXPAND_IDS: usize = 10;

/// Longest one-line summary a search hit carries.
pub const MAX_SUMMARY_CHARS: usize = 240;

/// Longest subject, predicate, or object value, in characters.
pub const MAX_VALUE_CHARS: usize = 2_048;

/// The shortest query the trigram tokenizer can answer.
pub const MIN_QUERY_CHARS: usize = 3;

/// Widest candidate set the scoring pass reads before ranking.
///
/// Not a correctness bound (search stays correct at any corpus size) — a
/// bound on how much work one query may spend before the top-K cut.
const MAX_CANDIDATES: i64 = 100;

/// One temporal fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triple {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: OffsetDateTime,
    /// `None` while the fact is current; the instant it was superseded.
    pub valid_until: Option<OffsetDateTime>,
    /// Where the fact came from — a session id, or `user` for an
    /// inspector-authored fact. Provenance the projection must carry.
    pub source: String,
}

/// One consolidated episodic summary of a session's turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub id: i64,
    pub session_id: String,
    pub summary: String,
    pub key_decisions: String,
    pub source_event_start: u64,
    pub source_event_end: u64,
    pub created_at: OffsetDateTime,
}

/// One search hit: identity and a one-line summary, never the full object.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    /// First [`MAX_SUMMARY_CHARS`] characters of the object.
    pub summary: String,
    pub score: f64,
    pub current: bool,
}

/// The projection database. Cheap-clone handle over one connection.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    connection: DbConnection,
}

/// Row counts for the inspector projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryCounts {
    pub facts: usize,
    pub episodes: usize,
}

impl MemoryStore {
    /// Open (creating if absent) the projection at `path`.
    pub async fn open(path: &Path) -> Result<Self, MemoryError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let parent = parent.to_path_buf();
            tokio::fs::create_dir_all(&parent)
                .await
                .map_err(|error| MemoryError::Unavailable {
                    detail: format!("create {}: {error}", parent.display()),
                })?;
        }
        let connection =
            DbConnection::open(path)
                .await
                .map_err(|error| MemoryError::Unavailable {
                    detail: format!("{}: {error}", path.display()),
                })?;
        let store = Self { connection };
        store
            .connection
            .call(|conn| conn.execute_batch(SCHEMA))
            .await
            .map_err(|error| call_error(&error))?;
        Ok(store)
    }

    /// Record a fact, automatically invalidating the current triple it
    /// supersedes (same subject and predicate).
    ///
    /// Returns the new fact's id. Superseded facts keep their history:
    /// `valid_until` is set, the row is never rewritten.
    pub async fn record_fact(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        source: &str,
        at: OffsetDateTime,
    ) -> Result<i64, MemoryError> {
        for (label, value) in [
            ("subject", subject),
            ("predicate", predicate),
            ("object", object),
        ] {
            if value.chars().count() > MAX_VALUE_CHARS {
                return Err(MemoryError::QueryRefused {
                    detail: format!("{label} exceeds {MAX_VALUE_CHARS} characters"),
                });
            }
        }

        let subject = subject.to_owned();
        let predicate = predicate.to_owned();
        let object = object.to_owned();
        let source = source.to_owned();
        let from = format_rfc3339(at);

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "UPDATE memory_facts SET valid_until = ?1 \
                     WHERE subject = ?2 AND predicate = ?3 AND valid_until IS NULL",
                    params![from, subject, predicate],
                )?;
                tx.execute(
                    "INSERT INTO memory_facts \
                     (subject, predicate, object, valid_from, valid_until, source) \
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                    params![subject, predicate, object, from, source],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO memory_fts (fact_id, body) VALUES (?1, ?2)",
                    params![id, format!("{subject} {predicate} {object}")],
                )?;
                tx.commit()?;
                Ok(id)
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Search current facts: bounded one-line summaries, hybrid-scored.
    pub async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<RecallHit>, MemoryError> {
        let limit = limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        if query.chars().count() < MIN_QUERY_CHARS {
            return Err(MemoryError::QueryRefused {
                detail: format!("query shorter than {MIN_QUERY_CHARS} characters cannot match"),
            });
        }

        let phrase = phrase_quote(query);
        self.connection
            .call(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT f.id, f.subject, f.predicate, f.object, f.valid_from, \
                     bm25(memory_fts) AS rank \
                     FROM memory_fts JOIN memory_facts AS f ON f.id = memory_fts.fact_id \
                     WHERE memory_fts MATCH ?1 AND f.valid_until IS NULL \
                     ORDER BY f.id LIMIT ?2",
                )?;
                let rows = statement.query_map(params![phrase, MAX_CANDIDATES], |row| {
                    Ok(Candidate {
                        id: row.get(0)?,
                        subject: row.get(1)?,
                        predicate: row.get(2)?,
                        object: row.get(3)?,
                        valid_from: parse_timestamp(&row.get::<_, String>(4)?)?,
                        fts: -row.get::<_, f64>(5)?,
                    })
                })?;

                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row?);
                }
                drop(statement);

                let now = OffsetDateTime::now_utc();
                let max_fts = candidates
                    .iter()
                    .map(|candidate| candidate.fts)
                    .fold(0.0_f64, f64::max)
                    .max(1.0_f64);

                let mut hits: Vec<RecallHit> = candidates
                    .into_iter()
                    .map(|candidate| {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "recency is a ranking heuristic; a 52-bit mantissa holds \
                                      any age in hours exactly"
                        )]
                        let age_hours = (now - candidate.valid_from).whole_hours().max(0) as f64;
                        let recency = 1.0 / (1.0 + age_hours / 24.0);
                        RecallHit {
                            id: candidate.id,
                            subject: candidate.subject,
                            predicate: candidate.predicate,
                            summary: truncate_chars(&candidate.object, MAX_SUMMARY_CHARS),
                            score: FTS_WEIGHT * (candidate.fts / max_fts)
                                + RECENCY_WEIGHT * recency,
                            current: true,
                        }
                    })
                    .collect();
                hits.sort_by(|a, b| {
                    // NaN cannot occur (scores are finite by construction), but a
                    // comparator that could panic on it is a crash, not a refusal —
                    // so ties resolve to the id ordering instead.
                    let by_score = b
                        .score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal);
                    by_score.then(b.id.cmp(&a.id))
                });
                hits.truncate(limit);
                Ok(hits)
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Chronology for one subject, oldest first, regardless of currency —
    /// the history view. Bounded to [`MAX_SEARCH_LIMIT`] entries.
    pub async fn timeline(&self, subject: &str) -> Result<Vec<Triple>, MemoryError> {
        let subject = subject.to_owned();
        self.connection
            .call(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT id, subject, predicate, object, valid_from, valid_until, source \
                     FROM memory_facts WHERE subject = ?1 \
                     ORDER BY valid_from, id LIMIT ?2",
                )?;
                let rows = statement.query_map(
                    params![subject, i64::try_from(MAX_SEARCH_LIMIT).unwrap_or(i64::MAX)],
                    triple_of_row,
                )?;
                let mut triples = Vec::new();
                for row in rows {
                    triples.push(row?);
                }
                Ok(triples)
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Full detail for named ids only — the targeted second fetch.
    pub async fn expand(&self, ids: &[i64]) -> Result<Vec<Triple>, MemoryError> {
        if ids.len() > MAX_EXPAND_IDS {
            return Err(MemoryError::QueryRefused {
                detail: format!(
                    "expand names {} ids; the limit is {MAX_EXPAND_IDS}",
                    ids.len()
                ),
            });
        }
        let ids = ids.to_vec();
        self.connection
            .call(move |conn| {
                let placeholders = ids
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT id, subject, predicate, object, valid_from, valid_until, source \
                     FROM memory_facts WHERE id IN ({placeholders}) ORDER BY id"
                );
                let mut statement = conn.prepare(&sql)?;
                let rows = statement.query_map([], triple_of_row)?;
                let mut triples = Vec::new();
                for row in rows {
                    triples.push(row?);
                }
                Ok(triples)
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Record an episodic summary of a range of events for a session.
    pub async fn record_episode(
        &self,
        session_id: &str,
        summary: &str,
        key_decisions: &str,
        event_start: u64,
        event_end: u64,
        created_at: OffsetDateTime,
    ) -> Result<i64, MemoryError> {
        let session_id = session_id.to_owned();
        let summary = summary.to_owned();
        let key_decisions = key_decisions.to_owned();
        let created_at_str = format_rfc3339(created_at);

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO memory_episodes \
                     (session_id, summary, key_decisions, source_event_start, source_event_end, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        session_id,
                        summary,
                        key_decisions,
                        event_start,
                        event_end,
                        created_at_str
                    ],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO memory_consolidation_log (session_id, last_processed_sequence, consolidated_at) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(session_id) DO UPDATE SET \
                     last_processed_sequence = excluded.last_processed_sequence, \
                     consolidated_at = excluded.consolidated_at",
                    params![session_id, event_end, created_at_str],
                )?;
                tx.commit()?;
                Ok(id)
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Get the highest event sequence already consolidated for this session.
    pub async fn get_consolidation_progress(
        &self,
        session_id: &str,
    ) -> Result<Option<u64>, MemoryError> {
        let session_id = session_id.to_owned();
        self.connection
            .call(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT last_processed_sequence FROM memory_consolidation_log WHERE session_id = ?1",
                )?;
                let mut rows = statement.query(params![session_id])?;
                if let Some(row) = rows.next()? {
                    let seq: i64 = row.get(0)?;
                    Ok(u64::try_from(seq.max(0)).ok())
                } else {
                    Ok(None)
                }
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Row counts for the inspector. `None` until a query succeeds —
    /// "unknown" is a reportable state, zero is a claim (AGENTS.md §1.3).
    pub async fn counts(&self) -> Result<MemoryCounts, MemoryError> {
        self.connection
            .call(|conn| {
                let facts: i64 =
                    conn.query_row("SELECT COUNT(*) FROM memory_facts", [], |row| row.get(0))?;
                let episodes: i64 =
                    conn.query_row("SELECT COUNT(*) FROM memory_episodes", [], |row| row.get(0))?;
                Ok(MemoryCounts {
                    facts: usize::try_from(facts).unwrap_or(usize::MAX),
                    episodes: usize::try_from(episodes).unwrap_or(usize::MAX),
                })
            })
            .await
            .map_err(|error| call_error(&error))
    }

    /// Get recent episodic summaries across sessions, newest first.
    pub async fn get_recent_episodes(&self, limit: usize) -> Result<Vec<Episode>, MemoryError> {
        let limit = limit.clamp(1, 50);
        self.connection
            .call(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT id, session_id, summary, key_decisions, source_event_start, source_event_end, created_at \
                     FROM memory_episodes ORDER BY id DESC LIMIT ?1",
                )?;
                let rows = statement.query_map(params![i64::try_from(limit).unwrap_or(50)], episode_of_row)?;
                let mut episodes = Vec::new();
                for row in rows {
                    episodes.push(row?);
                }
                Ok(episodes)
            })
            .await
            .map_err(|error| call_error(&error))
    }
}

/// One scoring candidate, read inside the connection closure.
struct Candidate {
    id: i64,
    subject: String,
    predicate: String,
    object: String,
    valid_from: OffsetDateTime,
    /// Inverted bm25 (bm25 is "smaller is better"), so larger is better here.
    fts: f64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory_facts (
    id          INTEGER PRIMARY KEY,
    subject     TEXT NOT NULL,
    predicate   TEXT NOT NULL,
    object      TEXT NOT NULL,
    valid_from  TEXT NOT NULL,
    valid_until TEXT,
    source      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS memory_facts_subject ON memory_facts (subject, valid_from);
CREATE INDEX IF NOT EXISTS memory_facts_current ON memory_facts (subject, predicate)
    WHERE valid_until IS NULL;
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5 (
    fact_id UNINDEXED,
    body,
    tokenize = 'trigram'
);
CREATE TABLE IF NOT EXISTS memory_episodes (
    id                   INTEGER PRIMARY KEY,
    session_id           TEXT NOT NULL,
    summary              TEXT NOT NULL,
    key_decisions        TEXT NOT NULL,
    source_event_start   INTEGER NOT NULL,
    source_event_end     INTEGER NOT NULL,
    created_at           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS memory_episodes_session ON memory_episodes (session_id, created_at);
CREATE TABLE IF NOT EXISTS memory_consolidation_log (
    session_id              TEXT PRIMARY KEY,
    last_processed_sequence INTEGER NOT NULL,
    consolidated_at         TEXT NOT NULL
);
";

fn episode_of_row(row: &tokio_rusqlite::rusqlite::Row<'_>) -> SqlResult<Episode> {
    let start_i64: i64 = row.get(4)?;
    let end_i64: i64 = row.get(5)?;
    Ok(Episode {
        id: row.get(0)?,
        session_id: row.get(1)?,
        summary: row.get(2)?,
        key_decisions: row.get(3)?,
        source_event_start: u64::try_from(start_i64.max(0)).unwrap_or(0),
        source_event_end: u64::try_from(end_i64.max(0)).unwrap_or(0),
        created_at: parse_timestamp(&row.get::<_, String>(6)?)?,
    })
}

fn triple_of_row(row: &tokio_rusqlite::rusqlite::Row<'_>) -> SqlResult<Triple> {
    Ok(Triple {
        id: row.get(0)?,
        subject: row.get(1)?,
        predicate: row.get(2)?,
        object: row.get(3)?,
        valid_from: parse_timestamp(&row.get::<_, String>(4)?)?,
        valid_until: row
            .get::<_, Option<String>>(5)?
            .as_deref()
            .map(parse_timestamp)
            .transpose()?,
        source: row.get(6)?,
    })
}

/// Parse a stored RFC 3339 timestamp, failing as a SQL conversion error so it
/// crosses the closure boundary as a typed failure rather than a panic.
fn parse_timestamp(text: &str) -> SqlResult<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339)
        .map_err(|error| SqlError::FromSqlConversionFailure(0, SqlType::Text, Box::new(error)))
}

/// Phrase-quote user text so FTS5 operators are searched for, not executed.
fn phrase_quote(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

fn format_rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let mut truncated: String = text.chars().take(limit).collect();
        truncated.push('…');
        truncated
    }
}

fn call_error(error: &tokio_rusqlite::Error) -> MemoryError {
    MemoryError::Execution {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;

    async fn store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let opened = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        (dir, opened)
    }

    #[tokio::test]
    async fn recording_a_superseding_fact_invalidates_not_rewrites() {
        let (_dir, store) = store().await;
        let june = OffsetDateTime::now_utc();
        let july = june + time::Duration::hours(24 * 30);

        store
            .record_fact("auth", "uses", "Lucia", "user", june)
            .await
            .unwrap();
        let new_id = store
            .record_fact("auth", "uses", "BetterAuth", "user", july)
            .await
            .unwrap();

        let timeline = store.timeline("auth").await.unwrap();
        assert_eq!(timeline.len(), 2, "the superseded fact keeps its history");
        let old = timeline.first().expect("oldest first");
        let new = timeline.get(1).expect("newest second");
        assert_eq!(old.object, "Lucia");
        assert!(old.valid_until.is_some(), "old fact invalidated");
        assert_eq!(new.id, new_id);
        assert_eq!(new.valid_until, None, "new fact is current");
    }

    #[tokio::test]
    async fn search_returns_one_line_summaries_scored_hybrid() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();
        store
            .record_fact("auth", "uses", "BetterAuth for all sessions", "user", now)
            .await
            .unwrap();
        store
            .record_fact(
                "auth",
                "migration",
                "moved from Lucia to BetterAuth in June",
                "user",
                now - time::Duration::days(90),
            )
            .await
            .unwrap();

        let hits = store.search("BetterAuth", None).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits.len() <= DEFAULT_SEARCH_LIMIT);
        for hit in &hits {
            assert!(hit.summary.chars().count() <= MAX_SUMMARY_CHARS + 1);
            assert!(hit.current);
        }
        // The 90-day-old fact matches both terms but is older; the fresh one
        // must not rank below it.
        let top = hits.first().expect("at least one hit");
        assert_eq!(top.subject, "auth");
        assert_eq!(top.predicate, "uses");
    }

    #[tokio::test]
    async fn superseded_facts_do_not_surface_in_search() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();
        store
            .record_fact("auth", "uses", "Lucia", "user", now)
            .await
            .unwrap();
        store
            .record_fact("auth", "uses", "BetterAuth", "user", now)
            .await
            .unwrap();

        let hits = store.search("Lucia", None).await.unwrap();
        assert!(
            hits.is_empty(),
            "an invalidated fact must not be recalled as current"
        );
        // The timeline still answers "what did we believe before".
        let timeline = store.timeline("auth").await.unwrap();
        assert_eq!(timeline.first().expect("oldest fact").object, "Lucia");
    }

    #[tokio::test]
    async fn fts_operators_are_searched_for_not_executed() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();
        store
            .record_fact("notes", "contains", "NEAR OR * operators", "user", now)
            .await
            .unwrap();
        // A query made entirely of FTS5 operators must not error as syntax.
        let hits = store.search("NEAR OR", None).await.unwrap();
        assert!(!hits.is_empty());
    }

    #[tokio::test]
    async fn a_query_shorter_than_the_trigram_minimum_is_refused() {
        let (_dir, store) = store().await;
        let error = store.search("ab", None).await.unwrap_err();
        assert_eq!(error.reason_code(), ReasonCode::WorkspaceSearchRefused);
    }

    #[tokio::test]
    async fn expand_returns_full_detail_only_for_named_ids() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();
        let first = store
            .record_fact("auth", "uses", "BetterAuth", "user", now)
            .await
            .unwrap();
        let second = store
            .record_fact(
                "tests",
                "run_with",
                "cargo test --all-features",
                "user",
                now,
            )
            .await
            .unwrap();

        let triples = store.expand(&[first]).await.unwrap();
        assert_eq!(triples.len(), 1);
        let expanded = triples.first().expect("the named id exists");
        assert_eq!(expanded.id, first);
        assert_eq!(expanded.object, "BetterAuth");
        assert_ne!(expanded.id, second);
    }

    #[tokio::test]
    async fn expand_refuses_beyond_the_id_cap() {
        let (_dir, store) = store().await;
        let ids: Vec<i64> = (0..=i64::try_from(MAX_EXPAND_IDS).unwrap_or(i64::MAX)).collect();
        let error = store.expand(&ids).await.unwrap_err();
        assert_eq!(error.reason_code(), ReasonCode::WorkspaceSearchRefused);
    }

    #[tokio::test]
    async fn oversized_values_are_refused() {
        let (_dir, store) = store().await;
        let error = store
            .record_fact(
                "s",
                "p",
                &"x".repeat(MAX_VALUE_CHARS + 1),
                "user",
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.reason_code(), ReasonCode::WorkspaceSearchRefused);
    }

    #[tokio::test]
    async fn an_oversized_limit_is_clamped_not_honoured() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();
        for index in 0..MAX_SEARCH_LIMIT + 10 {
            store
                .record_fact(
                    "bulk",
                    &format!("note_{index}"),
                    "filler content here",
                    "user",
                    now,
                )
                .await
                .unwrap();
        }
        let hits = store.search("filler", Some(10_000)).await.unwrap();
        assert!(hits.len() <= MAX_SEARCH_LIMIT);
    }

    #[tokio::test]
    async fn the_projection_is_disposable_by_construction() {
        // Standing Law #2, as a test: deleting the database file and opening
        // a fresh one at the same path is a supported operation, not a
        // recovery scenario.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let store = MemoryStore::open(&path).await.unwrap();
        store
            .record_fact("k", "v", "gone", "user", OffsetDateTime::now_utc())
            .await
            .unwrap();
        drop(store);
        std::fs::remove_file(&path).unwrap();
        let fresh = MemoryStore::open(&path).await.unwrap();
        assert!(fresh.search("gone", None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn counts_report_what_is_actually_in_the_projection() {
        let (_dir, store) = store().await;
        let now = OffsetDateTime::now_utc();

        let empty = store.counts().await.unwrap();
        assert_eq!(
            empty,
            MemoryCounts {
                facts: 0,
                episodes: 0
            }
        );

        store
            .record_fact("auth", "uses", "BetterAuth", "user", now)
            .await
            .unwrap();
        store
            .record_fact("auth", "uses", "Lucia", "user", now)
            .await
            .unwrap();
        store
            .record_episode("s1", "did things", "decided", 0, 9, now)
            .await
            .unwrap();

        let counted = store.counts().await.unwrap();
        // Both facts count: the invalidated one is still in the database, and
        // the number describes the projection, not the current view.
        assert_eq!(
            counted,
            MemoryCounts {
                facts: 2,
                episodes: 1
            }
        );
    }
}

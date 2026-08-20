//! Phase D4 tests for deterministic workspace search.
//!
//! The contract half (filter/page types, `EventStore` method, FTS5 migration)
//! and the producer half (real queries, real filter handling, rebuild) are both
//! here now. What these tests pin:
//!
//! - a backend with no index refuses rather than returning an empty page, and an
//!   indexed one answers — "nothing matched" and "nothing was searched" are
//!   different claims (AGENTS.md §1.3);
//! - a rebuild reproduces the same document set and the same order;
//! - a query cannot reach outside the project it was scoped to;
//! - FTS5 syntax and terminal control characters in a query are searched for,
//!   not executed;
//! - the snippet comes from the indexed text column. That one is not
//!   hypothetical: migration 4 declared `text_content` at position 9 while the
//!   producer asked `snippet()` for position 7, so every snippet was drawn from
//!   `file_path`. Nothing caught it because no test looked at snippet *content*.
//!
//! The 100,000-event benchmark is `#[ignore]`d and opt-in, like the live
//! provider tests: it takes minutes, and a slow default suite gets skipped,
//! which is worse than an explicit opt-in. Its measured numbers belong in the
//! report, produced by running it — not by describing it. The version of
//! this file before the producer was named `benchmark_100k_workspace_search`
//! while inserting 10,000 events and asserting zero results from a stub; that is
//! the failure mode to avoid, not benchmarking itself.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use mjolnr::core::event::{MjolnrEvent, SessionId};
use mjolnr::core::message::CanonicalMessage;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::store::{EventStore, ProjectId, StoreError, WorkspaceSearchFilter};
use mjolnr::store::memory::InMemoryEventStore;
use mjolnr::store::sqlite::SqliteEventStore;

fn sample_filter() -> WorkspaceSearchFilter {
    query_filter("System")
}

/// `WorkspaceSearchFilter` is `#[non_exhaustive]`, so a struct expression is
/// unavailable from outside the crate — deliberately, since adding a filter
/// field must not break every caller. Built by mutation instead.
fn query_filter(query: &str) -> WorkspaceSearchFilter {
    let mut filter = WorkspaceSearchFilter::default();
    filter.query = Some(query.to_owned());
    filter.limit = 50;
    filter
}

fn paged(query: &str, limit: u32, cursor: Option<String>) -> WorkspaceSearchFilter {
    let mut filter = query_filter(query);
    filter.limit = limit;
    filter.cursor = cursor;
    filter
}

fn scoped(query: &str, project: ProjectId) -> WorkspaceSearchFilter {
    let mut filter = query_filter(query);
    filter.project_id = Some(project);
    filter
}

/// A store with one project, one session, and the given message texts indexed.
async fn store_with_messages(
    directory: &std::path::Path,
    texts: &[&str],
) -> (SqliteEventStore, ProjectId, SessionId) {
    let store = SqliteEventStore::open(&directory.join("mjolnr.db"))
        .await
        .unwrap();
    let project = store.open_project(directory.to_path_buf()).await.unwrap();
    let session = SessionId::new();
    store
        .create_session(session, project, "search".to_owned(), None)
        .await
        .unwrap();
    store
        .append(MjolnrEvent::SessionCreated {
            session,
            provider: ProviderId::new("fake"),
            model: ModelId::new("fake-1"),
        })
        .await
        .unwrap();
    for text in texts {
        store
            .append(MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::system(*text)),
            })
            .await
            .unwrap();
    }
    (store, project, session)
}

/// The in-memory store has no index and still refuses honestly.
///
/// This was a two-store assertion while D4 was contract-only. The SQLite half
/// is now wrong — that store has a producer and answers — so it moved to
/// `an_indexed_store_answers_rather_than_refusing` below. Deleting the test
/// outright would have lost the half that is still load-bearing: a backend with
/// no index must say so, never return an empty page that reads as
/// "nothing matched" (AGENTS.md §1.3).
#[tokio::test]
async fn a_store_without_an_index_refuses_rather_than_returning_an_empty_page() {
    let memory: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let error = memory
        .search_workspace(sample_filter())
        .await
        .expect_err("in-memory store must refuse, not fabricate an empty page");
    assert!(
        matches!(error, StoreError::Unavailable { .. }),
        "the refusal is typed, not prose: {error}"
    );
}

/// The SQLite store answers. An empty result set from it is a real answer —
/// "the index was searched and matched nothing" — which is exactly what the
/// contract-era refusal existed to avoid claiming falsely.
#[tokio::test]
async fn an_indexed_store_answers_rather_than_refusing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteEventStore::open(&temp_dir.path().join("mjolnr.db"))
        .await
        .unwrap();
    let page = sqlite
        .search_workspace(sample_filter())
        .await
        .expect("an indexed store answers");
    assert!(page.items.is_empty(), "nothing has been indexed yet");
    assert!(page.next_cursor.is_none());
}

/// A query shorter than a trigram cannot match, which is different from not
/// matching. It refuses instead of returning an empty page.
#[tokio::test]
async fn a_query_too_short_for_the_trigram_index_refuses() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteEventStore::open(&temp_dir.path().join("mjolnr.db"))
        .await
        .unwrap();
    let error = sqlite
        .search_workspace(query_filter("ab"))
        .await
        .expect_err("two characters cannot be matched by a trigram index");
    assert!(
        matches!(error, StoreError::Refused { .. }),
        "an unanswerable question is a refusal, not an unavailable store: {error}"
    );
}

/// The index schema declares exactly the columns the producer writes.
///
/// This replaces an assertion that the table was *empty* — true while D4 was
/// contract-only, false now that appends index. What still matters is the shape,
/// because `snippet()` addresses columns positionally and a reordering silently
/// draws snippets from the wrong column (migration 4 did exactly that).
#[tokio::test]
async fn the_index_declares_only_the_columns_the_producer_writes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("mjolnr.db");
    let store = SqliteEventStore::open(&db_path).await.unwrap();
    drop(store); // flush schema before opening a raw connection

    let connection = tokio_rusqlite::rusqlite::Connection::open(&db_path).unwrap();
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'workspace_search'",
            [],
            |row| row.get(0),
        )
        .expect("the workspace_search FTS5 table must exist after migration");

    // `text_content` must be last: that position is what `snippet()` is given.
    let columns: Vec<&str> = sql
        .split_once('(')
        .and_then(|(_, rest)| rest.rsplit_once(')'))
        .map(|(inner, _)| inner)
        .expect("parseable create statement")
        .split(',')
        .map(str::trim)
        .filter(|part| !part.starts_with("tokenize"))
        .map(|part| part.split_whitespace().next().unwrap_or_default())
        .collect();
    assert_eq!(
        columns.last(),
        Some(&"text_content"),
        "text_content must be the last column: snippet() is given its position, \
         not its name — got {columns:?}"
    );
    // The two columns the producer refuses to fill on principle are gone: a
    // session's status changes, so a copy inside an append-only index is stale
    // from the moment it does.
    assert!(!columns.contains(&"status"), "got {columns:?}");
    assert!(!columns.contains(&"time_range"), "got {columns:?}");
}

/// The bug migration 5 exists to fix: a snippet must come from the indexed text.
///
/// Asserting on *content*, not merely on a non-empty string. The previous
/// producer returned snippets drawn from `file_path`, which is empty for a
/// message event — so a "snippet is a String" assertion would have passed.
#[tokio::test]
async fn a_snippet_carries_the_matched_text_not_another_column() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (store, _project, _session) =
        store_with_messages(temp_dir.path(), &["the wombat audit found nothing"]).await;

    let page = store
        .search_workspace(query_filter("wombat"))
        .await
        .expect("search answers");
    let hit = page.items.first().expect("the indexed message matches");
    assert!(
        hit.match_snippet.contains("wombat"),
        "the snippet must come from the indexed text column, got {:?}",
        hit.match_snippet
    );
}

/// §D4: rebuilding the index produces the same document set and stable order.
///
/// Both halves are asserted. The set, because a rebuild that quietly indexed
/// fewer documents would still return *an* order; and the order, because §D4's
/// determinism claim is about what a user sees, not about row counts.
#[tokio::test]
async fn a_rebuild_reproduces_the_same_documents_in_the_same_order() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (store, _project, _session) = store_with_messages(
        temp_dir.path(),
        &[
            "alpha wombat one",
            "beta wombat two",
            "gamma wombat three",
            "delta wombat four",
        ],
    )
    .await;

    let before = store
        .search_workspace(query_filter("wombat"))
        .await
        .expect("search answers");
    assert_eq!(before.items.len(), 4, "every message is indexed on append");

    let rebuilt = store.rebuild_search_index().await.expect("rebuild");
    assert_eq!(
        rebuilt, 5,
        "four messages plus the SessionCreated event are indexable"
    );

    let after = store
        .search_workspace(query_filter("wombat"))
        .await
        .expect("search answers");

    let ids = |page: &mjolnr::core::store::WorkspaceSearchPage| {
        page.items
            .iter()
            .map(|item| item.event_id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&before),
        ids(&after),
        "a rebuild must reproduce the same documents in the same order"
    );
}

/// A query scoped to one project must not reach into another's records.
///
/// Two databases would prove nothing — they cannot leak into each other. Both
/// projects live in one database, which is the arrangement where a missing
/// `project_id` predicate actually leaks.
#[tokio::test]
async fn a_project_scoped_query_cannot_reach_another_project() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&temp_dir.path().join("mjolnr.db"))
        .await
        .unwrap();

    let mut projects = Vec::new();
    for name in ["one", "two"] {
        let root = temp_dir.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let project = store.open_project(root).await.unwrap();
        let session = SessionId::new();
        store
            .create_session(session, project, name.to_owned(), None)
            .await
            .unwrap();
        store
            .append(MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::system(format!(
                    "wombat secret for project {name}"
                ))),
            })
            .await
            .unwrap();
        projects.push(project);
    }

    let unscoped = store
        .search_workspace(query_filter("wombat"))
        .await
        .expect("search answers");
    assert_eq!(
        unscoped.items.len(),
        2,
        "without a scope both projects are visible — the control for this test"
    );

    let first = *projects.first().expect("first project");
    let scoped = store
        .search_workspace(scoped("wombat", first))
        .await
        .expect("search answers");
    assert_eq!(
        scoped.items.len(),
        1,
        "a project-scoped query must return only that project's records"
    );
}

/// FTS5 operators in a query are searched for, not executed. An unescaped
/// `NEAR(` would be a syntax error surfacing as a store failure; an unescaped
/// `"` would close the phrase and leave syntax behind it.
#[tokio::test]
async fn fts5_syntax_in_a_query_is_data_not_an_operator() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (store, _project, _session) =
        store_with_messages(temp_dir.path(), &["wombat OR badger"]).await;

    for query in [
        "NEAR(wombat badger)",
        "wombat OR badger",
        "wombat*",
        "say \"wombat\"",
        "wombat AND (badger",
    ] {
        let page = store.search_workspace(query_filter(query)).await;
        assert!(
            page.is_ok(),
            "query {query:?} must be treated as text, not executed: {:?}",
            page.err()
        );
    }

    // And the phrase really is matched as a phrase.
    let page = store
        .search_workspace(query_filter("wombat OR badger"))
        .await
        .expect("search answers");
    assert_eq!(
        page.items.len(),
        1,
        "the literal phrase matches the one message containing it"
    );
}

/// Terminal control characters must not survive into a query, so a refusal or
/// snippet echoing one cannot move a terminal cursor.
#[tokio::test]
async fn control_characters_in_a_query_do_not_reach_the_index() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (store, _project, _session) = store_with_messages(temp_dir.path(), &["wombat"]).await;

    let page = store
        .search_workspace(query_filter("wom\u{1b}[31mbat"))
        .await
        .expect("search answers");
    // The escape is stripped, leaving "wom[31mbat", which matches nothing —
    // the point is that it neither errors nor executes.
    assert!(page.items.is_empty());
}

/// A cursor issued for one filter must not page another: doing so silently
/// skips or repeats results, which is indistinguishable from data loss.
#[tokio::test]
async fn a_cursor_from_another_filter_is_refused() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (store, _project, _session) = store_with_messages(
        temp_dir.path(),
        &["wombat one", "wombat two", "wombat three"],
    )
    .await;

    let page = store
        .search_workspace(paged("wombat", 1, None))
        .await
        .expect("search answers");
    let cursor = page.next_cursor.expect("more results remain");

    let error = store
        .search_workspace(paged("badger", 1, Some(cursor.clone())))
        .await
        .expect_err("a cursor from another filter must be refused");
    assert!(matches!(error, StoreError::Refused { .. }), "got {error:?}");

    // The same cursor against its own filter still works, so the refusal is
    // about the filter mismatch and not about cursors being broken.
    let next = store
        .search_workspace(paged("wombat", 1, Some(cursor)))
        .await
        .expect("its own filter pages fine");
    assert_eq!(next.items.len(), 1);
}

/// `find_session_by_dir` is the one D4 query that shipped real SQL. Keep it
/// honest: an unknown directory reports "not found", which is a true claim.
#[tokio::test]
async fn find_session_by_dir_reports_not_found_honestly() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&temp_dir.path().join("mjolnr.db"))
        .await
        .unwrap();

    let found = store
        .find_session_by_dir(temp_dir.path().join("no-such-project"))
        .await
        .unwrap();
    assert_eq!(found, None);
}

#[test]
fn the_filter_type_round_trips_its_defaults() {
    let filter = WorkspaceSearchFilter::default();
    assert_eq!(filter.limit, 0);
    assert!(filter.cursor.is_none());

    let _ = SessionId::new(); // the id types used by the filter construct fine
}

/// The §D4 latency acceptance: a synthetic 100,000-event store, with measured
/// p50/p95 reported in the report.
///
/// `#[ignore]` and opt-in, the same treatment the live provider tests get, for
/// the same reason: it takes minutes, and a default suite slow enough to be
/// skipped is worse than one that says what it does not run. Run it with
/// `cargo test --all-features --test workspace_search -- --ignored --nocapture`
/// and put the numbers it prints in the report. Numbers not produced this way
/// have no business being in the report.
///
/// It asserts a *shape*, not a threshold — a wall-clock bound would make the
/// suite fail on a loaded laptop, which teaches contributors to ignore it. The
/// measurement is the deliverable; the assertions only prove the corpus was
/// real and the query actually matched something.
#[tokio::test]
#[ignore = "inserts 100,000 events; opt in with --ignored"]
async fn measure_search_latency_on_a_hundred_thousand_events() {
    const EVENTS: usize = 100_000;
    const QUERIES: usize = 100;

    let temp_dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&temp_dir.path().join("mjolnr.db"))
        .await
        .unwrap();
    let project = store
        .open_project(temp_dir.path().to_path_buf())
        .await
        .unwrap();
    let session = SessionId::new();
    store
        .create_session(session, project, "bench".to_owned(), None)
        .await
        .unwrap();

    let insert_started = std::time::Instant::now();
    for index in 0..EVENTS {
        // Varied text so the trigram index has real breadth rather than one
        // token repeated 100,000 times, which would make every query trivially
        // hot and the measurement meaningless.
        store
            .append(MjolnrEvent::MessageAppended {
                session,
                message: Box::new(CanonicalMessage::system(format!(
                    "event {index} touching module_{} with wombat marker {}",
                    index % 512,
                    index % 97
                ))),
            })
            .await
            .unwrap();
    }
    let insert_elapsed = insert_started.elapsed();

    let mut samples = Vec::with_capacity(QUERIES);
    for index in 0..QUERIES {
        let query = format!("module_{}", index % 512);
        let started = std::time::Instant::now();
        let page = store
            .search_workspace(query_filter(&query))
            .await
            .expect("search answers");
        samples.push(started.elapsed());
        assert!(
            !page.items.is_empty(),
            "query {query:?} must match the corpus, or the measurement is of an empty index"
        );
    }

    samples.sort_unstable();
    // Integer ceiling division rather than float arithmetic: a percentile over
    // 100 samples has no business going through f64, and the cast back to an
    // index is exactly the truncation clippy objects to.
    let percentile = |numerator: usize, denominator: usize| {
        let position = samples
            .len()
            .saturating_mul(numerator)
            .saturating_add(denominator - 1)
            / denominator;
        samples
            .get(position.saturating_sub(1))
            .copied()
            .unwrap_or_default()
    };

    // Printed, not asserted: the report needs the number, and a threshold here
    // would fail on a loaded machine and train people to ignore this test.
    println!("--- Phase D4 search latency ---");
    println!("corpus:  {EVENTS} events, inserted in {insert_elapsed:?}");
    println!("queries: {QUERIES}");
    println!("p50:     {:?}", percentile(50, 100));
    println!("p95:     {:?}", percentile(95, 100));
    println!("max:     {:?}", samples.last().copied().unwrap_or_default());
}

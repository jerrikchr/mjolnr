//! §D6 end to end: a real GitHub read becomes a recorded imported item.
//!
//! What this asserts is not "the HTTP works" — `src/integrations/github` covers
//! that against the same mock — but that **the durable log changes only when a
//! fetch actually returned something**, and that what lands in it is what the
//! remote said rather than a tidied-up version of it.
//!
//! The producer is injected through `Runtime::spawn_with_task_source` and
//! pointed at a `wiremock` server, so the whole command path runs without
//! touching the network and without mutating the process environment — a
//! mutation that races every other test in the binary, and one this
//! repository's `unsafe` lint forbids outright.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::error::ReasonCode;
use mjolnr::core::event::{MjolnrEvent, SessionId};
use mjolnr::core::frontier::{NodeId, Provenance};
use mjolnr::core::imported::{ImportedItem, ImportedItemState};
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSnapshot};
use mjolnr::core::store::EventStore;
use mjolnr::integrations::TaskSource;
use mjolnr::integrations::github::GitHubSource;
use mjolnr::providers::fake::FakeProvider;
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "ghp_thisisnotarealtokenjustatestfixture";

/// A runtime whose GitHub producer points at `server`.
async fn open_project_with(
    server: &MockServer,
) -> (
    Runtime,
    tempfile::TempDir,
    Arc<InMemoryEventStore>,
    SessionId,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    initialize_repository(temp.path());
    let store = Arc::new(InMemoryEventStore::new());
    let producer = GitHubSource::new(mjolnr::core::secrets::Secret::new(TOKEN.to_owned()))
        .with_base_url(server.uri());
    let runtime = Runtime::spawn_with_task_source(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(producer) as Arc<dyn TaskSource>,
    );
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    let session = settle_until(&runtime, |snap| snap.session.is_some())
        .await
        .session
        .expect("session");
    (runtime, temp, store, session)
}

async fn settle_until(
    runtime: &Runtime,
    predicate: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = runtime.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition settled")
}

/// The log is what this file checks against: session state and the board are
/// both projections of these events, so asserting on the events asserts on the
/// thing that survives a restart.
async fn fetched_items(store: &InMemoryEventStore, session: SessionId) -> Vec<ImportedItem> {
    store
        .events(session)
        .await
        .expect("events")
        .into_iter()
        .filter_map(|stored| match stored.event {
            MjolnrEvent::ImportedItemFetched { item, .. } => Some(item),
            _ => None,
        })
        .collect()
}

fn fetch(task_id: &str) -> MjolnrCommand {
    MjolnrCommand::FetchTask {
        source: "github".to_owned(),
        task_id: task_id.to_owned(),
    }
}

fn fetch_batch(task_ids: &[&str]) -> MjolnrCommand {
    MjolnrCommand::FetchTasks {
        source: "github".to_owned(),
        task_ids: task_ids
            .iter()
            .map(|task_id| (*task_id).to_owned())
            .collect(),
    }
}

fn issue_body(number: u64, state: &str, updated_at: &str) -> serde_json::Value {
    serde_json::json!({
        "title": format!("Issue {number}"),
        "body": "Third-party text.",
        "html_url": format!("https://github.com/octocat/hello/issues/{number}"),
        "updated_at": updated_at,
        "state": state
    })
}

async fn serving(number: u64, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/repos/octocat/hello/issues/{number}")))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn git(repository: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn initialize_repository(root: &std::path::Path) -> String {
    git(root, &["init", "-b", "feature-parser"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "mjolnr Test"]);
    std::fs::write(root.join("parser.rs"), "fn parse() {}\n").expect("file");
    git(root, &["add", "parser.rs"]);
    git(root, &["commit", "-m", "initial"]);
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("head");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

fn submit(head_commit: &str) -> MjolnrCommand {
    MjolnrCommand::SubmitChange {
        source: "github".to_owned(),
        remote_id: "octocat/hello#1".to_owned(),
        expected_revision: "2026-08-06T10:00:00Z".to_owned(),
        title: "Fix the parser".to_owned(),
        body: "The parser needs this change.".to_owned(),
        head_commit: head_commit.to_owned(),
        head_branch: "feature-parser".to_owned(),
        base_branch: "main".to_owned(),
    }
}

#[tokio::test]
async fn a_matching_local_commit_creates_a_pull_request_and_records_terminal_events() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "open",
            "2026-08-06T10:00:00Z",
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/octocat/hello/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "html_url": "https://github.com/octocat/hello/pull/99"
        })))
        .mount(&server)
        .await;
    let (runtime, temp, store, session) = open_project_with(&server).await;
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(temp.path())
        .output()
        .expect("head");
    let head = String::from_utf8(head.stdout)
        .expect("utf8")
        .trim()
        .to_owned();
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/branches/feature-parser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "commit": { "sha": head }
        })))
        .mount(&server)
        .await;
    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("import");
    runtime.dispatch(submit(&head)).await.expect("submit");
    let events = store.events(session).await.expect("events");
    assert!(events.iter().any(|stored| matches!(
        &stored.event,
        MjolnrEvent::ToolCompleted { name, result, .. }
            if name == "submit_change" && result.content == "https://github.com/octocat/hello/pull/99"
    )));
    assert!(
        events
            .iter()
            .any(|stored| matches!(stored.event, MjolnrEvent::RunFinished { .. }))
    );
}

#[tokio::test]
async fn a_changed_local_head_is_refused_before_the_remote_submission() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "open",
            "2026-08-06T10:00:00Z",
        )))
        .mount(&server)
        .await;
    let (runtime, temp, _store, _session) = open_project_with(&server).await;
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(temp.path())
        .output()
        .expect("head");
    let head = String::from_utf8(head.stdout)
        .expect("utf8")
        .trim()
        .to_owned();
    std::fs::write(temp.path().join("parser.rs"), "fn parse() { true }\n").expect("file");
    git(temp.path(), &["add", "parser.rs"]);
    git(temp.path(), &["commit", "-m", "move-head"]);
    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("import");
    let error = runtime
        .dispatch(submit(&head))
        .await
        .expect_err("the supplied commit is stale");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceStaleRevision)
    );
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
}

#[tokio::test]
async fn a_fetched_issue_is_recorded_once_and_reaches_the_board_with_its_provenance() {
    let server = serving(
        1,
        ResponseTemplate::new(200).set_body_json(issue_body(1, "open", "2026-08-06T10:00:00Z")),
    )
    .await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("the issue is fetched and recorded");

    let items = fetched_items(&store, session).await;
    assert_eq!(items.len(), 1, "one fetch records one event");
    let item = items.first().expect("the recorded item");
    assert_eq!(item.integration, "github");
    assert_eq!(item.remote_id, "octocat/hello#1");
    assert_eq!(item.title, "Issue 1");
    assert_eq!(item.state, ImportedItemState::Open);
    assert_eq!(
        item.fetched_revision, "2026-08-06T10:00:00Z",
        "the revision a later change will be pinned to comes from the remote"
    );
    assert_eq!(
        item.source_url, "https://github.com/octocat/hello/issues/1",
        "the provenance a human follows survives the whole path"
    );
    assert!(
        item.blocked_by.is_empty(),
        "blockers are mjolnr's own ordering; a remote does not get to set them"
    );

    let board = runtime.query_board().await.expect("the board answers");
    let node = board
        .frontier
        .iter()
        .find(|node| node.id == NodeId::Imported(item.id))
        .expect("the fetched issue reaches the board");
    assert_eq!(
        node.provenance,
        Provenance::ExternalUnverified,
        "a fetched item is third-party data and must project as such"
    );
    assert_eq!(node.label, "Issue 1");
}

/// A re-fetch of an already-imported remote is a refresh, not a second row:
/// the fetched content attaches to the *same* board id, the event is pinned to
/// the revision the record held, and `apply_refresh` — reached through
/// `validate_event`, the same guard the replay path uses — decides whether it
/// is allowed. Here the remote moved, so the refresh advances the recorded
/// item to the new revision and state.
#[tokio::test]
async fn a_refetch_of_a_moved_remote_refreshes_the_recorded_item() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "open",
            "2026-08-06T10:00:00Z",
        )))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("first fetch");
    let first = fetched_items(&store, session)
        .await
        .into_iter()
        .next()
        .expect("the fetch recorded an item");
    assert_eq!(first.state, ImportedItemState::Open);

    // The remote moved: a later `updated_at`, and the state flipped to closed.
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "closed",
            "2026-08-06T11:00:00Z",
        )))
        .mount(&server)
        .await;

    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("the re-fetch becomes a refresh");

    let events = store.events(session).await.expect("events");
    let fetched: Vec<ImportedItem> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            MjolnrEvent::ImportedItemFetched { item, .. } => Some(item.clone()),
            _ => None,
        })
        .collect();
    let refreshed: Vec<ImportedItem> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            MjolnrEvent::ImportedItemRefreshed { item, .. } => Some(item.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(fetched.len(), 1, "one import, not two");
    assert_eq!(refreshed.len(), 1, "one refresh, not a second import");
    let refresh = refreshed.first().expect("the refreshed item");
    assert_eq!(refresh.id, first.id, "same board row, not a second one");
    assert_eq!(
        refresh.fetched_revision, "2026-08-06T11:00:00Z",
        "the recorded item advanced to what the remote says now"
    );
    assert_eq!(refresh.state, ImportedItemState::Closed);

    // The board reflects the refresh, not a second node.
    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .frontier
            .iter()
            .chain(board.settled.iter())
            .filter(|node| node.id == NodeId::Imported(first.id))
            .count()
            == 1,
        "one board row, advanced by the refresh"
    );
    assert!(
        board
            .settled
            .iter()
            .any(|node| node.id == NodeId::Imported(first.id)),
        "a now-closed issue is settled work after the refresh"
    );
}

/// A re-fetch of a remote that has not moved is refused: re-recording the same
/// revision would hide that the remote moved, so `apply_refresh` refuses
/// `SameRevision`. Nothing is recorded — the worst outcome here would be a
/// refresh that silently re-wrote the same state and looked like a change.
#[tokio::test]
async fn a_refetch_of_an_unchanged_remote_is_refused_and_records_nothing() {
    let server = serving(
        1,
        ResponseTemplate::new(200).set_body_json(issue_body(1, "open", "2026-08-06T10:00:00Z")),
    )
    .await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("first fetch");
    assert_eq!(fetched_items(&store, session).await.len(), 1);

    let error = runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect_err("the remote has not moved, so there is nothing to refresh");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error
            .to_string()
            .contains("would hide that the remote moved"),
        "the refusal must say why re-recording the same revision is refused: {error}"
    );

    // The log is unchanged: still one import, and no refresh was recorded.
    assert_eq!(fetched_items(&store, session).await.len(), 1);
    let refreshed = store
        .events(session)
        .await
        .expect("events")
        .into_iter()
        .filter(|stored| matches!(stored.event, MjolnrEvent::ImportedItemRefreshed { .. }))
        .count();
    assert_eq!(refreshed, 0, "a refused refresh records nothing");
}

/// A fetch that fails leaves the log exactly as it was. The failure mode this
/// rules out is the worst one available here: a half-recorded item the board
/// shows and the remote never confirmed.
#[tokio::test]
async fn a_failed_fetch_records_nothing_and_keeps_its_own_reason_code() {
    for (status, expected) in [
        (404, ReasonCode::WorkspaceCapabilityUnavailable),
        (401, ReasonCode::WorkspaceAuthRefused),
        (429, ReasonCode::ProviderRateLimit),
        (500, ReasonCode::ProviderRelay),
    ] {
        let server = serving(2, ResponseTemplate::new(status)).await;
        let (runtime, _temp, store, session) = open_project_with(&server).await;

        let error = runtime
            .dispatch(fetch("octocat/hello#2"))
            .await
            .expect_err("the fetch failed");
        assert_eq!(
            error.reason_code(),
            Some(expected),
            "a {status} must keep its own meaning through the runtime: {error}"
        );
        assert!(
            error.to_string().contains("nothing was recorded"),
            "the refusal must state what did not happen: {error}"
        );
        assert!(fetched_items(&store, session).await.is_empty());
    }
}

/// Contract (c), through the whole path: a state this version cannot interpret
/// is recorded as `Unknown` and never settles a board node. Recording it as
/// `Open` would turn "we did not learn" into "we checked and it is outstanding",
/// durably, where nothing later could tell the two apart.
#[tokio::test]
async fn a_state_the_producer_could_not_interpret_is_recorded_as_unknown_and_never_settles() {
    let server = serving(
        3,
        ResponseTemplate::new(200).set_body_json(issue_body(
            3,
            "some-state-from-the-future",
            "2026-08-06T11:00:00Z",
        )),
    )
    .await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    runtime
        .dispatch(fetch("octocat/hello#3"))
        .await
        .expect("an unreadable state is still a successful read");

    let items = fetched_items(&store, session).await;
    let item = items.first().expect("recorded");
    assert_eq!(item.state, ImportedItemState::Unknown);

    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .settled
            .iter()
            .all(|node| node.id != NodeId::Imported(item.id)),
        "an unknown state must never settle a node"
    );
    assert!(
        board
            .frontier
            .iter()
            .any(|node| node.id == NodeId::Imported(item.id)),
        "it is still work mjolnr knows about, so it stays on the board"
    );
}

#[tokio::test]
async fn a_malformed_id_and_an_integration_without_credentials_are_different_refusals() {
    let server = serving(1, ResponseTemplate::new(200)).await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    // Nothing addressable: a shape bug the caller fixes.
    let error = runtime
        .dispatch(fetch("not-an-id"))
        .await
        .expect_err("malformed");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));

    // Addressable and implemented, but not authenticated: not the caller's
    // schema mistake, and no request should leave the process.
    let error = runtime
        .dispatch(MjolnrCommand::FetchTask {
            source: "linear".to_owned(),
            task_id: "SIM-1".to_owned(),
        })
        .await
        .expect_err("linear has no credential");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceAuthRefused),
        "an unauthenticated integration is refused, not invalid: {error}"
    );
    assert!(
        error.to_string().contains("no credential"),
        "a refusal before the network must say so: {error}"
    );

    assert!(fetched_items(&store, session).await.is_empty());
}

#[tokio::test]
async fn a_batch_records_its_successful_prefix_and_stops_on_the_first_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "open",
            "2026-08-06T10:00:00Z",
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let (runtime, _temp, store, session) = open_project_with(&server).await;

    let error = runtime
        .dispatch(fetch_batch(&["octocat/hello#1", "octocat/hello#2"]))
        .await
        .expect_err("the second task is not present");

    assert!(
        error
            .to_string()
            .contains("batch stopped after 1 successful item")
    );
    let items = fetched_items(&store, session).await;
    assert_eq!(items.len(), 1);
    assert_eq!(
        items.first().map(|item| item.remote_id.as_str()),
        Some("octocat/hello#1")
    );
}

/// The injected source stands in only for the integration it names. Without
/// this, a test seam could quietly answer for an integration that has no
/// producer at all, and every "linear is unavailable" assertion would be
/// meaningless.
#[tokio::test]
async fn an_injected_source_does_not_answer_for_another_integration() {
    let server = serving(
        1,
        ResponseTemplate::new(200).set_body_json(issue_body(1, "open", "2026-08-06T10:00:00Z")),
    )
    .await;
    let (runtime, _temp, _store, _session) = open_project_with(&server).await;

    let error = runtime
        .dispatch(MjolnrCommand::FetchTask {
            source: "linear".to_owned(),
            task_id: "octocat/hello#1".to_owned(),
        })
        .await
        .expect_err("the github source must not answer as linear");
    assert_eq!(error.reason_code(), Some(ReasonCode::WorkspaceAuthRefused));
}

/// A fetched item survives a restart, because it is in the log and not only in
/// the session. The board after a resume is rebuilt from the same events, and a
/// re-fetch routes to the same refresh guard the live path used.
#[tokio::test]
async fn a_fetched_item_is_rebuilt_from_the_log_after_a_restart() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "closed",
            "2026-08-06T10:00:00Z",
        )))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let (runtime, temp, store, session) = open_project_with(&server).await;
    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("fetched");
    let item = fetched_items(&store, session)
        .await
        .into_iter()
        .next()
        .expect("recorded")
        .clone();
    runtime.close().await.expect("close");

    // The remote moved while mjolnr was down. The resumed session rebuilds the
    // item from the log, so the re-fetch finds it already imported and routes
    // to a refresh — pinned to the revision the log holds, decided by the same
    // `apply_refresh` the live path used.
    Mock::given(method("GET"))
        .and(path("/repos/octocat/hello/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_body(
            1,
            "closed",
            "2026-08-06T12:00:00Z",
        )))
        .mount(&server)
        .await;
    let producer = GitHubSource::new(mjolnr::core::secrets::Secret::new(TOKEN.to_owned()))
        .with_base_url(server.uri());
    let runtime = Runtime::spawn_with_task_source(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(producer) as Arc<dyn TaskSource>,
    );
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle_until(&runtime, |snap| snap.session.is_some()).await;

    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .settled
            .iter()
            .any(|node| node.id == NodeId::Imported(item.id)),
        "a closed issue is settled work, and the restart rebuilds it as such"
    );

    runtime
        .dispatch(fetch("octocat/hello#1"))
        .await
        .expect("the re-fetch becomes a pinned refresh across the restart");

    let refreshed: Vec<ImportedItem> = store
        .events(session)
        .await
        .expect("events")
        .into_iter()
        .filter_map(|stored| match stored.event {
            MjolnrEvent::ImportedItemRefreshed { item, .. } => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(refreshed.len(), 1, "one refresh, not a second import");
    let refresh = refreshed.first().expect("the refreshed item");
    assert_eq!(refresh.id, item.id, "same board row across the restart");
    assert_eq!(
        refresh.fetched_revision, "2026-08-06T12:00:00Z",
        "the recorded item advanced to the revision the moved remote reports"
    );
}

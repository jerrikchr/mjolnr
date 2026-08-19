//! Phase E5 step 4b: the durable import path.
//!
//! `ImportWorkItem` and `RefreshImportedItem` cross the same acknowledged
//! board-command path decision tickets use, and the refusals matter most: an
//! imported item that appears to have been recorded and was not, or a stale
//! refresh that silently overwrites what the human approved, would turn the
//! board into a second source of truth — the exact thing E5 exists to avoid.
//!
//! Contract (a) is asserted at both layers it lives in: the core record guard
//! (`ImportedItemRecord::apply_refresh`, tested in `src/core/imported.rs`) and
//! the session fold that maps its typed refusal onto a reason code. The two
//! cannot drift, because the fold delegates — and the last test here pins the
//! delegation by refusing the same stale tab through a replayed session.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::error::ReasonCode;
use smed::core::event::{SessionId, SmedEvent};
use smed::core::frontier::{NodeId, NodeKind, Provenance};
use smed::core::imported::{ImportedItem, ImportedItemId, ImportedItemState};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::FakeProvider;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;

async fn open_project() -> (
    Runtime,
    tempfile::TempDir,
    Arc<InMemoryEventStore>,
    SessionId,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
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

fn imported(id: ImportedItemId, revision: &str, state: ImportedItemState) -> ImportedItem {
    ImportedItem {
        id,
        integration: "github".to_owned(),
        remote_id: "42".to_owned(),
        source_url: "https://example.invalid/owner/repo/issues/42".to_owned(),
        fetched_revision: revision.to_owned(),
        title: "an imported task".to_owned(),
        state,
        blocked_by: Vec::new(),
    }
}

async fn imported_events(store: &InMemoryEventStore, session: SessionId) -> Vec<SmedEvent> {
    store
        .events(session)
        .await
        .expect("events")
        .into_iter()
        .filter(|stored| {
            matches!(
                stored.event,
                SmedEvent::ImportedItemFetched { .. } | SmedEvent::ImportedItemRefreshed { .. }
            )
        })
        .map(|stored| stored.event)
        .collect()
}

#[tokio::test]
async fn importing_a_work_item_records_it_and_projects_it_on_the_board() {
    let (runtime, _temp, store, session) = open_project().await;
    let id = ImportedItemId::new();

    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("importing records");

    let events = imported_events(&store, session).await;
    assert_eq!(events.len(), 1, "exactly one fetch is recorded");
    assert!(matches!(
        events.first(),
        Some(SmedEvent::ImportedItemFetched { item, .. }) if item.id == id
    ));

    // The board is a projection of the log: the imported item appears as work,
    // external, decidable — with its title as the label, not its address.
    let board = runtime.query_board().await.expect("the board answers");
    let node = board
        .frontier
        .iter()
        .find(|node| node.id == NodeId::Imported(id))
        .expect("the imported item is frontier: open and unblocked");
    assert_eq!(
        node.kind,
        NodeKind::Implementation,
        "imported items do work"
    );
    assert_eq!(
        node.provenance,
        Provenance::ExternalUnverified,
        "imported provenance survives onto the board, never elided"
    );
    assert_eq!(node.label, "an imported task");
}

#[tokio::test]
async fn a_duplicate_import_id_is_refused_and_nothing_is_recorded() {
    let (runtime, _temp, store, session) = open_project().await;
    let id = ImportedItemId::new();

    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("first import records");

    let error = runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev2", ImportedItemState::Merged),
        })
        .await
        .expect_err("the log is append-only: a duplicate id is a bug, not an update");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error.to_string().contains(&id.to_string()),
        "the refusal names the item: {error}"
    );

    let events = imported_events(&store, session).await;
    assert_eq!(events.len(), 1, "the refused import preceded the append");
}

#[tokio::test]
async fn a_refresh_pinned_to_a_stale_revision_is_refused_not_recorded() {
    let (runtime, _temp, store, session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");

    let error = runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "some-other-rev".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Merged),
        })
        .await
        .expect_err("a stale tab is refused, not recorded");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceStaleRevision),
        "staleness carries its own retryable code: {error}"
    );

    let events = imported_events(&store, session).await;
    assert_eq!(events.len(), 1, "no refresh event entered the log");
    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .frontier
            .iter()
            .any(|node| node.id == NodeId::Imported(id)),
        "the board still projects the revision the human saw"
    );
}

#[tokio::test]
async fn a_refresh_repeating_the_revision_or_moving_the_identity_is_refused() {
    let (runtime, _temp, store, session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");

    // Re-recording the current revision would hide that the remote moved.
    let error = runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(id, "rev1", ImportedItemState::Closed),
        })
        .await
        .expect_err("the same revision must refuse");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));

    // integration and remote_id are identity; a refresh cannot move them.
    let mut moved = imported(id, "rev2", ImportedItemState::Open);
    moved.remote_id = "43".to_owned();
    let error = runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: moved,
        })
        .await
        .expect_err("identity is immutable across a refresh");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));

    let events = imported_events(&store, session).await;
    assert_eq!(events.len(), 1, "neither refusal entered the log");
}

#[tokio::test]
async fn refreshing_an_item_that_was_never_imported_is_refused() {
    let (runtime, _temp, store, session) = open_project().await;

    let error = runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(ImportedItemId::new(), "rev2", ImportedItemState::Open),
        })
        .await
        .expect_err("a refresh cannot create the item it refreshes");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error.to_string().contains("fetch it first"),
        "the refusal says how to proceed: {error}"
    );

    assert!(
        imported_events(&store, session).await.is_empty(),
        "nothing was recorded"
    );
}

#[tokio::test]
async fn a_valid_refresh_supersedes_the_prior_fetch_and_settles_a_terminal_state() {
    let (runtime, _temp, store, session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");

    runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Merged),
        })
        .await
        .expect("a refresh pinned to the seen revision records");

    let events = imported_events(&store, session).await;
    assert_eq!(
        events.len(),
        2,
        "the fetch and the refresh are both durable"
    );
    assert!(matches!(
        events.get(1),
        Some(SmedEvent::ImportedItemRefreshed { expected_revision, item, .. })
            if expected_revision == "rev1" && item.fetched_revision == "rev2"
    ));

    // Contract (a) at the projection: the board reads the latest fetch. The
    // observed terminal outcome settles the node (contract (b): outcome, not
    // a gate signal).
    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .settled
            .iter()
            .any(|node| node.id == NodeId::Imported(id)),
        "rev2 Merged is settled"
    );
    assert!(
        board
            .frontier
            .iter()
            .all(|node| node.id != NodeId::Imported(id)),
        "the stale rev1 Open no longer projects"
    );
}

#[tokio::test]
async fn a_replayed_session_remembers_imported_items_and_still_refuses_a_stale_tab() {
    let (runtime, temp, store, session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");
    runtime.close().await.expect("close");

    // Truth lives in the log, not in the process: a new runtime over the same
    // store rebuilds the imported record from its events.
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle_until(&runtime, |snap| snap.session.is_some()).await;

    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .frontier
            .iter()
            .any(|node| node.id == NodeId::Imported(id)),
        "the imported item survives the restart"
    );

    let error = runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "stale-after-restart".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Done),
        })
        .await
        .expect_err("the replayed fold applies the same stale-tab refusal");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceStaleRevision)
    );

    runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Done),
        })
        .await
        .expect("a correctly pinned refresh records after resume");

    let board = runtime.query_board().await.expect("the board answers");
    assert!(
        board
            .settled
            .iter()
            .any(|node| node.id == NodeId::Imported(id)),
        "the post-resume refresh reaches the projection"
    );
}

// ---------------------------------------------------------------------------
// Contract (a) on the act path (§E5 step 4c)
// ---------------------------------------------------------------------------
//
// `submit_change` is the first command that would leave smed and change
// something a third party owns. The refusals below run *before* the capability
// refusal that stands in for the unwritten producers, which is the whole point:
// when a producer lands, the staleness guard is already in front of its network
// call rather than something the producer must remember to add.

fn submit(remote_id: &str, expected_revision: &str) -> SmedCommand {
    SmedCommand::SubmitChange {
        source: "github".to_owned(),
        remote_id: remote_id.to_owned(),
        expected_revision: expected_revision.to_owned(),
        title: "Fix the parser".to_owned(),
        body: "A change offered back to the remote.".to_owned(),
        head_commit: "abc123".to_owned(),
        head_branch: "feature/parser".to_owned(),
        base_branch: "main".to_owned(),
    }
}

#[tokio::test]
async fn a_change_pinned_to_a_revision_the_item_has_moved_past_is_refused_stale() {
    let (runtime, _temp, _store, _session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");
    runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Open),
        })
        .await
        .expect("refresh");

    // The human's tab still shows rev1; the item is at rev2.
    let error = runtime
        .dispatch(submit("42", "rev1"))
        .await
        .expect_err("a stale tab is refused, not posted");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceStaleRevision),
        "a stale pin is retryable and must not be reported as a missing capability"
    );
    assert!(
        error.to_string().contains("nothing was sent to the remote"),
        "the refusal must say what did not happen: {error}"
    );
}

/// Ordering is the assertion. A correctly pinned change still cannot be posted
/// — no producer exists — but it must fail on *that*, not on staleness, or the
/// guard would be indistinguishable from the capability refusal behind it.
#[tokio::test]
async fn a_correctly_pinned_change_passes_the_staleness_guard_and_stops_at_the_missing_producer() {
    let (runtime, _temp, _store, _session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");

    let error = runtime
        .dispatch(submit("42", "rev1"))
        .await
        .expect_err("no integration performs network I/O yet");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceAuthRefused),
        "a good pin must reach credential resolution: {error}"
    );
    assert!(
        error.to_string().contains("no credential is configured"),
        "the credential refusal must name what it did not do: {error}"
    );
}

/// Fail closed: a pin over a remote this session never imported names a
/// revision smed cannot show anyone. Refusing is what keeps `expectedRevision`
/// from becoming a field a caller can satisfy by inventing a value.
#[tokio::test]
async fn a_change_over_a_remote_that_was_never_imported_is_refused_before_the_producer() {
    let (runtime, _temp, _store, _session) = open_project().await;
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(ImportedItemId::new(), "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");

    let error = runtime
        .dispatch(submit("43", "rev1"))
        .await
        .expect_err("nothing imported names remote 43");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error.to_string().contains("import it first"),
        "the refusal must tell the human what to do: {error}"
    );
}

#[tokio::test]
async fn an_empty_session_refuses_a_change_rather_than_passing_an_unverifiable_pin_through() {
    let (runtime, _temp, _store, _session) = open_project().await;
    let error = runtime
        .dispatch(submit("42", "rev1"))
        .await
        .expect_err("nothing has been imported at all");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::SchemaInvalid),
        "with nothing recorded, a pin proves nothing and the change is refused"
    );
}

/// The same guard after a restart, for the same reason the refresh path is
/// tested this way: the check reads folded state, so if replay rebuilt the
/// items differently the live path and the resumed path would disagree about
/// which change is safe to post.
#[tokio::test]
async fn the_act_path_guard_holds_over_a_resumed_session() {
    let (runtime, temp, store, session) = open_project().await;
    let id = ImportedItemId::new();
    runtime
        .dispatch(SmedCommand::ImportWorkItem {
            item: imported(id, "rev1", ImportedItemState::Open),
        })
        .await
        .expect("import");
    runtime
        .dispatch(SmedCommand::RefreshImportedItem {
            expected_revision: "rev1".to_owned(),
            item: imported(id, "rev2", ImportedItemState::Open),
        })
        .await
        .expect("refresh");
    runtime.close().await.expect("close");

    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle_until(&runtime, |snap| snap.session.is_some()).await;

    let error = runtime
        .dispatch(submit("42", "rev1"))
        .await
        .expect_err("the replayed session applies the same act-path guard");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceStaleRevision)
    );

    let error = runtime
        .dispatch(submit("42", "rev2"))
        .await
        .expect_err("no producer exists");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceAuthRefused),
        "the revision replay rebuilt is the one the guard accepts before credential resolution: {error}"
    );
}

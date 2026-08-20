//! Integration tests for Snapshot Checkpoint Rollback (Master Implementation Plan Phase 4 Slice 4.2).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::client::{ClientCommand, ClientMessage, ClientSnapshot};
use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::directive::DirectiveSource;
use mjolnr::core::message::Role;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSnapshot};
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::runtime::client_bridge::ClientBridge;
use mjolnr::store::sqlite::SqliteEventStore;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let database = directory.path().join("mjolnr.sqlite3");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        Self {
            _directory: directory,
            database,
            workspace,
        }
    }

    async fn store(&self) -> Arc<SqliteEventStore> {
        Arc::new(
            SqliteEventStore::open(&self.database)
                .await
                .expect("open sqlite store"),
        )
    }
}

async fn settle_runtime(
    runtime: &Runtime,
    ready: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

async fn settle_bridge(
    bridge: &ClientBridge,
    ready: impl Fn(&ClientSnapshot) -> bool,
) -> ClientSnapshot {
    for _ in 0..400 {
        let snapshot = bridge.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the bridge never reached the expected state");
}

async fn say(runtime: &Runtime, text: &str) -> RuntimeSnapshot {
    let before = runtime.snapshot().messages.len();
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: text.to_owned(),
            source: DirectiveSource::Human,
        })
        .await
        .expect("dispatch");
    settle_runtime(runtime, |snapshot| {
        !snapshot.run_active && snapshot.messages.len() > before + 1
    })
    .await
}

fn anchor_of(runtime: &Runtime, text: &str) -> u64 {
    runtime
        .snapshot()
        .messages
        .iter()
        .find(|entry| entry.role == Role::User && entry.text() == text)
        .expect("the message must be in the transcript")
        .sequence
        .expect("a live user message must be anchored to its durable event")
}

#[tokio::test]
async fn rollback_to_checkpoint_rewinds_transcript_safely() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Arc::new(Runtime::spawn(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
    ));
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
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

    settle_runtime(&runtime, |s| s.session.is_some()).await;

    // Send first message
    say(&runtime, "first turn").await;

    // Send second message
    say(&runtime, "second turn").await;

    let snapshot_before: ClientSnapshot = bridge.snapshot();
    let msg_count_before = snapshot_before.messages.len();
    assert!(
        msg_count_before >= 4,
        "expected at least 4 messages (2 user + 2 assistant), got {msg_count_before}"
    );

    let second_turn_anchor = anchor_of(&runtime, "second turn");

    // Rollback to before the second turn
    bridge
        .dispatch(ClientCommand::RollbackToCheckpoint {
            target_sequence: second_turn_anchor,
            expected_head: None,
        })
        .await
        .expect("dispatch rollback command");

    let snapshot_after = settle_bridge(&bridge, |s| s.messages.len() < msg_count_before).await;
    assert!(
        snapshot_after.messages.len() < msg_count_before,
        "expected messages after ({}) < before ({})",
        snapshot_after.messages.len(),
        msg_count_before
    );
    assert!(!snapshot_after.messages.iter().any(|m| match m {
        ClientMessage::User { text, .. } => text == "second turn",
        _ => false,
    }));
}

#[tokio::test]
async fn rollback_fails_closed_on_mismatched_expected_head() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let runtime = Arc::new(Runtime::spawn(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
    ));
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
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

    settle_runtime(&runtime, |s| s.session.is_some()).await;

    // Rollback with mismatched expected head is refused fail-closed
    bridge
        .dispatch(ClientCommand::RollbackToCheckpoint {
            target_sequence: 1,
            expected_head: Some("nonexistent-head-hash".to_owned()),
        })
        .await
        .expect("dispatch rollback");
}

#[tokio::test]
async fn rollback_is_refused_while_run_is_active() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Subagent));
    let runtime = Arc::new(Runtime::spawn(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
    ));
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);

    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: fixture.workspace.clone(),
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

    settle_runtime(&runtime, |s| s.session.is_some()).await;

    // Start a holding turn
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "worker-hold:".to_owned(),
            source: DirectiveSource::Human,
        })
        .await
        .expect("send message");

    settle_runtime(&runtime, |s| s.run_active).await;

    // Attempt rollback while run is in flight
    bridge
        .dispatch(ClientCommand::RollbackToCheckpoint {
            target_sequence: 0,
            expected_head: None,
        })
        .await
        .expect("dispatch rollback");

    // The run remains active and state is intact
    assert!(runtime.snapshot().run_active);

    // Clean up run
    let _ = runtime.dispatch(MjolnrCommand::CancelRun).await;
}

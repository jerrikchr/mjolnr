//! Phase 12 end-to-end headless host contracts.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::event::SessionId;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::policy::PolicyMode;
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::EventStore;
use mjolnr::headless::{EXIT_REFUSED, EXIT_VERIFIED, HeadlessOutcome};
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::store::sqlite::SqliteEventStore;

async fn ready(runtime: &Runtime, policy: PolicyMode) {
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = runtime.snapshot();
            if snapshot.session.is_some() && snapshot.policy == policy {
                return;
            }
            snapshots.changed().await.expect("runtime snapshot");
        }
    })
    .await
    .expect("runtime ready");
}

async fn configured_runtime(
    workspace: &std::path::Path,
    store: Arc<SqliteEventStore>,
    policy: PolicyMode,
) -> Runtime {
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::GuardedLoop));
    let runtime = Runtime::spawn(vec![provider], store as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.to_path_buf(),
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
    runtime
        .dispatch(MjolnrCommand::SetPolicy { mode: policy })
        .await
        .expect("policy");
    ready(&runtime, policy).await;
    runtime
}

#[tokio::test]
async fn full_auto_headless_run_is_verified_durable_and_resumable() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("fixture.txt"), "before\n").expect("fixture");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(workspace.path())
        .status()
        .expect("git init");
    assert!(status.success());
    let database = workspace.path().join("mjolnr.db");
    let store = Arc::new(SqliteEventStore::open(&database).await.expect("store"));
    let runtime =
        configured_runtime(workspace.path(), Arc::clone(&store), PolicyMode::FullAuto).await;

    let report = mjolnr::headless::run(&runtime, "update the fixture".to_owned())
        .await
        .expect("headless run");
    assert_eq!(report.outcome, HeadlessOutcome::Verified);
    assert_eq!(report.exit_code, EXIT_VERIFIED);
    let session = uuid::Uuid::parse_str(&report.session_id)
        .map(SessionId::from_uuid)
        .expect("session id");
    runtime.close().await.expect("close");
    assert!(store.events(session).await.expect("events").len() > 10);
    drop(store);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resumed_provider: Arc<dyn Provider> = Arc::new(FakeProvider::default());
    let resumed_store = Arc::new(
        SqliteEventStore::open(&database)
            .await
            .expect("reopen store"),
    );
    let resumed = Runtime::spawn(
        vec![resumed_provider],
        Arc::clone(&resumed_store) as Arc<dyn EventStore>,
    );
    resumed
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_path_buf(),
        })
        .await
        .expect("open resumed project");
    resumed
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");
    let mut snapshots = resumed.snapshots();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if resumed.snapshot().messages.len() > 4 {
                return;
            }
            snapshots.changed().await.expect("resumed snapshot");
        }
    })
    .await
    .expect("history restored");
    resumed.close().await.expect("close resumed");
    drop(resumed_store);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn approval_dependent_headless_step_refuses_instead_of_hanging() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("fixture.txt"), "before\n").expect("fixture");
    let store = Arc::new(
        SqliteEventStore::open(&workspace.path().join("mjolnr.db"))
            .await
            .expect("store"),
    );
    let runtime = configured_runtime(workspace.path(), store, PolicyMode::WorkspaceWrite).await;
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        mjolnr::headless::run(&runtime, "update the fixture".to_owned()),
    )
    .await
    .expect("headless must not hang")
    .expect("headless run");
    assert_eq!(report.outcome, HeadlessOutcome::Refused);
    assert_eq!(report.exit_code, EXIT_REFUSED);
    assert_eq!(report.reason_code.as_deref(), Some("APPROVAL_DENIED"));
    runtime.close().await.expect("close");
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[test]
fn cli_without_a_tty_emits_one_json_line_and_leaves_a_listed_session() {
    let fixture = tempfile::tempdir().expect("fixture");
    let data = fixture.path().join("data");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mjolnr"))
        .args(["--data-dir"])
        .arg(&data)
        .args([
            "exec",
            "inspect the repository",
            "--provider",
            "ollama",
            "--model",
            "llama3.2",
        ])
        .current_dir(fixture.path())
        .output()
        .expect("headless process");
    assert!(
        !output.status.success(),
        "an unavailable local provider must not be success"
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout must be one NDJSON record"
    );
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON report");
    let session = report["session_id"].as_str().expect("session id");
    assert_eq!(
        report["exit_code"].as_i64(),
        output.status.code().map(i64::from)
    );

    let sessions = std::process::Command::new(env!("CARGO_BIN_EXE_mjolnr"))
        .args(["--data-dir"])
        .arg(&data)
        .args(["sessions", "list"])
        .output()
        .expect("sessions process");
    assert!(sessions.status.success());
    assert!(
        String::from_utf8(sessions.stdout)
            .expect("sessions utf8")
            .contains(session)
    );
}

#[test]
fn the_removed_fake_provider_is_refused_before_a_session_starts() {
    let fixture = tempfile::tempdir().expect("fixture");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mjolnr"))
        .args(["--data-dir"])
        .arg(fixture.path().join("data"))
        .args(["exec", "reply", "--provider", "fake", "--model", "fake-1"])
        .current_dir(fixture.path())
        .output()
        .expect("headless process");

    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON report");
    assert_eq!(
        report["reason_code"].as_str(),
        Some("PROVIDER_INCOMPATIBLE_MODEL")
    );
    assert_eq!(report["session_id"].as_str(), Some("unknown"));
}

//! Proof that the database holds no secrets (`AGENTS.md` §3).
//!
//! # Why this scans raw bytes
//!
//! Every other persistence test asks the store questions and checks the answers.
//! This one does not trust the store's answers at all: it drives a session and
//! then reads the file off disk, byte by byte.
//!
//! That is the only form of the assertion that would survive the bug it exists
//! to catch. A leak arrives through a path nobody remembered — a `Debug` impl, a
//! serialised environment, an error string quoting a request body — and every
//! one of those is invisible to a test that queries the schema it knows about.
//! The file is the boundary. `AGENTS.md` §1.5: "Secrets never leave their
//! boundary. Not into logs, argv, SQLite, `Debug` output, panics, fixtures,
//! child environments, or the screen."
//!
//! # What this proves, and what it does not
//!
//! It proves no credential-shaped data is in the file after a session that
//! exercises messages, tool results, errors, and a checkpoint.
//!
//! It does **not** prove a live OpenAI credential stays out, because no live
//! call runs here. The structural argument for that is stronger than a test
//! anyway: the runtime never holds a credential — only the provider adapter
//! does, and [`EventPayload`](smed) has no field one could occupy. This scan is
//! the belt to that braces, and it is what would catch a `Debug` leak that the
//! structure did not anticipate.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::PathBuf;
use std::sync::Arc;

use smed::core::command::{ApprovalDecision, ApprovalId, SmedCommand};
use smed::core::event::{RunId, SessionId, SmedEvent};
use smed::core::message::{CanonicalMessage, ToolEffect, ToolResult};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::secrets::{Secret, environment_variable};
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::sqlite::SqliteEventStore;
use tempfile::TempDir;

/// The shapes a leaked credential or environment snapshot takes.
///
/// Not a sentinel injected through `set_var`: mutating the process environment
/// is `unsafe` in edition 2024, and `unsafe` is forbidden here — including in
/// tests (`AGENTS.md` §3). These are the strings that would actually be present
/// if any of the known leak paths were open, which is what a scan can honestly
/// check for.
const FORBIDDEN: &[(&str, &str)] = &[
    ("Authorization", "an authorization header name"),
    ("Bearer ", "a bearer token"),
    ("OPENAI_API_KEY", "a provider key environment name"),
    ("_API_KEY", "any provider key environment name"),
    ("api_key", "a credential field"),
    ("PATH=", "a raw environment snapshot"),
    ("HOME=", "a raw environment snapshot"),
    ("exact_commands", "an approval grant"),
];

/// Every file SQLite may write for one database.
///
/// The `-wal` file is the one a naive check forgets: a committed transaction can
/// live there long before it reaches the main file, so scanning only
/// `smed.sqlite3` would read a database that has not been written yet.
fn database_files(database: &std::path::Path) -> Vec<PathBuf> {
    let mut files = vec![database.to_path_buf()];
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        let path = PathBuf::from(name);
        if path.exists() {
            files.push(path);
        }
    }
    files
}

fn assert_clean(database: &std::path::Path) {
    let files = database_files(database);
    assert!(
        files
            .iter()
            .any(|file| { std::fs::metadata(file).is_ok_and(|metadata| metadata.len() > 0) }),
        "the scan found nothing to read — it would pass on an empty database, \
         which would make it worthless"
    );

    for file in files {
        let bytes = std::fs::read(&file).expect("read database file");
        for (needle, what) in FORBIDDEN {
            let found = bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes());
            assert!(
                !found,
                "{what} (`{needle}`) reached {} — secrets never enter SQLite (AGENTS.md §3)",
                file.display()
            );
        }
    }
}

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

fn fixture() -> Fixture {
    let directory = TempDir::new().expect("temp dir");
    let database = directory.path().join("smed.sqlite3");
    let workspace = directory
        .path()
        .canonicalize()
        .expect("canonical")
        .join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    Fixture {
        _directory: directory,
        database,
        workspace,
    }
}

#[tokio::test]
async fn a_credential_never_renders_itself() {
    // The mechanism the file scan depends on. If `Secret` ever derived `Debug`,
    // one `tracing` call or one error string would carry a key into the database
    // and every structural argument above would be void.
    let secret = Secret::new("sk-smed-test-DO-NOT-PERSIST-4a9f2c8e1b7d".to_owned());

    let rendered = format!("{secret:?}");
    assert_eq!(rendered, "Secret(<redacted>)");
    assert!(!rendered.contains("sk-smed"));
    assert!(
        !rendered.contains("40") && !rendered.contains("41"),
        "not even the length: that is information about the credential"
    );
}

#[tokio::test]
async fn a_session_driven_end_to_end_leaves_no_credential_shaped_data() {
    let fixture = fixture();
    let store = Arc::new(
        SqliteEventStore::open(&fixture.database)
            .await
            .expect("open"),
    );

    {
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
        let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
        runtime
            .dispatch(SmedCommand::OpenProject {
                root: fixture.workspace.clone(),
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
        runtime
            .dispatch(SmedCommand::SendUserMessage {
                text: "hello".to_owned(),
                source: smed::core::directive::DirectiveSource::Human,
            })
            .await
            .expect("send");

        for _ in 0..400 {
            if runtime.snapshot().messages.len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        runtime.close().await.expect("close");
    }

    // Close the connection so SQLite checkpoints the WAL into the main file. The
    // scan covers both regardless, but this is the state a user's disk is left
    // in, so it is the state worth checking.
    drop(store);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_clean(&fixture.database);
}

#[tokio::test]
async fn an_approval_grant_is_never_written_to_disk() {
    //  scopes an exact-command grant to one session. The core type has no
    // field for one and the wire format has no field for one; this checks the
    // file, which is the only place the claim can actually be falsified.
    let fixture = fixture();
    let store = SqliteEventStore::open(&fixture.database)
        .await
        .expect("open");
    let project = store
        .open_project(fixture.workspace.clone())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "test".to_owned(), None)
        .await
        .expect("session");

    let run = RunId::new();
    let approval = ApprovalId::new();
    for event in [
        SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        },
        SmedEvent::RunStarted { session, run },
        // A human granting the broadest authority smed can express.
        SmedEvent::ApprovalResolved {
            session,
            run,
            approval,
            decision: ApprovalDecision::ApproveExactForSession,
        },
        SmedEvent::ToolCompleted {
            session,
            run,
            call_id: "call_1".to_owned(),
            name: "run_command".to_owned(),
            result: ToolResult::ok("ok").with_effect(ToolEffect::Command {
                exit_code: Some(0),
                success: true,
                duration_ms: 1,
            }),
        },
    ] {
        store.append(event).await.expect("append");
    }

    // A checkpoint is where a grant would leak if `SessionCheckpoint` ever grew a
    // field for one, so one is written before the scan.
    store
        .write_checkpoint(smed::core::checkpoint::SessionCheckpoint {
            project_root: Some(fixture.workspace.clone()),
            messages: vec![CanonicalMessage::user("run it")],
            ..smed::core::checkpoint::SessionCheckpoint::empty(session)
        })
        .await
        .expect("checkpoint");

    drop(store);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // The `ApprovalResolved` *event* is durable on purpose — it is audit, and it
    // records that a human said yes. What must not exist is a stored **grant**:
    // state that would re-authorise the command after a restart.
    assert_clean(&fixture.database);
}

#[tokio::test]
async fn the_environment_variable_name_this_scan_looks_for_is_the_real_one() {
    // Guards the scan itself. If `environment_variable` changed shape, the list
    // above would be searching for a string smed no longer uses, and the test
    // would pass by looking in the wrong place.
    let variable = environment_variable(&ProviderId::new("openai"));
    assert_eq!(variable, "OPENAI_API_KEY");
    assert!(
        FORBIDDEN.iter().any(|(needle, _)| *needle == variable),
        "the scan must look for the variable smed actually reads"
    );
}

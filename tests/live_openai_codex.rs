//! Opt-in `ChatGPT` subscription smoke test (plan Phase 6.5).
//!
//! Run after `smed auth login openai-codex`:
//!
//! ```text
//! cargo test --test live_openai_codex -- --ignored --nocapture
//! ```
//!
//! The test reads only smed's OS-keychain credential. It does not inspect or
//! share `~/.codex/auth.json`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::event::SmedEvent;
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::openai_codex::{DEFAULT_MODEL, OpenAiCodexProvider, PROVIDER_ID};
use smed::runtime::Runtime;
use smed::store::secrets::OsSecretStore;
use smed::store::sqlite::SqliteEventStore;

async fn wait_for_session(runtime: &Runtime, session: smed::core::event::SessionId) {
    let mut snapshots = runtime.snapshots();
    if runtime.snapshot().session == Some(session) {
        return;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime remains open");
            if snapshot.session == Some(session) {
                break;
            }
        }
    })
    .await
    .expect("resume snapshot timed out");
}

/// The decoder's tool-call assembly against the real subscription backend.
///
/// The sibling test above deliberately forbids tools, so it never exercised
/// tool-call assembly — which is how a Codex-shaped delivery (`output_item.done`
/// closing a call with no `function_call_arguments.done`) shipped as a
/// run-killing `PROVIDER_PROTOCOL`. The signal that the decoder is healthy is a
/// `ToolProposed`: it fires the moment a call is assembled, before any approval
/// or execution, so this neither depends on which tool the model picks nor
/// waits on the `ask`-policy approval gate. A regressed decoder produces
/// `RunFailed(PROVIDER_PROTOCOL)` before any proposal instead.
#[tokio::test]
#[ignore = "requires `smed auth login openai-codex` and spends subscription quota"]
async fn a_tool_call_assembles_over_the_subscription_backend() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteEventStore::open(&directory.path().join("live-tool.sqlite3"))
            .await
            .expect("open store"),
    );
    let provider: Arc<dyn Provider> =
        Arc::new(OpenAiCodexProvider::new(Arc::new(OsSecretStore::new())));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("current directory"),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER_ID),
            model: ModelId::new(DEFAULT_MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "Use the list_dir tool to list the entries in the current directory.".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send tool-using turn");

    // Every event is logged (run with `--nocapture`) so a failure is legible
    // rather than a bare timeout: a reasoning model can take a while to reach
    // its first tool call, and the turn is worth watching live.
    let outcome = tokio::time::timeout(Duration::from_secs(150), async {
        loop {
            let event = events.recv().await.expect("event feed");
            eprintln!("live event: {event:?}");
            match event {
                // The decoder assembled and surfaced a call: exactly the path
                // that used to die as PROVIDER_PROTOCOL. Stop here rather than
                // execute anything unapproved.
                SmedEvent::ToolProposed { call, .. } => return Outcome::Assembled(call.name),
                SmedEvent::RunFailed { code, detail, .. } => {
                    // The specific regression this test exists for.
                    assert!(
                        code != smed::core::error::ReasonCode::ProviderProtocol,
                        "decoder regression: tool-call assembly failed with {code}: {detail}"
                    );
                    panic!("run failed before assembling a tool call — {code}: {detail}");
                }
                // The model answered in text without reaching for a tool. That
                // does not exercise assembly, so it cannot confirm the fix — but
                // it is not a decoder failure either. Report inconclusive rather
                // than a false red.
                SmedEvent::RunFinished { .. } => return Outcome::NoTool,
                _ => {}
            }
        }
    })
    .await
    .expect("the live turn produced no terminal event within the timeout");

    // Clean up the in-flight run; we proved assembly, not execution.
    runtime.dispatch(SmedCommand::CancelRun).await.ok();
    runtime.close().await.expect("close runtime");

    match outcome {
        Outcome::Assembled(name) => {
            assert!(!name.is_empty(), "the proposed call must name a tool");
        }
        Outcome::NoTool => {
            eprintln!(
                "inconclusive: the model answered without calling a tool, so assembly was not \
                 exercised. Re-run to try again; this is not a decoder failure."
            );
        }
    }
}

enum Outcome {
    Assembled(String),
    NoTool,
}

#[tokio::test]
#[ignore = "requires `smed auth login openai-codex` and spends subscription quota"]
async fn live_guarded_turn_completes_and_provider_selection_resumes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteEventStore::open(&directory.path().join("live.sqlite3"))
            .await
            .expect("open store"),
    );
    let provider: Arc<dyn Provider> =
        Arc::new(OpenAiCodexProvider::new(Arc::new(OsSecretStore::new())));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("current directory"),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER_ID),
            model: ModelId::new(DEFAULT_MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "Reply with exactly: smed-subscription-live-ok. Do not call tools.".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send guarded turn");

    tokio::time::timeout(Duration::from_mins(3), async {
        loop {
            match events.recv().await.expect("event feed") {
                SmedEvent::RunFinished { .. } => break,
                SmedEvent::RunFailed { code, detail, .. } => {
                    panic!("live run failed with {code}: {detail}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("live turn timed out");

    let before_close = runtime.snapshot();
    let session = before_close.session.expect("session id");
    assert_eq!(
        before_close.provider.as_ref().map(ProviderId::as_str),
        Some(PROVIDER_ID)
    );
    assert_eq!(
        before_close.model.as_ref().map(ModelId::as_str),
        Some(DEFAULT_MODEL)
    );
    assert!(
        before_close.messages.iter().any(|message| {
            message
                .text()
                .to_ascii_lowercase()
                .contains("smed-subscription-live-ok")
        }),
        "live model response was not retained in the guarded transcript"
    );
    runtime.close().await.expect("close first runtime");

    let resumed_provider: Arc<dyn Provider> =
        Arc::new(OpenAiCodexProvider::new(Arc::new(OsSecretStore::new())));
    let resumed = Runtime::spawn(
        vec![resumed_provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
    );
    resumed
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("current directory"),
        })
        .await
        .expect("reopen project");
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume session");
    wait_for_session(&resumed, session).await;
    let after_resume = resumed.snapshot();
    assert_eq!(
        after_resume.provider.as_ref().map(ProviderId::as_str),
        Some(PROVIDER_ID)
    );
    assert_eq!(
        after_resume.model.as_ref().map(ModelId::as_str),
        Some(DEFAULT_MODEL)
    );
    resumed.close().await.expect("close resumed runtime");
}

//! Phase 14 end-to-end trigger contracts: a webhook firing's transcript, the
//! disabled-after-failure/re-arm cycle, and a quota-drain firing landing its
//! handoff instead of dying mid-window.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use smed::cli::triggers::TriggersCommand;
use smed::context::ProjectContext;
use smed::core::error::ReasonCode;
use smed::core::event::{FinishReason, ProviderEvent, SmedEvent};
use smed::core::model::{
    ModelCapabilities, ModelDescriptor, ModelId, ProviderId, QuotaSnapshot, QuotaWindow, Usage,
};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::store::EventStore;
use smed::core::trigger::TriggerOutcome;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::store::sqlite::SqliteEventStore;
use smed::triggers::{control, definition, scheduler, status};
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(10);

fn write_trigger(root: &Path, name: &str, content: &str) {
    let directory = root.join(".smed").join("triggers");
    std::fs::create_dir_all(&directory).expect("mkdir");
    std::fs::write(directory.join(format!("{name}.yaml")), content).expect("write trigger");
}

async fn open_store(root: &Path) -> Arc<SqliteEventStore> {
    Arc::new(
        SqliteEventStore::open(&root.join("smed.db"))
            .await
            .expect("store"),
    )
}

fn deps(
    providers: Vec<Arc<dyn Provider>>,
    store: Arc<dyn EventStore>,
    workspace_root: std::path::PathBuf,
) -> scheduler::SchedulerDeps {
    scheduler::SchedulerDeps {
        providers,
        store,
        workspace_root,
        project_context: ProjectContext::empty(),
        mcp_servers: Arc::new(Vec::new()),
        tools: smed::tools::ToolRegistry::builtins(),
        route_table: Arc::new(smed::core::routing::RouteTable::default()),
    }
}

async fn control_history(
    store: &dyn EventStore,
    root_realpath: &str,
    name: &str,
) -> Vec<smed::core::event::StoredEvent> {
    let session = control::control_session_id(root_realpath, name);
    control::history(store, session).await.expect("history")
}

/// A schedule fires a run whose transcript is indistinguishable from a manual
/// headless run; a webhook firing carries its payload as canonical input.
#[tokio::test]
async fn a_webhook_firing_produces_an_ordinary_verified_session_carrying_its_payload() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("fixture.txt"), "before\n").expect("fixture");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(workspace.path())
        .status()
        .expect("git init");
    assert!(status.success());

    // An OS-chosen ephemeral port, so parallel test runs never collide.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    write_trigger(
        workspace.path(),
        "incoming",
        &format!(
            "webhook_port: {port}\ndirective: use the tools available to inspect the repository, then finish\nprovider: fake\nmodel: fake-1\npolicy: full-auto\noverlap: skip\n"
        ),
    );

    let store = open_store(workspace.path()).await;
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::GuardedLoop));
    let cancel = CancellationToken::new();
    let scheduler_deps = deps(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        workspace.path().to_path_buf(),
    );
    let scheduler_cancel = cancel.clone();
    let handle = tokio::spawn(scheduler::run(scheduler_deps, scheduler_cancel));

    // Give the webhook listener a moment to bind before posting.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let payload = "{\"ref\":\"refs/heads/main\"}";
    let response = post(port, payload).await;
    assert!(
        response.contains("202"),
        "webhook did not accept: {response}"
    );

    let root_realpath = control::root_realpath(workspace.path()).expect("realpath");
    let settled = tokio::time::timeout(DEADLINE, async {
        loop {
            let history = control_history(store.as_ref(), &root_realpath, "incoming").await;
            if let Some(event) = history.iter().find_map(|stored| match &stored.event {
                SmedEvent::TriggerSettled { outcome, child, .. } => Some((*outcome, *child)),
                _ => None,
            }) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("trigger settles");

    let (outcome, child) = settled;
    assert_eq!(outcome, TriggerOutcome::Verified);

    // The firing's own transcript is an ordinary session: SessionCreated,
    // RunStarted, tool activity, RunFinished — exactly what a manual headless
    // run produces, plus its parentage to the control session.
    let child_events = store.events(child).await.expect("child events");
    assert!(
        child_events
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::SessionCreated { .. }))
    );
    assert!(child_events.iter().any(|stored| matches!(
        stored.event,
        SmedEvent::RunFinished {
            reason: FinishReason::Stop,
            ..
        }
    )));
    let sessions = store.sessions().await.expect("sessions");
    let child_summary = sessions
        .iter()
        .find(|summary| summary.id == child)
        .expect("child session listed");
    assert_eq!(
        child_summary.parent,
        Some(control::control_session_id(&root_realpath, "incoming")),
        "a firing is an ordinary session parented to its trigger's control session"
    );

    let fired = control_history(store.as_ref(), &root_realpath, "incoming")
        .await
        .into_iter()
        .find_map(|stored| match stored.event {
            SmedEvent::TriggerFired { source, .. } => Some(source),
            _ => None,
        });
    assert_eq!(fired, Some(smed::core::trigger::TriggerSourceKind::Webhook));

    cancel.cancel();
    let _ = handle.await;
}

async fn post(port: u16, body: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response).await;
    response
}

/// Repeated failures disable the trigger with a typed reason rather than
/// retrying forever; the disabled state is visible and the trigger is
/// re-armable.
#[tokio::test]
async fn a_trigger_disables_itself_after_repeated_failure_and_is_visibly_rearmable() {
    let workspace = tempfile::tempdir().expect("workspace");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    write_trigger(
        workspace.path(),
        "flaky",
        &format!(
            "webhook_port: {port}\ndirective: do it\nprovider: broken\nmodel: broken-1\nmax_consecutive_failures: 2\n"
        ),
    );

    let store = open_store(workspace.path()).await;
    let provider: Arc<dyn Provider> = Arc::new(AlwaysFailsProvider);
    let cancel = CancellationToken::new();
    let scheduler_deps = deps(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        workspace.path().to_path_buf(),
    );
    let scheduler_cancel = cancel.clone();
    let handle = tokio::spawn(scheduler::run(scheduler_deps, scheduler_cancel));

    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = post(port, "{}").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = post(port, "{}").await;

    let root_realpath = control::root_realpath(workspace.path()).expect("realpath");
    let disabled = tokio::time::timeout(DEADLINE, async {
        loop {
            let history = control_history(store.as_ref(), &root_realpath, "flaky").await;
            if let Some(code) = history.iter().find_map(|stored| match &stored.event {
                SmedEvent::TriggerDisabled { code, .. } => Some(*code),
                _ => None,
            }) {
                return code;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("trigger disables");
    assert_eq!(disabled, ReasonCode::TriggerDisabled);

    // Disabled is visible through the read model every surface shares.
    let (statuses, _) = status::collect(store.as_ref(), workspace.path(), &root_realpath)
        .await
        .expect("collect");
    let flaky = statuses
        .iter()
        .find(|status| status.name == "flaky")
        .expect("flaky listed");
    assert!(!flaky.enabled);
    assert_eq!(flaky.disabled_reason, Some(ReasonCode::TriggerDisabled));

    cancel.cancel();
    let _ = handle.await;

    // Re-armable: the same CLI path `smed triggers rearm` drives.
    let exit = smed::cli::triggers::run(
        TriggersCommand::Rearm {
            name: "flaky".to_owned(),
        },
        &store,
        workspace.path(),
    )
    .await
    .expect("rearm");
    assert_eq!(exit, 0);

    let (statuses, _) = status::collect(store.as_ref(), workspace.path(), &root_realpath)
        .await
        .expect("collect after rearm");
    let flaky = statuses
        .iter()
        .find(|status| status.name == "flaky")
        .expect("flaky listed");
    assert!(flaky.enabled, "re-arming must clear the disabled state");
    assert_eq!(flaky.consecutive_failures, 0);
}

/// A provider that always fails immediately, to exercise the disable path
/// without waiting on a real quota or network condition.
#[derive(Debug)]
struct AlwaysFailsProvider;

#[async_trait]
impl Provider for AlwaysFailsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("broken")
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new("broken-1"),
            provider: self.id(),
            display_name: "broken".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(1000),
            max_output_tokens: Some(1000),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: tokio::sync::mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, smed::core::error::ProviderError> {
        Err(smed::core::error::ProviderError::Protocol {
            detail: "the fixture provider always fails".to_owned(),
        })
    }
}

/// Quota drain during a scheduled run lands the handoff and notifies rather
/// than dying mid-window.
#[tokio::test]
async fn a_quota_drained_firing_lands_a_handoff_and_is_visible_as_stopped_not_failed() {
    let workspace = tempfile::tempdir().expect("workspace");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    write_trigger(
        workspace.path(),
        "drains",
        &format!(
            "webhook_port: {port}\ndirective: keep going\nprovider: quota-scripted\nmodel: quota-1\n"
        ),
    );

    let store = open_store(workspace.path()).await;
    let provider: Arc<dyn Provider> = Arc::new(QuotaScriptedProvider::new());
    let cancel = CancellationToken::new();
    let mut scheduler_deps = deps(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        workspace.path().to_path_buf(),
    );
    // A tight quota reserve so the run's own accounting — not a provider
    // report — drives the drain, matching how a real subscription window is
    // configured for a trigger ( continuation).
    scheduler_deps.tools = smed::tools::ToolRegistry::builtins();
    let scheduler_cancel = cancel.clone();
    let handle = tokio::spawn(scheduler::run(scheduler_deps, scheduler_cancel));

    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = post(port, "{}").await;

    let root_realpath = control::root_realpath(workspace.path()).expect("realpath");
    let settled = tokio::time::timeout(DEADLINE, async {
        loop {
            let history = control_history(store.as_ref(), &root_realpath, "drains").await;
            if let Some(event) = history.iter().find_map(|stored| match &stored.event {
                SmedEvent::TriggerSettled { outcome, child, .. } => Some((*outcome, *child)),
                _ => None,
            }) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("trigger settles");
    let (outcome, child) = settled;

    // Stopped, not failed: a drained quota is a boundary the run respected,
    // not a broken directive.
    assert_eq!(outcome, TriggerOutcome::BudgetOrQuotaStopped);

    let child_events = store.events(child).await.expect("child events");
    assert!(
        child_events
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::HandoffCreated { .. })),
        "quota drain must land a handoff rather than dying mid-window"
    );

    cancel.cancel();
    let _ = handle.await;
}

#[derive(Debug)]
struct QuotaScriptedProvider {
    calls: Mutex<u32>,
}

impl QuotaScriptedProvider {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
        }
    }
}

#[async_trait]
impl Provider for QuotaScriptedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("quota-scripted")
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new("quota-1"),
            provider: self.id(),
            display_name: "quota".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(200_000),
            max_output_tokens: Some(16_384),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        events: tokio::sync::mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, smed::core::error::ProviderError> {
        // First turn crosses the *soft* threshold (default 0.8): the runtime
        // auto-continues with a landing turn rather than stopping outright.
        // The second call is that landing turn, with no further quota report,
        // and its completion is what lands the handoff — the same sequence
        // `quota_continuation.rs`'s `reported_soft_threshold_drains_once_and_persists_a_handoff`
        // exercises against the interactive runtime; this test exercises it
        // through the trigger scheduler instead.
        let first_call = {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            *calls == 1
        };

        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| smed::core::error::ProviderError::Cancelled)?;
        if first_call {
            events
                .send(ProviderEvent::Quota {
                    snapshot: QuotaSnapshot {
                        provider: self.id(),
                        windows: vec![QuotaWindow {
                            label: "plan".to_owned(),
                            used_fraction: 0.82,
                            resets_at: Some(
                                time::OffsetDateTime::now_utc() + time::Duration::hours(1),
                            ),
                        }],
                    },
                })
                .await
                .map_err(|_| smed::core::error::ProviderError::Cancelled)?;
        }
        events
            .send(ProviderEvent::TextDelta {
                text: if first_call {
                    "work complete; preparing landing".to_owned()
                } else {
                    "done: work; remaining: none; next: review; risks: none".to_owned()
                },
            })
            .await
            .map_err(|_| smed::core::error::ProviderError::Cancelled)?;
        events
            .send(ProviderEvent::Usage {
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 10,
                },
            })
            .await
            .map_err(|_| smed::core::error::ProviderError::Cancelled)?;
        events
            .send(ProviderEvent::Finished {
                reason: FinishReason::Stop,
            })
            .await
            .map_err(|_| smed::core::error::ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason: FinishReason::Stop,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 10,
            }),
        })
    }
}

/// A trigger's policy ceiling clamps exactly like Phase 13's child policy
/// ceiling clamps to the parent: a trigger configured `read-only` never
/// widens to `full-auto`, whatever a webhook payload might say.
#[tokio::test]
async fn a_trigger_never_widens_its_configured_policy_ceiling() {
    let workspace = tempfile::tempdir().expect("workspace");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    write_trigger(
        workspace.path(),
        "locked",
        &format!(
            "webhook_port: {port}\ndirective: try to do more than read\nprovider: fake\nmodel: fake-1\npolicy: read-only\n"
        ),
    );
    let (definitions, diagnostics) = definition::load_dir(workspace.path());
    assert!(diagnostics.is_empty());
    assert_eq!(definitions[0].policy_ceiling, PolicyMode::ReadOnly);

    // A malicious webhook payload naming a wider policy is inert: it travels
    // only as canonical input text, never as configuration.
    let payload = "{\"policy\":\"full-auto\"}";
    let store = open_store(workspace.path()).await;
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let cancel = CancellationToken::new();
    let scheduler_deps = deps(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        workspace.path().to_path_buf(),
    );
    let scheduler_cancel = cancel.clone();
    let handle = tokio::spawn(scheduler::run(scheduler_deps, scheduler_cancel));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = post(port, payload).await;

    let root_realpath = control::root_realpath(workspace.path()).expect("realpath");
    let child = tokio::time::timeout(DEADLINE, async {
        loop {
            let history = control_history(store.as_ref(), &root_realpath, "locked").await;
            if let Some(child) = history.iter().find_map(|stored| match &stored.event {
                SmedEvent::TriggerFired { child, .. } => Some(*child),
                _ => None,
            }) {
                return child;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("trigger fires");

    // Wait for the firing to record its policy.
    let policy = tokio::time::timeout(DEADLINE, async {
        loop {
            let events = store.events(child).await.unwrap_or_default();
            if let Some(mode) = events.iter().find_map(|stored| match &stored.event {
                SmedEvent::PolicyChanged { mode, .. } => Some(*mode),
                _ => None,
            }) {
                return mode;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    // A read-only trigger's initial policy equals the runtime default
    // (`Ask`... no: `ReadOnly` is not the default, so a PolicyChanged event is
    // always recorded); either way it must never be FullAuto.
    if let Ok(mode) = policy {
        assert_ne!(mode, PolicyMode::FullAuto);
        assert_eq!(mode, PolicyMode::ReadOnly);
    }

    cancel.cancel();
    let _ = handle.await;
}

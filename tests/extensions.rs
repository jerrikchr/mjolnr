//! Loading agent-authored extensions end to end.
//!
//! The unit tests cover parsing, discovery, and the scripted tool in isolation.
//! These drive a real runtime against a real SQLite file to pin the three
//! promises that only exist across layers: a discovered extension is *visible
//! but inert* until an explicit act; loading it records a typed event and makes
//! it callable; and a resumed session does **not** silently reload it — the act
//! must be repeated, because the record is evidence, not a reload instruction.

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

use async_trait::async_trait;
use smed::context::{DiscoveryConfig, DiscoveryLimits, ProjectContext};
use smed::core::command::{ApprovalDecision, SmedCommand};
use smed::core::error::ProviderError;
use smed::core::event::{ExtensionLoadAuthority, FinishReason, ProviderEvent, SmedEvent};
use smed::core::message::{ContentBlock, ToolCall, ToolOutcome};
use smed::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::{RuntimeSnapshot, RuntimeSubscription, SmedRuntime};
use smed::core::store::EventStore;
use smed::core::tool::ToolTier;
use smed::providers::fake::FakeProvider;
use smed::runtime::Runtime;
use smed::store::sqlite::SqliteEventStore;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const COUNT_LINES: &str = "name: count-lines
description: Count the lines in a file at the workspace root.
parameters:
  - name: path
    description: File to count, relative to the workspace root.
run:
  program: wc
  arguments: [\"-l\", \"${path}\"]
";

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
    user: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let database = directory.path().join("smed.sqlite3");
        let workspace = directory.path().join("workspace");
        let user = directory.path().join("user");
        std::fs::create_dir_all(workspace.join(".smed/extensions")).expect("extensions dir");
        std::fs::create_dir_all(&user).expect("user dir");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        Self {
            _directory: directory,
            database,
            workspace,
            user,
        }
    }

    fn write_extension(&self, file: &str, contents: &str) {
        std::fs::write(self.workspace.join(".smed/extensions").join(file), contents)
            .expect("write extension");
    }

    fn context(&self) -> ProjectContext {
        ProjectContext::discover(DiscoveryConfig {
            project_root: self.workspace.clone(),
            working_directory: self.workspace.clone(),
            user_native_skills: self.user.join("smed"),
            user_agent_skills: self.user.join("agents"),
            user_config: self.user.join("smed"),
            limits: DiscoveryLimits::default(),
        })
        .expect("discover context")
    }

    async fn store(&self) -> Arc<SqliteEventStore> {
        Arc::new(
            SqliteEventStore::open(&self.database)
                .await
                .expect("open database"),
        )
    }

    fn runtime(&self, store: &Arc<SqliteEventStore>) -> Runtime {
        let provider: Arc<dyn smed::core::provider::Provider> = Arc::new(FakeProvider::default());
        Runtime::spawn_with_project_context(
            vec![provider],
            Arc::clone(store) as Arc<dyn EventStore>,
            self.context(),
        )
    }
}

async fn settle(runtime: &Runtime, ready: impl Fn(&RuntimeSnapshot) -> bool) -> RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

async fn open_session(runtime: &Runtime, workspace: &std::path::Path) -> RuntimeSnapshot {
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: workspace.to_path_buf(),
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
    settle(runtime, |snapshot| snapshot.session.is_some()).await
}

async fn load(runtime: &Runtime, name: &str) -> RuntimeSnapshot {
    runtime
        .dispatch(SmedCommand::LoadExtension {
            name: name.to_owned(),
        })
        .await
        .expect("dispatch load");
    settle(runtime, |snapshot| {
        snapshot
            .last_extension_load
            .as_ref()
            .is_some_and(|report| report.name == name)
    })
    .await
}

#[tokio::test]
async fn a_discovered_extension_is_visible_but_not_loaded_until_the_act() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);
    let store = fixture.store().await;
    let runtime = fixture.runtime(&store);
    let snapshot = open_session(&runtime, &fixture.workspace).await;

    // Visible…
    assert!(
        snapshot
            .extensions
            .iter()
            .any(|extension| extension.name == "count-lines"),
        "a discovered extension must be listed on the snapshot"
    );
    // …but nothing has been loaded, so no act has been reported.
    assert!(snapshot.last_extension_load.is_none());

    let _ = runtime.close().await;
}

#[tokio::test]
async fn loading_records_a_typed_event_and_makes_the_tool_callable() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);
    let store = fixture.store().await;
    let runtime = fixture.runtime(&store);
    let opened = open_session(&runtime, &fixture.workspace).await;
    let session = opened.session.expect("session");

    let loaded = load(&runtime, "count-lines").await;
    let report = loaded.last_extension_load.expect("a load was reported");
    assert_eq!(report.loaded_program.as_deref(), Some("wc"));
    assert!(report.failure.is_none(), "{:?}", report.failure);

    // The load is a durable, typed event naming what was loaded and by what act.
    let events = store.events(session).await.expect("events");
    let recorded = events
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::ExtensionLoaded {
                name, program, by, ..
            } => Some((name.clone(), program.clone(), *by)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recorded,
        vec![(
            "count-lines".to_owned(),
            "wc".to_owned(),
            ExtensionLoadAuthority::Command
        )]
    );

    // Now callable: a second load finds the name already taken, which it could
    // only report if the first load actually registered the tool. Wait for the
    // *failure* report specifically — the successful first report is still on
    // the snapshot, so settling on the name alone would race it.
    runtime
        .dispatch(SmedCommand::LoadExtension {
            name: "count-lines".to_owned(),
        })
        .await
        .expect("dispatch second load");
    settle(&runtime, |snapshot| {
        snapshot
            .last_extension_load
            .as_ref()
            .and_then(|report| report.failure.as_deref())
            .is_some_and(|failure| failure.contains("already available"))
    })
    .await;

    let _ = runtime.close().await;
}

#[tokio::test]
async fn an_unknown_extension_is_refused_without_recording_anything() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);
    let store = fixture.store().await;
    let runtime = fixture.runtime(&store);
    let opened = open_session(&runtime, &fixture.workspace).await;
    let session = opened.session.expect("session");

    let refused = load(&runtime, "does-not-exist").await;
    let report = refused.last_extension_load.expect("a refusal was reported");
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("no discovered extension")),
        "{report:?}"
    );

    // A refusal writes nothing to the log.
    let events = store.events(session).await.expect("events");
    assert!(
        !events
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::ExtensionLoaded { .. })),
        "a refused load must record no ExtensionLoaded event"
    );

    let _ = runtime.close().await;
}

/// Calls the `count-lines` extension once, then finishes. Used to prove a
/// loaded extension's call is gated exactly like a native tool.
#[derive(Debug)]
struct CallingProvider;

const CALLER: &str = "caller";
const CALLER_MODEL: &str = "caller-1";

#[async_trait]
impl Provider for CallingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(CALLER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(CALLER_MODEL),
            provider: self.id(),
            display_name: CALLER_MODEL.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(8_192),
            max_output_tokens: Some(4_096),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let after_tool = request.messages.last().is_some_and(|message| {
            message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        });
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        let reason = if after_tool {
            events
                .send(ProviderEvent::TextDelta {
                    text: "done".to_owned(),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::Stop
        } else {
            let call = ToolCall {
                id: "ext-call".to_owned(),
                name: "count-lines".to_owned(),
                arguments: serde_json::json!({ "path": "README.md" }),
                provider_signature: None,
            };
            events
                .send(ProviderEvent::ToolCallStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            events
                .send(ProviderEvent::ToolCallCompleted { call })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::ToolCalls
        };
        events
            .send(ProviderEvent::Finished { reason })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason,
            usage: None,
        })
    }
}

async fn wait_event(
    events: &mut RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&SmedEvent) -> bool,
) -> SmedEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(error) => panic!("event feed ended while waiting for {label}: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
}

#[tokio::test]
async fn a_loaded_extension_call_is_previewed_gated_at_execute_and_refusable() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);
    let store = fixture.store().await;
    let provider: Arc<dyn Provider> = Arc::new(CallingProvider);
    let runtime = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        fixture.context(),
    );
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(CALLER),
            model: ModelId::new(CALLER_MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("set policy");
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    // Load the extension, then ask the model to work — it calls count-lines.
    load(&runtime, "count-lines").await;
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "count the readme".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    // The call is proposed through the same gate a native command uses: Execute
    // tier, an approval to resolve, and a preview showing the exact argv the
    // fixed program will run.
    let proposal = wait_event(
        &mut events,
        "extension proposal",
        |event| matches!(event, SmedEvent::ToolProposed { call, .. } if call.name == "count-lines"),
    )
    .await;
    let approval = match proposal {
        SmedEvent::ToolProposed {
            approval,
            tier,
            preview,
            ..
        } => {
            assert_eq!(tier, ToolTier::Execute, "an extension call is Execute-tier");
            assert!(
                preview.contains("wc -l") && preview.contains("README.md"),
                "the preview must show the substituted argv, got {preview:?}"
            );
            approval.expect("an Execute call under workspace-write must be gated")
        }
        other => panic!("expected a proposal, got {other:?}"),
    };

    // Refusable exactly like a built-in: deny it, and it does not run.
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::Deny,
        })
        .await
        .expect("deny");
    let finished = settle(&runtime, |snapshot| !snapshot.run_active).await;
    let refused = finished
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            ContentBlock::ToolResult { name, result, .. } if name == "count-lines" => {
                Some(result.outcome.clone())
            }
            _ => None,
        })
        .expect("the denied call must leave a refused tool result");
    assert_eq!(
        refused,
        ToolOutcome::Refused(smed::core::error::ReasonCode::ApprovalDenied),
        "a denied extension call is refused, not executed"
    );

    let _ = runtime.close().await;
}

/// Proposes `load_extension` for count-lines, then calls it once loaded.
///
/// The turn's action is decided by the last message: after a `load_extension`
/// result it calls `count-lines`; after a `count-lines` result it finishes;
/// otherwise it proposes the load. This is the agent-loop half of Phase 17 —
/// the model extending itself.
#[derive(Debug)]
struct SelfExtendingProvider;

const AGENT: &str = "agent";
const AGENT_MODEL: &str = "agent-1";

fn last_tool_result_name(request: &ProviderRequest) -> Option<String> {
    request
        .messages
        .last()?
        .blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult { name, .. } => Some(name.clone()),
            _ => None,
        })
}

#[async_trait]
impl Provider for SelfExtendingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(AGENT)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(AGENT_MODEL),
            provider: self.id(),
            display_name: AGENT_MODEL.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(8_192),
            max_output_tokens: Some(4_096),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let call = match last_tool_result_name(&request).as_deref() {
            Some("count-lines") => None,
            Some("load_extension") => Some(ToolCall {
                id: "call-count".to_owned(),
                name: "count-lines".to_owned(),
                arguments: serde_json::json!({ "path": "README.md" }),
                provider_signature: None,
            }),
            _ => Some(ToolCall {
                id: "call-load".to_owned(),
                name: "load_extension".to_owned(),
                arguments: serde_json::json!({ "name": "count-lines" }),
                provider_signature: None,
            }),
        };
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        let reason = match call {
            None => {
                events
                    .send(ProviderEvent::TextDelta {
                        text: "done".to_owned(),
                    })
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                FinishReason::Stop
            }
            Some(call) => {
                events
                    .send(ProviderEvent::ToolCallStarted {
                        id: call.id.clone(),
                        name: call.name.clone(),
                    })
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                events
                    .send(ProviderEvent::ToolCallCompleted { call })
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                FinishReason::ToolCalls
            }
        };
        events
            .send(ProviderEvent::Finished { reason })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason,
            usage: None,
        })
    }
}

#[tokio::test]
async fn a_human_approving_a_trust_gated_agent_load_records_approved_authority() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);
    std::fs::write(fixture.workspace.join("README.md"), "one\ntwo\n").expect("readme");
    let store = fixture.store().await;
    let provider: Arc<dyn Provider> = Arc::new(SelfExtendingProvider);
    let runtime = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        fixture.context(),
    );
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(AGENT),
            model: ModelId::new(AGENT_MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set full-auto");
    let opened = settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    let session = opened.session.expect("session");

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "improve the tooling".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    // Even under full-auto, a model-proposed load of a *project* extension is
    // gated — the trust gate fires because no human typed the command. It is
    // not silently self-loaded.
    let gated = settle(&runtime, |snapshot| {
        snapshot
            .pending_approval
            .as_ref()
            .is_some_and(|approval| approval.tool_name == "load_extension")
    })
    .await;
    let approval = gated.pending_approval.expect("load is gated").id;
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve the load");

    // The human approved the agent's proposal, so the record must not claim
    // that full-auto stood behind this load.
    let loaded = wait_event(&mut events, "extension loaded", |event| {
        matches!(event, SmedEvent::ExtensionLoaded { .. })
    })
    .await;
    match loaded {
        SmedEvent::ExtensionLoaded { name, by, .. } => {
            assert_eq!(name, "count-lines");
            assert_eq!(by, ExtensionLoadAuthority::Approved);
        }
        other => panic!("expected ExtensionLoaded, got {other:?}"),
    }

    // And the newly loaded tool is callable: the model calls it, and under
    // full-auto its Execute call auto-runs.
    wait_event(
        &mut events,
        "count-lines called",
        |event| matches!(event, SmedEvent::ToolProposed { call, .. } if call.name == "count-lines"),
    )
    .await;
    let finished = settle(&runtime, |snapshot| !snapshot.run_active).await;
    assert_eq!(finished.session, Some(session));

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_resumed_session_does_not_silently_reload_the_extension() {
    let fixture = Fixture::new();
    fixture.write_extension("count-lines.yaml", COUNT_LINES);

    let session = {
        let store = fixture.store().await;
        let runtime = fixture.runtime(&store);
        let opened = open_session(&runtime, &fixture.workspace).await;
        let session = opened.session.expect("session");
        let loaded = load(&runtime, "count-lines").await;
        assert!(
            loaded
                .last_extension_load
                .expect("loaded")
                .loaded_program
                .is_some()
        );
        runtime.close().await.expect("close");
        session
    };

    // Reopen the same session against the same store with a freshly discovered
    // context.
    let store = fixture.store().await;
    let runtime = fixture.runtime(&store);
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let resumed = settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    // The extension is re-discovered — visible again…
    assert!(
        resumed
            .extensions
            .iter()
            .any(|extension| extension.name == "count-lines")
    );

    // …but it was NOT silently re-registered: loading it now succeeds rather
    // than reporting the name already taken. A silent reload on resume would
    // have made this second load say "already available".
    let reloaded = load(&runtime, "count-lines").await;
    let report = reloaded.last_extension_load.expect("load reported");
    assert_eq!(
        report.loaded_program.as_deref(),
        Some("wc"),
        "resume must not silently reload the extension; the act must be repeated ({:?})",
        report.failure
    );

    let _ = runtime.close().await;
}

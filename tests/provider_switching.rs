//! Same-session provider/model switching contracts (plan Phase 6).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mjolnr::context::{DiscoveryConfig, DiscoveryLimits, ProjectContext};
use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::error::{ProviderError, ReasonCode};
use mjolnr::core::event::{FinishReason, MjolnrEvent, ProviderEvent};
use mjolnr::core::message::{ContentBlock, ToolCall};
use mjolnr::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use mjolnr::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSnapshot};
use mjolnr::core::store::EventStore;
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
enum Script {
    ToolThenText,
    Text,
}

#[derive(Debug)]
struct RecordingProvider {
    id: &'static str,
    model: &'static str,
    capabilities: ModelCapabilities,
    script: Script,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

#[derive(Debug)]
struct DisconnectedProvider;

#[derive(Debug)]
struct DynamicCatalogProvider;

#[async_trait]
impl Provider for DynamicCatalogProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("dynamic")
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new("stale-static-model"),
            provider: self.id(),
            display_name: "Stale static model".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        }]
    }

    async fn discover_models(
        &self,
        _cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        Ok(vec![ModelDescriptor {
            id: ModelId::new("new-account-model"),
            provider: self.id(),
            display_name: "New account model".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(32_768),
            max_output_tokens: None,
            tier: None,
        }])
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        panic!("catalog discovery must not start a completion")
    }
}

#[async_trait]
impl Provider for DisconnectedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("disconnected")
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new("offline-1"),
            provider: self.id(),
            display_name: "Offline".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        }]
    }

    fn credentialed(&self) -> bool {
        false
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        panic!("a disconnected provider must never receive a request")
    }
}

#[async_trait]
impl Provider for RecordingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.id)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(self.model),
            provider: self.id(),
            display_name: self.model.to_owned(),
            capabilities: self.capabilities,
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
        let has_result = request.messages.iter().any(|message| {
            message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        });
        self.requests.lock().expect("requests").push(request);
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        let reason = if matches!(self.script, Script::ToolThenText) && !has_result {
            for call in [
                ToolCall {
                    id: "activate_1".to_owned(),
                    name: "activate_skill".to_owned(),
                    arguments: serde_json::json!({"name":"switch-context"}),
                    provider_signature: None,
                },
                ToolCall {
                    id: "read_1".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({"path":"note.txt"}),
                    provider_signature: None,
                },
            ] {
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
            }
            FinishReason::ToolCalls
        } else {
            events
                .send(ProviderEvent::TextDelta {
                    text: format!("{} continued", self.id),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::Stop
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

async fn wait_snapshot(
    runtime: &Runtime,
    ready: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    let mut snapshots = runtime.snapshots();
    let current = runtime.snapshot();
    if ready(&current) {
        return current;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime open");
            if ready(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .expect("snapshot timeout")
}

fn project(fixture: &TempDir) -> (std::path::PathBuf, ProjectContext) {
    let workspace = fixture.path().join("workspace");
    let skill = fixture.path().join("user-skills/switch-context");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&skill).expect("skill");
    std::fs::write(workspace.join("note.txt"), "durable repository state").expect("note");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: switch-context\ndescription: Preserve switching context\n---\nACTIVATED SWITCH INSTRUCTIONS\n",
    )
    .expect("SKILL.md");
    let context = ProjectContext::discover(DiscoveryConfig {
        project_root: workspace.clone(),
        working_directory: workspace.clone(),
        user_native_skills: fixture.path().join("unused"),
        user_agent_skills: fixture.path().join("user-skills"),
        user_config: fixture.path().join("user-skills"),
        limits: DiscoveryLimits::default(),
    })
    .expect("context");
    (workspace, context)
}

#[tokio::test]
async fn runtime_publishes_the_discovered_catalog_instead_of_static_models() {
    let runtime = Runtime::spawn(
        vec![Arc::new(DynamicCatalogProvider)],
        Arc::new(InMemoryEventStore::new()),
    );
    let snapshot = wait_snapshot(&runtime, |snapshot| {
        snapshot.providers.iter().any(|provider| {
            provider.provider.as_str() == "dynamic"
                && provider.state == mjolnr::core::runtime::ProviderConnectionState::Connected
        })
    })
    .await;

    assert_eq!(snapshot.models.len(), 1);
    assert_eq!(
        snapshot.models[0].descriptor.id.as_str(),
        "new-account-model"
    );
    assert!(
        snapshot
            .models
            .iter()
            .all(|model| model.descriptor.id.as_str() != "stale-static-model")
    );
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn canonical_history_tools_skills_and_project_survive_provider_switching() {
    let fixture = TempDir::new().expect("fixture");
    let (workspace, context) = project(&fixture);
    let alpha_requests = Arc::new(Mutex::new(Vec::new()));
    let beta_requests = Arc::new(Mutex::new(Vec::new()));
    let alpha: Arc<dyn Provider> = Arc::new(RecordingProvider {
        id: "alpha",
        model: "alpha-1",
        capabilities: ModelCapabilities::text_and_tools(),
        script: Script::ToolThenText,
        requests: Arc::clone(&alpha_requests),
    });
    let beta: Arc<dyn Provider> = Arc::new(RecordingProvider {
        id: "beta",
        model: "beta-1",
        capabilities: ModelCapabilities::text_and_tools(),
        script: Script::Text,
        requests: Arc::clone(&beta_requests),
    });
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn_with_project_context(
        vec![Arc::clone(&alpha), Arc::clone(&beta)],
        Arc::clone(&store) as Arc<dyn EventStore>,
        context,
    );
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.clone(),
        })
        .await
        .expect("open");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new("alpha"),
            model: ModelId::new("alpha-1"),
        })
        .await
        .expect("create");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "inspect the repository".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send alpha");
    let alpha_done = wait_snapshot(&runtime, |snapshot| {
        !snapshot.run_active
            && snapshot
                .messages
                .iter()
                .any(|message| message.text() == "alpha continued")
    })
    .await;
    let session = alpha_done.session.expect("session");
    let canonical_workspace = workspace.canonicalize().expect("root");
    assert_eq!(
        alpha_done.workspace_root.as_ref(),
        Some(&canonical_workspace)
    );
    assert!(
        alpha_done
            .activated_skills
            .iter()
            .any(|name| name == "switch-context")
    );
    assert!(alpha_done.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { result, .. } if result.content.contains("durable repository state"))
        })
    }));
    let before_switch = alpha_done.clone();

    let mut events = runtime.subscribe();
    runtime
        .dispatch(MjolnrCommand::SelectModel {
            provider: ProviderId::new("beta"),
            model: ModelId::new("beta-1"),
        })
        .await
        .expect("switch");
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let MjolnrEvent::ModelChanged {
                provider, model, ..
            } = events.recv().await.expect("event")
            {
                return (provider, model);
            }
        }
    })
    .await
    .expect("model event");
    assert_eq!(changed.0.as_str(), "beta");
    assert_eq!(changed.1.as_str(), "beta-1");
    let after_switch = wait_snapshot(&runtime, |snapshot| {
        snapshot
            .provider
            .as_ref()
            .is_some_and(|id| id.as_str() == "beta")
            && snapshot
                .model
                .as_ref()
                .is_some_and(|id| id.as_str() == "beta-1")
    })
    .await;
    assert_eq!(
        after_switch.messages.as_ref(),
        before_switch.messages.as_ref()
    );
    assert_eq!(after_switch.workspace_root, before_switch.workspace_root);
    assert_eq!(after_switch.policy, before_switch.policy);
    assert_eq!(after_switch.budget, before_switch.budget);
    assert_eq!(after_switch.usage, before_switch.usage);
    assert_eq!(
        after_switch.activated_skills.as_ref(),
        before_switch.activated_skills.as_ref()
    );
    assert_eq!(
        after_switch.workspace_trusted,
        before_switch.workspace_trusted
    );

    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "continue from the prior work".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send beta");
    let beta_done = wait_snapshot(&runtime, |snapshot| {
        !snapshot.run_active
            && snapshot
                .messages
                .iter()
                .any(|message| message.text() == "beta continued")
    })
    .await;
    assert!(
        beta_done
            .activated_skills
            .iter()
            .any(|name| name == "switch-context")
    );
    let beta_request = beta_requests.lock().expect("beta requests")[0].clone();
    assert!(beta_request.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { result, .. } if result.content.contains("ACTIVATED SWITCH INSTRUCTIONS"))
        })
    }));
    assert!(beta_request.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { result, .. } if result.content.contains("durable repository state"))
        })
    }));
    assert_eq!(alpha_requests.lock().expect("alpha requests").len(), 2);
    runtime.close().await.expect("close");

    let (workspace, context) = project(&fixture);
    let resumed = Runtime::spawn_with_project_context(
        vec![alpha, beta],
        Arc::clone(&store) as Arc<dyn EventStore>,
        context,
    );
    resumed
        .dispatch(MjolnrCommand::OpenProject { root: workspace })
        .await
        .expect("reopen");
    resumed
        .dispatch(MjolnrCommand::ResumeSession { session })
        .await
        .expect("resume");
    let restored = wait_snapshot(&resumed, |snapshot| snapshot.session == Some(session)).await;
    assert_eq!(restored.provider.expect("provider").as_str(), "beta");
    assert_eq!(restored.model.expect("model").as_str(), "beta-1");
    assert_eq!(restored.messages.as_ref(), beta_done.messages.as_ref());
    assert_eq!(
        restored.activated_skills.as_ref(),
        beta_done.activated_skills.as_ref()
    );
    resumed.close().await.expect("close resumed");
}

#[tokio::test]
async fn incompatible_model_switch_is_durable_and_sends_no_provider_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(RecordingProvider {
        id: "limited",
        model: "limited-1",
        capabilities: ModelCapabilities {
            streaming: true,
            ..ModelCapabilities::default()
        },
        script: Script::Text,
        requests: Arc::clone(&requests),
    });
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new("limited"),
            model: ModelId::new("limited-1"),
        })
        .await
        .expect("create");
    let session = wait_snapshot(&runtime, |snapshot| snapshot.session.is_some())
        .await
        .session
        .expect("session");
    runtime
        .dispatch(MjolnrCommand::SelectModel {
            provider: ProviderId::new("limited"),
            model: ModelId::new("limited-1"),
        })
        .await
        .expect("select");
    let stored = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let events = store.events_from(session, 0).await.expect("events");
            if events.iter().any(|stored| {
                matches!(
                    stored.event,
                    MjolnrEvent::ModelChangeRefused {
                        code: ReasonCode::ProviderIncompatibleModel,
                        ..
                    }
                )
            }) {
                return events;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable refusal");
    assert!(
        stored
            .iter()
            .any(|event| matches!(event.event, MjolnrEvent::ModelChangeRefused { .. }))
    );
    assert!(requests.lock().expect("requests").is_empty());
    assert_eq!(
        runtime.snapshot().model.expect("model").as_str(),
        "limited-1"
    );
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn disconnected_provider_is_auth_visible_model_hidden_and_direct_switch_refused() {
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(
        vec![Arc::new(DisconnectedProvider)],
        Arc::clone(&store) as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new("disconnected"),
            model: ModelId::new("offline-1"),
        })
        .await
        .expect("create");
    let snapshot = wait_snapshot(&runtime, |snapshot| {
        snapshot.session.is_some() && !snapshot.providers.is_empty()
    })
    .await;
    assert!(snapshot.models.is_empty());
    assert_eq!(
        snapshot.providers[0].state,
        mjolnr::core::runtime::ProviderConnectionState::Disconnected
    );
    let session = snapshot.session.expect("session");

    runtime
        .dispatch(MjolnrCommand::SelectModel {
            provider: ProviderId::new("disconnected"),
            model: ModelId::new("offline-1"),
        })
        .await
        .expect("select");
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let events = store.events_from(session, 0).await.expect("events");
            if events.iter().any(|stored| {
                matches!(
                    stored.event,
                    MjolnrEvent::ModelChangeRefused {
                        code: ReasonCode::ProviderAuth,
                        ..
                    }
                )
            }) {
                return events;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable auth refusal");
    assert!(events.iter().any(|stored| matches!(
        stored.event,
        MjolnrEvent::ModelChangeRefused {
            code: ReasonCode::ProviderAuth,
            ..
        }
    )));
    runtime.close().await.expect("close");
}

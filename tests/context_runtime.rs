//! Runtime-level Phase 5 contracts: trust, progressive disclosure, and resume.

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
use smed::context::{DiscoveryConfig, DiscoveryLimits, ProjectContext};
use smed::core::command::{ApprovalDecision, SmedCommand};
use smed::core::error::{ProviderError, ReasonCode};
use smed::core::event::{FinishReason, ProviderEvent};
use smed::core::message::{ContentBlock, ToolCall, ToolOutcome};
use smed::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = "skill-test";
const MODEL: &str = "skill-test-1";

#[derive(Debug)]
struct SkillProvider {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    skill: String,
}

#[async_trait]
impl Provider for SkillProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        [MODEL, "skill-test-2"]
            .into_iter()
            .map(|model| ModelDescriptor {
                id: ModelId::new(model),
                provider: self.id(),
                display_name: model.to_owned(),
                capabilities: ModelCapabilities::text_and_tools(),
                context_tokens: Some(8_192),
                max_output_tokens: Some(4_096),
                tier: None,
            })
            .collect()
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let has_result = request.messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, ContentBlock::ToolResult { name, .. } if name == "activate_skill")
            })
        });
        self.requests.lock().expect("requests").push(request);
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        let reason = if has_result {
            events
                .send(ProviderEvent::TextDelta {
                    text: "done".to_owned(),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::Stop
        } else {
            let call = ToolCall {
                id: "skill-call".to_owned(),
                name: "activate_skill".to_owned(),
                arguments: serde_json::json!({ "name": self.skill }),
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

fn context(fixture: &TempDir, skill: &str, project: bool) -> (std::path::PathBuf, ProjectContext) {
    let workspace = fixture.path().join("workspace");
    let user = fixture.path().join("user");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        workspace.join("AGENTS.md"),
        "Always preserve typed reasons.",
    )
    .expect("AGENTS.md");
    let root = if project {
        workspace.join(".agents/skills")
    } else {
        user.join("agents")
    };
    let directory = root.join(skill);
    std::fs::create_dir_all(directory.join("scripts")).expect("skill directories");
    std::fs::write(
        directory.join("SKILL.md"),
        format!(
            "---\nname: {skill}\ndescription: Use this skill for guarded reviews.\nallowed-tools: Bash(*)\n---\nFULL ACTIVATED INSTRUCTIONS\nSee scripts/attempt.sh.\n"
        ),
    )
    .expect("SKILL.md");
    std::fs::write(
        directory.join("scripts/attempt.sh"),
        "touch script-must-not-run",
    )
    .expect("script");
    let discovered = ProjectContext::discover(DiscoveryConfig {
        project_root: workspace.clone(),
        working_directory: workspace.clone(),
        user_native_skills: user.join("smed"),
        user_agent_skills: user.join("agents"),
        user_config: user.join("smed"),
        limits: DiscoveryLimits::default(),
    })
    .expect("context");
    (workspace, discovered)
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

#[tokio::test]
async fn event_driven_snapshots_reuse_the_discovered_skill_catalog_arc() {
    let fixture = TempDir::new().expect("fixture");
    let (workspace, context) = context(&fixture, "shared-catalog", false);
    let provider: Arc<dyn Provider> = Arc::new(SkillProvider {
        requests: Arc::new(Mutex::new(Vec::new())),
        skill: "shared-catalog".to_owned(),
    });
    let runtime = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::new(InMemoryEventStore::new()),
        context,
    );
    runtime
        .dispatch(SmedCommand::OpenProject { root: workspace })
        .await
        .expect("open");
    let first = wait_snapshot(&runtime, |snapshot| !snapshot.skills.is_empty()).await;
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::ReadOnly,
        })
        .await
        .expect("set policy");
    let second = wait_snapshot(&runtime, |snapshot| snapshot.policy == PolicyMode::ReadOnly).await;

    assert!(
        Arc::ptr_eq(&first.skills, &second.skills),
        "publishing a snapshot must bump the catalog Arc, not clone every summary"
    );
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn project_activation_requires_trust_then_survives_resume_and_model_change() {
    let fixture = TempDir::new().expect("fixture");
    let (workspace, context) = context(&fixture, "guarded-review", true);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(SkillProvider {
        requests: Arc::clone(&requests),
        skill: "guarded-review".to_owned(),
    });
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        context.clone(),
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "review".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let pending = wait_snapshot(&runtime, |snapshot| snapshot.pending_approval.is_some()).await;
    assert!(!pending.workspace_trusted);
    let approval = pending.pending_approval.expect("trust prompt");
    assert_eq!(approval.tool_name, "activate_skill");
    assert!(approval.preview.contains("Trust this workspace"));

    {
        let first = requests.lock().expect("requests");
        let request = &first[0];
        let system = request.system.as_deref().expect("system context");
        assert!(system.contains("Always preserve typed reasons."));
        assert!(system.contains("Use this skill for guarded reviews."));
        assert!(!system.contains("FULL ACTIVATED INSTRUCTIONS"));
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == "activate_skill")
        );
    }

    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval: approval.id,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve trust");
    let finished = wait_snapshot(&runtime, |snapshot| {
        !snapshot.run_active
            && snapshot
                .activated_skills
                .iter()
                .any(|name| name == "guarded-review")
    })
    .await;
    assert!(finished.workspace_trusted);
    assert!(!workspace.join("script-must-not-run").exists());

    let result = {
        let captured = requests.lock().expect("requests");
        assert_eq!(captured.len(), 2);
        captured[1]
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                ContentBlock::ToolResult { result, .. } => Some(result.content.clone()),
                _ => None,
            })
            .expect("activation result")
    };
    assert!(result.contains("FULL ACTIVATED INSTRUCTIONS"));
    assert!(result.contains("scripts/attempt.sh"));
    assert!(!result.contains("touch script-must-not-run"));
    let session = finished.session.expect("session");
    runtime.close().await.expect("close");

    let provider: Arc<dyn Provider> = Arc::new(SkillProvider {
        requests: Arc::new(Mutex::new(Vec::new())),
        skill: "guarded-review".to_owned(),
    });
    let resumed = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::clone(&store) as Arc<dyn EventStore>,
        context,
    );
    resumed
        .dispatch(SmedCommand::OpenProject { root: workspace })
        .await
        .expect("open project");
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let restored = wait_snapshot(&resumed, |snapshot| snapshot.session == Some(session)).await;
    assert!(restored.workspace_trusted);
    assert!(
        restored
            .activated_skills
            .iter()
            .any(|name| name == "guarded-review")
    );
    resumed
        .dispatch(SmedCommand::SelectModel {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new("skill-test-2"),
        })
        .await
        .expect("switch model");
    let switched = wait_snapshot(&resumed, |snapshot| {
        snapshot
            .model
            .as_ref()
            .is_some_and(|model| model.as_str() == "skill-test-2")
    })
    .await;
    assert!(
        switched
            .activated_skills
            .iter()
            .any(|name| name == "guarded-review")
    );
    resumed.close().await.expect("close resumed");
}

#[tokio::test]
async fn user_skill_activation_needs_no_workspace_trust_prompt() {
    let fixture = TempDir::new().expect("fixture");
    let (workspace, context) = context(&fixture, "user-review", false);
    let provider: Arc<dyn Provider> = Arc::new(SkillProvider {
        requests: Arc::new(Mutex::new(Vec::new())),
        skill: "user-review".to_owned(),
    });
    let store = Arc::new(InMemoryEventStore::new());
    let runtime =
        Runtime::spawn_with_project_context(vec![provider], store as Arc<dyn EventStore>, context);
    runtime
        .dispatch(SmedCommand::OpenProject { root: workspace })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "review".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let finished = wait_snapshot(&runtime, |snapshot| {
        !snapshot.run_active && !snapshot.activated_skills.is_empty()
    })
    .await;
    assert!(finished.pending_approval.is_none());
    assert!(!finished.workspace_trusted);
    runtime.close().await.expect("close");
}

#[tokio::test]
async fn a_catalog_cannot_be_reused_for_another_workspace() {
    let fixture = TempDir::new().expect("fixture");
    let (_workspace, context) = context(&fixture, "bounded-context", true);
    let other_workspace = fixture.path().join("other-workspace");
    std::fs::create_dir_all(&other_workspace).expect("other workspace");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(SkillProvider {
        requests: Arc::clone(&requests),
        skill: "bounded-context".to_owned(),
    });
    let runtime = Runtime::spawn_with_project_context(
        vec![provider],
        Arc::new(InMemoryEventStore::new()),
        context,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: other_workspace,
        })
        .await
        .expect("open other project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "review".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let finished = wait_snapshot(&runtime, |snapshot| {
        !snapshot.run_active
            && snapshot.messages.iter().any(|message| {
                message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
            })
    })
    .await;
    assert!(finished.activated_skills.is_empty());
    assert!(!finished.workspace_trusted);
    assert!(finished.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { result, .. }
                    if result.outcome == ToolOutcome::Refused(ReasonCode::PathOutsideWorkspace)
            )
        })
    }));
    assert_eq!(requests.lock().expect("requests").len(), 2);
    runtime.close().await.expect("close");
}

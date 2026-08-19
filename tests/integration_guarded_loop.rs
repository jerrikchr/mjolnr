//! Deterministic Phase 3 repository transcript with no network or credential.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use smed::core::command::{ApprovalDecision, ApprovalId, SmedCommand};
use smed::core::error::ReasonCode;
use smed::core::event::{FinishReason, SmedEvent};
use smed::core::message::{ContentBlock, Role, ToolEffect, ToolOutcome};
use smed::core::model::{ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;

async fn wait_for(
    events: &mut smed::core::runtime::RuntimeSubscription,
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

fn approval_from(event: SmedEvent, expected_tool: &str) -> ApprovalId {
    match event {
        SmedEvent::ToolProposed {
            approval: Some(approval),
            call,
            ..
        } => {
            assert_eq!(call.name, expected_tool);
            approval
        }
        other => panic!("expected an approval proposal, got {other:?}"),
    }
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {arguments:?} failed");
}

#[allow(clippy::too_many_lines)]
fn event_label(event: &SmedEvent) -> String {
    match event {
        SmedEvent::SessionCreated { .. } => "session-created".to_owned(),
        SmedEvent::MessageAppended { message, .. } => match message.role {
            Role::System => "message:system".to_owned(),
            Role::User => "message:user".to_owned(),
            Role::Assistant => "message:assistant".to_owned(),
            Role::Tool => "message:tool".to_owned(),
        },
        SmedEvent::RunStarted { .. } => "run-started".to_owned(),
        SmedEvent::TextDelta { .. } => "text-delta".to_owned(),
        SmedEvent::ReasoningDelta { .. } => "reasoning-delta".to_owned(),
        SmedEvent::ToolAssembling { name, .. } => format!("assembling:{name}"),
        SmedEvent::QuotaReported { .. } => "quota".to_owned(),
        SmedEvent::QuotaBoundaryReached { .. } => "quota-boundary".to_owned(),
        SmedEvent::HandoffCreated { .. } => "handoff-created".to_owned(),
        SmedEvent::UsageReported { .. } => "usage".to_owned(),
        SmedEvent::PolicyChanged { .. } => "policy-changed".to_owned(),
        SmedEvent::ExtensionLoaded { name, .. } => format!("extension-loaded:{name}"),
        SmedEvent::ToolProposed { call, .. } => format!("proposed:{}", call.name),
        SmedEvent::ApprovalResolved { .. } => "approval-resolved".to_owned(),
        SmedEvent::ToolCompleted { name, .. } => format!("completed:{name}"),
        SmedEvent::ToolFailed { name, .. } => format!("failed:{name}"),
        SmedEvent::BudgetExhausted { .. } => "budget-exhausted".to_owned(),
        SmedEvent::RunFinished { .. } => "run-finished".to_owned(),
        SmedEvent::RunFailed { .. } => "run-failed".to_owned(),
        SmedEvent::ModelChanged { .. } => "model-changed".to_owned(),
        SmedEvent::ModelChangeRefused { code, .. } => format!("model-change-refused:{code}"),
        SmedEvent::FileSaved { path, .. } => format!("file-saved:{path}"),
        SmedEvent::SubagentSpawned { child, .. } => format!("subagent-spawned:{child}"),
        SmedEvent::SubagentResultLate { child, .. } => {
            format!("subagent-result-late:{child}")
        }
        SmedEvent::ReadSetCollision {
            reader,
            writer,
            path,
            ..
        } => format!("read-set-collision:{reader}:{writer}:{path}"),
        SmedEvent::SubagentActivity { child, .. } => format!("subagent-activity:{child}"),
        SmedEvent::RecoveryRequired { work, .. } => {
            format!("recovery-required:{}", work.kind.label())
        }
        SmedEvent::RecoveryResolved { decision, .. } => {
            format!("recovery-resolved:{}", decision.label())
        }
        SmedEvent::SessionEnded { .. } => "session-ended".to_owned(),
        SmedEvent::TriggerFired { trigger, .. } => format!("trigger-fired:{trigger}"),
        SmedEvent::TriggerSettled { trigger, .. } => format!("trigger-settled:{trigger}"),
        SmedEvent::TriggerSkipped { trigger, .. } => format!("trigger-skipped:{trigger}"),
        SmedEvent::TriggerQueued { trigger, .. } => format!("trigger-queued:{trigger}"),
        SmedEvent::TriggerReplaced { trigger, .. } => format!("trigger-replaced:{trigger}"),
        SmedEvent::TriggerDisabled { trigger, .. } => format!("trigger-disabled:{trigger}"),
        SmedEvent::TriggerRearmed { trigger, .. } => format!("trigger-rearmed:{trigger}"),
        SmedEvent::RouteSelected { route, .. } => format!("route-selected:{route}"),
        SmedEvent::RouteAdvanced { route, .. } => format!("route-advanced:{route}"),
        SmedEvent::RouteExhausted { route, .. } => format!("route-exhausted:{route}"),
        SmedEvent::BreakerStateChanged { provider, .. } => {
            format!("breaker-state-changed:{provider}")
        }
        SmedEvent::SpawnEnvelopeArmed { max_children, .. } => {
            format!("spawn-envelope-armed:{max_children}")
        }
        SmedEvent::SpawnEnvelopeDrawn {
            children,
            children_remaining,
            ..
        } => format!("spawn-envelope-drawn:{children}/{children_remaining}"),
        SmedEvent::SpawnEnvelopeCleared { reason, .. } => {
            format!("spawn-envelope-cleared:{reason:?}")
        }
        SmedEvent::PolicyClamped { from, to, tier, .. } => {
            format!(
                "policy-clamped:{}->{}:{}",
                from.label(),
                to.label(),
                tier.label()
            )
        }
        SmedEvent::PlanInterviewStarted { .. } => "plan-interview-started".to_owned(),
        SmedEvent::PlanQuestionAsked { .. } => "plan-question-asked".to_owned(),
        SmedEvent::PlanQuestionAnswered { .. } => "plan-question-answered".to_owned(),
        SmedEvent::PlanPrdProposed { .. } => "plan-prd-proposed".to_owned(),
        SmedEvent::PlanProposed { .. } => "plan-proposed".to_owned(),
        SmedEvent::PlanReviewed { .. } => "plan-reviewed".to_owned(),
        SmedEvent::PlanApproved { .. } => "plan-approved".to_owned(),
        SmedEvent::PlanHandoffCreated { .. } => "plan-handoff-created".to_owned(),
        SmedEvent::CouncilReviewed { .. } => "council-reviewed".to_owned(),
        SmedEvent::CouncilFindingDispositionRecorded { .. } => {
            "council-finding-disposition-recorded".to_owned()
        }
        SmedEvent::CouncilAmendmentProposed { amendment, .. } => {
            format!("council-amendment-proposed:{}", amendment.path)
        }
        SmedEvent::ReviewNoteRecorded { thread, .. } => format!("review-note:{thread}"),
        SmedEvent::ReviewCommentAdded { thread, .. } => format!("review-comment:{thread}"),
        SmedEvent::ReviewRequestSent { threads, .. } => {
            format!("review-request-sent:{}", threads.len())
        }
        SmedEvent::ReviewRequestAnswered { threads, .. } => {
            format!("review-request-answered:{}", threads.len())
        }
        SmedEvent::DecisionTicketOpened { ticket, .. } => format!("ticket-opened:{}", ticket.id),
        SmedEvent::DecisionTicketResolved { resolution, .. } => {
            format!("ticket-resolved:{}", resolution.id)
        }
        SmedEvent::ImportedItemFetched { item, .. } => format!("imported-fetched:{}", item.id),
        SmedEvent::ImportedItemRefreshed { item, .. } => {
            format!("imported-refreshed:{}", item.id)
        }
        SmedEvent::ImportedActRecorded { act, .. } => format!("imported-act:{}", act.act_id),
        SmedEvent::ImportedCommentRecorded { item_id, .. } => {
            format!("imported-comment:{item_id}")
        }
    }
}

fn assert_complete_transcript(stored: &[smed::core::event::StoredEvent]) {
    let actual = stored
        .iter()
        .map(|event| event_label(&event.event))
        .collect::<Vec<_>>();
    let expected = [
        "session-created",
        "message:user",
        "run-started",
        "usage",
        "message:assistant",
        "proposed:read_file",
        "completed:read_file",
        "usage",
        "message:assistant",
        "proposed:edit_file",
        "approval-resolved",
        "completed:edit_file",
        "usage",
        "message:assistant",
        "proposed:run_command",
        "approval-resolved",
        "completed:run_command",
        "usage",
        "message:assistant",
        "proposed:finish_task",
        "completed:finish_task",
        "run-finished",
    ];
    assert_eq!(actual, expected);
}

async fn assert_evidence_transcript(
    runtime: &Runtime,
    store: &InMemoryEventStore,
    edit_approval: ApprovalId,
) {
    let session = runtime.snapshot().session.expect("session");
    let stored = store.events(session).await.expect("stored transcript");
    assert_complete_transcript(&stored);
    let edit_decision = stored
        .iter()
        .find(|event| {
            matches!(event.event, SmedEvent::ApprovalResolved { approval, .. } if approval == edit_approval)
        })
        .expect("edit decision");
    let edit_result = stored
        .iter()
        .find(|event| {
            matches!(event.event, SmedEvent::ToolCompleted { ref name, .. } if name == "edit_file")
        })
        .expect("edit result");
    assert!(
        edit_decision.sequence < edit_result.sequence,
        "approval must be stored before mutation completion"
    );

    let command_result = stored
        .iter()
        .find(|event| {
            matches!(
                event.event,
                SmedEvent::ToolCompleted {
                    ref name,
                    result: smed::core::message::ToolResult {
                        effect: ToolEffect::Command { success: true, .. },
                        ..
                    },
                    ..
                } if name == "run_command"
            )
        })
        .expect("successful command evidence");
    assert!(edit_result.sequence < command_result.sequence);
    let finish_proposal = stored
        .iter()
        .find_map(|event| match &event.event {
            SmedEvent::ToolProposed { call, .. } if call.name == "finish_task" => Some(call),
            _ => None,
        })
        .expect("finish proposal");
    let cited = finish_proposal
        .arguments
        .get("evidence_event_ids")
        .and_then(serde_json::Value::as_array)
        .expect("evidence array");
    assert_eq!(cited, &[serde_json::json!(command_result.id.to_string())]);
    assert!(
        stored
            .iter()
            .all(|event| !matches!(event.event, SmedEvent::RunFailed { .. }))
    );
}

#[tokio::test]
async fn guarded_loop_persists_intent_before_effect_and_finishes_with_evidence() {
    let repo = TempDir::new().expect("temp repository");
    let fixture = repo.path().join("fixture.txt");
    std::fs::write(&fixture, "before\n").expect("fixture");
    git(repo.path(), &["init", "--quiet"]);
    git(repo.path(), &["add", "fixture.txt"]);

    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::GuardedLoop));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: repo.path().to_owned(),
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
            text: "update the fixture and verify it".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    let edit = wait_for(&mut events, "edit approval", |event| {
        matches!(event, SmedEvent::ToolProposed { call, approval: Some(_), .. } if call.name == "edit_file")
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&fixture).expect("fixture remains readable"),
        "before\n",
        "proposal must be durable before the edit occurs"
    );
    let edit_approval = approval_from(edit, "edit_file");
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval: edit_approval,
            decision: ApprovalDecision::AutoByPolicy,
        })
        .await
        .expect("policy-only decision is ignored");
    assert_eq!(
        std::fs::read_to_string(&fixture).expect("fixture still unchanged"),
        "before\n",
        "a client must not impersonate full-auto policy"
    );
    assert_eq!(
        runtime
            .snapshot()
            .pending_approval
            .as_ref()
            .map(|value| value.id),
        Some(edit_approval)
    );
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval: edit_approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve edit");

    let command = wait_for(&mut events, "command approval", |event| {
        matches!(event, SmedEvent::ToolProposed { call, approval: Some(_), .. } if call.name == "run_command")
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&fixture).expect("fixture edited"),
        "after\n"
    );
    let command_approval = approval_from(command, "run_command");
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval: command_approval,
            decision: ApprovalDecision::ApproveExactForSession,
        })
        .await
        .expect("approve command");

    let terminal = wait_for(&mut events, "verified finish", |event| {
        matches!(
            event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        )
    })
    .await;
    match terminal {
        SmedEvent::RunFinished { reason, .. } => assert_eq!(reason, FinishReason::Stop),
        other => panic!("guarded loop failed: {other:?}"),
    }

    assert_evidence_transcript(&runtime, &store, edit_approval).await;
}

#[tokio::test]
async fn mutation_cannot_be_marked_verified_without_later_command_evidence() {
    let repo = TempDir::new().expect("temp repository");
    std::fs::write(repo.path().join("fixture.txt"), "before\n").expect("fixture");
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::EvidenceMissing));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: repo.path().to_owned(),
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
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("set policy");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "make an unverified edit".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "terminal event", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    assert_eq!(
        std::fs::read_to_string(repo.path().join("fixture.txt")).expect("fixture"),
        "after\n"
    );
    let refusal = runtime
        .snapshot()
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            ContentBlock::ToolResult { name, result, .. } if name == "finish_task" => {
                Some(result.outcome.clone())
            }
            _ => None,
        });
    assert_eq!(
        refusal,
        Some(ToolOutcome::Refused(
            smed::core::error::ReasonCode::CompletionEvidenceMissing
        ))
    );
}

#[tokio::test]
async fn full_auto_is_audited_and_does_not_survive_resume() {
    let repo = TempDir::new().expect("temp repository");
    let fixture = repo.path().join("fixture.txt");
    std::fs::write(&fixture, "before\n").expect("fixture");
    git(repo.path(), &["init", "--quiet"]);
    git(repo.path(), &["add", "fixture.txt"]);

    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::GuardedLoop));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: repo.path().to_owned(),
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
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set full-auto");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "update and verify".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "full-auto finish", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    let session = runtime.snapshot().session.expect("session");
    let stored = store.events(session).await.expect("history");
    let auto_resolutions = stored
        .iter()
        .filter(|stored| {
            matches!(
                stored.event,
                SmedEvent::ApprovalResolved {
                    decision: ApprovalDecision::AutoByPolicy,
                    ..
                }
            )
        })
        .count();
    assert_eq!(auto_resolutions, 2, "edit and command must be audited");
    assert!(runtime.snapshot().pending_approval.is_none());
    runtime.close().await.expect("close first runtime");

    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    let resumed = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    resumed
        .dispatch(SmedCommand::OpenProject {
            root: repo.path().to_owned(),
        })
        .await
        .expect("reopen project");
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    assert_eq!(resumed.snapshot().policy, PolicyMode::Ask);
    resumed.close().await.expect("close resumed runtime");
}

#[tokio::test]
async fn full_auto_cannot_edit_outside_the_workspace() {
    let parent = TempDir::new().expect("temp parent");
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let outside = parent.path().join("outside.txt");
    std::fs::write(&outside, "safe\n").expect("outside fixture");

    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> =
        Arc::new(FakeProvider::new(FakeScript::OutsideWorkspaceWrite));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: workspace.clone(),
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
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set full-auto");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "edit outside".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "refused finish", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    assert_eq!(std::fs::read_to_string(outside).expect("outside"), "safe\n");
    let refused = runtime
        .snapshot()
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    result: smed::core::message::ToolResult {
                        outcome: ToolOutcome::Refused(ReasonCode::PathOutsideWorkspace),
                        ..
                    },
                    ..
                }
            )
        });
    assert!(refused, "containment refusal must reach the model");
    runtime.close().await.expect("close runtime");
}

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

use mjolnr::core::command::{ApprovalDecision, ApprovalId, MjolnrCommand};
use mjolnr::core::error::ReasonCode;
use mjolnr::core::event::{FinishReason, MjolnrEvent};
use mjolnr::core::message::{ContentBlock, Role, ToolEffect, ToolOutcome};
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::policy::PolicyMode;
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::{FakeProvider, FakeScript};
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;
use tempfile::TempDir;

async fn wait_for(
    events: &mut mjolnr::core::runtime::RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&MjolnrEvent) -> bool,
) -> MjolnrEvent {
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

fn approval_from(event: MjolnrEvent, expected_tool: &str) -> ApprovalId {
    match event {
        MjolnrEvent::ToolProposed {
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
fn event_label(event: &MjolnrEvent) -> String {
    match event {
        MjolnrEvent::SessionCreated { .. } => "session-created".to_owned(),
        MjolnrEvent::MessageAppended { message, .. } => match message.role {
            Role::System => "message:system".to_owned(),
            Role::User => "message:user".to_owned(),
            Role::Assistant => "message:assistant".to_owned(),
            Role::Tool => "message:tool".to_owned(),
        },
        MjolnrEvent::RunStarted { .. } => "run-started".to_owned(),
        MjolnrEvent::TextDelta { .. } => "text-delta".to_owned(),
        MjolnrEvent::ReasoningDelta { .. } => "reasoning-delta".to_owned(),
        MjolnrEvent::ToolAssembling { name, .. } => format!("assembling:{name}"),
        MjolnrEvent::QuotaReported { .. } => "quota".to_owned(),
        MjolnrEvent::QuotaBoundaryReached { .. } => "quota-boundary".to_owned(),
        MjolnrEvent::HandoffCreated { .. } => "handoff-created".to_owned(),
        MjolnrEvent::UsageReported { .. } => "usage".to_owned(),
        MjolnrEvent::PolicyChanged { .. } => "policy-changed".to_owned(),
        MjolnrEvent::ExtensionLoaded { name, .. } => format!("extension-loaded:{name}"),
        MjolnrEvent::ToolProposed { call, .. } => format!("proposed:{}", call.name),
        MjolnrEvent::ApprovalResolved { .. } => "approval-resolved".to_owned(),
        MjolnrEvent::ToolCompleted { name, .. } => format!("completed:{name}"),
        MjolnrEvent::ToolFailed { name, .. } => format!("failed:{name}"),
        MjolnrEvent::BudgetExhausted { .. } => "budget-exhausted".to_owned(),
        MjolnrEvent::RunFinished { .. } => "run-finished".to_owned(),
        MjolnrEvent::RunFailed { .. } => "run-failed".to_owned(),
        MjolnrEvent::ModelChanged { .. } => "model-changed".to_owned(),
        MjolnrEvent::ModelChangeRefused { code, .. } => format!("model-change-refused:{code}"),
        MjolnrEvent::FileSaved { path, .. } => format!("file-saved:{path}"),
        MjolnrEvent::SubagentSpawned { child, .. } => format!("subagent-spawned:{child}"),
        MjolnrEvent::SubagentResultLate { child, .. } => {
            format!("subagent-result-late:{child}")
        }
        MjolnrEvent::ReadSetCollision {
            reader,
            writer,
            path,
            ..
        } => format!("read-set-collision:{reader}:{writer}:{path}"),
        MjolnrEvent::SubagentActivity { child, .. } => format!("subagent-activity:{child}"),
        MjolnrEvent::RecoveryRequired { work, .. } => {
            format!("recovery-required:{}", work.kind.label())
        }
        MjolnrEvent::RecoveryResolved { decision, .. } => {
            format!("recovery-resolved:{}", decision.label())
        }
        MjolnrEvent::SessionEnded { .. } => "session-ended".to_owned(),
        MjolnrEvent::TriggerFired { trigger, .. } => format!("trigger-fired:{trigger}"),
        MjolnrEvent::TriggerSettled { trigger, .. } => format!("trigger-settled:{trigger}"),
        MjolnrEvent::TriggerSkipped { trigger, .. } => format!("trigger-skipped:{trigger}"),
        MjolnrEvent::TriggerQueued { trigger, .. } => format!("trigger-queued:{trigger}"),
        MjolnrEvent::TriggerReplaced { trigger, .. } => format!("trigger-replaced:{trigger}"),
        MjolnrEvent::TriggerDisabled { trigger, .. } => format!("trigger-disabled:{trigger}"),
        MjolnrEvent::TriggerRearmed { trigger, .. } => format!("trigger-rearmed:{trigger}"),
        MjolnrEvent::RouteSelected { route, .. } => format!("route-selected:{route}"),
        MjolnrEvent::RouteAdvanced { route, .. } => format!("route-advanced:{route}"),
        MjolnrEvent::RouteExhausted { route, .. } => format!("route-exhausted:{route}"),
        MjolnrEvent::BreakerStateChanged { provider, .. } => {
            format!("breaker-state-changed:{provider}")
        }
        MjolnrEvent::SpawnEnvelopeArmed { max_children, .. } => {
            format!("spawn-envelope-armed:{max_children}")
        }
        MjolnrEvent::SpawnEnvelopeDrawn {
            children,
            children_remaining,
            ..
        } => format!("spawn-envelope-drawn:{children}/{children_remaining}"),
        MjolnrEvent::SpawnEnvelopeCleared { reason, .. } => {
            format!("spawn-envelope-cleared:{reason:?}")
        }
        MjolnrEvent::PolicyClamped { from, to, tier, .. } => {
            format!(
                "policy-clamped:{}->{}:{}",
                from.label(),
                to.label(),
                tier.label()
            )
        }
        MjolnrEvent::PlanInterviewStarted { .. } => "plan-interview-started".to_owned(),
        MjolnrEvent::PlanQuestionAsked { .. } => "plan-question-asked".to_owned(),
        MjolnrEvent::PlanQuestionAnswered { .. } => "plan-question-answered".to_owned(),
        MjolnrEvent::PlanPrdProposed { .. } => "plan-prd-proposed".to_owned(),
        MjolnrEvent::PlanProposed { .. } => "plan-proposed".to_owned(),
        MjolnrEvent::PlanReviewed { .. } => "plan-reviewed".to_owned(),
        MjolnrEvent::PlanApproved { .. } => "plan-approved".to_owned(),
        MjolnrEvent::PlanHandoffCreated { .. } => "plan-handoff-created".to_owned(),
        MjolnrEvent::CouncilReviewed { .. } => "council-reviewed".to_owned(),
        MjolnrEvent::CouncilFindingDispositionRecorded { .. } => {
            "council-finding-disposition-recorded".to_owned()
        }
        MjolnrEvent::CouncilAmendmentProposed { amendment, .. } => {
            format!("council-amendment-proposed:{}", amendment.path)
        }
        MjolnrEvent::ReviewNoteRecorded { thread, .. } => format!("review-note:{thread}"),
        MjolnrEvent::ReviewCommentAdded { thread, .. } => format!("review-comment:{thread}"),
        MjolnrEvent::ReviewRequestSent { threads, .. } => {
            format!("review-request-sent:{}", threads.len())
        }
        MjolnrEvent::ReviewRequestAnswered { threads, .. } => {
            format!("review-request-answered:{}", threads.len())
        }
        MjolnrEvent::DecisionTicketOpened { ticket, .. } => format!("ticket-opened:{}", ticket.id),
        MjolnrEvent::DecisionTicketResolved { resolution, .. } => {
            format!("ticket-resolved:{}", resolution.id)
        }
        MjolnrEvent::ImportedItemFetched { item, .. } => format!("imported-fetched:{}", item.id),
        MjolnrEvent::ImportedItemRefreshed { item, .. } => {
            format!("imported-refreshed:{}", item.id)
        }
        MjolnrEvent::ImportedActRecorded { act, .. } => format!("imported-act:{}", act.act_id),
        MjolnrEvent::ImportedCommentRecorded { item_id, .. } => {
            format!("imported-comment:{item_id}")
        }
    }
}

fn assert_complete_transcript(stored: &[mjolnr::core::event::StoredEvent]) {
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
            matches!(event.event, MjolnrEvent::ApprovalResolved { approval, .. } if approval == edit_approval)
        })
        .expect("edit decision");
    let edit_result = stored
        .iter()
        .find(|event| {
            matches!(event.event, MjolnrEvent::ToolCompleted { ref name, .. } if name == "edit_file")
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
                MjolnrEvent::ToolCompleted {
                    ref name,
                    result: mjolnr::core::message::ToolResult {
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
            MjolnrEvent::ToolProposed { call, .. } if call.name == "finish_task" => Some(call),
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
            .all(|event| !matches!(event.event, MjolnrEvent::RunFailed { .. }))
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
        .dispatch(MjolnrCommand::OpenProject {
            root: repo.path().to_owned(),
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
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "update the fixture and verify it".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    let edit = wait_for(&mut events, "edit approval", |event| {
        matches!(event, MjolnrEvent::ToolProposed { call, approval: Some(_), .. } if call.name == "edit_file")
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&fixture).expect("fixture remains readable"),
        "before\n",
        "proposal must be durable before the edit occurs"
    );
    let edit_approval = approval_from(edit, "edit_file");
    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
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
        .dispatch(MjolnrCommand::ResolveApproval {
            approval: edit_approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve edit");

    let command = wait_for(&mut events, "command approval", |event| {
        matches!(event, MjolnrEvent::ToolProposed { call, approval: Some(_), .. } if call.name == "run_command")
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&fixture).expect("fixture edited"),
        "after\n"
    );
    let command_approval = approval_from(command, "run_command");
    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
            approval: command_approval,
            decision: ApprovalDecision::ApproveExactForSession,
        })
        .await
        .expect("approve command");

    let terminal = wait_for(&mut events, "verified finish", |event| {
        matches!(
            event,
            MjolnrEvent::RunFinished { .. } | MjolnrEvent::RunFailed { .. }
        )
    })
    .await;
    match terminal {
        MjolnrEvent::RunFinished { reason, .. } => assert_eq!(reason, FinishReason::Stop),
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
        .dispatch(MjolnrCommand::OpenProject {
            root: repo.path().to_owned(),
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
        .dispatch(MjolnrCommand::SetPolicy {
            mode: PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("set policy");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "make an unverified edit".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "terminal event", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
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
            mjolnr::core::error::ReasonCode::CompletionEvidenceMissing
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
        .dispatch(MjolnrCommand::OpenProject {
            root: repo.path().to_owned(),
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
        .dispatch(MjolnrCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set full-auto");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "update and verify".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "full-auto finish", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
    })
    .await;

    let session = runtime.snapshot().session.expect("session");
    let stored = store.events(session).await.expect("history");
    let auto_resolutions = stored
        .iter()
        .filter(|stored| {
            matches!(
                stored.event,
                MjolnrEvent::ApprovalResolved {
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
        .dispatch(MjolnrCommand::OpenProject {
            root: repo.path().to_owned(),
        })
        .await
        .expect("reopen project");
    resumed
        .dispatch(MjolnrCommand::ResumeSession { session })
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
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.clone(),
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
        .dispatch(MjolnrCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set full-auto");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "edit outside".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    wait_for(&mut events, "refused finish", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
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
                    result: mjolnr::core::message::ToolResult {
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

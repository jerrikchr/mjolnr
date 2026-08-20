//! Unit and async integration tests for `ClientBridge`.

#![allow(clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, watch};

use crate::core::client::{
    ClientApprovalDecision, ClientCommand, ClientEvent, ClientFinishReason, ClientMessage,
    ClientPolicy, ClientRecovery, ClientRecoveryDecision, ClientResumeChoice, ClientToolOutcome,
    ClientUpdate,
};
use crate::core::command::{ApprovalDecision, MjolnrCommand};
use crate::core::council::{CouncilContribution, CouncilReview};
use crate::core::directive::DirectiveSource;
use crate::core::error::{MjolnrError, ReasonCode};
use crate::core::event::{FinishReason, MjolnrEvent, RunId, SessionId};
use crate::core::message::{CanonicalMessage, ToolCall, ToolEffect, ToolOutcome, ToolResult};
use crate::core::model::{ModelId, ProviderId};
use crate::core::plan::{
    PlanApproval, PlanHandoff, PlanId, PlanProposal, PlanReview, PlanStage, PlanStep, PlanWorkflow,
    ReviewVerdict, RevisionId,
};
use crate::core::policy::{PendingApproval, PolicyMode};
use crate::core::recovery::{Authority, InterruptedKind, InterruptedWork, RecoveryState};
use crate::core::runtime::{MjolnrRuntime, RuntimeSnapshot, RuntimeSubscription, SnapshotStream};
use crate::core::tool::ToolTier;

use super::bridge::ClientBridge;
use super::command::command_to_mjolnr;
use super::convert::{event_to_client, snapshot_to_client};
use crate::core::client::MAX_ACTIVITY_TEXT;
use crate::core::client::MAX_MESSAGE_TEXT;
use crate::core::client::MAX_SNAPSHOT_MESSAGES;

/// A mock runtime for testing the bridge async behaviors.
#[derive(Debug)]
struct TestRuntime {
    events_tx: broadcast::Sender<MjolnrEvent>,
    snapshot_tx: std::sync::Mutex<Option<watch::Sender<RuntimeSnapshot>>>,
    dispatched_commands: std::sync::Mutex<Vec<MjolnrCommand>>,
    refuse_dispatch_with: std::sync::Mutex<Option<MjolnrError>>,
    board: std::sync::Mutex<Option<Result<crate::core::frontier::BoardOverview, MjolnrError>>>,
}

impl TestRuntime {
    fn new() -> Self {
        let (events_tx, _) = broadcast::channel(16);
        let (snapshot_tx, _) = watch::channel(RuntimeSnapshot::default());
        Self {
            events_tx,
            snapshot_tx: std::sync::Mutex::new(Some(snapshot_tx)),
            dispatched_commands: std::sync::Mutex::new(Vec::new()),
            refuse_dispatch_with: std::sync::Mutex::new(None),
            board: std::sync::Mutex::new(None),
        }
    }

    fn commands(&self) -> Vec<MjolnrCommand> {
        self.dispatched_commands.lock().unwrap().clone()
    }

    fn refuse_dispatch_with(&self, error: MjolnrError) {
        *self.refuse_dispatch_with.lock().unwrap() = Some(error);
    }

    fn answer_board_with(&self, result: Result<crate::core::frontier::BoardOverview, MjolnrError>) {
        *self.board.lock().unwrap() = Some(result);
    }
}

#[async_trait]
impl MjolnrRuntime for TestRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot_tx
            .lock()
            .unwrap()
            .as_ref()
            .map(|tx| tx.borrow().clone())
            .unwrap_or_default()
    }

    fn snapshots(&self) -> SnapshotStream {
        let guard = self.snapshot_tx.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            SnapshotStream::new(tx.subscribe())
        } else {
            let (_, rx) = watch::channel(RuntimeSnapshot::default());
            SnapshotStream::new(rx)
        }
    }

    fn subscribe(&self) -> RuntimeSubscription {
        RuntimeSubscription::new(self.events_tx.subscribe())
    }

    async fn dispatch(&self, command: MjolnrCommand) -> Result<(), MjolnrError> {
        self.dispatched_commands.lock().unwrap().push(command);
        if let Some(error) = self.refuse_dispatch_with.lock().unwrap().take() {
            return Err(error);
        }
        Ok(())
    }

    async fn read_workspace_files(
        &self,
        _request: crate::core::workspace_files::WorkspaceFileRequest,
    ) -> Result<crate::core::workspace_files::WorkspaceFileAnswer, MjolnrError> {
        Err(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "this test runtime opens no project, so there is nothing to read files from",
        ))
    }

    async fn search_workspace(
        &self,
        _filter: crate::core::store::WorkspaceSearchFilter,
    ) -> Result<crate::core::store::WorkspaceSearchPage, MjolnrError> {
        Err(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "workspace search is not yet implemented (contract landed in D4)",
        ))
    }

    async fn query_board(&self) -> Result<crate::core::frontier::BoardOverview, MjolnrError> {
        match self.board.lock().unwrap().take() {
            Some(result) => result,
            None => Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "this test runtime opens no project, so there is no board to answer from",
            )),
        }
    }

    async fn query_repository_history(
        &self,
        _limit: u32,
    ) -> Result<crate::core::repository::RepositoryHistory, MjolnrError> {
        Err(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "this test runtime opens no project, so there is no repository history to answer from",
        ))
    }

    async fn close(&self) -> Result<(), MjolnrError> {
        self.snapshot_tx.lock().unwrap().take();
        Ok(())
    }
}

fn snapshot_with(messages: Vec<CanonicalMessage>) -> RuntimeSnapshot {
    let entries: Vec<crate::core::message::TranscriptEntry> = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            crate::core::message::TranscriptEntry::anchored(
                u64::try_from(index).unwrap_or(0),
                message,
            )
        })
        .collect();
    RuntimeSnapshot {
        messages: Arc::new(entries),
        ..RuntimeSnapshot::default()
    }
}

#[test]
fn the_latest_council_review_reaches_the_client_as_advisory_data() {
    let mut snapshot = snapshot_with(Vec::new());
    snapshot.last_council = Some(CouncilReview {
        review_id: crate::core::council::CouncilReviewId::new(),
        question: "Which boundary should land first?".to_owned(),
        plan_id: None,
        prd_id: None,
        contributions: vec![CouncilContribution {
            role: "plan".to_owned(),
            proposal: "Keep the gate in Rust.".to_owned(),
            critique: Some("Do not let the client approve it.".to_owned()),
        }],
        rounds_conducted: 2,
        artifact: None,
        findings: vec![crate::core::council::CouncilFinding {
            id: crate::core::council::CouncilFindingId::new(),
            section: "Question".to_owned(),
            title: "Council recommendation".to_owned(),
            positions: vec![],
            disposition: None,
        }],
    });

    let client = snapshot_to_client(1, &snapshot);
    let review = client.council.expect("council projection");
    assert_eq!(review.question, "Which boundary should land first?");
    assert_eq!(review.rounds_conducted, 2);
    assert_eq!(review.contributions[0].role, "plan");
    assert_eq!(
        review.contributions[0].critique.as_deref(),
        Some("Do not let the client approve it.")
    );
}

#[test]
fn the_snapshot_is_bounded_with_an_honest_omitted_count() {
    let messages: Vec<CanonicalMessage> = (0..MAX_SNAPSHOT_MESSAGES + 50)
        .map(|index| CanonicalMessage::user(format!("message {index}")))
        .collect();
    let client = snapshot_to_client(1, &snapshot_with(messages));

    assert_eq!(client.messages.len(), MAX_SNAPSHOT_MESSAGES);
    assert_eq!(client.messages_omitted, 50);
    let Some(ClientMessage::User { text, .. }) = client.messages.last() else {
        panic!("expected a user message");
    };
    assert_eq!(
        text,
        &format!("message {}", MAX_SNAPSHOT_MESSAGES + 49),
        "the newest transcript entries are the ones a snapshot keeps"
    );
}

#[test]
fn long_message_text_is_truncated_and_disclosed() {
    let long = "x".repeat(MAX_MESSAGE_TEXT + 100);
    let client = snapshot_to_client(1, &snapshot_with(vec![CanonicalMessage::user(long)]));
    let Some(ClientMessage::User {
        text,
        text_truncated,
        ..
    }) = client.messages.first()
    else {
        panic!("expected a user message");
    };
    assert_eq!(text.chars().count(), MAX_MESSAGE_TEXT);
    assert!(text_truncated);
}

#[test]
fn tool_results_carry_outcome_and_bounded_detail() {
    let result = ToolResult {
        outcome: ToolOutcome::Refused(ReasonCode::PolicyReadOnly),
        content: "y".repeat(MAX_ACTIVITY_TEXT + 10),
        truncated: false,
        effect: ToolEffect::None,
        evidence_event_id: None,
    };
    let client = snapshot_to_client(
        1,
        &snapshot_with(vec![CanonicalMessage::tool_result(
            "call_1",
            "write_file",
            result,
        )]),
    );
    let Some(ClientMessage::Tool {
        outcome,
        reason_code,
        detail_truncated,
        ..
    }) = client.messages.first()
    else {
        panic!("expected a tool message");
    };
    assert_eq!(*outcome, ClientToolOutcome::Refused);
    assert_eq!(reason_code.as_deref(), Some("POLICY_READ_ONLY"));
    assert!(detail_truncated);
}

#[test]
fn approval_and_recovery_convert_without_client_inference() {
    let mut snapshot = snapshot_with(Vec::new());
    snapshot.pending_approval = Some(PendingApproval {
        id: crate::core::command::ApprovalId::new(),
        tool_name: "run_command".to_owned(),
        tier: ToolTier::Execute,
        preview: "/bin/ls".to_owned(),
    });
    snapshot.recovery = RecoveryState::Required(InterruptedWork {
        run: RunId::new(),
        kind: InterruptedKind::EffectUncertain {
            authority: Authority::Policy,
            call: ToolCall {
                id: "c".to_owned(),
                name: "edit_file".to_owned(),
                arguments: serde_json::json!({}),
                provider_signature: None,
            },
            tier: ToolTier::Write,
            preview: "diff".to_owned(),
        },
    });

    let client = snapshot_to_client(7, &snapshot);
    let approval = client.pending_approval.expect("approval");
    assert_eq!(approval.tier, "execute");

    let ClientRecovery::Required {
        kind,
        effect_is_certain,
        tool_name,
        ..
    } = client.recovery
    else {
        panic!("expected recovery state");
    };
    assert_eq!(kind, "EFFECT_UNCERTAIN");
    assert!(!effect_is_certain);
    assert_eq!(tool_name.as_deref(), Some("edit_file"));
}

#[test]
fn plan_workflows_serialize_with_pascal_stage_tags_and_camel_case_fields() {
    let plan_id = PlanId::new();
    let proposal = PlanProposal {
        plan_id,
        revision_id: RevisionId::new(2),
        title: "Bridge the plan state".to_owned(),
        summary: "Expose client-only DTOs for the wire.".to_owned(),
        steps: vec![
            PlanStep {
                index: 1,
                title: "Define DTOs".to_owned(),
                description: "Create client plan structures.".to_owned(),
            },
            PlanStep {
                index: 2,
                title: "Convert snapshot".to_owned(),
                description: "Map runtime workflow deterministically.".to_owned(),
            },
        ],
        proposed_at: parse_timestamp("2026-07-29T04:00:00Z"),
    };
    let review = PlanReview {
        plan_id,
        revision_id: RevisionId::new(2),
        reviewer: "critic".to_owned(),
        verdict: ReviewVerdict::Approve,
        feedback: "Ready for the client.".to_owned(),
        reviewed_at: parse_timestamp("2026-07-29T04:05:00Z"),
    };
    let approval = PlanApproval {
        plan_id,
        revision_id: RevisionId::new(2),
        approver: "owner".to_owned(),
        decision: ReviewVerdict::Approve,
        note: Some("Proceed.".to_owned()),
        approved_at: parse_timestamp("2026-07-29T04:10:00Z"),
    };
    let handoff = PlanHandoff {
        plan_id,
        revision_id: RevisionId::new(2),
        handoff_note: "Implementation begins.".to_owned(),
        created_at: parse_timestamp("2026-07-29T04:15:00Z"),
    };
    let mut workflow = PlanWorkflow::new(plan_id);
    workflow.active_revision = Some(RevisionId::new(2));
    workflow.stage = PlanStage::Reviewed {
        proposal: proposal.clone(),
        reviews: vec![review.clone()],
    };
    workflow.proposals = vec![proposal];
    workflow.reviews = vec![review];
    workflow.approvals = vec![approval];
    workflow.handoffs = vec![handoff];

    let mut snapshot = snapshot_with(Vec::new());
    snapshot.plan = Some(workflow);

    let client = snapshot_to_client(11, &snapshot);
    let json = serde_json::to_value(&client).expect("serialize");

    assert_eq!(json["plan"]["planId"], plan_id.to_string());
    assert_eq!(json["plan"]["activeRevision"], 2);
    assert!(json["plan"]["stage"].get("Reviewed").is_some());
    assert_eq!(
        json["plan"]["stage"]["Reviewed"]["proposal"]["steps"][0]["description"],
        "Create client plan structures."
    );
    assert_eq!(
        json["plan"]["stage"]["Reviewed"]["reviews"][0]["verdict"],
        "approve"
    );
    assert_eq!(json["plan"]["approvals"][0]["decision"], "approve");
    assert_eq!(
        json["plan"]["handoffs"][0]["handoffNote"],
        "Implementation begins."
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn the_command_allowlist_maps_one_for_one() {
    let session = SessionId::new().to_string();
    let approval = crate::core::command::ApprovalId::new().to_string();
    // Minted once and cloned so both sides of the imported-item cases carry
    // the same id: `valid_imported_item()` mints a fresh one per call.
    let imported_item = valid_imported_item();
    let cases: Vec<(ClientCommand, MjolnrCommand)> = vec![
        (
            ClientCommand::OpenProject {
                root: "/work".to_owned(),
            },
            MjolnrCommand::OpenProject {
                root: std::path::PathBuf::from("/work"),
            },
        ),
        (
            ClientCommand::CreateSession {
                provider: "openai".to_owned(),
                model: "gpt-4o".to_owned(),
            },
            MjolnrCommand::CreateSession {
                provider: ProviderId::new("openai"),
                model: ModelId::new("gpt-4o"),
            },
        ),
        (
            ClientCommand::ResumeSession {
                session: session.clone(),
            },
            MjolnrCommand::ResumeSession {
                session: SessionId::from_uuid(uuid::Uuid::parse_str(&session).unwrap()),
            },
        ),
        (
            ClientCommand::ResolveResume {
                choice: ClientResumeChoice::Full,
            },
            MjolnrCommand::ResolveResume {
                choice: crate::core::continuation::ResumeChoice::Full,
            },
        ),
        (
            ClientCommand::SendMessage {
                text: "hello".to_owned(),
            },
            MjolnrCommand::SendUserMessage {
                text: "hello".to_owned(),
                source: DirectiveSource::Human,
            },
        ),
        (ClientCommand::CancelRun, MjolnrCommand::CancelRun),
        (
            ClientCommand::ResolveApproval {
                approval: approval.clone(),
                decision: ClientApprovalDecision::ApproveExactForSession,
            },
            MjolnrCommand::ResolveApproval {
                approval: crate::core::command::ApprovalId::from_uuid(
                    uuid::Uuid::parse_str(&approval).unwrap(),
                ),
                decision: ApprovalDecision::ApproveExactForSession,
            },
        ),
        (
            ClientCommand::ResolveRecovery {
                decision: ClientRecoveryDecision::EndSession,
            },
            MjolnrCommand::ResolveRecovery {
                decision: crate::core::recovery::RecoveryDecision::EndSession,
            },
        ),
        (
            ClientCommand::SetPolicy {
                policy: ClientPolicy::ReadOnly,
            },
            MjolnrCommand::SetPolicy {
                mode: PolicyMode::ReadOnly,
            },
        ),
        (ClientCommand::EndSession, MjolnrCommand::EndSession),
        (
            ClientCommand::CreateWorktree {
                name: "branch1".to_owned(),
                base_revision: "main".to_owned(),
            },
            MjolnrCommand::CreateWorktree {
                name: "branch1".to_owned(),
                base_revision: "main".to_owned(),
            },
        ),
        (
            ClientCommand::ForkWork {
                name: "branch1".to_owned(),
                base_revision: "main".to_owned(),
            },
            MjolnrCommand::ForkWork {
                name: "branch1".to_owned(),
                base_revision: "main".to_owned(),
            },
        ),
        (
            ClientCommand::StartChild {
                name: "child1".to_owned(),
                directive: "do work".to_owned(),
                policy_ceiling: Some(ClientPolicy::Ask),
                budget: Some(10),
            },
            MjolnrCommand::StartChild {
                name: "child1".to_owned(),
                directive: "do work".to_owned(),
                policy_ceiling: Some(PolicyMode::Ask),
                budget: Some(10),
            },
        ),
        // An omitted ceiling maps to `None`: inherit the parent's policy
        // unchanged. Children inherit less, never more (AGENTS.md §11.4).
        (
            ClientCommand::StartChild {
                name: "child2".to_owned(),
                directive: "do more work".to_owned(),
                policy_ceiling: None,
                budget: None,
            },
            MjolnrCommand::StartChild {
                name: "child2".to_owned(),
                directive: "do more work".to_owned(),
                policy_ceiling: None,
                budget: None,
            },
        ),
        (
            ClientCommand::CancelChild {
                name: "child1".to_owned(),
            },
            MjolnrCommand::CancelChild {
                name: "child1".to_owned(),
            },
        ),
        (
            ClientCommand::PreserveBranch {
                name: "branch1".to_owned(),
            },
            MjolnrCommand::PreserveBranch {
                name: "branch1".to_owned(),
            },
        ),
        (
            ClientCommand::SettleChild {
                name: "child1".to_owned(),
            },
            MjolnrCommand::SettleChild {
                name: "child1".to_owned(),
            },
        ),
        (
            ClientCommand::DiscardSettledWorktree {
                name: "branch1".to_owned(),
            },
            MjolnrCommand::DiscardSettledWorktree {
                name: "branch1".to_owned(),
            },
        ),
        (
            ClientCommand::ImportWorkItem {
                item: imported_item.clone(),
            },
            MjolnrCommand::ImportWorkItem {
                item: imported_item.clone(),
            },
        ),
        (
            ClientCommand::RefreshImportedItem {
                expected_revision: "rev1".to_owned(),
                item: imported_item.clone(),
            },
            MjolnrCommand::RefreshImportedItem {
                expected_revision: "rev1".to_owned(),
                item: imported_item.clone(),
            },
        ),
    ];

    for (client_command, expected) in cases {
        let mapped = command_to_mjolnr(&client_command)
            .expect("mapped")
            .expect("a runtime command");
        assert_eq!(mapped, expected);
    }

    assert!(
        command_to_mjolnr(&ClientCommand::RequestSnapshot)
            .expect("mapped")
            .is_none(),
        "RequestSnapshot is bridge-handled and never reaches the runtime"
    );
}

#[test]
fn a_repository_refresh_actually_reaches_the_runtime() {
    // Asserted as `Some(RefreshRepository)` rather than merely "does not
    // error". A missing arm in `command_to_mjolnr` falls through to the
    // catch-all `Ok(None)`, the bridge reads that as "no command, emit
    // snapshot", and the user gets a re-sent snapshot with nothing refreshed —
    // the exact silent no-op the D6 report recorded. A weaker assertion
    // passes against that bug.
    let mapped = command_to_mjolnr(&ClientCommand::RefreshRepository)
        .expect("mapped")
        .expect("a refresh must reach the runtime, not be swallowed as a no-op");
    assert_eq!(mapped, MjolnrCommand::RefreshRepository);
}

#[test]
fn a_repository_refresh_carries_no_root_a_caller_could_supply() {
    // The command is deliberately field-free: accepting a root here would be a
    // second way to point mjolnr at a directory, bypassing every refusal
    // `OpenProject` applies. This pins that on the wire, so adding a field
    // fails here before it ships.
    let json = serde_json::to_value(ClientCommand::RefreshRepository).expect("serialize");
    let object = json.as_object().expect("a tagged object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        vec!["type"],
        "a refresh must carry nothing but its tag, got {json}"
    );
}

#[test]
fn a_save_file_carries_the_client_digest_and_text_to_the_runtime() {
    let mapped = command_to_mjolnr(&ClientCommand::SaveFile {
        path: "src/main.rs".to_owned(),
        expected_digest: "a".repeat(64),
        text: "fn main() {}\n".to_owned(),
    })
    .expect("mapped")
    .expect("a save must reach the runtime");

    assert_eq!(
        mapped,
        MjolnrCommand::SaveFile {
            path: "src/main.rs".to_owned(),
            expected_digest: "a".repeat(64),
            text: "fn main() {}\n".to_owned(),
        }
    );
}

#[test]
fn a_send_message_always_arrives_as_human_text() {
    let mapped = command_to_mjolnr(&ClientCommand::SendMessage {
        text: "from the client".to_owned(),
    })
    .expect("mapped")
    .expect("command");
    assert!(
        matches!(
            mapped,
            MjolnrCommand::SendUserMessage {
                source: DirectiveSource::Human,
                ..
            }
        ),
        "client text is always human-sourced: {mapped:?}"
    );
}

#[test]
fn plan_question_commands_preserve_the_explicit_workflow_identity() {
    let plan_id = crate::core::plan::PlanId::new();
    let question_id = crate::core::plan::QuestionId::new();
    let asked = command_to_mjolnr(&ClientCommand::AskPlanQuestion {
        plan_id: plan_id.to_string(),
        prompt: "Which scope?".to_string(),
        options: vec!["Narrow".to_string()],
        is_multi_select: false,
    })
    .expect("mapped")
    .expect("command");
    let answered = command_to_mjolnr(&ClientCommand::AnswerPlanQuestion {
        plan_id: plan_id.to_string(),
        question_id: question_id.to_string(),
        selected_options: vec!["Narrow".to_string()],
        freeform_text: None,
    })
    .expect("mapped")
    .expect("command");

    assert!(matches!(
        asked,
        MjolnrCommand::AskPlanQuestion {
            plan_id: mapped,
            ..
        } if mapped == plan_id
    ));
    assert!(matches!(
        answered,
        MjolnrCommand::AnswerPlanQuestion {
            plan_id: mapped,
            answer,
        } if mapped == plan_id && answer.question_id == question_id
    ));
}

#[test]
fn every_plan_command_maps_its_identity_revision_and_decision() {
    let plan_id = crate::core::plan::PlanId::new();
    let id = plan_id.to_string();
    let steps = vec![crate::core::client::ClientPlanStep {
        index: 1,
        title: "Inspect".to_owned(),
        description: "Read the contract".to_owned(),
    }];

    let proposed = command_to_mjolnr(&ClientCommand::ProposePlan {
        plan_id: id.clone(),
        revision: 2,
        title: "Plan".to_owned(),
        summary: "Summary".to_owned(),
        steps,
    })
    .expect("mapped")
    .expect("command");
    let reviewed = command_to_mjolnr(&ClientCommand::ReviewPlan {
        plan_id: id.clone(),
        revision: 2,
        reviewer: "Architect".to_owned(),
        verdict: crate::core::client::ClientReviewVerdict::Iterate,
        feedback: "Tighten it".to_owned(),
    })
    .expect("mapped")
    .expect("command");
    let approved = command_to_mjolnr(&ClientCommand::ApprovePlan {
        plan_id: id.clone(),
        revision: 2,
        decision: crate::core::client::ClientReviewVerdict::Approve,
        note: Some("Proceed".to_owned()),
    })
    .expect("mapped")
    .expect("command");
    let handed_off = command_to_mjolnr(&ClientCommand::HandoffPlan {
        plan_id: id,
        revision: 2,
        note: "Execute".to_owned(),
    })
    .expect("mapped")
    .expect("command");

    assert!(matches!(
        proposed,
        MjolnrCommand::ProposePlan { proposal }
            if proposal.plan_id == plan_id
                && proposal.revision_id == RevisionId::new(2)
                && proposal.steps.len() == 1
    ));
    assert!(matches!(
        reviewed,
        MjolnrCommand::ReviewPlan { review }
            if review.plan_id == plan_id
                && review.revision_id == RevisionId::new(2)
                && review.verdict == ReviewVerdict::Iterate
    ));
    assert!(matches!(
        approved,
        MjolnrCommand::ApprovePlan { approval }
            if approval.plan_id == plan_id
                && approval.revision_id == RevisionId::new(2)
                && approval.decision == ReviewVerdict::Approve
                && approval.approver == "Human"
    ));
    assert!(matches!(
        handed_off,
        MjolnrCommand::HandoffPlan { handoff }
            if handoff.plan_id == plan_id
                && handoff.revision_id == RevisionId::new(2)
    ));
}

#[test]
fn council_disposition_command_preserves_review_finding_and_note() {
    let review_id = crate::core::council::CouncilReviewId::new();
    let finding_id = crate::core::council::CouncilFindingId::new();
    let mapped = command_to_mjolnr(&ClientCommand::ResolveCouncilFinding {
        review_id: review_id.to_string(),
        finding_id: finding_id.to_string(),
        disposition: crate::core::client::ClientCouncilDisposition::Defer,
        note: Some("Need a human decision".to_owned()),
    })
    .expect("mapped")
    .expect("command");

    assert!(matches!(
        mapped,
        MjolnrCommand::ResolveCouncilFinding {
            review_id: mapped_review,
            finding_id: mapped_finding,
            disposition: crate::core::council::CouncilDisposition::Defer,
            note: Some(note),
        } if mapped_review == review_id && mapped_finding == finding_id && note == "Need a human decision"
    ));
}

#[test]
fn council_amendment_command_carries_the_review_and_refuses_a_malformed_id() {
    let review_id = crate::core::council::CouncilReviewId::new();
    let mapped = command_to_mjolnr(&ClientCommand::ProposeCouncilAmendment {
        review_id: review_id.to_string(),
    })
    .expect("mapped")
    .expect("command");

    assert!(matches!(
        mapped,
        MjolnrCommand::ProposeCouncilAmendment {
            review_id: mapped_review,
        } if mapped_review == review_id
    ));

    command_to_mjolnr(&ClientCommand::ProposeCouncilAmendment {
        review_id: "not-a-uuid".to_owned(),
    })
    .expect_err("a malformed review id is refused at the bridge");
}

#[test]
fn malformed_commands_are_refused_with_stable_codes() {
    let cases = vec![
        ClientCommand::ResumeSession {
            session: "not-a-uuid".to_owned(),
        },
        ClientCommand::ResumeSession {
            session: String::new(),
        },
        ClientCommand::ResolveApproval {
            approval: "zzz".to_owned(),
            decision: ClientApprovalDecision::ApproveOnce,
        },
        ClientCommand::SendMessage {
            text: "   ".to_owned(),
        },
        ClientCommand::OpenProject {
            root: String::new(),
        },
        ClientCommand::CreateSession {
            provider: String::new(),
            model: "m".to_owned(),
        },
        // Phase D2: child-run inputs are bounded at the bridge, before they
        // can reach `git worktree` / the agent loop as hostile identifiers.
        ClientCommand::CreateWorktree {
            name: String::new(),
            base_revision: "abc123".to_owned(),
        },
        ClientCommand::CreateWorktree {
            name: "../escape".to_owned(),
            base_revision: "abc123".to_owned(),
        },
        ClientCommand::CreateWorktree {
            name: "-flag".to_owned(),
            base_revision: "abc123".to_owned(),
        },
        ClientCommand::CreateWorktree {
            name: "a".repeat(crate::core::client::MAX_CHILD_RUN_NAME_BYTES + 1),
            base_revision: "abc123".to_owned(),
        },
        ClientCommand::CreateWorktree {
            name: "valid".to_owned(),
            base_revision: String::new(),
        },
        ClientCommand::CreateWorktree {
            name: "valid".to_owned(),
            base_revision: "bad revision with spaces".to_owned(),
        },
        ClientCommand::StartChild {
            name: "child".to_owned(),
            directive: "   ".to_owned(),
            policy_ceiling: None,
            budget: None,
        },
        ClientCommand::StartChild {
            name: "child".to_owned(),
            directive: "x".repeat(crate::core::client::MAX_CHILD_RUN_DIRECTIVE_BYTES + 1),
            policy_ceiling: None,
            budget: None,
        },
        ClientCommand::CancelChild {
            name: String::new(),
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "refusals carry the stable code, not prose: {command:?}"
        );
    }
}

/// The D2 guard that replaced a `todo!()` panic: a child-run command that
/// passes bridge validation and reaches the runtime is answered with a typed
/// `WorkspaceCapabilityUnavailable` refusal — never a panic, never a silent
/// no-op. When execution lands this test is replaced by lifecycle coverage
/// (e.g. cancelling a settled child must refuse differently).
#[tokio::test]
async fn child_run_commands_fail_closed_with_a_typed_refusal() {
    let runtime = Arc::new(TestRuntime::new());
    runtime.refuse_dispatch_with(MjolnrError::workspace_refused(
        ReasonCode::WorkspaceCapabilityUnavailable,
        "Capability 'startChild' is unavailable: child-run execution is not yet implemented",
    ));
    let bridge = ClientBridge::start(runtime.clone());

    for command in [
        ClientCommand::StartChild {
            name: "child".to_owned(),
            directive: "implement the feature".to_owned(),
            policy_ceiling: None,
            budget: None,
        },
        ClientCommand::CancelChild {
            name: "child".to_owned(),
        },
        ClientCommand::SettleChild {
            name: "child".to_owned(),
        },
    ] {
        let error = bridge
            .dispatch(command)
            .await
            .expect_err("a child-run command must be refused, not dropped");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::WorkspaceCapabilityUnavailable),
            "the typed code must round-trip to the client: {error}"
        );
        runtime.refuse_dispatch_with(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "Capability 'childRun' is unavailable: child-run execution is not yet implemented",
        ));
    }
}

/// Every Phase D5 value becomes an argv element of a `git` process, so the
/// bridge is where hostile input stops. Each case here is a refusal the
/// repository module must never have to untangle.
#[test]
fn repository_commands_refuse_hostile_or_unbounded_input_at_the_bridge() {
    let cases = vec![
        // An empty list would expand to "everything" or "nothing" depending on
        // the git subcommand — neither is the caller's stated intent.
        ClientCommand::StagePaths { paths: Vec::new() },
        ClientCommand::StagePaths {
            paths: vec![String::new()],
        },
        // git reads a leading `-` as a flag on several argument forms.
        ClientCommand::StagePaths {
            paths: vec!["-rf".to_owned()],
        },
        ClientCommand::StagePaths {
            paths: vec!["../outside/the/repo".to_owned()],
        },
        ClientCommand::StagePaths {
            paths: vec!["/etc/passwd".to_owned()],
        },
        ClientCommand::StagePaths {
            paths: vec!["file\nwith\ncontrol".to_owned()],
        },
        ClientCommand::StagePaths {
            paths: vec!["a.rs".to_owned(); crate::core::client::MAX_REPOSITORY_PATHS + 1],
        },
        ClientCommand::Unstage { paths: Vec::new() },
        ClientCommand::StageHunks {
            path: "a.rs".to_owned(),
            hunk_indices: Vec::new(),
        },
        // Branch names land in `.git/refs/heads/`; git's own check-ref-format
        // rules are the contract.
        ClientCommand::CreateBranch {
            name: "has space".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        ClientCommand::CreateBranch {
            name: "bad..range".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        ClientCommand::CreateBranch {
            name: "ref@{0}".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        ClientCommand::CreateBranch {
            name: "tip.lock".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        ClientCommand::CreateBranch {
            name: "-delete-me".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        ClientCommand::CreateBranch {
            name: "ok".to_owned(),
            base_revision: String::new(),
        },
        // An empty message would make mjolnr the author of an unauditable
        // record, and an absent expected revision makes the staleness guard
        // opt-in.
        ClientCommand::Commit {
            message: "   ".to_owned(),
            expected_index_revision: "abc123".to_owned(),
        },
        ClientCommand::Commit {
            message: "real message".to_owned(),
            expected_index_revision: String::new(),
        },
        ClientCommand::IntegrateChildBranch {
            name: "child".to_owned(),
            message: String::new(),
            expected_head: "abc123".to_owned(),
        },
        ClientCommand::IntegrateChildBranch {
            name: "child".to_owned(),
            message: "take it".to_owned(),
            expected_head: String::new(),
        },
        ClientCommand::IntegrateUpstream {
            message: String::new(),
            expected_head: "abc123".to_owned(),
        },
        ClientCommand::IntegrateUpstream {
            message: "take the fetched upstream".to_owned(),
            expected_head: String::new(),
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "refusals carry the stable code, not prose: {command:?}"
        );
    }
}

#[test]
fn clone_and_rebase_commands_refuse_unsafe_input_at_the_bridge() {
    let cases = [
        ClientCommand::CloneProject {
            source: String::new(),
            destination: "/tmp/new-project".to_owned(),
        },
        ClientCommand::CloneProject {
            source: "-danger".to_owned(),
            destination: "/tmp/new-project".to_owned(),
        },
        ClientCommand::CloneProject {
            source: "https://example.invalid/repo".to_owned(),
            destination: "relative-project".to_owned(),
        },
        ClientCommand::Rebase {
            onto: String::new(),
            expected_head: "deadbeef".to_owned(),
        },
        ClientCommand::Rebase {
            onto: "-main".to_owned(),
            expected_head: "deadbeef".to_owned(),
        },
        ClientCommand::Rebase {
            onto: "main".to_owned(),
            expected_head: String::new(),
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    }
}

#[test]
fn well_formed_repository_commands_map_through_with_their_expected_revisions() {
    let mapped = command_to_mjolnr(&ClientCommand::Commit {
        message: "Fix the thing".to_owned(),
        expected_index_revision: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned(),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::Commit {
            ref message,
            ref expected_index_revision,
        } if message == "Fix the thing"
            && expected_index_revision == "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    ));

    let mapped = command_to_mjolnr(&ClientCommand::IntegrateChildBranch {
        name: "mjolnr/sub-1".to_owned(),
        message: "Take the child's work after review".to_owned(),
        expected_head: "deadbeef".to_owned(),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::IntegrateChildBranch { ref message, .. }
            if message == "Take the child's work after review"
    ));

    let mapped = command_to_mjolnr(&ClientCommand::IntegrateUpstream {
        message: "Take the fetched upstream".to_owned(),
        expected_head: "deadbeef".to_owned(),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::IntegrateUpstream {
            ref message,
            ref expected_head,
        } if message == "Take the fetched upstream" && expected_head == "deadbeef"
    ));
}

#[test]
fn well_formed_clone_and_rebase_commands_keep_their_explicit_values() {
    let mapped = command_to_mjolnr(&ClientCommand::CloneProject {
        source: "https://example.invalid/repo".to_owned(),
        destination: "/tmp/new-project".to_owned(),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::CloneProject { ref source, ref destination }
            if source == "https://example.invalid/repo"
                && destination == std::path::Path::new("/tmp/new-project")
    ));

    let mapped = command_to_mjolnr(&ClientCommand::Rebase {
        onto: "main".to_owned(),
        expected_head: "deadbeef".to_owned(),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::Rebase { ref onto, ref expected_head }
            if onto == "main" && expected_head == "deadbeef"
    ));
}

/// Phase E5: the bridge owns the input-shape rules for decision tickets —
/// bounded text, at least two options, parsed and de-duplicated blockers.
#[test]
fn board_commands_refuse_hostile_or_unbounded_input_at_the_bridge() {
    // A decision with fewer than two options is not a decision; an
    // unparseable or duplicated blocker would fog a ticket behind silence; a
    // resolution naming no ticket records a decision against nothing.
    let cases = vec![
        ClientCommand::OpenDecisionTicket {
            question: "   ".to_owned(),
            kind: crate::core::client::ClientDecisionTicketKind::Research,
            options: vec!["a".to_owned(), "b".to_owned()],
            blocked_by: Vec::new(),
        },
        ClientCommand::OpenDecisionTicket {
            question: "which way".to_owned(),
            kind: crate::core::client::ClientDecisionTicketKind::Task,
            options: vec!["only one".to_owned()],
            blocked_by: Vec::new(),
        },
        ClientCommand::OpenDecisionTicket {
            question: "which way".to_owned(),
            kind: crate::core::client::ClientDecisionTicketKind::Task,
            options: vec!["a".to_owned(), String::new()],
            blocked_by: Vec::new(),
        },
        ClientCommand::OpenDecisionTicket {
            question: "which way".to_owned(),
            kind: crate::core::client::ClientDecisionTicketKind::Task,
            options: vec!["a".to_owned(), "b".to_owned()],
            blocked_by: vec!["not a uuid".to_owned()],
        },
        ClientCommand::OpenDecisionTicket {
            question: "which way".to_owned(),
            kind: crate::core::client::ClientDecisionTicketKind::Task,
            options: vec!["a".to_owned(), "b".to_owned()],
            blocked_by: {
                let id = uuid::Uuid::now_v7().to_string();
                vec![id.clone(), id]
            },
        },
        ClientCommand::ResolveDecisionTicket {
            ticket: "not a uuid".to_owned(),
            chosen_option: 0,
            note: None,
        },
        ClientCommand::ResolveDecisionTicket {
            ticket: uuid::Uuid::now_v7().to_string(),
            chosen_option: 0,
            note: Some("x".repeat(crate::core::client::MAX_TICKET_NOTE_BYTES + 1)),
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "refusals carry the stable code, not prose: {command:?}"
        );
    }
}

fn valid_imported_item() -> crate::core::imported::ImportedItem {
    crate::core::imported::ImportedItem {
        id: crate::core::imported::ImportedItemId::new(),
        integration: "github".to_owned(),
        remote_id: "42".to_owned(),
        source_url: "https://example.invalid/owner/repo/issues/42".to_owned(),
        fetched_revision: "rev1".to_owned(),
        title: "an imported task".to_owned(),
        state: crate::core::imported::ImportedItemState::Open,
        blocked_by: Vec::new(),
    }
}

/// Phase E5 step 4b: the bridge owns the input-shape rules for imported items.
/// The title is a third party's text, so it is bounded and refused with control
/// characters — the same ANSI-escape path `validate_remote_text` closes for
/// issue bodies. The identifiers are refused any control character at all.
#[test]
fn imported_item_commands_refuse_hostile_or_unbounded_input_at_the_bridge() {
    let mut item = valid_imported_item();
    item.integration = "   ".to_owned();
    let mut oversized_label = valid_imported_item();
    oversized_label.integration = "x".repeat(65);
    let mut control_label = valid_imported_item();
    control_label.integration = "git\u{1b}[2Jhub".to_owned();
    let mut empty_remote = valid_imported_item();
    empty_remote.remote_id = String::new();
    let mut long_remote = valid_imported_item();
    long_remote.remote_id = "x".repeat(257);
    let mut control_remote = valid_imported_item();
    control_remote.remote_id = "42\n".to_owned();
    let mut empty_revision = valid_imported_item();
    empty_revision.fetched_revision = "  ".to_owned();
    let mut long_revision = valid_imported_item();
    long_revision.fetched_revision = "x".repeat(513);
    let mut long_url = valid_imported_item();
    long_url.source_url = format!("https://example.invalid/{}", "x".repeat(2048));
    let mut control_url = valid_imported_item();
    control_url.source_url = "https://example.invalid/\u{7}bell".to_owned();
    let mut empty_title = valid_imported_item();
    empty_title.title = "   ".to_owned();
    let mut long_title = valid_imported_item();
    long_title.title = "x".repeat(crate::integrations::MAX_REMOTE_TITLE_BYTES + 1);
    // An ANSI escape in a third party's issue title must not reach a terminal
    // client through the board surface.
    let mut escape_title = valid_imported_item();
    escape_title.title = "clear\u{1b}[2Jyour screen".to_owned();
    let mut many_blockers = valid_imported_item();
    many_blockers.blocked_by =
        vec![
            crate::core::frontier::NodeId::Imported(crate::core::imported::ImportedItemId::new());
            crate::core::client::MAX_TICKET_BLOCKERS + 1
        ];

    let cases = vec![
        ClientCommand::ImportWorkItem { item },
        ClientCommand::ImportWorkItem {
            item: oversized_label,
        },
        ClientCommand::ImportWorkItem {
            item: control_label,
        },
        ClientCommand::ImportWorkItem { item: empty_remote },
        ClientCommand::ImportWorkItem { item: long_remote },
        ClientCommand::ImportWorkItem {
            item: control_remote,
        },
        ClientCommand::ImportWorkItem {
            item: empty_revision,
        },
        ClientCommand::ImportWorkItem {
            item: long_revision,
        },
        ClientCommand::ImportWorkItem { item: long_url },
        ClientCommand::ImportWorkItem { item: control_url },
        ClientCommand::ImportWorkItem { item: empty_title },
        ClientCommand::ImportWorkItem { item: long_title },
        ClientCommand::ImportWorkItem { item: escape_title },
        ClientCommand::ImportWorkItem {
            item: many_blockers,
        },
        // A newline is legitimate prose and survives in a title...
        ClientCommand::RefreshImportedItem {
            expected_revision: String::new(),
            item: valid_imported_item(),
        },
        ClientCommand::RefreshImportedItem {
            expected_revision: "x".repeat(513),
            item: valid_imported_item(),
        },
        // ...but a revision pin is an identifier, so even a newline is refused.
        ClientCommand::RefreshImportedItem {
            expected_revision: "rev1\n".to_owned(),
            item: valid_imported_item(),
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "refusals carry the stable code, not prose: {command:?}"
        );
    }

    // A newline in a title is legitimate prose — refusing it would make the
    // guard useless in practice, which is how guards get removed.
    let mut newline_title = valid_imported_item();
    newline_title.title = "line one\nline two".to_owned();
    let mapped = command_to_mjolnr(&ClientCommand::ImportWorkItem {
        item: newline_title,
    })
    .expect("a newline in third-party prose survives validation")
    .expect("a command");
    assert!(matches!(mapped, MjolnrCommand::ImportWorkItem { .. }));
}

#[test]
fn well_formed_board_commands_map_through_their_kinds_and_references() {
    let blocker = uuid::Uuid::now_v7();
    let mapped = command_to_mjolnr(&ClientCommand::OpenDecisionTicket {
        question: "Which format for the knowledge bundle?".to_owned(),
        kind: crate::core::client::ClientDecisionTicketKind::Grilling,
        options: vec!["okf".to_owned(), "adr set".to_owned()],
        blocked_by: vec![blocker.to_string()],
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::OpenDecisionTicket {
            ref kind,
            ref options,
            ref blocked_by,
            ..
        } if *kind == crate::core::board::DecisionTicketKind::Grilling
            && options.len() == 2
            && blocked_by == &[crate::core::board::DecisionTicketId::from_uuid(blocker)]
    ));

    let ticket = uuid::Uuid::now_v7();
    let mapped = command_to_mjolnr(&ClientCommand::ResolveDecisionTicket {
        ticket: ticket.to_string(),
        chosen_option: 1,
        note: Some("the queue is out of scope".to_owned()),
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::ResolveDecisionTicket {
            ticket: mapped_ticket,
            chosen_option: 1,
            ref note,
        } if mapped_ticket == crate::core::board::DecisionTicketId::from_uuid(ticket)
            && note.as_deref() == Some("the queue is out of scope")
    ));
}

/// The guard that replaced the shared `todo!()` panic for D5: a repository
/// command reaching a runtime with no open project is answered with a typed
/// refusal, so the client learns why nothing happened.
#[tokio::test]
async fn repository_commands_without_an_open_project_are_refused_not_dropped() {
    let runtime = Arc::new(TestRuntime::new());
    runtime.refuse_dispatch_with(MjolnrError::workspace_refused(
        ReasonCode::WorkspaceCapabilityUnavailable,
        "No project is open, so there is no repository to act on",
    ));
    let bridge = ClientBridge::start(runtime.clone());

    let error = bridge
        .dispatch(ClientCommand::StagePaths {
            paths: vec!["a.rs".to_owned()],
        })
        .await
        .expect_err("a repository command must be refused, not dropped");
    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceCapabilityUnavailable),
        "the typed code must round-trip to the client: {error}"
    );
}

/// Remote text is the one input on the wire that a third party authored, so the
/// bridge bounds it and strips what could reformat a terminal — before it
/// reaches the durable record (Phase D6).
#[test]
fn integration_commands_bound_and_sanitize_externally_supplied_text() {
    let cases = vec![
        ClientCommand::FetchTask {
            source: String::new(),
            task_id: "1".to_owned(),
        },
        // An integration id selects an account; it is an identifier, not prose.
        ClientCommand::FetchTask {
            source: "GitHub".to_owned(),
            task_id: "1".to_owned(),
        },
        ClientCommand::FetchTask {
            source: "git hub".to_owned(),
            task_id: "1".to_owned(),
        },
        ClientCommand::FetchTask {
            source: "github".to_owned(),
            task_id: String::new(),
        },
        ClientCommand::FetchTask {
            source: "github".to_owned(),
            task_id: "1\u{1b}[2J".to_owned(),
        },
        ClientCommand::SubmitChange {
            source: "github".to_owned(),
            request: crate::core::client::ClientRemoteChangeRequest {
                remote_id: "1".to_owned(),
                expected_revision: "rev1".to_owned(),
                // An ANSI escape in a PR title, arriving from a third party's
                // issue, must not reach a terminal client.
                title: "clear\u{1b}[2Jyour screen".to_owned(),
                body: "body".to_owned(),
                head_commit: "abc123".to_owned(),
                head_branch: "feature/parser".to_owned(),
                base_branch: "main".to_owned(),
            },
        },
        ClientCommand::SubmitChange {
            source: "github".to_owned(),
            request: crate::core::client::ClientRemoteChangeRequest {
                remote_id: "1".to_owned(),
                expected_revision: "rev1".to_owned(),
                title: "t".to_owned(),
                body: "x".repeat(crate::integrations::MAX_REMOTE_BODY_BYTES + 1),
                head_commit: "abc123".to_owned(),
                head_branch: "feature/parser".to_owned(),
                base_branch: "main".to_owned(),
            },
        },
    ];
    for command in cases {
        let error = command_to_mjolnr(&command).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "refusals carry the stable code, not prose: {command:?}"
        );
    }
}

/// §E5 contract (a) on the act path, at the bridge: the pin is required and is
/// an identifier, not prose. A `submitChange` that omits it never reaches the
/// runtime, so there is no path where the staleness check is skipped because a
/// client did not supply the field.
#[test]
fn a_change_without_a_usable_revision_pin_is_refused_at_the_bridge() {
    let with_pin = |pin: &str| ClientCommand::SubmitChange {
        source: "github".to_owned(),
        request: crate::core::client::ClientRemoteChangeRequest {
            remote_id: "42".to_owned(),
            expected_revision: pin.to_owned(),
            title: "Fix the parser".to_owned(),
            body: "body".to_owned(),
            head_commit: "abc123".to_owned(),
            head_branch: "feature/parser".to_owned(),
            base_branch: "main".to_owned(),
        },
    };
    for pin in ["", "   ", "rev\u{1b}[2J1", "rev\n1", &"r".repeat(513)] {
        let error = command_to_mjolnr(&with_pin(pin)).expect_err("must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::SchemaInvalid),
            "a revision pin is an identifier and is required: {pin:?}"
        );
    }
    assert!(
        command_to_mjolnr(&with_pin("rev1")).is_ok(),
        "an ordinary pin must survive validation"
    );
}

/// The pin is not dropped between the wire type and the command the runtime
/// sees. Without this, the bridge could validate a field it then discarded and
/// every downstream test would still pass.
#[test]
fn the_revision_pin_survives_the_mapping_into_the_runtime_command() {
    let mapped = command_to_mjolnr(&ClientCommand::SubmitChange {
        source: "github".to_owned(),
        request: crate::core::client::ClientRemoteChangeRequest {
            remote_id: "42".to_owned(),
            expected_revision: "rev7".to_owned(),
            title: "Fix the parser".to_owned(),
            body: "body".to_owned(),
            head_commit: "abc123".to_owned(),
            head_branch: "feature/parser".to_owned(),
            base_branch: "main".to_owned(),
        },
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::SubmitChange {
            ref expected_revision,
            ..
        } if expected_revision == "rev7"
    ));
}

#[test]
fn a_newline_in_a_remote_body_is_legitimate_and_survives_validation() {
    // An issue body has paragraphs. Refusing '\n' would make the guard useless
    // in practice, which is how guards get removed.
    let mapped = command_to_mjolnr(&ClientCommand::SubmitChange {
        source: "github".to_owned(),
        request: crate::core::client::ClientRemoteChangeRequest {
            remote_id: "42".to_owned(),
            expected_revision: "rev1".to_owned(),
            title: "Fix the parser".to_owned(),
            body: "First paragraph.\n\nSecond paragraph.\n\t- indented item".to_owned(),
            head_commit: "abc123".to_owned(),
            head_branch: "feature/parser".to_owned(),
            base_branch: "main".to_owned(),
        },
    })
    .expect("valid")
    .expect("a command");
    assert!(matches!(
        mapped,
        MjolnrCommand::SubmitChange { ref body, .. } if body.contains("Second paragraph")
    ));
}

/// The D6 guard: the contract reaches the runtime and is answered with a typed
/// refusal. The failure this replaces was worse than a panic — the variants fell
/// through the bridge's catch-all to `Ok(None)`, so the client saw a re-fetched
/// snapshot and no error at all.
#[tokio::test]
async fn integration_commands_are_refused_typed_rather_than_silently_dropped() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(runtime.clone());

    for command in [
        ClientCommand::FetchTask {
            source: "github".to_owned(),
            task_id: "123".to_owned(),
        },
        ClientCommand::SubmitChange {
            source: "linear".to_owned(),
            request: crate::core::client::ClientRemoteChangeRequest {
                remote_id: "SIM-1".to_owned(),
                expected_revision: "rev1".to_owned(),
                title: "t".to_owned(),
                body: "b".to_owned(),
                head_commit: "abc123".to_owned(),
                head_branch: "feature/parser".to_owned(),
                base_branch: "main".to_owned(),
            },
        },
    ] {
        runtime.refuse_dispatch_with(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "no integration performs network requests yet; no credential was read",
        ));
        let error = bridge
            .dispatch(command.clone())
            .await
            .expect_err("an integration command must be refused, not dropped");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::WorkspaceCapabilityUnavailable),
            "the typed code must round-trip to the client: {error}"
        );
    }

    // And the commands genuinely reached the runtime rather than being mapped
    // to `None` by the bridge's catch-all.
    let dispatched = runtime.commands();
    assert!(
        dispatched
            .iter()
            .any(|command| matches!(command, MjolnrCommand::FetchTask { .. })),
        "fetchTask never reached the runtime: {dispatched:?}"
    );
    assert!(
        dispatched
            .iter()
            .any(|command| matches!(command, MjolnrCommand::SubmitChange { .. })),
        "submitChange never reached the runtime: {dispatched:?}"
    );
}

#[test]
fn the_event_vocabulary_is_a_deliberate_subset() {
    let session = SessionId::new();
    let run = RunId::new();

    let kept = MjolnrEvent::RunFinished {
        session,
        run,
        reason: FinishReason::Cancelled,
    };
    assert!(matches!(
        event_to_client(&kept),
        Some(ClientEvent::RunFinished {
            reason: ClientFinishReason::Cancelled,
            ..
        })
    ));

    let saved = MjolnrEvent::FileSaved {
        session,
        path: "src/main.rs".to_owned(),
        observed_digest: "a".repeat(64),
        new_digest: "b".repeat(64),
        size_bytes: 13,
    };
    assert!(matches!(
        event_to_client(&saved),
        Some(ClientEvent::FileSaved {
            path,
            size_bytes: 13,
            ..
        }) if path == "src/main.rs"
    ));
}

// -----------------------------------------------------------------------------
// Async Integration Tests (Requirement #6)
// -----------------------------------------------------------------------------

#[tokio::test]
async fn async_integration_1_initial_snapshot_delivery() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(runtime);
    let mut rx = bridge.take_updates().expect("updates channel");

    let first = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout")
        .expect("update");

    match first {
        ClientUpdate::Snapshot { snapshot } => {
            assert_eq!(snapshot.revision, 0);
            assert_eq!(snapshot.session, None);
        }
        other => panic!("expected initial Snapshot update, got {other:?}"),
    }
}

fn parse_timestamp(raw: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .expect("valid timestamp")
}

#[tokio::test]
async fn async_integration_2_ordered_updates() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);
    let mut rx = bridge.take_updates().expect("updates channel");

    // Consume initial snapshot
    let _ = rx.recv().await;

    let run_id = RunId::new();
    runtime
        .events_tx
        .send(MjolnrEvent::RunStarted {
            session: SessionId::new(),
            run: run_id,
        })
        .unwrap();

    runtime
        .events_tx
        .send(MjolnrEvent::RunFinished {
            session: SessionId::new(),
            run: run_id,
            reason: FinishReason::Stop,
        })
        .unwrap();

    let update1 = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout")
        .expect("update");
    let update2 = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout")
        .expect("update");

    match (update1, update2) {
        (
            ClientUpdate::Event {
                sequence: seq1,
                event: ClientEvent::RunStarted { run: r1 },
            },
            ClientUpdate::Event {
                sequence: seq2,
                event: ClientEvent::RunFinished { run: r2, reason },
            },
        ) => {
            assert_eq!(seq1 + 1, seq2);
            assert_eq!(r1, run_id.to_string());
            assert_eq!(r2, run_id.to_string());
            assert_eq!(reason, ClientFinishReason::Stop);
        }
        (u1, u2) => panic!("expected ordered RunStarted then RunFinished, got {u1:?}, {u2:?}"),
    }
}

#[tokio::test]
async fn async_integration_3_lag_causes_explicit_resync() {
    let runtime = Arc::new(TestRuntime::new());
    // Start with capacity 1 to force lag quickly
    let bridge =
        ClientBridge::start_with_capacity(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>, 1);
    let mut rx = bridge.take_updates().expect("updates channel");

    // Drain initial snapshot
    let _ = rx.recv().await;

    // Send multiple events without reading from rx to fill the broadcast channel
    let run_id = RunId::new();
    for _ in 0..300 {
        let _ = runtime.events_tx.send(MjolnrEvent::RunStarted {
            session: SessionId::new(),
            run: run_id,
        });
    }

    // Now read from rx until we hit Resync or Closed
    let mut hit_resync = false;
    for _ in 0..50 {
        if let Ok(Some(ClientUpdate::Resync { missed, snapshot })) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            assert!(missed > 0, "must report missed events count > 0");
            assert_eq!(snapshot.revision, snapshot.revision);
            hit_resync = true;
            break;
        }
    }
    assert!(
        hit_resync,
        "must receive explicit ClientUpdate::Resync when lagged"
    );
}

#[tokio::test]
async fn async_integration_4_explicit_request_snapshot() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);
    let mut rx = bridge.take_updates().expect("updates channel");

    // Drain initial snapshot
    let _ = rx.recv().await;

    bridge
        .dispatch(ClientCommand::RequestSnapshot)
        .await
        .expect("dispatch RequestSnapshot");

    let response = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout")
        .expect("update");

    match response {
        ClientUpdate::Snapshot { snapshot } => {
            assert_eq!(snapshot.revision, 1);
        }
        other => panic!("expected Snapshot in response to RequestSnapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn async_integration_5_detached_closed_clients() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);
    let mut rx = bridge.take_updates().expect("updates channel");

    bridge.close().await.expect("close runtime bridge");

    // After runtime/bridge closes, rx must receive Closed
    drop(runtime);

    let mut received_closed = false;
    while let Ok(Some(update)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
        if matches!(update, ClientUpdate::Closed) {
            received_closed = true;
            break;
        }
    }
    assert!(
        received_closed,
        "closing runtime bridge must emit ClientUpdate::Closed"
    );
}

#[tokio::test]
async fn async_integration_6_cancellation_reaches_runtime() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);

    bridge
        .dispatch(ClientCommand::CancelRun)
        .await
        .expect("dispatch CancelRun");

    let cmds = runtime.commands();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0], MjolnrCommand::CancelRun);
}

#[tokio::test]
async fn async_integration_7_exactly_one_terminal_cancellation_outcome() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn MjolnrRuntime>);
    let mut rx = bridge.take_updates().expect("updates channel");

    // Consume initial snapshot
    let _ = rx.recv().await;

    let run_id = RunId::new();
    runtime
        .events_tx
        .send(MjolnrEvent::RunFinished {
            session: SessionId::new(),
            run: run_id,
            reason: FinishReason::Cancelled,
        })
        .unwrap();

    let update = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout")
        .expect("update");

    let ClientUpdate::Event {
        event: ClientEvent::RunFinished { run, reason },
        ..
    } = update
    else {
        panic!("expected RunFinished event, got {update:?}");
    };

    assert_eq!(run, run_id.to_string());
    assert_eq!(reason, ClientFinishReason::Cancelled);

    // Verify no secondary terminal event is sent
    let next = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
    assert!(
        next.is_err(),
        "exactly one terminal cancellation outcome must be delivered"
    );
}

/// Drain the bridge update receiver until a `Snapshot` matching `predicate`
/// arrives, or fail after `timeout`. Never sleeps — awaits the channel with a
/// bounded timeout (AGENTS.md §7).
async fn drain_until_snapshot(
    rx: &mut tokio::sync::mpsc::Receiver<ClientUpdate>,
    predicate: impl Fn(&crate::core::client::ClientSnapshot) -> bool,
    timeout: Duration,
) -> crate::core::client::ClientSnapshot {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let update = tokio::time::timeout(remaining, rx.recv())
            .await
            .expect("timed out waiting for matching snapshot update")
            .expect("bridge update channel closed unexpectedly");
        if let ClientUpdate::Snapshot { snapshot } = update
            && predicate(&snapshot)
        {
            return snapshot;
        }
    }
}

#[tokio::test]
async fn async_integration_8_store_backed_session_list_and_resume_over_bridge() {
    use crate::core::store::EventStore;
    use crate::runtime::Runtime;
    use crate::store::sqlite::SqliteEventStore;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let workspace = dir.path().canonicalize().expect("canonicalize");

    let created_id = {
        let store_sqlite = SqliteEventStore::open(&db_path).await.expect("open sqlite");
        let store: Arc<dyn EventStore> = Arc::new(store_sqlite);
        let runtime = Runtime::spawn(Vec::new(), store);
        let bridge = ClientBridge::start(Arc::new(runtime));
        let mut rx = bridge
            .take_updates()
            .expect("update receiver must be available");

        bridge
            .dispatch(ClientCommand::OpenProject {
                root: workspace.to_string_lossy().into_owned(),
            })
            .await
            .expect("open project");

        bridge
            .dispatch(ClientCommand::CreateSession {
                provider: "anthropic".to_owned(),
                model: "claude-3-5-sonnet".to_owned(),
            })
            .await
            .expect("create session");

        let snap = drain_until_snapshot(
            &mut rx,
            |s| s.session.is_some() && !s.sessions.is_empty(),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            snap.session.is_some(),
            "created session must be active in snapshot"
        );
        assert_eq!(
            snap.sessions.len(),
            1,
            "store-backed session list must populate ClientSnapshot.sessions"
        );
        assert_eq!(snap.sessions[0].id, snap.session.clone().unwrap());

        let session_id = snap.session.clone().unwrap();
        bridge.close().await.expect("clean shutdown");
        session_id
    };

    // Re-open runtime against the same SQLite database
    let store_sqlite = SqliteEventStore::open(&db_path)
        .await
        .expect("reopen sqlite");
    let store: Arc<dyn EventStore> = Arc::new(store_sqlite);
    let runtime = Runtime::spawn(Vec::new(), store);
    let bridge = ClientBridge::start(Arc::new(runtime));
    let mut rx = bridge
        .take_updates()
        .expect("update receiver must be available on re-opened bridge");

    let snap =
        drain_until_snapshot(&mut rx, |s| !s.sessions.is_empty(), Duration::from_secs(5)).await;

    assert_eq!(
        snap.sessions.len(),
        1,
        "re-opened runtime discovers SQLite session list"
    );
    assert_eq!(snap.sessions[0].id, created_id);

    bridge
        .dispatch(ClientCommand::ResumeSession {
            session: created_id.clone(),
        })
        .await
        .expect("resume session");

    let snap = drain_until_snapshot(
        &mut rx,
        |s| s.session == Some(created_id.clone()),
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(snap.session, Some(created_id));
    bridge.close().await.expect("clean shutdown");
}

/// A minimal `EventStore` mock whose `sessions()` call fails, used to
/// verify that store failures propagate through the bridge to
/// `ClientSnapshot.store_failure` (AGENTS.md §3 — never lie about
/// state).
#[derive(Debug)]
struct FailingEventStore;

#[async_trait]
impl crate::core::store::EventStore for FailingEventStore {
    async fn find_session_by_dir(
        &self,
        _project_root: std::path::PathBuf,
    ) -> Result<Option<crate::core::event::SessionId>, crate::core::store::StoreError> {
        Err(crate::core::store::StoreError::Unavailable {
            detail: "FailingEventStore refuses everything by design".to_owned(),
        })
    }

    async fn search_workspace(
        &self,
        _filter: crate::core::store::WorkspaceSearchFilter,
    ) -> Result<crate::core::store::WorkspaceSearchPage, crate::core::store::StoreError> {
        Err(crate::core::store::StoreError::Unavailable {
            detail: "FailingEventStore refuses everything by design".to_owned(),
        })
    }

    async fn open_project(
        &self,
        _root: std::path::PathBuf,
    ) -> Result<crate::core::store::ProjectId, crate::core::store::StoreError> {
        Ok(crate::core::store::ProjectId::new())
    }

    async fn create_session(
        &self,
        _session: crate::core::event::SessionId,
        _project: crate::core::store::ProjectId,
        _title: String,
        _parent: Option<crate::core::event::SessionId>,
    ) -> Result<(), crate::core::store::StoreError> {
        Ok(())
    }

    async fn end_session(
        &self,
        _session: crate::core::event::SessionId,
    ) -> Result<(), crate::core::store::StoreError> {
        Ok(())
    }

    async fn sessions(
        &self,
    ) -> Result<Vec<crate::core::store::SessionSummary>, crate::core::store::StoreError> {
        Err(crate::core::store::StoreError::Unavailable {
            detail: "SQLite disk I/O error [MJOLNR-ERR-STORE-001]".to_owned(),
        })
    }

    async fn append(
        &self,
        event: crate::core::event::MjolnrEvent,
    ) -> Result<crate::core::event::StoredEvent, crate::core::store::StoreError> {
        Ok(crate::core::event::StoredEvent {
            id: crate::core::event::EventId::new(),
            sequence: 1,
            occurred_at: time::OffsetDateTime::now_utc(),
            event,
        })
    }

    async fn events(
        &self,
        _session: crate::core::event::SessionId,
    ) -> Result<Vec<crate::core::event::StoredEvent>, crate::core::store::StoreError> {
        Ok(Vec::new())
    }

    async fn events_from(
        &self,
        _session: crate::core::event::SessionId,
        _from: u64,
    ) -> Result<Vec<crate::core::event::StoredEvent>, crate::core::store::StoreError> {
        Ok(Vec::new())
    }

    async fn session_tree(
        &self,
        _session: crate::core::event::SessionId,
    ) -> Result<Vec<crate::core::store::SessionTreeNode>, crate::core::store::StoreError> {
        Ok(Vec::new())
    }

    async fn branch_summary(
        &self,
        _session: crate::core::event::SessionId,
        _leaf: u64,
    ) -> Result<crate::core::store::BranchSummary, crate::core::store::StoreError> {
        Ok(crate::core::store::BranchSummary {
            origin: None,
            turns: 0,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands: Vec::new(),
            tool_failures: 0,
        })
    }

    async fn write_checkpoint(
        &self,
        checkpoint: crate::core::checkpoint::SessionCheckpoint,
    ) -> Result<u64, crate::core::store::StoreError> {
        Ok(checkpoint.messages.len() as u64)
    }

    async fn latest_checkpoint(
        &self,
        _session: crate::core::event::SessionId,
    ) -> Result<Option<crate::core::store::StoredCheckpoint>, crate::core::store::StoreError> {
        Ok(None)
    }

    async fn acquire_session(
        &self,
        session: crate::core::event::SessionId,
    ) -> Result<crate::core::store::SessionLease, crate::core::store::StoreError> {
        Ok(crate::core::store::SessionLease {
            session,
            token: uuid::Uuid::now_v7(),
        })
    }

    async fn release_session(
        &self,
        _lease: &crate::core::store::SessionLease,
    ) -> Result<(), crate::core::store::StoreError> {
        Ok(())
    }

    async fn break_lease(
        &self,
        _session: crate::core::event::SessionId,
    ) -> Result<(), crate::core::store::StoreError> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), crate::core::store::StoreError> {
        Ok(())
    }
}

/// Negative test: a store whose `sessions()` operation fails must
/// report the failure through `RuntimeSnapshot.store_failure`, which
/// the bridge propagates to `ClientSnapshot.store_failure`, so the
/// frontend can render a store-error notice truthfully (AGENTS.md
/// §3 — never lie about state).
#[tokio::test]
async fn async_integration_9_store_failure_reaches_client_snapshot() {
    use crate::runtime::Runtime;

    let store = Arc::new(FailingEventStore);
    let runtime = Runtime::spawn(Vec::new(), store);
    let bridge = ClientBridge::start(Arc::new(runtime));
    let mut rx = bridge.take_updates().expect("updates channel");

    let snap = drain_until_snapshot(
        &mut rx,
        |s| s.store_failure.is_some(),
        Duration::from_secs(5),
    )
    .await;

    assert!(
        snap.store_failure.is_some(),
        "store_failure must propagate from RuntimeSnapshot through the bridge \
         (got {:?})",
        snap.store_failure
    );

    // Cleanup: the Actor might reject close when store_failure is set;
    // that is expected and not a test failure.
    let _ = bridge.close().await;
}

/// The board query (Phase E5, step 3) maps through the bridge with kinds,
/// provenance, labels, and the "why is this fogged" blockers intact.
#[tokio::test]
async fn the_board_query_reaches_the_client_as_wire_shape() {
    use crate::core::frontier::{
        BoardNodeView, BoardOverview, FoggedNodeView, NodeId, NodeKind, Provenance,
    };

    let blocker = BoardNodeView {
        id: NodeId::Decision(crate::core::board::DecisionTicketId::new()),
        kind: NodeKind::Decision,
        provenance: Provenance::MjolnrGoverned,
        label: "the blocker question".to_owned(),
    };
    let fogged = BoardNodeView {
        id: NodeId::Plan(PlanId::new()),
        kind: NodeKind::Implementation,
        provenance: Provenance::MjolnrGoverned,
        label: "the plan title".to_owned(),
    };
    let settled = BoardNodeView {
        id: NodeId::Decision(crate::core::board::DecisionTicketId::new()),
        kind: NodeKind::Decision,
        provenance: Provenance::ExternalUnverified,
        label: "an imported ticket".to_owned(),
    };

    let runtime = Arc::new(TestRuntime::new());
    runtime.answer_board_with(Ok(BoardOverview {
        imported_tasks: std::collections::BTreeMap::new(),
        imported_acts: std::collections::BTreeMap::new(),
        frontier: vec![blocker.clone()],
        fog: vec![FoggedNodeView {
            node: fogged.clone(),
            waits_on: vec![blocker],
        }],
        settled: vec![settled],
        cycles: Vec::new(),
    }));
    let bridge = ClientBridge::start(runtime);

    let board = bridge
        .query_board()
        .await
        .expect("a well-formed board maps through the bridge");

    assert_eq!(board.frontier.len(), 1);
    assert_eq!(board.frontier[0].label, "the blocker question");
    assert_eq!(board.frontier[0].kind, "decision");
    assert_eq!(
        board.frontier[0].provenance,
        crate::core::client::workspace::TrustClass::MjolnrGoverned
    );

    assert_eq!(board.fog.len(), 1);
    let fogged_client = &board.fog[0];
    assert_eq!(fogged_client.node.kind, "implementation");
    assert_eq!(fogged_client.node.label, "the plan title");
    assert_eq!(fogged_client.waits_on.len(), 1);
    assert_eq!(
        fogged_client.waits_on[0].label, "the blocker question",
        "the fogged node carries the answer to 'why is this not decidable'"
    );

    assert_eq!(board.settled.len(), 1);
    assert_eq!(
        board.settled[0].provenance,
        crate::core::client::workspace::TrustClass::ExternalUnverified,
        "provenance crosses the boundary exactly, never elided"
    );
}

/// Without an open workspace the runtime refuses the board query with a
/// typed code that must reach the client unchanged — an absent board is a
/// refusal, never an empty-board lie (AGENTS.md §3).
#[tokio::test]
async fn the_board_query_refuses_without_an_open_workspace() {
    let runtime = Arc::new(TestRuntime::new());
    let bridge = ClientBridge::start(runtime);

    let error = bridge
        .query_board()
        .await
        .expect_err("no workspace open, so no board exists to answer from");

    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceCapabilityUnavailable),
        "the typed refusal code must round-trip to the client: {error}"
    );
}

/// Wire bounds are re-applied at the last transformation: a board over
/// `MAX_BOARD_NODES` is refused loudly, not truncated silently, even when
/// the runtime produced it.
#[test]
fn the_board_overview_over_the_wire_limit_is_refused() {
    use crate::core::client::board::MAX_BOARD_NODES;
    use crate::core::frontier::{BoardNodeView, BoardOverview, NodeId, NodeKind, Provenance};

    let mut frontier = Vec::new();
    for _ in 0..=MAX_BOARD_NODES {
        frontier.push(BoardNodeView {
            id: NodeId::Decision(crate::core::board::DecisionTicketId::new()),
            kind: NodeKind::Decision,
            provenance: Provenance::MjolnrGoverned,
            label: "overflow ticket".to_owned(),
        });
    }

    let error = super::board::board_overview_to_client(&BoardOverview {
        imported_tasks: std::collections::BTreeMap::new(),
        imported_acts: std::collections::BTreeMap::new(),
        frontier,
        fog: Vec::new(),
        settled: Vec::new(),
        cycles: Vec::new(),
    })
    .expect_err("an over-limit board must be refused, never truncated");

    assert_eq!(
        error.reason_code(),
        Some(ReasonCode::WorkspaceSearchRefused),
        "the bounds refusal carries the stable code: {error}"
    );
}

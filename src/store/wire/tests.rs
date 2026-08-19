use super::*;
use crate::core::command::ApprovalDecision;
use crate::core::continuation::{
    HandoffCheckpoint, HandoffId, QuotaReserveBasis, QuotaReservePhase, QuotaReserveStatus,
};
use crate::core::error::ReasonCode;
use crate::core::event::FinishReason;
use crate::core::message::{CanonicalMessage, ToolCall, ToolResult};
use crate::core::model::{QuotaSnapshot, Usage};
use crate::core::policy::PolicyMode;
use crate::core::recovery::{Authority, InterruptedKind, InterruptedWork, RecoveryDecision};
use crate::core::tool::ToolTier;

fn call() -> ToolCall {
    ToolCall {
        id: "call_1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({ "path": "a.rs" }),
        provider_signature: None,
    }
}

/// Every durable variant, so the round-trip test cannot silently miss one.
#[allow(
    clippy::too_many_lines,
    reason = "the exhaustive fixture deliberately lists every durable event variant"
)]
fn every_durable_event(session: SessionId, run: RunId) -> Vec<SmedEvent> {
    let review_id = crate::core::council::CouncilReviewId::new();
    let finding_id = crate::core::council::CouncilFindingId::new();
    let ticket_id = crate::core::board::DecisionTicketId::new();
    let blocker_id = crate::core::board::DecisionTicketId::new();
    let mut events = vec![
        SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new("fake"),
            model: ModelId::new("fake-1"),
        },
        SmedEvent::MessageAppended {
            session,
            message: Box::new(CanonicalMessage::user("hello")),
        },
        SmedEvent::RunStarted { session, run },
        SmedEvent::UsageReported {
            session,
            run,
            usage: Usage {
                input_tokens: 4,
                output_tokens: 9,
            },
        },
        SmedEvent::PolicyChanged {
            session,
            mode: PolicyMode::WorkspaceWrite,
        },
        SmedEvent::ExtensionLoaded {
            session,
            name: "count-lines".to_owned(),
            program: "wc".to_owned(),
            by: crate::core::event::ExtensionLoadAuthority::Command,
        },
        SmedEvent::ExtensionLoaded {
            session,
            name: "approved-tool".to_owned(),
            program: "approved".to_owned(),
            by: crate::core::event::ExtensionLoadAuthority::Approved,
        },
        SmedEvent::ToolProposed {
            session,
            run,
            approval: Some(ApprovalId::new()),
            call: call(),
            tier: ToolTier::Execute,
            preview: "git diff".to_owned(),
        },
        SmedEvent::ToolProposed {
            session,
            run,
            approval: None,
            call: call(),
            tier: ToolTier::Read,
            preview: String::new(),
        },
        SmedEvent::ApprovalResolved {
            session,
            run,
            approval: ApprovalId::new(),
            decision: ApprovalDecision::ApproveExactForSession,
        },
        SmedEvent::ToolCompleted {
            session,
            run,
            call_id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            result: ToolResult::ok("contents"),
        },
        SmedEvent::ToolFailed {
            session,
            run,
            call_id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            code: ReasonCode::ToolExecution,
            detail: "boom".to_owned(),
        },
        SmedEvent::BudgetExhausted { session, run },
        SmedEvent::RunFinished {
            session,
            run,
            reason: FinishReason::Stop,
        },
        SmedEvent::RunFailed {
            session,
            run,
            code: ReasonCode::ProviderAuth,
            detail: "bad key".to_owned(),
        },
        SmedEvent::ModelChanged {
            session,
            provider: ProviderId::new("openai"),
            model: ModelId::new("gpt-4o-mini"),
        },
        model_refusal(session),
        SmedEvent::FileSaved {
            session,
            path: "src/main.rs".to_owned(),
            observed_digest: "a".repeat(64),
            new_digest: "b".repeat(64),
            size_bytes: 13,
        },
        SmedEvent::SubagentSpawned {
            session,
            run,
            child: SessionId::new(),
            directive: "worker:alpha.txt".to_owned(),
            policy: PolicyMode::WorkspaceWrite,
            branch: "smed/sub-abc12345".to_owned(),
            worktree: "/tmp/smed-worktrees/abc".to_owned(),
        },
        SmedEvent::SubagentResultLate {
            session,
            child: SessionId::new(),
            detail: "{\"outcome\":\"completed\"}".to_owned(),
        },
        SmedEvent::ReadSetCollision {
            session,
            reader: SessionId::new(),
            writer: SessionId::new(),
            path: "shared.txt".to_owned(),
        },
        SmedEvent::RecoveryRequired {
            session,
            work: Box::new(InterruptedWork {
                run,
                kind: InterruptedKind::EffectUncertain {
                    authority: Authority::Approval(ApprovalId::new()),
                    call: call(),
                    tier: ToolTier::Write,
                    preview: "diff".to_owned(),
                },
            }),
        },
        SmedEvent::RecoveryResolved {
            session,
            decision: RecoveryDecision::AbandonAndContinue,
        },
        SmedEvent::SessionEnded { session },
        SmedEvent::PlanInterviewStarted {
            session,
            plan_id: crate::core::plan::PlanId::new(),
            goal: "Turn an idea into a reviewed plan".to_owned(),
        },
        SmedEvent::PlanQuestionAsked {
            session,
            plan_id: crate::core::plan::PlanId::new(),
            question: crate::core::plan::Question {
                id: crate::core::plan::QuestionId::new(),
                prompt: "Which architecture?".to_string(),
                options: vec!["Option A".to_string(), "Option B".to_string()],
                is_multi_select: false,
                created_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanQuestionAnswered {
            session,
            plan_id: crate::core::plan::PlanId::new(),
            answer: crate::core::plan::QuestionAnswer {
                question_id: crate::core::plan::QuestionId::new(),
                selected_options: vec!["Option A".to_string()],
                freeform_text: None,
                answered_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanPrdProposed {
            session,
            prd: crate::core::plan::ProductRequirementsDocument {
                id: crate::core::plan::PrdId::new(),
                plan_id: crate::core::plan::PlanId::new(),
                title: "Governed planning".to_owned(),
                problem: "The path from idea to plan is not durable".to_owned(),
                users: vec!["owner".to_owned()],
                requirements: vec![crate::core::plan::PrdRequirement {
                    id: "REQ-1".to_owned(),
                    title: "Persist the PRD".to_owned(),
                    description: "Record it before review".to_owned(),
                }],
                acceptance_criteria: vec!["Restart preserves it".to_owned()],
                non_goals: vec!["Automatic execution".to_owned()],
                constraints: vec!["Local-first".to_owned()],
                created_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanProposed {
            session,
            proposal: crate::core::plan::PlanProposal {
                plan_id: crate::core::plan::PlanId::new(),
                revision_id: crate::core::plan::RevisionId::initial(),
                title: "Build Feature".to_string(),
                summary: "Summary of feature".to_string(),
                steps: vec![crate::core::plan::PlanStep {
                    index: 1,
                    title: "Step 1".to_string(),
                    description: "First step".to_string(),
                }],
                proposed_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanReviewed {
            session,
            review: crate::core::plan::PlanReview {
                plan_id: crate::core::plan::PlanId::new(),
                revision_id: crate::core::plan::RevisionId::initial(),
                reviewer: "Council".to_string(),
                verdict: crate::core::plan::ReviewVerdict::Approve,
                feedback: "Looks good".to_string(),
                reviewed_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanApproved {
            session,
            approval: crate::core::plan::PlanApproval {
                plan_id: crate::core::plan::PlanId::new(),
                revision_id: crate::core::plan::RevisionId::initial(),
                approver: "Jerrik".to_string(),
                decision: crate::core::plan::ReviewVerdict::Approve,
                note: Some("Approved".to_string()),
                approved_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::PlanHandoffCreated {
            session,
            handoff: crate::core::plan::PlanHandoff {
                plan_id: crate::core::plan::PlanId::new(),
                revision_id: crate::core::plan::RevisionId::initial(),
                handoff_note: "Ready for execution".to_string(),
                created_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::CouncilReviewed {
            session,
            review: Box::new(crate::core::council::CouncilReview {
                review_id,
                question: "Which boundary should land first?".to_owned(),
                plan_id: None,
                prd_id: None,
                contributions: Vec::new(),
                rounds_conducted: 1,
                artifact: None,
                findings: vec![crate::core::council::CouncilFinding {
                    id: finding_id,
                    section: "Question".to_owned(),
                    title: "Keep the gate in Rust".to_owned(),
                    positions: Vec::new(),
                    disposition: None,
                }],
            }),
        },
        SmedEvent::CouncilFindingDispositionRecorded {
            session,
            disposition: crate::core::council::CouncilFindingDisposition {
                review_id,
                finding_id,
                disposition: crate::core::council::CouncilDisposition::Defer,
                note: Some("needs human comparison".to_owned()),
                decided_at: time::OffsetDateTime::now_utc(),
            },
        },
        SmedEvent::CouncilAmendmentProposed {
            session,
            amendment: Box::new(crate::core::council::CouncilAmendment {
                review_id,
                path: "docs/plan.md".to_owned(),
                source_digest: "0".repeat(64),
                accepted_findings: 1,
                text: "# Goal\nship it\n".to_owned(),
            }),
        },
        SmedEvent::DecisionTicketOpened {
            session,
            ticket: crate::core::board::DecisionTicket {
                id: ticket_id,
                question: "Ship with the queue or without it?".to_owned(),
                kind: crate::core::board::DecisionTicketKind::Prototype,
                options: vec!["with the queue".to_owned(), "without it".to_owned()],
                blocked_by: vec![blocker_id],
            },
        },
        SmedEvent::DecisionTicketResolved {
            session,
            resolution: crate::core::board::DecisionResolution {
                id: crate::core::board::DecisionResolutionId::new(),
                ticket: ticket_id,
                question: "Ship with the queue or without it?".to_owned(),
                options: vec!["with the queue".to_owned(), "without it".to_owned()],
                chosen_option: 1,
                decided_by: crate::core::board::DecisionAuthor::Human,
                decided_at: time::OffsetDateTime::now_utc(),
                note: Some("the queue is out of scope for this release".to_owned()),
                supersedes: None,
            },
        },
        SmedEvent::ImportedItemFetched {
            session,
            item: crate::core::imported::ImportedItem {
                id: crate::core::imported::ImportedItemId::new(),
                integration: "github".to_owned(),
                remote_id: "42".to_owned(),
                source_url: "https://example.invalid/42".to_owned(),
                fetched_revision: "rev1".to_owned(),
                title: "an imported work item".to_owned(),
                state: crate::core::imported::ImportedItemState::Open,
                blocked_by: Vec::new(),
            },
        },
        SmedEvent::ImportedItemRefreshed {
            session,
            expected_revision: "rev1".to_owned(),
            item: crate::core::imported::ImportedItem {
                id: crate::core::imported::ImportedItemId::new(),
                integration: "github".to_owned(),
                remote_id: "42".to_owned(),
                source_url: "https://example.invalid/42".to_owned(),
                fetched_revision: "rev2".to_owned(),
                title: "an imported work item".to_owned(),
                state: crate::core::imported::ImportedItemState::Merged,
                blocked_by: Vec::new(),
            },
        },
        SmedEvent::ImportedActRecorded {
            session,
            act: crate::core::imported::ImportedAct {
                act_id: crate::core::imported::ImportedActId::new(),
                item_id: crate::core::imported::ImportedItemId::new(),
                kind: crate::core::imported::ImportedActKind::PullRequest,
                expected_revision: "rev1".to_owned(),
                head_branch: "feat/harness".to_owned(),
                base_branch: "main".to_owned(),
                outcome: crate::core::imported::ImportedActOutcome::Submitted {
                    remote_url: "https://example.invalid/42/pull/7".to_owned(),
                },
            },
        },
        SmedEvent::ImportedCommentRecorded {
            session,
            item_id: crate::core::imported::ImportedItemId::new(),
            comment_id: "comment-1".to_owned(),
            body: "looks good".to_owned(),
        },
    ];
    events.extend(continuation_events(session, run));
    events
}

fn continuation_events(session: SessionId, run: RunId) -> [SmedEvent; 2] {
    [
        SmedEvent::QuotaBoundaryReached {
            session,
            run,
            reserve: QuotaReserveStatus {
                basis: QuotaReserveBasis::ProviderReported {
                    window: "plan".to_owned(),
                },
                used_fraction: Some(0.82),
                soft_threshold: 0.8,
                hard_threshold: 0.95,
                resets_at: Some(time::OffsetDateTime::now_utc()),
                phase: QuotaReservePhase::Draining,
            },
        },
        SmedEvent::HandoffCreated {
            session,
            handoff: Box::new(HandoffCheckpoint {
                id: HandoffId::new(),
                created_at: time::OffsetDateTime::now_utc(),
                status: "done / remaining / next / risks".to_owned(),
                provider: ProviderId::new("fake"),
                model: ModelId::new("fake-1"),
                files_read: vec![std::path::PathBuf::from("a.rs")],
                files_changed: Vec::new(),
                commands: Vec::new(),
                usage: Usage::default(),
                budget: crate::core::runtime::BudgetStatus::default(),
                activated_skills: Vec::new(),
            }),
        },
    ]
}

fn model_refusal(session: SessionId) -> SmedEvent {
    SmedEvent::ModelChangeRefused {
        session,
        provider: ProviderId::new("anthropic"),
        model: ModelId::new("claude-sonnet-5"),
        code: ReasonCode::RunActive,
        detail: "finish the active run first".to_owned(),
    }
}

#[test]
fn every_durable_event_survives_a_round_trip_through_json() {
    let session = SessionId::new();
    let run = RunId::new();

    for event in every_durable_event(session, run) {
        let payload = encode(event.clone()).expect("encode");
        let json = encode_json(&payload).expect("to json");
        let decoded = decode_json(&json, WIRE_VERSION).expect("from json");
        assert_eq!(
            decode(session, decoded),
            event,
            "event did not survive persistence: {event:?}"
        );
    }
}

#[test]
fn the_format_cannot_express_display_only_events() {
    let session = SessionId::new();
    let run = RunId::new();
    let events = [
        SmedEvent::TextDelta {
            session,
            run,
            text: "tok".to_owned(),
        },
        SmedEvent::ReasoningDelta {
            session,
            run,
            text: "private reasoning".to_owned(),
        },
        SmedEvent::ToolAssembling {
            session,
            run,
            name: "list_dir".to_owned(),
        },
        SmedEvent::QuotaReported {
            session,
            run,
            snapshot: QuotaSnapshot {
                provider: ProviderId::new("fake"),
                windows: Vec::new(),
            },
        },
        SmedEvent::SubagentActivity {
            session,
            run,
            child: SessionId::new(),
            label: "tool write_file".to_owned(),
        },
    ];

    for event in events {
        let error = encode(event).expect_err("display event must have no persisted form");
        assert!(matches!(error, WireError::Ephemeral { .. }));
    }
}

#[test]
fn the_kind_column_matches_the_payload_tag() {
    // Two representations of the same fact; if they drift, queries and
    // diagnostics start disagreeing with the data.
    let session = SessionId::new();
    for event in every_durable_event(session, RunId::new()) {
        let payload = encode(event).expect("encode");
        let kind = payload.kind();
        let json: serde_json::Value =
            serde_json::from_str(&encode_json(&payload).expect("json")).expect("value");
        assert_eq!(
            json.get("kind").and_then(serde_json::Value::as_str),
            Some(kind),
            "the kind column and the payload tag disagree"
        );
    }
}

#[test]
fn every_durable_variant_is_covered_by_the_round_trip() {
    // Guards the fixture itself: a new SmedEvent variant that nobody adds
    // here would otherwise be "tested" by omission.
    let kinds: std::collections::HashSet<&str> =
        every_durable_event(SessionId::new(), RunId::new())
            .into_iter()
            .map(|event| encode(event).expect("encode").kind())
            .collect();

    assert_eq!(
        kinds.len(),
        41,
        "SmedEvent has a durable variant with no round-trip coverage"
    );
}

#[test]
fn a_newer_payload_version_is_refused_rather_than_read_best_effort() {
    let payload = encode(SmedEvent::RunStarted {
        session: SessionId::new(),
        run: RunId::new(),
    })
    .expect("encode");
    let json = encode_json(&payload).expect("json");

    let error = decode_json(&json, WIRE_VERSION + 1).expect_err("must refuse");
    assert!(matches!(error, WireError::UnsupportedVersion { .. }));

    // An older payload stays readable: that is the whole point of the stamp.
    assert!(decode_json(&json, WIRE_VERSION).is_ok());
}

#[test]
fn a_checkpoint_refuses_a_newer_version() {
    let json = encode_checkpoint(SessionCheckpoint::empty(SessionId::new())).expect("encode");
    assert!(decode_checkpoint(&json, WIRE_VERSION).is_ok());
    assert!(matches!(
        decode_checkpoint(&json, WIRE_VERSION + 1),
        Err(WireError::UnsupportedVersion { .. })
    ));
}

#[test]
fn a_payload_is_not_debug_output() {
    // The plan's anti-pattern: Debug as a serialization contract. If someone
    // "simplifies" encode_json to format!("{event:?}"), this fails.
    let payload = encode(SmedEvent::RunStarted {
        session: SessionId::new(),
        run: RunId::new(),
    })
    .expect("encode");
    let json = encode_json(&payload).expect("json");

    assert!(json.starts_with('{'), "the payload must be JSON: {json}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&json).is_ok(),
        "the payload must parse as JSON: {json}"
    );
}

#[test]
fn the_message_bearing_kind_list_matches_the_event_predicate() {
    // Recovery re-anchors a checkpoint's transcript by counting message-bearing
    // events in SQL, but the transcript itself is built by the projection in
    // Rust. The two must agree about which events yield a message: if SQL
    // counted one the projection did not produce, every entry after it would
    // anchor to the wrong event, and `/tree` would silently rewind to the wrong
    // place. Checked over every durable variant so a new one cannot slip in on
    // one side only.
    for event in every_durable_event(SessionId::new(), RunId::new()) {
        let predicate = event.introduces_message();
        let kind = encode(event).expect("encode").kind();
        let in_sql = MESSAGE_BEARING_KINDS.contains(&kind);
        assert_eq!(
            predicate, in_sql,
            "`{kind}`: SmedEvent::introduces_message says {predicate}, \
             but MESSAGE_BEARING_KINDS says {in_sql}"
        );
    }
}

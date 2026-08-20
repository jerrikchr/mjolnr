//! Unit tests for the frontend-safe client contract types.

#![allow(clippy::indexing_slicing)]

use super::*;
use crate::core::command::ApprovalDecision;
use crate::core::policy::PolicyMode;

fn sample_plan_workflow() -> ClientPlanWorkflow {
    let proposal = ClientPlanProposal {
        plan_id: "0190d5f0-0000-7000-8000-000000000010".to_owned(),
        revision_id: 2,
        title: "Tighten the wire contract".to_owned(),
        summary: "Replace runtime plan types with client DTOs.".to_owned(),
        steps: vec![
            ClientPlanStep {
                index: 1,
                title: "Define DTOs".to_owned(),
                description: "Introduce client plan structures.".to_owned(),
            },
            ClientPlanStep {
                index: 2,
                title: "Convert runtime state".to_owned(),
                description: "Map every stage deterministically.".to_owned(),
            },
        ],
        proposed_at: "2026-07-29T04:00:00Z".to_owned(),
    };
    let review = ClientPlanReview {
        plan_id: proposal.plan_id.clone(),
        revision_id: proposal.revision_id,
        reviewer: "critic".to_owned(),
        verdict: ClientReviewVerdict::Approve,
        feedback: "Clear and bounded.".to_owned(),
        reviewed_at: "2026-07-29T04:05:00Z".to_owned(),
    };
    let approval = ClientPlanApproval {
        plan_id: proposal.plan_id.clone(),
        revision_id: proposal.revision_id,
        approver: "owner".to_owned(),
        decision: ClientReviewVerdict::Approve,
        note: Some("Ship it.".to_owned()),
        approved_at: "2026-07-29T04:10:00Z".to_owned(),
    };
    let handoff = ClientPlanHandoff {
        plan_id: proposal.plan_id.clone(),
        revision_id: proposal.revision_id,
        handoff_note: "Implementation can start.".to_owned(),
        created_at: "2026-07-29T04:15:00Z".to_owned(),
    };
    ClientPlanWorkflow {
        plan_id: proposal.plan_id.clone(),
        interview_goal: Some("A durable plan workflow".to_owned()),
        questions: vec![],
        answers: vec![],
        prd: None,
        council_link: None,
        active_revision: Some(proposal.revision_id),
        stage: ClientPlanStage::Reviewed {
            proposal: proposal.clone(),
            reviews: vec![review.clone()],
        },
        proposals: vec![proposal],
        reviews: vec![review],
        approvals: vec![approval],
        handoffs: vec![handoff],
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive snapshot fixture; splitting it hides which fields a round-trip covers"
)]
fn sample_snapshot() -> ClientSnapshot {
    ClientSnapshot {
        revision: 42,
        session: Some("0190d5f0-0000-7000-8000-000000000001".to_owned()),
        provider: Some("anthropic".to_owned()),
        model: Some("claude-sonnet-5".to_owned()),
        workspace_root: Some("/work/project".to_owned()),
        policy: ClientPolicy::Ask,
        run_active: true,
        usage: ClientUsage {
            input_tokens: 1200,
            output_tokens: 300,
        },
        budget: ClientBudget {
            provider_turns: 2,
            max_provider_turns: 25,
            tool_calls: 5,
            max_tool_calls: 100,
        },
        quota: None,
        messages: vec![
            ClientMessage::User {
                id: "m1".to_owned(),
                text: "hello".to_owned(),
                text_truncated: false,
            },
            ClientMessage::Assistant {
                id: "m2".to_owned(),
                text: "working on it".to_owned(),
                text_truncated: false,
                provider: Some("anthropic".to_owned()),
                model: Some("claude-sonnet-5".to_owned()),
                tool_calls: vec![ClientToolCallRef {
                    id: "call_1".to_owned(),
                    name: "read_file".to_owned(),
                }],
            },
            ClientMessage::Tool {
                id: "m3".to_owned(),
                name: "read_file".to_owned(),
                outcome: ClientToolOutcome::Ok,
                reason_code: None,
                detail: "file contents".to_owned(),
                detail_truncated: false,
            },
        ],
        messages_omitted: 7,
        pending_approval: Some(ClientApproval {
            id: "0190d5f0-0000-7000-8000-000000000002".to_owned(),
            tool_name: "run_command".to_owned(),
            tier: "execute".to_owned(),
            preview: "/bin/sh -c \"make test\"".to_owned(),
        }),
        recovery: ClientRecovery::Required {
            run: "0190d5f0-0000-7000-8000-000000000003".to_owned(),
            kind: "EFFECT_UNCERTAIN".to_owned(),
            summary: "`edit_file` may or may not have run.".to_owned(),
            effect_is_certain: false,
            tool_name: Some("edit_file".to_owned()),
            preview: Some("diff".to_owned()),
        },
        store_failure: None,
        context_diagnostics: vec![ClientContextDiagnostic {
            code: "SCHEMA_INVALID".to_owned(),
            detail: "sample diagnostic for the client wire round-trip".to_owned(),
        }],
        models: vec![ClientModelChoice {
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-5".to_owned(),
            display_name: "Claude Sonnet 5".to_owned(),
        }],
        resume_advice: Some(ClientResumeAdvice {
            warning: "stale".to_owned(),
            estimated_full_resume_tokens: 12000,
            has_handoff: true,
        }),
        sessions: vec![],
        active_persona: Some("architect".to_owned()),
        accounts: vec![],
        personas: vec![],
        souls: vec![],
        routes: vec![],
        council: None,
        plan: Some(sample_plan_workflow()),
        changes: None,
        repository: crate::core::client::workspace::RepositoryState {
            branch: Some("main".to_owned()),
            head: Some("abc123".to_owned()),
            index_revision: Some("tree789".to_owned()),
            dirty_count: 1,
            dirty_count_truncated: false,
            staged_files: vec!["src/lib.rs".to_owned()],
            modified_files: vec![],
            untracked_files: vec![],
            unmerged_files: vec![],
            rebase_in_progress: false,
            paths_truncated: false,
            remote_sync: crate::core::client::workspace::RepositorySyncState::Unknown,
            remote_sync_as_of: None,
            freshness: crate::core::client::workspace::RepositoryFreshness::CapturedAt {
                trigger: "projectOpened".to_owned(),
                sequence: 1,
            },
            trust: crate::core::client::workspace::TrustClass::MjolnrGoverned,
        },
        review_threads: crate::core::client::workspace::BoundedProjection {
            items: vec![crate::core::client::workspace::ReviewThreadSummary {
                id: "0190d5f0-0000-7000-8000-000000000004".to_owned(),
                status: "sent".to_owned(),
                comment_count: 1,
                comment_count_truncated: false,
                // A human's remark about code, never a mjolnr-governed
                // observation.
                trust: crate::core::client::workspace::TrustClass::OperatorControlled,
                anchor: crate::core::client::workspace::ReviewAnchorView {
                    path: "src/lib.rs".to_owned(),
                    side: "new".to_owned(),
                    line: 12,
                    hunk_header: "@@ -10,3 +10,4 @@".to_owned(),
                    capture_digest: "9f8e7d".to_owned(),
                    base_object_id: Some("abc123".to_owned()),
                },
                anchor_stale: true,
                comments: vec![crate::core::client::workspace::ReviewCommentView {
                    body: "handle the None case".to_owned(),
                    body_truncated: false,
                    created_at: "2026-07-30T12:00:00Z".to_owned(),
                }],
                response_message_id: Some("m2".to_owned()),
            }],
            limit: crate::core::client::workspace::MAX_REVIEW_THREADS_PER_ITEM,
            total: Some(1),
            truncated: false,
            reason_code: None,
        },
        memory: Some(ClientMemorySummary {
            rules_count: 2,
            user_profile_present: true,
            facts_count: Some(5),
            episodes_count: Some(1),
            projection_error: None,
            rules_error: None,
            rule_names: vec!["style".to_owned(), "security".to_owned()],
        }),
        plugins: vec![crate::core::plugin::PluginSummary {
            name: "acme.deploy".to_owned(),
            version: "1.0.0".to_owned(),
            publisher: "acme-corp".to_owned(),
            description: "Deployment governance plugin".to_owned(),
            tool_count: 1,
            hook_count: 1,
            required_credentials: vec!["DEPLOY_TOKEN".to_owned()],
            source_url: None,
        }],
        fleet: Some(crate::core::fleet::FleetSummary {
            visible: true,
            active_count: 1,
            agents: vec![crate::core::fleet::FleetAgentSummary {
                child_session_id: crate::core::event::SessionId::new(),
                short_name: "sub-1".to_owned(),
                role: Some("researcher".to_owned()),
                status: crate::core::fleet::FleetAgentStatus::Running,
                latest_activity: "searching codebase".to_owned(),
                feed: vec!["started".to_owned(), "searching codebase".to_owned()],
                worktree_branch: Some("mjolnr/worktree-sub-1".to_owned()),
            }],
        }),
        preview: Some(crate::core::preview::PreviewState::default()),
        external_agents: Vec::new(),
        external_agent_capability: crate::core::client::external_agent::ExternalAgentCapability {
            available: false,
            reason: Some("no external-agent profiles discovered".to_owned()),
        },
    }
}

#[test]
fn snapshot_round_trips_through_json() {
    let snapshot = sample_snapshot();
    let json = serde_json::to_string(&snapshot).expect("serialize");
    let back: ClientSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(snapshot, back);
}

#[test]
fn every_message_variant_round_trips() {
    let snapshot = sample_snapshot();
    for message in snapshot.messages {
        let json = serde_json::to_string(&message).expect("serialize");
        let back: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(message, back);
    }
}

#[test]
fn snapshot_uses_the_documented_wire_shape() {
    let json = serde_json::to_value(sample_snapshot()).expect("serialize");
    assert!(json.get("runActive").is_some());
    assert!(json.get("messagesOmitted").is_some());
    assert!(json.get("pendingApproval").is_some());
    assert_eq!(json["policy"], "ask");
    assert_eq!(json["messages"][0]["kind"], "user");
    assert_eq!(json["messages"][2]["kind"], "tool");
    assert_eq!(json["recovery"]["state"], "required");
    assert_eq!(
        json["plan"]["planId"],
        "0190d5f0-0000-7000-8000-000000000010"
    );
    assert_eq!(json["plan"]["activeRevision"], 2);
    assert_eq!(
        json["plan"]["stage"]["Reviewed"]["proposal"]["revisionId"],
        2
    );
    assert_eq!(
        json["plan"]["stage"]["Reviewed"]["reviews"][0]["verdict"],
        "approve"
    );
    assert_eq!(
        json["plan"]["handoffs"][0]["handoffNote"],
        "Implementation can start."
    );
}

#[test]
fn every_command_variant_round_trips() {
    let commands = vec![
        ClientCommand::OpenProject {
            root: "/work".to_owned(),
        },
        ClientCommand::CreateSession {
            provider: "openai".to_owned(),
            model: "gpt-4o-mini".to_owned(),
        },
        ClientCommand::ResumeSession {
            session: "0190d5f0-0000-7000-8000-000000000004".to_owned(),
        },
        ClientCommand::ResolveResume {
            choice: ClientResumeChoice::Compact,
        },
        ClientCommand::SendMessage {
            text: "do the thing".to_owned(),
        },
        ClientCommand::CancelRun,
        ClientCommand::ResolveApproval {
            approval: "0190d5f0-0000-7000-8000-000000000005".to_owned(),
            decision: ClientApprovalDecision::ApproveOnce,
        },
        ClientCommand::ResolveRecovery {
            decision: ClientRecoveryDecision::EndSession,
        },
        ClientCommand::SetPolicy {
            policy: ClientPolicy::WorkspaceWrite,
        },
        ClientCommand::StartPlanInterview {
            goal: "Make planning durable".to_owned(),
        },
        ClientCommand::AskPlanQuestion {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            prompt: "Which environment?".to_owned(),
            options: vec!["dev".to_owned(), "prod".to_owned()],
            is_multi_select: false,
        },
        ClientCommand::AnswerPlanQuestion {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            question_id: "0190d5f0-0000-7000-8000-000000000007".to_owned(),
            selected_options: vec!["dev".to_owned()],
            freeform_text: Some("Start narrow".to_owned()),
        },
        ClientCommand::ProposePlan {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            revision: 2,
            title: "Plan".to_owned(),
            summary: "Bounded".to_owned(),
            steps: vec![ClientPlanStep {
                index: 1,
                title: "Do it".to_owned(),
                description: "Precisely".to_owned(),
            }],
        },
        ClientCommand::ReviewPlan {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            revision: 2,
            reviewer: "critic".to_owned(),
            verdict: ClientReviewVerdict::Iterate,
            feedback: "One more pass".to_owned(),
        },
        ClientCommand::ApprovePlan {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            revision: 2,
            decision: ClientReviewVerdict::Approve,
            note: Some("Looks good".to_owned()),
        },
        ClientCommand::HandoffPlan {
            plan_id: "0190d5f0-0000-7000-8000-000000000006".to_owned(),
            revision: 2,
            note: "Execute".to_owned(),
        },
        ClientCommand::EndSession,
        ClientCommand::RequestSnapshot,
        ClientCommand::ExternalAgentList,
        ClientCommand::ExternalAgentLaunch {
            profile: "codex".to_owned(),
        },
        ClientCommand::ExternalAgentStop {
            id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        },
        ClientCommand::ExternalAgentImport {
            id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        },
    ];
    assert_eq!(
        commands.len(),
        22,
        "the client command allowlist changed; that is a reviewed contract act"
    );
    for command in commands {
        let json = serde_json::to_string(&command).expect("serialize");
        let back: ClientCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(command, back);
    }
}

#[test]
fn review_verdicts_keep_the_lowercase_kebab_wire_contract() {
    assert_eq!(
        serde_json::to_string(&ClientReviewVerdict::Approve).expect("serialize"),
        "\"approve\""
    );
    assert_eq!(
        serde_json::to_string(&ClientReviewVerdict::Iterate).expect("serialize"),
        "\"iterate\""
    );
    assert_eq!(
        serde_json::to_string(&ClientReviewVerdict::Reject).expect("serialize"),
        "\"reject\""
    );
}

#[test]
fn idle_plan_stage_keeps_the_unit_enum_wire_shape() {
    assert_eq!(
        serde_json::to_string(&ClientPlanStage::Idle).expect("serialize"),
        "\"Idle\""
    );
}

#[test]
fn a_command_cannot_smuggle_tool_execution_or_provider_calls() {
    for hostile in [
        r#"{"type":"executeTool","name":"run_command","arguments":{}}"#,
        r#"{"type":"callProvider","provider":"openai","request":{}}"#,
        r#"{"type":"runTool","tool":"write_file","input":"x"}"#,
        r#"{"type":"registerCredential","provider":"openai","secret":"sk-…"}"#,
        r#"{"type":"sendMessage","text":"hi","source":"internal"}"#,
    ] {
        assert!(
            serde_json::from_str::<ClientCommand>(hostile).is_err(),
            "the client command vocabulary must refuse non-contract intent: {hostile}"
        );
    }
}

/// §E5 contract (a): `expectedRevision` is required on the wire, so a client
/// that never learned about the staleness check cannot post a change without
/// one. An optional pin would default to "unchecked" for exactly the older
/// clients least likely to be showing a fresh view.
#[test]
fn a_submit_change_without_a_revision_pin_does_not_parse() {
    let without = r#"{"type":"submitChange","source":"github",
        "request":{"remoteId":"42","title":"t","body":"b"}}"#;
    assert!(
        serde_json::from_str::<ClientCommand>(without).is_err(),
        "a change with no revision pin must be refused at the wire, not defaulted"
    );

    let with = r#"{"type":"submitChange","source":"github",
        "request":{"remoteId":"42","expectedRevision":"rev1","title":"t","body":"b",
        "headCommit":"abc123","headBranch":"feature/parser","baseBranch":"main"}}"#;
    assert!(
        serde_json::from_str::<ClientCommand>(with).is_ok(),
        "the pinned form is the shape clients send"
    );
}

#[test]
fn approval_decisions_cannot_claim_policy_authority() {
    let decisions = [
        ClientApprovalDecision::Deny,
        ClientApprovalDecision::ApproveOnce,
        ClientApprovalDecision::ApproveExactForSession,
    ];
    assert_eq!(
        decisions.len(),
        3,
        "AutoByPolicy must stay unspeakable by clients: it records what the \
         runtime did, and a client may not speak for the policy engine"
    );
    assert!(serde_json::from_str::<ClientApprovalDecision>("\"auto-by-policy\"").is_err());
}

#[test]
fn the_event_feed_reports_auto_resolution_as_what_it_is() {
    assert_eq!(
        ClientApprovalResolution::from(ApprovalDecision::AutoByPolicy),
        ClientApprovalResolution::AutoByPolicy
    );
    assert_eq!(
        serde_json::to_string(&ClientApprovalResolution::AutoByPolicy).expect("serialize"),
        "\"auto-by-policy\""
    );
}

#[test]
fn every_event_variant_round_trips() {
    let events = vec![
        ClientEvent::SessionStarted {
            session: "s".to_owned(),
            provider: "p".to_owned(),
            model: "m".to_owned(),
        },
        ClientEvent::RunStarted {
            run: "r".to_owned(),
        },
        ClientEvent::TextDelta {
            run: "r".to_owned(),
            text: "fragment".to_owned(),
            text_truncated: false,
        },
        ClientEvent::ReasoningDelta {
            run: "r".to_owned(),
            text: "thinking".to_owned(),
            text_truncated: false,
        },
        ClientEvent::ToolAssembling {
            run: "r".to_owned(),
            name: "edit_file".to_owned(),
        },
        ClientEvent::ToolProposed {
            run: "r".to_owned(),
            approval: Some("a".to_owned()),
            name: "run_command".to_owned(),
            preview: "argv".to_owned(),
        },
        ClientEvent::ApprovalResolved {
            run: "r".to_owned(),
            approval: "a".to_owned(),
            decision: ClientApprovalResolution::AutoByPolicy,
        },
        ClientEvent::ToolCompleted {
            run: "r".to_owned(),
            name: "read_file".to_owned(),
            outcome: ClientToolOutcome::Refused,
            reason_code: Some("POLICY_READ_ONLY".to_owned()),
        },
        ClientEvent::RunFinished {
            run: "r".to_owned(),
            reason: ClientFinishReason::Cancelled,
        },
        ClientEvent::RunFailed {
            run: "r".to_owned(),
            code: "BUDGET_EXHAUSTED".to_owned(),
            detail: "budget".to_owned(),
            detail_truncated: false,
        },
        ClientEvent::PolicyChanged {
            policy: ClientPolicy::FullAuto,
        },
        ClientEvent::ModelChanged {
            provider: "openai".to_owned(),
            model: "gpt-4.1".to_owned(),
        },
        ClientEvent::SubagentActivity {
            child: "c".to_owned(),
            label: "child working".to_owned(),
        },
        ClientEvent::SubagentSpawned {
            child: "c".to_owned(),
            directive: "do a bounded thing".to_owned(),
            directive_truncated: false,
            branch: "mjolnr/child".to_owned(),
            worktree: "/tmp/mjolnr-child".to_owned(),
        },
        ClientEvent::RecoveryRequired {
            work: Box::new(ClientRecoveryWork {
                run: "r".to_owned(),
                kind: "PROVIDER_TURN_INTERRUPTED".to_owned(),
                summary: "interrupted".to_owned(),
                effect_is_certain: false,
                tool_name: None,
                preview: None,
            }),
        },
        ClientEvent::RecoveryResolved {
            decision: ClientRecoveryDecision::AbandonAndContinue,
        },
        ClientEvent::SessionEnded,
    ];
    for event in events {
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ClientEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }
}

#[test]
fn every_update_variant_round_trips() {
    let updates = vec![
        ClientUpdate::Snapshot {
            snapshot: sample_snapshot(),
        },
        ClientUpdate::Event {
            sequence: 9,
            event: ClientEvent::SessionEnded,
        },
        ClientUpdate::Resync {
            missed: 12,
            snapshot: sample_snapshot(),
        },
        ClientUpdate::Closed,
    ];
    for update in &updates {
        let json = serde_json::to_string(update).expect("serialize");
        let back: ClientUpdate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*update, back);
    }
    assert_eq!(
        serde_json::to_value(&updates[2]).expect("value")["type"],
        "resync"
    );
}

#[test]
fn session_summary_round_trips() {
    let summary = ClientSessionSummary {
        id: "s".to_owned(),
        title: "fix the bug".to_owned(),
        status: "active".to_owned(),
        rollup_status: ClientRollupStatus::Running,
        provider: Some("gemini".to_owned()),
        model: Some("gemini-2.5-pro".to_owned()),
        updated_at: "2026-07-28T12:00:00Z".to_owned(),
        event_count: 5,
        leased: true,
        parent: None,
    };
    let json = serde_json::to_string(&summary).expect("serialize");
    let back: ClientSessionSummary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(summary, back);
}

#[test]
fn policy_labels_match_the_internal_vocabulary() {
    assert_eq!(
        ClientPolicy::from(PolicyMode::ReadOnly).label(),
        "read-only"
    );
    assert_eq!(
        ClientPolicy::from(PolicyMode::FullAuto).label(),
        "full-auto"
    );
    assert_eq!(
        PolicyMode::from(ClientPolicy::WorkspaceWrite),
        PolicyMode::WorkspaceWrite
    );
}

#[test]
fn truncation_discloses_and_never_splits_a_scalar() {
    let (kept, cut) = truncate_text("hello", 10);
    assert_eq!(kept, "hello");
    assert!(!cut);

    let (cut_text, cut) = truncate_text("hello world", 5);
    assert_eq!(cut_text, "hello");
    assert!(cut);

    let emoji = "a🦀b🦀c";
    let (cut_text, cut) = truncate_text(emoji, 3);
    assert_eq!(cut_text, "a🦀b");
    assert!(cut);
}

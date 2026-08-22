//! Conversion functions between internal runtime state/events and frontend-safe DTOs.
//!
//! Rollup vocabulary (Phase D1): `rollup_status_for` collapses session state to
//! four values (`Running` / `Active` / `Draft` / `Completed`) until later phases
//! introduce blocked / failed / approval-required rollups. `SessionStatus` has no
//! archived state, so no rollup can ever produce one; `Completed` is the
//! catch-all for ended sessions, not a positive claim about outcome. Do not
//! extend the enum without re-reading `docs/integrated-workspace-phases.md` §D1.

use crate::core::client::{
    ClientAccount, ClientApproval, ClientBudget, ClientContextDiagnostic, ClientCouncilArtifact,
    ClientCouncilContribution, ClientCouncilDispositionRecord, ClientCouncilFinding,
    ClientCouncilPosition, ClientCouncilReview, ClientEvent, ClientMessage, ClientModelChoice,
    ClientPersonaScope, ClientPersonaSummary, ClientPlanAnswer, ClientPlanApproval,
    ClientPlanCouncilLink, ClientPlanHandoff, ClientPlanProposal, ClientPlanQuestion,
    ClientPlanReview, ClientPlanStage, ClientPlanWorkflow, ClientPrdRequirement,
    ClientProductRequirementsDocument, ClientProviderConnectionState, ClientQuota,
    ClientQuotaWindow, ClientRecovery, ClientRecoveryWork, ClientResumeAdvice, ClientRollupStatus,
    ClientRoute, ClientSessionSummary, ClientSnapshot, ClientToolCallRef, ClientToolOutcome,
    ClientUsage, MAX_ACTIVITY_TEXT, MAX_DIRECTIVE_TEXT, MAX_MESSAGE_TEXT, MAX_SNAPSHOT_MESSAGES,
    truncate_text,
};
use crate::core::context::{PersonaSummary, SkillScope};
use crate::core::event::MjolnrEvent;
use crate::core::message::{CanonicalMessage, ContentBlock, Role, ToolOutcome, TranscriptEntry};
use crate::core::plan::{
    PlanApproval, PlanHandoff, PlanProposal, PlanReview, PlanStage, PlanWorkflow, PrdRequirement,
    ProductRequirementsDocument, Question, QuestionAnswer,
};
use crate::core::recovery::{InterruptedKind, InterruptedWork, RecoveryState};
use crate::core::runtime::{ProviderConnectionState, RouteChoice, RuntimeSnapshot};
use crate::core::store::SessionSummary;
use crate::core::tool::ToolTier;

#[must_use]
pub fn snapshot_to_client(revision: u64, snapshot: &RuntimeSnapshot) -> ClientSnapshot {
    let total = snapshot.messages.len();
    let keep_from = total.saturating_sub(MAX_SNAPSHOT_MESSAGES);
    let messages: Vec<ClientMessage> = snapshot
        .messages
        .iter()
        .skip(keep_from)
        .map(message_to_client)
        .collect();
    let quota = snapshot_quota(snapshot);

    ClientSnapshot {
        revision,
        session: snapshot.session.map(|id| id.to_string()),
        provider: snapshot.provider.as_ref().map(|id| id.as_str().to_owned()),
        model: snapshot.model.as_ref().map(|id| id.as_str().to_owned()),
        workspace_root: snapshot
            .workspace_root
            .as_ref()
            .map(|root| root.display().to_string()),
        policy: snapshot.policy.into(),
        run_active: snapshot.run_active,
        usage: ClientUsage {
            input_tokens: snapshot.usage.input_tokens,
            output_tokens: snapshot.usage.output_tokens,
        },
        budget: ClientBudget {
            provider_turns: snapshot.budget.provider_turns,
            max_provider_turns: snapshot.budget.max_provider_turns,
            tool_calls: snapshot.budget.tool_calls,
            max_tool_calls: snapshot.budget.max_tool_calls,
        },
        quota,
        messages,
        messages_omitted: u64::try_from(keep_from).unwrap_or(u64::MAX),
        pending_approval: snapshot
            .pending_approval
            .as_ref()
            .map(|pending| ClientApproval {
                id: pending.id.to_string(),
                tool_name: pending.tool_name.clone(),
                tier: tier_label(pending.tier).to_owned(),
                preview: pending.preview.clone(),
            }),
        recovery: recovery_to_client(&snapshot.recovery),
        store_failure: snapshot.store_failure.clone(),
        context_diagnostics: snapshot
            .context_diagnostics
            .iter()
            .map(|diagnostic| ClientContextDiagnostic {
                code: diagnostic.code.as_str().to_owned(),
                detail: diagnostic.detail.clone(),
            })
            .collect(),
        models: snapshot
            .models
            .iter()
            .map(|choice| ClientModelChoice {
                provider: choice.descriptor.provider.as_str().to_owned(),
                model: choice.descriptor.id.as_str().to_owned(),
                display_name: choice.descriptor.display_name.clone(),
            })
            .collect(),
        resume_advice: snapshot
            .resume_advice
            .as_ref()
            .map(|advice| ClientResumeAdvice {
                warning: match &advice.warning {
                    crate::core::continuation::ResumeWarning::QuotaStopped { .. } => {
                        "quota-stopped".to_owned()
                    }
                    crate::core::continuation::ResumeWarning::Stale { .. } => "stale".to_owned(),
                },
                estimated_full_resume_tokens: advice.estimated_full_resume_tokens,
                has_handoff: advice.handoff.is_some(),
            }),
        active_persona: snapshot.active_persona.clone(),
        personas: snapshot_personas(snapshot),
        souls: snapshot.souls.as_ref().clone(),
        routes: snapshot_routes(snapshot),
        accounts: snapshot_accounts(snapshot),
        sessions: snapshot_sessions(snapshot),
        council: snapshot
            .last_council
            .as_ref()
            .map(|review| council_to_client(review, snapshot.last_council_amendment.as_ref())),
        plan: snapshot.plan.as_ref().map(plan_workflow_to_client),
        changes: super::workspace::project_change_set(&snapshot.changes, &snapshot.read_evidence),
        repository: super::workspace::project_repository_state(&snapshot.repository),
        review_threads: super::workspace::project_review_thread_summaries(
            &snapshot.review_threads,
            &snapshot.changes,
        ),
        memory: Some(memory_summary_to_client(&snapshot.memory)),
        plugins: (*snapshot.plugins).clone(),
        fleet: Some((*snapshot.fleet).clone()),
        preview: Some((*snapshot.preview).clone()),
        external_agents: snapshot.external_agents.clone(),
        external_agent_capability: snapshot.external_agent_capability.clone(),
    }
}

fn memory_summary_to_client(
    memory: &crate::core::memory::MemorySummary,
) -> crate::core::client::types::ClientMemorySummary {
    crate::core::client::types::ClientMemorySummary {
        rules_count: memory.rules_count,
        user_profile_present: memory.user_profile_present,
        facts_count: memory.facts_count,
        episodes_count: memory.episodes_count,
        projection_error: memory.projection_error.clone(),
        rules_error: memory.rules_error.clone(),
        rule_names: memory.rule_names.clone(),
    }
}

fn council_to_client(
    review: &crate::core::council::CouncilReview,
    amendment: Option<&crate::core::council::CouncilAmendment>,
) -> ClientCouncilReview {
    ClientCouncilReview {
        review_id: review.review_id.to_string(),
        question: review.question.clone(),
        contributions: review
            .contributions
            .iter()
            .map(|contribution| ClientCouncilContribution {
                role: contribution.role.clone(),
                proposal: contribution.proposal.clone(),
                critique: contribution.critique.clone(),
            })
            .collect(),
        rounds_conducted: review.rounds_conducted,
        artifact: review
            .artifact
            .as_ref()
            .map(|artifact| ClientCouncilArtifact {
                path: artifact.path.clone(),
                source_digest: artifact.source_digest.clone(),
            }),
        findings: review
            .findings
            .iter()
            .map(|finding| ClientCouncilFinding {
                id: finding.id.to_string(),
                section: finding.section.clone(),
                title: finding.title.clone(),
                positions: finding
                    .positions
                    .iter()
                    .map(|position| ClientCouncilPosition {
                        role: position.role.clone(),
                        response: position.response.clone(),
                        critique: position.critique.clone(),
                    })
                    .collect(),
                disposition: finding.disposition.as_ref().map(|disposition| {
                    ClientCouncilDispositionRecord {
                        disposition: match disposition.disposition {
                            crate::core::council::CouncilDisposition::Accept => {
                                crate::core::client::ClientCouncilDisposition::Accept
                            }
                            crate::core::council::CouncilDisposition::Reject => {
                                crate::core::client::ClientCouncilDisposition::Reject
                            }
                            crate::core::council::CouncilDisposition::Defer => {
                                crate::core::client::ClientCouncilDisposition::Defer
                            }
                        },
                        note: disposition.note.clone(),
                        decided_at: disposition.decided_at.to_string(),
                    }
                }),
            })
            .collect(),
        // Only carried when it belongs to this review: a proposal composed
        // against an earlier review must not appear attached to a later one.
        amendment: amendment
            .filter(|amendment| amendment.review_id == review.review_id)
            .map(|amendment| crate::core::client::ClientCouncilAmendment {
                review_id: amendment.review_id.to_string(),
                path: amendment.path.clone(),
                source_digest: amendment.source_digest.clone(),
                accepted_findings: amendment.accepted_findings,
                text: amendment.text.clone(),
            }),
    }
}

/// The single derivation for a session's rollup status. Leased beats status:
/// a session some process holds a write lease on is running, whatever its
/// stored status says. See the module docs for why there is no `Archived`.
#[must_use]
fn rollup_status_for(summary: &SessionSummary) -> ClientRollupStatus {
    if summary.leased {
        ClientRollupStatus::Running
    } else if summary.status == crate::core::store::SessionStatus::Active {
        if summary.event_count == 0 {
            ClientRollupStatus::Draft
        } else {
            ClientRollupStatus::Active
        }
    } else {
        ClientRollupStatus::Completed
    }
}

#[must_use]
pub fn session_summary_to_client(summary: &SessionSummary) -> ClientSessionSummary {
    ClientSessionSummary {
        id: summary.id.to_string(),
        title: summary.title.clone(),
        project_root: summary.project_root.to_string_lossy().into_owned(),
        status: summary.status.as_str().to_owned(),
        rollup_status: rollup_status_for(summary),
        provider: summary.provider.as_ref().map(|id| id.as_str().to_owned()),
        model: summary.model.as_ref().map(|id| id.as_str().to_owned()),
        updated_at: summary
            .updated_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        event_count: summary.event_count,
        leased: summary.leased,
        parent: summary.parent.map(|id| id.to_string()),
    }
}

fn message_to_client(entry: &TranscriptEntry) -> ClientMessage {
    let message = &entry.message;
    let id = message.id.to_string();
    let at = message
        .created_at
        .format(&time::format_description::well_known::Rfc3339)
        .ok();
    match message.role {
        Role::User => {
            let (text, text_truncated) = truncate_text(&message.text(), MAX_MESSAGE_TEXT);
            ClientMessage::User {
                id,
                text,
                text_truncated,
                at,
            }
        }
        Role::System => {
            let (text, text_truncated) = truncate_text(&message.text(), MAX_MESSAGE_TEXT);
            ClientMessage::System {
                id,
                text,
                text_truncated,
                at,
            }
        }
        Role::Assistant => {
            let (text, text_truncated) = truncate_text(&message.text(), MAX_MESSAGE_TEXT);
            ClientMessage::Assistant {
                id,
                text,
                text_truncated,
                provider: message.provider.as_ref().map(|id| id.as_str().to_owned()),
                model: message.model.as_ref().map(|id| id.as_str().to_owned()),
                tool_calls: message
                    .tool_calls()
                    .map(|call| ClientToolCallRef {
                        id: call.id.clone(),
                        name: call.name.clone(),
                    })
                    .collect(),
                at,
            }
        }
        Role::Tool => tool_message_to_client(id, message, at),
    }
}

fn tool_message_to_client(
    id: String,
    message: &CanonicalMessage,
    at: Option<String>,
) -> ClientMessage {
    for block in &message.blocks {
        if let ContentBlock::ToolResult { name, result, .. } = block {
            let (outcome, reason_code) = outcome_to_client(&result.outcome);
            let (detail, detail_truncated) = truncate_text(&result.content, MAX_ACTIVITY_TEXT);
            return ClientMessage::Tool {
                id,
                name: name.clone(),
                outcome,
                reason_code,
                detail,
                detail_truncated: detail_truncated || result.truncated,
                at,
            };
        }
    }
    let (text, text_truncated) = truncate_text(&message.text(), MAX_MESSAGE_TEXT);
    ClientMessage::System {
        id,
        text,
        text_truncated,
        at,
    }
}

fn outcome_to_client(outcome: &ToolOutcome) -> (ClientToolOutcome, Option<String>) {
    match outcome {
        ToolOutcome::Ok => (ClientToolOutcome::Ok, None),
        ToolOutcome::Refused(code) => (ClientToolOutcome::Refused, Some(code.as_str().to_owned())),
        ToolOutcome::Failed(code) => (ClientToolOutcome::Failed, Some(code.as_str().to_owned())),
    }
}

fn recovery_to_client(recovery: &RecoveryState) -> ClientRecovery {
    match recovery {
        RecoveryState::Clean => ClientRecovery::Clean,
        RecoveryState::Required(work) => {
            let work = recovery_work_to_client(work);
            ClientRecovery::Required {
                run: work.run,
                kind: work.kind,
                summary: work.summary,
                effect_is_certain: work.effect_is_certain,
                tool_name: work.tool_name,
                preview: work.preview,
            }
        }
    }
}

fn recovery_work_to_client(work: &InterruptedWork) -> ClientRecoveryWork {
    let (tool_name, preview) = match &work.kind {
        InterruptedKind::ProposalUnapproved { call, preview, .. }
        | InterruptedKind::EffectUncertain { call, preview, .. } => {
            (Some(call.name.clone()), Some(preview.clone()))
        }
        InterruptedKind::ProviderTurnInterrupted => (None, None),
    };
    ClientRecoveryWork {
        run: work.run.to_string(),
        kind: work.kind.label().to_owned(),
        summary: work.summary(),
        effect_is_certain: work.effect_is_certain(),
        tool_name,
        preview,
    }
}

fn plan_workflow_to_client(workflow: &PlanWorkflow) -> ClientPlanWorkflow {
    ClientPlanWorkflow {
        plan_id: workflow.plan_id.to_string(),
        interview_goal: workflow.interview_goal.clone(),
        questions: workflow.questions.iter().map(question_to_client).collect(),
        answers: workflow.answers.iter().map(answer_to_client).collect(),
        prd: workflow.prd.as_ref().map(prd_to_client),
        council_link: workflow
            .council_link
            .as_ref()
            .map(|link| ClientPlanCouncilLink {
                plan_id: link.plan_id.to_string(),
                prd_id: link.prd_id.to_string(),
                review_id: link.review_id.to_string(),
            }),
        active_revision: workflow.active_revision.map(|revision| revision.value()),
        stage: plan_stage_to_client(&workflow.stage),
        proposals: workflow
            .proposals
            .iter()
            .map(plan_proposal_to_client)
            .collect(),
        reviews: workflow.reviews.iter().map(plan_review_to_client).collect(),
        approvals: workflow
            .approvals
            .iter()
            .map(plan_approval_to_client)
            .collect(),
        handoffs: workflow
            .handoffs
            .iter()
            .map(plan_handoff_to_client)
            .collect(),
    }
}

fn plan_stage_to_client(stage: &PlanStage) -> ClientPlanStage {
    match stage {
        PlanStage::Idle => ClientPlanStage::Idle,
        PlanStage::QuestionPending { question } => ClientPlanStage::QuestionPending {
            question: question_to_client(question),
        },
        PlanStage::Proposed { proposal } => ClientPlanStage::Proposed {
            proposal: plan_proposal_to_client(proposal),
        },
        PlanStage::Reviewed { proposal, reviews } => ClientPlanStage::Reviewed {
            proposal: plan_proposal_to_client(proposal),
            reviews: reviews.iter().map(plan_review_to_client).collect(),
        },
        PlanStage::Approved { proposal, approval } => ClientPlanStage::Approved {
            proposal: plan_proposal_to_client(proposal),
            approval: plan_approval_to_client(approval),
        },
        PlanStage::IterateRequested { proposal, feedback } => ClientPlanStage::IterateRequested {
            proposal: plan_proposal_to_client(proposal),
            feedback: feedback.clone(),
        },
        PlanStage::Rejected { proposal, reason } => ClientPlanStage::Rejected {
            proposal: plan_proposal_to_client(proposal),
            reason: reason.clone(),
        },
        PlanStage::Handoff { proposal, handoff } => ClientPlanStage::Handoff {
            proposal: plan_proposal_to_client(proposal),
            handoff: plan_handoff_to_client(handoff),
        },
    }
}

fn question_to_client(question: &Question) -> ClientPlanQuestion {
    ClientPlanQuestion {
        id: question.id.to_string(),
        prompt: question.prompt.clone(),
        options: question.options.clone(),
        is_multi_select: question.is_multi_select,
        created_at: format_timestamp(question.created_at),
    }
}

fn answer_to_client(answer: &QuestionAnswer) -> ClientPlanAnswer {
    ClientPlanAnswer {
        question_id: answer.question_id.to_string(),
        selected_options: answer.selected_options.clone(),
        freeform_text: answer.freeform_text.clone(),
        answered_at: format_timestamp(answer.answered_at),
    }
}

fn prd_to_client(prd: &ProductRequirementsDocument) -> ClientProductRequirementsDocument {
    ClientProductRequirementsDocument {
        id: prd.id.to_string(),
        plan_id: prd.plan_id.to_string(),
        title: prd.title.clone(),
        problem: prd.problem.clone(),
        users: prd.users.clone(),
        requirements: prd.requirements.iter().map(requirement_to_client).collect(),
        acceptance_criteria: prd.acceptance_criteria.clone(),
        non_goals: prd.non_goals.clone(),
        constraints: prd.constraints.clone(),
        created_at: format_timestamp(prd.created_at),
    }
}

fn requirement_to_client(requirement: &PrdRequirement) -> ClientPrdRequirement {
    ClientPrdRequirement {
        id: requirement.id.clone(),
        title: requirement.title.clone(),
        description: requirement.description.clone(),
    }
}

fn plan_proposal_to_client(proposal: &PlanProposal) -> ClientPlanProposal {
    ClientPlanProposal {
        plan_id: proposal.plan_id.to_string(),
        revision_id: proposal.revision_id.value(),
        title: proposal.title.clone(),
        summary: proposal.summary.clone(),
        steps: proposal
            .steps
            .iter()
            .map(|step| crate::core::client::ClientPlanStep {
                index: step.index,
                title: step.title.clone(),
                description: step.description.clone(),
            })
            .collect(),
        proposed_at: format_timestamp(proposal.proposed_at),
    }
}

fn plan_review_to_client(review: &PlanReview) -> ClientPlanReview {
    ClientPlanReview {
        plan_id: review.plan_id.to_string(),
        revision_id: review.revision_id.value(),
        reviewer: review.reviewer.clone(),
        verdict: review.verdict.into(),
        feedback: review.feedback.clone(),
        reviewed_at: format_timestamp(review.reviewed_at),
    }
}

fn plan_approval_to_client(approval: &PlanApproval) -> ClientPlanApproval {
    ClientPlanApproval {
        plan_id: approval.plan_id.to_string(),
        revision_id: approval.revision_id.value(),
        approver: approval.approver.clone(),
        decision: approval.decision.into(),
        note: approval.note.clone(),
        approved_at: format_timestamp(approval.approved_at),
    }
}

fn plan_handoff_to_client(handoff: &PlanHandoff) -> ClientPlanHandoff {
    ClientPlanHandoff {
        plan_id: handoff.plan_id.to_string(),
        revision_id: handoff.revision_id.value(),
        handoff_note: handoff.handoff_note.clone(),
        created_at: format_timestamp(handoff.created_at),
    }
}

fn format_timestamp(timestamp: time::OffsetDateTime) -> String {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn persona_summary_to_client(summary: &PersonaSummary) -> ClientPersonaSummary {
    ClientPersonaSummary {
        name: summary.name.clone(),
        description: summary.description.clone(),
        scope: match summary.scope {
            SkillScope::Project => ClientPersonaScope::Project,
            SkillScope::User => ClientPersonaScope::User,
        },
    }
}

fn snapshot_sessions(snapshot: &RuntimeSnapshot) -> Vec<ClientSessionSummary> {
    snapshot
        .sessions
        .iter()
        .map(|s| ClientSessionSummary {
            id: s.id.to_string(),
            title: s.title.clone(),
            project_root: s.project_root.to_string_lossy().into_owned(),
            status: s.status.as_str().to_owned(),
            rollup_status: rollup_status_for(s),
            provider: s
                .provider
                .as_ref()
                .map(crate::core::model::ProviderId::as_str)
                .map(ToOwned::to_owned),
            model: s
                .model
                .as_ref()
                .map(crate::core::model::ModelId::as_str)
                .map(ToOwned::to_owned),
            updated_at: s.updated_at.to_string(),
            event_count: s.event_count,
            leased: s.leased,
            parent: s.parent.map(|id| id.to_string()),
        })
        .collect()
}

fn snapshot_personas(snapshot: &RuntimeSnapshot) -> Vec<ClientPersonaSummary> {
    snapshot
        .personas
        .iter()
        .map(persona_summary_to_client)
        .collect()
}

fn snapshot_routes(snapshot: &RuntimeSnapshot) -> Vec<ClientRoute> {
    snapshot.routes.iter().map(route_choice_to_client).collect()
}

fn route_choice_to_client(route: &RouteChoice) -> ClientRoute {
    ClientRoute {
        name: route.name.clone(),
        roles: route.roles.clone(),
        provider: route.provider.as_str().to_owned(),
        model: route.model.as_str().to_owned(),
        persona: route.persona.clone(),
    }
}

fn snapshot_accounts(snapshot: &RuntimeSnapshot) -> Vec<ClientAccount> {
    snapshot
        .providers
        .iter()
        .map(|connection| ClientAccount {
            provider: connection.provider.as_str().to_owned(),
            state: provider_connection_state_to_client(connection.state),
            detail: connection.detail.clone(),
        })
        .collect()
}

const fn provider_connection_state_to_client(
    state: ProviderConnectionState,
) -> ClientProviderConnectionState {
    match state {
        ProviderConnectionState::Disconnected => ClientProviderConnectionState::Disconnected,
        ProviderConnectionState::Discovering => ClientProviderConnectionState::Discovering,
        ProviderConnectionState::Connected => ClientProviderConnectionState::Connected,
        ProviderConnectionState::NeedsReauth => ClientProviderConnectionState::NeedsReauth,
        ProviderConnectionState::Unavailable => ClientProviderConnectionState::Unavailable,
    }
}

fn snapshot_quota(snapshot: &RuntimeSnapshot) -> Option<ClientQuota> {
    let model = snapshot
        .model
        .as_ref()
        .map(crate::core::model::ModelId::as_str);
    snapshot
        .quota
        .as_ref()
        .map(|quota| quota_to_client(quota, model))
}

fn quota_to_client(quota: &crate::core::model::QuotaSnapshot, model: Option<&str>) -> ClientQuota {
    ClientQuota {
        provider: quota.provider.as_str().to_owned(),
        windows: quota
            .windows
            .iter()
            .map(|window| ClientQuotaWindow {
                label: window.label.clone(),
                used_fraction: window.used_fraction,
                resets_at: window.resets_at.map(format_timestamp),
                is_relevant: model.is_some_and(|model| pool_covers_model(&window.label, model)),
            })
            .collect(),
    }
}

/// Mirrors `pool_covers_model` in `src/tui/chrome.rs`. Anthropic and Codex
/// windows (`"5h"`, `"7d"`, …) apply account-wide no matter which model is in
/// use, so they never match here and every window from those providers reads
/// as equally relevant to the frontend, which is correct — "worst of them" is
/// the right number for an account-wide window. Google's pools are split by
/// model family (`"gemini"`, `"claude/gpt"`, from `pool_label` in
/// `providers::gemini_cli`), so this lets the frontend prefer the pool that
/// actually covers the active model instead of showing an irrelevant one.
/// Duplicated as a literal match rather than shared with `chrome.rs` because
/// the TUI cannot depend on `providers` (AGENTS.md §2.1) and this keeps both
/// copies equally cheap to read, matching the codebase's existing choice.
fn pool_covers_model(label: &str, model: &str) -> bool {
    match label {
        "gemini" => model.contains("gemini"),
        "claude/gpt" => model.contains("claude") || model.contains("gpt"),
        _ => false,
    }
}

const fn tier_label(tier: ToolTier) -> &'static str {
    match tier {
        ToolTier::Read => "read",
        ToolTier::Write => "write",
        ToolTier::Execute => "execute",
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "exhaustive conversion across all MjolnrEvent variants"
)]
pub(super) fn event_to_client(event: &MjolnrEvent) -> Option<ClientEvent> {
    let mapped = match event {
        MjolnrEvent::SessionCreated {
            session,
            provider,
            model,
        } => ClientEvent::SessionStarted {
            session: session.to_string(),
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
        },
        MjolnrEvent::RunStarted { run, .. } => ClientEvent::RunStarted {
            run: run.to_string(),
        },
        MjolnrEvent::TextDelta { run, text, .. } => {
            let (text, text_truncated) = truncate_text(text, MAX_ACTIVITY_TEXT);
            ClientEvent::TextDelta {
                run: run.to_string(),
                text,
                text_truncated,
            }
        }
        MjolnrEvent::ReasoningDelta { run, text, .. } => {
            let (text, text_truncated) = truncate_text(text, MAX_ACTIVITY_TEXT);
            ClientEvent::ReasoningDelta {
                run: run.to_string(),
                text,
                text_truncated,
            }
        }
        MjolnrEvent::ToolAssembling { run, name, .. } => ClientEvent::ToolAssembling {
            run: run.to_string(),
            name: name.clone(),
        },
        MjolnrEvent::ToolProposed {
            run,
            approval,
            call,
            preview,
            ..
        } => ClientEvent::ToolProposed {
            run: run.to_string(),
            approval: approval.map(|id| id.to_string()),
            name: call.name.clone(),
            preview: preview.clone(),
        },
        MjolnrEvent::ApprovalResolved {
            run,
            approval,
            decision,
            ..
        } => ClientEvent::ApprovalResolved {
            run: run.to_string(),
            approval: approval.to_string(),
            decision: (*decision).into(),
        },
        MjolnrEvent::ToolCompleted {
            run, name, result, ..
        } => {
            let (outcome, reason_code) = outcome_to_client(&result.outcome);
            ClientEvent::ToolCompleted {
                run: run.to_string(),
                name: name.clone(),
                outcome,
                reason_code,
            }
        }
        MjolnrEvent::ToolFailed {
            run, name, code, ..
        } => ClientEvent::ToolCompleted {
            run: run.to_string(),
            name: name.clone(),
            outcome: ClientToolOutcome::Failed,
            reason_code: Some(code.as_str().to_owned()),
        },
        MjolnrEvent::RunFinished { run, reason, .. } => ClientEvent::RunFinished {
            run: run.to_string(),
            reason: (*reason).into(),
        },
        MjolnrEvent::RunFailed {
            run, code, detail, ..
        } => {
            let (detail, detail_truncated) = truncate_text(detail, MAX_ACTIVITY_TEXT);
            ClientEvent::RunFailed {
                run: run.to_string(),
                code: code.as_str().to_owned(),
                detail,
                detail_truncated,
            }
        }
        MjolnrEvent::PolicyChanged { mode, .. } => ClientEvent::PolicyChanged {
            policy: (*mode).into(),
        },
        MjolnrEvent::ModelChanged {
            provider, model, ..
        } => ClientEvent::ModelChanged {
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
        },
        MjolnrEvent::FileSaved {
            path,
            observed_digest,
            new_digest,
            size_bytes,
            ..
        } => ClientEvent::FileSaved {
            path: path.clone(),
            observed_digest: observed_digest.clone(),
            new_digest: new_digest.clone(),
            size_bytes: u32::try_from(*size_bytes).unwrap_or(u32::MAX),
        },
        MjolnrEvent::SubagentActivity { child, label, .. } => ClientEvent::SubagentActivity {
            child: child.to_string(),
            label: label.clone(),
        },
        MjolnrEvent::SubagentSpawned {
            child,
            directive,
            branch,
            worktree,
            ..
        } => {
            let (directive, directive_truncated) = truncate_text(directive, MAX_DIRECTIVE_TEXT);
            ClientEvent::SubagentSpawned {
                child: child.to_string(),
                directive,
                directive_truncated,
                branch: branch.clone(),
                worktree: worktree.clone(),
            }
        }
        MjolnrEvent::RecoveryRequired { work, .. } => ClientEvent::RecoveryRequired {
            work: Box::new(recovery_work_to_client(work)),
        },
        MjolnrEvent::RecoveryResolved { decision, .. } => ClientEvent::RecoveryResolved {
            decision: (*decision).into(),
        },
        MjolnrEvent::SessionEnded { .. } => ClientEvent::SessionEnded,
        MjolnrEvent::MessageAppended { .. }
        | MjolnrEvent::QuotaReported { .. }
        | MjolnrEvent::QuotaBoundaryReached { .. }
        | MjolnrEvent::HandoffCreated { .. }
        | MjolnrEvent::UsageReported { .. }
        | MjolnrEvent::PolicyClamped { .. }
        | MjolnrEvent::ExtensionLoaded { .. }
        | MjolnrEvent::BudgetExhausted { .. }
        | MjolnrEvent::ModelChangeRefused { .. }
        | MjolnrEvent::SpawnEnvelopeArmed { .. }
        | MjolnrEvent::SpawnEnvelopeDrawn { .. }
        | MjolnrEvent::SpawnEnvelopeCleared { .. }
        | MjolnrEvent::SubagentResultLate { .. }
        | MjolnrEvent::ReadSetCollision { .. }
        | MjolnrEvent::TriggerFired { .. }
        | MjolnrEvent::TriggerSettled { .. }
        | MjolnrEvent::TriggerSkipped { .. }
        | MjolnrEvent::TriggerQueued { .. }
        | MjolnrEvent::TriggerReplaced { .. }
        | MjolnrEvent::TriggerDisabled { .. }
        | MjolnrEvent::TriggerRearmed { .. }
        | MjolnrEvent::RouteSelected { .. }
        | MjolnrEvent::RouteAdvanced { .. }
        | MjolnrEvent::RouteExhausted { .. }
        | MjolnrEvent::BreakerStateChanged { .. }
        | MjolnrEvent::PlanQuestionAsked { .. }
        | MjolnrEvent::PlanQuestionAnswered { .. }
        | MjolnrEvent::PlanProposed { .. }
        | MjolnrEvent::PlanReviewed { .. }
        | MjolnrEvent::PlanApproved { .. }
        | MjolnrEvent::PlanHandoffCreated { .. }
        | MjolnrEvent::CouncilReviewed { .. }
        | MjolnrEvent::PlanInterviewStarted { .. }
        | MjolnrEvent::PlanPrdProposed { .. }
        | MjolnrEvent::CouncilFindingDispositionRecorded { .. }
        // Likewise the amendment: the draft reaches a client on the snapshot,
        // as `council.amendment`, so a late subscriber cannot hold a different
        // proposal from the one the runtime composed.
        | MjolnrEvent::CouncilAmendmentProposed { .. }
        // The review family's live state reaches a client on the snapshot, as
        // `reviewThreads`, for the reason the envelope's does: a subscriber
        // that joined late or lagged would otherwise hold a different set of
        // notes from the one the runtime has. There is no per-event delta to
        // reduce, so there is nothing to emit here.
        | MjolnrEvent::ReviewNoteRecorded { .. }
        | MjolnrEvent::ReviewCommentAdded { .. }
        | MjolnrEvent::ReviewRequestSent { .. }
        | MjolnrEvent::ReviewRequestAnswered { .. }
        // Board state (decision tickets + imported items) reaches a client on
        // the snapshot as the board projection, not as a per-event delta — a
        // projected board is a cross-session query, and a single event knows
        // nothing about the board it will land on.
        | MjolnrEvent::DecisionTicketOpened { .. }
        | MjolnrEvent::DecisionTicketResolved { .. }
        | MjolnrEvent::ImportedItemFetched { .. }
        | MjolnrEvent::ImportedItemRefreshed { .. }
        // Imported acts/comments (D6 step 5) are board history, projected the same
        // way: the board snapshot renders them, and no single event maps to a
        // client delta.
        | MjolnrEvent::ImportedActRecorded { .. }
        | MjolnrEvent::ImportedCommentRecorded { .. } => return None,
    };
    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::SessionId;
    use crate::core::store::SessionStatus;

    fn summary(status: SessionStatus, event_count: u64, leased: bool) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(),
            project_root: std::path::PathBuf::from("/tmp/mjolnr-rollup-test"),
            title: "rollup test".to_owned(),
            status,
            provider: None,
            model: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            event_count,
            last_checkpoint_sequence: None,
            leased,
            parent: None,
        }
    }

    /// The whole `SessionStatus` × `leased` × `event_count` matrix, one case
    /// per cell. D1's earlier fixtures asserted only `running`, which left the
    /// draft / completed derivations untested.
    #[test]
    fn rollup_status_covers_the_status_lease_count_matrix() {
        let cases = [
            (SessionStatus::Active, 0, true, ClientRollupStatus::Running),
            (SessionStatus::Active, 5, true, ClientRollupStatus::Running),
            (SessionStatus::Ended, 0, true, ClientRollupStatus::Running),
            (SessionStatus::Ended, 5, true, ClientRollupStatus::Running),
            (SessionStatus::Active, 0, false, ClientRollupStatus::Draft),
            (SessionStatus::Active, 5, false, ClientRollupStatus::Active),
            (
                SessionStatus::Ended,
                0,
                false,
                ClientRollupStatus::Completed,
            ),
            (
                SessionStatus::Ended,
                5,
                false,
                ClientRollupStatus::Completed,
            ),
        ];
        for (status, event_count, leased, expected) in cases {
            let actual = rollup_status_for(&summary(status, event_count, leased));
            assert_eq!(
                actual, expected,
                "status={status:?} events={event_count} leased={leased}"
            );
        }
    }

    #[test]
    fn rollup_status_wire_form_is_camel_case_and_round_trips() {
        for (variant, wire) in [
            (ClientRollupStatus::Running, "\"running\""),
            (ClientRollupStatus::Active, "\"active\""),
            (ClientRollupStatus::Draft, "\"draft\""),
            (ClientRollupStatus::Completed, "\"completed\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, wire);
            let parsed: ClientRollupStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    /// The D1 defect this types away: `archived` was consumed by the frontend
    /// but never produced by the runtime. With a closed enum the unknown value
    /// is refused at the wire instead of silently grouping wrong.
    #[test]
    fn rollup_status_refuses_values_the_runtime_cannot_produce() {
        assert!(serde_json::from_str::<ClientRollupStatus>("\"archived\"").is_err());
        assert!(serde_json::from_str::<ClientRollupStatus>("\"\"").is_err());
    }
}

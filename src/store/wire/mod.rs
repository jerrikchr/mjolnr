//! The persisted wire format.
//!
//! # Why mirror types instead of `#[derive(Serialize)]` on `core`
//!
//! Deriving serde on the canonical types would be a third of the code and would
//! make the database format an *accident* of whatever those types currently look
//! like. Renaming `SmedEvent::ToolProposed::preview` — a refactor with no
//! semantic content — would silently stop every stored session from loading, and
//! nothing in the diff would say so.
//!
//! Everything here is therefore an explicit mirror. The duplication is the
//! point: changing the database format requires editing a file whose only job is
//! the database format, and the round-trip tests fail the moment core and wire
//! disagree. This is the same split `providers/openai/wire.rs` makes for the
//! same reason — a wire contract smed does not control is not one it may leak
//! into its domain types, and a wire contract it *does* control is not one it
//! should let a refactor edit by accident.
//!
//!  states the rule directly: "persist explicit versioned wire
//! envelopes. Do not persist `Debug` output or use `Debug` formatting as a
//! serialization contract."
//!
//! # The format cannot express an ephemeral event
//!
//! There is no `TextDelta` variant in [`EventPayload`].  forbids one row
//! per token, and rather than relying on every call site checking
//! [`SmedEvent::is_durable`] first, the format simply has nowhere to put one:
//! [`encode`] returns [`WireError::Ephemeral`]. A rule enforced by a type does
//! not decay (`AGENTS.md` §2.4).

mod checkpoint;
mod enums;
mod message;
mod recovery;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::command::ApprovalId;
use crate::core::event::{RunId, SessionId, SmedEvent};
use crate::core::model::{ModelId, ProviderId};
use crate::store::wire::checkpoint::{CheckpointWire, HandoffWire, QuotaReserveWire};
use crate::store::wire::enums::{
    ApprovalDecisionWire, BreakerStateWire, EnvelopeEndWire, ExtensionLoadAuthorityWire,
    FinishReasonWire, OverlapPolicyWire, PolicyModeWire, ReasonCodeWire, RouteAdvanceConditionWire,
    RouteSelectionReasonWire, TriggerOutcomeWire, TriggerSourceKindWire, UsageWire,
};
use crate::store::wire::message::{MessageWire, ToolCallWire, ToolResultWire};
use crate::store::wire::recovery::{InterruptedWorkWire, RecoveryDecisionWire};

/// The version stamped into every `events.schema_version` and
/// `checkpoints.schema_version` column.
///
/// Distinct from the database's `user_version` (see `store/sqlite/schema.rs`):
/// that one describes the *tables*, this one describes the *payloads inside
/// them*. Phase 9 added full-auto policy/audit variants; version 6 adds the
/// Phase 13 subagent boundaries; version 7 added the Phase 14 trigger
/// lifecycle; version 8 adds the Phase 15 routing/breaker events; version 9
/// adds the Phase 17 `extension_loaded` event; version 10 adds the Phase 31
/// spawn-envelope events; version 11 adds the Phase 33 `policy_clamped`
/// event; version 12 distinguishes human-approved agent extension loads;
/// version 13 adds Phase A1 plan workflow events; version 15 adds the Phase D3
/// review-thread family; version 16 adds the D7 operator-controlled file-save
/// event; version 17 adds durable council review and disposition events;
/// version 18 adds council artifact identity and per-section finding labels;
/// version 19 adds the durable council-amendment event; version 20 adds Phase
/// E5 step 4b imported work-item events; version 21 adds Phase D6 step 5
/// durable imported-act records; version 22 adds the imported comment record;
/// version 23 adds the bounded interview and durable PRD events.
/// Conflating the payload version with the table version
/// would mean a payload change forced a table migration, or worse, did not.
pub(in crate::store) const WIRE_VERSION: u32 = 23;

/// Failures encoding or decoding a persisted payload.
#[derive(Debug, thiserror::Error)]
pub(in crate::store) enum WireError {
    /// An ephemeral event reached the store. A bug, not a data condition.
    #[error("refusing to persist an ephemeral event: {kind}")]
    Ephemeral { kind: &'static str },

    #[error("payload version {found} is newer than this build supports ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },

    #[error("{detail}")]
    Decode { detail: String },
}

/// One durable event's payload.
///
/// The session id is deliberately absent: it is the `events.session_id` column,
/// and storing it twice invites the two copies to disagree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::store) enum EventPayload {
    SessionCreated {
        provider: String,
        model: String,
    },
    MessageAppended {
        message: MessageWire,
    },
    RunStarted {
        run: Uuid,
    },
    UsageReported {
        run: Uuid,
        usage: UsageWire,
    },
    QuotaBoundaryReached {
        run: Uuid,
        reserve: QuotaReserveWire,
    },
    HandoffCreated {
        handoff: HandoffWire,
    },
    PolicyChanged {
        mode: PolicyModeWire,
    },
    PolicyClamped {
        from: PolicyModeWire,
        to: PolicyModeWire,
        provider: String,
        model: String,
        tier: enums::GovernanceTierWire,
    },
    ExtensionLoaded {
        name: String,
        program: String,
        by: ExtensionLoadAuthorityWire,
    },
    ToolProposed {
        run: Uuid,
        approval: Option<Uuid>,
        call: ToolCallWire,
        tier: enums::ToolTierWire,
        preview: String,
    },
    ApprovalResolved {
        run: Uuid,
        approval: Uuid,
        decision: ApprovalDecisionWire,
    },
    ToolCompleted {
        run: Uuid,
        call_id: String,
        name: String,
        result: ToolResultWire,
    },
    ToolFailed {
        run: Uuid,
        call_id: String,
        name: String,
        code: ReasonCodeWire,
        detail: String,
    },
    BudgetExhausted {
        run: Uuid,
    },
    RunFinished {
        run: Uuid,
        reason: FinishReasonWire,
    },
    RunFailed {
        run: Uuid,
        code: ReasonCodeWire,
        detail: String,
    },
    ModelChanged {
        provider: String,
        model: String,
    },
    ModelChangeRefused {
        provider: String,
        model: String,
        code: ReasonCodeWire,
        detail: String,
    },
    FileSaved {
        path: String,
        observed_digest: String,
        new_digest: String,
        size_bytes: u64,
    },
    SpawnEnvelopeArmed {
        ceiling: PolicyModeWire,
        max_children: u32,
        max_per_call: u32,
        max_provider_turns: u32,
        expires_after_turns: u32,
    },
    SpawnEnvelopeDrawn {
        run: Uuid,
        children: u32,
        provider_turns: u32,
        children_remaining: u32,
    },
    SpawnEnvelopeCleared {
        reason: EnvelopeEndWire,
    },
    SubagentSpawned {
        run: Uuid,
        child: Uuid,
        directive: String,
        policy: PolicyModeWire,
        branch: String,
        worktree: String,
    },
    SubagentResultLate {
        child: Uuid,
        detail: String,
    },
    ReadSetCollision {
        reader: Uuid,
        writer: Uuid,
        path: String,
    },
    RecoveryRequired {
        work: InterruptedWorkWire,
    },
    RecoveryResolved {
        decision: RecoveryDecisionWire,
    },
    SessionEnded,
    TriggerFired {
        trigger: String,
        child: Uuid,
        source: TriggerSourceKindWire,
    },
    TriggerSettled {
        trigger: String,
        child: Uuid,
        outcome: TriggerOutcomeWire,
        reason_code: Option<ReasonCodeWire>,
    },
    TriggerSkipped {
        trigger: String,
        overlap: OverlapPolicyWire,
        detail: String,
    },
    TriggerQueued {
        trigger: String,
    },
    TriggerReplaced {
        trigger: String,
        replaced_child: Uuid,
    },
    TriggerDisabled {
        trigger: String,
        code: ReasonCodeWire,
        consecutive_failures: u32,
    },
    TriggerRearmed {
        trigger: String,
    },
    RouteSelected {
        child: Option<Uuid>,
        route: String,
        position: u32,
        provider: String,
        model: String,
        reason: RouteSelectionReasonWire,
    },
    RouteAdvanced {
        run: Uuid,
        route: String,
        from_position: u32,
        to_position: u32,
        provider: String,
        model: String,
        condition: RouteAdvanceConditionWire,
    },
    RouteExhausted {
        run: Uuid,
        route: String,
        condition: RouteAdvanceConditionWire,
    },
    BreakerStateChanged {
        provider: String,
        from: BreakerStateWire,
        to: BreakerStateWire,
    },
    PlanInterviewStarted {
        plan_id: crate::core::plan::PlanId,
        goal: String,
    },
    PlanQuestionAsked {
        plan_id: crate::core::plan::PlanId,
        question: crate::core::plan::Question,
    },
    PlanQuestionAnswered {
        plan_id: crate::core::plan::PlanId,
        answer: crate::core::plan::QuestionAnswer,
    },
    PlanPrdProposed {
        prd: crate::core::plan::ProductRequirementsDocument,
    },
    PlanProposed {
        proposal: crate::core::plan::PlanProposal,
    },
    PlanReviewed {
        review: crate::core::plan::PlanReview,
    },
    PlanApproved {
        approval: crate::core::plan::PlanApproval,
    },
    PlanHandoffCreated {
        handoff: crate::core::plan::PlanHandoff,
    },
    CouncilReviewed {
        review: crate::core::council::CouncilReview,
    },
    CouncilFindingDispositionRecorded {
        disposition: crate::core::council::CouncilFindingDisposition,
    },
    CouncilAmendmentProposed {
        amendment: crate::core::council::CouncilAmendment,
    },
    // The review family  carries `core::review` types directly,
    // as the plan family above carries `core::plan` types. That is the narrow
    // exception this module's header allows: these types exist *because* they
    // are a durable record, they derive serde for that purpose alone, and no
    // client wire depends on them — `core::client::workspace` projects a
    // separate DTO. A mirror here would duplicate five structs that have no
    // second reader.
    ReviewNoteRecorded {
        thread: crate::core::review::ReviewThreadId,
        anchor: crate::core::review::ReviewAnchor,
        comment: crate::core::review::ReviewComment,
    },
    ReviewCommentAdded {
        thread: crate::core::review::ReviewThreadId,
        comment: crate::core::review::ReviewComment,
    },
    ReviewRequestSent {
        threads: Vec<crate::core::review::ReviewThreadId>,
        run: Uuid,
    },
    ReviewRequestAnswered {
        threads: Vec<crate::core::review::ReviewThreadId>,
        response_message: Uuid,
    },
    // The decision-ticket family (Phase E5) carries `core::board` types
    // directly, under the same narrow exception the plan/council/review
    // families use: they exist to be the durable record, and no client wire
    // depends on them.
    DecisionTicketOpened {
        ticket: crate::core::board::DecisionTicket,
    },
    DecisionTicketResolved {
        resolution: crate::core::board::DecisionResolution,
    },
    // Imported work items (Phase E5, step 4b) carry `core::imported` types
    // directly, under the same narrow exception: they are the durable record
    // of an external fetch, and no second reader exists.
    ImportedItemFetched {
        item: crate::core::imported::ImportedItem,
    },
    ImportedItemRefreshed {
        expected_revision: String,
        item: crate::core::imported::ImportedItem,
    },
    // D6 step 5 act records (phase D6, step 5) carry the `core::imported` act
    // type directly — a submit closed the clock on an external tracker and the
    // record is the story. Same narrow exception as the imported item events.
    ImportedActRecorded {
        act: crate::core::imported::ImportedAct,
    },
    ImportedCommentRecorded {
        item_id: crate::core::imported::ImportedItemId,
        comment_id: String,
        body: String,
    },
}

/// The `events.kind` values whose events append a transcript message.
///
/// The SQL spelling of [`SmedEvent::introduces_message`]. Recovery re-anchors
/// a checkpoint's transcript by counting these below the checkpoint's extent,
/// so this list and that predicate must name the same events — a test pins
/// them together, because a one-variant disagreement would anchor every entry
/// after it to the wrong event.
pub(in crate::store) const MESSAGE_BEARING_KINDS: [&str; 3] =
    ["message_appended", "tool_completed", "tool_failed"];

impl EventPayload {
    /// The value for the `events.kind` column.
    ///
    /// Derived from the payload rather than passed alongside it, so the column
    /// and the payload cannot drift.  requires the column; this keeps it
    /// honest.
    pub(in crate::store) const fn kind(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session_created",
            Self::MessageAppended { .. } => "message_appended",
            Self::RunStarted { .. } => "run_started",
            Self::UsageReported { .. } => "usage_reported",
            Self::QuotaBoundaryReached { .. } => "quota_boundary_reached",
            Self::HandoffCreated { .. } => "handoff_created",
            Self::PolicyChanged { .. } => "policy_changed",
            Self::PolicyClamped { .. } => "policy_clamped",
            Self::ExtensionLoaded { .. } => "extension_loaded",
            Self::ToolProposed { .. } => "tool_proposed",
            Self::ApprovalResolved { .. } => "approval_resolved",
            Self::ToolCompleted { .. } => "tool_completed",
            Self::ToolFailed { .. } => "tool_failed",
            Self::BudgetExhausted { .. } => "budget_exhausted",
            Self::RunFinished { .. } => "run_finished",
            Self::RunFailed { .. } => "run_failed",
            Self::ModelChanged { .. } => "model_changed",
            Self::ModelChangeRefused { .. } => "model_change_refused",
            Self::FileSaved { .. } => "file_saved",
            Self::SpawnEnvelopeArmed { .. } => "spawn_envelope_armed",
            Self::SpawnEnvelopeDrawn { .. } => "spawn_envelope_drawn",
            Self::SpawnEnvelopeCleared { .. } => "spawn_envelope_cleared",
            Self::SubagentSpawned { .. } => "subagent_spawned",
            Self::SubagentResultLate { .. } => "subagent_result_late",
            Self::ReadSetCollision { .. } => "read_set_collision",
            Self::RecoveryRequired { .. } => "recovery_required",
            Self::RecoveryResolved { .. } => "recovery_resolved",
            Self::SessionEnded => "session_ended",
            Self::TriggerFired { .. } => "trigger_fired",
            Self::TriggerSettled { .. } => "trigger_settled",
            Self::TriggerSkipped { .. } => "trigger_skipped",
            Self::TriggerQueued { .. } => "trigger_queued",
            Self::TriggerReplaced { .. } => "trigger_replaced",
            Self::TriggerDisabled { .. } => "trigger_disabled",
            Self::TriggerRearmed { .. } => "trigger_rearmed",
            Self::RouteSelected { .. } => "route_selected",
            Self::RouteAdvanced { .. } => "route_advanced",
            Self::RouteExhausted { .. } => "route_exhausted",
            Self::BreakerStateChanged { .. } => "breaker_state_changed",
            Self::PlanInterviewStarted { .. } => "plan_interview_started",
            Self::PlanQuestionAsked { .. } => "plan_question_asked",
            Self::PlanQuestionAnswered { .. } => "plan_question_answered",
            Self::PlanPrdProposed { .. } => "plan_prd_proposed",
            Self::PlanProposed { .. } => "plan_proposed",
            Self::PlanReviewed { .. } => "plan_reviewed",
            Self::PlanApproved { .. } => "plan_approved",
            Self::PlanHandoffCreated { .. } => "plan_handoff_created",
            Self::CouncilReviewed { .. } => "council_reviewed",
            Self::CouncilAmendmentProposed { .. } => "council_amendment_proposed",
            Self::CouncilFindingDispositionRecorded { .. } => {
                "council_finding_disposition_recorded"
            }
            Self::ReviewNoteRecorded { .. } => "review_note_recorded",
            Self::ReviewCommentAdded { .. } => "review_comment_added",
            Self::ReviewRequestSent { .. } => "review_request_sent",
            Self::ReviewRequestAnswered { .. } => "review_request_answered",
            Self::DecisionTicketOpened { .. } => "decision_ticket_opened",
            Self::DecisionTicketResolved { .. } => "decision_ticket_resolved",
            Self::ImportedItemFetched { .. } => "imported_item_fetched",
            Self::ImportedItemRefreshed { .. } => "imported_item_refreshed",
            Self::ImportedActRecorded { .. } => "imported_act_recorded",
            Self::ImportedCommentRecorded { .. } => "imported_comment_recorded",
        }
    }
}

/// Turn a durable event into its persisted payload.
///
/// # Errors
/// [`WireError::Ephemeral`] for a `TextDelta`, which has no persisted form.
#[allow(
    clippy::too_many_lines,
    reason = "one flat event-to-payload mapping; it grows by one arm per durable event and splitting it would hide which events are covered"
)]
pub(in crate::store) fn encode(event: SmedEvent) -> Result<EventPayload, WireError> {
    let payload = match event {
        ref event @ (SmedEvent::TextDelta { .. }
        | SmedEvent::ReasoningDelta { .. }
        | SmedEvent::ToolAssembling { .. }
        | SmedEvent::QuotaReported { .. }
        | SmedEvent::SubagentActivity { .. }) => {
            return Err(WireError::Ephemeral {
                kind: ephemeral_kind(event),
            });
        }
        event @ (SmedEvent::SessionCreated { .. } | SmedEvent::MessageAppended { .. }) => {
            encode_session_event(event)?
        }
        SmedEvent::RunStarted { run, .. } => EventPayload::RunStarted { run: run.as_uuid() },
        event @ (SmedEvent::UsageReported { .. }
        | SmedEvent::QuotaBoundaryReached { .. }
        | SmedEvent::HandoffCreated { .. }) => encode_continuation_event(event)?,
        SmedEvent::PolicyChanged { mode, .. } => EventPayload::PolicyChanged { mode: mode.into() },
        SmedEvent::PolicyClamped {
            from,
            to,
            provider,
            model,
            tier,
            ..
        } => EventPayload::PolicyClamped {
            from: from.into(),
            to: to.into(),
            provider: provider.to_string(),
            model: model.to_string(),
            tier: tier.into(),
        },
        SmedEvent::ExtensionLoaded {
            name, program, by, ..
        } => EventPayload::ExtensionLoaded {
            name,
            program,
            by: by.into(),
        },
        event @ (SmedEvent::ToolProposed { .. }
        | SmedEvent::ApprovalResolved { .. }
        | SmedEvent::ToolCompleted { .. }
        | SmedEvent::ToolFailed { .. }) => encode_tool_event(event)?,
        SmedEvent::BudgetExhausted { run, .. } => {
            EventPayload::BudgetExhausted { run: run.as_uuid() }
        }
        SmedEvent::RunFinished { run, reason, .. } => EventPayload::RunFinished {
            run: run.as_uuid(),
            reason: reason.into(),
        },
        SmedEvent::RunFailed {
            run, code, detail, ..
        } => EventPayload::RunFailed {
            run: run.as_uuid(),
            code: ReasonCodeWire(code),
            detail,
        },
        event @ (SmedEvent::ModelChanged { .. } | SmedEvent::ModelChangeRefused { .. }) => {
            encode_model_event(event)?
        }
        SmedEvent::FileSaved {
            path,
            observed_digest,
            new_digest,
            size_bytes,
            ..
        } => EventPayload::FileSaved {
            path,
            observed_digest,
            new_digest,
            size_bytes,
        },
        SmedEvent::SpawnEnvelopeArmed {
            ceiling,
            max_children,
            max_per_call,
            max_provider_turns,
            expires_after_turns,
            ..
        } => EventPayload::SpawnEnvelopeArmed {
            ceiling: ceiling.into(),
            max_children,
            max_per_call,
            max_provider_turns,
            expires_after_turns,
        },
        SmedEvent::SpawnEnvelopeDrawn {
            run,
            children,
            provider_turns,
            children_remaining,
            ..
        } => EventPayload::SpawnEnvelopeDrawn {
            run: run.as_uuid(),
            children,
            provider_turns,
            children_remaining,
        },
        SmedEvent::SpawnEnvelopeCleared { reason, .. } => EventPayload::SpawnEnvelopeCleared {
            reason: reason.into(),
        },
        event @ (SmedEvent::SubagentSpawned { .. }
        | SmedEvent::SubagentResultLate { .. }
        | SmedEvent::ReadSetCollision { .. }) => encode_subagent_event(event)?,
        SmedEvent::RecoveryRequired { work, .. } => EventPayload::RecoveryRequired {
            work: (*work).into(),
        },
        SmedEvent::RecoveryResolved { decision, .. } => EventPayload::RecoveryResolved {
            decision: decision.into(),
        },
        SmedEvent::SessionEnded { .. } => EventPayload::SessionEnded,
        event @ (SmedEvent::TriggerFired { .. }
        | SmedEvent::TriggerSettled { .. }
        | SmedEvent::TriggerSkipped { .. }
        | SmedEvent::TriggerQueued { .. }
        | SmedEvent::TriggerReplaced { .. }
        | SmedEvent::TriggerDisabled { .. }
        | SmedEvent::TriggerRearmed { .. }) => encode_trigger_event(event)?,
        event @ (SmedEvent::RouteSelected { .. }
        | SmedEvent::RouteAdvanced { .. }
        | SmedEvent::RouteExhausted { .. }
        | SmedEvent::BreakerStateChanged { .. }) => encode_routing_event(event)?,
        SmedEvent::PlanInterviewStarted { plan_id, goal, .. } => {
            EventPayload::PlanInterviewStarted { plan_id, goal }
        }
        SmedEvent::PlanQuestionAsked {
            plan_id, question, ..
        } => EventPayload::PlanQuestionAsked { plan_id, question },
        SmedEvent::PlanQuestionAnswered {
            plan_id, answer, ..
        } => EventPayload::PlanQuestionAnswered { plan_id, answer },
        SmedEvent::PlanPrdProposed { prd, .. } => EventPayload::PlanPrdProposed { prd },
        SmedEvent::PlanProposed { proposal, .. } => EventPayload::PlanProposed { proposal },
        SmedEvent::PlanReviewed { review, .. } => EventPayload::PlanReviewed { review },
        SmedEvent::PlanApproved { approval, .. } => EventPayload::PlanApproved { approval },
        SmedEvent::PlanHandoffCreated { handoff, .. } => {
            EventPayload::PlanHandoffCreated { handoff }
        }
        SmedEvent::CouncilReviewed { review, .. } => {
            EventPayload::CouncilReviewed { review: *review }
        }
        SmedEvent::CouncilFindingDispositionRecorded { disposition, .. } => {
            EventPayload::CouncilFindingDispositionRecorded { disposition }
        }
        SmedEvent::CouncilAmendmentProposed { amendment, .. } => {
            EventPayload::CouncilAmendmentProposed {
                amendment: *amendment,
            }
        }
        SmedEvent::ReviewNoteRecorded {
            thread,
            anchor,
            comment,
            ..
        } => EventPayload::ReviewNoteRecorded {
            thread,
            anchor,
            comment,
        },
        SmedEvent::ReviewCommentAdded {
            thread, comment, ..
        } => EventPayload::ReviewCommentAdded { thread, comment },
        SmedEvent::ReviewRequestSent { threads, run, .. } => EventPayload::ReviewRequestSent {
            threads,
            run: run.as_uuid(),
        },
        SmedEvent::ReviewRequestAnswered {
            threads,
            response_message,
            ..
        } => EventPayload::ReviewRequestAnswered {
            threads,
            response_message,
        },
        SmedEvent::DecisionTicketOpened { ticket, .. } => {
            EventPayload::DecisionTicketOpened { ticket }
        }
        SmedEvent::DecisionTicketResolved { resolution, .. } => {
            EventPayload::DecisionTicketResolved { resolution }
        }
        SmedEvent::ImportedItemFetched { item, .. } => EventPayload::ImportedItemFetched { item },
        SmedEvent::ImportedItemRefreshed {
            expected_revision,
            item,
            ..
        } => EventPayload::ImportedItemRefreshed {
            expected_revision,
            item,
        },
        SmedEvent::ImportedActRecorded { act, .. } => EventPayload::ImportedActRecorded { act },
        SmedEvent::ImportedCommentRecorded {
            item_id,
            comment_id,
            body,
            ..
        } => EventPayload::ImportedCommentRecorded {
            item_id,
            comment_id,
            body,
        },
    };
    Ok(payload)
}

fn encode_routing_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::RouteSelected {
            child,
            route,
            position,
            provider,
            model,
            reason,
            ..
        } => Ok(EventPayload::RouteSelected {
            child: child.map(|child| child.as_uuid()),
            route,
            position: u32::try_from(position).unwrap_or(u32::MAX),
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
            reason: reason.into(),
        }),
        SmedEvent::RouteAdvanced {
            run,
            route,
            from_position,
            to_position,
            provider,
            model,
            condition,
            ..
        } => Ok(EventPayload::RouteAdvanced {
            run: run.as_uuid(),
            route,
            from_position: u32::try_from(from_position).unwrap_or(u32::MAX),
            to_position: u32::try_from(to_position).unwrap_or(u32::MAX),
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
            condition: condition.into(),
        }),
        SmedEvent::RouteExhausted {
            run,
            route,
            condition,
            ..
        } => Ok(EventPayload::RouteExhausted {
            run: run.as_uuid(),
            route,
            condition: condition.into(),
        }),
        SmedEvent::BreakerStateChanged {
            provider, from, to, ..
        } => Ok(EventPayload::BreakerStateChanged {
            provider: provider.as_str().to_owned(),
            from: from.into(),
            to: to.into(),
        }),
        _ => Err(WireError::Decode {
            detail: "event is not routing state".to_owned(),
        }),
    }
}

fn encode_trigger_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::TriggerFired {
            trigger,
            child,
            source,
            ..
        } => Ok(EventPayload::TriggerFired {
            trigger,
            child: child.as_uuid(),
            source: source.into(),
        }),
        SmedEvent::TriggerSettled {
            trigger,
            child,
            outcome,
            reason_code,
            ..
        } => Ok(EventPayload::TriggerSettled {
            trigger,
            child: child.as_uuid(),
            outcome: outcome.into(),
            reason_code: reason_code.map(ReasonCodeWire),
        }),
        SmedEvent::TriggerSkipped {
            trigger,
            overlap,
            detail,
            ..
        } => Ok(EventPayload::TriggerSkipped {
            trigger,
            overlap: overlap.into(),
            detail,
        }),
        SmedEvent::TriggerQueued { trigger, .. } => Ok(EventPayload::TriggerQueued { trigger }),
        SmedEvent::TriggerReplaced {
            trigger,
            replaced_child,
            ..
        } => Ok(EventPayload::TriggerReplaced {
            trigger,
            replaced_child: replaced_child.as_uuid(),
        }),
        SmedEvent::TriggerDisabled {
            trigger,
            code,
            consecutive_failures,
            ..
        } => Ok(EventPayload::TriggerDisabled {
            trigger,
            code: ReasonCodeWire(code),
            consecutive_failures,
        }),
        SmedEvent::TriggerRearmed { trigger, .. } => Ok(EventPayload::TriggerRearmed { trigger }),
        _ => Err(WireError::Decode {
            detail: "event is not trigger state".to_owned(),
        }),
    }
}

fn encode_tool_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::ToolProposed {
            run,
            approval,
            call,
            tier,
            preview,
            ..
        } => Ok(EventPayload::ToolProposed {
            run: run.as_uuid(),
            approval: approval.map(ApprovalId::as_uuid),
            call: call.into(),
            tier: tier.into(),
            preview,
        }),
        SmedEvent::ApprovalResolved {
            run,
            approval,
            decision,
            ..
        } => Ok(EventPayload::ApprovalResolved {
            run: run.as_uuid(),
            approval: approval.as_uuid(),
            decision: decision.into(),
        }),
        SmedEvent::ToolCompleted {
            run,
            call_id,
            name,
            result,
            ..
        } => Ok(EventPayload::ToolCompleted {
            run: run.as_uuid(),
            call_id,
            name,
            result: result.into(),
        }),
        SmedEvent::ToolFailed {
            run,
            call_id,
            name,
            code,
            detail,
            ..
        } => Ok(EventPayload::ToolFailed {
            run: run.as_uuid(),
            call_id,
            name,
            code: ReasonCodeWire(code),
            detail,
        }),
        _ => Err(WireError::Decode {
            detail: "event is not tool state".to_owned(),
        }),
    }
}

fn encode_session_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::SessionCreated {
            provider, model, ..
        } => Ok(EventPayload::SessionCreated {
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
        }),
        SmedEvent::MessageAppended { message, .. } => Ok(EventPayload::MessageAppended {
            message: (*message).into(),
        }),
        _ => Err(WireError::Decode {
            detail: "event is not session state".to_owned(),
        }),
    }
}

fn encode_subagent_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::SubagentSpawned {
            run,
            child,
            directive,
            policy,
            branch,
            worktree,
            ..
        } => Ok(EventPayload::SubagentSpawned {
            run: run.as_uuid(),
            child: child.as_uuid(),
            directive,
            policy: policy.into(),
            branch,
            worktree,
        }),
        SmedEvent::SubagentResultLate { child, detail, .. } => {
            Ok(EventPayload::SubagentResultLate {
                child: child.as_uuid(),
                detail,
            })
        }
        SmedEvent::ReadSetCollision {
            reader,
            writer,
            path,
            ..
        } => Ok(EventPayload::ReadSetCollision {
            reader: reader.as_uuid(),
            writer: writer.as_uuid(),
            path,
        }),
        _ => Err(WireError::Decode {
            detail: "event is not subagent state".to_owned(),
        }),
    }
}

fn encode_continuation_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::UsageReported { run, usage, .. } => Ok(EventPayload::UsageReported {
            run: run.as_uuid(),
            usage: usage.into(),
        }),
        SmedEvent::QuotaBoundaryReached { run, reserve, .. } => {
            Ok(EventPayload::QuotaBoundaryReached {
                run: run.as_uuid(),
                reserve: reserve.into(),
            })
        }
        SmedEvent::HandoffCreated { handoff, .. } => Ok(EventPayload::HandoffCreated {
            handoff: (*handoff).into(),
        }),
        _ => Err(WireError::Decode {
            detail: "event is not continuation state".to_owned(),
        }),
    }
}

fn ephemeral_kind(event: &SmedEvent) -> &'static str {
    match event {
        SmedEvent::TextDelta { .. } => "text_delta",
        SmedEvent::ReasoningDelta { .. } => "reasoning_delta",
        SmedEvent::ToolAssembling { .. } => "tool_assembling",
        SmedEvent::QuotaReported { .. } => "quota_reported",
        SmedEvent::SubagentActivity { .. } => "subagent_activity",
        _ => "ephemeral",
    }
}

fn encode_model_event(event: SmedEvent) -> Result<EventPayload, WireError> {
    match event {
        SmedEvent::ModelChanged {
            provider, model, ..
        } => Ok(EventPayload::ModelChanged {
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
        }),
        SmedEvent::ModelChangeRefused {
            provider,
            model,
            code,
            detail,
            ..
        } => Ok(EventPayload::ModelChangeRefused {
            provider: provider.as_str().to_owned(),
            model: model.as_str().to_owned(),
            code: ReasonCodeWire(code),
            detail,
        }),
        _ => Err(WireError::Decode {
            detail: "event is not a model event".to_owned(),
        }),
    }
}

/// Rebuild a durable event from its persisted payload and its session column.
pub(in crate::store) fn decode(session: SessionId, payload: EventPayload) -> SmedEvent {
    // Split only to stay under the function-length limit; the two halves are one
    // flat mapping. Tool payloads are the larger group, so they move out.
    match payload {
        EventPayload::ToolProposed { .. }
        | EventPayload::ApprovalResolved { .. }
        | EventPayload::ToolCompleted { .. }
        | EventPayload::ToolFailed { .. } => decode_tool(session, payload),
        EventPayload::RouteSelected { .. }
        | EventPayload::RouteAdvanced { .. }
        | EventPayload::RouteExhausted { .. }
        | EventPayload::BreakerStateChanged { .. } => decode_routing(session, payload),
        other => decode_lifecycle(session, other),
    }
}

/// Route selection, advance, exhaustion, and breaker payloads.
fn decode_routing(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::RouteSelected {
            child,
            route,
            position,
            provider,
            model,
            reason,
        } => SmedEvent::RouteSelected {
            session,
            child: child.map(SessionId::from_uuid),
            route,
            position: position as usize,
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
            reason: reason.into(),
        },
        EventPayload::RouteAdvanced {
            run,
            route,
            from_position,
            to_position,
            provider,
            model,
            condition,
        } => SmedEvent::RouteAdvanced {
            session,
            run: RunId::from_uuid(run),
            route,
            from_position: from_position as usize,
            to_position: to_position as usize,
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
            condition: condition.into(),
        },
        EventPayload::RouteExhausted {
            run,
            route,
            condition,
        } => SmedEvent::RouteExhausted {
            session,
            run: RunId::from_uuid(run),
            route,
            condition: condition.into(),
        },
        EventPayload::BreakerStateChanged { provider, from, to } => {
            SmedEvent::BreakerStateChanged {
                session,
                provider: ProviderId::new(provider),
                from: from.into(),
                to: to.into(),
            }
        }
        // `decode` only calls this for a routing-shaped payload.
        other => decode_lifecycle(session, other),
    }
}

/// Session, run, and recovery payloads.
#[allow(
    clippy::too_many_lines,
    reason = "the inverse of `encode`'s flat mapping, and kept flat for the same reason"
)]
fn decode_lifecycle(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::SessionCreated { provider, model } => SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        },
        EventPayload::MessageAppended { message } => SmedEvent::MessageAppended {
            session,
            message: Box::new(message.into()),
        },
        EventPayload::RunStarted { run } => SmedEvent::RunStarted {
            session,
            run: RunId::from_uuid(run),
        },
        EventPayload::UsageReported { run, usage } => SmedEvent::UsageReported {
            session,
            run: RunId::from_uuid(run),
            usage: usage.into(),
        },
        EventPayload::QuotaBoundaryReached { run, reserve } => SmedEvent::QuotaBoundaryReached {
            session,
            run: RunId::from_uuid(run),
            reserve: reserve.into(),
        },
        EventPayload::HandoffCreated { handoff } => SmedEvent::HandoffCreated {
            session,
            handoff: Box::new(handoff.into()),
        },
        EventPayload::PolicyChanged { mode } => SmedEvent::PolicyChanged {
            session,
            mode: mode.into(),
        },
        EventPayload::PolicyClamped {
            from,
            to,
            provider,
            model,
            tier,
        } => SmedEvent::PolicyClamped {
            session,
            from: from.into(),
            to: to.into(),
            provider: crate::core::model::ProviderId::new(provider),
            model: crate::core::model::ModelId::new(model),
            tier: tier.into(),
        },
        EventPayload::ExtensionLoaded { name, program, by } => SmedEvent::ExtensionLoaded {
            session,
            name,
            program,
            by: by.into(),
        },
        EventPayload::BudgetExhausted { run } => SmedEvent::BudgetExhausted {
            session,
            run: RunId::from_uuid(run),
        },
        EventPayload::RunFinished { run, reason } => SmedEvent::RunFinished {
            session,
            run: RunId::from_uuid(run),
            reason: reason.into(),
        },
        EventPayload::RunFailed { run, code, detail } => SmedEvent::RunFailed {
            session,
            run: RunId::from_uuid(run),
            code: code.0,
            detail,
        },
        EventPayload::ModelChanged { provider, model } => SmedEvent::ModelChanged {
            session,
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        },
        EventPayload::ModelChangeRefused {
            provider,
            model,
            code,
            detail,
        } => SmedEvent::ModelChangeRefused {
            session,
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
            code: code.0,
            detail,
        },
        EventPayload::FileSaved {
            path,
            observed_digest,
            new_digest,
            size_bytes,
        } => SmedEvent::FileSaved {
            session,
            path,
            observed_digest,
            new_digest,
            size_bytes,
        },
        payload @ (EventPayload::SpawnEnvelopeArmed { .. }
        | EventPayload::SpawnEnvelopeDrawn { .. }
        | EventPayload::SpawnEnvelopeCleared { .. }) => decode_envelope(session, payload),
        payload @ (EventPayload::SubagentSpawned { .. }
        | EventPayload::SubagentResultLate { .. }
        | EventPayload::ReadSetCollision { .. }) => decode_subagent(session, payload),
        EventPayload::RecoveryRequired { work } => SmedEvent::RecoveryRequired {
            session,
            work: Box::new(work.into()),
        },
        EventPayload::RecoveryResolved { decision } => SmedEvent::RecoveryResolved {
            session,
            decision: decision.into(),
        },
        EventPayload::SessionEnded => SmedEvent::SessionEnded { session },
        payload @ (EventPayload::TriggerFired { .. }
        | EventPayload::TriggerSettled { .. }
        | EventPayload::TriggerSkipped { .. }
        | EventPayload::TriggerQueued { .. }
        | EventPayload::TriggerReplaced { .. }
        | EventPayload::TriggerDisabled { .. }
        | EventPayload::TriggerRearmed { .. }) => decode_trigger(session, payload),
        // Routed to `decode_tool`/`decode_routing` by `decode`.
        payload @ (EventPayload::ToolProposed { .. }
        | EventPayload::ApprovalResolved { .. }
        | EventPayload::ToolCompleted { .. }
        | EventPayload::ToolFailed { .. }) => decode_tool(session, payload),
        payload @ (EventPayload::RouteSelected { .. }
        | EventPayload::RouteAdvanced { .. }
        | EventPayload::RouteExhausted { .. }
        | EventPayload::BreakerStateChanged { .. }) => decode_routing(session, payload),
        EventPayload::PlanInterviewStarted { plan_id, goal } => SmedEvent::PlanInterviewStarted {
            session,
            plan_id,
            goal,
        },
        EventPayload::PlanQuestionAsked { plan_id, question } => SmedEvent::PlanQuestionAsked {
            session,
            plan_id,
            question,
        },
        EventPayload::PlanQuestionAnswered { plan_id, answer } => SmedEvent::PlanQuestionAnswered {
            session,
            plan_id,
            answer,
        },
        EventPayload::PlanPrdProposed { prd } => SmedEvent::PlanPrdProposed { session, prd },
        EventPayload::PlanProposed { proposal } => SmedEvent::PlanProposed { session, proposal },
        EventPayload::PlanReviewed { review } => SmedEvent::PlanReviewed { session, review },
        EventPayload::PlanApproved { approval } => SmedEvent::PlanApproved { session, approval },
        EventPayload::PlanHandoffCreated { handoff } => {
            SmedEvent::PlanHandoffCreated { session, handoff }
        }
        EventPayload::CouncilReviewed { review } => SmedEvent::CouncilReviewed {
            session,
            review: Box::new(review),
        },
        EventPayload::CouncilFindingDispositionRecorded { disposition } => {
            SmedEvent::CouncilFindingDispositionRecorded {
                session,
                disposition,
            }
        }
        EventPayload::CouncilAmendmentProposed { amendment } => {
            SmedEvent::CouncilAmendmentProposed {
                session,
                amendment: Box::new(amendment),
            }
        }
        EventPayload::ReviewNoteRecorded {
            thread,
            anchor,
            comment,
        } => SmedEvent::ReviewNoteRecorded {
            session,
            thread,
            anchor,
            comment,
        },
        EventPayload::ReviewCommentAdded { thread, comment } => SmedEvent::ReviewCommentAdded {
            session,
            thread,
            comment,
        },
        EventPayload::ReviewRequestSent { threads, run } => SmedEvent::ReviewRequestSent {
            session,
            threads,
            run: RunId::from_uuid(run),
        },
        EventPayload::ReviewRequestAnswered {
            threads,
            response_message,
        } => SmedEvent::ReviewRequestAnswered {
            session,
            threads,
            response_message,
        },
        EventPayload::DecisionTicketOpened { ticket } => {
            SmedEvent::DecisionTicketOpened { session, ticket }
        }
        EventPayload::DecisionTicketResolved { resolution } => SmedEvent::DecisionTicketResolved {
            session,
            resolution,
        },
        EventPayload::ImportedItemFetched { item } => {
            SmedEvent::ImportedItemFetched { session, item }
        }
        EventPayload::ImportedItemRefreshed {
            expected_revision,
            item,
        } => SmedEvent::ImportedItemRefreshed {
            session,
            expected_revision,
            item,
        },
        EventPayload::ImportedActRecorded { act } => {
            SmedEvent::ImportedActRecorded { session, act }
        }
        EventPayload::ImportedCommentRecorded {
            item_id,
            comment_id,
            body,
        } => SmedEvent::ImportedCommentRecorded {
            session,
            item_id,
            comment_id,
            body,
        },
    }
}

/// Subagent boundary payloads.
/// Decode the spawn-envelope events.
///
/// Split out for the same reason `decode_subagent` is: `decode_lifecycle` is a
/// flat mapping and stays readable only while no one family of events grows a
/// long arm inside it.
fn decode_envelope(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::SpawnEnvelopeArmed {
            ceiling,
            max_children,
            max_per_call,
            max_provider_turns,
            expires_after_turns,
        } => SmedEvent::SpawnEnvelopeArmed {
            session,
            ceiling: ceiling.into(),
            max_children,
            max_per_call,
            max_provider_turns,
            expires_after_turns,
        },
        EventPayload::SpawnEnvelopeDrawn {
            run,
            children,
            provider_turns,
            children_remaining,
        } => SmedEvent::SpawnEnvelopeDrawn {
            session,
            run: RunId::from_uuid(run),
            children,
            provider_turns,
            children_remaining,
        },
        EventPayload::SpawnEnvelopeCleared { reason } => SmedEvent::SpawnEnvelopeCleared {
            session,
            reason: reason.into(),
        },
        other => unreachable!("decode_envelope received {}", other.kind()),
    }
}

fn decode_subagent(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::SubagentSpawned {
            run,
            child,
            directive,
            policy,
            branch,
            worktree,
        } => SmedEvent::SubagentSpawned {
            session,
            run: RunId::from_uuid(run),
            child: SessionId::from_uuid(child),
            directive,
            policy: policy.into(),
            branch,
            worktree,
        },
        EventPayload::SubagentResultLate { child, detail } => SmedEvent::SubagentResultLate {
            session,
            child: SessionId::from_uuid(child),
            detail,
        },
        EventPayload::ReadSetCollision {
            reader,
            writer,
            path,
        } => SmedEvent::ReadSetCollision {
            session,
            reader: SessionId::from_uuid(reader),
            writer: SessionId::from_uuid(writer),
            path,
        },
        // `decode_lifecycle` only calls this for a subagent-shaped payload.
        other => decode_lifecycle(session, other),
    }
}

/// Trigger lifecycle payloads.
fn decode_trigger(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::TriggerFired {
            trigger,
            child,
            source,
        } => SmedEvent::TriggerFired {
            session,
            trigger,
            child: SessionId::from_uuid(child),
            source: source.into(),
        },
        EventPayload::TriggerSettled {
            trigger,
            child,
            outcome,
            reason_code,
        } => SmedEvent::TriggerSettled {
            session,
            trigger,
            child: SessionId::from_uuid(child),
            outcome: outcome.into(),
            reason_code: reason_code.map(|code| code.0),
        },
        EventPayload::TriggerSkipped {
            trigger,
            overlap,
            detail,
        } => SmedEvent::TriggerSkipped {
            session,
            trigger,
            overlap: overlap.into(),
            detail,
        },
        EventPayload::TriggerQueued { trigger } => SmedEvent::TriggerQueued { session, trigger },
        EventPayload::TriggerReplaced {
            trigger,
            replaced_child,
        } => SmedEvent::TriggerReplaced {
            session,
            trigger,
            replaced_child: SessionId::from_uuid(replaced_child),
        },
        EventPayload::TriggerDisabled {
            trigger,
            code,
            consecutive_failures,
        } => SmedEvent::TriggerDisabled {
            session,
            trigger,
            code: code.0,
            consecutive_failures,
        },
        EventPayload::TriggerRearmed { trigger } => SmedEvent::TriggerRearmed { session, trigger },
        // `decode_lifecycle` only calls this for a trigger-shaped payload;
        // this mirrors `decode_tool`'s own catch-all for the same reason.
        other => decode_lifecycle(session, other),
    }
}

/// Tool proposal, approval, and outcome payloads.
fn decode_tool(session: SessionId, payload: EventPayload) -> SmedEvent {
    match payload {
        EventPayload::ToolProposed {
            run,
            approval,
            call,
            tier,
            preview,
        } => SmedEvent::ToolProposed {
            session,
            run: RunId::from_uuid(run),
            approval: approval.map(ApprovalId::from_uuid),
            call: call.into(),
            tier: tier.into(),
            preview,
        },
        EventPayload::ApprovalResolved {
            run,
            approval,
            decision,
        } => SmedEvent::ApprovalResolved {
            session,
            run: RunId::from_uuid(run),
            approval: ApprovalId::from_uuid(approval),
            decision: decision.into(),
        },
        EventPayload::ToolCompleted {
            run,
            call_id,
            name,
            result,
        } => SmedEvent::ToolCompleted {
            session,
            run: RunId::from_uuid(run),
            call_id,
            name,
            result: result.into(),
        },
        EventPayload::ToolFailed {
            run,
            call_id,
            name,
            code,
            detail,
        } => SmedEvent::ToolFailed {
            session,
            run: RunId::from_uuid(run),
            call_id,
            name,
            code: code.0,
            detail,
        },
        // Routed to `decode_lifecycle` by `decode`.
        other => decode_lifecycle(session, other),
    }
}

/// Serialise a payload for the `payload_json` column.
pub(in crate::store) fn encode_json(payload: &EventPayload) -> Result<String, WireError> {
    serde_json::to_string(payload).map_err(|error| WireError::Decode {
        detail: format!("event payload could not be encoded: {error}"),
    })
}

/// Parse a `payload_json` column, refusing a version this build cannot read.
pub(in crate::store) fn decode_json(json: &str, version: u32) -> Result<EventPayload, WireError> {
    check_version(version)?;
    serde_json::from_str(json).map_err(|error| WireError::Decode {
        detail: format!("event payload could not be decoded: {error}"),
    })
}

/// Serialise a checkpoint for the `state_json` column.
pub(in crate::store) fn encode_checkpoint(
    checkpoint: SessionCheckpoint,
) -> Result<String, WireError> {
    serde_json::to_string(&CheckpointWire::from(checkpoint)).map_err(|error| WireError::Decode {
        detail: format!("checkpoint could not be encoded: {error}"),
    })
}

/// Parse a `state_json` column, refusing a version this build cannot read.
pub(in crate::store) fn decode_checkpoint(
    json: &str,
    version: u32,
) -> Result<SessionCheckpoint, WireError> {
    check_version(version)?;
    let wire: CheckpointWire = serde_json::from_str(json).map_err(|error| WireError::Decode {
        detail: format!("checkpoint could not be decoded: {error}"),
    })?;
    Ok(wire.into())
}

/// Refuse a payload written by a newer smed.
///
/// Fail closed (`AGENTS.md` §1.2): a build that reads a newer payload
/// best-effort will drop the fields it does not understand on the next write.
/// Losing the session is recoverable; silently truncating it is not.
fn check_version(found: u32) -> Result<(), WireError> {
    if found > WIRE_VERSION {
        return Err(WireError::UnsupportedVersion {
            found,
            supported: WIRE_VERSION,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;

//! Normalised provider events and durable smed events.
//!
//! Two distinct vocabularies, deliberately not merged:
//!
//! - [`ProviderEvent`] is what an adapter emits while decoding one upstream
//!   stream. Ephemeral, high-frequency, per-request.
//! - [`SmedEvent`] is what the runtime broadcasts and (from Phase 4) persists.
//!   It is the session's history.
//!
//! Render deltas may be lost; final blocks and checkpoints may not.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::command::{ApprovalDecision, ApprovalId};
use crate::core::continuation::{HandoffCheckpoint, QuotaReserveStatus};
use crate::core::council::{CouncilFindingDisposition, CouncilReview};
use crate::core::message::{CanonicalMessage, ToolCall, ToolResult};
use crate::core::model::{ModelId, ProviderId, QuotaSnapshot, Usage};
use crate::core::plan::{
    PlanApproval, PlanHandoff, PlanProposal, PlanReview, ProductRequirementsDocument, Question,
    QuestionAnswer,
};
use crate::core::policy::PolicyMode;
use crate::core::recovery::{InterruptedWork, RecoveryDecision};
use crate::core::review::{ReviewAnchor, ReviewComment, ReviewThreadId};
use crate::core::routing::{BreakerState, RouteAdvanceCondition, RouteSelectionReason};
use crate::core::tool::ToolTier;
use crate::core::trigger::{OverlapPolicy, TriggerOutcome, TriggerSourceKind};

/// The normalised event set every provider adapter maps onto.
///
/// Required variants are fixed by the plan. The one worth explaining is
/// [`UnknownUpstream`]: it is not defensive over-engineering. Anthropic's
/// documentation states that new event types may be added and clients "should
/// handle unknown event types gracefully" (`docs/provider-contract.md` §2). We
/// retain them diagnostically rather than crashing or, worse, silently
/// misinterpreting them.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Started,
    ReasoningDelta {
        text: String,
    },
    TextDelta {
        text: String,
    },
    /// A tool call began. Arguments are not available yet — they stream.
    ToolCallStarted {
        id: String,
        name: String,
    },
    /// A fragment of a tool call's arguments.
    ///
    /// **Fragments are keyed and not contiguous.** Accumulate per `id`; never
    /// assume the next fragment belongs to the call you last saw
    /// (`docs/provider-contract.md` §0).
    ToolArgumentsDelta {
        id: String,
        fragment: String,
    },
    Quota {
        snapshot: QuotaSnapshot,
    },
    /// The provider's completion boundary for a tool call. **This is the only
    /// point at which accumulated fragments may be parsed** — earlier is
    /// invalid JSON by construction.
    ToolCallCompleted {
        call: ToolCall,
    },
    Usage {
        usage: Usage,
    },
    Finished {
        reason: FinishReason,
    },
    /// Retained for diagnostics. Never fatal.
    UnknownUpstream {
        kind: String,
    },
    Failed {
        detail: String,
    },
}

/// Why a provider stream ended.
///
/// [`Incomplete`](Self::Incomplete) exists because OpenAI's OpenAPI spec
/// declares `response.incomplete` — a third terminal state beyond success and
/// failure, for a response that stopped early. Collapsing it into either would
/// misreport state (AGENTS.md §1.3), so it gets its own variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Incomplete,
    Cancelled,
    /// A manual handoff checkpoint was written.
    Handoff,
    /// The provider reserve entered drain and landed safely.
    QuotaDrained,
}

/// Why a spawn envelope stopped being in force.
///
/// Three distinct endings rather than one, because they mean different things to
/// whoever reads the record later: spent is the envelope doing its job, lapsed is
/// time running out on work that may be unfinished, and withdrawn is a human
/// changing their mind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeEnd {
    /// Every child or every provider turn was drawn.
    Spent,
    /// The turn budget elapsed.
    Lapsed,
    /// A human cleared it.
    Withdrawn,
    /// The session's policy narrowed below the envelope's ceiling.
    ///
    /// An envelope outliving the policy that justified it would be a standing
    /// grant nobody re-authorised.
    PolicyNarrowed,
}

/// Identifies a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Rebuild an id read from the store or typed by a user.
    ///
    /// Deliberately not `From<Uuid>`: minting a `SessionId` from an arbitrary
    /// UUID is something only persistence and the CLI may do, and spelling it
    /// out keeps that visible at the call site.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What allowed a newly loaded extension to become callable.
///
/// The load act is recorded so the log always shows *who or what* made a new
/// capability available. A human running the load command and a full-auto
/// policy deciding to are both legitimate, and a reader must be able to tell
/// them apart — the same reason [`ApprovalDecision`] distinguishes a human's
/// approval from an auto-by-policy one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionLoadAuthority {
    /// A human ran the `/load-extension` command.
    Command,
    /// Full-auto authorised an agent-proposed load without a human.
    FullAuto,
    /// A human approved an agent-proposed load at the Execute gate.
    Approved,
}

/// A durable session event.
///
/// Phase 4 persists these; Phase 1 keeps them in memory behind the same port.
/// The shape is chosen now because changing it later means a migration.
#[derive(Debug, Clone, PartialEq)]
pub enum SmedEvent {
    SessionCreated {
        session: SessionId,
        provider: ProviderId,
        model: ModelId,
    },
    MessageAppended {
        session: SessionId,
        message: Box<CanonicalMessage>,
    },
    /// A run began. One run is one provider exchange plus its tool activity.
    RunStarted {
        session: SessionId,
        run: RunId,
    },
    /// Ephemeral render delta. **Not durable** — coalesced into the final
    /// message block before persistence ("coalesce text deltas; do not
    /// commit one row per token").
    TextDelta {
        session: SessionId,
        run: RunId,
        text: String,
    },
    /// Ephemeral provider-private reasoning summary. Never durable or replayed.
    ReasoningDelta {
        session: SessionId,
        run: RunId,
        text: String,
    },
    /// A provider named a tool whose arguments are still being assembled.
    ToolAssembling {
        session: SessionId,
        run: RunId,
        name: String,
    },
    /// Provider-reported quota observed on an existing response.
    QuotaReported {
        session: SessionId,
        run: RunId,
        snapshot: QuotaSnapshot,
    },
    /// A quota threshold became governing state and was durably recorded.
    QuotaBoundaryReached {
        session: SessionId,
        run: RunId,
        reserve: QuotaReserveStatus,
    },
    /// Model-authored status plus mechanically derived event-log facts.
    HandoffCreated {
        session: SessionId,
        handoff: Box<HandoffCheckpoint>,
    },
    UsageReported {
        session: SessionId,
        run: RunId,
        usage: Usage,
    },
    PolicyChanged {
        session: SessionId,
        mode: PolicyMode,
    },
    /// The declared governance floor narrowed the session's policy
    /// .
    ///
    /// Distinct from [`PolicyChanged`](Self::PolicyChanged) on purpose: that
    /// one records a human deciding, this one records the runtime deciding
    /// *for* them. Reading the ledger later, "the owner set workspace-write"
    /// and "the owner set full-auto and the model's tier would not carry it"
    /// are different answers to the same question, and a single event type
    /// would have made them indistinguishable.
    ///
    /// Recorded rather than applied quietly for the reason law 6's cap is:
    /// a session that silently stopped being full-auto would be lying about
    /// its own state.
    PolicyClamped {
        session: SessionId,
        from: PolicyMode,
        to: PolicyMode,
        provider: crate::core::model::ProviderId,
        model: crate::core::model::ModelId,
        tier: crate::core::governance::GovernanceTier,
    },
    /// An agent-authored extension was loaded and is now callable this session
    /// . Records the tool's name, the fixed program it runs,
    /// and what authorised the load. Session-scoped: a resumed session does not
    /// replay this into a live registration — the record is evidence, not a
    /// reload instruction.
    ExtensionLoaded {
        session: SessionId,
        name: String,
        program: String,
        by: ExtensionLoadAuthority,
    },
    ToolProposed {
        session: SessionId,
        run: RunId,
        approval: Option<ApprovalId>,
        call: ToolCall,
        tier: ToolTier,
        preview: String,
    },
    ApprovalResolved {
        session: SessionId,
        run: RunId,
        approval: ApprovalId,
        decision: ApprovalDecision,
    },
    ToolCompleted {
        session: SessionId,
        run: RunId,
        call_id: String,
        name: String,
        result: ToolResult,
    },
    ToolFailed {
        session: SessionId,
        run: RunId,
        call_id: String,
        name: String,
        code: crate::core::error::ReasonCode,
        detail: String,
    },
    BudgetExhausted {
        session: SessionId,
        run: RunId,
    },
    RunFinished {
        session: SessionId,
        run: RunId,
        reason: FinishReason,
    },
    RunFailed {
        session: SessionId,
        run: RunId,
        code: crate::core::error::ReasonCode,
        detail: String,
    },
    ModelChanged {
        session: SessionId,
        provider: ProviderId,
        model: ModelId,
    },
    ModelChangeRefused {
        session: SessionId,
        provider: ProviderId,
        model: ModelId,
        code: crate::core::error::ReasonCode,
        detail: String,
    },
    /// A human-controlled desktop editor save completed after its stale and
    /// containment checks. The file contents are intentionally not duplicated
    /// in the event log; these digests identify the transition and the bytes
    /// remain on disk.
    FileSaved {
        session: SessionId,
        path: String,
        observed_digest: String,
        new_digest: String,
        size_bytes: u64,
    },
    /// A subagent session was dispatched from this session.
    ///
    /// Durable: the spawn boundary — which child, on which branch, in which
    /// worktree, under which clamped policy — is the fact recovery and the
    /// audit trail reason about. Recorded before the child starts, so a crash
    /// mid-spawn still names the child that might exist.
    /// A human armed a spawn envelope.
    ///
    /// Durable because the audit's job is to answer "what did this human
    /// authorise, and what was done with it?" — and the first half of that
    /// question has no answer if the authorisation itself is not recorded.
    SpawnEnvelopeArmed {
        session: SessionId,
        ceiling: PolicyMode,
        max_children: u32,
        max_per_call: u32,
        max_provider_turns: u32,
        expires_after_turns: u32,
    },
    /// A spawn drew against the active envelope.
    ///
    /// The second half of that question. Each draw records what it took and what
    /// remained, so the ledger reconstructs the envelope's whole life without
    /// needing the in-memory state that produced it.
    SpawnEnvelopeDrawn {
        session: SessionId,
        run: RunId,
        children: u32,
        provider_turns: u32,
        children_remaining: u32,
    },
    /// The envelope ended — spent, lapsed, or withdrawn.
    SpawnEnvelopeCleared {
        session: SessionId,
        reason: EnvelopeEnd,
    },
    SubagentSpawned {
        session: SessionId,
        run: RunId,
        child: SessionId,
        directive: String,
        policy: PolicyMode,
        branch: String,
        worktree: String,
    },
    /// A child record arrived after its spawn group had already settled.
    ///
    /// Durable because a late result is recorded, never silently dropped and
    /// never allowed to reopen the settled group.
    SubagentResultLate {
        session: SessionId,
        child: SessionId,
        detail: String,
    },
    /// A concurrent sibling's mutation invalidated a file this child had read.
    ///
    /// Durable because it is the fact that turns an otherwise-verified finish
    /// into a re-validation requirement: without it, the record would show a
    /// child completing normally while its read of `path` was stale. `reader`
    /// is the child whose read is invalidated; `writer` is the sibling whose
    /// mutation did it.
    ReadSetCollision {
        session: SessionId,
        reader: SessionId,
        writer: SessionId,
        path: String,
    },
    /// Ephemeral child activity forwarded for display, mapped from the child's
    /// own event stream. Never durable: the child's transcript is the durable
    /// record; this exists so a watching human sees movement.
    SubagentActivity {
        session: SessionId,
        run: RunId,
        child: SessionId,
        label: String,
    },
    /// A resumed session found work that was interrupted by a crash.
    ///
    /// Durable so the audit trail records that smed stopped and asked, rather
    /// than only recording what happened afterwards. Without it, a transcript
    /// would show a gap and then a decision, with nothing explaining why.
    RecoveryRequired {
        session: SessionId,
        work: Box<InterruptedWork>,
    },
    /// A human resolved interrupted work.
    ///
    /// Durable because the decision is the thing that unblocks autonomous work.
    /// A decision held only in memory would be re-asked after the next restart,
    /// or worse, silently forgotten.
    RecoveryResolved {
        session: SessionId,
        decision: RecoveryDecision,
    },
    /// The session accepts no further work.
    SessionEnded {
        session: SessionId,
    },
    /// A trigger began a firing.
    ///
    /// Recorded on the trigger's control session — the durable home for a
    /// trigger's lifecycle, distinct from `child`, which is the ordinary
    /// session the firing itself runs in ("every firing is a session").
    TriggerFired {
        session: SessionId,
        trigger: String,
        child: SessionId,
        source: TriggerSourceKind,
    },
    /// A firing reached a terminal outcome.
    TriggerSettled {
        session: SessionId,
        trigger: String,
        child: SessionId,
        outcome: TriggerOutcome,
        reason_code: Option<crate::core::error::ReasonCode>,
    },
    /// An occurrence was dropped because the trigger's overlap policy is
    /// [`OverlapPolicy::Skip`] and a firing was already in flight (or the
    /// trigger is disabled).
    TriggerSkipped {
        session: SessionId,
        trigger: String,
        overlap: OverlapPolicy,
        detail: String,
    },
    /// An occurrence was held for the in-flight firing to settle
    /// ([`OverlapPolicy::Queue`]).
    TriggerQueued {
        session: SessionId,
        trigger: String,
    },
    /// The in-flight firing was cancelled so this occurrence could start
    /// ([`OverlapPolicy::Replace`]).
    TriggerReplaced {
        session: SessionId,
        trigger: String,
        replaced_child: SessionId,
    },
    /// The trigger disabled itself after repeated firing failures. It fires no
    /// more occurrences until a human re-arms it.
    TriggerDisabled {
        session: SessionId,
        trigger: String,
        code: crate::core::error::ReasonCode,
        consecutive_failures: u32,
    },
    /// A human re-armed a disabled trigger.
    TriggerRearmed {
        session: SessionId,
        trigger: String,
    },
    /// A route was selected for this session, or for a child spawned from it
    /// . Every "why this model" question has a one-line
    /// answer here: the route name, the starting position, and the reason —
    /// named explicitly, resolved from a task class, or the configured child
    /// default.
    RouteSelected {
        session: SessionId,
        /// `Some` when this selection is for a child spawned from `session`;
        /// `None` when it is `session`'s own attach. Mirrors how
        /// [`TriggerFired`](Self::TriggerFired) names its `child` distinctly
        /// from the control `session` that recorded it.
        child: Option<SessionId>,
        route: String,
        position: usize,
        provider: ProviderId,
        model: ModelId,
        reason: RouteSelectionReason,
    },
    /// A route advanced one position along its ordered chain because a typed
    /// condition fired. Evidence: the rule that fired is named, not inferred.
    RouteAdvanced {
        session: SessionId,
        run: RunId,
        route: String,
        from_position: usize,
        to_position: usize,
        provider: ProviderId,
        model: ModelId,
        condition: RouteAdvanceCondition,
    },
    /// A route had no viable position left. A typed stop, never a silent
    /// retry loop.
    RouteExhausted {
        session: SessionId,
        run: RunId,
        route: String,
        condition: RouteAdvanceCondition,
    },
    /// A provider's circuit breaker changed state (/// "Closed → Open → `HalfOpen`... a breaker state change is an event, not a
    /// log line").
    BreakerStateChanged {
        session: SessionId,
        provider: ProviderId,
        from: BreakerState,
        to: BreakerState,
    },
    /// A human started a bounded model-led interview.
    PlanInterviewStarted {
        session: SessionId,
        plan_id: crate::core::plan::PlanId,
        goal: String,
    },
    /// A clarification question was asked.
    PlanQuestionAsked {
        session: SessionId,
        plan_id: crate::core::plan::PlanId,
        question: Question,
    },
    /// A clarification question was answered.
    PlanQuestionAnswered {
        session: SessionId,
        plan_id: crate::core::plan::PlanId,
        answer: QuestionAnswer,
    },
    /// The interview produced a durable PRD artifact.
    PlanPrdProposed {
        session: SessionId,
        prd: ProductRequirementsDocument,
    },
    /// A plan revision was proposed.
    PlanProposed {
        session: SessionId,
        proposal: PlanProposal,
    },
    /// An advisory review was recorded for a plan revision.
    PlanReviewed {
        session: SessionId,
        review: PlanReview,
    },
    /// A human approval decision was recorded for a plan revision.
    PlanApproved {
        session: SessionId,
        approval: PlanApproval,
    },
    /// An approved plan entered handoff.
    PlanHandoffCreated {
        session: SessionId,
        handoff: PlanHandoff,
    },
    /// A completed advisory council review, including the evidence a human
    /// may later disposition. Durable before its transcript rendering.
    CouncilReviewed {
        session: SessionId,
        review: Box<CouncilReview>,
    },
    /// A human disposition on one council finding. This is advisory metadata;
    /// it never approves a tool or starts a side effect.
    CouncilFindingDispositionRecorded {
        session: SessionId,
        disposition: CouncilFindingDisposition,
    },
    /// A human-reviewable amended artifact was composed from accepted
    /// findings. Durable because the proposal states which review and which
    /// digest it was built from, so a later save can be judged against it.
    /// Composing a proposal is not a write and not an approval.
    CouncilAmendmentProposed {
        session: SessionId,
        amendment: Box<crate::core::council::CouncilAmendment>,
    },
    /// A human pinned a note to one line of one diff.
    ///
    /// Durable because §D3 requires notes to survive a restart with their
    /// original anchor. The anchor travels whole rather than as a reference to
    /// the capture that produced it: captures are not durable, so a note that
    /// stored only a digest would come back from a restart unable to say which
    /// line it was about.
    ReviewNoteRecorded {
        session: SessionId,
        thread: ReviewThreadId,
        anchor: ReviewAnchor,
        comment: ReviewComment,
    },
    /// A further human remark on an existing thread.
    ReviewCommentAdded {
        session: SessionId,
        thread: ReviewThreadId,
        comment: ReviewComment,
    },
    /// A revision request naming these threads was sent into the session.
    ///
    /// Recorded only once the run carrying it actually started, so this event
    /// never asserts a request that did not go out. `run` is the run that will
    /// answer it.
    ReviewRequestSent {
        session: SessionId,
        threads: Vec<ReviewThreadId>,
        run: RunId,
    },
    /// The run started by a review request produced its answer.
    ///
    /// `response_message` is the `CanonicalMessage` id a client already keys
    /// its transcript by, which is what makes the §D3 "link to the resulting
    /// smed response" bullet a link a surface can follow. A run that was
    /// cancelled or failed emits nothing here, and the threads stay linked to
    /// no response — the honest record of what happened.
    ReviewRequestAnswered {
        session: SessionId,
        threads: Vec<ReviewThreadId>,
        response_message: Uuid,
    },
    /// A decision ticket was opened (Phase E5). Durable because tickets are
    /// permanent records — long-lived, spanning sessions — and the log is
    /// their truth.
    DecisionTicketOpened {
        session: SessionId,
        ticket: crate::core::board::DecisionTicket,
    },
    /// A human resolved a decision ticket. Records judgement and never
    /// authority (ADR-0015): nothing in here approves a tool, widens a
    /// policy, or satisfies an evidence claim — it moves the frontier only.
    DecisionTicketResolved {
        session: SessionId,
        resolution: crate::core::board::DecisionResolution,
    },
    /// An external work item was fetched and recorded as an imported node
    /// (Phase E5, step 4b). The frontier already knew how to project it; this
    /// is the durable fact that makes it appear. `externalUnverified` by
    /// construction (design §2, ADR-0014) — untrusted text, never authority.
    ImportedItemFetched {
        session: SessionId,
        item: crate::core::imported::ImportedItem,
    },
    /// A previously imported item was refreshed at a new revision.
    /// `expected_revision` is the revision the human saw when they approved the
    /// refresh; a mismatch is refused with `WORKSPACE_STALE_REVISION` rather than
    /// recorded — a stale tab is refused, not recorded (contract (a), the same
    /// revision-pinning `ReviewNoteRecorded` relies on for its `capture_digest`).
    ImportedItemRefreshed {
        session: SessionId,
        expected_revision: String,
        item: crate::core::imported::ImportedItem,
    },
    /// A mutating act on an imported item was recorded (phase D6, step 5) — a
    /// submitted pull request, or the honest `Uncertain` attempt whose result
    /// protocol was not proven. Durable because the board's act history is the
    /// product: the record is the only thing that says what a rendered act
    /// shipped, and recovery governance needs the ambiguity on disk, not in a
    /// retry policy. It reports what the deterministic code already did; it is
    /// not itself a gate (the act's own guards ran before this event existed).
    ImportedActRecorded {
        session: SessionId,
        act: crate::core::imported::ImportedAct,
    },
    /// A human-authored comment recorded against an external discussion (Phase D6
    /// sync-back). Prose, bounded, never authority — same framing discipline as
    /// `ImportedItem`'s title/body.
    ImportedCommentRecorded {
        session: SessionId,
        item_id: crate::core::imported::ImportedItemId,
        comment_id: String,
        body: String,
    },
}

impl SmedEvent {
    /// Whether this event belongs in durable history.
    ///
    /// Text deltas are the exception: they exist to drive a render and are
    /// reconstructable from the final message. Persisting them would put one
    /// row per token in SQLite, which  forbids.
    #[must_use]
    pub const fn is_durable(&self) -> bool {
        !matches!(
            self,
            Self::TextDelta { .. }
                | Self::ReasoningDelta { .. }
                | Self::ToolAssembling { .. }
                | Self::QuotaReported { .. }
                | Self::SubagentActivity { .. }
        )
    }

    /// Whether replaying this event appends exactly one transcript message
    /// .
    ///
    /// This is the rule that lets a checkpoint's transcript be re-anchored to
    /// the record: the message-bearing events below the checkpoint's extent
    /// correspond, one for one and in order, to the messages it stored. It
    /// lives here, on the event, so the projection that *creates* messages and
    /// the store queries that *count* them cannot drift apart — a store whose
    /// idea of "message-bearing" differed by one variant would anchor every
    /// entry after it to the wrong event.
    #[must_use]
    pub const fn introduces_message(&self) -> bool {
        matches!(
            self,
            Self::MessageAppended { .. } | Self::ToolCompleted { .. } | Self::ToolFailed { .. }
        )
    }

    #[must_use]
    pub const fn session(&self) -> SessionId {
        match self {
            Self::SessionCreated { session, .. }
            | Self::MessageAppended { session, .. }
            | Self::RunStarted { session, .. }
            | Self::TextDelta { session, .. }
            | Self::ReasoningDelta { session, .. }
            | Self::ToolAssembling { session, .. }
            | Self::QuotaReported { session, .. }
            | Self::QuotaBoundaryReached { session, .. }
            | Self::HandoffCreated { session, .. }
            | Self::UsageReported { session, .. }
            | Self::PolicyChanged { session, .. }
            | Self::PolicyClamped { session, .. }
            | Self::ExtensionLoaded { session, .. }
            | Self::SpawnEnvelopeArmed { session, .. }
            | Self::SpawnEnvelopeDrawn { session, .. }
            | Self::SpawnEnvelopeCleared { session, .. }
            | Self::ToolProposed { session, .. }
            | Self::ApprovalResolved { session, .. }
            | Self::ToolCompleted { session, .. }
            | Self::ToolFailed { session, .. }
            | Self::BudgetExhausted { session, .. }
            | Self::RunFinished { session, .. }
            | Self::RunFailed { session, .. }
            | Self::ModelChanged { session, .. }
            | Self::ModelChangeRefused { session, .. }
            | Self::FileSaved { session, .. }
            | Self::SubagentSpawned { session, .. }
            | Self::SubagentResultLate { session, .. }
            | Self::ReadSetCollision { session, .. }
            | Self::SubagentActivity { session, .. }
            | Self::RecoveryRequired { session, .. }
            | Self::RecoveryResolved { session, .. }
            | Self::SessionEnded { session }
            | Self::TriggerFired { session, .. }
            | Self::TriggerSettled { session, .. }
            | Self::TriggerSkipped { session, .. }
            | Self::TriggerQueued { session, .. }
            | Self::TriggerReplaced { session, .. }
            | Self::TriggerDisabled { session, .. }
            | Self::TriggerRearmed { session, .. }
            | Self::RouteSelected { session, .. }
            | Self::RouteAdvanced { session, .. }
            | Self::RouteExhausted { session, .. }
            | Self::BreakerStateChanged { session, .. }
            | Self::PlanInterviewStarted { session, .. }
            | Self::PlanQuestionAsked { session, .. }
            | Self::PlanQuestionAnswered { session, .. }
            | Self::PlanPrdProposed { session, .. }
            | Self::PlanProposed { session, .. }
            | Self::PlanReviewed { session, .. }
            | Self::PlanApproved { session, .. }
            | Self::PlanHandoffCreated { session, .. }
            | Self::CouncilReviewed { session, .. }
            | Self::CouncilFindingDispositionRecorded { session, .. }
            | Self::CouncilAmendmentProposed { session, .. }
            | Self::ReviewNoteRecorded { session, .. }
            | Self::ReviewCommentAdded { session, .. }
            | Self::ReviewRequestSent { session, .. }
            | Self::ReviewRequestAnswered { session, .. }
            | Self::DecisionTicketOpened { session, .. }
            | Self::DecisionTicketResolved { session, .. }
            | Self::ImportedItemFetched { session, .. }
            | Self::ImportedItemRefreshed { session, .. }
            | Self::ImportedActRecorded { session, .. }
            | Self::ImportedCommentRecorded { session, .. } => *session,
        }
    }
}

/// Stable identity assigned to every durable event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(Uuid);

impl EventId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Identifies one run within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(Uuid);

impl RunId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A durable event with its assigned ordering.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    pub id: EventId,
    pub sequence: u64,
    pub occurred_at: OffsetDateTime,
    pub event: SmedEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_deltas_are_not_durable() {
        let session = SessionId::new();
        let run = RunId::new();

        let delta = SmedEvent::TextDelta {
            session,
            run,
            text: "a".to_owned(),
        };
        assert!(!delta.is_durable(), "one row per token is forbidden by ");

        let finished = SmedEvent::RunFinished {
            session,
            run,
            reason: FinishReason::Stop,
        };
        assert!(finished.is_durable());
    }

    #[test]
    fn session_ids_are_time_sortable() {
        let first = SessionId::new();
        let second = SessionId::new();
        assert!(first < second);
    }
}

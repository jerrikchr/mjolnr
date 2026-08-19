//! Session state owned by the runtime actor.
//!
//! This is the authoritative transcript. Nothing outside the actor task holds a
//! mutable reference to it, which is what lets the runtime be lock-free on the
//! hot path: clients get an `Arc` snapshot, not a borrow.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::core::continuation::{HandoffCheckpoint, QuotaReserveStatus, ResumeAdvice};
use crate::core::event::SessionId;
use crate::core::message::{CanonicalMessage, ContentBlock, ToolCall, TranscriptEntry};
use crate::core::model::{ModelId, ProviderId, Usage};
use crate::core::policy::{PendingApproval, PolicyMode};
use crate::core::recovery::RecoveryState;
use crate::core::routing::{BreakerView, CircuitBreaker, RouteRuntime};
use crate::core::runtime::{BudgetStatus, RuntimeSnapshot};
use crate::core::tool::CommandSpec;
use crate::core::tool::ReadSet;

/// Everything the runtime knows about the open session.
#[derive(Debug, Default)]
pub struct SessionState {
    pub session: Option<SessionId>,
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
    pub workspace_root: Option<std::path::PathBuf>,
    pub usage: Usage,
    pub policy: PolicyMode,
    pub pending_approval: Option<PendingApproval>,
    pub budget: BudgetStatus,
    /// Tier 1 frozen rules snapshot loaded at session start (master implementation plan §2.1).
    pub rules_snapshot: crate::memory::RulesSnapshot,
    /// Why the Tier 1 rules load refused or failed, when it did. A refusal
    /// must not read as "this workspace declares no rules" (AGENTS.md §1.3).
    pub rules_load_error: Option<String>,
    /// Shared with the background consolidation task: the last known memory
    /// projection counts, and why they are unknown when they are. The actor
    /// and its consolidation task are the same authority over this state, so
    /// the slot is a bridge, not a second owner.
    pub memory_projection: Arc<std::sync::Mutex<crate::runtime::memory::MemoryProjection>>,
    pub plan: Option<crate::core::plan::PlanWorkflow>,
    pub read_set: Arc<ReadSet>,
    /// Exact-command grants for **this** session only.
    ///
    /// Never checkpointed and never rebuilt on resume.  scopes a grant
    /// to one session; carrying it across a restart would widen the authority a
    /// human granted without them doing anything (`docs/persistence.md` §6).
    pub exact_commands: HashSet<CommandSpec>,
    /// The spawn envelope armed for **this** session only.
    ///
    /// Sits beside `exact_commands` because it is the same kind of thing at a
    /// different N: a human act that pre-authorises a bounded set of future
    /// acts. Never checkpointed and never rebuilt on resume, for the same
    /// reason — a session that comes back without the human doing anything is
    /// not the stretch of work they armed it for.
    pub envelope: Option<crate::core::envelope::ActiveEnvelope>,
    /// Why the last arming attempt was refused, for the client to show.
    pub envelope_refusal: Option<String>,
    pub last_mutation_sequence: Option<u64>,
    pub successful_command_evidence: BTreeMap<String, u64>,
    pub activated_skills: BTreeSet<String>,
    /// The most recent `/reload` result, surfaced on the snapshot.
    pub last_reload: Option<crate::core::context::ReloadReport>,
    /// The most recent load-extension result, surfaced on the snapshot
    /// .
    pub last_extension_load: Option<crate::core::context::ExtensionLoadReport>,
    /// The session tree as of the last `LoadSessionTree`, including abandoned
    /// branches. Empty until something asks for it.
    pub tree: Arc<Vec<crate::core::store::SessionTreeNode>>,
    /// Summary of the branch most recently switched away from (
    /// 16.5). Cleared when a new session starts.
    pub left_branch: Option<crate::core::store::BranchSummary>,
    /// Set by a rewind: the sequence the *next* durable event must record as
    /// its parent, making it a branch point rather than a linear continuation
    /// . Cleared once consumed.
    pub branch_parent: Option<Option<u64>>,
    /// Messages queued to steer the run in flight.
    ///
    /// FIFO and deliberately not persisted: an undelivered steering message
    /// belongs to the turn it was typed against, and resurrecting it into a
    /// later session would deliver it into a context the user never saw.
    pub steering: std::collections::VecDeque<String>,
    pub workspace_trusted: bool,
    pub handoff: Option<HandoffCheckpoint>,
    pub quota_reserve: QuotaReserveStatus,
    /// The full multi-window quota snapshot behind `quota_reserve`'s single
    /// worst window. Kept as durable session state rather than left
    /// to a fire-and-forget event so a freshly resynced/reconnected client sees
    /// it immediately instead of waiting for the next provider response to
    /// report it again.
    pub quota: Option<crate::core::model::QuotaSnapshot>,
    pub resume_advice: Option<ResumeAdvice>,
    /// This session's live position on an attached route.
    /// `None` whenever no route is attached, which is exactly present-day
    /// behaviour: the session simply uses its configured provider/model.
    pub route: Option<RouteRuntime>,
    /// The persona the session has explicitly selected ,
    /// overriding whatever the active route would wear. `None` means no
    /// override: the active route's own persona applies, or the bare Soul does.
    /// Runtime-only and deliberately not persisted — a resumed session reverts
    /// to the route's persona, the same posture the queues take toward
    /// undelivered dynamic state.
    pub persona_override: Option<String>,
    /// Per-provider circuit breaker state, populated lazily as an attached
    /// route's hops are touched. Never populated when no route is attached.
    pub breakers: HashMap<ProviderId, CircuitBreaker>,
    pub sessions: Arc<Vec<crate::core::store::SessionSummary>>,
    /// What git last said about the open project ( producer).
    ///
    /// Belongs to the project rather than the session, which is why
    /// [`reset_keeping_project`](Self::reset_keeping_project) keeps it beside
    /// `workspace_root`: ending a session does not change the repository, and
    /// clearing it would blank the surface and then silently refill it on the
    /// next unrelated trigger.
    pub repository: crate::core::repository::RepositoryView,
    /// The exact diffs behind `repository`, captured in the same refresh (plan
    /// §Phase D3 producer). Kept beside it, and reset with it, because a change
    /// set paired with a status from another moment describes a working tree
    /// that never existed.
    pub changes: crate::core::change_capture::ChangeView,
    /// Which durable `ToolCompleted` event recorded each read, keyed by the
    /// workspace-relative path the effect carried.
    ///
    /// Session-scoped, unlike `changes` and `repository`: the evidence is
    /// "this session read this file", and carrying it across a new session
    /// would attribute a read to work that never performed it. A `BTreeMap`
    /// rather than a `Vec` because a second read of the same file supersedes
    /// the first — the newest event is the one an edit was made against.
    pub read_evidence: BTreeMap<String, crate::core::change_capture::ReadRecord>,
    /// Line notes this session pinned to a diff, oldest first.
    ///
    /// Session-scoped like the read evidence beside it, and durable unlike it:
    /// every mutation arrives as a `Review*` event, folded in by the single
    /// reducer in `runtime::review` so a replayed session and a live one cannot
    /// hold different notes.
    pub review_threads: crate::runtime::review::ReviewThreads,
    /// The latest durable advisory council review and its human dispositions.
    /// Council evidence is session state, not an actor-side cache, so resume
    /// and reconnect cannot silently forget it.
    pub last_council: Option<crate::core::council::CouncilReview>,
    /// The latest amendment a human asked the runtime to compose from accepted
    /// findings. Durable state rather than a cache, so a reconnect does not
    /// silently drop a proposal the operator was part-way through reviewing.
    /// Holding it authorizes nothing: it is text awaiting a human save.
    pub last_council_amendment: Option<crate::core::council::CouncilAmendment>,
    /// Decision tickets opened in this session and their current effective
    /// resolutions (Phase E5).
    ///
    /// Session-scoped like the review threads beside it, and durable: every
    /// mutation arrives as a `DecisionTicket*` event folded in by one reducer,
    /// so a live session and a replayed one cannot disagree. Permanence
    /// *across* sessions comes from the event log itself — the cross-session
    /// projection is the frontier's job, and it is explicitly not this map's.
    pub decision_tickets:
        BTreeMap<crate::core::board::DecisionTicketId, crate::core::board::DecisionTicketRecord>,
    /// Imported work items fetched into this session (Phase E5, step 4b).
    ///
    /// Session-scoped like decision tickets, and durable: every mutation arrives
    /// as an `ImportedItem*` event folded in by the same board reducer, so a
    /// live session and a replayed one cannot disagree.
    pub imported_items:
        BTreeMap<crate::core::imported::ImportedItemId, crate::core::imported::ImportedItem>,
    /// Durable records of mutating acts this session performed on imported work
    /// items (phase D6, step 5) — submitted pull requests and the honest
    /// `Uncertain` attempts whose result protocol was not proven.
    ///
    /// Session-scoped like imported items, and durable: every act arrives as an
    /// `ImportedActRecorded` event folded in by the same reducer, so a live
    /// session and a replayed one cannot disagree. The event log is the truth;
    /// this map exists so the session's own fold and the board projection share
    /// one record.
    pub imported_acts:
        BTreeMap<crate::core::imported::ImportedActId, crate::core::imported::ImportedAct>,
    /// Number of completed repository refreshes, for
    /// [`RepositoryProjection::capture_sequence`](crate::core::repository::RepositoryProjection::capture_sequence).
    pub repository_captures: u32,
    /// Non-durable provider projection. Durable history remains in `messages`.
    compact_context: Option<CompactContext>,
    /// Set when a durable write did not happen.
    ///
    /// While this is set the session accepts no new work: continuing would build
    /// on history the store never accepted, and the gap would surface only after
    /// the next restart, as work nobody can account for (`AGENTS.md` §1.3).
    pub store_failure: Option<String>,
    /// `Arc` so a snapshot is a refcount bump rather than a transcript copy
    /// (AGENTS.md §5).
    messages: Arc<Vec<TranscriptEntry>>,
}

#[derive(Debug)]
struct CompactContext {
    seed: Vec<CanonicalMessage>,
    history_floor: usize,
}

impl SessionState {
    /// Clear the session, keeping the project open.
    ///
    /// A method rather than struct-update syntax because `messages` is private:
    /// the transcript may only be replaced through the type that owns its `Arc`
    /// invariant, not assembled field-by-field from outside.
    pub fn reset_keeping_project(&mut self) {
        let workspace_root = self.workspace_root.take();
        // The repository belongs to the project, not the session. Dropping it
        // here would blank the surface on every new session and then refill it
        // on the next unrelated trigger, which reads as data appearing for no
        // reason. The capture counter rides along so a client can still tell
        // that a later projection is newer than one it already rendered.
        let repository = std::mem::take(&mut self.repository);
        let changes = std::mem::take(&mut self.changes);
        let repository_captures = self.repository_captures;
        // The memory projection is project-scoped for the same reason the
        // repository is: `memory.db` lives under the workspace root, not the
        // session. Resetting it would flip the inspector to "unknown" on every
        // new session for no change in the data.
        let memory_projection = std::mem::take(&mut self.memory_projection);
        *self = Self {
            workspace_root,
            repository,
            changes,
            repository_captures,
            memory_projection,
            read_set: Arc::new(ReadSet::default()),
            ..Self::default()
        };
    }

    /// Validate a durable event against a copy of the authoritative workflow.
    ///
    /// The actor calls this before append so a refused transition can never
    /// enter durable history. The actor is the only state writer, so a
    /// successful validation remains valid until the immediately following
    /// append and reduction.
    pub fn validate_event(
        &self,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        Self::validate_imported_comment(event)?;
        let mut plan = self.plan.clone();
        let mut council = self.last_council.clone();
        let mut tickets = self.decision_tickets.clone();
        let mut imported = self.imported_items.clone();
        let mut acts = self.imported_acts.clone();
        Self::apply_plan_event(&mut plan, event)?;
        Self::apply_council_event(&mut council, event)?;
        Self::apply_board_event(&mut tickets, event)?;
        Self::apply_imported_event(&mut imported, event)?;
        Self::apply_imported_act_event(&mut acts, &imported, event)
    }

    fn validate_imported_comment(
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::event::SmedEvent;
        if let SmedEvent::ImportedCommentRecorded { body, .. } = event {
            if body.len() > crate::integrations::MAX_REMOTE_BODY_BYTES {
                return Err(crate::core::error::SmedError::workspace_refused(
                    crate::core::error::ReasonCode::SchemaInvalid,
                    format!(
                        "imported comment body may not exceed {} bytes",
                        crate::integrations::MAX_REMOTE_BODY_BYTES
                    ),
                ));
            }
            if body
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(crate::core::error::SmedError::workspace_refused(
                    crate::core::error::ReasonCode::SchemaInvalid,
                    "imported comment body may not contain control characters other than newline and tab",
                ));
            }
        }
        Ok(())
    }

    /// Apply a durable event onto session state.
    pub fn apply_event(
        &mut self,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        Self::validate_imported_comment(event)?;
        Self::apply_plan_event(&mut self.plan, event)?;
        if let crate::core::event::SmedEvent::CouncilAmendmentProposed { amendment, .. } = event {
            self.last_council_amendment = Some((**amendment).clone());
        }
        Self::apply_council_event(&mut self.last_council, event)?;
        Self::apply_board_event(&mut self.decision_tickets, event)?;
        Self::apply_imported_event(&mut self.imported_items, event)?;
        Self::apply_imported_act_event(&mut self.imported_acts, &self.imported_items, event)
    }

    /// Fold the Phase E5 decision-ticket events into the ticket map.
    ///
    /// Two refusals protect the log from the two ways the recorded graph
    /// could stop meaning something: an id recorded twice, and a blocker
    /// named against a ticket that does not exist in this set. Cycles need
    /// no check here — with edges recorded only at open and ids minted by
    /// that same open, a blocking cycle cannot be constructed through this
    /// path at all; surfacing the cycles that arrive *via imported items* is
    /// the frontier's job.
    fn apply_board_event(
        tickets: &mut BTreeMap<
            crate::core::board::DecisionTicketId,
            crate::core::board::DecisionTicketRecord,
        >,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::board::DecisionTicketRecord;
        use crate::core::error::{ReasonCode, SmedError};
        use crate::core::event::SmedEvent;

        match event {
            SmedEvent::DecisionTicketOpened { ticket, .. } => {
                if tickets.contains_key(&ticket.id) {
                    return Err(SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "decision ticket {} was already recorded; the log is append-only, \
                             so a duplicate id is a bug, not an update",
                            ticket.id
                        ),
                    ));
                }
                for blocker in &ticket.blocked_by {
                    if !tickets.contains_key(blocker) {
                        return Err(SmedError::workspace_refused(
                            ReasonCode::SchemaInvalid,
                            format!(
                                "decision ticket {} names blocker {blocker}, which does not \
                                 exist in this session; a blocking edge into nothing would fog \
                                 the ticket behind an id that can never resolve. Nothing was \
                                 recorded",
                                ticket.id
                            ),
                        ));
                    }
                }
                tickets.insert(
                    ticket.id,
                    DecisionTicketRecord {
                        ticket: ticket.clone(),
                        resolution: None,
                    },
                );
                Ok(())
            }
            SmedEvent::DecisionTicketResolved { resolution, .. } => {
                let record = tickets.get_mut(&resolution.ticket).ok_or_else(|| {
                    SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "resolution {} names ticket {}, which does not exist in this \
                             session; a decision cannot be recorded against nothing",
                            resolution.id, resolution.ticket
                        ),
                    )
                })?;
                record
                    .apply_resolution(resolution.clone())
                    .map_err(|detail| {
                        SmedError::workspace_refused(ReasonCode::SchemaInvalid, detail)
                    })
            }
            _ => Ok(()),
        }
    }

    /// Fold the Phase E5 step 4b imported-item events into the item map.
    ///
    /// `ImportedItemFetched` records the fetch; `ImportedItemRefreshed` must name
    /// the revision the human saw when they approved the refresh — a mismatch is
    /// refused with `WORKSPACE_STALE_REVISION` rather than recorded (contract (a),
    /// the same revision-pinning `ReviewNoteRecorded` relies on for its
    /// `capture_digest`). `Unknown` is never cached as a value; a re-fetch at a
    /// new revision supersedes it (contract (c)).
    ///
    /// The refresh checks live in `ImportedItemRecord::apply_refresh`, not here:
    /// this fold maps the typed refusal onto a reason code, so the live path
    /// (`validate_event`) and the replay path apply the same guard — the same
    /// delegation `apply_board_event` uses for resolutions.
    fn apply_imported_event(
        items: &mut BTreeMap<
            crate::core::imported::ImportedItemId,
            crate::core::imported::ImportedItem,
        >,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::error::{ReasonCode, SmedError};
        use crate::core::event::SmedEvent;
        use crate::core::imported::{ImportedItemRecord, RefreshRefusal};

        match event {
            SmedEvent::ImportedItemFetched { item, .. } => {
                if items.contains_key(&item.id) {
                    return Err(SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "imported item {} was already recorded; the log is append-only, \
                             so a duplicate id is a bug, not an update — refresh it instead",
                            item.id
                        ),
                    ));
                }
                items.insert(item.id, item.clone());
                Ok(())
            }
            SmedEvent::ImportedItemRefreshed {
                expected_revision,
                item,
                ..
            } => {
                let current = items.get(&item.id).ok_or_else(|| {
                    SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "imported item {} does not exist in this session; a refresh cannot \
                             create it — fetch it first",
                            item.id
                        ),
                    )
                })?;
                let mut record = ImportedItemRecord::new(current.clone());
                record
                    .apply_refresh(expected_revision, item.clone())
                    .map_err(|refusal| {
                        // A stale pin is retryable — re-fetch and try again —
                        // so it carries its own code; the rest are shape bugs.
                        let code = match refusal {
                            RefreshRefusal::StaleRevision { .. } => {
                                ReasonCode::WorkspaceStaleRevision
                            }
                            RefreshRefusal::UnknownItem { .. }
                            | RefreshRefusal::SameRevision { .. }
                            | RefreshRefusal::IdentityMoved => ReasonCode::SchemaInvalid,
                        };
                        SmedError::workspace_refused(code, refusal.to_string())
                    })?;
                items.insert(item.id, record.item);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Fold the Phase D6 step 5 `ImportedActRecorded` event into the act map.
    ///
    /// The act carries the item it was made against; a session that has no such
    /// imported item refuses the fold — an act over a stranger cannot be
    /// recorded as provenance (the board renderer joins on this id). Duplicate
    /// `act_id`s are refused the same way the item map refuses them: the log is
    /// append-only, so a repeat is a bug, not an update.
    fn apply_imported_act_event(
        acts: &mut BTreeMap<
            crate::core::imported::ImportedActId,
            crate::core::imported::ImportedAct,
        >,
        items: &BTreeMap<
            crate::core::imported::ImportedItemId,
            crate::core::imported::ImportedItem,
        >,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::error::{ReasonCode, SmedError};
        use crate::core::event::SmedEvent;

        if matches!(event, SmedEvent::ImportedCommentRecorded { .. }) {
            return Ok(());
        }
        match event {
            SmedEvent::ImportedActRecorded { act, .. } => {
                if !items.contains_key(&act.item_id) {
                    return Err(SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "act {} was recorded against imported item {}, which does not exist \
                             in this session",
                            act.act_id, act.item_id
                        ),
                    ));
                }
                if acts.contains_key(&act.act_id) {
                    return Err(SmedError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "act {} was already recorded; the log is append-only, so a duplicate \
                             id is a bug, not an update",
                            act.act_id
                        ),
                    ));
                }
                acts.insert(act.act_id, act.clone());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn apply_council_event(
        review_state: &mut Option<crate::core::council::CouncilReview>,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::event::SmedEvent;

        match event {
            SmedEvent::CouncilReviewed { review, .. } => {
                *review_state = Some((**review).clone());
                Ok(())
            }
            SmedEvent::CouncilFindingDispositionRecorded { disposition, .. } => {
                let review = review_state.as_mut().ok_or_else(|| {
                    crate::core::error::SmedError::workspace_refused(
                        crate::core::error::ReasonCode::SchemaInvalid,
                        "a council finding disposition requires a completed review",
                    )
                })?;
                review
                    .apply_disposition(disposition.clone())
                    .map_err(|detail| {
                        crate::core::error::SmedError::workspace_refused(
                            crate::core::error::ReasonCode::SchemaInvalid,
                            detail,
                        )
                    })
            }
            _ => Ok(()),
        }
    }

    fn apply_plan_event(
        plan_state: &mut Option<crate::core::plan::PlanWorkflow>,
        event: &crate::core::event::SmedEvent,
    ) -> crate::core::error::SmedResult<()> {
        use crate::core::event::SmedEvent;
        use crate::core::plan::PlanWorkflow;

        match event {
            SmedEvent::PlanInterviewStarted { plan_id, goal, .. } => {
                let plan = plan_state.get_or_insert_with(|| PlanWorkflow::new(*plan_id));
                if plan.plan_id != *plan_id {
                    return Err(crate::core::error::SmedError::plan_invalid_transition(
                        "plan mismatch",
                        "start interview",
                        "interview plan_id does not match workflow plan_id",
                    ));
                }
                plan.start_interview(goal.clone())?;
            }
            SmedEvent::PlanQuestionAsked {
                plan_id, question, ..
            } => {
                let plan = plan_state.get_or_insert_with(|| PlanWorkflow::new(*plan_id));
                plan.ask_question(question.clone())?;
            }
            SmedEvent::PlanQuestionAnswered {
                plan_id, answer, ..
            } => {
                let plan = Self::existing_plan(
                    plan_state,
                    "answer question",
                    "cannot answer a question before its plan workflow exists",
                )?;
                if plan.plan_id != *plan_id {
                    return Err(crate::core::error::SmedError::plan_invalid_transition(
                        "plan mismatch",
                        "answer question",
                        "answer plan_id does not match workflow plan_id",
                    ));
                }
                plan.answer_question(answer)?;
            }
            SmedEvent::PlanPrdProposed { prd, .. } => {
                let plan = Self::existing_plan(
                    plan_state,
                    "record PRD",
                    "cannot record a PRD before its interview workflow exists",
                )?;
                plan.record_prd(prd.clone())?;
            }
            SmedEvent::PlanProposed { proposal, .. } => {
                let plan_id = proposal.plan_id;
                let plan = plan_state.get_or_insert_with(|| PlanWorkflow::new(plan_id));
                plan.propose_plan(proposal.clone())?;
            }
            SmedEvent::PlanReviewed { review, .. } => {
                let plan = Self::existing_plan(
                    plan_state,
                    "review plan",
                    "cannot review a plan before its workflow exists",
                )?;
                plan.review_plan(review.clone())?;
            }
            SmedEvent::PlanApproved { approval, .. } => {
                let plan = Self::existing_plan(
                    plan_state,
                    "approve plan",
                    "cannot approve a plan before its workflow exists",
                )?;
                plan.approve_plan(approval.clone())?;
            }
            SmedEvent::PlanHandoffCreated { handoff, .. } => {
                let plan = Self::existing_plan(
                    plan_state,
                    "handoff plan",
                    "cannot hand off a plan before its workflow exists",
                )?;
                plan.handoff_plan(handoff.clone())?;
            }
            SmedEvent::CouncilReviewed { review, .. } => {
                if let (Some(plan_id), Some(prd_id)) = (review.plan_id, review.prd_id) {
                    let plan = Self::existing_plan(
                        plan_state,
                        "link council review",
                        "cannot link a council review before its plan workflow exists",
                    )?;
                    plan.link_council_review(crate::core::plan::PlanCouncilLink {
                        plan_id,
                        prd_id,
                        review_id: review.review_id,
                    })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn existing_plan<'a>(
        plan_state: &'a mut Option<crate::core::plan::PlanWorkflow>,
        action: &'static str,
        detail: &'static str,
    ) -> crate::core::error::SmedResult<&'a mut crate::core::plan::PlanWorkflow> {
        plan_state.as_mut().ok_or_else(|| {
            crate::core::error::SmedError::plan_invalid_transition("no workflow", action, detail)
        })
    }

    /// Replace the transcript with the messages on `events` (
    /// 16.5) and reduce workflow state.
    pub fn rebuild_messages_from(
        &mut self,
        events: &[crate::core::event::StoredEvent],
    ) -> Result<(), crate::core::store::StoreError> {
        let mut plan = None;
        let mut council = None;
        let mut messages = Vec::new();
        for stored in events {
            Self::apply_plan_event(&mut plan, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "workflow replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            Self::apply_council_event(&mut council, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "council replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            if let crate::core::event::SmedEvent::MessageAppended { message, .. } = &stored.event {
                messages.push(TranscriptEntry::anchored(
                    stored.sequence,
                    (**message).clone(),
                ));
            }
        }
        self.plan = plan;
        self.last_council = council;
        self.messages = Arc::new(messages);
        Ok(())
    }

    /// Rebuild only the structured records — the plan workflow, the council
    /// review, and the decision tickets — from the complete append-only
    /// branch.
    ///
    /// Plan approval is deliberately not copied into checkpoints: checkpoints
    /// must not become a second authority record. Resume therefore projects
    /// the workflow from its durable events even when transcript recovery can
    /// start from a later checkpoint. Decision tickets take their durability
    /// from the same argument: an event log the checkpoint cannot shadow.
    pub fn rebuild_durable_records_from(
        &mut self,
        events: &[crate::core::event::StoredEvent],
    ) -> Result<(), crate::core::store::StoreError> {
        let mut plan = None;
        let mut council = None;
        let mut tickets = BTreeMap::new();
        let mut imported = BTreeMap::new();
        let mut acts = BTreeMap::new();
        for stored in events {
            Self::apply_plan_event(&mut plan, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "workflow replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            Self::apply_council_event(&mut council, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "council replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            Self::apply_board_event(&mut tickets, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "decision-ticket replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            Self::apply_imported_event(&mut imported, &stored.event).map_err(|error| {
                crate::core::store::StoreError::Decode {
                    detail: format!(
                        "imported-item replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                }
            })?;
            Self::apply_imported_act_event(&mut acts, &imported, &stored.event).map_err(
                |error| crate::core::store::StoreError::Decode {
                    detail: format!(
                        "imported-act replay refused at sequence {}: {error}",
                        stored.sequence
                    ),
                },
            )?;
        }
        self.plan = plan;
        self.last_council = council;
        self.decision_tickets = tickets;
        self.imported_items = imported;
        self.imported_acts = acts;
        Ok(())
    }

    /// Append a message anchored to the durable event that introduced it.
    ///
    /// `sequence` is `None` only when no such event exists — see
    /// [`TranscriptEntry`]. Every caller that has just persisted an event
    /// passes that event's sequence, so the live transcript and one replayed
    /// from the store agree entry for entry.
    pub fn push_message(&mut self, sequence: Option<u64>, message: CanonicalMessage) {
        // `make_mut` clones only when a snapshot is still alive, so the steady
        // state is an in-place push. This is the one place the transcript is
        // ever copied, and only under contention.
        Arc::make_mut(&mut self.messages).push(TranscriptEntry { sequence, message });
    }

    #[must_use]
    pub fn messages(&self) -> &Arc<Vec<TranscriptEntry>> {
        &self.messages
    }

    /// The transcript as bare messages, for the paths that carry history
    /// somewhere sequences have no meaning — a provider request, a checkpoint,
    /// a token estimate.
    #[must_use]
    pub fn plain_messages(&self) -> Vec<CanonicalMessage> {
        self.messages
            .iter()
            .map(|entry| entry.message.clone())
            .collect()
    }

    /// Provider context projection. Compact mode never mutates durable history.
    #[must_use]
    pub fn provider_messages(&self) -> Vec<CanonicalMessage> {
        let Some(compact) = &self.compact_context else {
            return self.plain_messages();
        };
        let mut messages = compact.seed.clone();
        if let Some(tail) = self.messages.get(compact.history_floor..) {
            messages.extend(tail.iter().map(|entry| entry.message.clone()));
        }
        messages
    }

    pub fn enable_compact_context(&mut self, recent_turns: usize) -> bool {
        let Some(handoff) = &self.handoff else {
            return false;
        };
        let history_floor = recent_turn_floor(&self.messages, recent_turns);
        let mut seed = vec![CanonicalMessage::system(handoff.compact_seed())];
        if let Some(recent) = self.messages.get(history_floor..) {
            seed.extend(recent.iter().map(|entry| entry.message.clone()));
        }
        self.compact_context = Some(CompactContext {
            seed,
            history_floor: self.messages.len(),
        });
        true
    }

    pub fn disable_compact_context(&mut self) {
        self.compact_context = None;
    }

    #[must_use]
    /// Clone the current memory projection status, treating a poisoned lock
    /// as unknown rather than panicking inside a snapshot.
    pub fn memory_projection(&self) -> crate::runtime::memory::MemoryProjection {
        self.memory_projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn snapshot(&self, run_active: bool, recovery: RecoveryState) -> RuntimeSnapshot {
        RuntimeSnapshot {
            session: self.session,
            provider: self.provider.clone(),
            model: self.model.clone(),
            messages: Arc::clone(&self.messages),
            tree: Arc::clone(&self.tree),
            left_branch: self.left_branch.clone(),
            envelope: self.envelope.clone(),
            envelope_refusal: self.envelope_refusal.clone(),
            run_active,
            usage: self.usage,
            workspace_root: self.workspace_root.clone(),
            policy: self.policy,
            pending_approval: self.pending_approval.clone(),
            budget: self.budget,
            recovery,
            store_failure: self.store_failure.clone(),
            skills: Arc::new(Vec::new()),
            prompts: Arc::new(Vec::new()),
            extensions: Arc::new(Vec::new()),
            last_reload: self.last_reload.clone(),
            last_extension_load: self.last_extension_load.clone(),
            last_discovery: None,
            last_council: self.last_council.clone(),
            last_council_amendment: self.last_council_amendment.clone(),
            activated_skills: Arc::new(self.activated_skills.iter().cloned().collect()),
            context_diagnostics: Arc::new(Vec::new()),
            workspace_trusted: self.workspace_trusted,
            handoff: self.handoff.clone(),
            quota_reserve: self.quota_reserve.clone(),
            quota: self.quota.clone(),
            resume_advice: self.resume_advice.clone(),
            mcp_servers: Arc::new(Vec::new()),
            triggers: Arc::new(Vec::new()),
            route: self.route.clone(),
            breakers: Arc::new(
                self.breakers
                    .iter()
                    .map(|(provider, breaker)| BreakerView {
                        provider: provider.clone(),
                        state: breaker.state(),
                        consecutive_failures: breaker.consecutive_failures(),
                    })
                    .collect(),
            ),
            // Overwritten by `publish_snapshot`, which holds the registry.
            models: Arc::new(Vec::new()),
            // Overwritten by `publish_snapshot`, which owns live discovery.
            providers: Arc::new(Vec::new()),
            // Overwritten by `publish_snapshot`, which holds the route table.
            routes: Arc::new(Vec::new()),
            personas: Arc::new(Vec::new()),
            active_persona: None,
            souls: Arc::new(Vec::new()),
            sessions: Arc::clone(&self.sessions),
            plan: self.plan.clone(),
            repository: self.repository.clone(),
            changes: self.changes.clone(),
            read_evidence: Arc::new(self.read_evidence.values().cloned().collect()),
            review_threads: Arc::new(self.review_threads.values().cloned().collect()),
            memory: {
                // `None` counts until a query succeeds: unknown is reportable,
                // zero is a claim (AGENTS.md §1.3). A poisoned lock reads as
                // unknown rather than panicking inside a snapshot.
                let projection = self.memory_projection();
                Arc::new(crate::core::memory::MemorySummary {
                    rules_count: self.rules_snapshot.rules.len(),
                    user_profile_present: self.rules_snapshot.user_profile.is_some(),
                    facts_count: projection.counts.map(|counts| counts.facts),
                    episodes_count: projection.counts.map(|counts| counts.episodes),
                    projection_error: projection.error,
                    rules_error: self.rules_load_error.clone(),
                    rule_names: self
                        .rules_snapshot
                        .rules
                        .iter()
                        .map(|r| r.name.clone())
                        .collect(),
                })
            },
            // Overwritten by `publish_snapshot`, which holds the plugin catalog.
            plugins: Arc::new(Vec::new()),
            fleet: Arc::default(),
            preview: Arc::default(),
            external_agents: Vec::new(),
            external_agent_capability:
                crate::core::client::external_agent::ExternalAgentCapability {
                    available: false,
                    reason: Some("external-agent profiles not yet loaded".to_owned()),
                },
        }
    }
}

fn recent_turn_floor(messages: &[TranscriptEntry], recent_turns: usize) -> usize {
    if recent_turns == 0 {
        return messages.len();
    }
    let mut seen = 0_usize;
    for (index, message) in messages.iter().enumerate().rev() {
        if message.role == crate::core::message::Role::User {
            seen = seen.saturating_add(1);
            if seen == recent_turns {
                return index;
            }
        }
    }
    0
}

/// Accumulates one provider stream into a finished assistant message.
///
/// The two jobs that make this non-trivial, both driven by real provider
/// behaviour (`docs/provider-contract.md` §0):
///
/// 1. Text arrives as fragments and must be coalesced — persisting one event per
///    token is forbidden.
/// 2. Tool arguments arrive as partial JSON keyed by call id, are **not
///    contiguous**, and may only be parsed at the provider's completion
///    boundary.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    text: String,
    /// Keyed by provider call id. A `Vec` of pairs rather than a map: calls are
    /// few, order matters for the final message, and this keeps arrival order
    /// without a second index.
    pending_arguments: Vec<(String, String)>,
    completed_calls: Vec<ToolCall>,
    usage: Option<Usage>,
    unknown_upstream: Vec<String>,
}

impl StreamAccumulator {
    pub fn push_text(&mut self, fragment: &str) {
        self.text.push_str(fragment);
    }

    /// Record that a tool call started. Arguments follow as fragments.
    pub fn start_tool_call(&mut self, id: String) {
        if !self.pending_arguments.iter().any(|(key, _)| key == &id) {
            self.pending_arguments.push((id, String::new()));
        }
    }

    /// Append an argument fragment to the call it belongs to.
    ///
    /// Keyed by `id` precisely because fragments from different calls may
    /// interleave. Appending to "the current call" would corrupt both.
    pub fn push_arguments(&mut self, id: &str, fragment: &str) {
        if let Some((_, buffer)) = self.pending_arguments.iter_mut().find(|(key, _)| key == id) {
            buffer.push_str(fragment);
        } else {
            // A fragment for a call we never saw start. Tolerated rather than
            // dropped: some providers may not announce a start.
            self.pending_arguments
                .push((id.to_owned(), fragment.to_owned()));
        }
    }

    /// Accept a fully-parsed tool call from the adapter.
    ///
    /// The adapter owns the parse because only it knows its provider's
    /// completion boundary.
    pub fn complete_tool_call(&mut self, call: ToolCall) {
        self.pending_arguments.retain(|(key, _)| key != &call.id);
        self.completed_calls.push(call);
    }

    pub fn set_usage(&mut self, usage: Usage) {
        self.usage = Some(usage);
    }

    pub fn note_unknown(&mut self, kind: String) {
        self.unknown_upstream.push(kind);
    }

    #[must_use]
    pub fn usage(&self) -> Option<Usage> {
        self.usage
    }

    /// Tool calls whose arguments never reached a completion boundary.
    ///
    /// Non-empty means the stream ended mid-tool-call. The accumulated text is
    /// **not** valid JSON and must never be parsed as a rescue attempt.
    #[must_use]
    pub fn unterminated_calls(&self) -> usize {
        self.pending_arguments.len()
    }

    #[must_use]
    pub fn unknown_upstream(&self) -> &[String] {
        &self.unknown_upstream
    }

    /// Build the final assistant message, or `None` if the stream produced
    /// nothing worth recording.
    #[must_use]
    pub fn finish(self, provider: ProviderId, model: ModelId) -> Option<CanonicalMessage> {
        let mut blocks = Vec::new();

        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text { text: self.text });
        }
        for call in self.completed_calls {
            blocks.push(ContentBlock::ToolCall(call));
        }

        if blocks.is_empty() {
            return None;
        }

        Some(CanonicalMessage::assistant(blocks, provider, model))
    }
}

// AGENTS.md §7: tests may panic freely — a panicking assertion is a failing
// test, not a corrupted terminal. `clippy.toml` covers unwrap/expect/panic in
// tests but has no equivalent for indexing, so it is stated per module.
#[allow(clippy::indexing_slicing)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_fragments_coalesce_into_one_block() {
        let mut accumulator = StreamAccumulator::default();
        accumulator.push_text("Hel");
        accumulator.push_text("lo");

        let message = accumulator
            .finish(ProviderId::new("fake"), ModelId::new("fake-1"))
            .expect("message");

        assert_eq!(
            message.blocks.len(),
            1,
            "fragments must coalesce, not stack"
        );
        assert_eq!(message.text(), "Hello");
    }

    #[test]
    fn interleaved_tool_arguments_do_not_corrupt_each_other() {
        // The failure this guards: appending to "the current call" instead of
        // keying by id silently merges two tool calls into nonsense.
        let mut accumulator = StreamAccumulator::default();
        accumulator.start_tool_call("call_a".to_owned());
        accumulator.start_tool_call("call_b".to_owned());

        accumulator.push_arguments("call_a", "{\"x\":");
        accumulator.push_arguments("call_b", "{\"y\":");
        accumulator.push_arguments("call_a", " 1}");
        accumulator.push_arguments("call_b", " 2}");

        assert_eq!(accumulator.unterminated_calls(), 2);

        accumulator.complete_tool_call(ToolCall {
            id: "call_a".to_owned(),
            name: "t".to_owned(),
            arguments: serde_json::json!({ "x": 1 }),
            provider_signature: None,
        });
        assert_eq!(accumulator.unterminated_calls(), 1);

        accumulator.complete_tool_call(ToolCall {
            id: "call_b".to_owned(),
            name: "t".to_owned(),
            arguments: serde_json::json!({ "y": 2 }),
            provider_signature: None,
        });
        assert_eq!(accumulator.unterminated_calls(), 0);

        let message = accumulator
            .finish(ProviderId::new("fake"), ModelId::new("fake-1"))
            .expect("message");
        let calls: Vec<_> = message.tool_calls().collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["x"], 1);
        assert_eq!(calls[1].arguments["y"], 2);
    }

    #[test]
    fn a_stream_that_ends_mid_tool_call_reports_it() {
        let mut accumulator = StreamAccumulator::default();
        accumulator.start_tool_call("call_a".to_owned());
        accumulator.push_arguments("call_a", "{\"pa");

        assert_eq!(
            accumulator.unterminated_calls(),
            1,
            "an unterminated call must be visible, not silently dropped"
        );
    }

    #[test]
    fn an_empty_stream_produces_no_message() {
        let accumulator = StreamAccumulator::default();
        assert!(
            accumulator
                .finish(ProviderId::new("fake"), ModelId::new("fake-1"))
                .is_none()
        );
    }

    #[test]
    fn snapshots_share_the_transcript_rather_than_copying_it() {
        let mut state = SessionState::default();
        state.push_message(Some(0), CanonicalMessage::user("hello"));

        let snapshot = state.snapshot(false, RecoveryState::Clean);
        assert!(
            Arc::ptr_eq(&snapshot.messages, state.messages()),
            "a snapshot must share the transcript; copying it per frame is O(n^2) over a session"
        );
    }

    #[test]
    fn corrupt_plan_history_refuses_rebuild_without_partial_state() {
        let session = SessionId::new();
        let plan_id = crate::core::plan::PlanId::new();
        let stored = crate::core::event::StoredEvent {
            id: crate::core::event::EventId::new(),
            sequence: 1,
            occurred_at: time::OffsetDateTime::now_utc(),
            event: crate::core::event::SmedEvent::PlanQuestionAnswered {
                session,
                plan_id,
                answer: crate::core::plan::QuestionAnswer {
                    question_id: crate::core::plan::QuestionId::new(),
                    selected_options: vec!["A".to_string()],
                    freeform_text: None,
                    answered_at: time::OffsetDateTime::now_utc(),
                },
            },
        };
        let mut state = SessionState::default();

        let error = state
            .rebuild_messages_from(&[stored])
            .expect_err("out-of-order history must refuse");

        assert!(matches!(
            error,
            crate::core::store::StoreError::Decode { .. }
        ));
        assert!(
            state.plan.is_none(),
            "failed replay must not publish a partial workflow"
        );
    }

    // -----------------------------------------------------------------------
    // Imported act folding (phase D6, step 5)
    // -----------------------------------------------------------------------

    fn act_fixture() -> crate::core::imported::ImportedAct {
        crate::core::imported::ImportedAct {
            act_id: crate::core::imported::ImportedActId::new(),
            item_id: crate::core::imported::ImportedItemId::from_uuid(uuid::Uuid::from_u128(
                0x019a_0000_0000_7000_8000_0000_0000_0042,
            )),
            kind: crate::core::imported::ImportedActKind::PullRequest,
            expected_revision: "rev1".to_owned(),
            head_branch: "feat/harness".to_owned(),
            base_branch: "main".to_owned(),
            outcome: crate::core::imported::ImportedActOutcome::Submitted {
                remote_url: "https://example.invalid/7".to_owned(),
            },
        }
    }

    fn item_fixture() -> crate::core::imported::ImportedItem {
        crate::core::imported::ImportedItem {
            id: crate::core::imported::ImportedItemId::from_uuid(uuid::Uuid::from_u128(
                0x019a_0000_0000_7000_8000_0000_0000_0042,
            )),
            integration: "github".to_owned(),
            remote_id: "42".to_owned(),
            source_url: "https://example.invalid/42".to_owned(),
            fetched_revision: "rev1".to_owned(),
            title: "an imported task".to_owned(),
            state: crate::core::imported::ImportedItemState::Open,
            blocked_by: Vec::new(),
        }
    }

    fn stored(
        event: crate::core::event::SmedEvent,
        sequence: u64,
    ) -> crate::core::event::StoredEvent {
        crate::core::event::StoredEvent {
            id: crate::core::event::EventId::new(),
            sequence,
            occurred_at: time::OffsetDateTime::now_utc(),
            event,
        }
    }

    #[test]
    fn an_act_event_folds_into_state_and_survives_replay() {
        let session = SessionId::new();
        let item = item_fixture();
        let act = act_fixture();

        let mut state = SessionState::default();
        state
            .apply_event(&crate::core::event::SmedEvent::ImportedItemFetched { session, item })
            .expect("the item fetch folds");
        state
            .apply_event(&crate::core::event::SmedEvent::ImportedActRecorded {
                session,
                act: act.clone(),
            })
            .expect("the act folds onto the item it names");
        assert_eq!(
            state.imported_acts.len(),
            1,
            "the fold records the act in session state"
        );
        assert_eq!(state.imported_acts[&act.act_id], act);

        let mut replayed = SessionState::default();
        replayed
            .rebuild_durable_records_from(&[
                stored(
                    crate::core::event::SmedEvent::ImportedItemFetched {
                        session,
                        item: item_fixture(),
                    },
                    1,
                ),
                stored(
                    crate::core::event::SmedEvent::ImportedActRecorded { session, act },
                    2,
                ),
            ])
            .expect("the durable branch replays cleanly");
        assert_eq!(
            replayed.imported_acts.len(),
            1,
            "replay restores the act the live path folded"
        );
    }

    #[test]
    fn an_act_over_an_item_this_session_never_imported_is_refused() {
        let session = SessionId::new();
        let mut state = SessionState::default();
        state
            .apply_event(&crate::core::event::SmedEvent::ImportedItemFetched {
                session,
                item: item_fixture(),
            })
            .expect("the item fetch folds");

        let mut stranger = act_fixture();
        stranger.item_id = crate::core::imported::ImportedItemId::new();
        let error = state
            .validate_event(&crate::core::event::SmedEvent::ImportedActRecorded {
                session,
                act: stranger,
            })
            .expect_err("an act over a stranger must refuse");
        assert!(
            matches!(
                error,
                crate::core::error::SmedError::WorkspaceRefused {
                    code: crate::core::error::ReasonCode::SchemaInvalid,
                    ..
                }
            ),
            "an unrecorded act is a shape bug, not a retryable stale pin"
        );
        assert!(
            state.imported_acts.is_empty(),
            "a refused act must not leave a partial record behind"
        );
    }

    #[test]
    fn a_duplicate_act_id_is_refused_like_a_duplicate_item() {
        let session = SessionId::new();
        let mut state = SessionState::default();
        state
            .apply_event(&crate::core::event::SmedEvent::ImportedItemFetched {
                session,
                item: item_fixture(),
            })
            .expect("the item fetch folds");
        let act = act_fixture();
        state
            .apply_event(&crate::core::event::SmedEvent::ImportedActRecorded {
                session,
                act: act.clone(),
            })
            .expect("first record folds");

        let error = state
            .validate_event(&crate::core::event::SmedEvent::ImportedActRecorded { session, act })
            .expect_err("the log is append-only; a repeat must refuse");
        assert!(
            matches!(
                error,
                crate::core::error::SmedError::WorkspaceRefused {
                    code: crate::core::error::ReasonCode::SchemaInvalid,
                    ..
                }
            ),
            "a duplicate act id is a bug, not an update"
        );
        assert_eq!(state.imported_acts.len(), 1);
    }
}

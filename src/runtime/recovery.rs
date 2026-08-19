//! Rebuilding a session from durable history.
//!
//! ```text
//! latest checkpoint ──┐
//!                     ├──▶ project() ──▶ SessionState + RecoveryState
//! events from there ──┘
//! ```
//!
//! # Why this is a pure function
//!
//! Recovery is the code most likely to be wrong and least likely to be exercised
//! in normal use. Every crash case in 's checklist is a question
//! about a specific event sequence, so this takes events in and returns state
//! out — no store, no actor, no clock. The crash matrix is then a table test
//! rather than a choreography of processes.
//!
//! # The checkpoint is an optimisation, never the truth
//!
//! State is always `checkpoint + every event after it`. A mutation completed
//! after the last checkpoint is recovered from its `ToolCompleted` event, which
//! is what  means by "a completed mutation after the latest
//! checkpoint must be recoverable from events rather than lost". A checkpoint
//! that were treated as the whole truth would silently drop it.

use std::sync::Arc;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::command::{ApprovalDecision, ApprovalId};
use crate::core::error::ToolError;
use crate::core::event::{RunId, SmedEvent, StoredEvent};
use crate::core::message::{ToolCall, ToolEffect, ToolResult};
use crate::core::recovery::{Authority, InterruptedKind, InterruptedWork, RecoveryState};
use crate::core::store::SessionStatus;
use crate::core::tool::{ReadSet, ToolTier};
use crate::runtime::session::SessionState;

/// A session rebuilt from durable history.
#[derive(Debug)]
pub struct Recovered {
    pub state: SessionState,
    pub recovery: RecoveryState,
    pub status: SessionStatus,
}

/// Rebuild a session.
///
/// `events` must start at the checkpoint's `sequence` (or 0 when there is no
/// checkpoint) and be contiguous — the store guarantees both, refusing a history
/// with a gap rather than handing one over.
///
/// `covered_message_sequences` re-anchors the checkpoint's own transcript to the
/// events that produced it , so a resumed session offers the
/// same branch points a live one does. It is the store's answer for the same
/// branch these `events` came from; see [`BranchResume`](crate::core::store::BranchResume).
///
/// # Errors
/// Only if the rebuilt read set cannot be written, which means a poisoned lock
/// on a set this function just created — i.e. never in practice.
pub fn project(
    checkpoint: Option<SessionCheckpoint>,
    covered_message_sequences: &[u64],
    events: &[StoredEvent],
) -> Result<Recovered, ToolError> {
    let (mut state, mut status) = base_state(checkpoint, covered_message_sequences)?;
    let mut tracker = Tracker::default();

    for stored in events {
        apply(&mut state, &mut tracker, stored, &mut status)?;
    }

    // The same rule a fork or a clone applies: a session that comes back must
    // not come back with authority nobody re-armed.
    state.policy = state.policy.carried_forward();

    Ok(Recovered {
        recovery: tracker.finish(),
        state,
        status,
    })
}

/// The state a checkpoint restores, before events are replayed onto it.
///
/// Note what is *not* restored: `exact_commands`. The checkpoint has no field
/// for it (`core::checkpoint`), and this function does not rebuild it from
/// `ApprovalResolved` events either. Both halves are needed — a projection that
/// helpfully re-derived grants from history would defeat the type that refuses
/// to store them (`docs/persistence.md` §6).
fn base_state(
    checkpoint: Option<SessionCheckpoint>,
    covered_message_sequences: &[u64],
) -> Result<(SessionState, SessionStatus), ToolError> {
    let mut state = SessionState::default();
    let Some(checkpoint) = checkpoint else {
        return Ok((state, SessionStatus::Active));
    };

    let status = checkpoint.status;

    state.session = Some(checkpoint.session);
    state.workspace_root = checkpoint.project_root;
    state.provider = checkpoint.provider;
    state.model = checkpoint.model;
    state.usage = checkpoint.usage;
    state.policy = checkpoint.policy;
    state.budget = checkpoint.budget;
    state.read_set = Arc::new(ReadSet::restore(checkpoint.read_set)?);
    // Restored before the replay below, which then overwrites any entry whose
    // event is *also* replayed. Both paths write the same value, so the order
    // only matters for reads the checkpoint covers and the replay does not —
    // which is the case this field is on the checkpoint for.
    state.read_evidence = checkpoint
        .read_evidence
        .into_iter()
        .map(|record| (record.path.clone(), record))
        .collect();
    // Same arrangement as the read evidence above: restored from the
    // checkpoint, then any `Review*` event the replay still covers is folded in
    // on top by the one reducer both paths use.
    state.review_threads = checkpoint
        .review_threads
        .into_iter()
        .map(|thread| (thread.id, thread))
        .collect();
    state.last_mutation_sequence = checkpoint.last_mutation_sequence;
    state.successful_command_evidence = checkpoint.successful_command_evidence;
    state.activated_skills = checkpoint.activated_skills.into_iter().collect();
    state.workspace_trusted = checkpoint.workspace_trusted;
    state.handoff = checkpoint.handoff;
    state.quota_reserve = checkpoint.quota_reserve;
    state.route = checkpoint.route;

    // Re-anchor the checkpoint's transcript to the events that produced it, so
    // a resumed session offers the same branch points a live one does.
    //
    // The anchors are applied only when there is exactly one per message. A
    // mismatch means this build and the one that wrote the checkpoint disagree
    // about which events yield messages, and a transcript anchored off-by-one
    // would point `/tree` at the wrong branch point — a silently wrong rewind
    // is worse than an unavailable one, so every entry goes unanchored instead
    // (`AGENTS.md` §1.3).
    let anchors: &[u64] = if covered_message_sequences.len() == checkpoint.messages.len() {
        covered_message_sequences
    } else {
        &[]
    };
    for (index, message) in checkpoint.messages.into_iter().enumerate() {
        state.push_message(anchors.get(index).copied(), message);
    }

    Ok((state, status))
}

#[allow(
    clippy::too_many_lines,
    reason = "one flat replay match;  added the route-selection and route-advance arms alongside the existing projections"
)]
fn apply(
    state: &mut SessionState,
    tracker: &mut Tracker,
    stored: &StoredEvent,
    status: &mut SessionStatus,
) -> Result<(), ToolError> {
    if is_projection_noop(&stored.event) {
        return Ok(());
    }
    match &stored.event {
        SmedEvent::SessionCreated {
            session,
            provider,
            model,
        } => {
            state.session = Some(*session);
            state.provider = Some(provider.clone());
            state.model = Some(model.clone());
        }
        SmedEvent::ModelChanged {
            provider, model, ..
        } => {
            state.provider = Some(provider.clone());
            state.model = Some(model.clone());
        }
        SmedEvent::MessageAppended { message, .. } => {
            state.push_message(Some(stored.sequence), (**message).clone());
        }
        SmedEvent::UsageReported { usage, .. } => {
            // The same rule the live path applies as it emits the event.
            state.usage.input_tokens += usage.input_tokens;
            state.usage.output_tokens += usage.output_tokens;
        }
        SmedEvent::PolicyChanged { mode, .. } => state.policy = *mode,
        SmedEvent::QuotaBoundaryReached { reserve, .. } => {
            state.quota_reserve = reserve.clone();
        }
        SmedEvent::HandoffCreated { handoff, .. } => {
            state.handoff = Some((**handoff).clone());
        }
        SmedEvent::RunStarted { run, .. } => tracker.open_run(*run),
        SmedEvent::ToolProposed {
            run,
            approval,
            call,
            tier,
            preview,
            ..
        } => tracker.propose(*run, approval.as_ref(), call, *tier, preview),
        SmedEvent::ApprovalResolved {
            run,
            approval,
            decision,
            ..
        } => tracker.resolve_approval(*run, *approval, *decision),
        SmedEvent::ToolCompleted {
            run,
            call_id,
            name,
            result,
            ..
        } => {
            tracker.settle(*run);
            observe_result(state, stored, result)?;
            let mut restored = result.clone();
            restored.evidence_event_id = Some(stored.id.to_string());
            state.push_message(
                Some(stored.sequence),
                crate::core::message::CanonicalMessage::tool_result(call_id, name, restored),
            );
        }
        SmedEvent::ToolFailed {
            run,
            call_id,
            name,
            code,
            detail,
            ..
        } => {
            tracker.settle(*run);
            state.push_message(
                Some(stored.sequence),
                crate::core::message::CanonicalMessage::tool_result(
                    call_id,
                    name,
                    ToolResult::failed(*code, detail.clone()),
                ),
            );
        }
        // Three ways a run stops being in flight: it ended, it failed, or a human
        // resolved what the crash left of it. All three mean the same thing to
        // the tracker — there is no interrupted work here any more.
        SmedEvent::RunFinished { run, .. } | SmedEvent::RunFailed { run, .. } => {
            tracker.close_run(*run);
        }
        SmedEvent::RecoveryResolved { decision, .. } => {
            tracker.clear();
            if *decision == crate::core::recovery::RecoveryDecision::EndSession {
                *status = SessionStatus::Ended;
            }
        }
        SmedEvent::SessionEnded { .. } => {
            *status = SessionStatus::Ended;
            tracker.clear();
        }
        // A resumed session keeps its route position  — the
        // same replay `ModelChanged` gets, since a selection or advance is
        // itself a provider/model change with a route name attached.
        SmedEvent::RouteSelected {
            child: None,
            route,
            position,
            provider,
            model,
            ..
        } => {
            state.route = Some(crate::core::routing::RouteRuntime {
                route: route.clone(),
                position: *position,
            });
            state.provider = Some(provider.clone());
            state.model = Some(model.clone());
        }
        SmedEvent::RouteAdvanced {
            route,
            to_position,
            provider,
            model,
            ..
        } => {
            state.route = Some(crate::core::routing::RouteRuntime {
                route: route.clone(),
                position: *to_position,
            });
            state.provider = Some(provider.clone());
            state.model = Some(model.clone());
        }
        // Review threads replay through the same reducer the live path uses, so
        // a resumed session and a live one cannot hold different notes (plan
        // §Phase D3).
        event @ (SmedEvent::ReviewNoteRecorded { .. }
        | SmedEvent::ReviewCommentAdded { .. }
        | SmedEvent::ReviewRequestSent { .. }
        | SmedEvent::ReviewRequestAnswered { .. }) => {
            super::review::apply_event(&mut state.review_threads, event);
        }
        // Every remaining variant is a projection no-op, filtered by
        // `is_projection_noop` above.
        _ => {}
    }
    Ok(())
}

/// Events that change no projected state:
///
/// - `BudgetExhausted` is always followed by its own terminal run event.
/// - `RecoveryRequired` is narration for the transcript; the recovery state
///   is derived from the tool and run events, not from this marker.
/// - `TextDelta` has no persisted form at all (the wire format has no variant
///   for one), so reaching here means a caller broadcast a delta into a
///   projection. Ignoring it is right: the coalesced message carries the
///   text.
/// - The subagent boundaries narrate the parent's transcript; the child is
///   its own session with its own projection, so nothing here rebuilds from
///   them.
/// - The trigger boundaries  narrate a trigger's *control*
///   session, a different session from the one being replayed here.
const fn is_projection_noop(event: &SmedEvent) -> bool {
    matches!(
        event,
        SmedEvent::BudgetExhausted { .. }
            | SmedEvent::ModelChangeRefused { .. }
            | SmedEvent::FileSaved { .. }
            | SmedEvent::RecoveryRequired { .. }
            | SmedEvent::TextDelta { .. }
            | SmedEvent::ReasoningDelta { .. }
            | SmedEvent::ToolAssembling { .. }
            | SmedEvent::QuotaReported { .. }
            | SmedEvent::SubagentSpawned { .. }
            | SmedEvent::SubagentResultLate { .. }
            | SmedEvent::ReadSetCollision { .. }
            | SmedEvent::SubagentActivity { .. }
            | SmedEvent::TriggerFired { .. }
            | SmedEvent::TriggerSettled { .. }
            | SmedEvent::TriggerSkipped { .. }
            | SmedEvent::TriggerQueued { .. }
            | SmedEvent::TriggerReplaced { .. }
            | SmedEvent::TriggerDisabled { .. }
            | SmedEvent::TriggerRearmed { .. }
            // A child's route selection narrates the parent's transcript,
            // exactly as `SubagentSpawned` does; the child is its own
            // session with its own projection.
            | SmedEvent::RouteSelected { child: Some(_), .. }
            | SmedEvent::RouteExhausted { .. }
            | SmedEvent::BreakerStateChanged { .. }
    )
}

/// Fold a completed tool result into state, exactly as the live path does.
///
/// The read set is rebuilt here because the tools update it through a shared
/// `Arc` at execution time, which a replay obviously cannot repeat.
fn observe_result(
    state: &mut SessionState,
    stored: &StoredEvent,
    result: &ToolResult,
) -> Result<(), ToolError> {
    if !result.outcome.is_ok() {
        return Ok(());
    }

    match &result.effect {
        ToolEffect::Read { path, sha256 } => {
            observe_path(state, path, sha256)?;
            // Replayed from the same field the live path uses, so a resumed
            // session cites the same event id it cited before the restart
            // rather than losing the evidence to the restart.
            state.read_evidence.insert(
                path.clone(),
                crate::core::change_capture::ReadRecord {
                    path: path.clone(),
                    sha256: sha256.clone(),
                    tool_event_id: stored.id.to_string(),
                },
            );
        }
        ToolEffect::Mutation { path, sha256 } => {
            observe_path(state, path, sha256)?;
            state.last_mutation_sequence = Some(stored.sequence);
        }
        ToolEffect::Command { success: true, .. } => {
            state
                .successful_command_evidence
                .insert(stored.id.to_string(), stored.sequence);
        }
        ToolEffect::SkillActivated { name, project } => {
            state.activated_skills.insert(name.clone());
            state.workspace_trusted |= *project;
        }
        ToolEffect::None
        | ToolEffect::Command { success: false, .. }
        | ToolEffect::Completion { .. } => {}
    }
    Ok(())
}

/// Re-key a stored effect path back onto the read set.
///
/// `ToolEffect` carries the **display** path — workspace-relative, produced by
/// `tools::files::display_path` — while the read set is keyed by the absolute
/// canonical path. Joining onto the root recovers the original in both cases:
/// a relative path joins normally, and an absolute one (which `display_path`
/// emits when `strip_prefix` fails) replaces the root entirely, which is
/// `Path::join`'s documented behaviour.
///
/// Without a root there is nothing to join onto, and a relative key would be a
/// read set entry no tool could ever match. Skipping is the fail-closed answer:
/// the next edit refuses with `FILE_NOT_OBSERVED` and the model reads again.
fn observe_path(state: &SessionState, path: &str, sha256: &str) -> Result<(), ToolError> {
    let Some(root) = state.workspace_root.as_ref() else {
        return Ok(());
    };
    state.read_set.observe(root.join(path), sha256.to_owned())
}

/// Follows one run's in-flight work while events are replayed.
#[derive(Debug, Default)]
struct Tracker {
    open: Option<OpenRun>,
}

#[derive(Debug)]
struct OpenRun {
    run: RunId,
    tool: Option<PendingWork>,
}

#[derive(Debug)]
enum PendingWork {
    /// Proposed and gated. The gate never opened.
    AwaitingApproval {
        approval: ApprovalId,
        call: ToolCall,
        tier: ToolTier,
        preview: String,
    },
    /// Authorised and handed to a tool task.
    Started {
        authority: Authority,
        call: ToolCall,
        tier: ToolTier,
        preview: String,
    },
}

impl Tracker {
    fn open_run(&mut self, run: RunId) {
        self.open = Some(OpenRun { run, tool: None });
    }

    fn close_run(&mut self, run: RunId) {
        if self.open.as_ref().is_some_and(|open| open.run == run) {
            self.open = None;
        }
    }

    fn clear(&mut self) {
        self.open = None;
    }

    /// A tool reached a terminal outcome; the run itself may continue.
    fn settle(&mut self, run: RunId) {
        if let Some(open) = self.open.as_mut().filter(|open| open.run == run) {
            open.tool = None;
        }
    }

    fn propose(
        &mut self,
        run: RunId,
        approval: Option<&ApprovalId>,
        call: &ToolCall,
        tier: ToolTier,
        preview: &str,
    ) {
        // A proposal outside its open run cannot happen through the runtime.
        // History is data, though, so ignore the mismatched proposal and retain
        // the original open run as interrupted rather than attaching work to the
        // wrong authority.
        let Some(open) = self.open.as_mut().filter(|open| open.run == run) else {
            return;
        };

        open.tool = Some(match approval {
            // Gated on a human. Nothing has started.
            Some(approval) => PendingWork::AwaitingApproval {
                approval: *approval,
                call: call.clone(),
                tier,
                preview: preview.to_owned(),
            },
            // No gate: the policy already authorised this tier, so the tool
            // starts immediately. From here the effect is uncertain.
            None => PendingWork::Started {
                authority: Authority::Policy,
                call: call.clone(),
                tier,
                preview: preview.to_owned(),
            },
        });
    }

    fn resolve_approval(&mut self, run: RunId, approval: ApprovalId, decision: ApprovalDecision) {
        let Some(open) = self.open.as_mut().filter(|open| open.run == run) else {
            return;
        };
        let Some(PendingWork::AwaitingApproval {
            approval: pending, ..
        }) = open.tool.as_ref()
        else {
            return;
        };
        if *pending != approval {
            return;
        }
        let Some(PendingWork::AwaitingApproval {
            approval: _,
            call,
            tier,
            preview,
        }) = open.tool.take()
        else {
            return;
        };

        open.tool = match decision {
            // Denied work never runs; the runtime records a refusal instead.
            ApprovalDecision::Deny => None,
            ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveExactForSession => {
                Some(PendingWork::Started {
                    authority: Authority::Approval(approval),
                    call,
                    tier,
                    preview,
                })
            }
            ApprovalDecision::AutoByPolicy => Some(PendingWork::Started {
                authority: Authority::Policy,
                call,
                tier,
                preview,
            }),
        };
    }

    fn finish(self) -> RecoveryState {
        let Some(open) = self.open else {
            return RecoveryState::Clean;
        };

        let kind = match open.tool {
            // A run with no tool in flight died during a provider call.
            None => InterruptedKind::ProviderTurnInterrupted,
            Some(PendingWork::AwaitingApproval {
                approval: _,
                call,
                tier,
                preview,
            }) => InterruptedKind::ProposalUnapproved {
                call,
                tier,
                preview,
            },
            Some(PendingWork::Started {
                authority,
                call,
                tier,
                preview,
            }) => InterruptedKind::EffectUncertain {
                authority,
                call,
                tier,
                preview,
            },
        };

        RecoveryState::Required(InterruptedWork {
            run: open.run,
            kind,
        })
    }
}
